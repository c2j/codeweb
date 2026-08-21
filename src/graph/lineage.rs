use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;
use petgraph::Direction;
use std::collections::HashSet;

use crate::graph::{AccessMode, CodeGraph, ColumnSummary, Edge, Node, WriteKind};
use crate::parser::{ColumnMapping, ColumnSource, MappingKind};

/// Direction of lineage traversal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineageDirection {
    /// Upstream: who writes/defines this node
    Upstream,
    /// Downstream: who consumes/reads this node
    Downstream,
}

/// Role of a source edge: does the source's data flow into the target?
///
/// `Flow` — the source's columns appear among the target's written columns (data-bearing
/// edge). `Reference` — the source is only used for filtering/joining/parameters, its
/// columns do not flow into the target. `Unknown` — the source has no DDL columns to
/// measure against (inferred table, `SELECT *` view, external table), so a missing
/// measurement is not evidence of a reference edge (issue #146 revision 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityRole {
    Flow,
    Reference,
    Unknown,
}

impl EntityRole {
    /// Short tree marker: `[flow]`, `[ref]`, `[ref?]`.
    pub fn label(self) -> &'static str {
        match self {
            EntityRole::Flow => "flow",
            EntityRole::Reference => "ref",
            EntityRole::Unknown => "ref?",
        }
    }

    /// JSON role value.
    pub fn json(self) -> &'static str {
        match self {
            EntityRole::Flow => "flow",
            EntityRole::Reference => "ref",
            EntityRole::Unknown => "unknown",
        }
    }

    /// Sort rank: flow first, then reference, then unknown.
    fn rank(self) -> u8 {
        match self {
            EntityRole::Flow => 0,
            EntityRole::Reference => 1,
            EntityRole::Unknown => 2,
        }
    }
}

/// Column overlap of one source edge: `overlap` columns of `total` target written columns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowCoverage {
    pub overlap: usize,
    pub total: usize,
}

/// L0 flow/reference classification configuration — mirrors `[lineage]` in codeweb.toml.
#[derive(Debug, Clone)]
pub struct LineageConfig {
    /// Absolute minimum column overlap for a flow source.
    pub flow_min_overlap: usize,
    /// Minimum overlap ratio of the target's written columns for a flow source.
    pub flow_min_ratio: f64,
    /// Column names excluded from overlap computation (project-wide same-name noise).
    pub ignore_columns: Vec<String>,
}

impl Default for LineageConfig {
    fn default() -> Self {
        Self {
            flow_min_overlap: 8,
            flow_min_ratio: 0.15,
            ignore_columns: Vec::new(),
        }
    }
}

impl LineageConfig {
    fn is_ignored(&self, column: &str) -> bool {
        self.ignore_columns
            .iter()
            .any(|c| c.eq_ignore_ascii_case(column))
    }
}

/// Which rendering of the lineage tree to produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineageView {
    /// Entity tree with per-edge process attribution (default).
    Tree,
    /// Entity-only tree, processes hidden.
    Entity,
    /// Explicit `source ──[process]──▶ target` relationship lines.
    Relation,
    /// Grouped by connecting process with a transformation summary.
    Grouped,
}

impl std::str::FromStr for LineageView {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "tree" => Ok(LineageView::Tree),
            "entity" => Ok(LineageView::Entity),
            "relation" => Ok(LineageView::Relation),
            "grouped" => Ok(LineageView::Grouped),
            other => Err(format!(
                "unknown view '{}', expected 'tree', 'entity', 'relation' or 'grouped'",
                other
            )),
        }
    }
}

/// Display options for lineage rendering.
#[derive(Debug, Clone, Copy)]
pub struct DisplayOptions {
    pub view: LineageView,
    /// Filter to flow sources only (`--flow-only`).
    pub flow_only: bool,
}

impl DisplayOptions {
    pub fn new(view: LineageView, flow_only: bool) -> Self {
        Self { view, flow_only }
    }
}

/// A step in the lineage chain (routine node with access metadata).
#[derive(Debug, Clone)]
pub struct LineageStep {
    pub routine_idx: NodeIndex,
    pub modes: AccessMode,
    pub write_kinds: Vec<WriteKind>,
    /// For child `via` steps only: flow role of that (source, routine, target) edge.
    /// `None` for a node's own writer/reader steps.
    pub role: Option<EntityRole>,
    pub coverage: Option<FlowCoverage>,
}

/// A child of a lineage node, annotated with the routine(s) that connect parent → child.
///
/// Upstream, a connecting routine READS the child and WRITES the parent; downstream it
/// READS the parent and WRITES the child. Keeping the routine — and the flow role of the
/// edge — on the child (rather than only on the node) is what lets the display attribute
/// each hop to a concrete routine and mark it as flow/reference.
#[derive(Debug, Clone)]
pub struct LineageChild {
    pub node: LineageNode,
    /// Routines (or views, via DependsOn) connecting this node to the child.
    pub via: Vec<LineageStep>,
}

/// A node in the lineage result tree.
#[derive(Debug, Clone)]
pub struct LineageNode {
    pub idx: NodeIndex,
    pub _depth: usize,
    pub steps: Vec<LineageStep>, // How we got here (immediate incoming routine steps)
    pub children: Vec<LineageChild>,
    /// Incoming column-producing write edges (INSERT/UPDATE/...) whose column analysis
    /// carries no mappings — the L0 recall ceiling (issue #146 revision 3).
    pub edges_without_column_data: usize,
}

/// Compute table-level lineage using depth-first traversal with per-path ancestors.
///
/// Algorithm:
/// - UPSTREAM: table → (Incoming Write TableAccess) → routine → (Outgoing Read TableAccess) → table → recurse
/// - DOWNSTREAM: table → (Incoming Read TableAccess) → routine → (Outgoing Write TableAccess) → table → recurse
/// - Views: follow DependsOn edges as if they were transparent
///
/// A per-path ancestor set (rather than one global `visited` set) keeps a table reached
/// through two different routines visible under both parents, while still cutting off a
/// cycle that returns to an ancestor. Every parent→child edge records the connecting
/// routine (the child's `via`) and its flow role/coverage (L0, issue #146).
pub fn lineage_table(
    graph: &CodeGraph,
    start_idx: NodeIndex,
    direction: LineageDirection,
    depth: usize,
    cfg: &LineageConfig,
) -> LineageNode {
    let mut ancestors: HashSet<NodeIndex> = HashSet::new();
    ancestors.insert(start_idx);
    build_table_lineage(graph, start_idx, direction, depth, cfg, &mut ancestors)
}

/// Record a step for `routine` in `steps`, merging with an existing entry for the same
/// routine: one routine may touch a table through several statements (e.g. both INSERT
/// and UPDATE), and each contributes its own write kinds.
fn push_step(
    steps: &mut Vec<LineageStep>,
    routine_idx: NodeIndex,
    modes: AccessMode,
    write_kinds: &[WriteKind],
) {
    if let Some(existing) = steps.iter_mut().find(|s| s.routine_idx == routine_idx) {
        existing.modes |= modes;
        for kind in write_kinds {
            if !existing.write_kinds.contains(kind) {
                existing.write_kinds.push(*kind);
            }
        }
    } else {
        steps.push(LineageStep {
            routine_idx,
            modes,
            write_kinds: write_kinds.to_vec(),
            role: None,
            coverage: None,
        });
    }
}

/// Write operations that (may) produce target columns. A bare DELETE/TRUNCATE produces
/// no columns, so its edge contributes no mappings and never trips the
/// "edge without column data" warning. An empty kind set is treated as column-producing
/// (unknown rather than silently excluded).
fn produces_columns(write_kinds: &HashSet<WriteKind>) -> bool {
    if write_kinds.is_empty() {
        return true;
    }
    write_kinds.iter().any(|k| {
        matches!(
            k,
            WriteKind::Insert
                | WriteKind::InsertSelect
                | WriteKind::Update
                | WriteKind::MergeInsert
                | WriteKind::MergeUpdate
                | WriteKind::MergeDelete
                | WriteKind::SelectInto
        )
    })
}

/// DDL column names (lowercased) of a table/view node; `None` when the node has none
/// (inferred tables, `SELECT *` views, external tables) — the "no evidence" case that
/// classifies as [`EntityRole::Unknown`].
fn entity_ddl_columns(graph: &CodeGraph, idx: NodeIndex) -> Option<Vec<String>> {
    let cols: &Vec<ColumnSummary> = match &graph[idx] {
        Node::Table { columns, .. }
        | Node::View { columns, .. }
        | Node::MaterializedView { columns, .. } => columns,
        _ => return None,
    };
    if cols.is_empty() {
        None
    } else {
        Some(cols.iter().map(|c| c.name.to_lowercase()).collect())
    }
}

