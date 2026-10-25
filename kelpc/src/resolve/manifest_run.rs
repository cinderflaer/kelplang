//! `[run]` (issue #303): the boot table that `kelpc boot` reads. It names the
//! QEMU machine a freestanding example boots on, the serial lines the boot must
//! print, and the two options the examples use (`ramfb`, `smp`). It replaces the
//! per-example `run.sh` scripts, which each repeated the same QEMU command.

use super::*;

/// The QEMU machine a `[run]` table boots on. Each names the machine, CPU and
/// base flags the example scripts used before the table existed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BootMachine {
    Riscv64,
    Aarch64,
    X86_64,
}

impl BootMachine {
    fn from_name(name: &str) -> Option<BootMachine> {
        match name {
            "riscv64" => Some(BootMachine::Riscv64),
            "aarch64" => Some(BootMachine::Aarch64),
            "x86_64" => Some(BootMachine::X86_64),
            _ => None,
        }
    }

    /// The QEMU program for this machine.
    pub(crate) fn binary(self) -> &'static str {
        match self {
            BootMachine::Riscv64 => "qemu-system-riscv64",
            BootMachine::Aarch64 => "qemu-system-aarch64",
            BootMachine::X86_64 => "qemu-system-x86_64",
        }
    }

    /// The flags every boot on this machine passes, before `-kernel`.
    pub(crate) fn base_args(self) -> &'static [&'static str] {
        match self {
            // `-bios none` starts the image at 0x80000000 with no firmware in
            // front of it, which is why the linker script puts `_start` there.
            BootMachine::Riscv64 => &[
                "-machine", "virt", "-bios", "none", "-nographic", "-no-reboot", "-serial", "mon:stdio",
            ],
            // The Xen PVH loader path SeaBIOS takes for `-kernel`; `qemu64` is
            // the core the examples' target machine is built for.
            BootMachine::X86_64 => &[
                "-M", "q35", "-cpu", "qemu64", "-nographic", "-serial", "mon:stdio", "-no-reboot",
            ],
            BootMachine::Aarch64 => &["-M", "virt", "-cpu", "cortex-a57", "-nographic", "-serial", "mon:stdio"],
        }
    }
}

/// A finished `[run]` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunDecl {
    pub(crate) machine: BootMachine,
    /// The serial lines the boot must print, each whole, in no particular
    /// order. The last one is also the line the boot stops at.
    pub(crate) expect: Vec<String>,
    /// `ramfb = true` adds `-device ramfb`, the framebuffer a Polyp boot draws to.
    pub(crate) ramfb: bool,
    /// `smp = N` adds `-smp N` when N is above 1.
    pub(crate) smp: u32,
}

/// A `[run]` table while its lines are read. A key that is absent stays `None`,
/// so a missing required key and a repeated one are both caught.
#[derive(Debug, Default)]
pub(crate) struct RunDraft {
    qemu: Option<BootMachine>,
    expect: Option<Vec<String>>,
    ramfb: Option<bool>,
    smp: Option<u32>,
}

/// `"text"`: the text between the two double quotes, with nothing after them.
fn single_quoted(value: &str) -> Option<&str> {
    value.strip_prefix('"').and_then(|v| v.strip_suffix('"')).filter(|v| !v.contains('"'))
}

/// `"a", "b"`, the items of a bracketed list, in order. A line comment is cut
/// before this runs, so a `#` cannot appear in an expected line.
fn quoted_list(inner: &str) -> Option<Vec<String>> {
    let mut items = Vec::new();
    let mut rest = inner.trim_start();
    while !rest.is_empty() {
        let body = rest.strip_prefix('"')?;
        let close = body.find('"')?;
        items.push(body[..close].to_string());
        rest = body[close + 1..].trim_start();
        if let Some(after) = rest.strip_prefix(',') {
            rest = after.trim_start();
        } else if !rest.is_empty() {
            return None;
        }
    }
    Some(items)
}

