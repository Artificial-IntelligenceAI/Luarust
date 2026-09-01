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
use std::cell::{Cell, RefCell};
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
    Text(Vec<Rc<String>>),
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
            Store::Text(_) => std::mem::size_of::<Rc<String>>(),
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
    /// and no destructor.
    static HEAP: RefCell<Vec<Array>> = const { RefCell::new(Vec::new()) };

    /// Slots whose array has been collected, ready to be handed out again.
    ///
    /// A dead slot keeps its place in `HEAP` -- an index has to stay an index -- and gives
    /// up its elements, which is where the memory actually was.
    static FREE: RefCell<Vec<u32>> = const { RefCell::new(Vec::new()) };

    /// Bytes handed out since the last collection, and the figure that asks for another.
    /// `None` is a program that has said it does not want collecting.
    static SINCE: Cell<usize> = const { Cell::new(0) };
    static THRESHOLD: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Forget every array. Called when a program begins, so one run never sees another's.
pub fn clear() {
    HEAP.with(|heap| heap.borrow_mut().clear());
    FREE.with(|free| free.borrow_mut().clear());
    SINCE.with(|since| since.set(0));
}

/// What a program does about arrays nothing can reach any more.
///
/// It travels in the chunk, because it is a decision about the program rather than about
/// the machine running it — the same reason `overflow` travels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Collect {
    /// Never. Right for a program that makes a few arrays and exits.
    #[default]
    Off,
    /// When enough has been handed out to be worth the walk.
    Silent,
    /// At every opportunity, for the smallest heap a program can run in.
    Aggressive,
}

impl Collect {
    /// How many bytes may be handed out before a collection, or `None` for never.
    pub fn threshold(self) -> Option<usize> {
        match self {
            Collect::Off => None,
            Collect::Silent => Some(1 << 20),
            Collect::Aggressive => Some(4096),
        }
    }

    /// The number it is written as in a chunk, and back.
    pub fn tag(self) -> u32 {
        match self {
            Collect::Off => 0,
            Collect::Silent => 1,
            Collect::Aggressive => 2,
        }
    }

    pub fn from_tag(tag: u32) -> Option<Collect> {
        Some(match tag {
            0 => Collect::Off,
            1 => Collect::Silent,
            2 => Collect::Aggressive,
            _ => return None,
        })
    }
}

/// How many bytes may be handed out before the heap asks to be collected.
///
/// `None` turns collection off, which is a real answer: a program that makes a few arrays
/// and exits should not pay for a collector it has no use for.
pub fn set_threshold(bytes: Option<usize>) {
    THRESHOLD.with(|t| t.set(bytes));
    SINCE.with(|since| since.set(0));
}

/// Whether enough has been handed out since the last collection to want another.
///
/// Asking is free. The answer is a load and a compare, so a program with collection off
/// pays one predictable branch per array it makes and nothing else at all.
pub fn wants_collecting() -> bool {
    match THRESHOLD.with(Cell::get) {
        None => false,
        Some(limit) => SINCE.with(Cell::get) >= limit,
    }
}

