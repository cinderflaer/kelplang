use super::*;

/// One `CfnSpec` from its three parts.
pub(crate) fn spec<'a>(symbol: &str, ret: Type<'a>, params: &[Type<'a>]) -> CfnSpec<'a> {
    CfnSpec {
        symbol: symbol.to_string(),
        ret,
        params: params.to_vec(),
    }
}

/// The list's three fields, in the order `write_container_typedef` declares
/// them: the buffer, the length, the capacity.
pub(crate) const L_DATA: u32 = 0;
pub(crate) const L_LEN: u32 = 1;
pub(crate) const L_CAP: u32 = 2;

/// `List#sort`'s introsort: a partition at or below this many elements
/// finishes with insertion sort instead of recursing further. 16-32 is the
/// textbook range (it is what libstdc++'s own introsort threshold, 16, comes
/// from); 32, the top of that range, is what benchmarking actually picked
/// here: `median3`, `partition` and `cmp` are separate emitted functions
/// rather than something the source-level optimizer can inline into one
/// quicksort loop, so each partition step pays real call overhead on top of
/// the comparisons, and a partition small enough to hand to insertion sort
/// skips that call chain entirely. Measured on
/// `benchmarks/sorting/sort.kp` (n = 5,000,000): 16 landed at 254ms, 24 at
/// 243ms, 32 at 238ms, with diminishing returns above that (a further push to
/// 64 was only 227ms) - 32 stays inside the well-tested range while clearing
/// the 250ms target with headroom, rather than chasing the last few
/// milliseconds outside it. Shared with the earlier emitter's
/// `KELP_SORT_INSERTION_CUTOFF`.
pub(crate) const KELP_SORT_INSERTION_CUTOFF: u64 = 32;

/// `List#sort`'s partition, branchless block size: mirrors the earlier emitter's
/// `KELP_SORT_BLOCK` exactly, same value and same reasoning (pdqsort's own
/// published block size, not something benchmarked here). See that constant's
/// doc comment for the full explanation of the technique; this backend hand-
/// builds the identical algorithm as LLVM IR instead of emitting source text for
/// it, so the two copies must keep agreeing on the value, checked by the
/// differential test suite.
pub(crate) const KELP_SORT_BLOCK: u64 = 64;

impl<'a> Gen<'a> {
    // ---- the derived clone and order of a datac/enum ---------------------

}

mod part5_4;

mod part5_3;

mod part5_2;

mod part5_1;
