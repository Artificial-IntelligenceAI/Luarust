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
use luarust_parse::ast::{BinOp, Ty};
use std::io::Write;

/// Why the JIT handed a program back rather than compiling it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Declined {
    pub because: String,
}

/// Whether the JIT will take a program at all.
///
/// Cheap, and answered before any LLVM machinery is built, so a caller can fall back
/// without having paid for a context.
pub fn accepts(program: &Checked) -> Result<(), Declined> {
    fn ty_ok(ty: Ty) -> bool {
        ty.is_integer() || matches!(ty, Ty::B32 | Ty::B64)
    }
    fn expr(e: &Expr) -> Result<(), Declined> {
        if !ty_ok(e.ty()) {
            return Err(Declined { because: format!("`{}` is not compiled yet", e.ty().word()) });
        }
        match e {
            Expr::Binary { op: BinOp::Pow, .. } => {
                Err(Declined { because: "raising to a power is not compiled yet".into() })
            }
            Expr::Binary { lhs, rhs, .. } => expr(lhs).and_then(|()| expr(rhs)),
            Expr::Neg { operand, .. } => expr(operand),
            Expr::Const(value) => match value {
                Value::Num { ty, .. } if ty_ok(*ty) => Ok(()),
                other => Err(Declined {
                    because: format!("`{}` is not compiled yet", other.ty().word()),
                }),
            },
            _ => Ok(()),
        }
    }
    fn block(stmts: &[Stmt]) -> Result<(), Declined> {
        for stmt in stmts {
            match stmt {
                Stmt::Store { value, .. } => expr(value)?,
                Stmt::Print { items, .. } => {
                    for item in items {
                        if let Item::Value(e) = item {
                            expr(e)?;
                        }
                    }
                }
                Stmt::Loop { ty, from, to, body, .. } => {
                    if !ty_ok(*ty) {
                        return Err(Declined {
                            because: format!("counting in `{}` is not compiled yet", ty.word()),
                        });
                    }
                    expr(from)?;
                    expr(to)?;
                    block(body)?;
                }
            }
        }
        Ok(())
    }
    block(&program.stmts)
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

    let compiled = unsafe {
        engine
            .get_function::<unsafe extern "C" fn() -> i64>("luarust_main")
            .map_err(|why| Declined { because: format!("the compiled program was lost: {why}") })?
    };

    runtime::begin();
    let outcome = unsafe { compiled.call() };
    let _ = out.write_all(&runtime::taken());
    let _ = out.flush();

    Ok(decode(outcome, &emitter.spans))
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

struct Emitter<'ctx> {
    context: &'ctx Context,
    builder: Builder<'ctx>,
    main: FunctionValue<'ctx>,
    /// One `alloca` per variable, in the order the checker numbered them.
    slots: Vec<PointerValue<'ctx>>,
    types: Vec<Ty>,
    /// Where the fallback leaves its answer. Made once, in the entry block: an `alloca`
    /// anywhere else is stack allocated on every pass of whatever loop it is inside, which
    /// a ten-million-iteration program notices immediately by running out of stack.
    out_slot: PointerValue<'ctx>,
    /// Where each fault can happen, so a code can be turned back into a place.
    spans: Vec<Span>,
    overflow: Overflow,
    helpers: Helpers<'ctx>,
}