/// Free every array no root can reach, and return how many went.
///
/// Mark and sweep, and no more than that is needed: an array's elements are scalars, so
/// nothing in this language can contain itself and there are no cycles to chase. What
/// cannot be reached from a root is garbage, and reference counting would have found
/// exactly the same set.
///
/// A swept slot keeps its index and loses its elements. Indices have to stay stable
/// because a handle *is* an index, and the elements are where the memory was: dropping
/// the store hands the bytes back to the allocator there and then.
pub fn collect<'a>(roots: impl IntoIterator<Item = &'a Value>) -> usize {
    let live = HEAP.with(|heap| heap.borrow().len());
    if live == 0 {
        SINCE.with(|since| since.set(0));
        return 0;
    }

    let mut marked = vec![false; live];
    let mut pending: Vec<u32> = Vec::new();
    for root in roots {
        note(root, &mut marked, &mut pending);
    }

    // An element that is itself an array cannot be written today -- the parser refuses
    // `array` twice -- but the heap can hold one, and a collector that assumed otherwise
    // would be a trap laid for whoever writes them.
    while let Some(index) = pending.pop() {
        let inner: Vec<Value> = HEAP.with(|heap| {
            let heap = heap.borrow();
            let array = &heap[index as usize];
            if array.element.array().is_none() {
                return Vec::new();
            }
            (0..array.len()).map(|at| load(array, at)).collect()
        });
        for value in &inner {
            note(value, &mut marked, &mut pending);
        }
    }

    let mut freed = 0;
    HEAP.with(|heap| {
        FREE.with(|free| {
            let mut heap = heap.borrow_mut();
            let mut free = free.borrow_mut();
            for (index, alive) in marked.iter().enumerate() {
                if *alive || heap[index].store.is_empty() {
                    continue;
                }
                heap[index].store = store_for(heap[index].element, 0);
                free.push(index as u32);
                freed += 1;
            }
        });
    });
    SINCE.with(|since| since.set(0));
    freed
}

/// Mark one value, and queue it if it is an array whose elements might hold more.
fn note(value: &Value, marked: &mut [bool], pending: &mut Vec<u32>) {
    let Value::Num { ty, bits } = value else { return };
    if ty.array().is_none() {
        return;
    }
    let index = *bits as usize;
    if index >= marked.len() || marked[index] {
        return;
    }
    marked[index] = true;
    pending.push(index as u32);
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
        Ty::Str => Store::Text(vec![Rc::new(String::new()); len]),
        Ty::Er => Store::Exact(vec![Rc::new(Exact::zero()); len]),
    }
}

/// Make an array of this many elements, every one of them `fill`.
pub fn make(element: Ty, len: usize, fill: &Value) -> u32 {
    let mut array = Array { element, store: store_for(element, len) };
    for at in 0..len {
        write(&mut array, at, fill);
    }
    place(array)
}

/// Make an array holding exactly these, in order.
pub fn of(element: Ty, items: &[Value]) -> u32 {
    let mut array = Array { element, store: store_for(element, items.len()) };
    for (at, item) in items.iter().enumerate() {
        write(&mut array, at, item);
    }
    place(array)
}

/// Put an array in the heap, in a swept slot if there is one, and count what it cost.
fn place(array: Array) -> u32 {
    SINCE.with(|since| since.set(since.get().saturating_add(array.bytes())));
    HEAP.with(|heap| {
        let mut heap = heap.borrow_mut();
        if let Some(index) = FREE.with(|free| free.borrow_mut().pop()) {
            heap[index as usize] = array;
            return index;
        }
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
        // Read without the borrow flag. `borrow()` is a read, a compare, a write and a
        // second write when the guard drops, on every element a loop touches — for a check
        // that cannot fail here. Nothing between this line and the end of the function
        // touches the heap again: `load` reads one element and builds a value out of it,
        // and the only heap it could reach is this one, which it does not.
        //
        // # Safety
        // The heap is one thread's, and no `&mut` to it can be live: everything that takes
        // one does so inside its own `HEAP.with`, and none of them calls this.
        let heap = unsafe { &*heap.as_ptr() };
        let array = heap.get(index as usize)?;
        if at >= array.len() {
            return None;
        }
        Some(load(array, at))
    })
}

