//! Turning a checked program into bytecode.
//!
//! The checker has already numbered every variable, so those numbers are the low
//! registers and the compiler only has to find room for the values it makes along the way.
//! Temporaries are handed out above the variables and given back at the end of each
//! statement, since nothing a statement computes outlives it — except a loop's far bound,
//! which has to survive its body and is parked below the body's temporaries for exactly
//! that reason.

use crate::chunk::{Chunk, Op, Reg};
use luarust_check::ir::{Checked, Expr, Item, Stmt};
use luarust_check::value::{Value, one_of};
use luarust_diag::Span;
use luarust_parse::ast::BinOp;

/// Compile a checked program.
pub fn compile(program: &Checked) -> Chunk {
    let mut compiler = Compiler {
        chunk: Chunk {
            code: Vec::new(),
            spans: Vec::new(),
            consts: Vec::new(),
            texts: Vec::new(),
            registers: program.slots,
            overflow: program.overflow,
        },
        floor: program.slots as Reg,
        next: program.slots as Reg,
    };
    compiler.block(&program.stmts);
    compiler.emit(Op::Halt, Span::default());
    compiler.chunk
}

struct Compiler {
    chunk: Chunk,
    /// Where temporaries start. Raised while compiling a loop body, so that the body
    /// cannot reuse the registers the loop itself is still holding.
    floor: Reg,
    /// The next free temporary.
    next: Reg,
}

impl Compiler {
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
            | Op::JumpIfEqual { target: t, .. } => *t = target,
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
        if let Some(found) = self.chunk.consts.iter().position(|existing| *existing == value) {
            return found as u32;
        }
        self.chunk.consts.push(value);
        (self.chunk.consts.len() - 1) as u32
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
                            let reg = self.operand(expr, *span);
                            self.emit(Op::PrintValue { src: reg }, *span);
                            self.release();
                        }
                    }
                }
            }

            Stmt::Loop { slot, ty, from, to, body, span } => {
                let counter = *slot as Reg;
                self.expr(from, counter, *span);

                // The far bound and the step have to outlive the body, so they are taken
                // before the floor is raised and the body starts handing out its own.
                let limit = self.temp();
                self.expr(to, limit, *span);
                let step = self.temp();
                let one = self.konst(one_of(*ty));
                self.emit(Op::Const { dst: step, konst: one }, *span);

                let outer_floor = self.floor;
                self.floor = self.next;

                // Counting down is an empty range, so leave before running anything.
                let skip = self.emit(
                    Op::JumpIfGreater { lhs: counter, rhs: limit, target: 0 },
                    *span,
                );

                let top = self.here();
                self.block(body);

                // Step only while the counter is below the bound, so a loop can reach the
                // top of its type without the increment that would take it past.
                let done = self.emit(
                    Op::JumpIfEqual { lhs: counter, rhs: limit, target: 0 },
                    *span,
                );
                self.emit(
                    Op::Binary { op: BinOp::Add, dst: counter, lhs: counter, rhs: step },
                    *span,
                );
                self.emit(Op::Jump { target: top }, *span);

                self.land(skip);
                self.land(done);

                self.floor = outer_floor;
                self.release();
            }
        }
    }

    /// A register holding this expression's value, without copying when there is already
    /// one — a variable is read where it lives.
    fn operand(&mut self, expr: &Expr, span: Span) -> Reg {
        if let Expr::Load { slot, .. } = expr {
            return *slot as Reg;
        }
        let reg = self.temp();
        self.expr(expr, reg, span);
        reg
    }

    /// Emit whatever puts this expression's value in `dst`.
    fn expr(&mut self, expr: &Expr, dst: Reg, span: Span) {
        match expr {
            Expr::Const(value) => {
                let index = self.konst(value.clone());
                self.emit(Op::Const { dst, konst: index }, span);
            }

            Expr::Load { slot, .. } => {
                let src = *slot as Reg;
                if src != dst {
                    self.emit(Op::Move { dst, src }, span);
                }
            }

            Expr::TimeNow { ty, span } => {
                self.emit(Op::TimeNow { dst, ty: *ty }, *span);
            }

            Expr::Neg { operand, span, .. } => {
                let src = self.operand(operand, *span);
                self.emit(Op::Neg { dst, src }, *span);
            }

            Expr::Binary { op, lhs, rhs, span, .. } => {
                // Take both sides before writing the answer, since `dst` may well be one
                // of them -- `set ['sum'] = [math { 'sum' + 'i' }]` is the common case.
                let mark = self.next;
                let a = self.operand(lhs, *span);
                let b = self.operand(rhs, *span);
                self.emit(Op::Binary { op: *op, dst, lhs: a, rhs: b }, *span);
                self.next = mark;
            }
        }
    }
}
