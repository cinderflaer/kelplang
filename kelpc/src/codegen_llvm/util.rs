use super::*;

/// An expression that reads state and cannot write any: literals, names,
/// operators, field reads, borrows, conversions and element reads of a `List`
/// or `Bytes` whose own operands are read-only. A call of any other function
/// is not read-only, because it may push to or set a container.
pub(crate) fn read_only_expr(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::IntLit(_)
        | ExprKind::BlockBox { .. }
        | ExprKind::CharLit(_)
        | ExprKind::FloatLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::Ident(_)
        | ExprKind::StaticRef(_)
        | ExprKind::Capture { .. } => true,
        ExprKind::Unary { expr, .. } | ExprKind::Ref { expr, .. } => read_only_expr(expr),
        ExprKind::Binary { lhs, rhs, .. } => read_only_expr(lhs) && read_only_expr(rhs),
        ExprKind::FieldAccess { base, .. } => read_only_expr(base),
        ExprKind::Call { callee, args } => {
            let reads = callee.contains("#to_")
                || callee == "List#get"
                || callee == "Bytes#get";
            reads && args.iter().all(read_only_expr)
        }
        _ => false,
    }
}

/// A symbol name for a LLVM function, mirroring the earlier emitter's `c_name` without
/// reaching into that (private) helper: `main` keeps its name because the C
/// runtime requires it, everything else gets the `kelp_` prefix and the same
/// sanitizing mangler, so the name is a valid symbol and cannot collide with a
/// libc one.
pub(crate) fn symbol_name(name: &str) -> String {
    if name == "main" {
        "main".to_string()
    } else {
        format!("kelp_{}", crate::types::c_ident(name))
    }
}

/// Whether an integer type is signed. The signedness is what picks `sdiv` over
/// `udiv`, `ashr` over `lshr` and `icmp slt` over `ult`; `char` is unsigned
/// because it is a code point (`uint32_t`), and `bool` is neither (it never
/// reaches a division).
pub(crate) fn is_signed(ty: &Ty) -> bool {
    matches!(ty, Ty::I32 | Ty::I64 | Ty::Isize)
}

/// The bit width of an integer-like type, or `None` for anything else.
pub(crate) fn int_width(ty: &Ty) -> Option<u32> {
    Some(match ty {
        Ty::I32 | Ty::U32 | Ty::Char => 32,
        Ty::I64 | Ty::U64 | Ty::Usize | Ty::Isize => 64,
        Ty::U8 => 8,
        Ty::U16 => 16,
        Ty::Bool => 1,
        _ => return None,
    })
}

/// The width in bits of an integer the byte-order conversions cover, or `None`
/// when `ty` has no such conversion (`bool`, `char`, `u8`, a pointer).
/// `usize`/`isize` are the target's pointer-width integers, which `llvm_type`
/// maps to `i64`, so their swap is 64 bits wide; the earlier emitter reads the same
/// width from `sizeof` in `KELP_SWAP_SIZE`.
pub(crate) fn endian_bits(ty: &Ty) -> Option<u32> {
    Some(match ty {
        Ty::U16 => 16,
        Ty::U32 | Ty::I32 => 32,
        Ty::U64 | Ty::I64 | Ty::Usize | Ty::Isize => 64,
        _ => return None,
    })
}

/// The refusal message for a construct the lowering does not handle. `what` is
/// the construct's name, quoted when it is a language word (`'enum'`) and a
/// phrase when it is a shape; `stage` is the MK7 stage that lowered it. Every
/// stage that has landed still passes its label, so the string reads as
/// history; a refusal that fires today names a shape the frontend does not
/// currently produce, so it says the shortfall is the backend's rather than
/// sending the caller to a backend they cannot choose.
pub(crate) fn refuse(what: &str, stage: &str) -> String {
    format!("the llvm backend cannot lower {what} yet (MK7 {stage}); this is a compiler defect")
}

