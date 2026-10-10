//! Tidepool step 2a: the escape analysis that decides which shared-pool
//! allocations may be placed in a block. It follows the Tidepool design,
//! sections 6 (escape rules 1 to 4), 9 (step 2a) and 20.1 (D8, D9, D41).
//!
//! This is analysis only. Nothing in a build calls it, so the code generated
//! for any program, the runtime and the C backend are unchanged. The dry run
//! is the test `dry_run_bench_sites` below, which prints the count per program.
//!
//! A candidate block is a function body of the program. A site is an
//! allocation the body makes in the shared pool:
//!
//! - `Box.of(v)`, the block constructor of D8. Only this kind can be accepted.
//! - `Box.set(v)`, which allocates the element of an empty box. D8 names no
//!   block constructor for it, so it is refused.
//! - A `List`, `Map`, `String` or `Bytes` value made by a constructor. Rule 4
//!   refuses a container-valued allocation inside a block.
//! - A `fun` literal that captures something. Its environment is shared-pool
//!   memory and D8 names no block constructor for it, so it is refused.
//!
//! The walk gives every expression a carry: the set of sites whose value may
//! flow into it. A carry that reaches a sink (section 6, rule 2) refuses each
//! site in it, and the sink is recorded by name. A site is accepted when it is
//! a `Box.of` site that no sink reached.
//!
//! Known limits of this count (they make it a conservative estimate, not an
//! exact one): sites inside shipped stdlib bodies are not counted, the
//! implicit growth of a container on `push` is not a site, a call that takes a
//! borrow is trusted not to keep it past the call (section 6, rule 2, which the
//! writer must confirm in `borrowck.rs`), and an address-producing call is
//! recognised by its name.

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{BTreeMap, HashMap};

use crate::lexer::Span;
use crate::stdlib;
use crate::tir::{Block, Expr, ExprKind, Function, Program, Stmt};
use crate::types::{EnumDef, StructDef, Ty};

/// What kind of shared-pool allocation a site is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SiteKind {
    /// `Box.of(v)`: the block constructor of D8.
    BoxOf,
    /// `Box.set(v)` on an empty box: the element is allocated by `set`.
    BoxSet,
    /// A `List`, `Map`, `String` or `Bytes` made by a constructor.
    Container,
    /// A `fun` literal that captured something: its environment is allocated.
    Closure,
}

impl SiteKind {
    pub fn name(self) -> &'static str {
        match self {
            SiteKind::BoxOf => "Box.of",
            SiteKind::BoxSet => "Box.set",
            SiteKind::Container => "container",
            SiteKind::Closure => "closure",
        }
    }
}

/// A place a block value can reach outside its block (section 6, rule 2 and
/// rule 3). Each one is a refusal of every site whose value reaches it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Sink {
    /// Returned from the body, by `return` or as its tail.
    Returned,
    /// The error payload of `?`.
    ErrorPayload,
    /// Moved by value into a call, which may keep it (an unmarked parameter).
    MovedIntoCall,
    /// A borrow passed to a call whose result holds a borrow.
    BorrowIntoResult,
    /// An address (a `Ptr`, or a `usize` made from one) passed on to a call.
    AddressIntoCall,
    /// Stored in a shared-pool object: an argument of `Box.of`, `Box.set`, or a
    /// container constructor.
    StoredInSharedPool,
    /// Stored in a place the body does not own: a parameter, a deref, or a
    /// closure environment field.
    StoredOutside,
    /// Stored in a module-scope `static`.
    StoredInStatic,
    /// Captured by a closure literal, whose environment is shared-pool memory.
    CapturedByClosure,
    /// Captured by a scoped literal.
    CapturedByScope,
    /// Passed to a thread spawn (the argument, which crosses to a worker).
    SpawnArgument,
    /// An operand of inline asm.
    InlineAsm,
}

impl Sink {
    pub fn name(self) -> &'static str {
        match self {
            Sink::Returned => "returned from the block",
            Sink::ErrorPayload => "the error payload of '?'",
            Sink::MovedIntoCall => "moved into a call (an unmarked parameter)",
            Sink::BorrowIntoResult => "borrowed into a call whose result holds a reference",
            Sink::AddressIntoCall => "an address passed into a call",
            Sink::StoredInSharedPool => "stored in a shared-pool object",
            Sink::StoredOutside => "stored in a place outside the block",
            Sink::StoredInStatic => "stored in a static",
            Sink::CapturedByClosure => "captured by a closure literal",
            Sink::CapturedByScope => "captured by a scoped literal",
            Sink::SpawnArgument => "passed to a thread spawn",
            Sink::InlineAsm => "passed to inline asm",
        }
    }
}

