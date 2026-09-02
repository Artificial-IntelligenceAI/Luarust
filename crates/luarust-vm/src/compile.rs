//! Turning a checked program into bytecode.
//!
//! The checker has already numbered every variable, so those numbers are the low
//! registers and the compiler only has to find room for the values it makes along the way.
//! Temporaries are handed out above the variables and given back at the end of each
//! statement, since nothing a statement computes outlives it — except a loop's far bound,
//! which has to survive its body and is parked below the body's temporaries for exactly
//! that reason.

use crate::chunk::{Chunk, Op, Reg, Routine};
use luarust_core::heap::Collect;
use luarust_core::value::{Division, Engine, Floats, Insistence};
use luarust_check::ir::{Checked, Expr, Item, Stmt};
use luarust_check::value::{Overflow, Value, one_of};
use luarust_diag::Span;
use luarust_parse::ast::{BinOp, LogicOp, Ty};

/// Compile a checked program.
///
/// Twice, on purpose. The first pass exists only to find out which constants the program
/// uses; the second gives each of them a register of its own, loaded once before anything
/// runs. Without that, a loop reloads the same constant on every pass — `mod 1000000007`
/// costs a load and a remainder rather than a remainder — and it is the innermost
/// instruction in the hottest place a program has.
pub fn compile(program: &Checked) -> Chunk {
    // Two passes: the first only to learn which constants the top level uses, the second
    // to preload them into registers before anything runs.
    let scouted = Compiler::new(program, Vec::new()).run(program);
    let mut chunk = Compiler::new(program, scouted.consts).run(program);

    // Each function gets its own code and its own register file. Constants are not
    // preloaded inside one -- the pool is shared and a function uses an arbitrary part of
    // it, so there is no prefix to hoist. A `Const` per use instead, which is one
    // instruction and the obvious thing to improve later.
    for func in &program.funcs {
        let mut inner = Compiler::for_function(func, std::mem::take(&mut chunk.consts));
        inner.chunk.texts = std::mem::take(&mut chunk.texts);
        inner.block(&func.body);
        if func.returns.is_none() {
            inner.emit(Op::ReturnNothing, func.span);
        }
        chunk.consts = std::mem::take(&mut inner.chunk.consts);
        chunk.texts = std::mem::take(&mut inner.chunk.texts);
        chunk.funcs.push(Routine {
            code: inner.chunk.code,
            spans: inner.chunk.spans,
            registers: inner.chunk.registers,
            params: func.params.clone(),
            returns: func.returns,
        });
    }
    chunk
}

struct Compiler {
    chunk: Chunk,
    /// Where temporaries start. Raised while compiling a loop body, so that the body
    /// cannot reuse the registers the loop itself is still holding.
    floor: Reg,
    /// The next free temporary.
    next: Reg,
    /// Jumps waiting for the end of the loop being compiled, one per `break` inside it.
    /// A `break` leaves the innermost loop, which is the one on top.
    breaks: Vec<Vec<usize>>,
    /// The first register holding a constant, once the constants are known.
    const_base: Reg,
    /// How many of them there are. Zero on the first pass, when they are not known yet.
    preloaded: usize,
}

impl Compiler {
    fn new(program: &Checked, consts: Vec<Value>) -> Self {
        let preloaded = consts.len();
        let base = program.slots as Reg;
        let temps = base + preloaded as Reg;
        Self {
            chunk: Chunk {
                code: Vec::new(),
                spans: Vec::new(),
                consts,
                texts: Vec::new(),
                registers: temps as usize,
                overflow: program.overflow,
                collect: program.collect,
                floats: program.floats,
                engine: program.engine,
                insistence: program.insistence,
                division: program.division,
                funcs: Vec::new(),
            },
            floor: temps,
            next: temps,
            breaks: Vec::new(),
            const_base: base,
            preloaded,
        }
    }