/// Classify one source edge `(child → connector → parent)` as flow/reference/unknown
/// (L0, issue #146). The connector is a routine or a view; the classification measures
/// how many of the *written* table's columns trace back to the *origin* table:
///
/// - upstream: origin = child, written = parent (the routine's write edges to the parent);
/// - downstream: origin = parent, written = child (the routine's write edges to the child);
/// - views: DependsOn edges (view → base) carry the view's column mappings, so they are
///   read from the connector to the origin table.
///
/// Returns `(role, coverage, has_column_data)`:
/// - `coverage` = `(overlap, written_total)` when the origin has DDL columns, else `None`;
/// - `has_column_data` = whether the connecting edges carried any column mappings.
pub fn classify_edge_role(
    graph: &CodeGraph,
    direction: LineageDirection,
    child_idx: NodeIndex,
    connector_idx: NodeIndex,
    parent_idx: NodeIndex,
    cfg: &LineageConfig,
) -> (EntityRole, Option<FlowCoverage>, bool) {
    let (origin, written) = match direction {
        LineageDirection::Upstream => (child_idx, parent_idx),
        LineageDirection::Downstream => (parent_idx, child_idx),
    };

    // Origin DDL columns decide the basis: absent → Unknown (no evidence, not a negative).
    let Some(origin_cols) = entity_ddl_columns(graph, origin) else {
        return (EntityRole::Unknown, None, false);
    };
    let origin_set: HashSet<String> = origin_cols
        .into_iter()
        .filter(|c| !cfg.is_ignored(c))
        .collect();

    let mut mappings: Vec<ColumnMapping> = Vec::new();
    let mut has_column_data = false;

    // Routine case: the connector's column-producing write edges to the written table.
    for edge_ref in graph.edges_connecting(connector_idx, written) {
        if let Edge::TableAccess {
            modes,
            write_kinds,
            column_analysis,
            ..
        } = edge_ref.weight()
        {
            if !modes.contains(AccessMode::Write) || !produces_columns(write_kinds) {
                continue;
            }
            if let Some(a) = column_analysis {
                if !a.column_mappings.is_empty() {
                    has_column_data = true;
                    mappings.extend(a.column_mappings.iter().cloned());
                }
            }
        }
    }
    // View case: DependsOn edges (view → base = origin) carry the view column mappings.
    for edge_ref in graph.edges_connecting(connector_idx, origin) {
        if let Edge::DependsOn {
            column_analysis: Some(a),
            ..
        } = edge_ref.weight()
        {
            if !a.column_mappings.is_empty() {
                has_column_data = true;
                mappings.extend(a.column_mappings.iter().cloned());
            }
        }
    }

    let mut source_names: HashSet<String> = HashSet::new();
    for m in &mappings {
        for s in &m.sources {
            if let ColumnSource::Column { column, .. } = s {
                source_names.insert(column.to_lowercase());
            }
        }
    }

    let overlap = source_names
        .iter()
        .filter(|name| origin_set.contains(*name))
        .count();

    // Denominator: the written table's column total (its DDL columns; fall back to the
    // distinct mapping target columns when it has no DDL).
    let written_total = match entity_ddl_columns(graph, written) {
        Some(cols) => cols.len(),
        None => {
            let mut distinct: HashSet<&str> = HashSet::new();
            for m in &mappings {
                distinct.insert(m.target_column.as_str());
            }
            distinct.len()
        }
    };

    // Threshold: `min(written_total, max(abs_min, ceil(written_total × ratio)))` — the
    // absolute floor is capped by the written table's column count so narrow tables are
    // not excluded (issue #146 revision 1).
    let threshold = if written_total == 0 {
        cfg.flow_min_overlap
    } else {
        let ratio = (written_total as f64 * cfg.flow_min_ratio).ceil() as usize;
        written_total.min(cfg.flow_min_overlap.max(ratio))
    };

    let role = if overlap >= threshold {
        EntityRole::Flow
    } else {
        EntityRole::Reference
    };
    (
        role,
        Some(FlowCoverage {
            overlap,
            total: written_total,
        }),
        has_column_data,
    )
}

fn build_table_lineage(
    graph: &CodeGraph,
    idx: NodeIndex,
    direction: LineageDirection,
    depth: usize,
    cfg: &LineageConfig,
    ancestors: &mut HashSet<NodeIndex>,
) -> LineageNode {
    let mut steps: Vec<LineageStep> = Vec::new();
    let mut children: Vec<LineageChild> = Vec::new();
    let mut edges_without_column_data = 0usize;

    if depth == 0 {
        return LineageNode {
            idx,
            _depth: 0,
            steps,
            children,
            edges_without_column_data,
        };
    }

    let wanted = match direction {
        LineageDirection::Upstream => AccessMode::Write,
        LineageDirection::Downstream => AccessMode::Read,
    };
    let hop = match direction {
        LineageDirection::Upstream => AccessMode::Read,
        LineageDirection::Downstream => AccessMode::Write,
    };

    // Routines on the relevant side of this table. All incoming edges of a qualifying
    // routine are merged so the step label shows its full access profile (`[R,W:update]`
    // when it both reads and writes the table, matching `detail`'s CALLERS).
    let mut routine_edges: Vec<(NodeIndex, AccessMode, Vec<WriteKind>)> = Vec::new();
    let mut qualified: HashSet<NodeIndex> = HashSet::new();
    for edge_ref in graph.edges_directed(idx, Direction::Incoming) {
        if let Edge::TableAccess {
            modes,
            write_kinds,
            column_analysis,
            ..
        } = edge_ref.weight()
        {
            if modes.contains(wanted) {
                qualified.insert(edge_ref.source());
            }
            if let Some(entry) = routine_edges
                .iter_mut()
                .find(|(r, _, _)| *r == edge_ref.source())
            {
                entry.1 |= *modes;
                for kind in write_kinds.iter() {
                    if !entry.2.contains(kind) {
                        entry.2.push(*kind);
                    }
                }
            } else {
                routine_edges.push((
                    edge_ref.source(),
                    *modes,
                    write_kinds.iter().copied().collect(),
                ));
            }
            if modes.contains(AccessMode::Write)
                && produces_columns(write_kinds)
                && column_analysis
                    .as_ref()
                    .is_none_or(|a| a.column_mappings.is_empty())
            {
                edges_without_column_data += 1;
            }
        }
    }
    routine_edges.retain(|(r, _, _)| qualified.contains(r));
    routine_edges.sort_by_key(|(r, _, _)| r.index());

    for (routine_idx, routine_modes, routine_write_kinds) in routine_edges {
        // Statement-scoped hop basis (issue #147): only tables touched in the SAME
        // statement as this routine's access of `idx` are connected — a routine that
        // reads table X in one statement and writes `idx` in another no longer presents
        // X as upstream of `idx`. Upstream scopes by the write edge's read_tables;
        // downstream scopes per target (does the write edge for the child also read
        // `idx`?). Old stores without read_tables fall back to all reads/writes.
        let mut statement_tables: HashSet<String> = HashSet::new();
        let mut have_scope = false;
        for edge_ref in graph.edges_connecting(routine_idx, idx) {
            if let Edge::TableAccess {
                modes,
                column_analysis: Some(a),
                ..
            } = edge_ref.weight()
            {
                // Any wanted-side edge carrying read_tables (even an empty set) means the
                // store is statement-aware and the scope applies; only a store with NO
                // read_tables at all (pre-#147) falls back to all reads/writes.
                if modes.contains(wanted) {
                    if let Some(rt) = &a.read_tables {
                        have_scope = true;
                        statement_tables.extend(rt.iter().map(|t| t.to_lowercase()));
                    }
                }
            }
        }
        let restrict_hops = have_scope;
        let idx_name = entity_name(graph, idx).to_lowercase();

        // The other side of the hop: for upstream, the tables this routine reads while
        // writing `idx`; for downstream, the tables it writes while reading `idx`. The
        // child-edge modes/write kinds are what the edge label shows.
        let mut hop_targets: Vec<(NodeIndex, AccessMode, Vec<WriteKind>)> = Vec::new();
        for edge_ref in graph.edges_directed(routine_idx, Direction::Outgoing) {
            if let Edge::TableAccess {
                modes,
                write_kinds,
                column_analysis,
                ..
            } = edge_ref.weight()
            {
                if modes.contains(hop) {
                    if restrict_hops {
                        let allowed = match direction {
                            LineageDirection::Upstream => statement_tables
                                .contains(&entity_name(graph, edge_ref.target()).to_lowercase()),
                            // Downstream: the child is downstream only when the write
                            // statement for it also reads `idx` — including through a
                            // cursor/record chain, which puts `idx` in the write edge's
                            // resolved read_tables.
                            LineageDirection::Downstream => column_analysis
                                .as_ref()
                                .and_then(|a| a.read_tables.as_ref())
                                .is_some_and(|rt| {
                                    rt.iter().any(|t| t.eq_ignore_ascii_case(&idx_name))
                                }),
                        };
                        if !allowed {
                            continue;
                        }
                    }
                    hop_targets.push((
                        edge_ref.target(),
                        *modes,
                        write_kinds.iter().copied().collect(),
                    ));
                }
            }
        }
        if hop_targets.is_empty() {
            // The routine writes (upstream) or reads (downstream) this table but touches
            // no counterpart table — INSERT ... VALUES, a bare DELETE, or a SELECT-only
            // read. Record it so the write/read is not silently invisible.
            push_step(&mut steps, routine_idx, routine_modes, &routine_write_kinds);
            continue;
        }
        for (target, target_modes, target_write_kinds) in hop_targets {
            push_step(&mut steps, routine_idx, routine_modes, &routine_write_kinds);
            let (role, coverage, _) =
                classify_edge_role(graph, direction, target, routine_idx, idx, cfg);
            let via_step = LineageStep {
                routine_idx,
                modes: target_modes,
                write_kinds: target_write_kinds,
                role: Some(role),
                coverage,
            };
            match children.iter_mut().find(|c| c.node.idx == target) {
                // A table reached through two different routines is a child under both —
                // each connecting routine is recorded on the same child's `via`.
                Some(child) => {
                    if !child.via.iter().any(|s| s.routine_idx == routine_idx) {
                        child.via.push(via_step);
                    }
                }
                None if !ancestors.contains(&target) => {
                    ancestors.insert(target);
                    children.push(LineageChild {
                        node: build_table_lineage(
                            graph,
                            target,
                            direction,
                            depth - 1,
                            cfg,
                            ancestors,
                        ),
                        via: vec![via_step],
                    });
                    ancestors.remove(&target);
                }
                None => {}
            }
        }
    }

    let depends_dir = match direction {
        LineageDirection::Upstream => Direction::Outgoing,
        LineageDirection::Downstream => Direction::Incoming,
    };
    for edge_ref in graph.edges_directed(idx, depends_dir) {
        if matches!(edge_ref.weight(), Edge::DependsOn { .. }) {
            let other = match direction {
                LineageDirection::Upstream => edge_ref.target(),
                LineageDirection::Downstream => edge_ref.source(),
            };
            let step_routine = match direction {
                LineageDirection::Upstream => idx,
                LineageDirection::Downstream => other,
            };
            push_step(&mut steps, step_routine, AccessMode::Read, &[]);
            let (role, coverage, _) =
                classify_edge_role(graph, direction, other, step_routine, idx, cfg);
            let via_step = LineageStep {
                routine_idx: step_routine,
                modes: AccessMode::Read,
                write_kinds: Vec::new(),
                role: Some(role),
                coverage,
            };
            match children.iter_mut().find(|c| c.node.idx == other) {
                Some(child) => {
                    if !child.via.iter().any(|s| s.routine_idx == step_routine) {
                        child.via.push(via_step);
                    }
                }
                None if !ancestors.contains(&other) => {
                    ancestors.insert(other);
                    children.push(LineageChild {
                        node: build_table_lineage(
                            graph,
                            other,
                            direction,
                            depth - 1,
                            cfg,
                            ancestors,
                        ),
                        via: vec![via_step],
                    });
                    ancestors.remove(&other);
                }
                None => {}
            }
        }
    }

    LineageNode {
        idx,
        _depth: 0,
        steps,
        children,
        edges_without_column_data,
    }
}

