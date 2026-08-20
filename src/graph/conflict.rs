//! Cross-procedure table-lock conflict detection (openGauss 8-level matrix).
//!
//! Reports "would conflict IF executed concurrently". No transaction model.
//! Default reporters should filter to High.

use crate::graph::AccessMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConflictSeverity {
    Medium,
    High,
}

const CONFLICT: [[bool; 8]; 8] = [
    //        L1     L2     L3     L4     L5     L6     L7     L8
    /* L1 */ [false, false, false, false, false, false, false, true],
    /* L2 */ [false, false, false, false, false, false, true, true],
    /* L3 */ [false, false, false, false, true, true, true, true],
    /* L4 */ [false, false, false, true, true, true, true, true],
    /* L5 */ [false, false, true, true, false, true, true, true],
    /* L6 */ [false, false, true, true, true, true, true, true],
    /* L7 */ [false, true, true, true, true, true, true, true],
    /* L8 */ [true, true, true, true, true, true, true, true],
];

fn levels_in(modes: AccessMode) -> impl Iterator<Item = usize> {
    (1u8..=8).filter_map(move |lvl| {
        let present = match lvl {
            1 => modes.contains(AccessMode::Read),
            2 => modes.contains(AccessMode::LockRead),
            3 => modes.contains(AccessMode::Write),
            4 => modes.contains(AccessMode::ShareUpdateExclusive),
            5 => modes.contains(AccessMode::Share),
            6 => modes.contains(AccessMode::ShareRowExclusive),
            7 => modes.contains(AccessMode::Exclusive),
            8 => modes.contains(AccessMode::AccessExclusive),
            _ => false,
        };
        present.then_some((lvl - 1) as usize)
    })
}

pub fn locks_conflict(a: AccessMode, b: AccessMode) -> bool {
    for i in levels_in(a) {
        for j in levels_in(b) {
            if CONFLICT[i][j] {
                return true;
            }
        }
    }
    false
}

pub fn conflict_severity(a: AccessMode, b: AccessMode) -> Option<ConflictSeverity> {
    if !locks_conflict(a, b) {
        return None;
    }
    if a.contains(AccessMode::AccessExclusive) || b.contains(AccessMode::AccessExclusive) {
        Some(ConflictSeverity::High)
    } else {
        Some(ConflictSeverity::Medium)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::AccessMode;

    #[test]
    fn l8_conflicts_with_everything_including_l1() {
        assert_eq!(
            conflict_severity(AccessMode::AccessExclusive, AccessMode::Read),
            Some(ConflictSeverity::High)
        );
        assert_eq!(
            conflict_severity(AccessMode::Read, AccessMode::AccessExclusive),
            Some(ConflictSeverity::High)
        );
    }

    #[test]
    fn two_inserts_do_not_conflict() {
        assert_eq!(
            conflict_severity(AccessMode::Write, AccessMode::Write),
            None
        );
    }

    #[test]
    fn create_index_l5_vs_insert_l3_is_medium() {
        assert_eq!(
            conflict_severity(AccessMode::Share, AccessMode::Write),
            Some(ConflictSeverity::Medium)
        );
    }

    #[test]
    fn two_selects_do_not_conflict() {
        assert_eq!(conflict_severity(AccessMode::Read, AccessMode::Read), None);
    }

    #[test]
    fn l7_conflicts_with_l2_but_l6_does_not() {
        assert_eq!(
            conflict_severity(AccessMode::Exclusive, AccessMode::LockRead),
            Some(ConflictSeverity::Medium)
        );
        assert_eq!(
            conflict_severity(AccessMode::ShareRowExclusive, AccessMode::LockRead),
            None
        );
    }

    #[test]
    fn mixed_bits_use_any_pair() {
        let a = AccessMode::Read | AccessMode::AccessExclusive;
        assert_eq!(
            conflict_severity(a, AccessMode::Read),
            Some(ConflictSeverity::High)
        );
    }

    #[test]
    fn vacuum_l4_self_conflicts_medium() {
        assert_eq!(
            conflict_severity(
                AccessMode::ShareUpdateExclusive,
                AccessMode::ShareUpdateExclusive
            ),
            Some(ConflictSeverity::Medium)
        );
    }
}
