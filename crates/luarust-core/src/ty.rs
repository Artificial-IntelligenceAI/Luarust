//! The types and operators, which outlive the syntax they were written in.

/// One of Luarust's types.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ty {
    B16,
    B32,
    B64,
    B128,
    B256,
    D32,
    D64,
    D128,
    Er,
    I8,
    I16,
    I32,
    I64,
    U8,
    U16,
    U32,
    U64,
    Bool,
    Str,
}

impl Ty {
    pub fn from_word(word: &str) -> Option<Self> {
        Some(match word {
            "b16" => Ty::B16,
            "b32" => Ty::B32,
            "b64" => Ty::B64,
            "b128" => Ty::B128,
            "b256" => Ty::B256,
            "d32" => Ty::D32,
            "d64" => Ty::D64,
            "d128" => Ty::D128,
            "er" => Ty::Er,
            "i8" => Ty::I8,
            "i16" => Ty::I16,
            "i32" => Ty::I32,
            "i64" => Ty::I64,
            "ui8" => Ty::U8,
            "ui16" => Ty::U16,
            "ui32" => Ty::U32,
            "ui64" => Ty::U64,
            "bool" => Ty::Bool,
            "str" => Ty::Str,
            _ => return None,
        })
    }

    pub fn word(self) -> &'static str {
        match self {
            Ty::B16 => "b16",
            Ty::B32 => "b32",
            Ty::B64 => "b64",
            Ty::B128 => "b128",
            Ty::B256 => "b256",
            Ty::D32 => "d32",
            Ty::D64 => "d64",
            Ty::D128 => "d128",
            Ty::Er => "er",
            Ty::I8 => "i8",
            Ty::I16 => "i16",
            Ty::I32 => "i32",
            Ty::I64 => "i64",
            Ty::U8 => "ui8",
            Ty::U16 => "ui16",
            Ty::U32 => "ui32",
            Ty::U64 => "ui64",
            Ty::Bool => "bool",
            Ty::Str => "str",
        }
    }

    /// Whether iteration 1 can actually run this one.
    pub fn implemented(self) -> bool {
        !matches!(self, Ty::D32 | Ty::D64 | Ty::D128 | Ty::Er)
    }

    pub fn is_float(self) -> bool {
        matches!(self, Ty::B16 | Ty::B32 | Ty::B64 | Ty::B128 | Ty::B256)
    }

    pub fn is_integer(self) -> bool {
        matches!(
            self,
            Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 | Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64
        )
    }

    pub fn is_signed(self) -> bool {
        matches!(self, Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64)
    }

    /// Width in bits, for the integers.
    pub fn int_bits(self) -> Option<u32> {
        Some(match self {
            Ty::I8 | Ty::U8 => 8,
            Ty::I16 | Ty::U16 => 16,
            Ty::I32 | Ty::U32 => 32,
            Ty::I64 | Ty::U64 => 64,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Pow,
    Mod,
}

impl BinOp {
    pub fn word(self) -> &'static str {
        match self {
            BinOp::Add => "+",
            BinOp::Sub => "-",
            BinOp::Mul => "*",
            BinOp::Div => "/",
            BinOp::Pow => "**",
            BinOp::Mod => "mod",
        }
    }
}

/// How two values are being compared.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CmpOp {
    Less,
    Greater,
    Equal,
    /// `</=`
    LessEqual,
    /// `>/=`
    GreaterEqual,
    /// `!=`, `≠`, `not=`
    NotEqual,
}

impl CmpOp {
    pub fn word(self) -> &'static str {
        match self {
            CmpOp::Less => "<",
            CmpOp::Greater => ">",
            CmpOp::Equal => "=",
            CmpOp::LessEqual => "</=",
            CmpOp::GreaterEqual => ">/=",
            CmpOp::NotEqual => "!=",
        }
    }

    /// Whether this one puts values in order, rather than only asking whether they are the
    /// same. `=` and `!=` work on anything; the other four need numbers.
    pub fn orders(self) -> bool {
        !matches!(self, CmpOp::Equal | CmpOp::NotEqual)
    }
}
