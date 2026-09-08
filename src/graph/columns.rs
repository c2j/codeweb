//! #165 (P0): per-procedure/per-package aggregated column-analysis query surface.
//!
//! A routine that writes the same table across multiple statements (e.g. one `INSERT`
//! per branch of a legacy PL/SQL procedure) attaches a distinct [`ColumnAnalysis`] to
//! each statement's `TableAccess` edges. Without aggregation, a caller wanting "every
//! hard filter and join this procedure relies on" would have to walk the graph itself
//! and reconcile duplicates across edges. [`column_analysis_of_routine`] (and its
//! package-scoped sibling [`column_analysis_of_package`]) does that walk once and
//! unions every diagnostic field with `HashSet`-backed dedup, so the same filter/join
//! attached to two statements is reported once, not twice.
//!
//! The output schema ([`AggregatedColumnAnalysis`]) mirrors [`ColumnAnalysis`] field
//! names 1:1 — issue #165 explicitly rules out an extra display-tree wrapper layer —
//! so the MCP/HTTP surfaces planned for a later task can reuse this struct unchanged.

use std::collections::HashSet;

use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use petgraph::Direction;

use crate::graph::{CodeGraph, Edge, Node};
use crate::parser::{
    ColumnMapping, EnumMapping, HardFilter, InsertColumnInfo, JoinCondition, SelectIntoMapping,
    UpdateColumnInfo,
};

/// `codeweb columns` JSON output schema (schema_version=1).
///
/// Field names mirror [`ColumnAnalysis`](crate::parser::ColumnAnalysis) 1:1 by design
/// (issue #165). `procedure` names the queried entity (the resolved procedure/function
/// name in `--procedure` mode, or the resolved package name in `--package` mode, since
/// a package query has no single "the" procedure). `package` is `Some` whenever the
/// query's scope is known to belong to a package: the routine's own
/// [`RoutineId::package`](crate::graph::RoutineId) in `--procedure` mode, or the queried
/// package's own name in `--package` mode.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AggregatedColumnAnalysis {
    pub schema_version: u32,
    pub procedure: String,
    pub package: Option<String>,
    pub tables: Vec<String>,
    pub join_conditions: Vec<JoinCondition>,
    pub hard_filters: Vec<HardFilter>,
    pub select_into: Vec<SelectIntoMapping>,
    pub enum_mappings: Vec<EnumMapping>,
    pub column_mappings: Vec<ColumnMapping>,
    pub insert_columns: Vec<InsertColumnInfo>,
    pub update_columns: Vec<UpdateColumnInfo>,
    pub read_tables: Vec<String>,
}

/// Working accumulator for [`collect_diagnostics`] — same fields as
/// [`AggregatedColumnAnalysis`] minus the identity fields (`schema_version`,
/// `procedure`, `package`), which only the public entry points know how to fill in.
#[derive(Default)]
struct Diagnostics {
    tables: Vec<String>,
    join_conditions: Vec<JoinCondition>,
    hard_filters: Vec<HardFilter>,
    select_into: Vec<SelectIntoMapping>,
    enum_mappings: Vec<EnumMapping>,
    column_mappings: Vec<ColumnMapping>,
    insert_columns: Vec<InsertColumnInfo>,
    update_columns: Vec<UpdateColumnInfo>,
    read_tables: Vec<String>,
}