    /// A compiler for one function body. Its registers begin with the parameters, in
    /// order, which is what lets a call put the arguments straight where they belong.
    fn for_function(func: &luarust_check::ir::Function, consts: Vec<Value>) -> Self {
        let base = func.slots as Reg;
        Self {
            chunk: Chunk {
                code: Vec::new(),
                spans: Vec::new(),
                consts,
                texts: Vec::new(),
                registers: func.slots,
                overflow: Overflow::Wrap,
                // A function body's chunk is scaffolding: its code is lifted into the
                // program's own chunk, and these two are read from that one.
                collect: Collect::default(),
                floats: Floats::default(),
                engine: Engine::default(),
                insistence: Insistence::default(),
                division: Division::default(),
                funcs: Vec::new(),
            },
            floor: base,
            next: base,
            breaks: Vec::new(),
            const_base: base,
            // Nothing preloaded, so `const_operand` emits a `Const` where it is used.
            preloaded: 0,
        }
    }

    fn run(mut self, program: &Checked) -> Chunk {
        // Every constant into its own register, once, before anything else happens.
        for index in 0..self.preloaded {
            self.emit(
                Op::Const { dst: self.const_base + index as Reg, konst: index as u32 },
                Span::default(),
            );
        }
        self.block(&program.stmts);
        self.emit(Op::Halt, Span::default());
        self.chunk
    }

    fn emit(&mut self, op: Op, span: Span) -> usize {
        self.chunk.code.push(op);
        self.chunk.spans.push(span);
        self.chunk.code.len() - 1
    }

    fn here(&self) -> u32 {
        self.chunk.code.len() as u32
    }

    /// Point a jump written earlier at wherever we are now.
    fn land(&mut self, at: usize) {
        let target = self.here();
        match &mut self.chunk.code[at] {
            Op::Jump { target: t }
            | Op::JumpIfGreater { target: t, .. }
            | Op::JumpIfEqual { target: t, .. }
            | Op::JumpIfFalse { target: t, .. }
            | Op::JumpIfTrue { target: t, .. } => *t = target,
            other => panic!("tried to point {other:?} somewhere"),
        }
    }

    fn temp(&mut self) -> Reg {
        let reg = self.next;
        self.next += 1;
        self.chunk.registers = self.chunk.registers.max(self.next as usize);
        reg
    }

    /// Give every temporary back. Called between statements.
    fn release(&mut self) {
        self.next = self.floor;
    }

    fn konst(&mut self, value: Value) -> u32 {
        if let Some(found) = self.chunk.consts.iter().position(|existing| Self::identical(existing, &value))
        {
            return found as u32;
        }
        self.chunk.consts.push(value);
        (self.chunk.consts.len() - 1) as u32
    }

    /// Whether two constants are the same *thing*, rather than the same *number*.
    ///
    /// The pool cannot dedupe with `==`. A `Value` compares numerically, and `-0` and `0`
    /// are numerically equal -- so a program that wrote `-0` and then `0` got one slot
    /// holding `-0`, and its `0` quietly became `-0`. The pool wants sameness of
    /// representation: the same type, and the same bits.
    fn identical(a: &Value, b: &Value) -> bool {
        match (a, b) {
            (Value::Num { ty: x, bits: m }, Value::Num { ty: y, bits: n }) => x == y && m == n,
            (Value::Wide { ty: x, bits: m }, Value::Wide { ty: y, bits: n }) => x == y && m == n,
            (Value::Bool(x), Value::Bool(y)) => x == y,
            (Value::Str(x), Value::Str(y)) => x == y,
            (Value::Exact(x), Value::Exact(y)) => x == y,
            _ => false,
        }
    }

    fn text(&mut self, value: &str) -> u32 {
        if let Some(found) = self.chunk.texts.iter().position(|existing| existing == value) {
            return found as u32;
        }
        self.chunk.texts.push(value.to_string());
        (self.chunk.texts.len() - 1) as u32
    }

    fn block(&mut self, stmts: &[Stmt]) {
        for stmt in stmts {
            self.stmt(stmt);
            self.release();
        }
    }