// ── Labels ────────────────────────────────────────────────────────────────────

/// One routine label, detail-aligned: `proc:name [R,W:update]`.
fn step_label(step: &LineageStep, graph: &CodeGraph) -> String {
    let key = crate::graph::key::NodeKey::from_node(&graph[step.routine_idx]).to_string();
    let wk: HashSet<WriteKind> = step.write_kinds.iter().copied().collect();
    match crate::graph::access_mode_label(step.modes, &wk) {
        Some(mode) => format!("{} [{}]", key, mode),
        None => key,
    }
}

fn step_labels(node: &LineageNode, graph: &CodeGraph) -> Vec<String> {
    node.steps
        .iter()
        .filter(|step| step.routine_idx != node.idx)
        .map(|step| step_label(step, graph))
        .collect()
}

/// Aggregated role of a child across its connecting routines: flow wins, then reference,
/// then unknown (issue #146 revision 4 — max semantics, never summed).
fn child_role(child: &LineageChild) -> EntityRole {
    let mut role = EntityRole::Unknown;
    for step in &child.via {
        if let Some(r) = step.role {
            if r == EntityRole::Flow {
                return EntityRole::Flow;
            }
            if r == EntityRole::Reference {
                role = EntityRole::Reference;
            }
        }
    }
    role
}

/// Role tag padded to a fixed width so tree lines align: `[flow] `, `[ref]  `, `[ref?] `.
fn role_tag(role: EntityRole) -> String {
    format!("{:<7}", format!("[{}]", role.label()))
}

/// Aggregated coverage of a child: the max overlap among its connecting edges.
fn child_coverage(child: &LineageChild) -> Option<FlowCoverage> {
    child
        .via
        .iter()
        .filter_map(|s| s.coverage)
        .max_by_key(|c| c.overlap)
}

/// Coverage label, role-aware: `(flow 10/12)` when the source's columns flow into the
/// target, `(overlap 5/29)` when they merely share names (reference/unknown) — so the
/// number is never misread as a flow count for a reference source.
fn coverage_label(role: EntityRole, cov: FlowCoverage) -> String {
    let word = match role {
        EntityRole::Flow => "flow",
        EntityRole::Reference | EntityRole::Unknown => "overlap",
    };
    format!("({} {}/{})", word, cov.overlap, cov.total)
}

/// A table with no in-code writers is a terminal input (data loaded externally — e.g.
/// parameters, exchange feeds, staging loaded by dynamic SQL). Marking it reconciles the
/// "no writers found" hint of its own upstream query with its appearance as a source
/// elsewhere. Views are excluded: they are derived from base tables, not external input.
fn is_terminal_input(graph: &CodeGraph, idx: NodeIndex) -> bool {
    if !matches!(&graph[idx], Node::Table { .. }) {
        return false;
    }
    !graph.edges_directed(idx, Direction::Incoming).any(|e| {
        matches!(
            e.weight(),
            Edge::TableAccess { modes, .. } if modes.contains(AccessMode::Write)
        )
    })
}

/// `[external]` marker for terminal-input children, empty otherwise.
fn terminal_marker(graph: &CodeGraph, idx: NodeIndex) -> &'static str {
    if is_terminal_input(graph, idx) {
        "  [external]"
    } else {
        ""
    }
}

/// Whether the child's role rests on DDL columns or on missing evidence.
fn child_coverage_basis(graph: &CodeGraph, child: &LineageChild) -> &'static str {
    if entity_ddl_columns(graph, child.node.idx).is_some() {
        "ddl_columns"
    } else {
        "no_ddl_columns"
    }
}

fn sort_children(children: &mut [&LineageChild]) {
    children.sort_by(|a, b| {
        child_role(a)
            .rank()
            .cmp(&child_role(b).rank())
            .then_with(|| {
                child_coverage(b)
                    .map_or(0, |c| c.overlap)
                    .cmp(&child_coverage(a).map_or(0, |c| c.overlap))
            })
    });
}

/// Root summary line: `← 19 upstream entities · flow 4 · reference 15`.
fn summary_line(
    node: &LineageNode,
    direction: LineageDirection,
    opts: &DisplayOptions,
) -> Option<String> {
    if node.children.is_empty() {
        return None;
    }
    let mut flow = 0usize;
    let mut reference = 0usize;
    let mut unknown = 0usize;
    for child in &node.children {
        if opts.flow_only && child_role(child) != EntityRole::Flow {
            continue;
        }
        match child_role(child) {
            EntityRole::Flow => flow += 1,
            EntityRole::Reference => reference += 1,
            EntityRole::Unknown => unknown += 1,
        }
    }
    let entity_word = |n: usize| match direction {
        LineageDirection::Upstream => format!(
            "{} upstream {}",
            n,
            if n == 1 { "entity" } else { "entities" }
        ),
        LineageDirection::Downstream => format!(
            "{} downstream {}",
            n,
            if n == 1 { "entity" } else { "entities" }
        ),
    };
    let mut parts = vec![
        entity_word(flow + reference + unknown),
        format!("flow {}", flow),
        format!("reference {}", reference),
    ];
    if unknown > 0 {
        parts.push(format!("unknown {}", unknown));
    }
    // Under --flow-only the hidden reference/unknown sources are still counted, so the
    // reader can tell "no reference sources" from "reference sources filtered out".
    if opts.flow_only {
        let hidden = node
            .children
            .iter()
            .filter(|c| child_role(c) != EntityRole::Flow)
            .count();
        if hidden > 0 {
            parts.push(format!("{} reference filtered", hidden));
        }
    }
    if node.edges_without_column_data > 0 {
        parts.push(format!(
            "⚠ {} edges without column data",
            node.edges_without_column_data
        ));
    }
    Some(parts.join(" · "))
}