struct Helpers<'ctx> {
    print_text: FunctionValue<'ctx>,
    print_value: FunctionValue<'ctx>,
    time_now: FunctionValue<'ctx>,
    fallback: FunctionValue<'ctx>,
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

        let time_now = module.add_function("luarust_time_now", i64_t.fn_type(&[], false), None);
        engine.add_global_mapping(&time_now, runtime::time_now as *const () as usize);

        let fallback = module.add_function(
            "luarust_fallback",
            i64_t.fn_type(
                &[i32_t.into(), i32_t.into(), i64_t.into(), i64_t.into(), i32_t.into(), ptr_t.into()],
                false,
            ),
            None,
        );
        engine.add_global_mapping(&fallback, runtime::fallback as *const () as usize);

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
            types: vec![Ty::I64; program.slots],
            spans: Vec::new(),
            overflow: program.overflow,
            helpers: Helpers { print_text, print_value, time_now, fallback },
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
            Stmt::Store { slot, value, .. } => {
                let value = self.expr(value);
                self.types[*slot] = value.1;
                self.builder.build_store(self.slots[*slot], value.0).expect("a store");
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
                            let (value, ty) = self.expr(expr);
                            let bits = self.to_bits(value, ty);
                            let tag = self.context.i32_type().const_int(runtime::tag_of(ty) as u64, false);
                            self.builder
                                .build_call(self.helpers.print_value, &[bits.into(), tag.into()], "")
                                .expect("a call");
                        }
                    }
                }
                let _ = span;
            }

            Stmt::Loop { slot, ty, from, to, body, span } => self.loop_stmt(*slot, *ty, from, to, body, *span),
        }
    }

    fn loop_stmt(&mut self, slot: usize, ty: Ty, from: &Expr, to: &Expr, body: &[Stmt], span: Span) {
        let (start, _) = self.expr(from);
        self.builder.build_store(self.slots[slot], start).expect("a store");
        self.types[slot] = ty;
        let (limit, _) = self.expr(to);

        let check = self.context.append_basic_block(self.main, "loop.check");
        let top = self.context.append_basic_block(self.main, "loop.top");
        let step = self.context.append_basic_block(self.main, "loop.step");
        let done = self.context.append_basic_block(self.main, "loop.done");

        self.builder.build_unconditional_branch(check).expect("a branch");

        // Counting down is an empty range, so the first thing asked is whether to run at
        // all rather than whether to stop.
        self.builder.position_at_end(check);
        let counter = self.load(slot, ty);
        let past = self.greater(counter, limit, ty);
        self.builder.build_conditional_branch(past, done, top).expect("a branch");

        self.builder.position_at_end(top);
        self.block(body);
        let counter = self.load(slot, ty);
        let finished = self.equal(counter, limit, ty);
        self.builder.build_conditional_branch(finished, done, step).expect("a branch");

        // Stepping only after the body, and only below the bound, so a loop can reach the
        // top of its type without the increment that would take it past.
        self.builder.position_at_end(step);
        let counter = self.load(slot, ty);
        let one = self.one(ty);
        let next = self.arithmetic(BinOp::Add, counter, one, ty, span);
        self.builder.build_store(self.slots[slot], next).expect("a store");
        self.builder.build_unconditional_branch(top).expect("a branch");

        self.builder.position_at_end(done);
    }

    // ---- values -----------------------------------------------------------------

    fn expr(&mut self, expr: &Expr) -> (BasicValueEnum<'ctx>, Ty) {
        match expr {
            Expr::Const(value) => (self.constant(value), value.ty()),

            Expr::Load { slot, ty, .. } => (self.load(*slot, *ty), *ty),

            Expr::TimeNow { ty, .. } => {
                let bits = self
                    .builder
                    .build_call(self.helpers.time_now, &[], "now")
                    .expect("a call")
                    .try_as_basic_value()
                    .expect_basic("the clock returns a value")
                    .into_int_value();
                let as_float = self
                    .builder
                    .build_bit_cast(bits, self.context.f64_type(), "seconds")
                    .expect("a cast");
                (as_float, *ty)
            }

            Expr::Neg { operand, ty, span } => {
                let (value, _) = self.expr(operand);
                if ty.is_float() {
                    // Not `0 - x`: `0.0 - 0.0` is `+0.0` where negating a zero gives
                    // `-0.0`, and the other two paths flip the sign bit. One bit, in one
                    // value, that the fuzzer would eventually have found.
                    let negated = self
                        .builder
                        .build_float_neg(value.into_float_value(), "neg")
                        .expect("a negate");
                    return (negated.into(), *ty);
                }
                let zero = self.zero(*ty);
                (self.arithmetic(BinOp::Sub, zero, value, *ty, *span), *ty)
            }

            Expr::Binary { op, lhs, rhs, ty, span } => {
                let (a, _) = self.expr(lhs);
                let (b, _) = self.expr(rhs);
                (self.arithmetic(*op, a, b, *ty, *span), *ty)
            }
        }
    }

    fn constant(&self, value: &Value) -> BasicValueEnum<'ctx> {
        let Value::Num { ty, bits } = value else { unreachable!("declined earlier") };
        match ty {
            Ty::B32 => {
                let float = f32::from_bits(*bits as u32);
                self.context.f32_type().const_float(float as f64).into()
            }
            Ty::B64 => self.context.f64_type().const_float(f64::from_bits(*bits)).into(),
            _ => self.int_type(*ty).const_int(*bits, false).into(),
        }
    }

    fn int_type(&self, ty: Ty) -> inkwell::types::IntType<'ctx> {
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
                let width = ty.int_bits().unwrap_or(64);
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
                if ty.int_bits().unwrap_or(64) == 64 {
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

    fn one(&self, ty: Ty) -> BasicValueEnum<'ctx> {
        match ty {
            Ty::B32 => self.context.f32_type().const_float(1.0).into(),
            Ty::B64 => self.context.f64_type().const_float(1.0).into(),
            _ => self.int_type(ty).const_int(1, false).into(),
        }
    }

    fn greater(&self, a: BasicValueEnum<'ctx>, b: BasicValueEnum<'ctx>, ty: Ty) -> inkwell::values::IntValue<'ctx> {
        if ty.is_float() {
            self.builder
                .build_float_compare(FloatPredicate::OGT, a.into_float_value(), b.into_float_value(), "gt")
                .expect("a compare")
        } else {
            let predicate = if ty.is_signed() { IntPredicate::SGT } else { IntPredicate::UGT };
            self.builder
                .build_int_compare(predicate, a.into_int_value(), b.into_int_value(), "gt")
                .expect("a compare")
        }
    }

    fn equal(&self, a: BasicValueEnum<'ctx>, b: BasicValueEnum<'ctx>, ty: Ty) -> inkwell::values::IntValue<'ctx> {
        if ty.is_float() {
            self.builder
                .build_float_compare(FloatPredicate::OEQ, a.into_float_value(), b.into_float_value(), "eq")
                .expect("a compare")
        } else {
            self.builder
                .build_int_compare(IntPredicate::EQ, a.into_int_value(), b.into_int_value(), "eq")
                .expect("a compare")
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
                let width = ty.int_bits().unwrap_or(64);
                if width == 64 {
                    bits.into()
                } else {
                    self.builder.build_int_truncate(bits, self.int_type(ty), "t").expect("a truncate").into()
                }
            }
        }
    }
}
