//! Where arrays live.
//!
//! An array is **not** a run of `Value`s. A `Value` is twenty-four bytes and carries its
//! own type, which for an array is a type the array already stated — so a thousand `ui8`s
//! as a run of values would be twenty-four thousand bytes to hold a thousand, each one
//! labelled with a label that could not have said anything else.
//!
//! So the elements are stored packed, by width: a `ui8` array is a run of bytes, a `b64`
//! array is a run of eight-byte words. A thousand `ui8`s is a thousand bytes. Beyond the
//! memory, that is what makes indexing *arithmetic* — element `n` is at `base + n × width`
//! — which is one machine instruction rather than a call back here to open a box.
//!
//! A value holds a **handle** into this, and nothing else. That keeps `Value` exactly the
//! size and shape it was: adding a reference-counted variant to it cost the VM fifteen
//! percent in drop glue alone, for values that were not even arrays.

use crate::Ty;
use crate::value::{Bits, Value};
use luarust_num::Exact;
use std::cell::RefCell;
use std::rc::Rc;

/// The elements, laid out by how wide one is.
///
/// Every type the language has is one of these. Which one is decided once, when the array
/// is made, from a type that cannot change afterwards.
#[derive(Clone, Debug)]
pub enum Store {
    /// `bool`, `i8`, `ui8`.
    Byte(Vec<u8>),
    /// `b16`, `i16`, `ui16`.
    Half(Vec<u16>),
    /// `b32`, `d32`, `i32`, `ui32`, and a handle to another array.
    Word(Vec<u32>),
    /// `b64`, `d64`, `i64`, `ui64`.
    Long(Vec<u64>),
    /// `b128`, `b256`, `d128` — the ones with no machine width.
    Wide(Vec<Bits>),
    /// `str`, which is shared rather than copied.
    Text(Vec<Rc<str>>),
    /// `er`, likewise.
    Exact(Vec<Rc<Exact>>),
}