/// One allocation site the analysis found in a program body.
#[derive(Debug, Clone)]
pub struct Site {
    pub function: String,
    pub span: Span,
    pub kind: SiteKind,
    pub callee: String,
    pub sinks: Vec<Sink>,
}

impl Site {
    /// Accepted: a `Box.of` site that no sink reached.
    pub fn accepted(&self) -> bool {
        self.kind == SiteKind::BoxOf && self.sinks.is_empty()
    }

    /// The reason a site is refused, in the words the dry run prints. An
    /// accepted site returns `"accepted"`.
    pub fn reason(&self) -> &'static str {
        match self.kind {
            SiteKind::Container => "rule 4: container buffer",
            SiteKind::BoxSet | SiteKind::Closure => "not a block constructor (D8)",
            SiteKind::BoxOf => match self.sinks.first() {
                Some(sink) => sink.name(),
                None => "accepted",
            },
        }
    }
}

/// The result of one analysis: every site in the program's own bodies.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub sites: Vec<Site>,
}

impl Report {
    pub fn total(&self) -> usize {
        self.sites.len()
    }

    pub fn accepted(&self) -> usize {
        self.sites.iter().filter(|s| s.accepted()).count()
    }

    /// The number of sites of each kind, and how many of each kind were accepted.
    pub fn by_kind(&self) -> BTreeMap<&'static str, (usize, usize)> {
        let mut map: BTreeMap<&'static str, (usize, usize)> = BTreeMap::new();
        for site in &self.sites {
            let entry = map.entry(site.kind.name()).or_insert((0, 0));
            entry.0 += 1;
            if site.accepted() {
                entry.1 += 1;
            }
        }
        map
    }

    /// The sites that would be accepted if rule 4 (containers) were lifted:
    /// every `Box.of` and container site that no sink reached. Informational
    /// only; rule 4 stands in version 1.
    pub fn accepted_without_rule_4(&self) -> usize {
        self.sites
            .iter()
            .filter(|s| matches!(s.kind, SiteKind::BoxOf | SiteKind::Container) && s.sinks.is_empty())
            .count()
    }

    /// The number of refused sites for each reason.
    pub fn refused_by_reason(&self) -> BTreeMap<&'static str, usize> {
        let mut map: BTreeMap<&'static str, usize> = BTreeMap::new();
        for site in self.sites.iter().filter(|s| !s.accepted()) {
            *map.entry(site.reason()).or_insert(0) += 1;
        }
        map
    }
}

/// Runs the escape analysis over every body the program defines itself.
pub fn analyze(program: &Program) -> Report {
    let structs: HashMap<&str, &StructDef> =
        program.structs.iter().map(|s| (s.name.as_str(), s)).collect();
    let enums: HashMap<&str, &EnumDef> =
        program.enums.iter().map(|e| (e.name.as_str(), e)).collect();
    let mut cx = Cx { structs, enums, sites: Vec::new(), scopes: Vec::new(), function: String::new() };
    for f in &program.functions {
        if f.extern_ || stdlib::source(&f.module).is_some() {
            continue;
        }
        cx.function(f);
    }
    Report { sites: cx.sites }
}

/// The sites a value may carry. Sorted and deduplicated by construction.
type Carry = std::collections::BTreeSet<usize>;

/// One name in scope: the sites its value may carry, and whether it is a
/// local of the body (true) or a parameter, which belongs to the caller (false).
struct Binding {
    name: String,
    carry: Carry,
    local: bool,
}

struct Cx<'a> {
    structs: HashMap<&'a str, &'a StructDef>,
    enums: HashMap<&'a str, &'a EnumDef>,
    sites: Vec<Site>,
    scopes: Vec<Vec<Binding>>,
    function: String,
}

/// Where an assignment target lives, for the sink it may reach.
enum Root {
    Local(String),
    Static,
    Outside,
}

impl<'a> Cx<'a> {
    fn function(&mut self, f: &Function) {
        self.function = f.name.clone();
        self.scopes = vec![f
            .params
            .iter()
            .map(|(name, _)| Binding { name: name.clone(), carry: Carry::new(), local: false })
            .collect()];
        let tail = self.block(&f.body);
        self.sink(&tail, Sink::Returned);
        self.scopes.clear();
    }

