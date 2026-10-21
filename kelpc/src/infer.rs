use std::collections::HashMap;

use crate::ast::{self, BinOp, Repr, UnOp};
use crate::diag::Diagnostic;
use crate::lexer::Span;
use crate::managed;
use crate::resolve::Project;
use crate::tir;
use crate::types::{EnumDef, FieldDef, FnSig, StructDef, Ty};

#[derive(Debug)]
pub struct TypeError {
    pub message: String,
    pub span: Span,
    /// The next step a reader can take, carried onto the `Diagnostic`.
    pub help: Option<String>,
}

type Result<T> = std::result::Result<T, TypeError>;

impl TypeError {
    /// A `Span` only means something inside the file it came from, so every
    /// error leaving this pass is tagged with the module that produced it.
    fn in_module(self, module: &str) -> Diagnostic {
        let mut diag = Diagnostic::new(self.message, self.span, module);
        diag.help = self.help;
        diag
    }

    /// The same error re-pointed at another span, for the sites that only
    /// learn the real location after an inner resolution succeeds: a parameter
    /// type resolved at a call site belongs to the call, not to the
    /// declaration it was written in.
    fn at(mut self, span: Span) -> TypeError {
        self.span = span;
        self
    }
}


mod env;
pub(crate) use env::*;

mod ctx;
pub(crate) use ctx::*;

mod exports;
pub(crate) use exports::*;

mod builtin_table;
pub(crate) use builtin_table::*;

mod pd_wiring;
pub(crate) use pd_wiring::*;

mod trait_checks;
pub(crate) use trait_checks::*;

mod fn_values;
pub(crate) use fn_values::*;

mod element_errors;
pub(crate) use element_errors::*;

mod builtin_sigs;
pub(crate) use builtin_sigs::*;

mod const_scope;
pub(crate) use const_scope::*;

mod program;
pub(crate) use program::*;

mod passdown_init;
pub(crate) use passdown_init::*;

mod function;
pub(crate) use function::*;

mod loops_closures;
pub(crate) use loops_closures::*;

mod expr;
pub(crate) use expr::*;

mod ident;
pub(crate) use ident::*;

mod field;
pub(crate) use field::*;

mod generics;
pub(crate) use generics::*;

mod when_try_blocks;
pub(crate) use when_try_blocks::*;

mod asm_compare;
pub(crate) use asm_compare::*;

mod args_coerce;
pub(crate) use args_coerce::*;

mod thread_layout;
pub(crate) use thread_layout::*;

mod claims_impls;
pub(crate) use claims_impls::*;

mod instantiate;
pub(crate) use instantiate::*;

mod method_calls;
pub(crate) use method_calls::*;

mod dyn_conversions;
pub(crate) use dyn_conversions::*;

mod sync_prims;
pub(crate) use sync_prims::*;

mod if_caps;
pub(crate) use if_caps::*;

#[cfg(test)]
#[path = "infer/tests.rs"]
mod tests;

mod const_block;
mod const_expr;
mod signatures;
mod type_params;
mod type_resolve;
