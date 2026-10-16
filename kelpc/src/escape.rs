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
//! - A `List`, `Map`, `String` or `Bytes` value made by a constructor. Step 2b
//!   (R57, Q1, Q3, Q7, Q9) accepts a `List` or `Map` of numbers made with
//!   `new`, `empty`, `with_capacity` or `of`. `String` and `Bytes` wait for the
//!   byte buffer path (Q7).
//! - A `fun` literal that captures something. Its environment is shared-pool
//!   memory and D8 names no block constructor for it, so it is refused.
//!
//! The walk gives every expression a carry: the set of sites whose value may
//! flow into it. A carry that reaches a sink (section 6, rule 2) refuses each
//! site in it, and the sink is recorded by name. A site is accepted when no
//! sink reached it and no refusal was decided at the site (for a container,
//! the form, the String and Bytes rule and the element rule).
//!
//! Known limits of this count (they make it a conservative estimate, not an
//! exact one): sites inside shipped stdlib bodies are not counted, the
//! implicit growth of a container on `push` is not a site, a container made by
//! a method (`slice`, `keys`, `copy_range`) is not a site, a call that takes a
//! shared borrow is trusted not to keep it past the call (section 6, rule 2,
//! which the writer must confirm in `borrowck.rs`), an address-producing call
//! is recognised by its name, and a `defer` is found by its line in the source
//! text (the lowering in `defer.rs` leaves no mark in the typed program).

#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{BTreeMap, HashMap};

use crate::diag::SourceMap;
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
    /// A container mutably borrowed by a call other than the container's own
    /// method (R60, Q5): the call could grow it outside the block's checks.
    MutableBorrowIntoCall,
    /// A grow call (`push`, `insert` and the like) written in a defer (R60).
    GrowInDefer,
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
            Sink::MutableBorrowIntoCall => "mutably borrowed by a call",
            Sink::GrowInDefer => "grown in a defer",
        }
    }
}

/// Refusal texts decided at a container site, before any sink is read.
const REFUSE_FORM: &str = "Q3 and Q9: this form has no block constructor";
const REFUSE_BYTES: &str = "Q7: String and Bytes wait for the byte buffer path";
const REFUSE_ELEMENT: &str = "R57: the element holds a pointer, a resource or a usize";

/// The container owners and their methods that can grow a buffer.
const CONTAINER_OWNERS: [&str; 4] = ["List", "Map", "String", "Bytes"];
const GROW_METHODS: [&str; 5] = ["push", "insert", "push_str", "append", "reserve"];

/// One allocation site the analysis found in a program body.
#[derive(Debug, Clone)]
pub struct Site {
    pub function: String,
    pub span: Span,
    pub kind: SiteKind,
    pub callee: String,
    /// The constructor form: the name after `#` in the callee (`new`, `of`, ...).
    pub form: String,
    /// The element type of a container site, as the type displays it.
    pub element: String,
    /// A refusal decided at the site itself (container form, Q7, R57).
    pub refused: Option<&'static str>,
    /// The grow calls that reach this container (counted, not part of the share).
    pub grows: usize,
    pub sinks: Vec<Sink>,
}

impl Site {
    /// Accepted: a `Box.of` site that no sink reached, or a container site
    /// with no refusal at the site and no sink.
    pub fn accepted(&self) -> bool {
        match self.kind {
            SiteKind::BoxOf => self.sinks.is_empty(),
            SiteKind::Container => self.refused.is_none() && self.sinks.is_empty(),
            SiteKind::BoxSet | SiteKind::Closure => false,
        }
    }

