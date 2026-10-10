//! The Kelp runtime, in Rust: stage 1 of removing C from the compiler.
//!
//! The crate builds as a `no_std` static library (`libkelp_rt.a`, linked into
//! LLVM-built programs) and as an rlib (read by `kelpc` for the ABI layout
//! table in [`abi`]). Every function a generated program calls is a
//! `#[unsafe(no_mangle)] pub unsafe extern "C"` item; a private helper is a
//! plain `fn`, so it is never exported.
//!
//! The C runtime `kelpc/runtime/kelp_rt.c` stays in place until its consumers
//! are gone. Both runtimes must behave the same; the differential run is the
//! check.

#![cfg_attr(not(test), no_std)]

pub mod abi;
pub mod alloc;
pub mod cleanup;
pub mod panic;
pub mod env;
pub mod io;
pub mod net;
pub mod proc;