/// Refuse a `datac` field or an `enum` payload field this slice cannot lay out,
/// with the construct named.
///
/// A `String`/`Bytes` field, a field of an owned aggregate and a field that
/// names a `datac` or an `enum` are laid out (MK7 §3.1, §3.4), and so are a
/// container, an array and a function value or closure (S2D, S2E); everything
/// else is named by its own construct. The `str`/`View` rows are unreachable
/// from the frontend (`types::is_borrowed_field_ty` rejects them) but are
/// refused here rather than mis-lowered if they ever appear.
pub(crate) fn refuse_field(ty: &Ty) -> Result<(), String> {
    match ty {
        Ty::Bytes | Ty::String => Ok(()),
        // A container field is laid out now (S2D): a `List`/`Box`/`Map` holds its
        // contents through a pointer, an array inline, and both are complete
        // types by the time the `datac` that names one is given a body.
        Ty::List(_) | Ty::Box(_) | Ty::Map(..) | Ty::Array(..) => Ok(()),
        // An `Atomic<T>` field is a one-word struct holding a scalar (MK9-S3-M1
        // stage A1), so it is as nameable as the container beside it. MK9 9.3
        // leaves "may an atomic be a `datac` field" open, and neither backend
        // decides it: the frontend has no rule that refuses one, so both lay it
        // out, and what is refused is an atomic as a *container element* (the
        // element sets are unchanged), where the container's own routines would
        // read and write it as a plain location.
        Ty::Atomic(_) => Ok(()),
        // A `Thread<T>` field is one pointer to the worker record (MK9-S3-M1
        // stage C1), so it is as nameable as the atomic beside it. The
        // frontend accepts a handle as a field and refuses it as a container
        // element, and the pointer is what both backends lay out.
        Ty::Thread(_) => Ok(()),
        // A `Mutex<T>` field is the same one pointer to a runtime record
        // (B-5), accepted as a field a `lock` borrows. An `RwLock<T>` and a
        // `Condvar` field are the same pointer (`std/sync`).
        Ty::Mutex(_) | Ty::RwLock(_) | Ty::Condvar => Ok(()),
        // An owned trait object field (A-12 slice 7) is the fat pair struct the
        // container pass emits before every `datac`, exactly as a `Box` field
        // is. The *borrowed* spelling never reaches here: the frontend refuses
        // it first (`types::is_borrowed_field_ty`).
        Ty::BoxDyn { .. } => Ok(()),
        // An opaque handle field (gap 1) is one pointer; the frontend allows
        // it as a field, a local, a parameter and a return. A `Ptr<T>` field
        // (kernel stage 13) is the same one-pointer layout: the arena's own
        // `datac` stores its base this way.
        Ty::Handle(_) | Ty::Ptr(_) => Ok(()),
        Ty::Str | Ty::View => Ok(()),
        Ty::Ref { .. } => Err(refuse("'a reference field'", "S2F")),
        // A function value is a `ptr` field and a closure is the fat pointer's
        // struct, laid out by the container pass before this one (S2E).
        Ty::Fn { .. } | Ty::Closure { .. } | Ty::ScopedFn { .. } => Ok(()),
        Ty::Struct(_) => Ok(()),
        other if llvm_scalar_type(other) => Ok(()),
        other => Err(refuse_type(other)),
    }
}

/// Whether a `tir` type is one of the scalars the S1 lowering already emits. A
/// type that is not a scalar and not an aggregate is refused by name in
/// `refuse_field`, so this only has to recognize the scalars.
pub(crate) fn llvm_scalar_type(ty: &Ty) -> bool {
    matches!(
        ty,
        Ty::I32
            | Ty::I64
            | Ty::U32
            | Ty::U64
            | Ty::Usize
            | Ty::Isize
            | Ty::U8
            | Ty::U16
            | Ty::F32
            | Ty::F64
            | Ty::Bool
            | Ty::Char
    )
}

/// Name the type in a refusal, with the stage that will lower it. A container
/// is the container slice (S2D); a function value, a closure and a borrow of
/// either are lowered from S2E on, so those rows are unreachable and are kept
/// only so the match stays total without a panic.
pub(crate) fn refuse_type(ty: &Ty) -> String {
    let (what, stage) = match ty {
        Ty::Struct(_) => ("'datac'", "S2B"),
        Ty::Bytes => ("'Bytes'", "S2B"),
        Ty::String => ("'String'", "S2B"),
        Ty::List(_) => ("'List'", "S2D"),
        Ty::Box(_) => ("'Box'", "S2D"),
        Ty::Map(..) => ("'Map'", "S2D"),
        Ty::Array(..) => ("'an array'", "S2D"),
        Ty::Ref { .. } | Ty::Fn { .. } | Ty::Closure { .. } | Ty::ScopedFn { .. } => {
            return no_value(&format!("the type '{}'", ty.display()))
        }
        Ty::Str | Ty::View => ("a borrowed view", "S2D"),
        Ty::Handle(_) => ("an opaque handle", "S1"),
        Ty::Unit => ("'unit'", "S1"),
        other => return refuse(&format!("the type '{}'", other.display()), "S1"),
    };
    refuse(what, stage)
}