/// The stored bits of element `at`, for a reader that already knows what they are.
///
/// `None` where there is no such element — and where the element is not bits-shaped,
/// which a reader treats as "go the [`read`] way", not as missing. Skipping the
/// `Value` in between matters to a register file made of words: element loops were
/// building one here and taking it apart one call later.
pub fn read_bits(index: u32, at: usize) -> Option<u64> {
    HEAP.with(|heap| {
        // Read without the borrow flag, for the reasons `read` gives.
        //
        // # Safety
        // As for `read`: the heap is one thread's, and no `&mut` can be live here.
        let heap = unsafe { &*heap.as_ptr() };
        let array = heap.get(index as usize)?;
        match &array.store {
            Store::Byte(v) => Some(u64::from(*v.get(at)?)),
            Store::Half(v) => Some(u64::from(*v.get(at)?)),
            Store::Word(v) => Some(u64::from(*v.get(at)?)),
            Store::Long(v) => v.get(at).copied(),
            _ => None,
        }
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
            Store::Text(v) => v.push(Rc::new(String::new())),
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

        // The same thousand held as a run of values would be sixteen times that -- a
        // type and a number each, where the array knows the type once for all of them.
        assert_eq!(std::mem::size_of::<Value>() * 1000, 16_000);
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

#[cfg(test)]
mod collecting {
    use super::*;
    use crate::ty;

    fn int(ty: Ty, n: u64) -> Value {
        Value::Num { ty, bits: n }
    }

    /// A `ui8` array's handle, as a value a root set would hold.
    fn held(index: u32) -> Value {
        handle(ty::growable(Ty::U8).expect("a `ui8` array type"), index)
    }

    #[test]
    fn what_nothing_points_at_goes() {
        clear();
        make(Ty::U8, 1000, &int(Ty::U8, 7));
        assert_eq!(footprint(), (1, 1000), "the array should be there to start with");

        let freed = collect(&[]);
        assert_eq!(freed, 1, "with no roots at all, the array is garbage");
        assert_eq!(footprint().1, 0, "and its thousand bytes should have gone back");
    }

    #[test]
    fn what_a_root_points_at_stays() {
        clear();
        let kept = make(Ty::U8, 100, &int(Ty::U8, 1));
        let dropped = make(Ty::U8, 900, &int(Ty::U8, 2));

        let roots = [held(kept)];
        assert_eq!(collect(&roots), 1, "only the unreachable one goes");
        assert_eq!(footprint().1, 100, "and the reachable one keeps its hundred bytes");
        assert_eq!(length(kept), 100, "the kept array is still readable");
        assert_eq!(length(dropped), 0, "the swept one gave up its elements");
    }

    #[test]
    fn a_swept_slot_is_handed_out_again() {
        clear();
        let first = make(Ty::U8, 50, &int(Ty::U8, 0));
        collect(&[]);
        let second = make(Ty::U8, 50, &int(Ty::U8, 0));
        assert_eq!(first, second, "the second array should reuse the first one's slot");
        assert_eq!(footprint().0, 1, "so the heap does not grow at all");
    }

    #[test]
    fn a_loop_that_makes_and_forgets_does_not_grow() {
        clear();
        // What the collector exists for: a program that makes an array, stops looking at
        // it, and goes round again. Without sweeping this is a thousand live arrays.
        for _ in 0..1000 {
            let made = make(Ty::U64, 100, &int(Ty::U64, 3));
            let roots = [held(made)];
            collect(&roots);
        }
        let (arrays, bytes) = footprint();
        // Two slots, not one: the next array is made before the last one is collected, so
        // both are in play for a moment and the loop alternates between them. Two is the
        // point -- it is a thousand iterations and the heap did not grow with them.
        assert_eq!(arrays, 2, "two slots, taking turns, for a thousand arrays");
        assert_eq!(bytes, 800, "holding one array's worth of elements, not a thousand");
    }

    #[test]
    fn collecting_is_off_until_it_is_asked_for() {
        clear();
        set_threshold(None);
        make(Ty::U8, 10_000, &int(Ty::U8, 0));
        assert!(!wants_collecting(), "a program that said no should never be asked");

        set_threshold(Some(4096));
        assert!(!wants_collecting(), "setting a threshold starts the count over");
        make(Ty::U8, 5000, &int(Ty::U8, 0));
        assert!(wants_collecting(), "past the threshold, it wants collecting");
        collect(&[]);
        assert!(!wants_collecting(), "and having collected, it does not");
    }
}
