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
use luarust_parse::ast::LogicOp;
use luarust_check::value::{
    DEPTH_LIMIT, Fault, Overflow, Value, binary_op, compare, format_of, holds, negate, one_of,
};
pub use luarust_check::value::Stopped;
use luarust_diag::Span;
use luarust_num::binary::{self, Comparison, Round};
use luarust_parse::ast::{BinOp, Ty};
use std::io::Write;
use std::time::Instant;

type Outcome<T> = Result<T, Stopped>;


/// Run a checked program, writing whatever it prints to `out`.
pub fn run(program: &Checked, out: &mut impl Write) -> Outcome<()> {
    let mut machine = Machine {
        slots: vec![None; program.slots],
        overflow: program.overflow,
        started: Instant::now(),
        program,
        depth: 0,
    };
    match machine.block(&program.stmts, out)? {
        // Nothing at the top level can return: the checker refuses a `return` outside a
        // function, so there is nothing here for one to mean.
        Flow::Returned(_) => unreachable!("`return` outside a function was checked for"),
        Flow::Went => Ok(()),
    }
}

/// What happened after a statement: either the next one runs, or the function is over.
enum Flow {
    Went,
    Returned(Option<Value>),
}

struct Machine<'a> {
    slots: Vec<Option<Value>>,
    overflow: Overflow,
    started: Instant,
    program: &'a Checked,
    depth: usize,
}

