//! The optimization level `kelpc build -O<n>` selects (MK7 S6,
//! the record in `kelp wiki page meta/milestones.md`).
//!
//! One level is one level for both backends, which is what makes the
//! differential sweep a comparison *at a level* rather than two runs that
//! happen to agree. A level therefore carries three spellings and they are
//! built in one place so they cannot drift:
//!
//! - the module pass pipeline LLVM runs, `default<O<n>>` (`LLVM S6` adds it to
//!   `codegen_llvm::compile`);
//! - the target machine's codegen level, the `LLVMCodeGenOptLevel` the object
//!   is written with;
//! - the `-O<n>` flag the C backend's `cc` invocation, the per-program C object
//!   and the final link are given.
//!
//! The default is `-O2` because the C backend has always run `cc -O2`: an
//! unset flag must not change what the default build produces. `-O0` is the
//! dev build §4.3 asks for; `-O0` also writes the textual IR before any pass
//! runs, so the artifact beside the binary is the lowering either way (see
//! `codegen_llvm::compile`).
//!
//! `-Os` is offered because both `cc` and LLVM accept it. `-Oz` is not: it is
//! a clang-only spelling and the C backend's `cc` may be gcc, whose refusal
//! would make the `cc` calls and the LLVM backend unable to speak one vocabulary
//! (`-Oz` is out for that reason, §4.3).

use llvm_sys::target_machine::LLVMCodeGenOptLevel;

/// One optimization level, in the vocabulary both backends share.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptLevel {
    O0,
    O1,
    O2,
    O3,
    /// Optimize for size. The spelling is `-Os` on both sides; LLVM's pipeline
    /// is `default<Os>` and its codegen level is `Default`, which is what clang
    /// uses for `-Os`.
    Os,
}

impl OptLevel {
    /// What an unset `-O` means. `-O2` is today's C backend's `cc -O2`, so the
    /// default build does not regress when the flag is absent.
    pub const DEFAULT: OptLevel = OptLevel::O2;