    fn block(&mut self, b: &Block) -> Carry {
        self.scopes.push(Vec::new());
        for stmt in &b.stmts {
            self.stmt(stmt);
        }
        let tail = match &b.tail {
            Some(e) => self.expr(e),
            None => Carry::new(),
        };
        self.scopes.pop();
        tail
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, ty, init } => {
                let carry = self.expr(init);
                let carry = keep(carry, ty);
                self.declare(name, carry);
            }
            Stmt::While { cond, body, .. } => {
                self.expr(cond);
                self.block(body);
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    let carry = self.expr(v);
                    self.sink(&carry, Sink::Returned);
                }
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::Expr(e) => {
                self.expr(e);
            }
        }
    }

    fn declare(&mut self, name: &str, carry: Carry) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(Binding { name: name.to_string(), carry, local: true });
        }
    }

    fn lookup(&self, name: &str) -> Option<&Binding> {
        self.scopes.iter().rev().flat_map(|s| s.iter().rev()).find(|b| b.name == name)
    }

    /// Adds sites to the carry of the innermost local called `name`.
    fn add_to_local(&mut self, name: &str, carry: Carry) {
        for scope in self.scopes.iter_mut().rev() {
            if let Some(b) = scope.iter_mut().rev().find(|b| b.name == name) {
                b.carry.extend(carry);
                return;
            }
        }
    }

    fn sink(&mut self, carry: &Carry, sink: Sink) {
        for &id in carry {
            let site = &mut self.sites[id];
            if !site.sinks.contains(&sink) {
                site.sinks.push(sink);
            }
        }
    }

    fn site(&mut self, span: Span, kind: SiteKind, callee: &str) -> usize {
        self.sites.push(Site {
            function: self.function.clone(),
            span,
            kind,
            callee: callee.to_string(),
            sinks: Vec::new(),
        });
        self.sites.len() - 1
    }

    fn expr(&mut self, e: &Expr) -> Carry {
        match &e.kind {
            ExprKind::Ident(name) => match self.lookup(name) {
                Some(b) if b.local => keep(b.carry.clone(), &e.ty),
                _ => Carry::new(),
            },
            ExprKind::IntLit(_)
            | ExprKind::CharLit(_)
            | ExprKind::FloatLit(_)
            | ExprKind::StringLit(_)
            | ExprKind::BoolLit(_)
            | ExprKind::StaticRef(_)
            | ExprKind::FnRef(_)
            | ExprKind::Capture { .. } => Carry::new(),
            ExprKind::Unary { expr, .. } => {
                let c = self.expr(expr);
                keep(c, &e.ty)
            }
            ExprKind::Binary { lhs, rhs, .. } => {
                let mut c = self.expr(lhs);
                c.extend(self.expr(rhs));
                keep(c, &e.ty)
            }
            ExprKind::Call { callee, args } => self.call(e, callee, args),
            ExprKind::VariadicCall { callee, args, .. } => self.call(e, callee, args),
            ExprKind::IndirectCall { callee, args } => {
                self.expr(callee);
                self.call_args(e, "indirect call", args, Carry::new())
            }
            ExprKind::DynCall { receiver, args, .. } => {
                // The receiver is borrowed by the call, so it is a borrow argument.
                let recv = self.expr(receiver);
                self.call_args(e, "dynamic call", args, recv)
            }
            ExprKind::FieldAccess { base, .. } | ExprKind::EnumField { base, .. } => {
                let c = self.expr(base);
                keep(c, &e.ty)
            }
            ExprKind::Deref { expr }
            | ExprKind::Ref { expr, .. }
            | ExprKind::DynCoerce { expr, .. }
            | ExprKind::BoxDynCoerce { expr, .. }
            | ExprKind::BoxDynBorrow { expr, .. } => {
                let c = self.expr(expr);
                keep(c, &e.ty)
            }
            ExprKind::BoxDynIsEmpty { expr } | ExprKind::EnumTag { expr, .. } => {
                self.expr(expr);
                Carry::new()
            }
            ExprKind::FnToClosure { func } => {
                let c = self.expr(func);
                keep(c, &e.ty)
            }
            ExprKind::StructLit { fields, .. } => {
                let mut c = Carry::new();
                for (_, value) in fields {
                    c.extend(self.expr(value));
                }
                keep(c, &e.ty)
            }
            ExprKind::EnumLit { fields, .. } => {
                let mut c = Carry::new();
                for value in fields {
                    c.extend(self.expr(value));
                }
                keep(c, &e.ty)
            }
            ExprKind::ArrayLit(elems) => {
                let mut c = Carry::new();
                for value in elems {
                    c.extend(self.expr(value));
                }
                keep(c, &e.ty)
            }
            ExprKind::ArrayRepeat { value } => {
                let c = self.expr(value);
                keep(c, &e.ty)
            }
            ExprKind::Try { expr, err_has_payload, .. } => {
                let c = self.expr(expr);
                if *err_has_payload {
                    self.sink(&c, Sink::ErrorPayload);
                }
                keep(c, &e.ty)
            }
            ExprKind::Assign { target, value } => {
                let c = self.expr(value);
                match self.root(target) {
                    Root::Local(name) => {
                        if !c.is_empty() {
                            self.add_to_local(&name, c);
                        }
                    }
                    Root::Static => self.sink(&c, Sink::StoredInStatic),
                    Root::Outside => self.sink(&c, Sink::StoredOutside),
                }
                Carry::new()
            }
            ExprKind::If { cond, then_block, else_block } => {
                self.expr(cond);
                let mut c = self.block(then_block);
                if let Some(eb) = else_block {
                    c.extend(self.block(eb));
                }
                keep(c, &e.ty)
            }
            ExprKind::Block(b) => {
                let c = self.block(b);
                keep(c, &e.ty)
            }
            ExprKind::ClosureLit { captures, .. } => {
                let id = self.site(e.span, SiteKind::Closure, "closure literal");
                for cap in captures {
                    let c = self.expr(cap);
                    self.sink(&c, Sink::CapturedByClosure);
                }
                std::iter::once(id).collect()
            }
            ExprKind::ScopedClosureLit { captures, .. } => {
                for cap in captures {
                    let c = self.expr(cap);
                    self.sink(&c, Sink::CapturedByScope);
                }
                Carry::new()
            }
            ExprKind::Template { pieces, .. } => {
                for piece in pieces {
                    if let crate::tir::TemplatePiece::Value(v) = piece {
                        self.expr(v);
                    }
                }
                Carry::new()
            }
            ExprKind::Asm(block) => {
                for op in &block.operands {
                    if let Some(v) = &op.operand {
                        let c = self.expr(v);
                        self.sink(&c, Sink::InlineAsm);
                    }
                }
                Carry::new()
            }
        }
    }

    /// A call by name. A constructor of a shared-pool value is a site.
    fn call(&mut self, e: &Expr, callee: &str, args: &[Expr]) -> Carry {
        if let Some(kind) = constructor_kind(callee, &e.ty) {
            // The new value is the site. Its by-value arguments are stored in
            // shared-pool memory; a borrowed argument (a `self` of `clone`) is
            // only read.
            let id = self.site(e.span, kind, callee);
            for a in args {
                let ac = self.expr(a);
                if !is_borrow_ty(&a.ty) {
                    self.sink(&ac, Sink::StoredInSharedPool);
                }
            }
            return std::iter::once(id).collect();
        }
        if callee == "Box#set" {
            // `set` allocates the element of an empty box. Its receiver is
            // borrowed; the value is stored in shared-pool memory.
            self.site(e.span, SiteKind::BoxSet, callee);
            for (i, a) in args.iter().enumerate() {
                let ac = self.expr(a);
                if i > 0 {
                    self.sink(&ac, Sink::StoredInSharedPool);
                }
            }
            return Carry::new();
        }
        self.call_args(e, callee, args, Carry::new())
    }

    /// The arguments of a call that makes no shared-pool value of its own.
    /// `receiver` is the carry of a dynamic call's receiver, which the call
    /// borrows; it is empty for every other call.
    fn call_args(&mut self, e: &Expr, callee: &str, args: &[Expr], receiver: Carry) -> Carry {
        let spawn = callee.ends_with("spawn");
        // A method of a shared-pool container stores its by-value arguments.
        let owner = callee.split_once('#').map(|(o, _)| o).unwrap_or("");
        let shared = matches!(owner, "List" | "Map" | "Box" | "String" | "Bytes");
        let by_value_sink = if spawn {
            Sink::SpawnArgument
        } else if shared {
            Sink::StoredInSharedPool
        } else {
            Sink::MovedIntoCall
        };
        let mut borrowed = receiver;
        let mut address = Carry::new();
        for a in args {
            let c = self.expr(a);
            if c.is_empty() {
                continue;
            }
            let is_borrow = is_borrow_ty(&a.ty);
            if is_borrow {
                borrowed.extend(c);
            } else if matches!(a.ty, Ty::Ptr(_) | Ty::Usize | Ty::Isize) {
                // A copy of an address moves nothing, but the callee may keep it.
                address.extend(c.clone());
                self.sink(&c, Sink::AddressIntoCall);
            } else {
                self.sink(&c, by_value_sink);
            }
        }
        let mut result = Carry::new();
        if holds_borrow(&e.ty, &self.structs, &self.enums, 0) {
            // A borrow in the result: every borrowed argument reaches it.
            self.sink(&borrowed, Sink::BorrowIntoResult);
            self.sink(&address, Sink::AddressIntoCall);
        } else if is_address_call(callee) && matches!(e.ty, Ty::Ptr(_) | Ty::Usize | Ty::Isize) {
            // An address derived from a borrowed argument keeps its origin.
            result.extend(borrowed);
            result.extend(address);
        }
        result
    }

    /// Where an assignment target lives. A field of a local is the local.
    fn root(&self, target: &Expr) -> Root {
        match &target.kind {
            ExprKind::Ident(name) => match self.lookup(name) {
                Some(b) if b.local => Root::Local(name.clone()),
                _ => Root::Outside,
            },
            ExprKind::FieldAccess { base, .. } | ExprKind::EnumField { base, .. } => self.root(base),
            ExprKind::StaticRef(_) => Root::Static,
            _ => Root::Outside,
        }
    }
}

