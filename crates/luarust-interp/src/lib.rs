//! Running a checked program directly.
//!
//! This walks the checked tree and does what it says, with no bytecode, no jumps and no
//! compilation of any kind. It is meant to be slow and obviously correct, because its
//! real job is not speed: when the bytecode VM and then the JIT arrive, this is what they
//! will be checked against. A reference that is simple enough to be read and agreed with
//! is worth more than a fast one.
//!
//! Every number it computes comes from `luarust-num`, so the answers here are the same
//! answers by construction rather than by two implementations being written carefully.

use luarust_check::ir::{Checked, Expr, Item, Stmt};
use luarust_check::value::{Fault, Overflow, Value, binary_op, format_of, negate};
use luarust_diag::{Diagnostic, Span};
use luarust_num::binary::{self, Comparison, Round};
use luarust_parse::ast::{BinOp, Ty};
use std::io::Write;
use std::time::Instant;

/// A fault, and where in the program it happened.
#[derive(Clone, Debug)]
pub struct Stopped {
    pub fault: Fault,
    pub span: Span,
}

impl Stopped {
    /// The same shape as every other Luarust error, so a running program's complaints
    /// read exactly like a compiler's.
    pub fn diagnostic(&self) -> Diagnostic {
        Diagnostic::new(self.fault.code, self.fault.message.clone())
            .primary(self.span, "while running this")
            .rule(self.fault.rule)
            .fix(self.fault.fix.clone())
    }
}

type Outcome<T> = Result<T, Stopped>;

/// Run a checked program, writing whatever it prints to `out`.
pub fn run(program: &Checked, out: &mut impl Write) -> Outcome<()> {
    let mut machine = Machine {
        slots: vec![None; program.slots],
        overflow: program.overflow,
        started: Instant::now(),
    };
    machine.block(&program.stmts, out)
}

struct Machine {
    slots: Vec<Option<Value>>,
    overflow: Overflow,
    started: Instant,
}

impl Machine {
    fn block(&mut self, stmts: &[Stmt], out: &mut impl Write) -> Outcome<()> {
        for stmt in stmts {
            self.stmt(stmt, out)?;
        }
        Ok(())
    }

    fn stmt(&mut self, stmt: &Stmt, out: &mut impl Write) -> Outcome<()> {
        match stmt {
            Stmt::Store { slot, value, .. } => {
                let value = self.eval(value)?;
                self.slots[*slot] = Some(value);
                Ok(())
            }

            Stmt::Print { items, .. } => {
                for item in items {
                    match item {
                        Item::Text(text) => {
                            let _ = out.write_all(text.as_bytes());
                        }
                        Item::Value(expr) => {
                            let value = self.eval(expr)?;
                            let _ = out.write_all(value.to_string().as_bytes());
                        }
                    }
                }
                let _ = out.flush();
                Ok(())
            }

            Stmt::Loop { slot, ty, from, to, body, span } => {
                let from = self.eval(from)?;
                let to = self.eval(to)?;
                self.count(*slot, *ty, from, to, body, *span, out)
            }
        }
    }

    /// Count from one bound to the other, inclusive, running the body once per value.
    ///
    /// The step happens only after the body has run and only while the counter is below
    /// the far bound, so a loop can reach the very top of its type without the increment
    /// that would take it past.
    #[allow(clippy::too_many_arguments)]
    fn count(
        &mut self,
        slot: usize,
        ty: Ty,
        from: Value,
        to: Value,
        body: &[Stmt],
        span: Span,
        out: &mut impl Write,
    ) -> Outcome<()> {
        let one = one_of(ty);
        let mut current = from;

        // Counting down is an empty range rather than a reversed one, so nothing runs.
        if compare(&current, &to) == Comparison::Greater {
            return Ok(());
        }

        loop {
            self.slots[slot] = Some(current.clone());
            self.block(body, out)?;
            if compare(&current, &to) != Comparison::Less {
                return Ok(());
            }
            current = binary_op(BinOp::Add, &current, &one, Overflow::Wrap)
                .map_err(|fault| Stopped { fault, span })?;
        }
    }

    fn eval(&mut self, expr: &Expr) -> Outcome<Value> {
        match expr {
            Expr::Const(value) => Ok(value.clone()),

            Expr::Load { slot, ty, span } => match &self.slots[*slot] {
                Some(value) => Ok(value.clone()),
                None => Err(Stopped {
                    fault: Fault {
                        code: "R0009",
                        message: "this is read before it was ever given a value.".to_string(),
                        rule: "a variable holds a value from the moment it is declared",
                        fix: format!("give it a `{}` value where it is declared.", ty.word()),
                    },
                    span: *span,
                }),
            },

            // Seconds since the program began. Monotonic, so it only ever moves forward.
            Expr::TimeNow { ty, span } => {
                let seconds = self.started.elapsed().as_secs_f64();
                let fmt = format_of(*ty).expect("the clock is read as a float");
                let text = format!("{seconds:.9}");
                let bits = binary::from_decimal::<8>(fmt, Round::TiesToEven, &text).map_err(|_| {
                    Stopped {
                        fault: Fault {
                            code: "R0010",
                            message: "the clock could not be read.".to_string(),
                            rule: "`time.now` is a count of seconds",
                            fix: "report this: it should not be able to happen.".to_string(),
                        },
                        span: *span,
                    }
                })?;
                Ok(Value::Float { ty: *ty, bits })
            }

            Expr::Neg { operand, span, .. } => {
                let value = self.eval(operand)?;
                negate(&value, self.overflow).map_err(|fault| Stopped { fault, span: *span })
            }

            Expr::Binary { op, lhs, rhs, span, .. } => {
                let lhs = self.eval(lhs)?;
                let rhs = self.eval(rhs)?;
                binary_op(*op, &lhs, &rhs, self.overflow)
                    .map_err(|fault| Stopped { fault, span: *span })
            }
        }
    }
}

/// One, of whichever numeric type.
fn one_of(ty: Ty) -> Value {
    if ty.is_integer() {
        Value::int(ty, 1)
    } else {
        let fmt = format_of(ty).expect("a loop counts in a number");
        Value::Float { ty, bits: binary::arith::one::<8>(fmt, false) }
    }
}

fn compare(a: &Value, b: &Value) -> Comparison {
    match (a, b) {
        (Value::Int { .. }, Value::Int { .. }) => {
            match a.as_i128().unwrap().cmp(&b.as_i128().unwrap()) {
                std::cmp::Ordering::Less => Comparison::Less,
                std::cmp::Ordering::Equal => Comparison::Equal,
                std::cmp::Ordering::Greater => Comparison::Greater,
            }
        }
        (Value::Float { ty, bits: x }, Value::Float { bits: y, .. }) => {
            let fmt = format_of(*ty).expect("a float type has a format");
            binary::compare(fmt, *x, *y)
        }
        _ => Comparison::Unordered,
    }
}
