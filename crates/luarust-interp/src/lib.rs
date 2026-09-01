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
use luarust_core::heap;
use luarust_parse::ast::LogicOp;
use luarust_check::value::{
    DEPTH_LIMIT, Fault, Overflow, Value, binary_op, compare, format_of, holds, int_compare,
    int_op, negate, one_of,
};
pub use luarust_check::value::Stopped;
use luarust_diag::Span;
use luarust_num::binary::{self, Comparison, Round};
use luarust_parse::ast::{BinOp, Ty};
use std::io::Write;
use std::time::Instant;

type Outcome<T> = Result<T, Stopped>;


/// Run a checked program, writing whatever it prints to `out`.
/// The tree-walker does not collect, and cannot as it stands.
///
/// Its intermediate values live in Rust locals up the recursion -- the `held` vector
/// being filled for a new array, an argument evaluated but not yet passed. The collector
/// is handed roots, and it cannot be handed those: they are on the Rust stack where
/// nothing can enumerate them. Sweeping here would free an array the program was in the
/// middle of building.
///
/// The VM has no such trouble, because its intermediates live in registers and registers
/// are roots. That difference is the whole reason a real collector for compiled code
/// needs statepoints: machine registers are the same problem again.
///
/// This costs nothing that matters. The tree-walker is the oracle the other paths are
/// checked against, not something anybody ships, and collecting changes no output -- so
/// the paths agree whether it collects or not.
pub fn run(program: &Checked, out: &mut impl Write) -> Outcome<()> {
    heap::clear();
    // The same answers the chunk would carry, taken from the program directly.
    heap::set_threshold(program.collect.threshold());
    luarust_core::value::set_floats(program.floats);
    luarust_core::value::set_division(program.division);
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
        Flow::Broke => unreachable!("`break` outside a loop was checked for"),
        Flow::Went => Ok(()),
    }
}

/// What happened after a statement: either the next one runs, or the function is over.
enum Flow {
    Went,
    Broke,
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

            // The condition before every pass, and the counter one higher after each.
            Stmt::While { counter, condition, body, span } => {
                if let Some((slot, ty)) = counter {
                    self.slots[*slot] = Some(Value::zero(*ty));
                }
                loop {
                    if !truth(&self.eval(condition, out)?) {
                        return Ok(Flow::Went);
                    }
                    // Counted at the start of the pass, so during the first it is one and
                    // afterwards it holds however many ran -- the same promise a counting
                    // loop makes, which never steps past the last value it took.
                    if let Some((slot, ty)) = counter {
                        let held = self.slots[*slot].clone().expect("the counter was set");
                        let next = binary_op(BinOp::Add, &held, &one_of(*ty), self.overflow)
                            .map_err(|fault| Stopped { fault, span: *span })?;
                        self.slots[*slot] = Some(next);
                    }
                    match self.block(body, out)? {
                        Flow::Went => {}
                        Flow::Broke => return Ok(Flow::Went),
                        returned => return Ok(returned),
                    }
                }
            }

            Stmt::Break { .. } => Ok(Flow::Broke),