fn node_key(graph: &CodeGraph, idx: NodeIndex) -> String {
    crate::graph::key::NodeKey::from_node(&graph[idx]).to_string()
}

/// Bare entity name (the part after `type:`) for compact group lists.
fn entity_name(graph: &CodeGraph, idx: NodeIndex) -> String {
    match &graph[idx] {
        Node::Table { name, .. }
        | Node::View { name, .. }
        | Node::MaterializedView { name, .. } => name.clone(),
        _ => node_key(graph, idx),
    }
}

// ── Tree views (--view) ───────────────────────────────────────────────────────

/// Format lineage node as a tree string, honoring `--view` and `--flow-only`.
pub fn format_lineage_tree(
    node: &LineageNode,
    graph: &CodeGraph,
    direction: LineageDirection,
    indent: usize,
    opts: &DisplayOptions,
) -> String {
    match opts.view {
        LineageView::Tree => format_tree_view(node, graph, direction, indent, opts),
        LineageView::Entity => format_entity_view(node, graph, direction, indent, opts),
        LineageView::Relation => format_relation_view(node, graph, direction, indent, opts),
        LineageView::Grouped => format_grouped_view(node, graph, direction, indent, opts),
    }
}

/// `--view tree` (default): entity tree with per-edge process attribution and
/// flow/reference role markers.
fn format_tree_view(
    node: &LineageNode,
    graph: &CodeGraph,
    direction: LineageDirection,
    indent: usize,
    opts: &DisplayOptions,
) -> String {
    let mut out = String::new();
    let indent_str = "  ".repeat(indent);
    out.push_str(&format!("{}{}", indent_str, node_key(graph, node.idx)));

    // Node label, detail-aligned: `[proc:A [W:insert_select,insert,delete], ...]`.
    let steps = step_labels(node, graph);
    if !steps.is_empty() {
        out.push_str(&format!("  [{}]", steps.join(", ")));
    }
    if indent == 0 {
        if let Some(summary) = summary_line(node, direction, opts) {
            let arrow = match direction {
                LineageDirection::Upstream => '←',
                LineageDirection::Downstream => '→',
            };
            out.push_str(&format!("  {} {}", arrow, summary));
        }
    }
    out.push('\n');

    let mut children: Vec<&LineageChild> = node.children.iter().collect();
    if opts.flow_only {
        children.retain(|c| child_role(c) == EntityRole::Flow);
    }
    sort_children(&mut children);
    for child in children {
        out.push_str(&format_tree_child_line(
            child,
            graph,
            direction,
            indent + 1,
            opts,
        ));
    }
    out
}

fn format_tree_child_line(
    child: &LineageChild,
    graph: &CodeGraph,
    direction: LineageDirection,
    indent: usize,
    opts: &DisplayOptions,
) -> String {
    let indent_str = "  ".repeat(indent);
    let role = child_role(child);
    let mut line = format!(
        "{}{} {}",
        indent_str,
        role_tag(role),
        node_key(graph, child.node.idx)
    );

    // Edge attribution: which routine(s) connect the parent to this child, in the
    // direction of data flow (upstream the child feeds the parent, downstream the parent
    // feeds the child).
    let via: Vec<String> = child
        .via
        .iter()
        .filter(|step| step.routine_idx != child.node.idx)
        .map(|step| step_label(step, graph))
        .collect();
    if !via.is_empty() {
        let arrow = match direction {
            LineageDirection::Upstream => '←',
            LineageDirection::Downstream => '→',
        };
        line.push_str(&format!("  {} {}", arrow, via.join(", ")));
    }

    // The child's own incoming steps (its writers upstream / readers downstream).
    let steps = step_labels(&child.node, graph);
    if !steps.is_empty() {
        line.push_str(&format!("  [{}]", steps.join(", ")));
    }

    if let Some(cov) = child_coverage(child) {
        line.push_str(&format!("  {}", coverage_label(role, cov)));
    }
    line.push_str(terminal_marker(graph, child.node.idx));
    line.push('\n');

    let mut sub: Vec<&LineageChild> = child.node.children.iter().collect();
    if opts.flow_only {
        sub.retain(|c| child_role(c) == EntityRole::Flow);
    }
    sort_children(&mut sub);
    for grandchild in sub {
        line.push_str(&format_tree_child_line(
            grandchild,
            graph,
            direction,
            indent + 1,
            opts,
        ));
    }
    line
}

/// `--view entity`: entity relationships only, no process information.
fn format_entity_view(
    node: &LineageNode,
    graph: &CodeGraph,
    direction: LineageDirection,
    indent: usize,
    opts: &DisplayOptions,
) -> String {
    format_entity_view_inner(node, graph, direction, indent, opts, None)
}

/// `--view entity`: entity relationships only, no process information. Each node's header
/// line carries the role tag and coverage of the edge that reached it from its parent.
fn format_entity_view_inner(
    node: &LineageNode,
    graph: &CodeGraph,
    direction: LineageDirection,
    indent: usize,
    opts: &DisplayOptions,
    prefix: Option<(EntityRole, Option<FlowCoverage>)>,
) -> String {
    let mut out = String::new();
    let pad = "  ".repeat(indent);
    match prefix {
        Some((role, cov)) => {
            out.push_str(&format!(
                "{}{} {}",
                pad,
                role_tag(role),
                node_key(graph, node.idx)
            ));
            if let Some(c) = cov {
                out.push_str(&format!("  {}", coverage_label(role, c)));
            }
            out.push_str(terminal_marker(graph, node.idx));
        }
        None => {
            out.push_str(&format!("{}{}", pad, node_key(graph, node.idx)));
            if indent == 0 {
                if let Some(summary) = summary_line(node, direction, opts) {
                    let arrow = match direction {
                        LineageDirection::Upstream => '←',
                        LineageDirection::Downstream => '→',
                    };
                    out.push_str(&format!("  {} {}", arrow, summary));
                }
            }
        }
    }
    out.push('\n');

    let mut children: Vec<&LineageChild> = node.children.iter().collect();
    if opts.flow_only {
        children.retain(|c| child_role(c) == EntityRole::Flow);
    }
    sort_children(&mut children);
    for child in children {
        out.push_str(&format_entity_view_inner(
            &child.node,
            graph,
            direction,
            indent + 1,
            opts,
            Some((child_role(child), child_coverage(child))),
        ));
    }
    out
}

/// `--view relation`: each line is a self-contained `source ──[process]──▶ target`
/// relationship; only the root prints a header line.
fn format_relation_view(
    node: &LineageNode,
    graph: &CodeGraph,
    direction: LineageDirection,
    indent: usize,
    opts: &DisplayOptions,
) -> String {
    format_relation_view_inner(node, graph, direction, indent, opts, true)
}

fn format_relation_view_inner(
    node: &LineageNode,
    graph: &CodeGraph,
    direction: LineageDirection,
    indent: usize,
    opts: &DisplayOptions,
    is_root: bool,
) -> String {
    let mut out = String::new();
    if is_root {
        out.push_str(&format!(
            "{}{}",
            "  ".repeat(indent),
            node_key(graph, node.idx)
        ));
        if let Some(summary) = summary_line(node, direction, opts) {
            let arrow = match direction {
                LineageDirection::Upstream => '←',
                LineageDirection::Downstream => '→',
            };
            out.push_str(&format!("  {} {}", arrow, summary));
        }
        out.push('\n');
    }

    let mut children: Vec<&LineageChild> = node.children.iter().collect();
    if opts.flow_only {
        children.retain(|c| child_role(c) == EntityRole::Flow);
    }
    sort_children(&mut children);
    for child in children {
        let role = child_role(child);
        // Data flows toward the receiving entity: the child for upstream, the node for
        // downstream. The arrow always points at the receiver.
        let (from, to) = match direction {
            LineageDirection::Upstream => (child.node.idx, node.idx),
            LineageDirection::Downstream => (node.idx, child.node.idx),
        };
        let via: Vec<String> = child
            .via
            .iter()
            .filter(|step| step.routine_idx != child.node.idx)
            .map(|step| step_label(step, graph))
            .collect();
        let mut line = format!(
            "{}{} ──[{}]──▶ {}",
            "  ".repeat(indent + 1),
            node_key(graph, from),
            via.join(", "),
            node_key(graph, to)
        );
        line.push_str(&format!("  [{}", role.label()));
        if let Some(cov) = child_coverage(child) {
            // `[flow 25/29]` for flow; `[ref overlap 5/29]` for reference — the role word
            // already says "flow", so only the reference side gains the "overlap" qualifier.
            match role {
                EntityRole::Flow => line.push_str(&format!(" {}/{}", cov.overlap, cov.total)),
                EntityRole::Reference | EntityRole::Unknown => {
                    line.push_str(&format!(" overlap {}/{}", cov.overlap, cov.total));
                }
            }
        }
        line.push(']');
        line.push_str(terminal_marker(graph, child.node.idx));
        line.push('\n');
        out.push_str(&line);
        out.push_str(&format_relation_view_inner(
            &child.node,
            graph,
            direction,
            indent + 1,
            opts,
            false,
        ));
    }
    out
}