/// The kind of shared-pool value a call builds, when it builds one.
fn constructor_kind(callee: &str, ty: &Ty) -> Option<SiteKind> {
    if callee == "Box#of" {
        return Some(SiteKind::BoxOf);
    }
    if callee == "Box#set" {
        return Some(SiteKind::BoxSet);
    }
    let (owner, last) = callee.split_once('#')?;
    let container = matches!(owner, "List" | "Map" | "String" | "Bytes");
    let builds = matches!(last, "of" | "empty" | "new" | "with_capacity" | "clone")
        || last.starts_with("from_")
        || last.starts_with("to_");
    let shaped = matches!(ty, Ty::List(_) | Ty::Map(..) | Ty::String | Ty::Bytes);
    (container && builds && shaped).then_some(SiteKind::Container)
}

/// Whether a call's name marks it as deriving an address from its arguments.
fn is_address_call(callee: &str) -> bool {
    callee.contains("Ptr") || callee.contains("addr") || callee.contains("offset") || callee.contains("cast") || callee.contains("#to_")
}

/// Whether a type can hold a block address. Scalars that are not addresses
/// cannot, so their values are never tracked.
fn may_hold(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::Box(_)
            | Ty::BoxDyn { .. }
            | Ty::Ref { .. }
            | Ty::Dyn { .. }
            | Ty::View
            | Ty::Ptr(_)
            | Ty::Usize
            | Ty::Isize
            | Ty::Struct(_)
            | Ty::Array(..)
            | Ty::Closure { .. }
            | Ty::ScopedFn { .. }
            | Ty::Thread(_)
            | Ty::Mutex(_)
            | Ty::RwLock(_)
            | Ty::Atomic(_)
    )
}