impl Machine<'_> {
    fn block(&mut self, stmts: &[Stmt], out: &mut impl Write) -> Outcome<Flow> {
        for stmt in stmts {
            match self.stmt(stmt, out)? {
                Flow::Went => {}
                returned => return Ok(returned),
            }
        }
        Ok(Flow::Went)
    }

    fn stmt(&mut self, stmt: &Stmt, out: &mut impl Write) -> Outcome<Flow> {
        match stmt {
            Stmt::Store { slot, value, .. } => {
                let value = self.eval(value, out)?;
                self.slots[*slot] = Some(value);
                Ok(Flow::Went)
            }

            Stmt::Print { items, .. } => {
                for item in items {
                    match item {
                        Item::Text(text) => {
                            let _ = out.write_all(text.as_bytes());
                        }
                        Item::Value(expr) => {
                            let value = self.eval(expr, out)?;
                            let _ = out.write_all(value.to_string().as_bytes());
                        }
                    }
                }
                let _ = out.flush();
                Ok(Flow::Went)
            }

            Stmt::Loop { slot, ty, from, to, body, span } => {
                let from = self.eval(from, out)?;
                let to = self.eval(to, out)?;
                self.count(*slot, *ty, from, to, body, *span, out)
            }

            // The first arm whose condition holds, and only that one. A condition after
            // it is never asked, which is what makes an earlier arm able to guard a later.
            Stmt::If { arms, otherwise, .. } => {
                for arm in arms {
                    if truth(&self.eval(&arm.condition, out)?) {
                        return self.block(&arm.body, out);
                    }
                }
                self.block(otherwise, out)
            }

            // Called for what it does; whatever it answers is dropped here.
            Stmt::Call { func, args, span } => {
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    values.push(self.eval(arg, out)?);
                }
                self.call(*func, values, *span, out)?;
                Ok(Flow::Went)
            }

            Stmt::Return { value, .. } => {
                let value = match value {
                    Some(expr) => Some(self.eval(expr, out)?),
                    None => None,
                };
                Ok(Flow::Returned(value))
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
    ) -> Outcome<Flow> {
        let one = one_of(ty);
        let mut current = from;

        // Counting down is an empty range rather than a reversed one, so nothing runs.
        if compare(&current, &to) == Comparison::Greater {
            return Ok(Flow::Went);
        }

        loop {
            self.slots[slot] = Some(current.clone());
            // A `return` inside the body leaves the loop and the function together.
            if let Flow::Returned(value) = self.block(body, out)? {
                return Ok(Flow::Returned(value));
            }
            if compare(&current, &to) != Comparison::Less {
                return Ok(Flow::Went);
            }
            current = binary_op(BinOp::Add, &current, &one, Overflow::Wrap)
                .map_err(|fault| Stopped { fault, span })?;
        }
    }

    /// Run one function and give back what it answered.
    fn call(
        &mut self,
        func: usize,
        args: Vec<Value>,
        span: Span,
        out: &mut impl Write,
    ) -> Outcome<Option<Value>> {
        if self.depth >= DEPTH_LIMIT {
            return Err(Stopped {
                fault: Fault {
                    code: "R0011",
                    message: format!("this has called itself {DEPTH_LIMIT} deep."),
                    rule: "a call may only go so deep before the program is stopped",
                    fix: "give the recursion a case that stops, or write it as a loop."
                        .to_string(),
                },
                span,
            });
        }

        let function = &self.program.funcs[func];
        let mut slots = vec![None; function.slots];
        for (slot, value) in args.into_iter().enumerate() {
            slots[slot] = Some(value);
        }

        // The caller's slots are set aside rather than shared: a function sees its own
        // parameters and whatever it declares, and nothing of whoever called it.
        let outer = std::mem::replace(&mut self.slots, slots);
        self.depth += 1;
        let body = &self.program.funcs[func].body;
        // Printing from inside a call goes wherever the call was written, which for the
        // reference interpreter means the same stream throughout.
        let flow = self.block(body, out);
        self.depth -= 1;
        self.slots = outer;

        match flow? {
            Flow::Returned(value) => Ok(value),
            // A function that answers nothing reaching its end is simply over.
            Flow::Went => Ok(None),
        }
    }

    fn eval(&mut self, expr: &Expr, out: &mut impl Write) -> Outcome<Value> {
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
                Ok(Value::float(*ty, bits))
            }

            Expr::Neg { operand, span, .. } => {
                let value = self.eval(operand, out)?;
                negate(&value, self.overflow).map_err(|fault| Stopped { fault, span: *span })
            }

            // A NaN is not less than, greater than, or equal to anything, itself
            // included -- so all three answer false, which is what unordered means.
            Expr::Compare { op, lhs, rhs, .. } => {
                let lhs = self.eval(lhs, out)?;
                let rhs = self.eval(rhs, out)?;
                Ok(Value::Bool(holds(*op, compare(&lhs, &rhs))))
            }

            // The right side is only worked out when the left did not settle it. That is
            // not an optimisation: it is what lets `'d' != 0 and 'n' div 'd' > 1` be
            // asked at all, since the second half is a fault when the first is false.
            Expr::Logic { op, lhs, rhs, .. } => {
                let lhs = truth(&self.eval(lhs, out)?);
                let settled = match op {
                    LogicOp::And => !lhs,
                    LogicOp::Or => lhs,
                };
                if settled {
                    return Ok(Value::Bool(lhs));
                }
                Ok(Value::Bool(truth(&self.eval(rhs, out)?)))
            }

            Expr::Not { operand, .. } => Ok(Value::Bool(!truth(&self.eval(operand, out)?))),

            // Its own slots, its own scope, and the arguments already in the first of
            // them -- the checker gave the parameters the lowest slot numbers for exactly
            // this reason.
            Expr::Call { func, args, span, .. } => {
                let mut values = Vec::with_capacity(args.len());
                for arg in args {
                    values.push(self.eval(arg, out)?);
                }
                Ok(self.call(*func, values, *span, out)?.expect("it answers a value"))
            }

            Expr::Binary { op, lhs, rhs, span, .. } => {
                let lhs = self.eval(lhs, out)?;
                let rhs = self.eval(rhs, out)?;
                binary_op(*op, &lhs, &rhs, self.overflow)
                    .map_err(|fault| Stopped { fault, span: *span })
            }
        }
    }
}

/// What a condition answered. The checker has already refused anything that is not a
/// `bool`, so there is nothing here to decide.
fn truth(value: &Value) -> bool {
    match value {
        Value::Bool(answer) => *answer,
        other => unreachable!("a condition checked as `bool` held {other:?}"),
    }
}