/// Transformation summary of one routine's write edges into `node`, counted by mapping
/// kind (issue #146 revision 6 — no fragile function-name extraction in v1).
fn transform_summary(graph: &CodeGraph, routine: NodeIndex, node_idx: NodeIndex) -> Option<String> {
    let (mut aggregate, mut derived, mut direct) = (0usize, 0usize, 0usize);
    let mut any = false;
    for edge_ref in graph.edges_connecting(routine, node_idx) {
        if let Edge::TableAccess {
            modes,
            column_analysis: Some(a),
            ..
        } = edge_ref.weight()
        {
            if modes.contains(AccessMode::Write) && !a.column_mappings.is_empty() {
                any = true;
                for m in &a.column_mappings {
                    match m.kind {
                        MappingKind::Aggregated { .. } => aggregate += 1,
                        MappingKind::Derived => derived += 1,
                        MappingKind::Direct => direct += 1,
                    }
                }
            }
        }
    }
    if !any {
        return None;
    }
    Some(format!(
        "transform: aggregate {} · derived {} · direct {}",
        aggregate, derived, direct
    ))
}

/// `--view grouped`: children grouped by the connecting process, with a per-process
/// transformation summary and flow/reference sub-lists.
fn format_grouped_view(
    node: &LineageNode,
    graph: &CodeGraph,
    direction: LineageDirection,
    indent: usize,
    opts: &DisplayOptions,
) -> String {
    let mut out = String::new();
    let indent_str = "  ".repeat(indent);
    out.push_str(&format!("{}{}", indent_str, node_key(graph, node.idx)));
    let steps = step_labels(node, graph);
    if !steps.is_empty() {
        out.push_str(&format!("  [{}]", steps.join(", ")));
    }
    if indent == 0 {
        if let Some(summary) = summary_line(node, direction, opts) {
            let arrow = match direction {
                LineageDirection::Upstream => '←',
                LineageDirection::Downstream => '→',
            };
            out.push_str(&format!("  {} {}", arrow, summary));
        }
    }
    out.push('\n');

    // Group children by connecting routine (a child with two connecting routines appears
    // in both groups, independently attributed). The group keeps one via step per routine
    // so its header can show the detail-aligned mode bracket.
    let mut groups: Vec<(NodeIndex, LineageStep, Vec<&LineageChild>)> = Vec::new();
    for child in &node.children {
        if opts.flow_only && child_role(child) != EntityRole::Flow {
            continue;
        }
        for step in &child.via {
            if step.routine_idx == child.node.idx {
                continue;
            }
            let routine = step.routine_idx;
            if let Some(g) = groups.iter_mut().find(|(r, _, _)| *r == routine) {
                g.2.push(child);
            } else {
                groups.push((routine, step.clone(), vec![child]));
            }
        }
    }
    groups.sort_by_key(|(r, _, _)| r.index());

    for (routine, _via_step, members) in groups {
        // Header shows the routine's own access profile on the node being expanded
        // (upstream: how it writes the parent; downstream: how it reads it).
        let routine_label = node
            .steps
            .iter()
            .find(|s| s.routine_idx == routine)
            .map(|s| step_label(s, graph))
            .unwrap_or_else(|| node_key(graph, routine));
        out.push_str(&format!(
            "{}── {}   feeds {}\n",
            "  ".repeat(indent + 1),
            routine_label,
            members.len()
        ));
        if let Some(transform) = transform_summary(graph, routine, node.idx) {
            out.push_str(&format!("{}   {}\n", "  ".repeat(indent + 2), transform));
        }
        let mut flow: Vec<&LineageChild> = members
            .iter()
            .copied()
            .filter(|c| child_role(c) == EntityRole::Flow)
            .collect();
        let mut reference: Vec<&LineageChild> = members
            .iter()
            .copied()
            .filter(|c| child_role(c) == EntityRole::Reference)
            .collect();
        let mut unknown: Vec<&LineageChild> = members
            .iter()
            .copied()
            .filter(|c| child_role(c) == EntityRole::Unknown)
            .collect();
        sort_children(&mut flow);
        sort_children(&mut reference);
        sort_children(&mut unknown);
        for (label, list) in [("flow", &flow), ("ref", &reference), ("ref?", &unknown)] {
            if list.is_empty() {
                continue;
            }
            let items: Vec<String> = list
                .iter()
                .map(|c| {
                    let name = entity_name(graph, c.node.idx);
                    match child_coverage(c) {
                        Some(cov) => format!("{} ({}/{})", name, cov.overlap, cov.total),
                        None => name,
                    }
                })
                .collect();
            out.push_str(&format!(
                "{}{:<5} {}\n",
                "  ".repeat(indent + 2),
                label,
                items.join(", ")
            ));
        }
        // Recurse into each member's own subtree (grouped view at the next level),
        // only when the member has children of its own — leaf members were already
        // listed inline and need no header.
        for member in members {
            if member.node.children.is_empty() {
                continue;
            }
            out.push_str(&format_grouped_view(
                &member.node,
                graph,
                direction,
                indent + 2,
                opts,
            ));
        }
    }
    out
}

// ── Graph exports (--format dot / mermaid) ────────────────────────────────────

/// One entity-level edge of the lineage tree, in data-flow direction
/// (source → receiver).
struct EntityEdge {
    from: NodeIndex,
    to: NodeIndex,
    via: Vec<LineageStep>,
    role: EntityRole,
}

fn collect_entity_edges(
    node: &LineageNode,
    direction: LineageDirection,
    opts: &DisplayOptions,
    out: &mut Vec<EntityEdge>,
) {
    for child in &node.children {
        if opts.flow_only && child_role(child) != EntityRole::Flow {
            continue;
        }
        let (from, to) = match direction {
            LineageDirection::Upstream => (child.node.idx, node.idx),
            LineageDirection::Downstream => (node.idx, child.node.idx),
        };
        out.push(EntityEdge {
            from,
            to,
            via: child.via.clone(),
            role: child_role(child),
        });
        collect_entity_edges(&child.node, direction, opts, out);
    }
}

/// DOT export of the lineage subgraph: entity nodes, entity→entity edges labelled with
/// the connecting routine(s). Flow edges are thick solid, reference edges thin dashed,
/// unknown edges dotted.
pub fn format_lineage_dot(
    roots: &[(&LineageNode, LineageDirection)],
    graph: &CodeGraph,
    opts: &DisplayOptions,
) -> String {
    let mut edges: Vec<EntityEdge> = Vec::new();
    for (node, direction) in roots {
        collect_entity_edges(node, *direction, opts, &mut edges);
    }

    let mut out = String::from("digraph lineage {\n");
    let mut seen: HashSet<(usize, usize)> = HashSet::new();
    for e in &edges {
        let from = node_key(graph, e.from);
        let to = node_key(graph, e.to);
        let via: Vec<String> = e
            .via
            .iter()
            .filter(|s| s.routine_idx != e.from && s.routine_idx != e.to)
            .map(|s| step_label(s, graph))
            .collect();
        let label = if via.is_empty() {
            String::new()
        } else {
            format!("label=\"{}\"", via.join(", "))
        };
        let style = match e.role {
            EntityRole::Flow => "penwidth=3",
            EntityRole::Reference => "style=dashed,penwidth=1",
            EntityRole::Unknown => "style=dotted,penwidth=1",
        };
        let key = format!("{} -> {}", from, to);
        if seen.insert((e.from.index(), e.to.index())) {
            out.push_str(&format!(
                "  \"{}\" -> \"{}\" [{}{}];\n",
                from,
                to,
                if label.is_empty() {
                    String::new()
                } else {
                    format!("{},", label)
                },
                style
            ));
        }
        let _ = key;
    }
    out.push_str("}\n");
    out
}