    fn stmt(&mut self, stmt: &Stmt) {
        match stmt {
            Stmt::Store { slot, value, span } => {
                // Straight into the variable's own register, with no copy in between.
                self.expr(value, *slot as Reg, *span);
            }

            Stmt::Print { items, span } => {
                for item in items {
                    match item {
                        Item::Text(text) => {
                            let index = self.text(text);
                            self.emit(Op::PrintText { text: index }, *span);
                        }
                        Item::Value(expr) => {
                            let ty = expr.ty();
                            let reg = self.operand(expr, *span);
                            self.emit(Op::PrintValue { src: reg, ty }, *span);
                            self.release();
                        }
                    }
                }
            }

            Stmt::Loop { slot, ty, from, to, body, span } => {
                let counter = *slot as Reg;
                self.expr(from, counter, *span);

                // The far bound and the step have to outlive the body, so they are taken
                // before the floor is raised and the body starts handing out its own. A
                // constant bound needs no register of its own at all -- it already has one.
                let limit = self.operand(to, *span);
                let step = self.const_operand(one_of(*ty), *span);

                let outer_floor = self.floor;
                self.floor = self.next;
                self.breaks.push(Vec::new());

                // Counting down is an empty range, so leave before running anything.
                let skip = self.emit(
                    Op::JumpIfGreater { lhs: counter, rhs: limit, ty: *ty, target: 0 },
                    *span,
                );

                let top = self.here();
                self.block(body);

                // Step only while the counter is below the bound, so a loop can reach the
                // top of its type without the increment that would take it past.
                let done = self.emit(
                    Op::JumpIfEqual { lhs: counter, rhs: limit, ty: *ty, target: 0 },
                    *span,
                );
                self.emit(
                    Op::Binary { op: BinOp::Add, ty: *ty, dst: counter, lhs: counter, rhs: step, nonnegative: false },
                    *span,
                );
                self.emit(Op::Jump { target: top }, *span);

                self.land(skip);
                self.land(done);
                for jump in self.breaks.pop().expect("a loop opened one") {
                    self.land(jump);
                }

                self.floor = outer_floor;
                self.release();
            }

            // Each arm jumps over the next when its condition held, so exactly one body
            // runs. The conditions after it are never reached, let alone asked.
            Stmt::If { arms, otherwise, span } => {
                let mut leaving = Vec::new();
                for arm in arms {
                    let mark = self.next;
                    let cond = self.operand(&arm.condition, *span);
                    let skip = self.emit(Op::JumpIfFalse { cond, target: 0 }, *span);
                    self.next = mark;

                    let outer_floor = self.floor;
                    self.floor = self.next;
                    self.block(&arm.body);
                    self.floor = outer_floor;

                    // Nothing after a body that ran is wanted -- but an `if` with no
                    // `else` and one arm has nothing to jump over, so it does not.
                    if arms.len() > 1 || !otherwise.is_empty() {
                        leaving.push(self.emit(Op::Jump { target: 0 }, *span));
                    }
                    self.land(skip);
                }

                let outer_floor = self.floor;
                self.floor = self.next;
                self.block(otherwise);
                self.floor = outer_floor;

                for jump in leaving {
                    self.land(jump);
                }
                self.release();
            }

            // The condition before every pass, and the counter one higher after each.
            Stmt::While { counter, condition, body, span } => {
                if let Some((slot, ty)) = counter {
                    self.expr(&Expr::Const(Value::zero(*ty)), *slot as Reg, *span);
                }

                let outer_floor = self.floor;
                self.floor = self.next;
                self.breaks.push(Vec::new());

                let top = self.here();
                let mark = self.next;
                let held = self.operand(condition, *span);
                let done = self.emit(Op::JumpIfFalse { cond: held, target: 0 }, *span);
                self.next = mark;

                // Counted at the start of the pass, so afterwards it holds however many
                // ran rather than one more than that.
                if let Some((slot, ty)) = counter {
                    let step = self.const_operand(one_of(*ty), *span);
                    let reg = *slot as Reg;
                    self.emit(
                        Op::Binary { op: BinOp::Add, ty: *ty, dst: reg, lhs: reg, rhs: step, nonnegative: false },
                        *span,
                    );
                }
                self.block(body);
                self.emit(Op::Jump { target: top }, *span);

                self.land(done);
                for jump in self.breaks.pop().expect("a loop opened one") {
                    self.land(jump);
                }
                self.floor = outer_floor;
                self.release();
            }

            Stmt::StoreAt { array, at, value, span } => {
                let mark = self.next;
                let ty = array.ty();
                let target = self.operand(array, *span);
                let (base, rank) = self.arguments(at, *span);
                let held = self.operand(value, *span);
                self.emit(
                    Op::StoreAt { array: target, at: base, rank: rank as u8, value: held, ty },
                    *span,
                );
                self.next = mark;
            }

            Stmt::Break { span } => {
                let jump = self.emit(Op::Jump { target: 0 }, *span);
                self.breaks.last_mut().expect("`break` outside a loop was checked for").push(jump);
            }

            Stmt::Return { value, span } => {
                match value {
                    Some(expr) => {
                        let mark = self.next;
                        let ty = expr.ty();
                        let src = self.operand(expr, *span);
                        self.emit(Op::Return { src, ty }, *span);
                        self.next = mark;
                    }
                    None => {
                        self.emit(Op::ReturnNothing, *span);
                    }
                }
            }

            // Called for what it does. The answer, if there is one, goes to a temporary
            // that is released immediately.
            Stmt::Call { func, args, span } => {
                let mark = self.next;
                let (base, argc) = self.arguments(args, *span);
                let dst = self.temp();
                self.emit(Op::Call { func: *func as u32, base, argc, dst }, *span);
                self.next = mark;
            }
        }
    }

