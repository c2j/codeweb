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

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use petgraph::Direction;

use crate::graph::key::NodeKey;
use crate::graph::store::GraphStore;
use crate::graph::{write_kind_label, AccessMode, CodeGraph, Edge, Node, SourceLocation};
use crate::parser::{
    ColumnMapping, CrossTableEquality, EnumMapping, FilterOperator, FilterValue, HardFilter,
    InsertColumnInfo, JoinCondition, RoutineParameter, SelectIntoMapping, UpdateColumnInfo,
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
    cross_table_equalities: Vec<CrossTableEquality>,
    hard_filters: Vec<HardFilter>,
    select_into: Vec<SelectIntoMapping>,
    enum_mappings: Vec<EnumMapping>,
    column_mappings: Vec<ColumnMapping>,
    insert_columns: Vec<InsertColumnInfo>,
    update_columns: Vec<UpdateColumnInfo>,
    read_tables: Vec<String>,
    /// First statement edge each row-level diagnostic was seen on. `seed-hints`
    /// attaches these as `provenance`; `columns --format json` ignores them, so
    /// its output is unchanged.
    hard_filter_provenance: HashMap<HardFilter, SourceLocation>,
    cross_table_equality_provenance: HashMap<CrossTableEquality, SourceLocation>,
    enum_mapping_provenance: HashMap<EnumMapping, SourceLocation>,
}

/// `codeweb columns --format seed-hints` output schema (issue #181).
///
/// This is a **predicate/table requirement inventory**, not a seed specification.
/// The consumer is a test-data generator, so the document is organized by "what
/// rows must exist and what shape they need" rather than by statement: the
/// routine's signature, one entry per table with the operations performed on it,
/// the literal filters that select the rows, and the cross-table equalities that
/// tie rows in different tables together.
///
/// # Hints are necessary, not sufficient
///
/// The inventory is derived statically and is deliberately incomplete:
///
/// - Dynamic SQL (`EXECUTE IMMEDIATE`, `OPEN ... FOR`) is not analyzed, so any
///   predicate it builds is invisible.
/// - A predicate whose alias or `%ROWTYPE` record cannot be resolved is either
///   dropped or reported without a `table` (see [`HintConfidence::Low`]).
/// - Enumerations list values the routine *mentions*, not values that make a
///   branch *reachable* — picking which value to seed is the caller's decision.
///
/// Satisfying every hint therefore does not guarantee the routine runs, and the
/// output never contains SQL: codeweb does not generate seed data and never
/// connects to a database. Each hint carries [`Provenance`] (file/line) and a
/// [`HintConfidence`] so a consumer can weigh it.
///
/// Field names for the diagnostic parts stay close to
/// [`AggregatedColumnAnalysis`] (`hard_filters`, ...) so a consumer that already
/// reads `columns --format json` reuses its parsing; the hint wrappers add
/// `provenance` and `confidence`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SeedHints {
    pub schema_version: u32,
    /// Self-describing document kind. Lets a consumer assert it received the
    /// inventory rather than some other `columns` payload.
    pub kind: &'static str,
    /// One-line statement of the necessary-not-sufficient contract above, in the
    /// document itself so it travels with the JSON.
    pub caveat: &'static str,
    /// Resolved procedure/function name in `--procedure` mode, or the resolved
    /// package name in `--package` mode (same identity rule as
    /// [`AggregatedColumnAnalysis::procedure`]).
    pub procedure: String,
    pub package: Option<String>,
    /// Declared parameters, in signature order.
    pub parameters: Vec<RoutineParameter>,
    /// One entry per table the routine touches, sorted by name.
    pub tables: Vec<TableSeedHint>,
    pub hard_filters: Vec<HardFilterHint>,
    /// Equalities between columns of *different* tables whose sides are not plain
    /// column references (e.g. `substr(c.trade_no, -3) = r.check_type`). Plain
    /// `column = column` pairs stay in `columns --format json`'s
    /// `join_conditions`; these are the ones that cannot be expressed there.
    pub cross_table_equalities: Vec<CrossTableEqualityHint>,
    /// Literal values of the columns configured as discriminators
    /// (`[analysis] discriminator_columns` or `--discriminator`), with where each
    /// was found. Empty when no discriminator column is configured.
    pub discriminator_values: Vec<DiscriminatorValue>,
}

