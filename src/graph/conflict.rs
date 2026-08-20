//! Cross-procedure table-lock conflict detection (openGauss 8-level matrix).
//!
//! Reports "would conflict IF executed concurrently". No transaction model.
//! Default reporters should filter to High.

use crate::graph::{AccessMode, CodeGraph, DataFlowKind, Edge, Node};
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConflictSeverity {
    Medium,
    High,
}

const CONFLICT: [[bool; 8]; 8] = [
    //        L1     L2     L3     L4     L5     L6     L7     L8
    /* L1 */
    [false, false, false, false, false, false, false, true],
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProcTableLock {
    pub proc: NodeIndex,
    pub table: NodeIndex,
    pub modes: AccessMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockConflict {
    pub table: NodeIndex,
    pub proc_a: NodeIndex,
    pub proc_b: NodeIndex,
    pub modes_a: AccessMode,
    pub modes_b: AccessMode,
    pub severity: ConflictSeverity,
}

pub fn conflicts_among(locks: &[ProcTableLock]) -> Vec<LockConflict> {
    let mut merged: HashMap<(NodeIndex, NodeIndex), AccessMode> = HashMap::new();
    for lock in locks {
        let key = (lock.proc, lock.table);
        merged
            .entry(key)
            .and_modify(|m| *m |= lock.modes)
            .or_insert(lock.modes);
    }

    let mut by_table: HashMap<NodeIndex, Vec<(NodeIndex, AccessMode)>> = HashMap::new();
    for ((proc, table), modes) in merged {
        by_table.entry(table).or_default().push((proc, modes));
    }

    let mut out = Vec::new();
    for (table, mut procs) in by_table {
        procs.sort_by_key(|(p, _)| p.index());
        for i in 0..procs.len() {
            for j in (i + 1)..procs.len() {
                let (proc_a, modes_a) = procs[i];
                let (proc_b, modes_b) = procs[j];
                if proc_a == proc_b {
                    continue;
                }
                if let Some(severity) = conflict_severity(modes_a, modes_b) {
                    out.push(LockConflict {
                        table,
                        proc_a,
                        proc_b,
                        modes_a,
                        modes_b,
                        severity,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| {
        b.severity
            .cmp(&a.severity)
            .then(a.table.index().cmp(&b.table.index()))
            .then(a.proc_a.index().cmp(&b.proc_a.index()))
            .then(a.proc_b.index().cmp(&b.proc_b.index()))
    });
    out
}

pub fn find_conflicts(graph: &CodeGraph) -> Vec<LockConflict> {
    let mut locks = Vec::new();
    for edge in graph.edge_references() {
        let Edge::TableAccess {
            flow_kind, modes, ..
        } = edge.weight()
        else {
            continue;
        };
        if *flow_kind != DataFlowKind::DmlAccess {
            continue;
        }
        let src = edge.source();
        let dst = edge.target();
        if !matches!(graph[src], Node::Procedure { .. } | Node::Function { .. }) {
            continue;
        }
        if !matches!(
            graph[dst],
            Node::Table { .. } | Node::View { .. } | Node::MaterializedView { .. }
        ) {
            continue;
        }
        locks.push(ProcTableLock {
            proc: src,
            table: dst,
            modes: *modes,
        });
    }
    conflicts_among(&locks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::AccessMode;
    use petgraph::graph::NodeIndex;

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

    #[test]
    fn conflicts_among_truncate_vs_select_is_high() {
        let locks = vec![
            ProcTableLock {
                proc: NodeIndex::new(1),
                table: NodeIndex::new(10),
                modes: AccessMode::AccessExclusive,
            },
            ProcTableLock {
                proc: NodeIndex::new(2),
                table: NodeIndex::new(10),
                modes: AccessMode::Read,
            },
        ];
        let out = conflicts_among(&locks);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].severity, ConflictSeverity::High);
    }

    #[test]
    fn conflicts_among_two_inserts_empty() {
        let locks = vec![
            ProcTableLock {
                proc: NodeIndex::new(1),
                table: NodeIndex::new(10),
                modes: AccessMode::Write,
            },
            ProcTableLock {
                proc: NodeIndex::new(2),
                table: NodeIndex::new(10),
                modes: AccessMode::Write,
            },
        ];
        assert!(conflicts_among(&locks).is_empty());
    }

    #[test]
    fn conflicts_among_skips_same_proc() {
        let locks = vec![
            ProcTableLock {
                proc: NodeIndex::new(1),
                table: NodeIndex::new(10),
                modes: AccessMode::AccessExclusive,
            },
            ProcTableLock {
                proc: NodeIndex::new(1),
                table: NodeIndex::new(10),
                modes: AccessMode::Read,
            },
        ];
        assert!(conflicts_among(&locks).is_empty());
    }

    #[test]
    fn conflicts_among_ors_modes_per_proc_table() {
        let locks = vec![
            ProcTableLock {
                proc: NodeIndex::new(1),
                table: NodeIndex::new(10),
                modes: AccessMode::Read,
            },
            ProcTableLock {
                proc: NodeIndex::new(1),
                table: NodeIndex::new(10),
                modes: AccessMode::AccessExclusive,
            },
            ProcTableLock {
                proc: NodeIndex::new(2),
                table: NodeIndex::new(10),
                modes: AccessMode::Read,
            },
        ];
        let out = conflicts_among(&locks);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].severity, ConflictSeverity::High);
    }
}
