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

pub mod runtime;

use inkwell::OptimizationLevel;
use inkwell::builder::Builder;
use inkwell::context::Context;
use inkwell::execution_engine::ExecutionEngine;
use inkwell::module::Module;
use inkwell::values::{BasicValueEnum, FunctionValue, PointerValue};
use inkwell::{FloatPredicate, IntPredicate};
use luarust_check::ir::{Checked, Expr, Item, Stmt};
use luarust_check::value::{Fault, Overflow, Stopped, Value};
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
/// It takes all of them now. The types it has no instructions for — `b128`, `b256`,
/// `bool`, `str` — live in numbered cells on the Rust side, and compiled code carries the
/// number. This is kept as a function, and kept public, because it is the thing that would
/// have to say no again if a type arrived that had nowhere to live.
pub fn accepts(_program: &Checked) -> Result<(), Declined> {
    Ok(())
}

/// Whether a value of this type can live in a register, or has to live in a cell.
fn celled(ty: Ty) -> bool {
    // `bool` used to be one of these. It stopped being one when comparisons arrived and
    // started *producing* them: a truth value is one bit, and putting every comparison's
    // answer in a cell would be a call for something the machine settles in one
    // instruction.
    matches!(ty, Ty::B128 | Ty::B256 | Ty::Str)
}

/// Compile a program and run it, or hand it back.
pub fn run(program: &Checked, out: &mut impl Write) -> Result<Result<(), Stopped>, Declined> {
    accepts(program)?;

    let context = Context::create();
    let module = context.create_module("luarust");
    let engine = module
        .create_jit_execution_engine(OptimizationLevel::Aggressive)
        .map_err(|why| Declined { because: format!("LLVM would not start: {why}") })?;

    let mut emitter = Emitter::new(&context, &module, &engine, program);
    emitter.emit(program);
    let spans = std::mem::take(&mut emitter.spans);

    let compiled = unsafe {
        engine
            .get_function::<unsafe extern "C" fn() -> i64>("luarust_main")
            .map_err(|why| Declined { because: format!("the compiled program was lost: {why}") })?
    };

    runtime::begin(emitter.cells);
    let outcome = unsafe { compiled.call() };
    let _ = out.write_all(&runtime::taken());
    let _ = out.flush();

    Ok(decode(outcome, &spans))
}

/// The LLVM IR, for looking at.
pub fn emit_ir(program: &Checked) -> Result<String, Declined> {
    accepts(program)?;
    let context = Context::create();
    let module = context.create_module("luarust");
    let engine = module
        .create_jit_execution_engine(OptimizationLevel::Aggressive)
        .map_err(|why| Declined { because: format!("LLVM would not start: {why}") })?;
    let mut emitter = Emitter::new(&context, &module, &engine, program);
    emitter.emit(program);
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
        runtime::DOES_NOT_FIT => Fault {
            code: "R0005",
            message: "this does not fit the width it is stored at.".into(),
            rule: "with `defaults.overflow.trap`, a whole number must fit the width it is stored at",
            fix: "use a wider type, or drop `defaults.overflow.trap` and let it wrap.".into(),
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

/// Where a value ended up: in a register, or in a cell on the Rust side.
#[derive(Clone, Copy)]
enum Emitted<'ctx> {
    Native(BasicValueEnum<'ctx>),
    /// The cell's number, which is known while compiling and so costs nothing to carry.
    Cell(u64),
}

impl<'ctx> Emitted<'ctx> {
    fn native(self) -> BasicValueEnum<'ctx> {
        match self {
            Emitted::Native(value) => value,
            Emitted::Cell(_) => unreachable!("a celled value was used as a register one"),
        }
    }

    fn cell(self) -> u64 {
        match self {
            Emitted::Cell(index) => index,
            Emitted::Native(_) => unreachable!("a register value was used as a celled one"),
        }
    }
}

struct Emitter<'ctx> {
    context: &'ctx Context,
    builder: Builder<'ctx>,
    main: FunctionValue<'ctx>,
    /// One `alloca` per variable, in the order the checker numbered them. Celled
    /// variables never touch theirs.
    slots: Vec<PointerValue<'ctx>>,
    /// Where the fallback leaves its answer. Made once, in the entry block: an `alloca`
    /// anywhere else is stack allocated on every pass of whatever loop it is inside, which
    /// a ten-million-iteration program notices immediately by running out of stack.
    out_slot: PointerValue<'ctx>,
    /// What the cells start out holding. Handed to the runtime before the program runs.
    cells: Vec<Value>,
    /// Which cell each celled variable lives in.
    slot_cells: std::collections::HashMap<usize, u64>,
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
}

