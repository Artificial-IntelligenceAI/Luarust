//! Where a whole number can be, worked out before anything runs.
//!
//! This pass walks the checked program once and keeps, for every slot, the interval its
//! value must lie in. It exists to answer one question: at this `div` or `mod`, is the
//! dividend provably at or above zero and the divisor provably above it? Where the
//! answer is yes, floored and truncated division are the same operation, nothing about
//! the divisor needs guarding, and a compiler may say so — that is the `nonnegative`
//! flag on [`Expr::Binary`], and this file is the only place that ever sets it.
//!
//! Everything here over-approximates. An interval may be wider than the values a run
//! can produce, never narrower, so a flag that is set is a proof and a flag that is not
//! set is only a shrug. The interesting cases:
//!
//! - Arithmetic is done in `i128`, where no 64-bit operation can overflow, and then
//!   held against the type's own range. A result that fits is exact. One that may not
//!   fit depends on what overflow means here: under `wrap` the value can be anything
//!   the type can hold, so the interval widens to the type's whole range; under `trap`
//!   the program stops instead of wrapping, so the executions that continue are exactly
//!   the ones inside the range, and the interval is clipped to it.
//! - A floored remainder takes the sign of its divisor: against a divisor in `[1, d]`
//!   it lands in `[0, d-1]` whatever the dividend was.
//! - A counting loop declares its counter outright: `[from, to]` needs no discovery.
//!   The rest of the body is run to a fixed point — around again until nothing widens,
//!   giving up after a few passes by forgetting whatever would not settle.
//! - `break` leaves a loop mid-body, so the state at every `break` is folded into the
//!   state after the loop, not just the state at the body's end.
//! - A `while` body may run any number of times on conditions this pass does not
//!   read, so every slot it stores is forgotten before the body is believed.
//!
//! Flags are only committed on a final pass over each loop body, made with the settled
//! intervals — the passes that were still searching for the fixed point prove nothing.

use crate::ir::{Checked, Expr, Item, Stmt};
use crate::value::Overflow;
use luarust_parse::ast::{BinOp, Ty};

/// A closed interval. Kept in `i128` so every 64-bit value, signed or not, fits with
/// room to add and multiply without wrapping the analysis itself.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Range {
    lo: i128,
    hi: i128,
}

impl Range {
    fn hull(a: Range, b: Range) -> Range {
        Range { lo: a.lo.min(b.lo), hi: a.hi.max(b.hi) }
    }
}

/// The whole range of an integer type, and `None` for everything else.
fn ty_range(ty: Ty) -> Option<Range> {
    let bits = ty.int_bits()?;
    Some(if ty.is_signed() {
        Range { lo: -(1i128 << (bits - 1)), hi: (1i128 << (bits - 1)) - 1 }
    } else {
        Range { lo: 0, hi: (1i128 << bits) - 1 }
    })
}

/// What is known per slot. `None` is not "empty" but "anything": a slot this pass has
/// lost track of, or one holding something that is not an integer at all.
type State = Vec<Option<Range>>;

fn hull_states(a: &State, b: &State) -> State {
    a.iter()
        .zip(b)
        .map(|(x, y)| match (x, y) {
            (Some(x), Some(y)) => Some(Range::hull(*x, *y)),
            _ => None,
        })
        .collect()
}

/// Work out every flag in the program. Called once, after checking found no faults —
/// the walk trusts the types the checker settled.
pub(crate) fn flag(checked: &mut Checked) {
    let overflow = checked.overflow;
    // A function's parameters can hold anything their types can, and nothing a call
    // does can reach the caller's slots, so every body stands alone.
    for func in &mut checked.funcs {
        let mut state: State = vec![None; func.slots];
        for (n, ty) in func.params.iter().enumerate() {
            state[n] = ty_range(*ty);
        }
        let mut walker = Walker { overflow, breaks: Vec::new() };
        walker.block(&mut func.body, &mut state, true);
    }
    let mut state: State = vec![None; checked.slots];
    let mut walker = Walker { overflow, breaks: Vec::new() };
    walker.block(&mut checked.stmts, &mut state, true);
}

