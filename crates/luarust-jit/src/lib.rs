//! Compiling Luarust to machine code with LLVM, in memory, and running it.
//!
//! The third way to run a program, and the one the other two exist to check. It is a JIT
//! in the sense that matters here — the machine code is made at run time, in memory, and
//! jumped into, with no object file and no linker anywhere — though it compiles the whole
//! program up front rather than waiting to find out which parts are hot. Tiering and
//! on-stack replacement come later; a program that spends its life inside one top-level
//! loop, which is what the benchmark is, would never trigger them anyway.
//!
//! **It declines more than it accepts, on purpose.** Integers and `b32`/`b64` are emitted
//! as native instructions, because those are exactly the cases where LLVM's arithmetic and
//! `luarust-num`'s are both correctly rounded and therefore identical. Everything else —
//! `b16`, `b128`, `b256`, powers, `bool`, `str` — makes it hand the program back, and the
//! VM runs it instead. An answer the three paths might disagree about is worth less than
//! no answer, and the disagreement would be found by the fuzzer at some unrelated moment
//! months later.

pub mod blocks;
mod runtime;

use inkwell::OptimizationLevel;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::execution_engine::ExecutionEngine;
use inkwell::module::Module;
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue};
use inkwell::{FloatPredicate, IntPredicate};
use luarust_check::value::{Fault, Overflow, Stopped, Value};
use luarust_vm::chunk::{Chunk, Op, Routine};
use luarust_diag::Span;
use luarust_parse::ast::{BinOp, CmpOp, Ty};

use std::io::Write;

/// Why the JIT handed a program back rather than compiling it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declined {
    pub because: String,
}

/// Whether the JIT will take a program at all.
///
/// The types it has no instructions for — `b128`, `b256`, `str` — live in numbered cells
/// on the Rust side, and compiled code carries the number.
///
/// Which is exactly why functions are declined for now. A cell number is decided when the
/// code is compiled, so every call to a function would share one set of cells, and a
/// function that called itself would overwrite the cells its caller was still using. The
/// fix is a stack of cells rather than a fixed row of them; until then a program with
/// functions in it goes to the VM, which has no such problem.
pub fn accepts(_chunk: &Chunk) -> Result<(), Declined> {
    Ok(())
}

/// Whether a value of this type can live in a register, or has to live in a cell.
fn celled(ty: Ty) -> bool {
    // `bool` used to be one of these. It stopped being one when comparisons arrived and
    // started *producing* them: a truth value is one bit, and putting every comparison's
    // answer in a cell would be a call for something the machine settles in one
    // instruction.
    // `er` joins them for a reason the others do not have: it is not merely wide, it is
    // unbounded. There is no register anywhere that could hold one.
    //
    // The decimals join them because no machine here has an instruction for any of them.
    // A `d64` would fit in a register perfectly well and there would be nothing to do
    // with it there, so it goes where the arithmetic is.
    matches!(ty, Ty::B128 | Ty::B256 | Ty::Str | Ty::Er) || ty.is_decimal()
}

/// Compile a program and run it, or hand it back.
pub fn run(chunk: &Chunk, out: &mut impl Write) -> Result<Result<(), Stopped>, Declined> {
    accepts(chunk)?;

    let context = Context::create();
    let module = context.create_module("luarust");
    let engine = module
        .create_jit_execution_engine(OptimizationLevel::Aggressive)
        .map_err(|why| Declined { because: format!("LLVM would not start: {why}") })?;

    let mut emitter = Emitter::new(&context, &module, &engine, chunk);
    emitter.emit(chunk, &module);
    let spans = std::mem::take(&mut emitter.spans);

    let compiled = unsafe {
        engine
            .get_function::<unsafe extern "C" fn() -> i64>("luarust_main")
            .map_err(|why| Declined { because: format!("the compiled program was lost: {why}") })?
    };

    runtime::begin(emitter.constants, emitter.main_frame, emitter.templates);
    let outcome = unsafe { compiled.call() };
    let _ = out.write_all(&runtime::taken());
    let _ = out.flush();

    Ok(decode(outcome, &spans))
}

/// The LLVM IR, for looking at.
pub fn emit_ir(chunk: &Chunk) -> Result<String, Declined> {
    accepts(chunk)?;
    let context = Context::create();
    let module = context.create_module("luarust");
    let engine = module
        .create_jit_execution_engine(OptimizationLevel::Aggressive)
        .map_err(|why| Declined { because: format!("LLVM would not start: {why}") })?;
    let mut emitter = Emitter::new(&context, &module, &engine, chunk);
    emitter.emit(chunk, &module);
    Ok(module.print_to_string().to_string())
}

/// Turn what the compiled program returned back into a fault.
///
/// Zero is success. Anything else carries the code in its low byte and which span it
/// happened at in the rest, so a fault from machine code points at source exactly as one
/// from the interpreter does.
fn decode(outcome: i64, spans: &[Span]) -> Result<(), Stopped> {
    if outcome == runtime::OK {
        return Ok(());
    }
    let code = outcome & 0xff;
    let span = spans.get((outcome >> 8) as usize).copied().unwrap_or_default();
    let fault = match code {
        runtime::DIVIDE_BY_ZERO => Fault {
            code: "R0002",
            message: "this divides a whole number by zero.".into(),
            rule: "an integer has no way to express what dividing by zero would give",
            fix: "check the divisor before dividing, or use a float type, where it is an infinity."
                .into(),
        },
        runtime::REMAINDER_BY_ZERO => Fault {
            code: "R0003",
            message: "this takes a remainder against zero.".into(),
            rule: "a remainder against zero is not a number",
            fix: "check the divisor before taking a remainder.".into(),
        },
        runtime::OUT_OF_RANGE => {
            let (at, length) = runtime::reached();
            Fault {
                code: "R0015",
                message: format!("there is no element {at} here."),
                rule: "an array is counted from one, up to how many it holds",
                fix: if length == 0 {
                    "this one holds nothing at all.".to_string()
                } else {
                    format!("this one holds {length}, so the last is {length} and the first is 1.")
                },
            }
        }
        runtime::FRACTIONAL_POWER => Fault {
            code: "R0012",
            message: "this raises an exact number to a power that is not whole.".into(),
            rule: "a ratio raised to a whole power is a ratio, and raised to anything else usually is not",
            fix: "use a whole exponent, or a float type, where the answer can be approximated."
                .into(),
        },
        runtime::POWER_TOO_LARGE => Fault {
            code: "R0013",
            message: format!(
                "this raises an exact number to a power above {}.",
                luarust_num::Exact::POWER_LIMIT
            ),
            rule: "an exact answer has to be written down, and that one would not fit anywhere",
            fix: "use a smaller exponent, or a float type, where the answer is rounded to a width."
                .into(),
        },
        runtime::TOO_DEEP => Fault {
            code: "R0011",
            message: format!(
                "this has called itself {} deep.",
                luarust_check::value::DEPTH_LIMIT
            ),
            rule: "a call may only go so deep before the program is stopped",
            fix: "give the recursion a case that stops, or write it as a loop.".into(),
        },
        runtime::DOES_NOT_FIT => Fault {
            code: "R0005",
            message: "this does not fit the width it is stored at.".into(),
            rule: "with overflow set to trap, a whole number must fit the width it is stored at",
            fix: "use a wider type, or let overflow wrap.".into(),
        },
        _ => Fault {
            code: "R0011",
            message: "the compiled program stopped.".into(),
            rule: "a program stops when an operation has no answer",
            fix: "run it with `luarust interp` to find out what happened.".into(),
        },
    };
    Err(Stopped { fault, span })
}


struct Emitter<'ctx> {
    context: &'ctx Context,
    builder: Builder<'ctx>,
    /// The function being emitted into. Blocks are appended to this one.
    main: FunctionValue<'ctx>,
    /// One LLVM function per Luarust function, all declared before any is filled in, so a
    /// call may name one that has not been emitted yet -- which is what lets two of them
    /// call each other.
    funcs: Vec<FunctionValue<'ctx>>,
    /// One `alloca` per register. A celled value lives in the frame cell of the same
    /// number instead, which is why nothing here has to be allocated or looked up.
    regs: Vec<PointerValue<'ctx>>,
    /// Where each jump target lands, for the routine being emitted.
    landings: std::collections::BTreeMap<usize, inkwell::basic_block::BasicBlock<'ctx>>,
    /// Where the fallback leaves its answer. Made once, in the entry block: an `alloca`
    /// anywhere else is stack allocated on every pass of whatever loop it is inside, which
    /// a ten-million-iteration program notices immediately by running out of stack.
    out_slot: PointerValue<'ctx>,
    /// Where a call leaves an answer the machine can hold. Read straight after the call,
    /// so one slot serves every call site -- and it is an entry-block `alloca`, because
    /// one made inside a loop grows the stack once per pass.
    answer_slot: PointerValue<'ctx>,
    /// Cells nothing writes to, shared by every frame.
    constants: Vec<Value>,
    /// What the frame being emitted starts out holding.
    frame: Vec<Value>,
    /// One finished frame layout per function.
    templates: Vec<Vec<Value>>,
    /// The top level's frame.
    main_frame: Vec<Value>,
    /// Where each fault can happen, so a code can be turned back into a place.
    spans: Vec<Span>,
    overflow: Overflow,
    helpers: Helpers<'ctx>,
}