/// Scan every `TableAccess` edge of `routines` (both directions, mirroring
/// [`super::lineage::mappings_of_routine`]'s defensive both-direction walk — today's
/// builder always emits routine→table edges outgoing, but scanning both keeps this
/// correct if that ever changes) and union each `ColumnAnalysis` diagnostic field with
/// `HashSet`-based dedup.
///
/// `table_filter`, when set, narrows which edges are scanned to those whose *other*
/// endpoint (the table) matches case-insensitively — this is what shrinks `tables`
/// (and `read_tables`) to one table. Row-level fields (`join_conditions`,
/// `hard_filters`, `select_into`, ...) are NOT filtered by column value: the same
/// `ColumnAnalysis` is attached to every edge of one statement (the write edge and
/// every read edge), so an edge to the filtered table already carries exactly that
/// table's statements' constraints — including constraints that reference other
/// tables (e.g. a join partner, or a filter on a dimension table used by the same
/// statement). Dropping rows that merely *mention* another table would throw away
/// the join/filter context the caller is asking for; statement-level isolation for a
/// single table is what `read_tables` is for.
fn collect_diagnostics(
    graph: &CodeGraph,
    routines: &[NodeIndex],
    table_filter: Option<&str>,
) -> Diagnostics {
    let filter_lower = table_filter.map(|t| t.to_lowercase());

    let mut tables: HashSet<String> = HashSet::new();
    let mut read_tables: HashSet<String> = HashSet::new();

    let mut join_conditions: Vec<JoinCondition> = Vec::new();
    let mut jc_seen: HashSet<JoinCondition> = HashSet::new();
    let mut hard_filters: Vec<HardFilter> = Vec::new();
    let mut hf_seen: HashSet<HardFilter> = HashSet::new();
    let mut select_into: Vec<SelectIntoMapping> = Vec::new();
    let mut si_seen: HashSet<SelectIntoMapping> = HashSet::new();
    let mut enum_mappings: Vec<EnumMapping> = Vec::new();
    let mut em_seen: HashSet<EnumMapping> = HashSet::new();
    let mut column_mappings: Vec<ColumnMapping> = Vec::new();
    let mut cm_seen: HashSet<ColumnMapping> = HashSet::new();
    let mut insert_columns: Vec<InsertColumnInfo> = Vec::new();
    let mut ic_seen: HashSet<InsertColumnInfo> = HashSet::new();
    let mut update_columns: Vec<UpdateColumnInfo> = Vec::new();
    let mut uc_seen: HashSet<UpdateColumnInfo> = HashSet::new();

    for &routine in routines {
        for dir in [Direction::Outgoing, Direction::Incoming] {
            for edge_ref in graph.edges_directed(routine, dir) {
                let Edge::TableAccess {
                    column_analysis: Some(analysis),
                    ..
                } = edge_ref.weight()
                else {
                    continue;
                };

                let other = if dir == Direction::Outgoing {
                    edge_ref.target()
                } else {
                    edge_ref.source()
                };
                let table_name = match &graph[other] {
                    Node::Table { name, .. } | Node::View { name, .. } => name.clone(),
                    _ => continue,
                };

                if let Some(filt) = &filter_lower {
                    if table_name.to_lowercase() != *filt {
                        continue;
                    }
                }

                tables.insert(table_name);

                for jc in &analysis.join_conditions {
                    if jc_seen.insert(jc.clone()) {
                        join_conditions.push(jc.clone());
                    }
                }
                for hf in &analysis.hard_filters {
                    if hf_seen.insert(hf.clone()) {
                        hard_filters.push(hf.clone());
                    }
                }
                for si in &analysis.select_into {
                    if si_seen.insert(si.clone()) {
                        select_into.push(si.clone());
                    }
                }
                for em in &analysis.enum_mappings {
                    if em_seen.insert(em.clone()) {
                        enum_mappings.push(em.clone());
                    }
                }
                for cm in &analysis.column_mappings {
                    if cm_seen.insert(cm.clone()) {
                        column_mappings.push(cm.clone());
                    }
                }
                for ic in &analysis.insert_columns {
                    if ic_seen.insert(ic.clone()) {
                        insert_columns.push(ic.clone());
                    }
                }
                for uc in &analysis.update_columns {
                    if uc_seen.insert(uc.clone()) {
                        update_columns.push(uc.clone());
                    }
                }
                if let Some(rt) = &analysis.read_tables {
                    for t in rt {
                        read_tables.insert(t.clone());
                    }
                }
            }
        }
    }

    let mut tables: Vec<String> = tables.into_iter().collect();
    tables.sort();
    let mut read_tables: Vec<String> = read_tables.into_iter().collect();
    read_tables.sort();

    Diagnostics {
        tables,
        join_conditions,
        hard_filters,
        select_into,
        enum_mappings,
        column_mappings,
        insert_columns,
        update_columns,
        read_tables,
    }
}

/// Aggregate every `TableAccess` diagnostic for one routine (procedure or function)
/// into a single [`AggregatedColumnAnalysis`]. Returns `None` when `routine` is not a
/// `Node::Procedure`/`Node::Function` — callers should verify the node type up front
/// (as `codeweb columns`'s CLI handler does) so this is a defensive fallback, not the
/// primary error path.
pub fn column_analysis_of_routine(
    graph: &CodeGraph,
    routine: NodeIndex,
    table_filter: Option<&str>,
) -> Option<AggregatedColumnAnalysis> {
    let (name, package) = match &graph[routine] {
        Node::Procedure { id, .. } | Node::Function { id, .. } => {
            (id.name.clone(), id.package.clone())
        }
        _ => return None,
    };

    let diag = collect_diagnostics(graph, &[routine], table_filter);

    Some(AggregatedColumnAnalysis {
        schema_version: 1,
        procedure: name,
        package,
        tables: diag.tables,
        join_conditions: diag.join_conditions,
        hard_filters: diag.hard_filters,
        select_into: diag.select_into,
        enum_mappings: diag.enum_mappings,
        column_mappings: diag.column_mappings,
        insert_columns: diag.insert_columns,
        update_columns: diag.update_columns,
        read_tables: diag.read_tables,
    })
}