    /// The vocabulary, in the order the diagnostics list it.
    pub const ALL: &'static [OptLevel] = &[
        OptLevel::O0,
        OptLevel::O1,
        OptLevel::O2,
        OptLevel::O3,
        OptLevel::Os,
    ];

    /// The spelling without the `-O`, used by the round-trip test below; the
    /// diagnostic path spells a level with `o_flag`.
    #[cfg(test)]
    pub fn name(self) -> &'static str {
        match self {
            OptLevel::O0 => "0",
            OptLevel::O1 => "1",
            OptLevel::O2 => "2",
            OptLevel::O3 => "3",
            OptLevel::Os => "s",
        }
    }

    /// The flag spelled the way `cc` and the command line take it.
    pub fn o_flag(self) -> &'static str {
        match self {
            OptLevel::O0 => "-O0",
            OptLevel::O1 => "-O1",
            OptLevel::O2 => "-O2",
            OptLevel::O3 => "-O3",
            OptLevel::Os => "-Os",
        }
    }

    /// The LLVM new-pass-manager pipeline string `LLVMRunPasses` is handed,
    /// which is what chooses the set of passes and their order.
    pub fn pipeline(self) -> &'static str {
        match self {
            OptLevel::O0 => "default<O0>",
            OptLevel::O1 => "default<O1>",
            OptLevel::O2 => "default<O2>",
            OptLevel::O3 => "default<O3>",
            OptLevel::Os => "default<Os>",
        }
    }

    /// The codegen level the `TargetMachine` is created with, which chooses
    /// instruction selection, scheduling and register allocation rather than
    /// the module passes.
    pub fn codegen_level(self) -> LLVMCodeGenOptLevel {
        match self {
            // No module passes and `CodeGenOpt::None`: the shape `for_host`
            // created before this milestone, and the cheap dev build §4.3 asks
            // for.
            OptLevel::O0 => LLVMCodeGenOptLevel::LLVMCodeGenLevelNone,
            OptLevel::O1 => LLVMCodeGenOptLevel::LLVMCodeGenLevelLess,
            OptLevel::O2 => LLVMCodeGenOptLevel::LLVMCodeGenLevelDefault,
            OptLevel::O3 => LLVMCodeGenOptLevel::LLVMCodeGenLevelAggressive,
            // clang maps `-Os` to the `Default` codegen level with the `Os`
            // pipeline, the same split this carries.
            OptLevel::Os => LLVMCodeGenOptLevel::LLVMCodeGenLevelDefault,
        }
    }

    /// Whether a pass pipeline ran over the module, which is what a debugger
    /// reads out of the compile unit's `isOptimized` flag. False only at
    /// `-O0`: the IR the lowering wrote is the IR the target machine lowers.
    pub fn is_optimized(self) -> bool {
        self != OptLevel::O0
    }

    /// The whole vocabulary as a diagnostic name, `-O0, -O1, -O2, -O3, -Os`.
    fn expected_list() -> String {
        let names: Vec<&str> = Self::ALL.iter().map(|l| l.o_flag()).collect();
        names.join(", ")
    }

    /// Parses one `-O` argument. `arg` is the spelling as it reached the
    /// command line, including the `-O`, because the diagnostic names exactly
    /// what the user typed.
    ///
    /// The error text is the point of this function: an out-of-range level must not read as a default,
    /// and `-Oz` must say why it is refused rather than read as a typo.
    pub fn parse(arg: &str) -> Result<OptLevel, String> {
        let suffix = arg.strip_prefix("-O").unwrap_or("");
        match suffix {
            "0" => Ok(OptLevel::O0),
            "1" => Ok(OptLevel::O1),
            "2" => Ok(OptLevel::O2),
            "3" => Ok(OptLevel::O3),
            "s" => Ok(OptLevel::Os),
            "z" => Err(format!(
                "'-Oz' is refused: it is a clang-only spelling and the C backend's \
                 `cc` may be gcc, so a level has to be one both accept; use '-Os' \
                 for size, or one of: {}",
                Self::expected_list()
            )),
            _ => Err(format!(
                "unknown optimization level '{arg}'; expected one of: {}",
                Self::expected_list()
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default must not regress the C backend's `cc -O2`.
    #[test]
    fn the_default_is_o2() {
        assert_eq!(OptLevel::DEFAULT, OptLevel::O2);
        assert_eq!(OptLevel::DEFAULT.o_flag(), "-O2");
    }

    /// Every level that can be selected round-trips through the spelling the
    /// command line uses, so the flag a caller writes is the level it gets.
    #[test]
    fn every_level_round_trips_through_its_flag() {
        for level in OptLevel::ALL {
            assert_eq!(OptLevel::parse(level.o_flag()).unwrap(), *level);
            assert_eq!(level.o_flag(), format!("-O{}", level.name()));
        }
    }

    /// The pipeline and the codegen level are two spellings of the same level,
    /// so no level can carry one and not the other.
    #[test]
    fn each_level_has_a_pipeline_a_codegen_level_and_a_flag() {
        assert_eq!(OptLevel::O0.pipeline(), "default<O0>");
        assert_eq!(OptLevel::O3.pipeline(), "default<O3>");
        assert_eq!(OptLevel::Os.pipeline(), "default<Os>");
        assert_eq!(
            OptLevel::O0.codegen_level(),
            LLVMCodeGenOptLevel::LLVMCodeGenLevelNone
        );
        assert_eq!(
            OptLevel::O3.codegen_level(),
            LLVMCodeGenOptLevel::LLVMCodeGenLevelAggressive
        );
    }

    /// Only `-O0` runs no pipeline; every other level tells a debugger the
    /// module was optimized.
    #[test]
    fn only_o0_is_unoptimized() {
        assert!(!OptLevel::O0.is_optimized());
        for level in [OptLevel::O1, OptLevel::O2, OptLevel::O3, OptLevel::Os] {
            assert!(level.is_optimized(), "{level:?}");
        }
    }

    /// `-Oz` is refused with the reason, not read as a default and not reported
    /// as an unknown spelling: the two messages mean different things.
    #[test]
    fn oz_is_refused_with_the_reason() {
        let e = OptLevel::parse("-Oz").unwrap_err();
        assert!(e.contains("clang-only"), "{e}");
        assert!(e.contains("-Os"), "{e}");
        assert!(!e.contains("unknown"), "{e}");
    }

    /// An out-of-range or unparseable level is a diagnostic that lists the
    /// vocabulary, never a silent fallback to the default.
    #[test]
    fn an_unknown_level_lists_the_vocabulary() {
        for arg in ["-O4", "-O", "-Ox", "-O22"] {
            let e = OptLevel::parse(arg).unwrap_err();
            assert_eq!(
                e,
                format!(
                    "unknown optimization level '{arg}'; expected one of: {}",
                    "-O0, -O1, -O2, -O3, -Os"
                ),
                "{arg}"
            );
        }
    }
}