/// The internal-error text for a value position the lowering expected to hold a
/// value and did not. It is an `Err`, not a panic: even an internal mistake
/// leaves the user with a named line rather than a backtrace.
pub(crate) fn no_value(what: &str) -> String {
    format!("internal error: the llvm backend has no value for {what} (MK7 S1)")
}

/// The block release at the end of a region (decision 139, step 1): one free
/// per region, the inner region's before the outer's, and both before the
/// function returns. The IR is read as text; nothing is linked or run.
#[cfg(test)]
mod region_release_tests {
    use super::*;
    use crate::diag::SourceMap;
    use crate::lexer::Span;
    use crate::tir;
    use crate::types::Ty;
    use std::collections::{HashMap, HashSet};

    pub(crate) fn span() -> Span {
        Span { start: 0, end: 0 }
    }

    pub(crate) fn region(name: &str, tail: tir::Expr) -> tir::Expr {
        tir::Expr {
            kind: tir::ExprKind::Region {
                name: name.to_string(),
                elem: Ty::I32,
                body: tir::Block {
                    stmts: Vec::new(),
                    tail: Some(Box::new(tail)),
                },
            },
            ty: Ty::I32,
            span: span(),
        }
    }

    pub(crate) fn one_function(body: tir::Block) -> tir::Program {
        tir::Program {
            structs: Vec::new(),
            enums: Vec::new(),
            pds: Vec::new(),
            functions: vec![tir::Function {
                name: "pick".to_string(),
                params: vec![("x".to_string(), Ty::I32)],
                ret: Ty::I32,
                body,
                span: span(),
                module: "main".to_string(),
                extern_: false,
                env: None,
                naked: false,
                interrupt: false,
                align: None,
                section: None,
            }],
            caps: Vec::new(),
            gated: HashMap::new(),
            declared_keys: HashSet::new(),
            drop_types: HashSet::new(),
            extern_ffi_symbols: HashMap::new(),
            const_data: Vec::new(),
            statics: Vec::new(),
            dyn_vtables: Vec::new(),
            dyn_traits: Vec::new(),
        }
    }