/// `kind` marker for [`SeedHints`].
pub const SEED_HINTS_KIND: &str = "predicate_inventory";
/// `caveat` text for [`SeedHints`].
pub const SEED_HINTS_CAVEAT: &str = "Static hints, not a specification: enumerations may be incomplete \
     (dynamic SQL and unresolved aliases are not analyzed) and satisfying them does not guarantee the \
     routine runs. codeweb never generates SQL and never connects to a database.";

/// Where a hint was found: the source file and the 1-based line of the statement
/// (or predicate) that produced it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Provenance {
    pub file: String,
    pub line: usize,
}

/// How much a hint can be trusted, per the rules documented on each field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HintConfidence {
    High,
    Medium,
    Low,
}

/// One table plus the operations the routine performs on it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TableSeedHint {
    pub name: String,
    /// Sorted, deduplicated operation labels: `read` plus every
    /// [`write_kind_label`](crate::graph::write_kind_label) seen on a
    /// `TableAccess` edge to this table (`insert`, `update`, `delete`, ...).
    pub ops: Vec<String>,
}

/// A hard filter plus where it came from.
///
/// `confidence` is `high` when the filter was attributed to a `table`, `low` when
/// it was not (the filter is real but a generator cannot place it without more
/// context).
#[derive(Debug, Clone, serde::Serialize)]
pub struct HardFilterHint {
    #[serde(flatten)]
    pub filter: HardFilter,
    pub provenance: Provenance,
    pub confidence: HintConfidence,
}

/// A cross-table equality plus where it came from.
///
/// `confidence` is always `high`: [`CrossTableEquality`] is only produced when
/// both sides resolve to a concrete table, so an unresolved side never reaches
/// the output.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CrossTableEqualityHint {
    #[serde(flatten)]
    pub equality: CrossTableEquality,
    pub provenance: Provenance,
    pub confidence: HintConfidence,
}

/// One literal value of a discriminator column and its provenance.
///
/// `confidence` is `high` for a `branch_condition` (a concrete `=`/`IN` condition
/// naming the column) and `medium` for a `cursor_decode` key (a `DECODE`/`CASE`
/// mapping key, where the trigger is a set of values rather than one condition).
#[derive(Debug, Clone, serde::Serialize)]
pub struct DiscriminatorValue {
    /// The configured discriminator column this value belongs to.
    pub column: String,
    pub value: String,
    /// Where the value was found: `cursor_decode` (a `DECODE`/`CASE` mapping on
    /// the column) or `branch_condition` (a PL `=`/`IN` condition naming it).
    pub source: String,
    /// The `IF`/`WHEN` condition that selects this value, when the extraction
    /// could attribute one. `None` for `cursor_decode`.
    pub trigger: Option<String>,
    pub provenance: Provenance,
    pub confidence: HintConfidence,
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
    let mut cross_table_equalities: Vec<CrossTableEquality> = Vec::new();
    let mut cte_seen: HashSet<CrossTableEquality> = HashSet::new();
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

    let mut hard_filter_provenance: HashMap<HardFilter, SourceLocation> = HashMap::new();
    let mut cross_table_equality_provenance: HashMap<CrossTableEquality, SourceLocation> =
        HashMap::new();
    let mut enum_mapping_provenance: HashMap<EnumMapping, SourceLocation> = HashMap::new();

