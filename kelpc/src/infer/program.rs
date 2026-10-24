use super::*;

mod program_types;
pub(crate) use program_types::*;

mod program_sigs;
pub(crate) use program_sigs::*;

mod program_bodies;
pub(crate) use program_bodies::*;

/// The passdown borrow modes (roadmap item 17). Every `pd var` used to be
/// handed to every referencing function as an exclusive borrow, so a function
/// that takes the passdown as an argument *and* reads it - `fun show(s: &str)
/// { print(cfg) }` called as `show(cfg)` - was refused with a borrow
/// conflict, and a read-only `pd var` was as good as unusable.
///
/// The upgrade set is discovered rather than guessed at. A first *probe* pass
/// runs with every passdown shared: a use that needs an exclusive borrow is
/// recorded instead of refused, and every call site records an edge. The probe
/// is a real body check whose `tir` is thrown away - which is what lets the
/// answer come from the checker itself, including generic instances,
/// overloads and `mut` receivers, instead of from a re-implementation of
/// resolution that could disagree with it. `close_pd_modes` then walks the
/// edges to a fixpoint, and the pass that emits the program runs again with
/// the result. A program with no passdown writes at all keeps the probe's
/// result and is compiled once.
pub fn check_program(
    program: &ast::Program,
    project: &Project,
) -> std::result::Result<tir::Program, Diagnostic> {
    // A-13: the whole program's export set is known before any body is
    // checked, so the private-in-public rule runs here, once.
    check_pub_signatures(program, project)?;
    check_extern_type_names(program)?;
    let inferred = infer_receiver_modes(program, project)?;
    let program = inferred.as_ref().unwrap_or(program);
    let probe = std::rc::Rc::new(std::cell::RefCell::new(PdProbe::default()));
    let probe_wiring = PdWiring {
        modes: std::rc::Rc::new(HashMap::new()),
        probe: Some(probe.clone()),
        writers: None,
    };
    let checked = check_program_pass(program, project, &probe_wiring)?;
    let exclusive_uses = probe.borrow().exclusive_uses.clone();
    if exclusive_uses.is_empty() {
        return Ok(checked);
    }
    let edges = probe.borrow().edges.clone();
    let final_wiring = PdWiring {
        modes: std::rc::Rc::new(close_pd_modes(&exclusive_uses, &edges)),
        probe: None,
        writers: None,
    };
    Ok(check_program_pass(program, project, &final_wiring)?)
}


/// The receiver modes of the methods written `fun Type.name` without `mut`
/// (issue #286). Such a method is exclusive exactly when its body writes self,
/// including through a call to another exclusive method on self. The set is
/// found by a fixpoint from the empty set: each pass checks every inferable
/// body with `self` exclusive and records the ones that write, and the set
/// grows until a pass finds the same writers. Returns the program with those
/// receivers written `mut`, or `None` when no method is exclusive.
pub(crate) fn infer_receiver_modes(program: &ast::Program, project: &Project) -> std::result::Result<Option<ast::Program>, Diagnostic> {
    if !program.functions.iter().any(|f| f.receiver.as_ref().is_some_and(|r| r.inferable)) {
        return Ok(None);
    }
    let mut exclusive: std::collections::HashSet<crate::lexer::Span> = std::collections::HashSet::new();
    let mut owned: Option<ast::Program> = None;
    loop {
        let writers = std::rc::Rc::new(std::cell::RefCell::new(std::collections::HashSet::new()));
        let wiring = PdWiring {
            modes: std::rc::Rc::new(HashMap::new()),
            probe: Some(std::rc::Rc::new(std::cell::RefCell::new(PdProbe::default()))),
            writers: Some(writers.clone()),
        };
        let current: &ast::Program = owned.as_ref().unwrap_or(program);
        check_program_pass(current, project, &wiring)?;
        let found = writers.borrow().clone();
        if found == exclusive {
            return Ok(owned);
        }
        let mut next = program.clone();
        for f in next.functions.iter_mut() {
            if let Some(r) = f.receiver.as_mut() {
                if r.inferable && found.contains(&f.span) {
                    r.mutable = true;
                }
            }
        }
        exclusive = found;
        owned = Some(next);
    }
}

/// One full body-checking pass, under the passdown borrow modes `wiring`
/// carries. Called twice at most: once as the probe, and once with the modes
/// the probe's findings produced.
pub(crate) fn check_program_pass(
    program: &ast::Program,
    project: &Project,
    wiring: &PdWiring,
) -> std::result::Result<tir::Program, Diagnostic> {
    let decls = declare_types(program, project)?;
    let (decls, signatures) = declare_signatures(program, project, decls)?;
    finish_program(program, project, wiring, decls, signatures)
}

/// What the declaration phase hands the signature phase: the program's type tables,
/// the trait and impl facts, and the environment every later phase reads.
pub(crate) struct TypeDecls {
    pub(crate) declared_keys: std::rc::Rc<std::collections::HashSet<String>>,
    pub(crate) declared_drops: std::rc::Rc<std::collections::HashSet<String>>,
    pub(crate) traits: std::rc::Rc<HashMap<String, TraitInfo>>,
    pub(crate) trait_impls: Vec<TraitImplInfo>,
    pub(crate) env: TypeEnv,
    pub(crate) fns: FuncEnv,
    pub(crate) structs: HashMap<String, StructDef>,
    pub(crate) struct_order: Vec<String>,
    pub(crate) const_fns: std::rc::Rc<ConstFns>,
    pub(crate) consts: HashMap<String, ConstInfo>,
    pub(crate) enums: HashMap<String, EnumDef>,
    pub(crate) enum_order: Vec<String>,
}

/// The function signatures and overload sets the signature phase builds.
pub(crate) struct Signatures {
    pub(crate) sigs: HashMap<String, FnSig>,
    pub(crate) overloads: HashMap<String, Vec<(Vec<Ty>, String)>>,
    pub(crate) decl_keys: Vec<String>,
}

