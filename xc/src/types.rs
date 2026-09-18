#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    I32,
    I64,
    U32,
    U64,
    F32,
    F64,
    Bool,
    Char,
    Unit,
    Str,
}

impl Ty {
    pub fn from_name(name: &str) -> Option<Ty> {
        Some(match name {
            "i32" => Ty::I32,
            "i64" => Ty::I64,
            "u32" => Ty::U32,
            "u64" => Ty::U64,
            "f32" => Ty::F32,
            "f64" => Ty::F64,
            "bool" => Ty::Bool,
            "char" => Ty::Char,
            "unit" => Ty::Unit,
            "str" => Ty::Str,
            _ => return None,
        })
    }

    pub fn is_numeric(&self) -> bool {
        matches!(
            self,
            Ty::I32 | Ty::I64 | Ty::U32 | Ty::U64 | Ty::F32 | Ty::F64
        )
    }

    pub fn is_integer(&self) -> bool {
        matches!(self, Ty::I32 | Ty::I64 | Ty::U32 | Ty::U64)
    }

    pub fn is_float(&self) -> bool {
        matches!(self, Ty::F32 | Ty::F64)
    }

    pub fn c_type(&self) -> &'static str {
        match self {
            Ty::I32 => "int32_t",
            Ty::I64 => "int64_t",
            Ty::U32 => "uint32_t",
            Ty::U64 => "uint64_t",
            Ty::F32 => "float",
            Ty::F64 => "double",
            Ty::Bool => "bool",
            Ty::Char => "char",
            Ty::Unit => "void",
            Ty::Str => "const char*",
        }
    }

    pub fn display(&self) -> &'static str {
        match self {
            Ty::I32 => "i32",
            Ty::I64 => "i64",
            Ty::U32 => "u32",
            Ty::U64 => "u64",
            Ty::F32 => "f32",
            Ty::F64 => "f64",
            Ty::Bool => "bool",
            Ty::Char => "char",
            Ty::Unit => "unit",
            Ty::Str => "str",
        }
    }
}

#[derive(Debug, Clone)]
pub struct FnSig {
    pub params: Vec<Ty>,
    pub ret: Ty,
}