struct Walker {
    overflow: Overflow,
    /// One entry per loop currently open: the hull of the states at its `break`s.
    breaks: Vec<Option<State>>,
}

impl Walker {
    fn block(&mut self, stmts: &mut [Stmt], state: &mut State, commit: bool) {
        for stmt in stmts {
            match stmt {
                Stmt::Store { slot, value, .. } => {
                    let range = self.eval(value, state, commit);
                    state[*slot] = range;
                }

                Stmt::Print { items, .. } => {
                    for item in items {
                        if let Item::Value(expr) = item {
                            self.eval(expr, state, commit);
                        }
                    }
                }

                Stmt::Loop { slot, ty, from, to, body, .. } => {
                    let from_range = self.eval(from, state, commit);
                    let to_range = self.eval(to, state, commit);
                    // Inside the body the counter is somewhere between the two bounds.
                    // When either bound is unknown, the counter still cannot leave its
                    // own type.
                    let counter = match (from_range, to_range) {
                        (Some(f), Some(t)) => Some(Range { lo: f.lo, hi: t.hi }),
                        _ => ty_range(*ty),
                    };
                    self.fixed_point(body, state, commit, *slot, counter);
                    // What the counter holds after the loop is the engines' business,
                    // not this pass's.
                    state[*slot] = None;
                }

                Stmt::While { counter, body, condition, .. } => {
                    // The body may run any number of times, on a condition this pass
                    // does not read, so nothing it stores can be believed on entry.
                    let mut stored = Vec::new();
                    stored_slots(body, &mut stored);
                    for slot in &stored {
                        state[*slot] = None;
                    }
                    if let Some((slot, _)) = counter {
                        state[*slot] = None;
                    }
                    self.eval(condition, state, commit);
                    let seed_slot = usize::MAX;
                    self.fixed_point(body, state, commit, seed_slot, None);
                }

                Stmt::If { arms, otherwise, .. } => {
                    // Every arm starts from the same state, since this pass does not
                    // read conditions, and what comes after sees the hull of every way
                    // through — the fall-past-everything way included, when there is
                    // no `else` to close it.
                    let entry = state.clone();
                    let mut exit: Option<State> = None;
                    for arm in arms.iter_mut() {
                        self.eval(&mut arm.condition, state, commit);
                        let mut branch = entry.clone();
                        self.block(&mut arm.body, &mut branch, commit);
                        exit = Some(match exit {
                            None => branch,
                            Some(seen) => hull_states(&seen, &branch),
                        });
                    }
                    let mut branch = entry.clone();
                    self.block(otherwise, &mut branch, commit);
                    let joined = match exit {
                        None => branch,
                        Some(seen) => hull_states(&seen, &branch),
                    };
                    *state = joined;
                }

                Stmt::StoreAt { array, at, value, .. } => {
                    self.eval(array, state, commit);
                    for index in at {
                        self.eval(index, state, commit);
                    }
                    self.eval(value, state, commit);
                }

                Stmt::Return { value, .. } => {
                    if let Some(expr) = value {
                        self.eval(expr, state, commit);
                    }
                    // Whatever follows in this block never runs; walking it anyway
                    // only widens, and a flag set in dead code is never asked.
                }

                // A call moves no slot: a function has its own, and the checker refuses
                // to resolve any name across a function boundary — `var.global` parses
                // and then cannot be reached from inside one (E0208). The day the
                // language grows closures, this arm grows a kill.
                Stmt::Call { args, .. } => {
                    for arg in args {
                        self.eval(arg, state, commit);
                    }
                }

                Stmt::Break { .. } => {
                    // The state here leaves the loop directly, bypassing the rest of
                    // the body, so the loop's exit must include it.
                    if let Some(collector) = self.breaks.last_mut() {
                        *collector = Some(match collector.take() {
                            None => state.clone(),
                            Some(seen) => hull_states(&seen, state),
                        });
                    }
                }
            }
        }
    }

