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
}