/// Aggregate every `TableAccess` diagnostic across all procedures/functions a package
/// contains (via `Edge::ContainsRoutine`, the same edge `codeweb detail`'s package
/// table-access summary uses — see `print_table_summary` in `src/main.rs`) into a
/// single [`AggregatedColumnAnalysis`]. Returns `None` when `package` is not a
/// `Node::Package`.
///
/// A package with zero `ContainsRoutine` children (e.g. an empty or partially-parsed
/// package) yields an aggregation with empty diagnostic vectors rather than `None` —
/// the package itself was found, so this is not the "unknown name" error case the CLI
/// guards against.
pub fn column_analysis_of_package(
    graph: &CodeGraph,
    package: NodeIndex,
    table_filter: Option<&str>,
) -> Option<AggregatedColumnAnalysis> {
    let pkg_name = match &graph[package] {
        Node::Package { name, .. } => name.clone(),
        _ => return None,
    };

    let children: Vec<NodeIndex> = graph
        .edges_directed(package, Direction::Outgoing)
        .filter(|e| matches!(e.weight(), Edge::ContainsRoutine))
        .map(|e| e.target())
        .collect();

    let diag = collect_diagnostics(graph, &children, table_filter);

    Some(AggregatedColumnAnalysis {
        schema_version: 1,
        procedure: pkg_name.clone(),
        package: Some(pkg_name),
        tables: diag.tables,
        join_conditions: diag.join_conditions,
        hard_filters: diag.hard_filters,
        select_into: diag.select_into,
        enum_mappings: diag.enum_mappings,
        column_mappings: diag.column_mappings,
        insert_columns: diag.insert_columns,
        update_columns: diag.update_columns,
        read_tables: diag.read_tables,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{AccessMode, DataFlowKind, RoutineId, RoutineKind, SourceLocation};
    use crate::parser::{ColumnAnalysis, FilterOperator, FilterValue};
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn loc() -> SourceLocation {
        SourceLocation {
            file: Arc::new(PathBuf::from("t.sql")),
            line: 1,
        }
    }

    fn empty_analysis() -> ColumnAnalysis {
        ColumnAnalysis {
            alias_map: BTreeMap::new(),
            column_refs: Vec::new(),
            join_conditions: Vec::new(),
            hard_filters: Vec::new(),
            enum_mappings: Vec::new(),
            select_into: Vec::new(),
            insert_columns: Vec::new(),
            update_columns: Vec::new(),
            column_mappings: Vec::new(),
            read_tables: None,
        }
    }

    /// Two separate `TableAccess` edges to the same table, each carrying an
    /// analysis with the same `HardFilter` (as if two statements wrote the same
    /// table with an identical filter): aggregation must dedup to one occurrence.
    #[test]
    fn dedups_identical_hard_filter_across_two_edges() {
        let mut graph = CodeGraph::new();
        let routine = graph.add_node(Node::Procedure {
            id: RoutineId {
                schema: None,
                package: None,
                name: "prc_test".to_string(),
                kind: RoutineKind::Procedure,
            },
            location: loc(),
            partial: false,
            body_sql: Vec::new(),
        });
        let table = graph.add_node(Node::Table {
            schema: None,
            name: "s1_src".to_string(),
            explicit: true,
            system: false,
            location: None,
            columns: Box::new(Vec::new()),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        });

        let same_filter = HardFilter {
            table: Some("s1_src".to_string()),
            column: "scdm".to_string(),
            operator: FilterOperator::Eq,
            value: FilterValue::String("001".to_string()),
            transform: None,
        };

        for _ in 0..2 {
            let mut analysis = empty_analysis();
            analysis.hard_filters.push(same_filter.clone());
            graph.add_edge(
                routine,
                table,
                Edge::TableAccess {
                    flow_kind: DataFlowKind::DmlAccess,
                    modes: AccessMode::Read,
                    write_kinds: Default::default(),
                    location: loc(),
                    column_analysis: Some(Box::new(analysis)),
                },
            );
        }

        let result = column_analysis_of_routine(&graph, routine, None).expect("aggregation");
        assert_eq!(
            result.hard_filters.len(),
            1,
            "identical hard_filter from two edges should dedup to one, got: {:?}",
            result.hard_filters
        );
        assert_eq!(result.hard_filters[0], same_filter);
        assert_eq!(result.tables, vec!["s1_src".to_string()]);
    }

    #[test]
    fn non_routine_node_returns_none() {
        let mut graph = CodeGraph::new();
        let table = graph.add_node(Node::Table {
            schema: None,
            name: "t".to_string(),
            explicit: true,
            system: false,
            location: None,
            columns: Box::new(Vec::new()),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        });
        assert!(column_analysis_of_routine(&graph, table, None).is_none());
    }
}