    /// Run a loop body around until its state stops widening, then once more to
    /// commit. `seed` pins the counter slot at the top of every pass; `usize::MAX`
    /// means there is no counter to pin.
    fn fixed_point(
        &mut self,
        body: &mut [Stmt],
        state: &mut State,
        commit: bool,
        seed_slot: usize,
        seed: Option<Range>,
    ) {
        let entry = state.clone();
        let mut settled = entry.clone();
        let mut round = 0;
        loop {
            let mut pass = settled.clone();
            if seed_slot != usize::MAX {
                pass[seed_slot] = seed;
            }
            self.breaks.push(None);
            self.block(body, &mut pass, false);
            self.breaks.pop();
            let merged = hull_states(&settled, &pass);
            if merged == settled {
                break;
            }
            settled = if round < 3 {
                merged
            } else if round == 3 {
                // Still growing: stop chasing and jump each unsettled slot to the top.
                // A slot that never went below zero is widened to "at or above zero,
                // any size", which keeps the one bit this pass exists to prove; one
                // that did is forgotten. The next passes verify the guess rather than
                // trust it — a slot that will not hold still even there is forgotten
                // too, and a forgotten slot stays forgotten, so this ends.
                settled
                    .iter()
                    .zip(&merged)
                    .map(|(a, b)| match (a == b, b) {
                        (true, _) => *a,
                        (false, Some(range)) if range.lo >= 0 => {
                            Some(Range { lo: 0, hi: (1i128 << 64) - 1 })
                        }
                        (false, _) => None,
                    })
                    .collect()
            } else {
                settled
                    .iter()
                    .zip(&merged)
                    .map(|(a, b)| if a == b { *a } else { None })
                    .collect()
            };
            round += 1;
        }

        let mut pass = settled.clone();
        if seed_slot != usize::MAX {
            pass[seed_slot] = seed;
        }
        self.breaks.push(None);
        self.block(body, &mut pass, commit);
        let broke = self.breaks.pop().expect("this loop pushed one");

        // The loop may have run no passes at all, ended at the body's end, or left
        // from any of its `break`s.
        *state = hull_states(&entry, &pass);
        if let Some(broke) = broke {
            *state = hull_states(state, &broke);
        }
    }

    fn eval(&mut self, expr: &mut Expr, state: &mut State, commit: bool) -> Option<Range> {
        match expr {
            Expr::Const(value) => value.as_i128().map(|exact| Range { lo: exact, hi: exact }),

            Expr::Load { slot, ty, .. } => state[*slot].or_else(|| ty_range(*ty)),

            Expr::TimeNow { .. } => None,

            Expr::Binary { op, ty, lhs, rhs, nonnegative, .. } => {
                let left = self.eval(lhs, state, commit);
                let right = self.eval(rhs, state, commit);
                if !ty.is_integer() {
                    return None;
                }
                let (Some(l), Some(r)) = (left, right) else {
                    return ty_range(*ty);
                };
                if commit
                    && matches!(op, BinOp::Div | BinOp::Mod)
                    && ty.is_signed()
                    && l.lo >= 0
                    && r.lo >= 1
                {
                    *nonnegative = true;
                }
                let exact = match op {
                    BinOp::Add => Some(Range { lo: l.lo + r.lo, hi: l.hi + r.hi }),
                    BinOp::Sub => Some(Range { lo: l.lo - r.hi, hi: l.hi - r.lo }),
                    BinOp::Mul => mul(l, r),
                    // Quotient and remainder against a divisor that is certainly
                    // positive; anything less certain falls back to the type.
                    BinOp::Div if l.lo >= 0 && r.lo >= 1 => {
                        Some(Range { lo: l.lo / r.hi, hi: l.hi / r.lo })
                    }
                    BinOp::Mod if r.lo >= 1 => Some(Range { lo: 0, hi: r.hi - 1 }),
                    _ => None,
                };
                held(exact, *ty, self.overflow)
            }

            Expr::Neg { ty, operand, .. } => {
                let inner = self.eval(operand, state, commit);
                if !ty.is_integer() {
                    return None;
                }
                let Some(r) = inner else { return ty_range(*ty) };
                held(Some(Range { lo: -r.hi, hi: -r.lo }), *ty, self.overflow)
            }

            Expr::Compare { lhs, rhs, .. } | Expr::Logic { lhs, rhs, .. } => {
                self.eval(lhs, state, commit);
                self.eval(rhs, state, commit);
                None
            }

            Expr::Not { operand, .. } => {
                self.eval(operand, state, commit);
                None
            }

            Expr::Call { ty, args, .. } => {
                for arg in args {
                    self.eval(arg, state, commit);
                }
                ty_range(*ty)
            }

            Expr::NewArray { items, .. } => {
                for item in items {
                    self.eval(item, state, commit);
                }
                None
            }

            Expr::Filled { length, value, .. } => {
                self.eval(length, state, commit);
                self.eval(value, state, commit);
                None
            }

            Expr::At { array, at, ty, .. } => {
                self.eval(array, state, commit);
                for index in at {
                    self.eval(index, state, commit);
                }
                ty_range(*ty)
            }

            // An array holds none or more, so a count starts at zero however little
            // else is known about it.
            Expr::Count { array, ty, .. } => {
                self.eval(array, state, commit);
                let whole = ty_range(*ty)?;
                Some(Range { lo: 0, hi: whole.hi })
            }
        }
    }
}

