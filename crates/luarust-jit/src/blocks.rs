//! Finding the basic blocks in a run of bytecode.
//!
//! Bytecode is flat: a list of instructions with jumps between them. LLVM is not — it
//! wants a graph of blocks, each entered only at the top and left only at the bottom. So
//! before anything can be emitted, the flat thing has to be read as the graph it always
//! was.
//!
//! A block begins at a **leader**, and an instruction is a leader if it is the first one,
//! or something jumps to it, or the instruction before it was a jump or a return. That is
//! the whole rule, and it is the oldest one in compilers.

use luarust_vm::chunk::Op;
use std::collections::BTreeSet;

/// Where each basic block of this code begins.
pub fn leaders(code: &[Op]) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    out.insert(0);

    for (at, op) in code.iter().enumerate() {
        match op {
            Op::Jump { target } => {
                out.insert(*target as usize);
                out.insert(at + 1);
            }
            Op::JumpIfFalse { target, .. }
            | Op::JumpIfTrue { target, .. }
            | Op::JumpIfGreater { target, .. }
            | Op::JumpIfEqual { target, .. } => {
                out.insert(*target as usize);
                out.insert(at + 1);
            }
            // Nothing falls out of these, so whatever follows is only reachable by being
            // jumped to -- and is a block whether or not anything jumps to it.
            Op::Return { .. } | Op::ReturnNothing | Op::Halt => {
                out.insert(at + 1);
            }
            _ => {}
        }
    }

    // The one past the end is not an instruction, however many jumps point at it.
    out.retain(|at| *at < code.len());
    out
}

/// Whether this instruction ends its block by itself.
pub fn terminates(op: &Op) -> bool {
    matches!(
        op,
        Op::Jump { .. }
            | Op::JumpIfFalse { .. }
            | Op::JumpIfTrue { .. }
            | Op::JumpIfGreater { .. }
            | Op::JumpIfEqual { .. }
            | Op::Return { .. }
            | Op::ReturnNothing
            | Op::Halt
    )
}

/// Which instructions can actually be reached from `entry`, and which routines called.
///
/// Starting a program reaches everything -- entry is nought and there is nowhere the flow
/// has not been. Taking one over at a loop head does not: the code that ran before the
/// loop cannot be reached again unless something jumps back to it, and a chunk's other
/// routines are only worth compiling if this entry can reach a call to them.
///
/// Note that "before the entry" and "unreachable" are not the same thing. Entering at an
/// inner loop's head, the outer loop's back edge jumps to a head that is *behind* the
/// entry, and everything the outer loop does is live. Following the graph gets that right
/// where comparing instruction numbers would not.
pub fn reachable(code: &[Op], entry: usize) -> BTreeSet<usize> {
    let mut seen = BTreeSet::new();
    let mut todo = vec![entry];
    while let Some(at) = todo.pop() {
        if at >= code.len() || !seen.insert(at) {
            continue;
        }
        let op = &code[at];
        match op {
            Op::Jump { target } => todo.push(*target as usize),
            Op::JumpIfFalse { target, .. }
            | Op::JumpIfTrue { target, .. }
            | Op::JumpIfGreater { target, .. }
            | Op::JumpIfEqual { target, .. } => {
                todo.push(*target as usize);
                todo.push(at + 1);
            }
            // Nothing carries on from these.
            Op::Return { .. } | Op::ReturnNothing | Op::Halt => {}
            _ => todo.push(at + 1),
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use super::*;
    use luarust_vm::chunk::Op;

    #[test]
    fn a_straight_run_is_one_block() {
        let code = [Op::Halt];
        assert_eq!(leaders(&code), BTreeSet::from([0]));
    }

    #[test]
    fn a_jump_starts_a_block_where_it_lands_and_where_it_leaves() {
        //  0 jump 2
        //  1 halt       <- unreachable, and still its own block
        //  2 halt
        let code = [Op::Jump { target: 2 }, Op::Halt, Op::Halt];
        assert_eq!(leaders(&code), BTreeSet::from([0, 1, 2]));
    }

    #[test]
    fn a_jump_past_the_end_starts_no_block_there() {
        // A loop's exit jump points one past the last instruction when the loop is the
        // last thing in the program. There is no instruction there to lead anything.
        let code = [Op::Jump { target: 1 }, Op::Halt];
        assert_eq!(leaders(&code), BTreeSet::from([0, 1]));
    }

    #[test]
    fn a_backward_jump_makes_the_loop_top_a_leader() {
        //  0 halt
        //  1 jump 1     <- lands on itself
        let code = [Op::Halt, Op::Jump { target: 1 }];
        assert_eq!(leaders(&code), BTreeSet::from([0, 1]));
    }

    #[test]
    fn everything_is_reachable_from_the_beginning() {
        let code = [Op::Jump { target: 2 }, Op::Halt, Op::Halt];
        // Not quite everything: instruction 1 is jumped over and nothing lands on it.
        assert_eq!(reachable(&code, 0), BTreeSet::from([0, 2]));
    }

    #[test]
    fn what_ran_before_the_entry_is_left_out() {
        //  0 halt        <- before the entry, and nothing jumps back to it
        //  1 jump 1      <- a loop on itself, entered here
        let code = [Op::Halt, Op::Jump { target: 1 }];
        assert_eq!(reachable(&code, 1), BTreeSet::from([1]));
    }

    #[test]
    fn an_enclosing_loop_is_reached_even_though_it_is_behind() {
        // Coming in at the inner loop's head, the outer loop's back edge lands behind the
        // entry -- and everything the outer loop does is live. Comparing instruction
        // numbers would call this dead; following the graph does not.
        //
        //  0 halt          <- genuinely before it all
        //  1 jump 3        <- the outer loop's head
        //  2 jump 1        <- the outer back edge
        //  3 jump 2        <- the inner loop, entered here
        let code = [
            Op::Halt,
            Op::Jump { target: 3 },
            Op::Jump { target: 1 },
            Op::Jump { target: 2 },
        ];
        assert_eq!(reachable(&code, 3), BTreeSet::from([1, 2, 3]));
    }
}
