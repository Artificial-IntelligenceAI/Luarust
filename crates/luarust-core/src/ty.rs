//! The types and operators, which outlive the syntax they were written in.

/// How many dimensions an array may have. Three is a shape you can still picture.
pub const MAX_RANK: usize = 3;

/// An array type: what it holds and what shape it is.
///
/// The element is a *scalar*, never another array. That is what keeps this `Copy` and
/// keeps a type from having to point at another one — and it is also the reason no value
/// in this language can contain itself, which turns out to matter more than it looks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ArrayOf {
    /// The element, held as its tag so that a type never points at a type.
    element: u8,
    /// The fixed dimensions, of which `rank` are used.
    dims: [u32; MAX_RANK],
    /// Zero when the array grows, and its length is whatever it has become.
    rank: u8,
}

impl ArrayOf {
    /// A growable array of this element.
    pub fn growable(element: Ty) -> Self {
        ArrayOf { element: element.tag(), dims: [0; MAX_RANK], rank: 0 }
    }

    /// A fixed array of this element and shape. `None` if the shape is more dimensions
    /// than an array may have, or if any of them is zero.
    pub fn fixed(element: Ty, shape: &[u32]) -> Option<Self> {
        if shape.is_empty() || shape.len() > MAX_RANK || shape.contains(&0) {
            return None;
        }
        let mut dims = [0; MAX_RANK];
        dims[..shape.len()].copy_from_slice(shape);
        Some(ArrayOf { element: element.tag(), dims, rank: shape.len() as u8 })
    }

    pub fn element(self) -> Ty {
        Ty::from_tag(self.element).expect("an element tag came from a type")
    }

    /// The fixed dimensions, as many as there are.
    pub fn shape(&self) -> &[u32] {
        &self.dims[..self.rank as usize]
    }

    /// Whether it grows, rather than being one fixed size for ever.
    pub fn grows(self) -> bool {
        self.rank == 0
    }

    /// How many elements a fixed one holds. `None` when it grows.
    pub fn length(self) -> Option<usize> {
        if self.grows() {
            return None;
        }
        Some(self.shape().iter().map(|d| *d as usize).product())
    }
}

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
    /// `array.ui32`, `array.8.ui32`, `array.2x3.ui32`.
    Array(ArrayOf),
}

impl Ty {
    /// A small number standing for this type, for the places that need one: an array's
    /// element, and the chunk format.
    pub fn tag(self) -> u8 {
        match self {
            Ty::B16 => 0,
            Ty::B32 => 1,
            Ty::B64 => 2,
            Ty::B128 => 3,
            Ty::B256 => 4,
            Ty::D32 => 5,
            Ty::D64 => 6,
            Ty::D128 => 7,
            Ty::Er => 8,
            Ty::I8 => 9,
            Ty::I16 => 10,
            Ty::I32 => 11,
            Ty::I64 => 12,
            Ty::U8 => 13,
            Ty::U16 => 14,
            Ty::U32 => 15,
            Ty::U64 => 16,
            Ty::Bool => 17,
            Ty::Str => 18,
            // An array is not a scalar and has no tag; nothing asks one for it.
            Ty::Array(_) => u8::MAX,
        }
    }

    pub fn from_tag(tag: u8) -> Option<Ty> {
        Some(match tag {
            0 => Ty::B16,
            1 => Ty::B32,
            2 => Ty::B64,
            3 => Ty::B128,
            4 => Ty::B256,
            5 => Ty::D32,
            6 => Ty::D64,
            7 => Ty::D128,
            8 => Ty::Er,
            9 => Ty::I8,
            10 => Ty::I16,
            11 => Ty::I32,
            12 => Ty::I64,
            13 => Ty::U8,
            14 => Ty::U16,
            15 => Ty::U32,
            16 => Ty::U64,
            17 => Ty::Bool,
            18 => Ty::Str,
            _ => return None,
        })
    }

    /// The array this is, if it is one.
    pub fn array(self) -> Option<ArrayOf> {
        match self {
            Ty::Array(of) => Some(of),
            _ => None,
        }
    }

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

    /// The type as it is written. An array's name is built, so this hands back an owned
    /// string; [`Ty::word`] is the one for the scalars, which are all names already.
    pub fn written(self) -> String {
        match self {
            Ty::Array(of) => {
                if of.grows() {
                    return format!("array.{}", of.element().word());
                }
                let shape: Vec<String> = of.shape().iter().map(|d| d.to_string()).collect();
                format!("array.{}.{}", shape.join("x"), of.element().word())
            }
            scalar => scalar.word().to_string(),
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            // An array's name depends on what is in it, so it cannot be one of these.
            // `written` is the one that names every type.
            Ty::Array(_) => "array",
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

    pub fn is_float(self) -> bool {
        matches!(self, Ty::B16 | Ty::B32 | Ty::B64 | Ty::B128 | Ty::B256) || self.is_decimal()
    }

    /// The IEEE 754 decimal formats, whose significands are decimal digits.
    pub fn is_decimal(self) -> bool {
        matches!(self, Ty::D32 | Ty::D64 | Ty::D128)
    }

    /// Whether arithmetic and ordering work on it, which is wider than "is a float".
    pub fn is_number(self) -> bool {
        self.is_integer() || self.is_float() || self == Ty::Er
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

/// Joining two conditions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LogicOp {
    And,
    Or,
}

impl LogicOp {
    pub fn word(self) -> &'static str {
        match self {
            LogicOp::And => "and",
            LogicOp::Or => "or",
        }
    }
}