/// A product interval. Saturating, because two unsigned 64-bit ends can overrun even
/// `i128` between them — and a saturated corner is still a sound bound, which [`held`]
/// then clips or widens by the type as usual.
fn mul(l: Range, r: Range) -> Option<Range> {
    let corners = [
        l.lo.saturating_mul(r.lo),
        l.lo.saturating_mul(r.hi),
        l.hi.saturating_mul(r.lo),
        l.hi.saturating_mul(r.hi),
    ];
    Some(Range {
        lo: *corners.iter().min().expect("four corners"),
        hi: *corners.iter().max().expect("four corners"),
    })
}

/// What an arithmetic answer is known to be once the type has had its say.
///
/// Within the type it is exact. Outside it, wrapping admits anything the type can
/// hold, and trapping stops the program instead — so the runs that continue hold
/// exactly the part that fit, and an interval entirely outside the type belongs to
/// runs that never continue at all.
fn held(exact: Option<Range>, ty: Ty, overflow: Overflow) -> Option<Range> {
    let whole = ty_range(ty)?;
    let Some(exact) = exact else { return Some(whole) };
    if exact.lo >= whole.lo && exact.hi <= whole.hi {
        return Some(exact);
    }
    match overflow {
        Overflow::Wrap => Some(whole),
        Overflow::Trap => Some(Range {
            lo: exact.lo.max(whole.lo),
            hi: exact.hi.min(whole.hi),
        })
        .filter(|clipped| clipped.lo <= clipped.hi)
        .or(Some(whole)),
    }
}

