//! The calling thread's cached registry index, plus one (zero when the thread
//! is not attached). This is the thread-local slot of section 7 of the design.
//!
//! `#[thread_local]` is unstable and this crate is `no_std`, so the slot is a
//! `.tbss` symbol defined in assembly and read with the local-exec TLS model,
//! which is what `__thread` compiles to in an executable. Only the owning
//! thread writes its own slot (attach sets it, detach clears it), so a copy can
//! never describe another thread. Other targets keep the pthread lookup only.
#![allow(unsafe_op_in_unsafe_fn)]

#[cfg(target_arch = "x86_64")]
core::arch::global_asm!(
    ".pushsection .tbss,\"awT\",@nobits",
    ".p2align 3",
    ".globl kelp_gc_tidx",
    ".type kelp_gc_tidx, @object",
    "kelp_gc_tidx:",
    ".zero 8",
    ".size kelp_gc_tidx, 8",
    ".popsection",
);

/// Read this thread's cached index plus one. Zero when not attached.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn get() -> usize {
    let v: usize;
    core::arch::asm!(
        "mov {0}, qword ptr fs:[kelp_gc_tidx@tpoff]",
        out(reg) v,
        options(nostack, readonly, preserves_flags),
    );
    v
}

/// Set this thread's cached index plus one (zero clears it). Owning thread only.
#[cfg(target_arch = "x86_64")]
#[inline(always)]
pub unsafe fn set(v: usize) {
    core::arch::asm!(
        "mov qword ptr fs:[kelp_gc_tidx@tpoff], {0}",
        in(reg) v,
        options(nostack, preserves_flags),
    );
}

#[cfg(not(target_arch = "x86_64"))]
pub unsafe fn get() -> usize {
    0
}

#[cfg(not(target_arch = "x86_64"))]
pub unsafe fn set(_v: usize) {}