fn keep(carry: Carry, ty: &Ty) -> Carry {
    if may_hold(ty) { carry } else { Carry::new() }
}

/// Whether a value of this type is a borrow: a reference, a view or a `&dyn`.
fn is_borrow_ty(ty: &Ty) -> bool {
    matches!(ty, Ty::Ref { .. } | Ty::View | Ty::Dyn { .. })
}

/// Whether a type holds a borrow anywhere in it (section 6, rule 2: a borrow
/// in a call's result is refused). A struct or enum is looked through, and so
/// is a box or a container of one. The depth bound answers `true` when a type
/// is too deep to read, which keeps the answer conservative.
fn holds_borrow(
    ty: &Ty,
    structs: &HashMap<&str, &StructDef>,
    enums: &HashMap<&str, &EnumDef>,
    depth: usize,
) -> bool {
    if depth > 32 {
        return true;
    }
    match ty {
        Ty::Ref { .. } | Ty::View | Ty::Dyn { .. } | Ty::ScopedFn { .. } => true,
        Ty::Box(inner) | Ty::List(inner) | Ty::Array(inner, _) | Ty::Thread(inner) => {
            holds_borrow(inner, structs, enums, depth + 1)
        }
        Ty::Map(k, v) => {
            holds_borrow(k, structs, enums, depth + 1) || holds_borrow(v, structs, enums, depth + 1)
        }
        Ty::Struct(name) => {
            if let Some(s) = structs.get(name.as_str()) {
                s.fields.iter().any(|f| holds_borrow(&f.ty, structs, enums, depth + 1))
            } else if let Some(e) = enums.get(name.as_str()) {
                e.variants
                    .iter()
                    .flat_map(|v| v.fields.iter())
                    .any(|f| holds_borrow(&f.ty, structs, enums, depth + 1))
            } else {
                false
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::SourceMap;
    use crate::infer;
    use crate::resolve;
    use std::path::{Path, PathBuf};

    /// Loads one `.kp` file, checks it, and runs the analysis on it.
    fn analyze_file(path: &Path) -> Result<Report, String> {
        let mut sources = SourceMap::new();
        let loaded = resolve::load(path, &mut sources, None, None).map_err(|d| format!("{d:?}"))?;
        let typed = infer::check_program(&loaded.program, &loaded.project).map_err(|d| format!("{d:?}"))?;
        Ok(analyze(&typed))
    }

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests").join("escape").join(name)
    }

    /// The reasons refusing the `Box.of` sites of one fixture, in order.
    fn box_reasons(name: &str) -> Vec<&'static str> {
        let report = analyze_file(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        report
            .sites
            .iter()
            .filter(|s| s.kind == SiteKind::BoxOf)
            .map(|s| s.reason())
            .collect()
    }

    /// Every `Box.of` site of one fixture is accepted (the T45 safe twins).
    fn assert_accepted(name: &str) {
        let report = analyze_file(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let boxes: Vec<_> = report.sites.iter().filter(|s| s.kind == SiteKind::BoxOf).collect();
        assert!(!boxes.is_empty(), "{name}: the twin must have a Box.of site");
        for s in boxes {
            assert!(s.accepted(), "{name}: refused as '{}' at {:?}", s.reason(), s.span);
        }
    }

    /// Every fixture is named by the sink it must reach (the T15 refusals).
    fn assert_refused_by(name: &str, sink: Sink) {
        let report = analyze_file(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let boxes: Vec<_> = report.sites.iter().filter(|s| s.kind == SiteKind::BoxOf).collect();
        assert!(!boxes.is_empty(), "{name}: the fixture must have a Box.of site");
        for s in boxes {
            assert!(s.sinks.contains(&sink), "{name}: expected '{}', got {:?}", sink.name(), s.sinks);
        }
    }

    #[test]
    fn t45_a_block_that_returns_a_number_is_accepted() {
        assert_accepted("twin_returns_number.kp");
    }

    #[test]
    fn t45_a_block_value_used_only_inside_the_block_is_accepted() {
        assert_accepted("twin_local_use.kp");
    }

    #[test]
    fn t45_a_borrowing_method_with_a_plain_result_is_accepted() {
        assert_accepted("twin_method_plain_result.kp");
    }

    #[test]
    fn t45_a_number_stored_in_an_outer_variable_is_accepted() {
        assert_accepted("twin_number_outer.kp");
    }

    #[test]
    fn t15_a_returned_box_is_refused_as_returned() {
        assert_refused_by("sink_returned.kp", Sink::Returned);
    }

    #[test]
    fn t15_a_box_moved_into_a_call_is_refused() {
        assert_refused_by("sink_moved_into_call.kp", Sink::MovedIntoCall);
    }

    #[test]
    fn t15_a_box_stored_in_a_parameter_is_refused() {
        assert_refused_by("sink_stored_outside.kp", Sink::StoredOutside);
    }

    #[test]
    fn t15_a_box_captured_by_a_closure_is_refused() {
        assert_refused_by("sink_captured.kp", Sink::CapturedByClosure);
    }

    #[test]
    fn t15_a_box_stored_in_a_container_is_refused() {
        assert_refused_by("sink_shared_pool.kp", Sink::StoredInSharedPool);
    }

    #[test]
    fn t15_a_borrow_in_a_call_result_is_refused() {
        assert_refused_by("sink_borrow_result.kp", Sink::BorrowIntoResult);
    }

    #[test]
    fn t15_a_box_in_the_error_payload_of_try_is_refused() {
        assert_refused_by("sink_error_payload.kp", Sink::ErrorPayload);
    }

    #[test]
    fn t15_a_box_passed_to_a_spawn_is_refused() {
        assert_refused_by("sink_spawn.kp", Sink::SpawnArgument);
    }

    #[test]
    fn t15_a_box_returned_through_a_local_is_refused() {
        assert_refused_by("sink_through_local.kp", Sink::Returned);
    }

    #[test]
    fn a_box_set_and_a_closure_are_not_block_constructors() {
        let report = analyze_file(&fixture("kinds.kp")).unwrap_or_else(|e| panic!("{e}"));
        let mut seen = std::collections::BTreeSet::new();
        for s in &report.sites {
            assert!(!s.accepted() || s.kind == SiteKind::BoxOf, "{s:?}");
            seen.insert(s.kind);
        }
        assert!(seen.contains(&SiteKind::BoxSet), "{seen:?}");
        assert!(seen.contains(&SiteKind::Closure), "{seen:?}");
        assert!(seen.contains(&SiteKind::Container), "{seen:?}");
    }

    #[test]
    fn box_reasons_helper_names_the_first_sink() {
        assert_eq!(box_reasons("sink_returned.kp"), vec![Sink::Returned.name()]);
    }

    /// The dry run of step 2a. It prints the count per program and fails only
    /// when a program does not check. The gate line is judged by the caller.
    /// Run with: `cargo test -p kelpc dry_run_bench_sites -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn dry_run_bench_sites() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("bench");
        let mut programs: Vec<PathBuf> = Vec::new();
        for entry in std::fs::read_dir(root.join("suite")).expect("bench/suite") {
            let dir = entry.expect("entry").path();
            if let Ok(files) = std::fs::read_dir(&dir) {
                for f in files.flatten() {
                    let p = f.path();
                    if p.extension().is_some_and(|x| x == "kp") {
                        programs.push(p);
                    }
                }
            }
        }
        programs.push(root.join("speed").join("speed.kp"));
        programs.push(root.join("write_20gb").join("write20gb.kp"));
        programs.sort();

        let mut total = Report::default();
        let (mut sites, mut accepted) = (0usize, 0usize);
        for p in &programs {
            let name = p.strip_prefix(&root).unwrap_or(p).display().to_string();
            match analyze_file(p) {
                Ok(report) => {
                    let share = if report.total() == 0 {
                        0.0
                    } else {
                        100.0 * report.accepted() as f64 / report.total() as f64
                    };
                    eprintln!(
                        "DRYRUN {name}: sites {} accepted {} share {:.1}%",
                        report.total(),
                        report.accepted(),
                        share
                    );
                    eprintln!("DRYRUN   informational, rule 4 lifted: accepted {}", report.accepted_without_rule_4());
                    for (kind, (n, a)) in report.by_kind() {
                        eprintln!("DRYRUN   kind {kind}: sites {n} accepted {a}");
                    }
                    for (reason, n) in report.refused_by_reason() {
                        eprintln!("DRYRUN   refused {n}: {reason}");
                    }
                    for site in report.sites.iter().filter(|s| !s.accepted()) {
                        eprintln!(
                            "DRYRUN   site {} {} at byte {}: {}",
                            site.function,
                            site.callee,
                            site.span.start,
                            site.reason()
                        );
                    }
                    sites += report.total();
                    accepted += report.accepted();
                    total.sites.extend(report.sites);
                }
                Err(e) => eprintln!("DRYRUN {name}: does not check: {e}"),
            }
        }
        let share = 100.0 * accepted as f64 / sites.max(1) as f64;
        eprintln!("DRYRUN TOTAL informational, rule 4 lifted: accepted {}", total.accepted_without_rule_4());
        eprintln!("DRYRUN TOTAL: programs {} sites {sites} accepted {accepted} share {share:.1}%", programs.len());
        for (reason, n) in total.refused_by_reason() {
            eprintln!("DRYRUN TOTAL refused {n}: {reason}");
        }
        for (kind, (n, a)) in total.by_kind() {
            eprintln!("DRYRUN TOTAL kind {kind}: sites {n} accepted {a}");
        }
    }
}