struct Helpers<'ctx> {
    print_text: FunctionValue<'ctx>,
    print_value: FunctionValue<'ctx>,
    time_now: FunctionValue<'ctx>,
    compare: FunctionValue<'ctx>,
    fallback: FunctionValue<'ctx>,
    cell_move: FunctionValue<'ctx>,
    cell_binary: FunctionValue<'ctx>,
    cell_neg: FunctionValue<'ctx>,
    cell_compare: FunctionValue<'ctx>,
    cell_time_now: FunctionValue<'ctx>,
    print_cell: FunctionValue<'ctx>,
    cell_stage: FunctionValue<'ctx>,
    cells_enter: FunctionValue<'ctx>,
    cells_leave: FunctionValue<'ctx>,
    cells_leave_with: FunctionValue<'ctx>,
    cell_take_answer: FunctionValue<'ctx>,
    cell_unstage: FunctionValue<'ctx>,
    call_depth: FunctionValue<'ctx>,
    array_base: FunctionValue<'ctx>,
    array_len: FunctionValue<'ctx>,
    array_new: FunctionValue<'ctx>,
    array_filled: FunctionValue<'ctx>,
    array_get: FunctionValue<'ctx>,
    array_put: FunctionValue<'ctx>,
    cell_from_bits: FunctionValue<'ctx>,
    print_array: FunctionValue<'ctx>,
    note_index: FunctionValue<'ctx>,
}

impl<'ctx> Emitter<'ctx> {
    fn new(
        context: &'ctx Context,
        module: &Module<'ctx>,
        engine: &ExecutionEngine<'ctx>,
        chunk: &Chunk,
    ) -> Self {
        let i64_t = context.i64_type();
        let i32_t = context.i32_type();
        let ptr_t = context.ptr_type(Default::default());
        let void_t = context.void_type();

        // The handful of things machine code cannot do for itself, wired straight to the
        // Rust functions the other two paths already use.
        let print_text =
            module.add_function("luarust_print_text", void_t.fn_type(&[ptr_t.into(), i64_t.into()], false), None);
        engine.add_global_mapping(&print_text, runtime::print_text as *const () as usize);

        let print_value =
            module.add_function("luarust_print_value", void_t.fn_type(&[i64_t.into(), i32_t.into()], false), None);
        engine.add_global_mapping(&print_value, runtime::print_value as *const () as usize);

        let time_now =
            module.add_function("luarust_time_now", i64_t.fn_type(&[i32_t.into()], false), None);
        engine.add_global_mapping(&time_now, runtime::time_now as *const () as usize);

        let compare = module.add_function(
            "luarust_compare",
            i32_t.fn_type(&[i32_t.into(), i64_t.into(), i64_t.into()], false),
            None,
        );
        engine.add_global_mapping(&compare, runtime::compare_values as *const () as usize);

        let fallback = module.add_function(
            "luarust_fallback",
            i64_t.fn_type(
                &[i32_t.into(), i32_t.into(), i64_t.into(), i64_t.into(), i32_t.into(), ptr_t.into()],
                false,
            ),
            None,
        );
        engine.add_global_mapping(&fallback, runtime::fallback as *const () as usize);

        // The cells: everything done to one is a call, because everything done to one was
        // always going to be.
        let cell_move =
            module.add_function("luarust_cell_move", void_t.fn_type(&[i64_t.into(), i64_t.into()], false), None);
        engine.add_global_mapping(&cell_move, runtime::cell_move as *const () as usize);

        let cell_binary = module.add_function(
            "luarust_cell_binary",
            i64_t.fn_type(&[i32_t.into(), i64_t.into(), i64_t.into(), i64_t.into(), i32_t.into()], false),
            None,
        );
        engine.add_global_mapping(&cell_binary, runtime::cell_binary as *const () as usize);

        let cell_neg = module.add_function(
            "luarust_cell_neg",
            i64_t.fn_type(&[i64_t.into(), i64_t.into(), i32_t.into()], false),
            None,
        );
        engine.add_global_mapping(&cell_neg, runtime::cell_neg as *const () as usize);

        let cell_compare = module.add_function(
            "luarust_cell_compare",
            i32_t.fn_type(&[i64_t.into(), i64_t.into()], false),
            None,
        );
        engine.add_global_mapping(&cell_compare, runtime::cell_compare as *const () as usize);

        let cell_time_now = module.add_function(
            "luarust_cell_time_now",
            void_t.fn_type(&[i64_t.into(), i32_t.into()], false),
            None,
        );
        engine.add_global_mapping(&cell_time_now, runtime::cell_time_now as *const () as usize);

        let cell_stage = module.add_function(
            "luarust_cell_stage",
            void_t.fn_type(&[i64_t.into()], false),
            None,
        );
        engine.add_global_mapping(&cell_stage, runtime::cell_stage as *const () as usize);

        let cells_enter = module.add_function(
            "luarust_cells_enter",
            void_t.fn_type(&[i64_t.into()], false),
            None,
        );
        engine.add_global_mapping(&cells_enter, runtime::cells_enter as *const () as usize);

        let cells_leave =
            module.add_function("luarust_cells_leave", void_t.fn_type(&[], false), None);
        engine.add_global_mapping(&cells_leave, runtime::cells_leave as *const () as usize);

        let cells_leave_with = module.add_function(
            "luarust_cells_leave_with",
            void_t.fn_type(&[i64_t.into()], false),
            None,
        );
        engine
            .add_global_mapping(&cells_leave_with, runtime::cells_leave_with as *const () as usize);

        let cell_unstage = module.add_function(
            "luarust_cell_unstage",
            void_t.fn_type(&[i64_t.into()], false),
            None,
        );
        engine.add_global_mapping(&cell_unstage, runtime::cell_unstage as *const () as usize);

        let cell_take_answer = module.add_function(
            "luarust_cell_take_answer",
            void_t.fn_type(&[i64_t.into()], false),
            None,
        );
        engine
            .add_global_mapping(&cell_take_answer, runtime::cell_take_answer as *const () as usize);

        let call_depth =
            module.add_function("luarust_call_depth", i64_t.fn_type(&[], false), None);
        engine.add_global_mapping(&call_depth, runtime::call_depth as *const () as usize);

        let array_base = module.add_function(
            "luarust_array_base",
            ptr_t.fn_type(&[i64_t.into()], false),
            None,
        );
        engine.add_global_mapping(&array_base, runtime::array_base as *const () as usize);

        let array_len =
            module.add_function("luarust_array_len", i64_t.fn_type(&[i64_t.into()], false), None);
        engine.add_global_mapping(&array_len, runtime::array_len as *const () as usize);

        let array_new = module.add_function(
            "luarust_array_new",
            i64_t.fn_type(&[i32_t.into(), i64_t.into(), i64_t.into()], false),
            None,
        );
        engine.add_global_mapping(&array_new, runtime::array_new as *const () as usize);

        let array_filled = module.add_function(
            "luarust_array_filled",
            i64_t.fn_type(&[i32_t.into(), i64_t.into(), i64_t.into()], false),
            None,
        );
        engine.add_global_mapping(&array_filled, runtime::array_filled as *const () as usize);

        let array_get = module.add_function(
            "luarust_array_get",
            void_t.fn_type(&[i64_t.into(), i64_t.into(), i64_t.into()], false),
            None,
        );
        engine.add_global_mapping(&array_get, runtime::array_get as *const () as usize);

        let array_put = module.add_function(
            "luarust_array_put",
            void_t.fn_type(&[i64_t.into(), i64_t.into(), i64_t.into()], false),
            None,
        );
        engine.add_global_mapping(&array_put, runtime::array_put as *const () as usize);

        let cell_from_bits = module.add_function(
            "luarust_cell_from_bits",
            void_t.fn_type(&[i64_t.into(), i64_t.into(), i32_t.into()], false),
            None,
        );
        engine.add_global_mapping(&cell_from_bits, runtime::cell_from_bits as *const () as usize);

        let print_array = module.add_function(
            "luarust_print_array",
            void_t.fn_type(&[i64_t.into(), i32_t.into()], false),
            None,
        );
        engine.add_global_mapping(&print_array, runtime::print_array as *const () as usize);

        let note_index = module.add_function(
            "luarust_note_index",
            void_t.fn_type(&[i64_t.into(), i64_t.into()], false),
            None,
        );
        engine.add_global_mapping(&note_index, runtime::note_index as *const () as usize);

        let print_cell =
            module.add_function("luarust_print_cell", void_t.fn_type(&[i64_t.into()], false), None);
        engine.add_global_mapping(&print_cell, runtime::print_cell as *const () as usize);

        let main = module.add_function("luarust_main", i64_t.fn_type(&[], false), None);
        let entry = context.append_basic_block(main, "entry");
        let builder = context.create_builder();
        builder.position_at_end(entry);

        // Every VM register gets its own stack slot. LLVM keeps the ones that deserve it
        // in real registers, which is the pass this arrangement exists to feed.
        let mut regs = Vec::new();
        for index in 0..chunk.registers {
            let alloca = builder
                .build_alloca(context.i64_type(), &format!("r{index}"))
                .expect("a register");
            regs.push(alloca);
        }

        let out_slot = builder.build_alloca(context.i64_type(), "fallback.out").expect("a slot");
        let answer_slot = builder.build_alloca(context.i64_type(), "call.answer").expect("a slot");

        Self {
            context,
            builder,
            main,
            regs,
            landings: std::collections::BTreeMap::new(),
            out_slot,
            answer_slot,
            spans: Vec::new(),
            overflow: chunk.overflow,
            funcs: Vec::new(),
            constants: Vec::new(),
            frame: Vec::new(),
            templates: Vec::new(),
            main_frame: Vec::new(),
            helpers: Helpers {
                print_text,
                print_value,
                time_now,
                compare,
                fallback,
                cell_move,
                cell_binary,
                cell_neg,
                cell_compare,
                cell_time_now,
                print_cell,
                cell_stage,
                cells_enter,
                cells_leave,
                cells_leave_with,
                cell_take_answer,
                cell_unstage,
                call_depth,
                array_base,
                array_len,
                array_new,
                array_filled,
                array_get,
                array_put,
                cell_from_bits,
                print_array,
                note_index,
            },
        }
    }

