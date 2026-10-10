//! The ABI shared by the Rust runtime and the code `kelpc` generates: the
//! `#[repr(C)]` structs, their layout table, and the C prototypes of every
//! exported function.
//!
//! `LAYOUTS` and `PROTOTYPES` are the single source of truth. The IR struct
//! bodies, the C typedefs and the C prelude are generated from them (wired in a
//! later work package), so the compiler and the runtime cannot disagree about a
//! field.

use core::ffi::{c_char, c_int};
use core::mem::{align_of, offset_of, size_of};

/// A growable byte buffer. Mirrors `kelp_Bytes` in `kelpc/runtime/kelp_rt.c`.
///
/// `cap` counts the usable bytes at `data`, excluding the NUL slot: the
/// allocation is always `cap + 1` bytes and `data[len]` is kept 0.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct Bytes {
    pub data: *mut u8,
    pub len: usize,
    pub cap: usize,
}

/// A growable string. Same three fields and invariants as [`Bytes`]. Mirrors
/// `kelp_String`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct String {
    pub data: *mut u8,
    pub len: usize,
    pub cap: usize,
}

/// A borrowed string view. Mirrors `kelp_View`.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct View {
    pub ptr: *const c_char,
    pub len: usize,
}

const _: () = {
    assert!(size_of::<Bytes>() == 24);
    assert!(align_of::<Bytes>() == 8);
    assert!(offset_of!(Bytes, len) == 8);
    assert!(offset_of!(Bytes, cap) == 16);
    assert!(size_of::<String>() == 24);
    assert!(align_of::<String>() == 8);
    assert!(offset_of!(String, len) == 8);
    assert!(offset_of!(String, cap) == 16);
    assert!(size_of::<View>() == 16);
    assert!(align_of::<View>() == 8);
    assert!(offset_of!(View, len) == 8);
};

/// The IR width of one struct field. These are the LLVM types in use today.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldKind {
    Ptr,
    I64,
    I32,
    I8,
    I1,
}

/// One field of a layout: its name, its IR width, and its C spelling.
#[derive(Clone, Copy, Debug)]
pub struct Field {
    pub name: &'static str,
    pub kind: FieldKind,
    pub c_type: &'static str,
}

/// One runtime struct: its C spelling (`kelp_Bytes`) and its fields in order.
#[derive(Clone, Copy, Debug)]
pub struct Layout {
    pub name: &'static str,
    pub fields: &'static [Field],
}

const BYTE_FIELDS: &[Field] = &[
    Field {
        name: "data",
        kind: FieldKind::Ptr,
        c_type: "unsigned char*",
    },
    Field {
        name: "len",
        kind: FieldKind::I64,
        c_type: "size_t",
    },
    Field {
        name: "cap",
        kind: FieldKind::I64,
        c_type: "size_t",
    },
];

const VIEW_FIELDS: &[Field] = &[
    Field {
        name: "ptr",
        kind: FieldKind::Ptr,
        c_type: "const char*",
    },
    Field {
        name: "len",
        kind: FieldKind::I64,
        c_type: "size_t",
    },
];

/// The layout table. Row order is the field order of each struct.
pub const LAYOUTS: &[Layout] = &[
    Layout { name: "kelp_Bytes", fields: BYTE_FIELDS },
    Layout { name: "kelp_String", fields: BYTE_FIELDS },
    Layout { name: "kelp_View", fields: VIEW_FIELDS },
];

/// The ABI version of this runtime. Bumped when an exported signature or a
/// layout changes.
pub const ABI_VERSION: c_int = 1;

/// The C prototype of every exported function, as the generated prelude spells
/// it. The first column is the symbol name.
pub const PROTOTYPES: &[(&str, &str)] = &[(
    "kelp_rt_abi_version",
    "int kelp_rt_abi_version(void);",
)];

/// Reports [`ABI_VERSION`]. Lets a program check that the runtime it was linked
/// with is the one it was compiled against.
///
/// # Safety
/// Takes no arguments and reads no memory; it is `unsafe` only because every
/// exported runtime function is.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn kelp_rt_abi_version() -> c_int {
    ABI_VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every exported name in `PROTOTYPES` must name a real function. Taking
    /// the function's address fails to compile if a symbol is renamed or
    /// removed, and the table row must carry the same name.
    #[test]
    fn exported_symbol_names_exist() {
        // Taking the address fails to compile if the export is renamed or
        // removed; the table check catches a prototype row left behind.
        let f: unsafe extern "C" fn() -> c_int = kelp_rt_abi_version;
        assert!(PROTOTYPES.iter().any(|(name, _)| *name == "kelp_rt_abi_version"));
        assert_eq!(unsafe { f() }, ABI_VERSION);
    }

    #[test]
    fn layout_table_matches_structs() {
        let bytes = LAYOUTS.iter().find(|l| l.name == "kelp_Bytes").unwrap();
        assert_eq!(bytes.fields.len(), 3);
        assert_eq!(bytes.fields[1].name, "len");
        let view = LAYOUTS.iter().find(|l| l.name == "kelp_View").unwrap();
        assert_eq!(view.fields.len(), 2);
        assert_eq!(size_of::<View>(), 16);
    }
}