/// Mermaid flowchart export: `==>` for flow edges, `-.->` for reference, `-..->` is not
/// valid — unknown uses `-.->` with a dotted style note.
pub fn format_lineage_mermaid(
    roots: &[(&LineageNode, LineageDirection)],
    graph: &CodeGraph,
    opts: &DisplayOptions,
) -> String {
    let mut edges: Vec<EntityEdge> = Vec::new();
    for (node, direction) in roots {
        collect_entity_edges(node, *direction, opts, &mut edges);
    }

    let mut out = String::from("flowchart LR\n");
    for e in &edges {
        let from = node_key(graph, e.from);
        let to = node_key(graph, e.to);
        let via: Vec<String> = e
            .via
            .iter()
            .filter(|s| s.routine_idx != e.from && s.routine_idx != e.to)
            .map(|s| step_label(s, graph))
            .collect();
        let label = via.join(", ");
        let connector = match e.role {
            EntityRole::Flow => "==>",
            EntityRole::Reference => "-.->",
            EntityRole::Unknown => "-.->",
        };
        if label.is_empty() {
            out.push_str(&format!("  \"{}\" {} \"{}\"\n", from, connector, to));
        } else {
            out.push_str(&format!(
                "  \"{}\" {}|{}| \"{}\"\n",
                from, connector, label, to
            ));
        }
    }
    out
}

/// Format lineage node as JSON (canonical tree shape regardless of `--view`, enriched
/// with role/coverage fields; issue #146 revisions 2/3/5).
pub fn format_lineage_json(
    node: &LineageNode,
    graph: &CodeGraph,
    opts: &DisplayOptions,
) -> serde_json::Value {
    // Match the tree view's ordering: flow first (by coverage), then reference, then
    // unknown — so JSON consumers see the same role ordering without re-sorting.
    let mut children: Vec<&LineageChild> = node
        .children
        .iter()
        .filter(|c| !opts.flow_only || child_role(c) == EntityRole::Flow)
        .collect();
    sort_children(&mut children);
    let children_json: Vec<serde_json::Value> = children
        .into_iter()
        .map(|child| {
            let mut child_json = format_lineage_json(&child.node, graph, opts);
            // Edge attribution: routines connecting parent → this child.
            let connected_by: Vec<String> = child
                .via
                .iter()
                .filter(|step| step.routine_idx != child.node.idx)
                .map(|step| step_label(step, graph))
                .collect();
            if !connected_by.is_empty() {
                child_json["connected_by"] = serde_json::json!(connected_by);
            }
            child_json["role"] = serde_json::json!(child_role(child).json());
            child_json["coverage_basis"] = serde_json::json!(child_coverage_basis(graph, child));
            child_json["terminal_input"] =
                serde_json::json!(is_terminal_input(graph, child.node.idx));
            if let Some(cov) = child_coverage(child) {
                child_json["flow_overlap"] = serde_json::json!(cov.overlap);
                child_json["flow_total"] = serde_json::json!(cov.total);
            }
            child_json
        })
        .collect();

    serde_json::json!({
        "node": node_key(graph, node.idx),
        "via": step_labels(node, graph),
        "edges_without_column_data": node.edges_without_column_data,
        "children": children_json,
    })
}

// ── Column-level lineage (#136) ──────────────────────────────────────────────

/// One column of one table or view, and where its value comes from (or goes to).
#[derive(Debug, Clone)]
pub struct ColumnLineageNode {
    pub table: String,
    pub column: String,
    pub steps: Vec<ColumnLineageStep>,
}

/// A single hop of column-level data flow.
#[derive(Debug, Clone)]
pub struct ColumnLineageStep {
    /// The routine performing the write (upstream) or the read (downstream).
    pub via: NodeIndex,
    pub kind: MappingKind,
    /// Source expression text, when the value is not a plain column copy.
    pub expression: Option<String>,
    /// The column, literal, variable, or dynamic marker on the other side.
    pub source: ColumnSource,
    /// Set only when `source` is a column that could be resolved and walked further.
    /// A literal, an unresolved variable, or dynamic SQL ends the chain here.
    pub next: Option<ColumnLineageNode>,
}

/// Collect the column mappings a routine declares, deduplicated.
///
/// The same per-statement `ColumnAnalysis` is attached to every edge of that statement,
/// so `INSERT INTO a SELECT FROM b` carries identical mappings on both the write edge to
/// `a` and the read edge from `b`. Gathering across all of a routine's edges and then
/// deduplicating is what keeps a self-referencing `UPDATE` from reporting every mapping
/// twice.
fn mappings_of_routine(graph: &CodeGraph, routine: NodeIndex) -> Vec<ColumnMapping> {
    let mut out: Vec<ColumnMapping> = Vec::new();
    let mut seen: HashSet<ColumnMapping> = HashSet::new();
    for dir in [Direction::Outgoing, Direction::Incoming] {
        for edge_ref in graph.edges_directed(routine, dir) {
            if let Edge::TableAccess {
                column_analysis: Some(analysis),
                ..
            } = edge_ref.weight()
            {
                for m in &analysis.column_mappings {
                    if seen.insert(m.clone()) {
                        out.push(m.clone());
                    }
                }
            }
        }
    }
    out
}

/// Find the Table or View node named `name`.
///
/// A schema-qualified name (`schema.table`) matches schema and table together. A bare
/// name resolves only when a single node carries it — with two schemas each holding a
/// table of the same name, returning the first would report one schema's pipeline as the
/// other's, so an ambiguous bare name resolves to `None`.
fn find_table_node(graph: &CodeGraph, name: &str) -> Option<NodeIndex> {
    let name_of = |idx: &NodeIndex| match &graph[*idx] {
        crate::graph::Node::Table { schema, name, .. }
        | crate::graph::Node::View { schema, name, .. } => Some((schema.as_deref(), name.as_str())),
        _ => None,
    };

    if let Some((schema, table)) = name.rsplit_once('.') {
        if !schema.is_empty() {
            return graph.node_indices().find(|idx| {
                name_of(idx)
                    .is_some_and(|(s, n)| n.eq_ignore_ascii_case(table) && s == Some(schema))
            });
        }
    }

    let mut matches = graph
        .node_indices()
        .filter(|idx| name_of(idx).is_some_and(|(_, n)| n.eq_ignore_ascii_case(name)));
    let first = matches.next()?;
    if matches.next().is_some() {
        return None;
    }
    Some(first)
}

fn eq(a: &str, b: &str) -> bool {
    a.eq_ignore_ascii_case(b)
}

/// Trace one column's data flow.
///
/// Upstream answers "what feeds this column": for each routine writing the table, the
/// mappings targeting this column, then recursively each of their source columns.
/// Downstream answers "where does this column go": for each routine reading the table,
/// the mappings that consume this column, then recursively their target columns.
pub fn lineage_column(
    graph: &CodeGraph,
    table: &str,
    column: &str,
    direction: LineageDirection,
    depth: usize,
) -> ColumnLineageNode {
    let mut seen = HashSet::new();
    lineage_column_inner(graph, table, column, direction, depth, &mut seen)
}