    /// Every routine, declared and then filled in.
    ///
    /// This walks *instructions*, not a tree. What arrives is a chunk: registers, jumps
    /// and a flat list of operations, each already saying what type it works on. Reading
    /// that rather than the checked tree is what lets a `.lrc` file be compiled — the
    /// same file the VM runs, and the same file anybody can be handed.
    fn emit(&mut self, chunk: &Chunk, module: &Module<'ctx>) {
        let i64_t = self.context.i64_type();
        let ptr_t = self.context.ptr_type(Default::default());

        // Declared before any is emitted, so a call may name one not written yet.
        for (index, routine) in chunk.funcs.iter().enumerate() {
            let mut params: Vec<inkwell::types::BasicMetadataTypeEnum> = Vec::new();
            if routine.returns.is_some_and(|ty| !celled(ty)) {
                params.push(ptr_t.into());
            }
            for ty in &routine.params {
                if !celled(*ty) {
                    params.push(i64_t.into());
                }
            }
            let declared = module.add_function(
                &format!("luarust_fn{index}"),
                i64_t.fn_type(&params, false),
                None,
            );
            self.funcs.push(declared);
        }

        for (index, routine) in chunk.funcs.iter().enumerate() {
            self.emit_routine(chunk, index, routine);
        }

        // The top level, whose frame is the one the program starts in.
        self.builder
            .position_at_end(self.main.get_last_basic_block().expect("main has its entry"));
        self.frame = vec![Value::Bool(false); chunk.registers];
        self.walk(chunk, &chunk.code, &chunk.spans, false);
        self.main_frame = std::mem::take(&mut self.frame);
    }

    /// One function: its own registers, its own frame of cells, its own blocks.
    fn emit_routine(&mut self, chunk: &Chunk, index: usize, routine: &Routine) {
        let outer_main = self.main;
        let outer_regs = std::mem::take(&mut self.regs);
        let outer_out = self.out_slot;
        let outer_answer = self.answer_slot;
        let outer_frame = std::mem::take(&mut self.frame);

        let function = self.funcs[index];
        self.main = function;
        let entry = self.context.append_basic_block(function, "entry");
        self.builder.position_at_end(entry);

        self.regs = (0..routine.registers)
            .map(|n| {
                self.builder
                    .build_alloca(self.context.i64_type(), &format!("r{n}"))
                    .expect("a register")
            })
            .collect();
        self.out_slot =
            self.builder.build_alloca(self.context.i64_type(), "fallback.out").expect("a slot");
        self.answer_slot =
            self.builder.build_alloca(self.context.i64_type(), "call.answer").expect("a slot");

        // A register that ever holds a celled value uses the frame cell of the same
        // number. A register holds one value at a time, so the two can never both be
        // wanted, and nothing has to be allocated or looked up.
        self.frame = vec![Value::Bool(false); routine.registers];

        let number = self.context.i64_type().const_int(index as u64, false);
        self.builder
            .build_call(self.helpers.cells_enter, &[number.into()], "")
            .expect("a call");

        // The arguments: celled ones were staged by the caller and are taken here in the
        // order they were staged; the rest arrived as machine arguments.
        let mut argument = u32::from(routine.returns.is_some_and(|ty| !celled(ty)));
        for (n, ty) in routine.params.iter().enumerate() {
            if celled(*ty) {
                let cell = self.cell_number(n as u64);
                self.builder
                    .build_call(self.helpers.cell_unstage, &[cell.into()], "")
                    .expect("a call");
            } else {
                let bits = function.get_nth_param(argument).expect("a parameter");
                argument += 1;
                self.builder.build_store(self.regs[n], bits).expect("a store");
            }
        }

        let span = routine.spans.first().copied().unwrap_or_default();
        self.guard_depth(span);
        self.walk(chunk, &routine.code, &routine.spans, true);

        self.templates.push(std::mem::take(&mut self.frame));
        self.frame = outer_frame;
        self.regs = outer_regs;
        self.out_slot = outer_out;
        self.answer_slot = outer_answer;
        self.main = outer_main;
    }

    /// Walk a run of instructions, emitting each into the block it belongs to.
    fn walk(&mut self, chunk: &Chunk, code: &[Op], spans: &[Span], inside: bool) {
        let made: std::collections::BTreeMap<usize, inkwell::basic_block::BasicBlock<'ctx>> =
            blocks::leaders(code)
                .iter()
                .map(|at| (*at, self.context.append_basic_block(self.main, &format!("at{at}"))))
                .collect();
        self.landings = made.clone();

        // Out of the entry block, where the allocas live, and into the first real one.
        self.builder.build_unconditional_branch(made[&0]).expect("a branch");

        for (at, op) in code.iter().enumerate() {
            if let Some(block) = made.get(&at) {
                // Falling into a new block from an unterminated one is an ordinary jump.
                let here = self.builder.get_insert_block().expect("a block");
                if here.get_terminator().is_none() {
                    self.builder.build_unconditional_branch(*block).expect("a branch");
                }
                self.builder.position_at_end(*block);
            }
            self.op(chunk, *op, spans[at]);
        }

        // Whatever block is open at the end. Unreachable in practice -- a routine returns
        // and the top level halts -- but LLVM will not take a block without a terminator
        // whether anything can reach it or not.
        let here = self.builder.get_insert_block().expect("a block");
        if here.get_terminator().is_none() {
            if inside {
                self.builder.build_call(self.helpers.cells_leave, &[], "").expect("a call");
            }
            self.builder
                .build_return(Some(&self.context.i64_type().const_zero()))
                .expect("a return");
        }
    }