    /// Put the arguments in consecutive registers, which is how a call hands them over.
    ///
    /// Every register is claimed before any is filled. Taking them one at a time looks
    /// the same and is not: working out an argument can leave the temporary counter
    /// raised, and then the next argument lands a register or two further along than the
    /// call is going to look. It cost a fuzzer 46,316 programs to find that.
    fn arguments(&mut self, args: &[Expr], span: Span) -> (Reg, u16) {
        // A single index looks like it needs no register of its own -- it is one value,
        // and staging it copies the loop counter into a temp on every pass, one
        // instruction in six of an array loop. Reading it where it lives makes the VM
        // about ten per cent faster on such a loop, and was tried and taken back out.
        //
        // The copy is load-bearing for the compiled path. With it, the value the address
        // is computed from is its own thing and LLVM vectorises the loop; without it the
        // address is derived from the loop counter directly and it stops -- thirteen
        // milliseconds to seventeen here, and vectorisation is worth thirteen times on
        // array loops, which ten per cent of the interpreter does not buy.
        //
        // `tests/optimised.rs` is what caught it. Worth revisiting when the checker can
        // prove an index in range, since the proof is what the vectoriser is missing.
        let base = self.next;
        let claimed: Vec<Reg> = args.iter().map(|_| self.temp()).collect();
        let mark = self.next;
        for (arg, reg) in args.iter().zip(claimed) {
            self.expr(arg, reg, span);
            self.next = mark;
        }
        (base, args.len() as u16)
    }

    /// A register holding this expression's value, without copying when there is already
    /// one — a variable is read where it lives, and so is a constant.
    fn operand(&mut self, expr: &Expr, span: Span) -> Reg {
        match expr {
            Expr::Load { slot, .. } => *slot as Reg,
            Expr::Const(value) => self.const_operand(value.clone(), span),
            _ => {
                let reg = self.temp();
                self.expr(expr, reg, span);
                reg
            }
        }
    }

    /// The register a constant already lives in, or a fresh one on the first pass.
    fn const_operand(&mut self, value: Value, span: Span) -> Reg {
        let index = self.konst(value);
        if (index as usize) < self.preloaded {
            return self.const_base + index as Reg;
        }
        let reg = self.temp();
        self.emit(Op::Const { dst: reg, konst: index }, span);
        reg
    }