fn lineage_column_inner(
    graph: &CodeGraph,
    table: &str,
    column: &str,
    direction: LineageDirection,
    depth: usize,
    seen: &mut HashSet<(String, String)>,
) -> ColumnLineageNode {
    let mut node = ColumnLineageNode {
        table: table.to_string(),
        column: column.to_string(),
        steps: Vec::new(),
    };

    let key = (table.to_lowercase(), column.to_lowercase());
    if depth == 0 || !seen.insert(key) {
        return node;
    }

    let Some(table_idx) = find_table_node(graph, table) else {
        return node;
    };

    // Routines on the relevant side of this table: writers for upstream, readers for
    // downstream.
    let wanted = match direction {
        LineageDirection::Upstream => AccessMode::Write,
        LineageDirection::Downstream => AccessMode::Read,
    };
    let mut routines: Vec<NodeIndex> = Vec::new();
    for edge_ref in graph.edges_directed(table_idx, Direction::Incoming) {
        if let Edge::TableAccess { modes, .. } = edge_ref.weight() {
            if modes.contains(wanted) && !routines.contains(&edge_ref.source()) {
                routines.push(edge_ref.source());
            }
        }
    }

    for routine in routines {
        for mapping in mappings_of_routine(graph, routine) {
            match direction {
                LineageDirection::Upstream => {
                    let targets_this_column = mapping
                        .target_table
                        .as_deref()
                        .is_some_and(|t| eq(t, table))
                        && eq(&mapping.target_column, column);
                    if !targets_this_column {
                        continue;
                    }
                    for source in &mapping.sources {
                        let next = match source {
                            ColumnSource::Column {
                                table: Some(src_table),
                                column: src_column,
                            } => Some(lineage_column_inner(
                                graph,
                                src_table,
                                src_column,
                                direction,
                                depth - 1,
                                seen,
                            )),
                            _ => None,
                        };
                        node.steps.push(ColumnLineageStep {
                            via: routine,
                            kind: mapping.kind.clone(),
                            expression: mapping.expression.clone(),
                            source: source.clone(),
                            next,
                        });
                    }
                }
                LineageDirection::Downstream => {
                    // This column is consumed if it appears among the mapping's sources.
                    let consumes_this_column = mapping.sources.iter().any(|s| match s {
                        ColumnSource::Column {
                            table: Some(t),
                            column: c,
                        } => eq(t, table) && eq(c, column),
                        _ => false,
                    });
                    if !consumes_this_column {
                        continue;
                    }
                    let Some(target_table) = mapping.target_table.as_deref() else {
                        continue;
                    };
                    let next = lineage_column_inner(
                        graph,
                        target_table,
                        &mapping.target_column,
                        direction,
                        depth - 1,
                        seen,
                    );
                    node.steps.push(ColumnLineageStep {
                        via: routine,
                        kind: mapping.kind.clone(),
                        expression: mapping.expression.clone(),
                        source: ColumnSource::Column {
                            table: Some(target_table.to_string()),
                            column: mapping.target_column.clone(),
                        },
                        next: Some(next),
                    });
                }
            }
        }
    }

    // Views define their columns through a body, not a routine: the mappings live on
    // DependsOn edges (view → base table). Upstream reads the view's own outgoing edges,
    // downstream reads the incoming edges of a base table the view selects from.
    match direction {
        LineageDirection::Upstream => {
            for edge_ref in graph.edges_directed(table_idx, Direction::Outgoing) {
                if let Edge::DependsOn {
                    column_analysis: Some(analysis),
                    ..
                } = edge_ref.weight()
                {
                    for mapping in &analysis.column_mappings {
                        let targets_this = mapping
                            .target_table
                            .as_deref()
                            .is_some_and(|t| eq(t, table))
                            && eq(&mapping.target_column, column);
                        if !targets_this {
                            continue;
                        }
                        for source in &mapping.sources {
                            let next = match source {
                                ColumnSource::Column {
                                    table: Some(src_table),
                                    column: src_column,
                                } => Some(lineage_column_inner(
                                    graph,
                                    src_table,
                                    src_column,
                                    direction,
                                    depth - 1,
                                    seen,
                                )),
                                _ => None,
                            };
                            node.steps.push(ColumnLineageStep {
                                via: table_idx,
                                kind: mapping.kind.clone(),
                                expression: mapping.expression.clone(),
                                source: source.clone(),
                                next,
                            });
                        }
                    }
                }
            }
        }
        LineageDirection::Downstream => {
            for edge_ref in graph.edges_directed(table_idx, Direction::Incoming) {
                if let Edge::DependsOn {
                    column_analysis: Some(analysis),
                    ..
                } = edge_ref.weight()
                {
                    let view_idx = edge_ref.source();
                    for mapping in &analysis.column_mappings {
                        let consumes_this = mapping.sources.iter().any(|s| match s {
                            ColumnSource::Column {
                                table: Some(t),
                                column: c,
                            } => eq(t, table) && eq(c, column),
                            _ => false,
                        });
                        if !consumes_this {
                            continue;
                        }
                        let Some(target_table) = mapping.target_table.as_deref() else {
                            continue;
                        };
                        let next = lineage_column_inner(
                            graph,
                            target_table,
                            &mapping.target_column,
                            direction,
                            depth - 1,
                            seen,
                        );
                        node.steps.push(ColumnLineageStep {
                            via: view_idx,
                            kind: mapping.kind.clone(),
                            expression: mapping.expression.clone(),
                            source: ColumnSource::Column {
                                table: Some(target_table.to_string()),
                                column: mapping.target_column.clone(),
                            },
                            next: Some(next),
                        });
                    }
                }
            }
        }
    }

    seen.remove(&(table.to_lowercase(), column.to_lowercase()));
    node
}

fn describe_kind(kind: &MappingKind) -> String {
    match kind {
        MappingKind::Direct => "direct".to_string(),
        MappingKind::Derived => "derived".to_string(),
        MappingKind::Aggregated { function, distinct } => {
            if *distinct {
                format!("{} DISTINCT", function)
            } else {
                function.clone()
            }
        }
    }
}

fn describe_source(source: &ColumnSource) -> String {
    match source {
        ColumnSource::Column {
            table: Some(t),
            column,
        } => format!("{}.{}", t, column),
        ColumnSource::Column {
            table: None,
            column,
        } => format!("?.{}", column),
        ColumnSource::Literal { value } => format!("literal {}", value),
        ColumnSource::Variable { name } => format!("variable {}", name),
        ColumnSource::Dynamic => "dynamic SQL".to_string(),
    }
}

pub fn format_column_lineage_tree(
    node: &ColumnLineageNode,
    graph: &CodeGraph,
    direction: LineageDirection,
    indent: usize,
) -> String {
    let mut out = String::new();
    if indent == 0 {
        out.push_str(&format!("{}.{}\n", node.table, node.column));
    }
    let pad = "  ".repeat(indent + 1);
    let arrow = match direction {
        LineageDirection::Upstream => "\u{2190}",
        LineageDirection::Downstream => "\u{2192}",
    };

    // One mapping fans out into a step per source, all sharing its expression. Printing
    // that expression on every line buries the sources it is meant to explain, so it is
    // shown once per run and abbreviated when long.
    let mut last_expression: Option<&str> = None;
    for step in &node.steps {
        let via = crate::graph::key::NodeKey::from_node(&graph[step.via]);
        let mut line = format!(
            "{}{} {}  [{}]",
            pad,
            arrow,
            describe_source(&step.source),
            describe_kind(&step.kind)
        );
        match step.expression.as_deref() {
            Some(expr) if last_expression != Some(expr) => {
                line.push_str(&format!(" {}", abbreviate(expr, 96)));
                last_expression = Some(expr);
            }
            Some(_) => {}
            None => last_expression = None,
        }
        line.push_str(&format!("  via {}", via));
        out.push_str(&line);
        out.push('\n');

        if let Some(next) = &step.next {
            out.push_str(&format_column_lineage_tree(
                next,
                graph,
                direction,
                indent + 1,
            ));
        }
    }
    out
}

/// Shorten an expression for display, cutting on a char boundary so multi-byte text
/// (Chinese identifiers and string literals are common in this codebase) stays valid.
fn abbreviate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    format!("{}…", head.trim_end())
}