            Stmt::StoreAt { array, at, value, span } => {
                let held = self.eval(array, out)?;
                let value = self.eval(value, out)?;
                let ty = array.ty();
                let handle = handle_of(&held);
                let index = self.offset(ty, handle, at, *span, out)?;
                heap::store(handle, index, &value);
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
        if ordering(ty, &current, &to) == Comparison::Greater {
            return Ok(Flow::Went);
        }

        loop {
            self.slots[slot] = Some(current.clone());
            // A `return` inside the body leaves the loop and the function together; a
            // `break` leaves only the loop.
            match self.block(body, out)? {
                Flow::Went => {}
                Flow::Broke => return Ok(Flow::Went),
                returned => return Ok(returned),
            }
            if ordering(ty, &current, &to) != Comparison::Less {
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
                fault: Box::new(Fault {
                    code: "R0011",
                    message: format!("this has called itself {DEPTH_LIMIT} deep."),
                    rule: "a call may only go so deep before the program is stopped",
                    fix: "give the recursion a case that stops, or write it as a loop."
                        .to_string(),
                }),
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
            Flow::Broke => unreachable!("`break` outside a loop was checked for"),
        }
    }

    /// Where an index lands, counting from one and flattening a shape row by row.
    ///
    /// `'m'[|2|, |3|]` in a `2x3` is the second row's third column, which is element five
    /// counting from nought — rows laid end to end, the way the literal writes them.
    fn offset(
        &mut self,
        ty: Ty,
        handle: u32,
        at: &[Expr],
        span: Span,
        out: &mut impl Write,
    ) -> Outcome<usize> {
        let of = ty.array().expect("only an array is indexed");
        let dims = of.dims();
        let mut flat = 0usize;

        for (place, index) in at.iter().enumerate() {
            let held = self.eval(index, out)?.as_i128().unwrap_or(0);
            // Counted from one, so nought is no element and the first is one.
            let size = dims.get(place).copied().unwrap_or(0) as i128;
            let past = if dims.is_empty() {
                heap::length(handle) as i128
            } else {
                size
            };
            if held < 1 || held > past {
                return Err(Stopped {
                    fault: out_of_range(held, past),
                    span,
                });
            }
            flat = flat * dims.get(place).copied().unwrap_or(1) as usize + (held as usize - 1);
        }
        Ok(flat)
    }

    fn eval(&mut self, expr: &Expr, out: &mut impl Write) -> Outcome<Value> {
        match expr {
            Expr::Const(value) => Ok(value.clone()),

            Expr::Load { slot, ty, span } => match &self.slots[*slot] {
                Some(value) => Ok(value.clone()),
                None => Err(Stopped {
                    fault: Box::new(Fault {
                        code: "R0009",
                        message: "this is read before it was ever given a value.".to_string(),
                        rule: "a variable holds a value from the moment it is declared",
                        fix: format!("give it a `{}` value where it is declared.", ty.word()),
                    }),
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
                        fault: Box::new(Fault {
                            code: "R0010",
                            message: "the clock could not be read.".to_string(),
                            rule: "`time.now` is a count of seconds",
                            fix: "report this: it should not be able to happen.".to_string(),
                        }),
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

            // A new array every time this is reached, so two passes of a loop are two
            // arrays rather than one written over twice.
            Expr::NewArray { ty, items, span } => {
                let of = ty.array().expect("a new array has an array type");
                let mut held = Vec::with_capacity(items.len());
                for item in items {
                    held.push(self.eval(item, out)?);
                }
                let _ = span;
                Ok(heap::handle(*ty, heap::of(of.element, &held)))
            }

            Expr::Filled { ty, length, value, span } => {
                let of = ty.array().expect("a filled array has an array type");
                let length = self.eval(length, out)?;
                let value = self.eval(value, out)?;
                let count = length.as_i128().unwrap_or(0);
                if count < 0 {
                    return Err(Stopped {
                        fault: Box::new(Fault::of(
                            "R0014",
                            "this asks for an array of fewer than no elements.",
                            "an array holds none or more",
                            "give it a length of nought or more.",
                        )),
                        span: *span,
                    });
                }
                Ok(heap::handle(*ty, heap::make(of.element, count as usize, &value)))
            }

            Expr::At { array, at, span, .. } => {
                let ty = array.ty();
                let held = self.eval(array, out)?;
                let handle = handle_of(&held);
                let index = self.offset(ty, handle, at, *span, out)?;
                heap::read(handle, index).ok_or_else(|| Stopped {
                    fault: out_of_range(index as i128 + 1, heap::length(handle) as i128),
                    span: *span,
                })
            }

            Expr::Count { array, ty, .. } => {
                let held = self.eval(array, out)?;
                let count = heap::length(handle_of(&held)) as u64;
                Ok(Value::Num { ty: *ty, bits: count })
            }

            Expr::Binary { op, ty, lhs, rhs, span, .. } => {
                let lhs = self.eval(lhs, out)?;
                let rhs = self.eval(rhs, out)?;
                // The checker already said what these are, so the integers go straight to
                // the arithmetic rather than through a dispatch that works it out again.
                if ty.is_integer()
                    && let (Value::Num { bits: a, .. }, Value::Num { bits: b, .. }) = (&lhs, &rhs)
                {
                    let bits = int_op(*op, *ty, *a, *b, self.overflow)
                        .map_err(|fault| Stopped { fault, span: *span })?;
                    return Ok(Value::Num { ty: *ty, bits });
                }
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

/// How two values of a known type order.
///
/// The same answer [`compare`] gives, reached without asking the values what they are:
/// the loop that calls this already knows, and it calls it twice a pass.
fn ordering(ty: Ty, a: &Value, b: &Value) -> Comparison {
    if ty.is_integer()
        && let (Value::Num { bits: x, .. }, Value::Num { bits: y, .. }) = (a, b)
    {
        return int_compare(ty, *x, *y);
    }
    compare(a, b)
}

/// The array a value points at.
fn handle_of(value: &Value) -> u32 {
    match value {
        Value::Num { bits, .. } => *bits as u32,
        other => unreachable!("an array value is a handle, and this is {other:?}"),
    }
}

/// Reaching for an element that is not there.
fn out_of_range(at: i128, length: i128) -> Box<Fault> {
    Box::new(Fault::of(
        "R0015",
        format!("there is no element {at} here."),
        "an array is counted from one, up to how many it holds",
        if length == 0 {
            "this one holds nothing at all.".to_string()
        } else {
            format!("this one holds {length}, so the last is {length} and the first is 1.")
        },
    ))
}