/// The value of `expect`: one double-quoted line, or a bracketed list of them.
fn expect_value(value: &str, at: Span) -> Result<Vec<String>, Diagnostic> {
    let lines = if let Some(one) = single_quoted(value) {
        Some(vec![one.to_string()])
    } else if let Some(inner) = value.strip_prefix('[').and_then(|v| v.strip_suffix(']')) {
        quoted_list(inner)
    } else {
        None
    };
    match lines {
        Some(lines) if lines.is_empty() => Err(Diagnostic::new(
            format!("'[run] expect' names no line to check in {MANIFEST_NAME}; name at least one"),
            at,
            "",
        )),
        Some(lines) => Ok(lines),
        None => Err(Diagnostic::new(
            format!(
                "'[run] expect' must be a double-quoted string or a bracketed list of double-quoted strings in {MANIFEST_NAME}, found '{value}'"
            ),
            at,
            "",
        )),
    }
}

/// One `key = value` line inside `coral.toml`'s `[run]` section (#303).
pub(crate) fn parse_run_line(draft: &mut RunDraft, line: &str, at: Span) -> Result<(), Diagnostic> {
    let Some((key, value)) = line.split_once('=') else {
        return Err(Diagnostic::new(
            format!("expected 'key = value' in {MANIFEST_NAME}'s [run] section, found '{line}'"),
            at,
            "",
        ));
    };
    let key = key.trim();
    let value = value.trim();
    let repeated = || {
        Diagnostic::new(format!("'[run] {key}' is set more than once in {MANIFEST_NAME}"), at, "")
    };
    match key {
        "qemu" => {
            if draft.qemu.is_some() {
                return Err(repeated());
            }
            let machine = single_quoted(value).and_then(BootMachine::from_name);
            let Some(machine) = machine else {
                return Err(Diagnostic::new(
                    format!(
                        "'[run] qemu' must name 'riscv64', 'aarch64' or 'x86_64' in {MANIFEST_NAME}, found '{value}'"
                    ),
                    at,
                    "",
                ));
            };
            draft.qemu = Some(machine);
        }
        "expect" => {
            if draft.expect.is_some() {
                return Err(repeated());
            }
            draft.expect = Some(expect_value(value, at)?);
        }
        "ramfb" => {
            if draft.ramfb.is_some() {
                return Err(repeated());
            }
            let flag = match value {
                "true" => true,
                "false" => false,
                _ => {
                    return Err(Diagnostic::new(
                        format!("'[run] ramfb' must be true or false in {MANIFEST_NAME}, found '{value}'"),
                        at,
                        "",
                    ));
                }
            };
            draft.ramfb = Some(flag);
        }
        "smp" => {
            if draft.smp.is_some() {
                return Err(repeated());
            }
            let count = value.parse::<u32>().ok().filter(|n| *n >= 1);
            let Some(count) = count else {
                return Err(Diagnostic::new(
                    format!(
                        "'[run] smp' must be a whole number of at least 1 in {MANIFEST_NAME}, found '{value}'"
                    ),
                    at,
                    "",
                ));
            };
            draft.smp = Some(count);
        }
        other => {
            return Err(Diagnostic::new(
                format!(
                    "unknown key '{other}' in {MANIFEST_NAME}'s [run] section: the table takes 'qemu', 'expect', 'ramfb' and 'smp'"
                ),
                at,
                "",
            ));
        }
    }
    Ok(())
}

/// The finished table once every line is read: `qemu` and `expect` are required.
pub(crate) fn finish_run(draft: RunDraft, at: Span) -> Result<RunDecl, Diagnostic> {
    let Some(machine) = draft.qemu else {
        return Err(Diagnostic::new(format!("'[run] qemu' is required in {MANIFEST_NAME}'s [run] table"), at, ""));
    };
    let Some(expect) = draft.expect else {
        return Err(Diagnostic::new(format!("'[run] expect' is required in {MANIFEST_NAME}'s [run] table"), at, ""));
    };
    Ok(RunDecl {
        machine,
        expect,
        ramfb: draft.ramfb.unwrap_or(false),
        smp: draft.smp.unwrap_or(1),
    })
}