    /// A register, read as whatever the instruction says it holds.
    fn get(&self, reg: u16, ty: Ty) -> BasicValueEnum<'ctx> {
        let raw = self
            .builder
            .build_load(self.context.i64_type(), self.regs[reg as usize], "load")
            .expect("a load")
            .into_int_value();
        self.of_bits(raw, ty)
    }

    /// Put a value in a register, as bits.
    fn put(&self, reg: u16, value: BasicValueEnum<'ctx>, ty: Ty) {
        let bits = self.to_bits(value, ty);
        self.builder.build_store(self.regs[reg as usize], bits).expect("a store");
    }

    /// One instruction.
    fn op(&mut self, chunk: &Chunk, op: Op, span: Span) {
        match op {
            Op::Const { dst, konst } => {
                let value = &chunk.consts[konst as usize];
                let ty = value.ty();
                if celled(ty) {
                    let from = self.constant_cell(value.clone());
                    self.call_cell_move(u64::from(dst), from);
                } else {
                    let held = self.constant(value);
                    self.put(dst, held, ty);
                }
            }

            Op::Move { dst, src, ty } => {
                if celled(ty) {
                    if dst != src {
                        self.call_cell_move(u64::from(dst), u64::from(src));
                    }
                } else {
                    let held = self.get(src, ty);
                    self.put(dst, held, ty);
                }
            }

            Op::Binary { op, ty, dst, lhs, rhs } => {
                if celled(ty) {
                    self.call_cell_binary(op, u64::from(dst), u64::from(lhs), u64::from(rhs), span);
                } else {
                    let (a, b) = (self.get(lhs, ty), self.get(rhs, ty));
                    let answer = self.arithmetic(op, a, b, ty, span);
                    self.put(dst, answer, ty);
                }
            }

            Op::Neg { dst, src, ty } => {
                if celled(ty) {
                    self.call_cell_neg(u64::from(dst), u64::from(src), span);
                } else {
                    let held = self.get(src, ty);
                    let answer = self.negated(held, ty, span);
                    self.put(dst, answer, ty);
                }
            }

            Op::Not { dst, src } => {
                let held = self
                    .builder
                    .build_load(self.context.i64_type(), self.regs[src as usize], "load")
                    .expect("a load")
                    .into_int_value();
                let flipped = self
                    .builder
                    .build_xor(held, self.context.i64_type().const_int(1, false), "not")
                    .expect("an xor");
                self.builder.build_store(self.regs[dst as usize], flipped).expect("a store");
            }

            Op::Compare { op, operands, dst, lhs, rhs } => {
                let truth = self.stands(lhs, rhs, operands, op);
                let widened = self
                    .builder
                    .build_int_z_extend(truth, self.context.i64_type(), "truth")
                    .expect("an extend");
                self.builder.build_store(self.regs[dst as usize], widened).expect("a store");
            }

            Op::TimeNow { dst, ty } => self.time_now(dst, ty),

            Op::PrintText { text } => {
                let written = &chunk.texts[text as usize];
                let global =
                    self.builder.build_global_string_ptr(written, "text").expect("a string");
                let len = self.context.i64_type().const_int(written.len() as u64, false);
                self.builder
                    .build_call(
                        self.helpers.print_text,
                        &[global.as_pointer_value().into(), len.into()],
                        "",
                    )
                    .expect("a call");
            }

            Op::PrintValue { src, ty } => {
                // A whole array is its elements, and they are packed rather than being
                // values -- so this is the one thing about one that goes back to Rust.
                if let Some(of) = ty.array() {
                    let handle = self.handle_in(src);
                    let element =
                        self.context.i32_type().const_int(u64::from(of.element.tag()), false);
                    self.builder
                        .build_call(
                            self.helpers.print_array,
                            &[handle.into(), element.into()],
                            "",
                        )
                        .expect("a call");
                } else if celled(ty) {
                    let cell = self.cell_number(u64::from(src));
                    self.builder
                        .build_call(self.helpers.print_cell, &[cell.into()], "")
                        .expect("a call");
                } else {
                    let held = self.get(src, ty);
                    let bits = self.to_bits(held, ty);
                    let tag = self.context.i32_type().const_int(runtime::tag_of(ty) as u64, false);
                    self.builder
                        .build_call(self.helpers.print_value, &[bits.into(), tag.into()], "")
                        .expect("a call");
                }
            }

            Op::Jump { target } => {
                let landing = self.landing(target);
                self.builder.build_unconditional_branch(landing).expect("a branch");
            }

            Op::JumpIfFalse { cond, target } | Op::JumpIfTrue { cond, target } => {
                let held = self
                    .builder
                    .build_load(self.context.i64_type(), self.regs[cond as usize], "load")
                    .expect("a load")
                    .into_int_value();
                let truth = self
                    .builder
                    .build_int_compare(IntPredicate::NE, held, held.get_type().const_zero(), "held")
                    .expect("a compare");
                let onward = self.context.append_basic_block(self.main, "on");
                let landing = self.landing(target);
                if matches!(op, Op::JumpIfFalse { .. }) {
                    self.builder.build_conditional_branch(truth, onward, landing)
                } else {
                    self.builder.build_conditional_branch(truth, landing, onward)
                }
                .expect("a branch");
                self.builder.position_at_end(onward);
            }

            Op::JumpIfGreater { lhs, rhs, ty, target }
            | Op::JumpIfEqual { lhs, rhs, ty, target } => {
                let want = if matches!(op, Op::JumpIfGreater { .. }) {
                    CmpOp::Greater
                } else {
                    CmpOp::Equal
                };
                let truth = self.stands(lhs, rhs, ty, want);
                let onward = self.context.append_basic_block(self.main, "on");
                let landing = self.landing(target);
                self.builder
                    .build_conditional_branch(truth, landing, onward)
                    .expect("a branch");
                self.builder.position_at_end(onward);
            }

            Op::Call { func, base, argc, dst } => self.emit_call(chunk, func, base, argc, dst),

            Op::Return { src, ty } => {
                if celled(ty) {
                    let cell = self.cell_number(u64::from(src));
                    self.builder
                        .build_call(self.helpers.cells_leave_with, &[cell.into()], "")
                        .expect("a call");
                } else {
                    let held = self.get(src, ty);
                    let bits = self.to_bits(held, ty);
                    let answer = self.main.get_nth_param(0).expect("the answer pointer");
                    self.builder
                        .build_store(answer.into_pointer_value(), bits)
                        .expect("a store");
                    self.builder.build_call(self.helpers.cells_leave, &[], "").expect("a call");
                }
                self.builder
                    .build_return(Some(&self.context.i64_type().const_zero()))
                    .expect("a return");
            }

            Op::ReturnNothing => {
                self.builder.build_call(self.helpers.cells_leave, &[], "").expect("a call");
                self.builder
                    .build_return(Some(&self.context.i64_type().const_zero()))
                    .expect("a return");
            }

            Op::NewArray { dst, items, count, ty } => {
                let of = ty.array().expect("a new array has an array type");
                // The elements go through cells, one after another, because a `str` or an
                // `er` among them is a value machine code cannot hold. Making an array is
                // not the hot part; reaching into one is.
                let first = self.stage_run(items, count, of.element);
                let element = self.context.i32_type().const_int(u64::from(of.element.tag()), false);
                let handle = self
                    .builder
                    .build_call(
                        self.helpers.array_new,
                        &[element.into(), first.into(), self.context.i64_type().const_int(u64::from(count), false).into()],
                        "array",
                    )
                    .expect("a call")
                    .try_as_basic_value()
                    .expect_basic("it answers a handle")
                    .into_int_value();
                self.builder.build_store(self.regs[dst as usize], handle).expect("a store");
            }

            Op::Filled { dst, length, value, ty } => {
                let of = ty.array().expect("a filled array has an array type");
                let count = self.get(length, Ty::U32);
                let count = self.to_bits(count, Ty::U32);
                let fill = self.cell_holding(value, of.element);
                let element = self.context.i32_type().const_int(u64::from(of.element.tag()), false);
                let handle = self
                    .builder
                    .build_call(
                        self.helpers.array_filled,
                        &[element.into(), count.into(), fill.into()],
                        "array",
                    )
                    .expect("a call")
                    .try_as_basic_value()
                    .expect_basic("it answers a handle")
                    .into_int_value();
                self.builder.build_store(self.regs[dst as usize], handle).expect("a store");
            }

            Op::At { dst, array, at, rank, ty } => {
                let of = ty.array().expect("only an array is indexed");
                let handle = self.handle_in(array);
                let flat = self.flatten(handle, at, rank, ty, span);
                if celled(of.element) {
                    // A shared element goes through a cell: taking a reference count is
                    // not something machine code should be doing.
                    let cell = self.cell_number(u64::from(dst));
                    self.builder
                        .build_call(
                            self.helpers.array_get,
                            &[handle.into(), flat.into(), cell.into()],
                            "",
                        )
                        .expect("a call");
                } else {
                    let held = self.element_at(handle, flat, of.element);
                    self.builder.build_store(self.regs[dst as usize], held).expect("a store");
                }
            }

            Op::StoreAt { array, at, rank, value, ty } => {
                let of = ty.array().expect("only an array is indexed");
                let handle = self.handle_in(array);
                let flat = self.flatten(handle, at, rank, ty, span);
                if celled(of.element) {
                    let cell = self.cell_number(u64::from(value));
                    self.builder
                        .build_call(
                            self.helpers.array_put,
                            &[handle.into(), flat.into(), cell.into()],
                            "",
                        )
                        .expect("a call");
                } else {
                    let held = self.get(value, of.element);
                    self.put_element(handle, flat, held, of.element);
                }
            }

            Op::Count { dst, array, ty } => {
                let handle = self.handle_in(array);
                let count = self
                    .builder
                    .build_call(self.helpers.array_len, &[handle.into()], "count")
                    .expect("a call")
                    .try_as_basic_value()
                    .expect_basic("it answers a length")
                    .into_int_value();
                let _ = ty;
                self.builder.build_store(self.regs[dst as usize], count).expect("a store");
            }

            Op::Halt => {
                self.builder
                    .build_return(Some(&self.context.i64_type().const_zero()))
                    .expect("a return");
            }
        }
    }

    /// The handle a register holds, as the machine word it is.
    fn handle_in(&self, reg: u16) -> inkwell::values::IntValue<'ctx> {
        self.builder
            .build_load(self.context.i64_type(), self.regs[reg as usize], "handle")
            .expect("a load")
            .into_int_value()
    }

    /// Where an index lands: counted from one, flattened row by row, and checked.
    ///
    /// The dimensions are known when this is compiled, so the arithmetic is emitted
    /// rather than worked out again at run time. Only the bound of a growable array has
    /// to be asked for.
    fn flatten(
        &mut self,
        handle: inkwell::values::IntValue<'ctx>,
        at: u16,
        rank: u8,
        ty: Ty,
        span: Span,
    ) -> inkwell::values::IntValue<'ctx> {
        let i64_t = self.context.i64_type();
        let of = ty.array().expect("only an array is indexed");
        let dims = of.dims().to_vec();
        let mut flat = i64_t.const_zero();

        for place in 0..rank as usize {
            let index = self.handle_in(at + place as u16);
            // One-based, so anything below one is out of range and so is anything at or
            // above the bound. Unsigned makes that one comparison: `index - 1 >= bound`
            // wraps for zero and catches both ends at once.
            let one = i64_t.const_int(1, false);
            let zeroed = self.builder.build_int_sub(index, one, "from.nought").expect("a subtract");
            let bound = match dims.get(place) {
                Some(size) => i64_t.const_int(u64::from(*size), false),
                None => self
                    .builder
                    .build_call(self.helpers.array_len, &[handle.into()], "len")
                    .expect("a call")
                    .try_as_basic_value()
                    .expect_basic("it answers a length")
                    .into_int_value(),
            };
            let past = self
                .builder
                .build_int_compare(IntPredicate::UGE, zeroed, bound, "past.the.end")
                .expect("a compare");

            // The index and the bound are known here and nowhere else, so they are handed
            // over before stopping -- otherwise the fault could only say that *some*
            // element was missing, where the other two paths name it.
            let stop = self.context.append_basic_block(self.main, "out.of.range");
            let carry_on = self.context.append_basic_block(self.main, "in.range");
            self.builder.build_conditional_branch(past, stop, carry_on).expect("a branch");
            self.builder.position_at_end(stop);
            self.builder
                .build_call(self.helpers.note_index, &[index.into(), bound.into()], "")
                .expect("a call");
            let code = self.fault_marker(span, runtime::OUT_OF_RANGE);
            self.builder
                .build_return(Some(&self.context.i64_type().const_int(code as u64, false)))
                .expect("a return");
            self.builder.position_at_end(carry_on);

            let stride = i64_t.const_int(u64::from(dims.get(place).copied().unwrap_or(1)), false);
            let scaled = self.builder.build_int_mul(flat, stride, "row").expect("a multiply");
            flat = self.builder.build_int_add(scaled, zeroed, "flat").expect("an add");
        }
        flat
    }

    /// One element, loaded straight out of the array.
    fn element_at(
        &mut self,
        handle: inkwell::values::IntValue<'ctx>,
        flat: inkwell::values::IntValue<'ctx>,
        element: Ty,
    ) -> inkwell::values::IntValue<'ctx> {
        let base = self.array_base(handle);
        let slot = self.slot_pointer(base, flat, element);
        let narrow = self.narrow_type(element);
        let held = self
            .builder
            .build_load(narrow, slot, "element")
            .expect("a load")
            .into_int_value();
        // Widened to the machine word a register holds. Unsigned, because the value's own
        // type decides what the bits mean and this only has to not lose any.
        self.builder
            .build_int_z_extend(held, self.context.i64_type(), "widened")
            .expect("an extend")
    }

    /// A value into one element, stored straight into the array.
    fn put_element(
        &mut self,
        handle: inkwell::values::IntValue<'ctx>,
        flat: inkwell::values::IntValue<'ctx>,
        value: BasicValueEnum<'ctx>,
        element: Ty,
    ) {
        let base = self.array_base(handle);
        let slot = self.slot_pointer(base, flat, element);
        let bits = self.to_bits(value, element);
        let narrow = self.narrow_type(element);
        let narrowed = self
            .builder
            .build_int_truncate_or_bit_cast(bits, narrow, "narrowed")
            .expect("a truncate");
        self.builder.build_store(slot, narrowed).expect("a store");
    }

    /// Where an array's elements begin. Asked for afresh each time, because making an
    /// array or growing one may have moved them.
    fn array_base(
        &mut self,
        handle: inkwell::values::IntValue<'ctx>,
    ) -> inkwell::values::PointerValue<'ctx> {
        self.builder
            .build_call(self.helpers.array_base, &[handle.into()], "base")
            .expect("a call")
            .try_as_basic_value()
            .expect_basic("it answers a pointer")
            .into_pointer_value()
    }

    /// `base + n × width`, which is the whole reason the elements are packed.
    fn slot_pointer(
        &mut self,
        base: inkwell::values::PointerValue<'ctx>,
        flat: inkwell::values::IntValue<'ctx>,
        element: Ty,
    ) -> inkwell::values::PointerValue<'ctx> {
        let width = self
            .context
            .i64_type()
            .const_int(luarust_core::heap::width_of(element) as u64, false);
        let offset = self.builder.build_int_mul(flat, width, "offset").expect("a multiply");
        unsafe {
            self.builder
                .build_gep(self.context.i8_type(), base, &[offset], "slot")
                .expect("a pointer")
        }
    }

    /// The integer type one packed element is, which is how wide it is stored.
    fn narrow_type(&self, element: Ty) -> inkwell::types::IntType<'ctx> {
        match luarust_core::heap::width_of(element) {
            1 => self.context.i8_type(),
            2 => self.context.i16_type(),
            4 => self.context.i32_type(),
            _ => self.context.i64_type(),
        }
    }

    /// A run of registers into a run of cells, so the runtime can read them all.
    fn stage_run(&mut self, first: u16, count: u16, element: Ty) -> inkwell::values::IntValue<'ctx> {
        for n in 0..count {
            let cell = self.cell_holding(first + n, element);
            let _ = cell;
        }
        // The cells are the registers' own, which are consecutive because the compiler
        // laid the elements out consecutively.
        self.cell_number(u64::from(first))
    }

    /// The cell a register's value is in, putting it there when it is not already.
    fn cell_holding(&mut self, reg: u16, element: Ty) -> inkwell::values::IntValue<'ctx> {
        if !celled(element) {
            // A packed value has no cell of its own, so it is written into the one that
            // shares its register number.
            let held = self.get(reg, element);
            let bits = self.to_bits(held, element);
            let tag = self.context.i32_type().const_int(u64::from(element.tag()), false);
            self.builder
                .build_call(
                    self.helpers.cell_from_bits,
                    &[self.cell_number(u64::from(reg)).into(), bits.into(), tag.into()],
                    "",
                )
                .expect("a call");
        }
        self.cell_number(u64::from(reg))
    }

    /// Whether two registers stand in a relation, celled or not.
    fn stands(&mut self, lhs: u16, rhs: u16, ty: Ty, op: CmpOp) -> inkwell::values::IntValue<'ctx> {
        if celled(ty) {
            return self.cells_compare(u64::from(lhs), u64::from(rhs), op);
        }
        let (a, b) = (self.get(lhs, ty), self.get(rhs, ty));
        self.relation(a, b, ty, op)
    }

    /// The block a jump lands in.
    fn landing(&self, target: u32) -> inkwell::basic_block::BasicBlock<'ctx> {
        self.landings[&(target as usize)]
    }

    /// A call: stage what travels through the runtime, pass the rest as arguments.
    fn emit_call(&mut self, chunk: &Chunk, func: u32, base: u16, argc: u16, dst: u16) {
        let callee = &chunk.funcs[func as usize];
        let returns = callee.returns;
        let params = callee.params.clone();

        let mut natives: Vec<inkwell::values::BasicMetadataValueEnum> = Vec::new();
        for (n, ty) in params.iter().enumerate().take(argc as usize) {
            let reg = base + n as u16;
            if celled(*ty) {
                let cell = self.cell_number(u64::from(reg));
                self.builder
                    .build_call(self.helpers.cell_stage, &[cell.into()], "")
                    .expect("a call");
            } else {
                let held = self.get(reg, *ty);
                natives.push(self.to_bits(held, *ty).into());
            }
        }

        let mut call_args: Vec<inkwell::values::BasicMetadataValueEnum> = Vec::new();
        if returns.is_some_and(|ty| !celled(ty)) {
            call_args.push(self.answer_slot.into());
        }
        call_args.extend(natives);

        let outcome = self
            .builder
            .build_call(self.funcs[func as usize], &call_args, "call")
            .expect("a call")
            .try_as_basic_value()
            .expect_basic("a function answers a fault code")
            .into_int_value();
        self.carry_fault(outcome);

        match returns {
            None => {}
            Some(ty) if celled(ty) => {
                let cell = self.cell_number(u64::from(dst));
                self.builder
                    .build_call(self.helpers.cell_take_answer, &[cell.into()], "")
                    .expect("a call");
            }
            Some(_) => {
                let bits = self
                    .builder
                    .build_load(self.context.i64_type(), self.answer_slot, "answered")
                    .expect("a load")
                    .into_int_value();
                self.builder.build_store(self.regs[dst as usize], bits).expect("a store");
            }
        }
    }

    /// Hand a callee's fault straight out. It already knows where it happened, so nothing
    /// is added to it — the line reported is the line that actually faulted.
    fn carry_fault(&mut self, outcome: inkwell::values::IntValue<'ctx>) {
        let failed = self
            .builder
            .build_int_compare(
                IntPredicate::NE,
                outcome,
                self.context.i64_type().const_zero(),
                "failed",
            )
            .expect("a compare");
        let stop = self.context.append_basic_block(self.main, "carried");
        let carry_on = self.context.append_basic_block(self.main, "no.fault");
        self.builder.build_conditional_branch(failed, stop, carry_on).expect("a branch");

        self.builder.position_at_end(stop);
        self.builder.build_return(Some(&outcome)).expect("a return");
        self.builder.position_at_end(carry_on);
    }

    /// Stop before the frames go deeper than the other two paths allow.
    fn guard_depth(&mut self, span: Span) {
        let depth = self
            .builder
            .build_call(self.helpers.call_depth, &[], "depth")
            .expect("a call")
            .try_as_basic_value()
            .expect_basic("it answers how deep the frames go")
            .into_int_value();
        let limit =
            self.context.i64_type().const_int(luarust_check::value::DEPTH_LIMIT as u64, false);
        let too_deep = self
            .builder
            .build_int_compare(IntPredicate::UGT, depth, limit, "too.deep")
            .expect("a comparison");

        let stop = self.context.append_basic_block(self.main, "too.deep");
        let carry_on = self.context.append_basic_block(self.main, "deep.enough");
        self.builder.build_conditional_branch(too_deep, stop, carry_on).expect("a branch");

        self.builder.position_at_end(stop);
        let code = self.fault_marker(span, runtime::TOO_DEEP);
        self.builder
            .build_return(Some(&self.context.i64_type().const_int(code as u64, false)))
            .expect("a return");
        self.builder.position_at_end(carry_on);
    }

    /// Note where a fault could happen, and give back the value to return if it does.
    fn fault_marker(&mut self, span: Span, code: i64) -> i64 {
        let index = self.spans.len() as i64;
        self.spans.push(span);
        (index << 8) | code
    }

    // ---- cells ------------------------------------------------------------------


    /// A cell holding a constant. Constants are shared by every frame, so their numbers
    /// carry a bit that says to look somewhere other than the frame.
    fn constant_cell(&mut self, value: Value) -> u64 {
        self.constants.push(value);
        (self.constants.len() - 1) as u64 | runtime::CONSTANT
    }

    fn cell_number(&self, index: u64) -> inkwell::values::IntValue<'ctx> {
        self.context.i64_type().const_int(index, false)
    }

    fn call_cell_move(&mut self, dst: u64, src: u64) {
        let (dst, src) = (self.cell_number(dst), self.cell_number(src));
        self.builder
            .build_call(self.helpers.cell_move, &[dst.into(), src.into()], "")
            .expect("a call");
    }

    fn call_cell_binary(&mut self, op: BinOp, dst: u64, a: u64, b: u64, span: Span) {
        let i32_t = self.context.i32_type();
        let trapping = i32_t.const_int(u64::from(self.overflow == Overflow::Trap), false);
        let outcome = self
            .builder
            .build_call(
                self.helpers.cell_binary,
                &[
                    i32_t.const_int(runtime::op_tag(op) as u64, false).into(),
                    self.cell_number(dst).into(),
                    self.cell_number(a).into(),
                    self.cell_number(b).into(),
                    trapping.into(),
                ],
                "celled",
            )
            .expect("a call")
            .try_as_basic_value()
            .expect_basic("it returns a fault code")
            .into_int_value();
        self.stop_if_nonzero(outcome, span);
    }

    fn cells_compare(&mut self, a: u64, b: u64, op: CmpOp) -> inkwell::values::IntValue<'ctx> {
        let outcome = self
            .builder
            .build_call(
                self.helpers.cell_compare,
                &[self.cell_number(a).into(), self.cell_number(b).into()],
                "ordering",
            )
            .expect("a call")
            .try_as_basic_value()
            .expect_basic("comparing returns a value")
            .into_int_value();
        self.ordering_matches(outcome, op)
    }

    /// Stop the program here if a call came back with anything but zero.
    fn stop_if_nonzero(&mut self, outcome: inkwell::values::IntValue<'ctx>, span: Span) {
        let i64_t = self.context.i64_type();
        let failed = self
            .builder
            .build_int_compare(IntPredicate::NE, outcome, i64_t.const_int(0, false), "failed")
            .expect("a compare");
        let stop = self.context.append_basic_block(self.main, "stop");
        let carry_on = self.context.append_basic_block(self.main, "carry.on");
        self.builder.build_conditional_branch(failed, stop, carry_on).expect("a branch");

        self.builder.position_at_end(stop);
        let marker = self.fault_marker(span, 0);
        let with_place = self
            .builder
            .build_or(outcome, i64_t.const_int(marker as u64, false), "where")
            .expect("an or");
        self.builder.build_return(Some(&with_place)).expect("a return");

        self.builder.position_at_end(carry_on);
    }

    // ---- values -----------------------------------------------------------------

    /// The clock, in whichever format it was asked for.
    fn time_now(&mut self, dst: u16, ty: Ty) {
        // Asked for in the format it is being read as. Answering in `b64` whatever was
        // wanted would put a `b64`'s bits in a `b32` register.
        let tag = self.context.i32_type().const_int(runtime::tag_of(ty) as u64, false);
        if celled(ty) {
            let number = self.cell_number(u64::from(dst));
            self.builder
                .build_call(self.helpers.cell_time_now, &[number.into(), tag.into()], "")
                .expect("a call");
            return;
        }
        let bits = self
            .builder
            .build_call(self.helpers.time_now, &[tag.into()], "now")
            .expect("a call")
            .try_as_basic_value()
            .expect_basic("the clock returns a value")
            .into_int_value();
        self.builder.build_store(self.regs[dst as usize], bits).expect("a store");
    }

    /// Negating a celled value, which is a call like everything else done to one.
    fn call_cell_neg(&mut self, dst: u64, src: u64, span: Span) {
        let i32_t = self.context.i32_type();
        let trapping = i32_t.const_int(u64::from(self.overflow == Overflow::Trap), false);
        let outcome = self
            .builder
            .build_call(
                self.helpers.cell_neg,
                &[self.cell_number(dst).into(), self.cell_number(src).into(), trapping.into()],
                "negated",
            )
            .expect("a call")
            .try_as_basic_value()
            .expect_basic("it returns a fault code")
            .into_int_value();
        self.stop_if_nonzero(outcome, span);
    }

    /// Negating a value the machine holds.
    fn negated(&mut self, value: BasicValueEnum<'ctx>, ty: Ty, span: Span) -> BasicValueEnum<'ctx> {
        // Flipping the sign bit and nothing else, which is exact for every value
        // including the zeros and the NaNs. Not `0 - x`: `0.0 - 0.0` is `+0.0`, where
        // negating a zero has to give `-0.0`.
        if ty == Ty::B16 {
            let sign = self.context.i16_type().const_int(0x8000, false);
            return self
                .builder
                .build_xor(value.into_int_value(), sign, "neg")
                .expect("an xor")
                .into();
        }
        if ty.is_float() && !ty.is_decimal() {
            return self
                .builder
                .build_float_neg(value.into_float_value(), "neg")
                .expect("a negate")
                .into();
        }
        let zero = self.zero(ty);
        self.arithmetic(BinOp::Sub, zero, value, ty, span)
    }

    fn constant(&self, value: &Value) -> BasicValueEnum<'ctx> {
        if let Value::Bool(truth) = value {
            return self.context.i64_type().const_int(u64::from(*truth), false).into();
        }
        let Value::Num { ty, bits } = value else { unreachable!("celled values do not come here") };
        match ty {
            Ty::B32 => {
                let float = f32::from_bits(*bits as u32);
                self.context.f32_type().const_float(float as f64).into()
            }
            Ty::B64 => self.context.f64_type().const_float(f64::from_bits(*bits)).into(),
            // b16 travels as its encoding, not as a number the machine understands.
            _ => self.int_type(*ty).const_int(*bits, false).into(),
        }
    }

    fn int_type(&self, ty: Ty) -> inkwell::types::IntType<'ctx> {
        if ty == Ty::B16 {
            return self.context.i16_type();
        }
        match ty.int_bits().unwrap_or(64) {
            8 => self.context.i8_type(),
            16 => self.context.i16_type(),
            32 => self.context.i32_type(),
            _ => self.context.i64_type(),
        }
    }

    /// The stack slots are all `i64`; a narrower or floating value is reshaped on the way
    /// in and out, which LLVM removes entirely once it can see through the alloca.
    /// The other direction from [`Self::to_bits`]: bits back into whatever they hold.
    fn of_bits(&self, raw: inkwell::values::IntValue<'ctx>, ty: Ty) -> BasicValueEnum<'ctx> {
        match ty {
            Ty::B64 => self.builder.build_bit_cast(raw, self.context.f64_type(), "f").expect("a cast"),
            Ty::B32 => {
                let narrow = self
                    .builder
                    .build_int_truncate(raw, self.context.i32_type(), "t")
                    .expect("a truncate");
                self.builder.build_bit_cast(narrow, self.context.f32_type(), "f").expect("a cast")
            }
            _ => {
                let width = if ty == Ty::B16 { 16 } else { ty.int_bits().unwrap_or(64) };
                if width == 64 {
                    raw.into()
                } else {
                    self.builder
                        .build_int_truncate(raw, self.int_type(ty), "t")
                        .expect("a truncate")
                        .into()
                }
            }
        }
    }

    /// A value as the `i64` a slot or a callback wants.
    fn to_bits(&self, value: BasicValueEnum<'ctx>, ty: Ty) -> inkwell::values::IntValue<'ctx> {
        match ty {
            Ty::B64 => self
                .builder
                .build_bit_cast(value.into_float_value(), self.context.i64_type(), "b")
                .expect("a cast")
                .into_int_value(),
            Ty::B32 => {
                let bits = self
                    .builder
                    .build_bit_cast(value.into_float_value(), self.context.i32_type(), "b")
                    .expect("a cast")
                    .into_int_value();
                self.builder
                    .build_int_z_extend(bits, self.context.i64_type(), "z")
                    .expect("an extend")
            }
            _ => {
                let int = value.into_int_value();
                let width = if ty == Ty::B16 { 16 } else { ty.int_bits().unwrap_or(64) };
                if width == 64 {
                    int
                } else {
                    self.builder
                        .build_int_z_extend(int, self.context.i64_type(), "z")
                        .expect("an extend")
                }
            }
        }
    }

    fn zero(&self, ty: Ty) -> BasicValueEnum<'ctx> {
        match ty {
            Ty::B32 => self.context.f32_type().const_float(0.0).into(),
            Ty::B64 => self.context.f64_type().const_float(0.0).into(),
            _ => self.int_type(ty).const_int(0, false).into(),
        }
    }

    /// Whether two values stand in a relation — one instruction for the types the machine
    /// can order, and a call for `b16`, whose values are sign-and-magnitude in sixteen
    /// bits and so are neither an integer nor a float comparison.
    fn relation(
        &self,
        a: BasicValueEnum<'ctx>,
        b: BasicValueEnum<'ctx>,
        ty: Ty,
        op: CmpOp,
    ) -> inkwell::values::IntValue<'ctx> {
        if ty == Ty::B16 {
            return self.compare_by_call(a, b, ty, op);
        }
        if ty.is_float() {
            // The *ordered* predicates, so a NaN answers false rather than sneaking a
            // true out of one of them -- except `!=`, which asks only that the two differ,
            // and an unordered pair does differ.
            let predicate = match op {
                CmpOp::Less => FloatPredicate::OLT,
                CmpOp::Greater => FloatPredicate::OGT,
                CmpOp::Equal => FloatPredicate::OEQ,
                CmpOp::LessEqual => FloatPredicate::OLE,
                CmpOp::GreaterEqual => FloatPredicate::OGE,
                CmpOp::NotEqual => FloatPredicate::UNE,
            };
            return self
                .builder
                .build_float_compare(predicate, a.into_float_value(), b.into_float_value(), "rel")
                .expect("a compare");
        }
        let predicate = match (op, ty.is_signed()) {
            (CmpOp::Less, true) => IntPredicate::SLT,
            (CmpOp::Less, false) => IntPredicate::ULT,
            (CmpOp::Greater, true) => IntPredicate::SGT,
            (CmpOp::Greater, false) => IntPredicate::UGT,
            (CmpOp::LessEqual, true) => IntPredicate::SLE,
            (CmpOp::LessEqual, false) => IntPredicate::ULE,
            (CmpOp::GreaterEqual, true) => IntPredicate::SGE,
            (CmpOp::GreaterEqual, false) => IntPredicate::UGE,
            (CmpOp::Equal, _) => IntPredicate::EQ,
            (CmpOp::NotEqual, _) => IntPredicate::NE,
        };
        self.builder
            .build_int_compare(predicate, a.into_int_value(), b.into_int_value(), "rel")
            .expect("a compare")
    }

    /// Ask `luarust-num` how two values order, and test the answer against the one wanted.
    fn compare_by_call(
        &self,
        a: BasicValueEnum<'ctx>,
        b: BasicValueEnum<'ctx>,
        ty: Ty,
        op: CmpOp,
    ) -> inkwell::values::IntValue<'ctx> {
        let i32_t = self.context.i32_type();
        let outcome = self
            .builder
            .build_call(
                self.helpers.compare,
                &[
                    i32_t.const_int(runtime::tag_of(ty) as u64, false).into(),
                    self.to_bits(a, ty).into(),
                    self.to_bits(b, ty).into(),
                ],
                "ordering",
            )
            .expect("a call")
            .try_as_basic_value()
            .expect_basic("comparing returns a value")
            .into_int_value();
        let _ = i32_t;
        self.ordering_matches(outcome, op)
    }

    /// Turn an ordering — less, equal, greater, or unordered — into the answer to one
    /// particular question about it.
    fn ordering_matches(
        &self,
        ordering: inkwell::values::IntValue<'ctx>,
        op: CmpOp,
    ) -> inkwell::values::IntValue<'ctx> {
        let i32_t = self.context.i32_type();
        let is = |code: u64, name: &str| {
            self.builder
                .build_int_compare(IntPredicate::EQ, ordering, i32_t.const_int(code, false), name)
                .expect("a compare")
        };
        match op {
            CmpOp::Less => is(runtime::LESS, "lt"),
            CmpOp::Equal => is(runtime::EQUAL, "eq"),
            CmpOp::Greater => is(runtime::GREATER, "gt"),
            CmpOp::LessEqual => {
                let (a, b) = (is(runtime::LESS, "lt"), is(runtime::EQUAL, "eq"));
                self.builder.build_or(a, b, "le").expect("an or")
            }
            CmpOp::GreaterEqual => {
                let (a, b) = (is(runtime::GREATER, "gt"), is(runtime::EQUAL, "eq"));
                self.builder.build_or(a, b, "ge").expect("an or")
            }
            // Unordered counts as differing, which is what makes a NaN unequal to itself.
            CmpOp::NotEqual => self
                .builder
                .build_int_compare(
                    IntPredicate::NE,
                    ordering,
                    i32_t.const_int(runtime::EQUAL, false),
                    "ne",
                )
                .expect("a compare"),
        }
    }

    /// Arithmetic, natively where that is certainly the same answer, and by calling back
    /// into `luarust-num` where it might not be.
    fn arithmetic(
        &mut self,
        op: BinOp,
        a: BasicValueEnum<'ctx>,
        b: BasicValueEnum<'ctx>,
        ty: Ty,
        span: Span,
    ) -> BasicValueEnum<'ctx> {
        // b16 has no instructions on either target, so all of it goes back.
        if ty == Ty::B16 {
            return self.call_fallback(op, a, b, ty, span);
        }
        if ty.is_float() {
            return match op {
                BinOp::Add => self.builder.build_float_add(a.into_float_value(), b.into_float_value(), "add").expect("add").into(),
                BinOp::Sub => self.builder.build_float_sub(a.into_float_value(), b.into_float_value(), "sub").expect("sub").into(),
                BinOp::Mul => self.builder.build_float_mul(a.into_float_value(), b.into_float_value(), "mul").expect("mul").into(),
                BinOp::Div => self.builder.build_float_div(a.into_float_value(), b.into_float_value(), "div").expect("div").into(),
                // A floored float remainder is several operations and one of them rounds,
                // so it goes back to the code the other paths use.
                _ => self.call_fallback(op, a, b, ty, span),
            };
        }

        // Trapping is opt-in and rare, and getting it exactly right in three places is
        // worth less than getting it right in one, so it goes back too.
        if self.overflow == Overflow::Trap {
            return self.call_fallback(op, a, b, ty, span);
        }

        let (x, y) = (a.into_int_value(), b.into_int_value());
        match op {
            BinOp::Add => self.builder.build_int_add(x, y, "add").expect("add").into(),
            BinOp::Sub => self.builder.build_int_sub(x, y, "sub").expect("sub").into(),
            BinOp::Mul => self.builder.build_int_mul(x, y, "mul").expect("mul").into(),
            // Division and remainder can fault, and the hardware would rather crash than
            // say so, so both are guarded before they happen.
            BinOp::Div | BinOp::Mod => self.int_division(op, x, y, ty, span).into(),
            BinOp::Pow => self.call_fallback(op, a, b, ty, span),
        }
    }

    /// Integer division and remainder, guarded.
    ///
    /// Two things the hardware will not survive being asked. Dividing by zero raises a
    /// fault the program has no way to catch, so it is checked first and the program stops
    /// itself. And the most negative value divided by `-1` has no answer that fits, which
    /// on x86-64 is a hardware trap rather than a wrong number — so that case is diverted
    /// through a harmless divisor and the right answer selected afterwards.
    ///
    /// The remainder is then floored, which costs one comparison and one add: `srem`
    /// takes the sign of the dividend, and mathematics takes the sign of the divisor.
    fn int_division(
        &mut self,
        op: BinOp,
        x: inkwell::values::IntValue<'ctx>,
        y: inkwell::values::IntValue<'ctx>,
        ty: Ty,
        span: Span,
    ) -> inkwell::values::IntValue<'ctx> {
        let int = self.int_type(ty);
        let zero = int.const_int(0, false);

        let is_zero = self
            .builder
            .build_int_compare(IntPredicate::EQ, y, zero, "divisor.is.zero")
            .expect("a compare");
        let code = if op == BinOp::Div { runtime::DIVIDE_BY_ZERO } else { runtime::REMAINDER_BY_ZERO };
        self.stop_if(is_zero, code, span);

        if !ty.is_signed() {
            return match op {
                BinOp::Div => self.builder.build_int_unsigned_div(x, y, "div").expect("div"),
                _ => self.builder.build_int_unsigned_rem(x, y, "rem").expect("rem"),
            };
        }

        // Divide by one instead of by minus one, and pick the answer afterwards.
        let minus_one = int.const_all_ones();
        let is_minus_one = self
            .builder
            .build_int_compare(IntPredicate::EQ, y, minus_one, "divisor.is.minus.one")
            .expect("a compare");
        let safe = self
            .builder
            .build_select(is_minus_one, int.const_int(1, false), y, "safe.divisor")
            .expect("a select")
            .into_int_value();

        if op == BinOp::Div {
            let quotient = self.builder.build_int_signed_div(x, safe, "div").expect("div");
            // Negating wraps, which is the answer wrapping arithmetic wants.
            let negated = self.builder.build_int_sub(zero, x, "neg").expect("sub");
            return self
                .builder
                .build_select(is_minus_one, negated, quotient, "div.result")
                .expect("a select")
                .into_int_value();
        }

        let remainder = self.builder.build_int_signed_rem(x, safe, "rem").expect("rem");
        let remainder = self
            .builder
            .build_select(is_minus_one, zero, remainder, "rem.result")
            .expect("a select")
            .into_int_value();

        // Floored: if the remainder is not zero and disagrees in sign with the divisor,
        // it is on the wrong side and the divisor is added back.
        let not_zero = self
            .builder
            .build_int_compare(IntPredicate::NE, remainder, zero, "rem.not.zero")
            .expect("a compare");
        let signs_differ = self
            .builder
            .build_int_compare(
                IntPredicate::SLT,
                self.builder.build_xor(remainder, y, "signs").expect("an xor"),
                zero,
                "signs.differ",
            )
            .expect("a compare");
        let needs_correcting = self.builder.build_and(not_zero, signs_differ, "needs").expect("an and");
        let corrected = self.builder.build_int_add(remainder, y, "corrected").expect("add");
        self.builder
            .build_select(needs_correcting, corrected, remainder, "floored")
            .expect("a select")
            .into_int_value()
    }

    /// Stop the program here, with this code, if the condition holds.
    fn stop_if(&mut self, condition: inkwell::values::IntValue<'ctx>, code: i64, span: Span) {
        let stop = self.context.append_basic_block(self.main, "stop");
        let carry_on = self.context.append_basic_block(self.main, "carry.on");
        self.builder.build_conditional_branch(condition, stop, carry_on).expect("a branch");

        self.builder.position_at_end(stop);
        let marker = self.fault_marker(span, code);
        self.builder
            .build_return(Some(&self.context.i64_type().const_int(marker as u64, false)))
            .expect("a return");

        self.builder.position_at_end(carry_on);
    }

    fn call_fallback(
        &mut self,
        op: BinOp,
        a: BasicValueEnum<'ctx>,
        b: BasicValueEnum<'ctx>,
        ty: Ty,
        span: Span,
    ) -> BasicValueEnum<'ctx> {
        let i32_t = self.context.i32_type();
        let i64_t = self.context.i64_type();

        let out = self.out_slot;
        let a_bits = self.to_bits(a, ty);
        let b_bits = self.to_bits(b, ty);
        let trapping = i32_t.const_int(u64::from(self.overflow == Overflow::Trap), false);

        let outcome = self
            .builder
            .build_call(
                self.helpers.fallback,
                &[
                    i32_t.const_int(runtime::op_tag(op) as u64, false).into(),
                    i32_t.const_int(runtime::tag_of(ty) as u64, false).into(),
                    a_bits.into(),
                    b_bits.into(),
                    trapping.into(),
                    out.into(),
                ],
                "fallback",
            )
            .expect("a call")
            .try_as_basic_value()
            .expect_basic("the fallback returns a value")
            .into_int_value();

        // Anything but zero means it could not be done, and the program stops there.
        let failed = self
            .builder
            .build_int_compare(IntPredicate::NE, outcome, i64_t.const_int(0, false), "failed")
            .expect("a compare");
        let stop = self.context.append_basic_block(self.main, "stop");
        let carry_on = self.context.append_basic_block(self.main, "carry.on");
        self.builder.build_conditional_branch(failed, stop, carry_on).expect("a branch");

        self.builder.position_at_end(stop);
        let marker = self.fault_marker(span, 0);
        let with_place = self
            .builder
            .build_or(outcome, i64_t.const_int(marker as u64, false), "where")
            .expect("an or");
        self.builder.build_return(Some(&with_place)).expect("a return");

        self.builder.position_at_end(carry_on);
        let raw = self.builder.build_load(i64_t, out, "result").expect("a load").into_int_value();
        self.value_from_bits(raw, ty)
    }

    /// The reverse of [`Self::to_bits`].
    fn value_from_bits(&self, bits: inkwell::values::IntValue<'ctx>, ty: Ty) -> BasicValueEnum<'ctx> {
        match ty {
            Ty::B64 => self.builder.build_bit_cast(bits, self.context.f64_type(), "f").expect("a cast"),
            Ty::B32 => {
                let narrow = self.builder.build_int_truncate(bits, self.context.i32_type(), "t").expect("a truncate");
                self.builder.build_bit_cast(narrow, self.context.f32_type(), "f").expect("a cast")
            }
            _ => {
                let width = if ty == Ty::B16 { 16 } else { ty.int_bits().unwrap_or(64) };
                if width == 64 {
                    bits.into()
                } else {
                    self.builder.build_int_truncate(bits, self.int_type(ty), "t").expect("a truncate").into()
                }
            }
        }
    }
}