    for &routine in routines {
        for dir in [Direction::Outgoing, Direction::Incoming] {
            for edge_ref in graph.edges_directed(routine, dir) {
                let Edge::TableAccess {
                    column_analysis: Some(analysis),
                    location,
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
                for cte in &analysis.cross_table_equalities {
                    if cte_seen.insert(cte.clone()) {
                        cross_table_equalities.push(cte.clone());
                        cross_table_equality_provenance
                            .entry(cte.clone())
                            .or_insert_with(|| location.clone());
                    }
                }
                for hf in &analysis.hard_filters {
                    if hf_seen.insert(hf.clone()) {
                        hard_filters.push(hf.clone());
                        hard_filter_provenance
                            .entry(hf.clone())
                            .or_insert_with(|| location.clone());
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
                        enum_mapping_provenance
                            .entry(em.clone())
                            .or_insert_with(|| location.clone());
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
        cross_table_equalities,
        hard_filters,
        select_into,
        enum_mappings,
        column_mappings,
        insert_columns,
        update_columns,
        read_tables,
        hard_filter_provenance,
        cross_table_equality_provenance,
        enum_mapping_provenance,
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

    let children = package_children(graph, package);

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

/// String values inside a `FilterValue`, flattening `IN` lists.
fn filter_value_strings(value: &FilterValue) -> Vec<String> {
    match value {
        FilterValue::String(s) => vec![s.clone()],
        FilterValue::Integer(i) => vec![i.to_string()],
        FilterValue::Float(f) => vec![f.clone()],
        FilterValue::List(items) => items.iter().flat_map(filter_value_strings).collect(),
        _ => Vec::new(),
    }
}

/// The configured discriminator column matching `column` (case-insensitive).
fn discriminator_for<'a>(discriminators: &'a [String], column: &str) -> Option<&'a String> {
    discriminators
        .iter()
        .find(|d| d.eq_ignore_ascii_case(column))
}

/// [`Provenance`] of a statement edge, with the file made relative to `base`
/// (the project root) so the hint travels well.
fn provenance_of(location: &SourceLocation, base: &Path) -> Provenance {
    let file = location
        .file
        .strip_prefix(base)
        .map(Path::to_path_buf)
        .unwrap_or_else(|_| location.file.to_path_buf());
    Provenance {
        file: file.display().to_string(),
        line: location.line,
    }
}

/// Source file of a routine node, relative to `base`, for predicate-level
/// provenance.
fn routine_file(graph: &CodeGraph, routine: NodeIndex, base: &Path) -> String {
    match &graph[routine] {
        Node::Procedure { location, .. } | Node::Function { location, .. } => location
            .file
            .strip_prefix(base)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| location.file.to_path_buf())
            .display()
            .to_string(),
        _ => String::new(),
    }
}

fn push_discriminator_value(
    out: &mut Vec<DiscriminatorValue>,
    seen: &mut HashSet<(String, String, String)>,
    entry: DiscriminatorValue,
) {
    let key = (
        entry.column.to_ascii_lowercase(),
        entry.value.clone(),
        entry.source.clone(),
    );
    if seen.insert(key) {
        out.push(entry);
    }
}

/// Literal values of the configured discriminator columns, with provenance.
///
/// Two sources, both already extracted for other purposes:
///
/// - `cursor_decode`: the keys of a `DECODE`/`CASE` mapping on a discriminator
///   column (`enum_mappings`, #165/#167). `confidence` is `medium`: a DECODE key
///   is one of a set, and the mapping carries no single triggering condition.
/// - `branch_condition`: the literal of an `=`/`IN` branch condition naming a
///   discriminator column (`procedure_predicates`, #167), where the rendered
///   condition is the trigger. `confidence` is `high` for `=` (one value, one
///   condition) and `medium` for `IN` (the condition selects a set).
///
/// A clause counts when its resolved column is a discriminator, or when the
/// rendered condition names one: a `%ROWTYPE` record field resolves to the
/// cursor's *projected* column, which need not keep the field's name (the
/// acceptance case's `r_bond_repurchase.operation_no` resolves to `bs`), so the
/// condition text is the reliable signal there. A predicate that filters several
/// columns can therefore contribute a non-discriminator literal; the hint is
/// advisory, not exact.
///
/// Values are deduplicated per (column, value, source) and sorted, so the output
/// does not depend on aggregation order. Returns empty when `discriminators` is
/// empty: codeweb ships no built-in discriminator column.
fn discriminator_values(
    store: &GraphStore,
    routines: &[NodeIndex],
    enum_mappings: &[EnumMapping],
    enum_provenance: &HashMap<EnumMapping, SourceLocation>,
    discriminators: &[String],
    base: &Path,
) -> Vec<DiscriminatorValue> {
    let mut out: Vec<DiscriminatorValue> = Vec::new();
    if discriminators.is_empty() {
        return out;
    }
    let mut seen: HashSet<(String, String, String)> = HashSet::new();

    for mapping in enum_mappings {
        let Some(column) = discriminator_for(discriminators, &mapping.column) else {
            continue;
        };
        let provenance = enum_provenance
            .get(mapping)
            .map(|location| provenance_of(location, base))
            .unwrap_or_else(|| Provenance {
                file: String::new(),
                line: 0,
            });
        for (key, _result) in &mapping.values {
            for value in filter_value_strings(key) {
                push_discriminator_value(
                    &mut out,
                    &mut seen,
                    DiscriminatorValue {
                        column: column.clone(),
                        value,
                        source: "cursor_decode".to_string(),
                        trigger: None,
                        provenance: provenance.clone(),
                        confidence: HintConfidence::Medium,
                    },
                );
            }
        }
    }

    for &routine in routines {
        let key = NodeKey::from_node(&store.graph()[routine]).to_string();
        let Some(predicates) = store.procedure_predicates.get(&key) else {
            continue;
        };
        let file = routine_file(store.graph(), routine, base);
        for predicate in predicates {
            let Some(table) = &predicate.table_predicate else {
                continue;
            };
            let origin_lower = predicate.origin.to_lowercase();
            let origin_match = discriminators
                .iter()
                .find(|d| origin_lower.contains(&d.to_lowercase()));
            for clause in &table.clauses {
                // Only the enumeration shapes: `=` and `IN`. A `<>` value is
                // something to avoid, not a value that selects a branch.
                if !matches!(clause.op, FilterOperator::Eq | FilterOperator::In) {
                    continue;
                }
                let column = discriminator_for(discriminators, &clause.column).or(origin_match);
                let Some(column) = column else {
                    continue;
                };
                let confidence = if matches!(clause.op, FilterOperator::Eq) {
                    HintConfidence::High
                } else {
                    HintConfidence::Medium
                };
                for value in filter_value_strings(&clause.value) {
                    push_discriminator_value(
                        &mut out,
                        &mut seen,
                        DiscriminatorValue {
                            column: column.clone(),
                            value,
                            source: "branch_condition".to_string(),
                            trigger: Some(predicate.origin.clone()),
                            provenance: Provenance {
                                file: file.clone(),
                                line: predicate.line,
                            },
                            confidence,
                        },
                    );
                }
            }
        }
    }

    out.sort_by(|a, b| (&a.column, &a.value, &a.source).cmp(&(&b.column, &b.value, &b.source)));
    out
}

/// Declared parameters of one routine, from the store's side table (empty when the
/// routine declared none, or when the store predates `routine_parameters`).
fn parameters_of_routine(store: &GraphStore, routine: NodeIndex) -> Vec<RoutineParameter> {
    let key = NodeKey::from_node(&store.graph()[routine]).to_string();
    store
        .routine_parameters
        .get(&key)
        .cloned()
        .unwrap_or_default()
}

/// Children of a package via `Edge::ContainsRoutine` (the same edge `codeweb detail`'s
/// package summary uses).
fn package_children(graph: &CodeGraph, package: NodeIndex) -> Vec<NodeIndex> {
    graph
        .edges_directed(package, Direction::Outgoing)
        .filter(|e| matches!(e.weight(), Edge::ContainsRoutine))
        .map(|e| e.target())
        .collect()
}

/// Per-table operation labels for `routines`, sorted and deduplicated.
///
/// Deliberately separate from [`collect_diagnostics`]: that walk skips edges
/// without a [`ColumnAnalysis`](crate::parser::ColumnAnalysis), while an operation
/// (e.g. `DELETE`, whose column analysis is empty) still has to show up in the
/// seed hints. Keeping the two walks apart also leaves `columns --format json`'s
/// `tables` list byte-identical.
fn table_operations(
    graph: &CodeGraph,
    routines: &[NodeIndex],
    table_filter: Option<&str>,
) -> Vec<TableSeedHint> {
    let filter_lower = table_filter.map(|t| t.to_lowercase());
    let mut ops: BTreeMap<String, BTreeSet<&'static str>> = BTreeMap::new();

    for &routine in routines {
        for dir in [Direction::Outgoing, Direction::Incoming] {
            for edge_ref in graph.edges_directed(routine, dir) {
                let Edge::TableAccess {
                    modes, write_kinds, ..
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
                    Node::Table { name, .. } | Node::View { name, .. } => name,
                    _ => continue,
                };
                if let Some(filt) = &filter_lower {
                    if table_name.to_lowercase() != *filt {
                        continue;
                    }
                }

                let entry = ops.entry(table_name.clone()).or_default();
                if modes.contains(AccessMode::Read) {
                    entry.insert("read");
                }
                for kind in write_kinds {
                    entry.insert(write_kind_label(kind));
                }
            }
        }
    }

    ops.into_iter()
        .map(|(name, ops)| TableSeedHint {
            name,
            ops: ops.into_iter().map(|o| o.to_string()).collect(),
        })
        .collect()
}

/// Wrap aggregated diagnostics with [`Provenance`] and [`HintConfidence`] for
/// `seed-hints`. Kept separate from [`collect_diagnostics`] so the shared
/// aggregation path that feeds `columns --format json` stays untouched.
fn hints_from_diagnostics(
    diag: Diagnostics,
    store: &GraphStore,
    routines: &[NodeIndex],
    discriminators: &[String],
    base: &Path,
) -> (
    Vec<HardFilterHint>,
    Vec<CrossTableEqualityHint>,
    Vec<DiscriminatorValue>,
) {
    let Diagnostics {
        hard_filters,
        cross_table_equalities,
        enum_mappings,
        hard_filter_provenance,
        cross_table_equality_provenance,
        enum_mapping_provenance,
        ..
    } = diag;

    let missing = || Provenance {
        file: String::new(),
        line: 0,
    };

    let hard_filters = hard_filters
        .into_iter()
        .map(|filter| {
            let provenance = hard_filter_provenance
                .get(&filter)
                .map(|location| provenance_of(location, base))
                .unwrap_or_else(missing);
            let confidence = if filter.table.is_some() {
                HintConfidence::High
            } else {
                HintConfidence::Low
            };
            HardFilterHint {
                filter,
                provenance,
                confidence,
            }
        })
        .collect();

    let cross_table_equalities = cross_table_equalities
        .into_iter()
        .map(|equality| {
            let provenance = cross_table_equality_provenance
                .get(&equality)
                .map(|location| provenance_of(location, base))
                .unwrap_or_else(missing);
            CrossTableEqualityHint {
                equality,
                provenance,
                confidence: HintConfidence::High,
            }
        })
        .collect();

    let disc_values = discriminator_values(
        store,
        routines,
        &enum_mappings,
        &enum_mapping_provenance,
        discriminators,
        base,
    );

    (hard_filters, cross_table_equalities, disc_values)
}

/// Seed-data hints for one routine (issue #181). Returns `None` when `routine` is
/// not a `Node::Procedure`/`Node::Function`, mirroring
/// [`column_analysis_of_routine`]'s defensive contract.
pub fn seed_hints_of_routine(
    store: &GraphStore,
    routine: NodeIndex,
    table_filter: Option<&str>,
    discriminators: &[String],
    base: &Path,
) -> Option<SeedHints> {
    let graph = store.graph();
    let (name, package) = match &graph[routine] {
        Node::Procedure { id, .. } | Node::Function { id, .. } => {
            (id.name.clone(), id.package.clone())
        }
        _ => return None,
    };

    let diag = collect_diagnostics(graph, &[routine], table_filter);
    let (hard_filters, cross_table_equalities, disc_values) =
        hints_from_diagnostics(diag, store, &[routine], discriminators, base);

    Some(SeedHints {
        schema_version: 1,
        kind: SEED_HINTS_KIND,
        caveat: SEED_HINTS_CAVEAT,
        procedure: name,
        package,
        parameters: parameters_of_routine(store, routine),
        tables: table_operations(graph, &[routine], table_filter),
        hard_filters,
        cross_table_equalities,
        discriminator_values: disc_values,
    })
}

/// Seed-data hints for every routine a package contains. Returns `None` when
/// `package` is not a `Node::Package`.
pub fn seed_hints_of_package(
    store: &GraphStore,
    package: NodeIndex,
    table_filter: Option<&str>,
    discriminators: &[String],
    base: &Path,
) -> Option<SeedHints> {
    let graph = store.graph();
    let pkg_name = match &graph[package] {
        Node::Package { name, .. } => name.clone(),
        _ => return None,
    };

    let children = package_children(graph, package);
    let diag = collect_diagnostics(graph, &children, table_filter);
    let (hard_filters, cross_table_equalities, disc_values) =
        hints_from_diagnostics(diag, store, &children, discriminators, base);

    let parameters = children
        .iter()
        .flat_map(|&child| parameters_of_routine(store, child))
        .collect();

    Some(SeedHints {
        schema_version: 1,
        kind: SEED_HINTS_KIND,
        caveat: SEED_HINTS_CAVEAT,
        procedure: pkg_name.clone(),
        package: Some(pkg_name),
        parameters,
        tables: table_operations(graph, &children, table_filter),
        hard_filters,
        cross_table_equalities,
        discriminator_values: disc_values,
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
            cross_table_equalities: Vec::new(),
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