    /// The reason a site is refused, in the words the dry run prints. An
    /// accepted site returns `"accepted"`.
    pub fn reason(&self) -> &'static str {
        match self.kind {
            SiteKind::BoxSet | SiteKind::Closure => "not a block constructor (D8)",
            SiteKind::BoxOf | SiteKind::Container => {
                if let Some(why) = self.refused {
                    return why;
                }
                match self.sinks.first() {
                    Some(sink) => sink.name(),
                    None => "accepted",
                }
            }
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

    /// The grow calls that reach an accepted container, counted once per
    /// accepted site they reach. Printed beside the share, not part of it.
    pub fn grow_calls_on_accepted(&self) -> usize {
        self.sites.iter().filter(|s| s.accepted()).map(|s| s.grows).sum()
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

/// Runs the escape analysis over every body the program defines itself. The
/// source texts are read only to find the `defer` lines (R60).
pub fn analyze(program: &Program, sources: &SourceMap) -> Report {
    let structs: HashMap<&str, &StructDef> =
        program.structs.iter().map(|s| (s.name.as_str(), s)).collect();
    let enums: HashMap<&str, &EnumDef> =
        program.enums.iter().map(|e| (e.name.as_str(), e)).collect();
    let mut defers: HashMap<String, Vec<Span>> = HashMap::new();
    for f in &program.functions {
        defers
            .entry(f.module.clone())
            .or_insert_with(|| sources.text(&f.module).map(defer_spans).unwrap_or_default());
    }
    let mut cx = Cx {
        structs,
        enums,
        sites: Vec::new(),
        scopes: Vec::new(),
        function: String::new(),
        module: String::new(),
        defers,
    };
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
    /// The module of the body being walked, which names its source text.
    module: String,
    /// The byte ranges of the `defer` statements of each module.
    defers: HashMap<String, Vec<Span>>,
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
        self.module = f.module.clone();
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
        let form = callee.split_once('#').map(|(_, f)| f).unwrap_or("").to_string();
        self.sites.push(Site {
            function: self.function.clone(),
            span,
            kind,
            callee: callee.to_string(),
            form,
            element: String::new(),
            refused: None,
            grows: 0,
            sinks: Vec::new(),
        });
        self.sites.len() - 1
    }

    /// Whether a span starts inside a `defer` statement of the body's module.
    fn in_defer(&self, at: Span) -> bool {
        self.defers
            .get(&self.module)
            .is_some_and(|spans| spans.iter().any(|d| d.start <= at.start && at.start < d.end))
    }

    /// The sites of a carry that are containers.
    fn container_sites(&self, carry: &Carry) -> Carry {
        carry.iter().copied().filter(|&id| self.sites[id].kind == SiteKind::Container).collect()
    }

    /// A grow call on the containers of `carry` (R60): each container counts
    /// the call, and a call written in a defer is a sink.
    fn grow(&mut self, carry: &Carry, at: Span) {
        let containers = self.container_sites(carry);
        for &id in &containers {
            self.sites[id].grows += 1;
        }
        if self.in_defer(at) {
            self.sink(&containers, Sink::GrowInDefer);
        }
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
            ExprKind::Region { body, .. } => {
                let c = self.block(body);
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
            if kind == SiteKind::Container {
                let (element, refused) = container_check(&self.sites[id].form, &e.ty);
                self.sites[id].element = element;
                self.sites[id].refused = refused;
            }
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
        let method = callee.split_once('#').map(|(_, m)| m).unwrap_or("");
        let shared = matches!(owner, "List" | "Map" | "Box" | "String" | "Bytes");
        let own_container = CONTAINER_OWNERS.contains(&owner);
        let grows = own_container && GROW_METHODS.contains(&method);
        let by_value_sink = if spawn {
            Sink::SpawnArgument
        } else if shared {
            Sink::StoredInSharedPool
        } else {
            Sink::MovedIntoCall
        };
        let mut borrowed = receiver;
        let mut address = Carry::new();
        for (i, a) in args.iter().enumerate() {
            let c = self.expr(a);
            // The receiver of a container's own method is not a call that
            // grows the container from outside: the method is the growth.
            let own_receiver = i == 0 && own_container;
            if own_receiver && grows {
                self.grow(&c, e.span);
            }
            if c.is_empty() {
                continue;
            }
            if matches!(a.ty, Ty::Ref { mutable: true, .. }) && !own_receiver {
                let containers = self.container_sites(&c);
                self.sink(&containers, Sink::MutableBorrowIntoCall);
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
            | Ty::List(_)
            | Ty::Map(..)
            | Ty::String
            | Ty::Bytes
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

/// The element type of a container site, and the refusal decided at the site.
/// The form is checked first (Q3, Q9), then the owner (Q7), then the element
/// (R57, Q1).
fn container_check(form: &str, ty: &Ty) -> (String, Option<&'static str>) {
    let element = match ty {
        Ty::List(inner) => inner.display(),
        Ty::Map(k, v) => format!("{}, {}", k.display(), v.display()),
        other => other.display(),
    };
    if !matches!(form, "new" | "empty" | "with_capacity" | "of") {
        return (element, Some(REFUSE_FORM));
    }
    let numbers = match ty {
        Ty::List(inner) => number_element(inner),
        Ty::Map(k, v) => number_element(k) && number_element(v),
        _ => return (element, Some(REFUSE_BYTES)),
    };
    if numbers { (element, None) } else { (element, Some(REFUSE_ELEMENT)) }
}

/// R57 with Q1: a number, a bool or a char holds no pointer and no resource.
/// A `usize` or `isize` may hold an address, so it is refused. Structs and
/// enums are refused too, because the resource check of a struct (D19) is
/// not in this analysis.
fn number_element(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::I32 | Ty::I64 | Ty::U32 | Ty::U64 | Ty::F32 | Ty::F64 | Ty::Bool | Ty::Char | Ty::U8 | Ty::U16
    )
}

/// The byte ranges of the `defer` statements in one source text. A statement
/// runs from its line to the line where its brackets close, so a `defer { ... }`
/// block is covered in full.
fn defer_spans(text: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut depth: i64 = 0;
    let mut start = 0;
    let mut inside = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if !inside {
            let body = line.trim_start();
            let is_defer = body.strip_prefix("defer").is_some_and(|r| r.starts_with(char::is_whitespace));
            if !is_defer {
                offset += line.len();
                continue;
            }
            inside = true;
            start = offset;
            depth = 0;
        }
        for ch in line.chars() {
            match ch {
                '{' | '(' | '[' => depth += 1,
                '}' | ')' | ']' => depth -= 1,
                _ => {}
            }
        }
        offset += line.len();
        if depth <= 0 {
            out.push(Span::new(start, offset));
            inside = false;
        }
    }
    out
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
        Ok(analyze(&typed, &sources))
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

    /// Every container site of one fixture is refused, and one of them reaches
    /// the named sink (the T94 and T95 refusals).
    fn assert_container_refused_by(name: &str, sink: Sink) {
        let report = analyze_file(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let containers: Vec<_> = report.sites.iter().filter(|s| s.kind == SiteKind::Container).collect();
        assert!(!containers.is_empty(), "{name}: the fixture must have a container site");
        for s in &containers {
            assert!(!s.accepted(), "{name}: accepted at byte {}", s.span.start);
        }
        assert!(
            containers.iter().any(|s| s.sinks.contains(&sink)),
            "{name}: expected '{}', got {:?}",
            sink.name(),
            containers.iter().map(|s| &s.sinks).collect::<Vec<_>>()
        );
    }

    /// Every container site of one fixture is accepted (the T95 twins).
    fn assert_container_accepted(name: &str) {
        let report = analyze_file(&fixture(name)).unwrap_or_else(|e| panic!("{name}: {e}"));
        let containers: Vec<_> = report.sites.iter().filter(|s| s.kind == SiteKind::Container).collect();
        assert!(!containers.is_empty(), "{name}: the twin must have a container site");
        for s in containers {
            assert!(s.accepted(), "{name}: refused as '{}' at byte {}", s.reason(), s.span.start);
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
    fn t94_a_list_returned_from_its_body_is_refused() {
        assert_container_refused_by("list_returned.kp", Sink::Returned);
    }

    #[test]
    fn t94_b_list_moved_into_a_call_is_refused() {
        assert_container_refused_by("list_moved_into_call.kp", Sink::MovedIntoCall);
    }

    #[test]
    fn t94_c_list_mutably_borrowed_by_a_call_is_refused() {
        assert_container_refused_by("list_mut_borrow_into_call.kp", Sink::MutableBorrowIntoCall);
    }

    #[test]
    fn t94_d_list_captured_by_a_closure_is_refused() {
        assert_container_refused_by("list_captured.kp", Sink::CapturedByClosure);
    }

    #[test]
    fn t94_e_list_stored_in_a_shared_pool_list_is_refused() {
        assert_container_refused_by("list_stored_in_shared_pool.kp", Sink::StoredInSharedPool);
    }

    #[test]
    fn t94_f_list_passed_to_a_thread_spawn_is_refused() {
        assert_container_refused_by("list_spawn_argument.kp", Sink::SpawnArgument);
    }

    #[test]
    fn t95_a_grow_call_in_a_defer_is_refused() {
        assert_container_refused_by("grow_in_defer.kp", Sink::GrowInDefer);
    }

    #[test]
    fn t95_b_mutable_borrow_into_a_call_is_refused() {
        assert_container_refused_by("list_mut_borrow_into_call.kp", Sink::MutableBorrowIntoCall);
    }

    #[test]
    fn t95_c_shared_borrow_with_a_plain_result_is_accepted() {
        assert_container_accepted("list_borrow_twin.kp");
    }

    #[test]
    fn t95_d_grow_call_in_the_owning_block_is_accepted() {
        assert_container_accepted("list_block_twin.kp");
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
            // A container of numbers may be accepted (R57); a closure and a
            // Box.set never are.
            assert!(!s.accepted() || matches!(s.kind, SiteKind::BoxOf | SiteKind::Container), "{s:?}");
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

    /// The bench directory the dry run reads.
    fn bench_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("bench")
    }

    /// The programs the dry run counts: every suite program, the speed program
    /// and the 20 GB write program.
    fn bench_programs() -> Vec<PathBuf> {
        let root = bench_root();
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
        programs
    }

    /// The 16 sites of the step 2a dry run (commit 0646ae4): program, function,
    /// callee and the byte where the site starts. T103 checks the denominator
    /// against this list, so it cannot change between runs.
    const BASELINE: [(&str, &str, &str, usize); 16] = [
        ("suite/binary-trees/binary-trees.kp", "make", "Box#of", 665),
        ("suite/binary-trees/binary-trees.kp", "make", "Box#of", 694),
        ("suite/fannkuch-redux/fannkuch-redux.kp", "main", "List#empty", 505),
        ("suite/fannkuch-redux/fannkuch-redux.kp", "main", "List#empty", 544),
        ("suite/fannkuch-redux/fannkuch-redux.kp", "main", "List#empty", 584),
        ("suite/hashmap/hashmap.kp", "main", "Map#new", 645),
        ("suite/json-tokenizer/json-tokenizer.kp", "main", "String#empty", 1200),
        ("suite/matmul/matmul.kp", "main", "List#empty", 561),
        ("suite/matmul/matmul.kp", "main", "List#empty", 597),
        ("suite/matmul/matmul.kp", "main", "List#empty", 633),
        ("suite/sort/sort.kp", "main", "List#empty", 2103),
        ("suite/sort-builtin/sort-builtin.kp", "main", "List#empty", 598),
        ("suite/spectral-norm/spectral-norm.kp", "main", "List#empty", 1331),
        ("suite/spectral-norm/spectral-norm.kp", "main", "List#empty", 1367),
        ("suite/spectral-norm/spectral-norm.kp", "main", "List#empty", 1405),
        ("suite/strings/strings.kp", "main", "String#empty", 544),
    ];

    /// T103: the gate counts the same 16 sites, each container site records its
    /// form and its element type, and the refusals add up to the total.
    #[test]
    fn t103_the_gate_counts_the_same_sites_and_its_total_adds_up() {
        let root = bench_root();
        let mut found: Vec<(String, String, String, usize)> = Vec::new();
        let mut total = Report::default();
        for p in bench_programs() {
            let name = p.strip_prefix(&root).unwrap_or(&p).display().to_string();
            let report = analyze_file(&p).unwrap_or_else(|e| panic!("{name}: {e}"));
            for s in &report.sites {
                found.push((name.clone(), s.function.clone(), s.callee.clone(), s.span.start));
            }
            total.sites.extend(report.sites);
        }
        found.sort();
        let mut expected: Vec<_> =
            BASELINE.iter().map(|(p, f, c, b)| (p.to_string(), f.to_string(), c.to_string(), *b)).collect();
        expected.sort();
        assert_eq!(found, expected, "the denominator changed since 0646ae4");
        for s in total.sites.iter().filter(|s| s.kind == SiteKind::Container) {
            assert!(!s.form.is_empty() && !s.element.is_empty(), "{s:?}");
        }
        let refused: usize = total.refused_by_reason().values().sum();
        assert_eq!(total.total(), total.accepted() + refused);
    }

    /// The dry run of step 2a, kept for step 2b. It prints the count per
    /// program and fails only when a program does not check. The gate line is
    /// judged by the caller. Run with:
    /// `cargo test -p kelpc --bin kelpc escape -- --include-ignored --nocapture`.
    #[test]
    #[ignore]
    fn dry_run_bench_sites() {
        let root = bench_root();
        let programs = bench_programs();

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
                    eprintln!("DRYRUN   grow calls on accepted containers: {}", report.grow_calls_on_accepted());
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
        eprintln!("DRYRUN TOTAL grow calls on accepted containers: {}", total.grow_calls_on_accepted());
        eprintln!("DRYRUN TOTAL: programs {} sites {sites} accepted {accepted} share {share:.1}%", programs.len());
        for (reason, n) in total.refused_by_reason() {
            eprintln!("DRYRUN TOTAL refused {n}: {reason}");
        }
        for (kind, (n, a)) in total.by_kind() {
            eprintln!("DRYRUN TOTAL kind {kind}: sites {n} accepted {a}");
        }
    }
}