/// Every slot a `Store` in these statements can touch, loops and arms included.
fn stored_slots(stmts: &[Stmt], out: &mut Vec<usize>) {
    for stmt in stmts {
        match stmt {
            Stmt::Store { slot, .. } => out.push(*slot),
            Stmt::Loop { slot, body, .. } => {
                out.push(*slot);
                stored_slots(body, out);
            }
            Stmt::While { counter, body, .. } => {
                if let Some((slot, _)) = counter {
                    out.push(*slot);
                }
                stored_slots(body, out);
            }
            Stmt::If { arms, otherwise, .. } => {
                for arm in arms {
                    stored_slots(&arm.body, out);
                }
                stored_slots(otherwise, out);
            }
            Stmt::Print { .. }
            | Stmt::StoreAt { .. }
            | Stmt::Return { .. }
            | Stmt::Call { .. }
            | Stmt::Break { .. } => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::Arm;

    fn checked(source: &str) -> Checked {
        let lexed = luarust_lex::lex(source);
        assert!(lexed.ok(), "lexing failed: {:#?}", lexed.errors);
        let parsed = luarust_parse::parse(source, &lexed.tokens);
        assert!(parsed.ok(), "parsing failed: {:#?}", parsed.errors);
        let (checked, errors) = crate::check(&parsed.program);
        assert!(errors.is_empty(), "expected no errors, got {errors:#?}");
        checked
    }

    /// The flag on every `mod` and integer `div` in the program, in source order.
    fn flags(source: &str) -> Vec<bool> {
        let checked = checked(source);
        let mut out = Vec::new();
        walk_stmts(&checked.stmts, &mut out);
        for func in &checked.funcs {
            walk_stmts(&func.body, &mut out);
        }
        out
    }

    fn walk_stmts(stmts: &[Stmt], out: &mut Vec<bool>) {
        for stmt in stmts {
            match stmt {
                Stmt::Store { value, .. } => walk_expr(value, out),
                Stmt::Print { items, .. } => {
                    for item in items {
                        if let Item::Value(expr) = item {
                            walk_expr(expr, out);
                        }
                    }
                }
                Stmt::Loop { from, to, body, .. } => {
                    walk_expr(from, out);
                    walk_expr(to, out);
                    walk_stmts(body, out);
                }
                Stmt::While { condition, body, .. } => {
                    walk_expr(condition, out);
                    walk_stmts(body, out);
                }
                Stmt::If { arms, otherwise, .. } => {
                    for Arm { condition, body } in arms {
                        walk_expr(condition, out);
                        walk_stmts(body, out);
                    }
                    walk_stmts(otherwise, out);
                }
                Stmt::StoreAt { array, at, value, .. } => {
                    walk_expr(array, out);
                    for index in at {
                        walk_expr(index, out);
                    }
                    walk_expr(value, out);
                }
                Stmt::Return { value: Some(expr), .. } => walk_expr(expr, out),
                Stmt::Call { args, .. } => {
                    for arg in args {
                        walk_expr(arg, out);
                    }
                }
                Stmt::Return { value: None, .. } | Stmt::Break { .. } => {}
            }
        }
    }

    fn walk_expr(expr: &Expr, out: &mut Vec<bool>) {
        match expr {
            Expr::Binary { op, lhs, rhs, nonnegative, .. } => {
                walk_expr(lhs, out);
                walk_expr(rhs, out);
                if matches!(op, BinOp::Div | BinOp::Mod) {
                    out.push(*nonnegative);
                }
            }
            Expr::Neg { operand, .. } | Expr::Not { operand, .. } => walk_expr(operand, out),
            Expr::Compare { lhs, rhs, .. } | Expr::Logic { lhs, rhs, .. } => {
                walk_expr(lhs, out);
                walk_expr(rhs, out);
            }
            Expr::Call { args, .. } => {
                for arg in args {
                    walk_expr(arg, out);
                }
            }
            Expr::NewArray { items, .. } => {
                for item in items {
                    walk_expr(item, out);
                }
            }
            Expr::Filled { length, value, .. } => {
                walk_expr(length, out);
                walk_expr(value, out);
            }
            Expr::At { array, at, .. } => {
                walk_expr(array, out);
                for index in at {
                    walk_expr(index, out);
                }
            }
            Expr::Const(_) | Expr::Load { .. } | Expr::TimeNow { .. } | Expr::Count { .. } => {}
        }
    }

    #[test]
    fn the_benchmark_loop_is_proven() {
        assert_eq!(
            flags(
                "var.local.mut.i64 ['sum'] = [|0|];\n\
                 loop.temp.range.i64 ['i'] = [|1|, |100000000|] {\n\
                 set ['sum'] = [math { ('sum' + 'i') mod 1000000007 }];\n\
                 }\n\
                 print['sum' \\n];\n"
            ),
            [true]
        );
    }

    #[test]
    fn a_dividend_that_may_be_negative_is_not() {
        assert_eq!(
            flags("var.local.i64 ['x'] = [|-5|];\nprint[math { 'x' mod 3 } \\n];\n"),
            [false]
        );
    }

    // The range of a floored remainder needs only the divisor's sign; the proof that
    // floored and truncated agree needs the dividend's too. A counter that starts
    // below zero must leave the flag alone, however tidy the result's range is.
    #[test]
    fn a_counter_from_below_zero_is_not() {
        assert_eq!(
            flags(
                "loop.temp.range.i64 ['i'] = [|-3|, |5|] {\n\
                 print[math { 'i' mod 7 } \\n];\n\
                 }\n"
            ),
            [false]
        );
    }

    #[test]
    fn a_divisor_that_may_be_nought_is_not() {
        assert_eq!(
            flags(
                "var.local.i64 ['nine'] = [|9|];\n\
                 loop.temp.range.i64 ['i'] = [|0|, |5|] {\n\
                 print[math { 'nine' mod 'i' } \\n];\n\
                 }\n"
            ),
            [false]
        );
    }

    #[test]
    fn both_arms_of_an_if_must_agree() {
        let proven = "var.local.i64 ['c'] = [|1|];\n\
                      var.local.mut.i64 ['x'] = [|5|];\n\
                      if [math { 'c' = 1 }] { set ['x'] = [|7|]; } else { set ['x'] = [|2|]; }\n\
                      print[math { 'x' mod 3 } \\n];\n";
        let unproven = "var.local.i64 ['c'] = [|1|];\n\
                        var.local.mut.i64 ['x'] = [|5|];\n\
                        if [math { 'c' = 1 }] { set ['x'] = [|7|]; } else { set ['x'] = [|-2|]; }\n\
                        print[math { 'x' mod 3 } \\n];\n";
        assert_eq!(flags(proven), [true]);
        assert_eq!(flags(unproven), [false]);
    }

    #[test]
    fn a_break_carries_its_state_out() {
        // The break leaves with `x` at -1; the body's end always has it back at 5.
        // The `mod` after the loop must see both ways out.
        assert_eq!(
            flags(
                "var.local.mut.i64 ['x'] = [|5|];\n\
                 loop.temp.range.i64 ['i'] = [|1|, |10|] {\n\
                 if [math { 'i' = 3 }] { set ['x'] = [|-1|]; break; }\n\
                 set ['x'] = [|5|];\n\
                 }\n\
                 print[math { 'x' mod 3 } \\n];\n"
            ),
            [false]
        );
    }

    #[test]
    fn wrapping_forgets_and_trapping_remembers() {
        // The sum may pass the top of `i64`. Under `wrap` it may then be anything, so
        // nothing is proven; under `trap` a program still running holds a value that
        // fit, which is enough.
        let body = "var.local.mut.i64 ['sum'] = [|2|];\n\
                    loop.temp.range.i64 ['i'] = [|1|, |100|] {\n\
                    set ['sum'] = [math { 'sum' * 'sum' }];\n\
                    }\n\
                    print[math { 'sum' mod 7 } \\n];\n";
        assert_eq!(flags(body), [false]);
        assert_eq!(flags(&format!("defaults.overflow.trap;\n{body}")), [true]);
    }

    #[test]
    fn a_while_body_spoils_what_it_stores() {
        assert_eq!(
            flags(
                "var.local.mut.i64 ['x'] = [|5|];\n\
                 var.local.mut.bool ['go'] = [|true|];\n\
                 loop.while ['go'] { set ['x'] = [math { 'x' - 1 }]; set ['go'] = [|false|]; }\n\
                 print[math { 'x' mod 3 } \\n];\n"
            ),
            [false]
        );
    }

    #[test]
    fn a_parameter_is_only_its_type() {
        assert_eq!(
            flags("fn.local.i64 ['half'] [i64 'n'] { return math { 'n' mod 2 }; }\nprint[half[|8|] \\n];\n"),
            [false]
        );
    }

    #[test]
    fn unsigned_never_carries_the_flag() {
        assert_eq!(
            flags("var.local.ui64 ['x'] = [|5|];\nprint[math { 'x' mod 3 } \\n];\n"),
            [false]
        );
    }
}