impl<'ctx> Emitter<'ctx> {
    fn new(
        context: &'ctx Context,
        module: &Module<'ctx>,
        engine: &ExecutionEngine<'ctx>,
        program: &Checked,
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

        let print_cell =
            module.add_function("luarust_print_cell", void_t.fn_type(&[i64_t.into()], false), None);
        engine.add_global_mapping(&print_cell, runtime::print_cell as *const () as usize);

        let main = module.add_function("luarust_main", i64_t.fn_type(&[], false), None);
        let entry = context.append_basic_block(main, "entry");
        let builder = context.create_builder();
        builder.position_at_end(entry);

        // Every variable gets its own stack slot. LLVM will keep the ones that deserve it
        // in registers.
        let mut slots = Vec::new();
        for index in 0..program.slots {
            let alloca = builder
                .build_alloca(context.i64_type(), &format!("slot{index}"))
                .expect("a stack slot");
            slots.push(alloca);
        }

        let out_slot = builder.build_alloca(context.i64_type(), "fallback.out").expect("a slot");

        Self {
            context,
            builder,
            main,
            slots,
            out_slot,
            spans: Vec::new(),
            overflow: program.overflow,
            cells: Vec::new(),
            slot_cells: std::collections::HashMap::new(),
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
            },
        }
    }

    fn emit(&mut self, program: &Checked) {
        self.block(&program.stmts);
        self.builder
            .build_return(Some(&self.context.i64_type().const_int(0, false)))
            .expect("a return");
    }

    /// Note where a fault could happen, and give back the value to return if it does.
    fn fault_marker(&mut self, span: Span, code: i64) -> i64 {
        let index = self.spans.len() as i64;
        self.spans.push(span);
        (index << 8) | code
    }

    fn block(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            self.stmt(stmt);
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Store { slot, value, span } => {
                let (emitted, ty) = self.expr(value);
                match emitted {
                    Emitted::Native(value) => {
                        // Whole-width, so nothing is left over from whatever was in the
                        // slot before. A narrow store into a wide slot leaves the top of
                        // it holding the last value that lived there.
                        let bits = self.to_bits(value, ty);
                        self.builder.build_store(self.slots[*slot], bits).expect("a store");
                    }
                    Emitted::Cell(src) => {
                        let dst = self.slot_cell(*slot, ty);
                        if dst != src {
                            self.call_cell_move(dst, src);
                        }
                    }
                }
                let _ = span;
            }

            Stmt::Print { items, span } => {
                for item in items {
                    match item {
                        Item::Text(text) => {
                            let global = self
                                .builder
                                .build_global_string_ptr(text, "text")
                                .expect("a string");
                            let len = self.context.i64_type().const_int(text.len() as u64, false);
                            self.builder
                                .build_call(
                                    self.helpers.print_text,
                                    &[global.as_pointer_value().into(), len.into()],
                                    "",
                                )
                                .expect("a call");
                        }
                        Item::Value(expr) => {
                            let (emitted, ty) = self.expr(expr);
                            match emitted {
                                Emitted::Native(value) => {
                                    let bits = self.to_bits(value, ty);
                                    let tag = self
                                        .context
                                        .i32_type()
                                        .const_int(runtime::tag_of(ty) as u64, false);
                                    self.builder
                                        .build_call(
                                            self.helpers.print_value,
                                            &[bits.into(), tag.into()],
                                            "",
                                        )
                                        .expect("a call");
                                }
                                Emitted::Cell(index) => {
                                    let cell = self.cell_number(index);
                                    self.builder
                                        .build_call(self.helpers.print_cell, &[cell.into()], "")
                                        .expect("a call");
                                }
                            }
                        }
                    }
                }
                let _ = span;
            }

            Stmt::Loop { slot, ty, from, to, body, span } => {
                self.loop_stmt(*slot, *ty, from, to, body, *span)
            }
        }
    }

    fn loop_stmt(&mut self, slot: usize, ty: Ty, from: &Expr, to: &Expr, body: &[Stmt], span: Span) {
        // Start the counter, and take the far bound somewhere it will survive the body.
        let (start, _) = self.expr(from);
        let counter = match start {
            Emitted::Native(value) => {
                let bits = self.to_bits(value, ty);
                self.builder.build_store(self.slots[slot], bits).expect("a store");
                None
            }
            Emitted::Cell(src) => {
                let dst = self.slot_cell(slot, ty);
                if dst != src {
                    self.call_cell_move(dst, src);
                }
                Some(dst)
            }
        };
        let (limit, _) = self.expr(to);

        let check = self.context.append_basic_block(self.main, "loop.check");
        let top = self.context.append_basic_block(self.main, "loop.top");
        let step = self.context.append_basic_block(self.main, "loop.step");
        let done = self.context.append_basic_block(self.main, "loop.done");

        self.builder.build_unconditional_branch(check).expect("a branch");

        // Counting down is an empty range, so the first thing asked is whether to run at
        // all rather than whether to stop.
        self.builder.position_at_end(check);
        let past = self.loop_test(counter, slot, ty, limit, CmpOp::Greater);
        self.builder.build_conditional_branch(past, done, top).expect("a branch");

        self.builder.position_at_end(top);
        self.block(body);
        let finished = self.loop_test(counter, slot, ty, limit, CmpOp::Equal);
        self.builder.build_conditional_branch(finished, done, step).expect("a branch");

        // Stepping only after the body, and only below the bound, so a loop can reach the
        // top of its type without the increment that would take it past.
        self.builder.position_at_end(step);
        match counter {
            None => {
                let held = self.load(slot, ty);
                let one = self.one(ty);
                let next = self.arithmetic(BinOp::Add, held, one, ty, span);
                let bits = self.to_bits(next, ty);
                self.builder.build_store(self.slots[slot], bits).expect("a store");
            }
            Some(cell) => {
                let one = self.constant_cell(luarust_check::value::one_of(ty));
                self.call_cell_binary(BinOp::Add, cell, cell, one, span);
            }
        }
        self.builder.build_unconditional_branch(top).expect("a branch");

        self.builder.position_at_end(done);
    }

    /// Whether the counter stands in the given relation to the bound.
    fn loop_test(
        &mut self,
        counter: Option<u64>,
        slot: usize,
        ty: Ty,
        limit: Emitted<'ctx>,
        wanted: CmpOp,
    ) -> inkwell::values::IntValue<'ctx> {
        match counter {
            None => {
                let held = self.load(slot, ty);
                self.relation(held, limit.native(), ty, wanted)
            }
            Some(cell) => self.cells_compare(cell, limit.cell(), wanted),
        }
    }

    // ---- cells ------------------------------------------------------------------

    /// A new cell, holding this to begin with.
    fn new_cell(&mut self, initial: Value) -> u64 {
        self.cells.push(initial);
        (self.cells.len() - 1) as u64
    }

    /// A cell holding a constant, which nothing ever writes to.
    fn constant_cell(&mut self, value: Value) -> u64 {
        self.new_cell(value)
    }

    /// The cell a celled variable lives in, made the first time it is stored to.
    fn slot_cell(&mut self, slot: usize, ty: Ty) -> u64 {
        if let Some(found) = self.slot_cells.get(&slot) {
            return *found;
        }
        let index = self.new_cell(Value::zero(ty));
        self.slot_cells.insert(slot, index);
        index
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

    fn expr(&mut self, expr: &Expr) -> (Emitted<'ctx>, Ty) {
        match expr {
            Expr::Const(value) => {
                if celled(value.ty()) {
                    let cell = self.constant_cell(value.clone());
                    return (Emitted::Cell(cell), value.ty());
                }
                (Emitted::Native(self.constant(value)), value.ty())
            }

            Expr::Load { slot, ty, .. } => {
                if celled(*ty) {
                    return (Emitted::Cell(self.slot_cell(*slot, *ty)), *ty);
                }
                (Emitted::Native(self.load(*slot, *ty)), *ty)
            }

            Expr::TimeNow { ty, span } => {
                // Asked for in the format it is being read as. Answering in b64 whatever
                // was wanted would put a b64's bits in a b32 variable.
                let tag = self.context.i32_type().const_int(runtime::tag_of(*ty) as u64, false);
                if celled(*ty) {
                    let dst = self.new_cell(Value::zero(*ty));
                    let number = self.cell_number(dst);
                    self.builder
                        .build_call(self.helpers.cell_time_now, &[number.into(), tag.into()], "")
                        .expect("a call");
                    return (Emitted::Cell(dst), *ty);
                }
                let bits = self
                    .builder
                    .build_call(self.helpers.time_now, &[tag.into()], "now")
                    .expect("a call")
                    .try_as_basic_value()
                    .expect_basic("the clock returns a value")
                    .into_int_value();
                let _ = span;
                (Emitted::Native(self.value_from_bits(bits, *ty)), *ty)
            }

            Expr::Neg { operand, ty, span } => {
                let (value, _) = self.expr(operand);
                if celled(*ty) {
                    let dst = self.new_cell(Value::zero(*ty));
                    let i32_t = self.context.i32_type();
                    let trapping = i32_t.const_int(u64::from(self.overflow == Overflow::Trap), false);
                    let outcome = self
                        .builder
                        .build_call(
                            self.helpers.cell_neg,
                            &[
                                self.cell_number(dst).into(),
                                self.cell_number(value.cell()).into(),
                                trapping.into(),
                            ],
                            "negated",
                        )
                        .expect("a call")
                        .try_as_basic_value()
                        .expect_basic("it returns a fault code")
                        .into_int_value();
                    self.stop_if_nonzero(outcome, *span);
                    return (Emitted::Cell(dst), *ty);
                }
                let value = value.native();
                // Negating a float is flipping its sign bit and nothing else, which is
                // exact for every value including the zeros and the NaNs. Not `0 - x`:
                // `0.0 - 0.0` is `+0.0` where negating a zero gives `-0.0`.
                if *ty == Ty::B16 {
                    let sign = self.context.i16_type().const_int(0x8000, false);
                    let flipped = self
                        .builder
                        .build_xor(value.into_int_value(), sign, "neg")
                        .expect("an xor");
                    return (Emitted::Native(flipped.into()), *ty);
                }
                if ty.is_float() {
                    let negated = self
                        .builder
                        .build_float_neg(value.into_float_value(), "neg")
                        .expect("a negate");
                    return (Emitted::Native(negated.into()), *ty);
                }
                let zero = self.zero(*ty);
                (Emitted::Native(self.arithmetic(BinOp::Sub, zero, value, *ty, *span)), *ty)
            }

            Expr::Compare { op, operands, lhs, rhs, .. } => {
                let (a, _) = self.expr(lhs);
                let (b, _) = self.expr(rhs);
                let truth = if celled(*operands) {
                    self.cells_compare(a.cell(), b.cell(), *op)
                } else {
                    self.relation(a.native(), b.native(), *operands, *op)
                };
                // One bit widened to the sixty-four a slot holds.
                let widened = self
                    .builder
                    .build_int_z_extend(truth, self.context.i64_type(), "truth")
                    .expect("an extend");
                (Emitted::Native(widened.into()), Ty::Bool)
            }

            Expr::Binary { op, lhs, rhs, ty, span } => {
                let (a, _) = self.expr(lhs);
                let (b, _) = self.expr(rhs);
                if celled(*ty) {
                    let dst = self.new_cell(Value::zero(*ty));
                    self.call_cell_binary(*op, dst, a.cell(), b.cell(), *span);
                    return (Emitted::Cell(dst), *ty);
                }
                (
                    Emitted::Native(self.arithmetic(*op, a.native(), b.native(), *ty, *span)),
                    *ty,
                )
            }
        }
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
    fn load(&self, slot: usize, ty: Ty) -> BasicValueEnum<'ctx> {
        let raw = self
            .builder
            .build_load(self.context.i64_type(), self.slots[slot], "load")
            .expect("a load")
            .into_int_value();
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

    /// One, of whichever type — taken from the same place every other execution path takes
    /// it, because "one" is not the integer 1 in every format. As a `b16` encoding, `1` is
    /// the smallest subnormal there is, and a loop counting up by it never arrives.
    fn one(&self, ty: Ty) -> BasicValueEnum<'ctx> {
        let Value::Num { bits, .. } = luarust_check::value::one_of(ty) else {
            unreachable!("a number")
        };
        match ty {
            Ty::B32 => self.context.f32_type().const_float(f64::from(f32::from_bits(bits as u32))).into(),
            Ty::B64 => self.context.f64_type().const_float(f64::from_bits(bits)).into(),
            _ => self.int_type(ty).const_int(bits, false).into(),
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