    /// Emit whatever puts this expression's value in `dst`.
    fn expr(&mut self, expr: &Expr, dst: Reg, span: Span) {
        match expr {
            Expr::Const(value) => {
                let index = self.konst(value.clone());
                self.emit(Op::Const { dst, konst: index }, span);
            }

            Expr::Load { slot, ty, .. } => {
                let src = *slot as Reg;
                if src != dst {
                    self.emit(Op::Move { dst, src, ty: *ty }, span);
                }
            }

            Expr::TimeNow { ty, span } => {
                self.emit(Op::TimeNow { dst, ty: *ty }, *span);
            }

            Expr::Neg { ty, operand, span } => {
                let mark = self.next;
                let src = self.operand(operand, *span);
                self.emit(Op::Neg { dst, src, ty: *ty }, *span);
                self.next = mark;
            }

            Expr::Compare { op, operands, lhs, rhs, span } => {
                let mark = self.next;
                let a = self.operand(lhs, *span);
                let b = self.operand(rhs, *span);
                self.emit(
                    Op::Compare { op: *op, operands: *operands, dst, lhs: a, rhs: b },
                    *span,
                );
                self.next = mark;
            }

            // The answer is built up in a register of its own and only moved to `dst` at
            // the end. Going straight into `dst` looks cheaper and is wrong: `dst` is
            // often the very variable a later side reads, and the left side's answer
            // would be sitting where that variable used to be. It cost a fuzzer 10,306
            // programs to say so.
            Expr::Logic { op, lhs, rhs, span } => {
                let mark = self.next;
                let held = self.temp();
                self.expr(lhs, held, *span);

                // If the left side settled it, the right is jumped over -- not evaluated
                // and discarded, but never run at all.
                let settled = match op {
                    LogicOp::And => self.emit(Op::JumpIfFalse { cond: held, target: 0 }, *span),
                    LogicOp::Or => self.emit(Op::JumpIfTrue { cond: held, target: 0 }, *span),
                };
                self.expr(rhs, held, *span);
                self.land(settled);

                self.emit(Op::Move { dst, src: held, ty: Ty::Bool }, *span);
                self.next = mark;
            }

            Expr::Call { func, args, span, .. } => {
                let mark = self.next;
                let (base, argc) = self.arguments(args, *span);
                self.emit(Op::Call { func: *func as u32, base, argc, dst }, *span);
                self.next = mark;
            }

            Expr::NewArray { ty, items, span } => {
                let mark = self.next;
                let (base, count) = self.arguments(items, *span);
                self.emit(Op::NewArray { dst, items: base, count, ty: *ty }, *span);
                self.next = mark;
            }

            Expr::Filled { ty, length, value, span } => {
                let mark = self.next;
                let length = self.operand(length, *span);
                let held = self.operand(value, *span);
                self.emit(Op::Filled { dst, length, value: held, ty: *ty }, *span);
                self.next = mark;
            }

            Expr::At { array, at, span, .. } => {
                let mark = self.next;
                let ty = array.ty();
                let target = self.operand(array, *span);
                let (base, rank) = self.arguments(at, *span);
                self.emit(Op::At { dst, array: target, at: base, rank: rank as u8, ty }, *span);
                self.next = mark;
            }

            Expr::Count { array, ty, span } => {
                let mark = self.next;
                let target = self.operand(array, *span);
                self.emit(Op::Count { dst, array: target, ty: *ty }, *span);
                self.next = mark;
            }

            Expr::Not { operand, span } => {
                let mark = self.next;
                let src = self.operand(operand, *span);
                self.emit(Op::Not { dst, src }, *span);
                self.next = mark;
            }

            Expr::Binary { op, ty, lhs, rhs, span, nonnegative } => {
                // Take both sides before writing the answer, since `dst` may well be one
                // of them -- `set ['sum'] = [math { 'sum' + 'i' }]` is the common case.
                let mark = self.next;
                let a = self.operand(lhs, *span);
                let b = self.operand(rhs, *span);
                self.emit(Op::Binary { op: *op, ty: *ty, dst, lhs: a, rhs: b, nonnegative: *nonnegative }, *span);
                self.next = mark;
            }
        }
    }
}