impl Store {
    /// How many elements it holds.
    pub fn len(&self) -> usize {
        match self {
            Store::Byte(v) => v.len(),
            Store::Half(v) => v.len(),
            Store::Word(v) => v.len(),
            Store::Long(v) => v.len(),
            Store::Wide(v) => v.len(),
            Store::Text(v) => v.len(),
            Store::Exact(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// How many bytes one element takes, for anyone counting the footprint.
    pub fn width(&self) -> usize {
        match self {
            Store::Byte(_) => 1,
            Store::Half(_) => 2,
            Store::Word(_) => 4,
            Store::Long(_) => 8,
            Store::Wide(_) => std::mem::size_of::<Bits>(),
            Store::Text(_) => std::mem::size_of::<Rc<str>>(),
            Store::Exact(_) => std::mem::size_of::<Rc<Exact>>(),
        }
    }
}

/// One array on the heap.
#[derive(Clone, Debug)]
pub struct Array {
    /// What it holds. An empty array still knows, which is why this is here and not
    /// worked out from the contents.
    pub element: Ty,
    pub store: Store,
}

impl Array {
    pub fn len(&self) -> usize {
        self.store.len()
    }

    pub fn is_empty(&self) -> bool {
        self.store.is_empty()
    }

    /// How many bytes the elements take, not counting the array itself.
    pub fn bytes(&self) -> usize {
        self.store.len() * self.store.width()
    }
}

thread_local! {
    /// Every array the program has made.
    ///
    /// A handle is an index into this, so a value that holds an array holds four bytes
    /// and no destructor. Nothing is freed yet — that is the collector's job, and this is
    /// the heap it will collect.
    static HEAP: RefCell<Vec<Array>> = const { RefCell::new(Vec::new()) };
}

/// Forget every array. Called when a program begins, so one run never sees another's.
pub fn clear() {
    HEAP.with(|heap| heap.borrow_mut().clear());
}

/// How many arrays are alive, and how many bytes their elements take.
pub fn footprint() -> (usize, usize) {
    HEAP.with(|heap| {
        let heap = heap.borrow();
        (heap.len(), heap.iter().map(Array::bytes).sum())
    })
}

/// An empty store for this element type, ready to be filled.
///
/// The shared kinds start every element pointing at one zero and one empty string, which
/// is the point of their being shared: a thousand `er` zeros is a thousand pointers to
/// one zero, not a thousand zeros.
#[allow(clippy::rc_clone_in_vec_init)]
fn store_for(element: Ty, len: usize) -> Store {
    match element {
        Ty::Bool | Ty::I8 | Ty::U8 => Store::Byte(vec![0; len]),
        Ty::B16 | Ty::I16 | Ty::U16 => Store::Half(vec![0; len]),
        Ty::B32 | Ty::D32 | Ty::I32 | Ty::U32 | Ty::Array(_) => Store::Word(vec![0; len]),
        Ty::B64 | Ty::D64 | Ty::I64 | Ty::U64 => Store::Long(vec![0; len]),
        Ty::B128 | Ty::B256 | Ty::D128 => Store::Wide(vec![Bits::ZERO; len]),
        Ty::Str => Store::Text(vec![Rc::from(""); len]),
        Ty::Er => Store::Exact(vec![Rc::new(Exact::zero()); len]),
    }
}

/// Make an array of this many elements, every one of them `fill`.
pub fn make(element: Ty, len: usize, fill: &Value) -> u32 {
    let mut array = Array { element, store: store_for(element, len) };
    for at in 0..len {
        write(&mut array, at, fill);
    }
    HEAP.with(|heap| {
        let mut heap = heap.borrow_mut();
        heap.push(array);
        (heap.len() - 1) as u32
    })
}

/// Make an array holding exactly these, in order.
pub fn of(element: Ty, items: &[Value]) -> u32 {
    let mut array = Array { element, store: store_for(element, items.len()) };
    for (at, item) in items.iter().enumerate() {
        write(&mut array, at, item);
    }
    HEAP.with(|heap| {
        let mut heap = heap.borrow_mut();
        heap.push(array);
        (heap.len() - 1) as u32
    })
}

/// The value an array holds as its handle.
pub fn handle(ty: Ty, index: u32) -> Value {
    Value::Num { ty, bits: u64::from(index) }
}

/// How long an array is.
pub fn length(index: u32) -> usize {
    HEAP.with(|heap| heap.borrow()[index as usize].len())
}

/// What an array holds.
pub fn element_of(index: u32) -> Ty {
    HEAP.with(|heap| heap.borrow()[index as usize].element)
}

/// Element `at`, as a value. `None` when there is no such element.
pub fn read(index: u32, at: usize) -> Option<Value> {
    HEAP.with(|heap| {
        let heap = heap.borrow();
        let array = &heap[index as usize];
        if at >= array.len() {
            return None;
        }
        Some(load(array, at))
    })
}

/// Put a value in element `at`. `false` when there is no such element.
pub fn store(index: u32, at: usize, value: &Value) -> bool {
    HEAP.with(|heap| {
        let mut heap = heap.borrow_mut();
        let array = &mut heap[index as usize];
        if at >= array.len() {
            return false;
        }
        write(array, at, value);
        true
    })
}

/// Add one to the end. Only a growable array should be asked.
pub fn push(index: u32, value: &Value) {
    HEAP.with(|heap| {
        let mut heap = heap.borrow_mut();
        let array = &mut heap[index as usize];
        match &mut array.store {
            Store::Byte(v) => v.push(0),
            Store::Half(v) => v.push(0),
            Store::Word(v) => v.push(0),
            Store::Long(v) => v.push(0),
            Store::Wide(v) => v.push(Bits::ZERO),
            Store::Text(v) => v.push(Rc::from("")),
            Store::Exact(v) => v.push(Rc::new(Exact::zero())),
        }
        let last = array.len() - 1;
        write(array, last, value);
    });
}

/// One element, packed, back into a value of the array's element type.
fn load(array: &Array, at: usize) -> Value {
    let ty = array.element;
    match &array.store {
        Store::Byte(v) => {
            if ty == Ty::Bool {
                Value::Bool(v[at] != 0)
            } else {
                Value::Num { ty, bits: u64::from(v[at]) }
            }
        }
        Store::Half(v) => Value::Num { ty, bits: u64::from(v[at]) },
        Store::Word(v) => Value::Num { ty, bits: u64::from(v[at]) },
        Store::Long(v) => Value::Num { ty, bits: v[at] },
        Store::Wide(v) => Value::Wide { ty, bits: Box::new(v[at]) },
        Store::Text(v) => Value::Str(Rc::clone(&v[at])),
        Store::Exact(v) => Value::Exact(Rc::clone(&v[at])),
    }
}

/// A value into one element, packed.
fn write(array: &mut Array, at: usize, value: &Value) {
    match &mut array.store {
        Store::Byte(v) => {
            v[at] = match value {
                Value::Bool(truth) => u8::from(*truth),
                Value::Num { bits, .. } => *bits as u8,
                other => unreachable!("a byte array holds {other:?}"),
            }
        }
        Store::Half(v) => v[at] = as_bits(value) as u16,
        Store::Word(v) => v[at] = as_bits(value) as u32,
        Store::Long(v) => v[at] = as_bits(value),
        Store::Wide(v) => {
            v[at] = value.bits().expect("a wide array holds values with bits");
        }
        Store::Text(v) => {
            let Value::Str(text) = value else { unreachable!("a text array holds text") };
            v[at] = Rc::clone(text);
        }
        Store::Exact(v) => {
            let Value::Exact(number) = value else { unreachable!("an `er` array holds `er`") };
            v[at] = Rc::clone(number);
        }
    }
}

fn as_bits(value: &Value) -> u64 {
    match value {
        Value::Num { bits, .. } => *bits,
        Value::Bool(truth) => u64::from(*truth),
        other => unreachable!("a packed array holds {other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ty;

    fn int(ty: Ty, n: u64) -> Value {
        Value::Num { ty, bits: n }
    }

    #[test]
    fn a_thousand_bytes_take_a_thousand_bytes() {
        clear();
        make(Ty::U8, 1000, &int(Ty::U8, 0));
        let (arrays, bytes) = footprint();
        assert_eq!(arrays, 1);
        assert_eq!(bytes, 1000, "a thousand `ui8`s should be a thousand bytes");

        // The same thousand as a run of values would be twenty-four times that.
        assert_eq!(std::mem::size_of::<Value>() * 1000, 24_000);
    }

    #[test]
    fn each_width_takes_its_width() {
        clear();
        for (ty, width) in [(Ty::U8, 1), (Ty::U16, 2), (Ty::U32, 4), (Ty::U64, 8), (Ty::Bool, 1)] {
            let before = footprint().1;
            make(ty, 100, &int(ty, 0));
            assert_eq!(footprint().1 - before, 100 * width, "{}", ty.word());
        }
    }

    #[test]
    fn what_goes_in_comes_out() {
        clear();
        for ty in [Ty::U8, Ty::I8, Ty::U16, Ty::I32, Ty::U64, Ty::B64] {
            let handle = of(ty, &[int(ty, 1), int(ty, 2), int(ty, 3)]);
            assert_eq!(length(handle), 3);
            for (at, want) in [1u64, 2, 3].into_iter().enumerate() {
                let held = read(handle, at).expect("in range");
                assert_eq!(held, int(ty, want), "{} at {at}", ty.word());
            }
        }
    }

    #[test]
    fn a_bool_array_holds_truth_rather_than_a_number() {
        clear();
        let handle = of(Ty::Bool, &[Value::Bool(true), Value::Bool(false)]);
        assert_eq!(read(handle, 0), Some(Value::Bool(true)));
        assert_eq!(read(handle, 1), Some(Value::Bool(false)));
    }

    #[test]
    fn the_shared_kinds_are_shared_rather_than_copied() {
        clear();
        let text = Value::text("a phrase worth not copying");
        let handle = of(Ty::Str, &[text.clone(), text.clone()]);
        assert_eq!(read(handle, 0), Some(text.clone()));
        // Both elements and the original are one string, not three.
        let Value::Str(original) = &text else { panic!("text") };
        assert_eq!(Rc::strong_count(original), 3);
    }

    #[test]
    fn reaching_past_the_end_is_nothing_rather_than_a_panic() {
        clear();
        let handle = of(Ty::U8, &[int(Ty::U8, 1)]);
        assert!(read(handle, 1).is_none());
        assert!(!store(handle, 1, &int(Ty::U8, 9)));
        assert!(read(handle, 0).is_some());
    }

    #[test]
    fn writing_an_element_changes_that_one_and_no_other() {
        clear();
        let handle = of(Ty::U32, &[int(Ty::U32, 1), int(Ty::U32, 2), int(Ty::U32, 3)]);
        assert!(store(handle, 1, &int(Ty::U32, 99)));
        assert_eq!(read(handle, 0), Some(int(Ty::U32, 1)));
        assert_eq!(read(handle, 1), Some(int(Ty::U32, 99)));
        assert_eq!(read(handle, 2), Some(int(Ty::U32, 3)));
    }

    #[test]
    fn a_growable_one_grows() {
        clear();
        let handle = of(Ty::U16, &[]);
        assert_eq!(length(handle), 0);
        for n in 1..=5u64 {
            push(handle, &int(Ty::U16, n));
        }
        assert_eq!(length(handle), 5);
        assert_eq!(read(handle, 4), Some(int(Ty::U16, 5)));
        assert_eq!(footprint().1, 10, "five `ui16`s are ten bytes");
    }

    #[test]
    fn an_array_of_arrays_holds_handles() {
        clear();
        let inner = of(Ty::U8, &[int(Ty::U8, 7)]);
        let inner_ty = ty::growable(Ty::U8).expect("a type");
        let outer = of(inner_ty, &[handle(inner_ty, inner)]);
        let held = read(outer, 0).expect("in range");
        let Value::Num { bits, .. } = held else { panic!("a handle") };
        assert_eq!(read(bits as u32, 0), Some(int(Ty::U8, 7)));
    }
}

#[cfg(test)]
mod printing {
    use super::*;
    use crate::ty;

    #[test]
    fn an_array_prints_its_elements_and_not_its_handle() {
        clear();
        let ty = ty::fixed(Ty::U8, &[3]).expect("a type");
        let held = handle(ty, of(Ty::U8, &[
            Value::Num { ty: Ty::U8, bits: 1 },
            Value::Num { ty: Ty::U8, bits: 2 },
            Value::Num { ty: Ty::U8, bits: 3 },
        ]));
        assert_eq!(held.to_string(), "[1, 2, 3]");
    }

    #[test]
    fn two_arrays_are_equal_when_they_are_one_array() {
        clear();
        let ty = ty::growable(Ty::U8).expect("a type");
        let one = of(Ty::U8, &[Value::Num { ty: Ty::U8, bits: 1 }]);
        let other = of(Ty::U8, &[Value::Num { ty: Ty::U8, bits: 1 }]);
        // The same contents, and not the same array.
        assert_eq!(handle(ty, one), handle(ty, one));
        assert_ne!(handle(ty, one), handle(ty, other));
    }
}

/// Where an array's elements actually are, and how wide one is.
///
/// This is what packing them was for: with a pointer and a width, reaching element `n` is
/// `base + n × width`, which compiled code can do in one instruction instead of calling
/// back here to be handed a value.
///
/// # Safety
///
/// The pointer is only good until the array grows or another array is made, either of
/// which may move it. Compiled code takes it and uses it in the same breath, and nothing
/// runs in between — there is one thread and it is inside a single instruction.
pub fn base_of(index: u32) -> (*mut u8, usize) {
    HEAP.with(|heap| {
        let mut heap = heap.borrow_mut();
        let array = &mut heap[index as usize];
        let width = array.store.width();
        let base = match &mut array.store {
            Store::Byte(v) => v.as_mut_ptr(),
            Store::Half(v) => v.as_mut_ptr().cast(),
            Store::Word(v) => v.as_mut_ptr().cast(),
            Store::Long(v) => v.as_mut_ptr().cast(),
            Store::Wide(v) => v.as_mut_ptr().cast(),
            Store::Text(v) => v.as_mut_ptr().cast(),
            Store::Exact(v) => v.as_mut_ptr().cast(),
        };
        (base, width)
    })
}

/// How wide one element of this type is, where it is one compiled code can reach.
pub fn width_of(element: Ty) -> usize {
    match element {
        Ty::Bool | Ty::I8 | Ty::U8 => 1,
        Ty::B16 | Ty::I16 | Ty::U16 => 2,
        Ty::B32 | Ty::D32 | Ty::I32 | Ty::U32 | Ty::Array(_) => 4,
        _ => 8,
    }
}

#[cfg(test)]
mod reaching {
    use super::*;

    #[test]
    fn the_pointer_points_at_the_elements() {
        clear();
        let handle = of(Ty::U8, &[
            Value::Num { ty: Ty::U8, bits: 7 },
            Value::Num { ty: Ty::U8, bits: 9 },
        ]);
        let (base, width) = base_of(handle);
        assert_eq!(width, 1);
        // Reading through the pointer sees what the array holds, and writing through it
        // is seen by the array -- which is the whole point of handing it out.
        unsafe {
            assert_eq!(*base, 7);
            assert_eq!(*base.add(1), 9);
            *base.add(1) = 11;
        }
        assert_eq!(read(handle, 1), Some(Value::Num { ty: Ty::U8, bits: 11 }));
    }

    #[test]
    fn the_widths_agree_with_the_stores() {
        clear();
        for ty in [Ty::U8, Ty::Bool, Ty::U16, Ty::B32, Ty::U64, Ty::B64] {
            let handle = of(ty, &[]);
            assert_eq!(base_of(handle).1, width_of(ty), "{}", ty.word());
        }
    }
}