pub fn format_column_lineage_json(
    node: &ColumnLineageNode,
    graph: &CodeGraph,
) -> serde_json::Value {
    let steps: Vec<serde_json::Value> = node
        .steps
        .iter()
        .map(|step| {
            serde_json::json!({
                "source": describe_source(&step.source),
                "kind": describe_kind(&step.kind),
                "expression": step.expression,
                "via": crate::graph::key::NodeKey::from_node(&graph[step.via]).to_string(),
                "next": step.next.as_ref().map(|n| format_column_lineage_json(n, graph)),
            })
        })
        .collect();

    serde_json::json!({
        "table": node.table,
        "column": node.column,
        "steps": steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{ColumnSummary, DataFlowKind, RoutineId, RoutineKind, SourceLocation};
    use crate::parser::ColumnAnalysis;
    use std::collections::BTreeMap;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn table_node(name: &str, cols: &[&str]) -> Node {
        Node::Table {
            schema: None,
            name: name.to_string(),
            explicit: true,
            system: false,
            location: None,
            columns: Box::new(
                cols.iter()
                    .map(|c| ColumnSummary {
                        name: c.to_string(),
                        data_type: "NUMBER".to_string(),
                        nullable: true,
                        is_primary_key: false,
                        default_value: None,
                        comment: None,
                    })
                    .collect(),
            ),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        }
    }

    fn inferred_table_node(name: &str) -> Node {
        Node::Table {
            schema: None,
            name: name.to_string(),
            explicit: false,
            system: false,
            location: None,
            columns: Box::new(Vec::new()),
            partition_by: None,
            distribute_by: None,
            tablespace: None,
            temporary: false,
            unlogged: false,
            ddl_source: None,
        }
    }

    fn proc_node(name: &str) -> Node {
        Node::Procedure {
            id: RoutineId {
                schema: None,
                package: None,
                name: name.to_string(),
                kind: RoutineKind::Procedure,
            },
            location: SourceLocation {
                file: Arc::new(PathBuf::from("t.sql")),
                line: 1,
            },
            partial: false,
            body_sql: Vec::new(),
        }
    }

    fn loc() -> SourceLocation {
        SourceLocation {
            file: Arc::new(PathBuf::from("t.sql")),
            line: 1,
        }
    }

    fn mapping(target: &str, sources: &[&str]) -> ColumnMapping {
        ColumnMapping {
            target_table: None,
            target_column: target.to_string(),
            position: None,
            sources: sources
                .iter()
                .map(|s| ColumnSource::Column {
                    table: None,
                    column: s.to_string(),
                })
                .collect(),
            kind: MappingKind::Direct,
            expression: None,
        }
    }

    fn write_edge(
        graph: &mut CodeGraph,
        from: NodeIndex,
        to: NodeIndex,
        mappings: Vec<ColumnMapping>,
        write_kinds: &[WriteKind],
    ) {
        graph.add_edge(
            from,
            to,
            Edge::TableAccess {
                flow_kind: DataFlowKind::DmlAccess,
                modes: AccessMode::Write,
                write_kinds: write_kinds.iter().copied().collect(),
                location: loc(),
                column_analysis: Some(Box::new(ColumnAnalysis {
                    alias_map: BTreeMap::new(),
                    column_refs: Vec::new(),
                    join_conditions: Vec::new(),
                    hard_filters: Vec::new(),
                    enum_mappings: Vec::new(),
                    select_into: Vec::new(),
                    insert_columns: Vec::new(),
                    update_columns: Vec::new(),
                    column_mappings: mappings,
                    read_tables: None,
                })),
            },
        );
    }

    fn cfg() -> LineageConfig {
        LineageConfig::default()
    }

    /// Wide target: 12 columns; a source feeding 10 is flow (≥ max(8, ceil(12×0.15))),
    /// a source feeding 2 is reference.
    #[test]
    fn classify_wide_target_flow_and_reference() {
        let mut g = CodeGraph::new();
        let src = g.add_node(table_node(
            "src_trade",
            &["id", "c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8", "c9"],
        ));
        let cfg_t = g.add_node(table_node("par_cfg", &["fund_code", "rate", "mode"]));
        let out = g.add_node(table_node(
            "out_tbl",
            &[
                "id", "c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8", "c9", "rate", "mode",
            ],
        ));
        let prc = g.add_node(proc_node("prc_main"));
        write_edge(
            &mut g,
            prc,
            out,
            vec![
                mapping("id", &["id"]),
                mapping("c1", &["c1"]),
                mapping("c2", &["c2"]),
                mapping("c3", &["c3"]),
                mapping("c4", &["c4"]),
                mapping("c5", &["c5"]),
                mapping("c6", &["c6"]),
                mapping("c7", &["c7"]),
                mapping("c8", &["c8"]),
                mapping("c9", &["c9"]),
                mapping("rate", &["rate"]),
                mapping("mode", &["mode"]),
            ],
            &[WriteKind::InsertSelect],
        );

        let (role, cov, has) =
            classify_edge_role(&g, LineageDirection::Upstream, src, prc, out, &cfg());
        assert_eq!(role, EntityRole::Flow);
        assert_eq!(
            cov,
            Some(FlowCoverage {
                overlap: 10,
                total: 12
            })
        );
        assert!(has);

        let (role, cov, _) =
            classify_edge_role(&g, LineageDirection::Upstream, cfg_t, prc, out, &cfg());
        assert_eq!(role, EntityRole::Reference);
        assert_eq!(
            cov,
            Some(FlowCoverage {
                overlap: 2,
                total: 12
            })
        );
    }

    /// Narrow target (2 columns): the absolute floor is capped by the target's column
    /// count, so a fully-covering source still reaches flow (issue #146 revision 1).
    #[test]
    fn classify_narrow_target_reaches_flow() {
        let mut g = CodeGraph::new();
        let src = g.add_node(table_node("src_t", &["id", "amount"]));
        let out = g.add_node(table_node("out_t", &["id", "total"]));
        let prc = g.add_node(proc_node("prc"));
        write_edge(
            &mut g,
            prc,
            out,
            vec![mapping("id", &["id"]), mapping("total", &["amount"])],
            &[WriteKind::InsertSelect],
        );

        let (role, cov, _) =
            classify_edge_role(&g, LineageDirection::Upstream, src, prc, out, &cfg());
        assert_eq!(role, EntityRole::Flow);
        assert_eq!(
            cov,
            Some(FlowCoverage {
                overlap: 2,
                total: 2
            })
        );
    }

    /// Boundary: exactly at the threshold is flow, one below is reference.
    #[test]
    fn classify_threshold_boundary() {
        let mut g = CodeGraph::new();
        let src8 = g.add_node(table_node(
            "src8",
            &["id", "c1", "c2", "c3", "c4", "c5", "c6", "c7"],
        ));
        let src7 = g.add_node(table_node(
            "src7",
            &["id", "c1", "c2", "c3", "c4", "c5", "c6"],
        ));
        let out = g.add_node(table_node(
            "out_t",
            &[
                "id", "c1", "c2", "c3", "c4", "c5", "c6", "c7", "c8", "c9", "c10", "c11",
            ],
        ));
        let prc = g.add_node(proc_node("prc"));
        write_edge(
            &mut g,
            prc,
            out,
            vec![
                mapping("id", &["id"]),
                mapping("c1", &["c1"]),
                mapping("c2", &["c2"]),
                mapping("c3", &["c3"]),
                mapping("c4", &["c4"]),
                mapping("c5", &["c5"]),
                mapping("c6", &["c6"]),
                mapping("c7", &["c7"]),
            ],
            &[WriteKind::InsertSelect],
        );

        let (role, _, _) =
            classify_edge_role(&g, LineageDirection::Upstream, src8, prc, out, &cfg());
        assert_eq!(
            role,
            EntityRole::Flow,
            "8/12 overlap equals the 8-column floor"
        );
        let (role, _, _) =
            classify_edge_role(&g, LineageDirection::Upstream, src7, prc, out, &cfg());
        assert_eq!(role, EntityRole::Reference, "7/12 is one below the floor");
    }

    /// A source without DDL columns is Unknown, not a silent reference (revision 2).
    #[test]
    fn classify_missing_ddl_columns_is_unknown() {
        let mut g = CodeGraph::new();
        let src = g.add_node(inferred_table_node("ext_source"));
        let out = g.add_node(table_node("out_t", &["id"]));
        let prc = g.add_node(proc_node("prc"));
        write_edge(
            &mut g,
            prc,
            out,
            vec![mapping("id", &["id"])],
            &[WriteKind::InsertSelect],
        );

        let (role, cov, _) =
            classify_edge_role(&g, LineageDirection::Upstream, src, prc, out, &cfg());
        assert_eq!(role, EntityRole::Unknown);
        assert_eq!(cov, None);
    }

    /// A column-producing write edge with zero mappings reports no column data, so the
    /// caller can surface the L0 recall ceiling (revision 3).
    #[test]
    fn classify_zero_mappings_reports_no_column_data() {
        let mut g = CodeGraph::new();
        let src = g.add_node(table_node("src_t", &["id"]));
        let out = g.add_node(table_node("out_t", &["id"]));
        let prc = g.add_node(proc_node("prc"));
        write_edge(&mut g, prc, out, Vec::new(), &[WriteKind::Insert]);

        let (_, _, has) = classify_edge_role(&g, LineageDirection::Upstream, src, prc, out, &cfg());
        assert!(!has, "edge without column data must be flagged");
    }

    /// Downstream: the routine writes the child; coverage measures how many of the
    /// child's written columns trace back to the parent.
    #[test]
    fn classify_downstream_uses_written_child_as_denominator() {
        let mut g = CodeGraph::new();
        let parent = g.add_node(table_node("par_t", &["id", "rate", "mode"]));
        let child = g.add_node(table_node("child_t", &["id", "rate"]));
        let prc = g.add_node(proc_node("prc"));
        write_edge(
            &mut g,
            prc,
            child,
            vec![mapping("id", &["id"]), mapping("rate", &["rate"])],
            &[WriteKind::InsertSelect],
        );

        let (role, cov, _) =
            classify_edge_role(&g, LineageDirection::Downstream, child, prc, parent, &cfg());
        assert_eq!(
            role,
            EntityRole::Flow,
            "2/2 of a 2-column child reaches the capped floor"
        );
        assert_eq!(
            cov,
            Some(FlowCoverage {
                overlap: 2,
                total: 2
            })
        );
    }

    #[test]
    fn ddl_write_kinds_do_not_produce_columns() {
        for k in [
            WriteKind::Truncate,
            WriteKind::AlterTable,
            WriteKind::DropTable,
            WriteKind::CreateIndex,
            WriteKind::LockTable,
            WriteKind::Vacuum,
        ] {
            let mut s = HashSet::new();
            s.insert(k);
            assert!(!produces_columns(&s), "{k:?}");
        }
    }
}