    pub(crate) fn lower(program: &tir::Program) -> String {
        let drops = crate::borrowck::check_program(program).expect("the program checks");
        let sources = SourceMap::new();
        let dir = std::env::temp_dir().join(format!("kelpc-region-release-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temp directory");
        let name = format!("main-{}", std::thread::current().name().unwrap_or("t").replace("::", "-"));
        compile_ir(
            program,
            &drops,
            &sources,
            None,
            &dir.join(format!("{name}.ll")),
            &dir.join(format!("{name}.o")),
        )
        .expect("the program lowers")
    }

    /// `pick` returns `x` from `block outer: i32 { block inner: i32 { x } }`:
    /// the inner free comes first, then the outer, then the `ret`.
    #[test]
    pub(crate) fn nested_regions_release_inner_first_before_the_return() {
        let x = tir::Expr {
            kind: tir::ExprKind::Ident("x".to_string()),
            ty: Ty::I32,
            span: span(),
        };
        let program = one_function(tir::Block {
            stmts: Vec::new(),
            tail: Some(Box::new(region("outer", region("inner", x)))),
        });
        let ir = lower(&program);
        let inner = ir
            .find("@kelp_gc_block_free(i64 %free.inner.handle")
            .unwrap_or_else(|| panic!("the inner region is freed:\n{ir}"));
        let outer = ir
            .find("@kelp_gc_block_free(i64 %free.outer.handle")
            .unwrap_or_else(|| panic!("the outer region is freed:\n{ir}"));
        let ret = ir.rfind("ret i32").expect("main returns");
        assert!(inner < outer, "the inner region is freed first:\n{ir}");
        assert!(outer < ret, "both regions are freed before the return:\n{ir}");
        assert_eq!(ir.matches("call i32 @kelp_gc_block_free(").count(), 2, "one free per region:\n{ir}");
        assert!(ir.contains("declare i32 @kelp_gc_block_free("), "the entry point is declared:\n{ir}");
    }

    /// A program with no region declares neither block entry point, so its IR
    /// is the one every program had before the block runtime.
    #[test]
    pub(crate) fn a_program_without_a_region_declares_no_block_entry_point() {
        let x = tir::Expr {
            kind: tir::ExprKind::Ident("x".to_string()),
            ty: Ty::I32,
            span: span(),
        };
        let program = one_function(tir::Block {
            stmts: Vec::new(),
            tail: Some(Box::new(x)),
        });
        let ir = lower(&program);
        assert!(!ir.contains("kelp_gc_block_"), "no block entry point is named:\n{ir}");
    }

    pub(crate) fn int(value: i128) -> tir::Expr {
        tir::Expr {
            kind: tir::ExprKind::IntLit(value),
            ty: Ty::I32,
            span: span(),
        }
    }

    /// `return` inside a region releases it before the `ret`, and the region's
    /// own end does not release it a second time (the body diverged).
    #[test]
    pub(crate) fn a_return_inside_a_region_releases_it_before_the_return() {
        let ret = tir::Stmt::Return {
            value: Some(int(7)),
            span: span(),
        };
        let inner = tir::Expr {
            kind: tir::ExprKind::Region {
                name: "arena".to_string(),
                elem: Ty::I32,
                body: tir::Block {
                    stmts: vec![ret],
                    tail: None,
                },
            },
            ty: Ty::Unit,
            span: span(),
        };
        let program = one_function(tir::Block {
            stmts: vec![tir::Stmt::Expr(inner)],
            tail: None,
        });
        let ir = lower(&program);
        assert_eq!(ir.matches("call i32 @kelp_gc_block_free(").count(), 1, "one release:\n{ir}");
        let free = ir.find("call i32 @kelp_gc_block_free(").expect("the release");
        let ret = ir.rfind("ret i32").expect("the return");
        assert!(free < ret, "the release comes before the return:\n{ir}");
    }

    /// `break` inside a region inside a loop releases the region on the way out
    /// of the loop, and the region that encloses the loop is released after it.
    #[test]
    pub(crate) fn a_break_releases_only_the_regions_inside_its_loop() {
        let brk = tir::Stmt::Break { span: span() };
        let inner = tir::Expr {
            kind: tir::ExprKind::Region {
                name: "inner".to_string(),
                elem: Ty::I32,
                body: tir::Block {
                    stmts: vec![brk],
                    tail: None,
                },
            },
            ty: Ty::Unit,
            span: span(),
        };
        let looping = tir::Stmt::While {
            cond: tir::Expr {
                kind: tir::ExprKind::BoolLit(true),
                ty: Ty::Bool,
                span: span(),
            },
            body: tir::Block {
                stmts: vec![tir::Stmt::Expr(inner)],
                tail: None,
            },
            iterating: None,
        };
        let outer = tir::Expr {
            kind: tir::ExprKind::Region {
                name: "outer".to_string(),
                elem: Ty::I32,
                body: tir::Block {
                    stmts: vec![looping],
                    tail: Some(Box::new(int(3))),
                },
            },
            ty: Ty::I32,
            span: span(),
        };
        let program = one_function(tir::Block {
            stmts: Vec::new(),
            tail: Some(Box::new(outer)),
        });
        let ir = lower(&program);
        let inner_free = ir.find("@kelp_gc_block_free(i64 %free.inner.handle").expect("the inner release");
        let exit = ir.find("while.exit:").expect("the loop exit");
        let outer_free = ir.find("@kelp_gc_block_free(i64 %free.outer.handle").expect("the outer release");
        assert!(inner_free < exit, "the break releases the inner region on its way out:\n{ir}");
        assert!(exit < outer_free, "the outer region is released after the loop:\n{ir}");
        assert!(!ir[..exit].contains("%free.outer.handle"), "the loop does not release the outer region:\n{ir}");
    }
}








