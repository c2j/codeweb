use std::sync::Arc;

use petgraph::graph::NodeIndex;
use petgraph::Direction;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::schemars;
use rmcp::tool;
use rmcp::tool_router;
use serde::Deserialize;
use serde_json;

use crate::graph::key::NodeKey;
use crate::graph::node_sub_type_tag;
use crate::graph::query::spec::QuerySpec;
use crate::graph::store::GraphStore;
use crate::graph::traverse;
use crate::graph::{CodeGraph, Node};

// ── Shared state ──

pub struct McpState {
    store: Arc<GraphStore>,
    project_name: String,
    empty_reason: Option<String>,
}

impl McpState {
    pub fn new(store: GraphStore, project_name: String, empty_reason: Option<String>) -> Self {
        Self {
            store: Arc::new(store),
            project_name,
            empty_reason,
        }
    }

    fn store(&self) -> &GraphStore {
        &self.store
    }

    fn graph(&self) -> &CodeGraph {
        self.store.graph()
    }

    fn graph_empty(&self) -> bool {
        self.store.graph().node_count() == 0
    }

    fn empty_graph_response(&self) -> String {
        let message = self.empty_reason.clone().unwrap_or_else(|| {
            "Code graph has 0 nodes — the project contains no analyzable source files.".to_string()
        });
        serde_json::to_string(&serde_json::json!({
            "status": "empty",
            "project": &self.project_name,
            "message": message,
            "hint": "Run `codeweb analyze` in the project directory, then restart the MCP server.",
        }))
        .unwrap_or_default()
    }
}

// ── Parameter structs ──

#[derive(Deserialize, schemars::JsonSchema)]
pub struct NodesParams {
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub node_type: Option<String>,
    #[serde(default)]
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct NodeDetailParams {
    pub ids: Vec<usize>,
    #[serde(default)]
    pub depth: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct TraceParams {
    pub from: String,
    #[serde(default)]
    pub depth: Option<usize>,
    #[serde(default)]
    pub max_nodes: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SearchSqlParams {
    pub sql: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct QueryParams {
    pub spec: serde_json::Value,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct ColumnAnalysisParams {
    #[serde(default)]
    pub procedure: Option<String>,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub table: Option<String>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct LineageParams {
    pub target: String,
    #[serde(default)]
    pub direction: Option<String>,
    #[serde(default)]
    pub depth: Option<usize>,
}

// ── Helper functions ──

fn tree_nodes_to_json(nodes: &[traverse::TreeNode], graph: &CodeGraph) -> Vec<serde_json::Value> {
    nodes
        .iter()
        .map(|node| {
            let key = NodeKey::from_node(&graph[node.idx]);
            serde_json::json!({
                "id": node.idx.index(),
                "key": key.to_string(),
                "type": node_sub_type_tag(&graph[node.idx]),
                "edge_label": node.edge_label,
                "has_more": node.has_more,
                "more_count": node.more_count,
                "children": tree_nodes_to_json(&node.children, graph),
            })
        })
        .collect()
}

// ── Tool implementations ──

#[tool_router]
impl McpState {
    /// Get project statistics
    #[tool(
        description = "Get project statistics including node counts by type and edge count. Always call this first to check graph status."
    )]
    fn codeweb_stats(&self) -> String {
        if self.graph_empty() {
            return self.empty_graph_response();
        }
        let stats = self.store().stats();
        let result = serde_json::json!({
            "status": "ready",
            "project": &self.project_name,
            "procedures": stats.procedures,
            "functions": stats.functions,
            "unresolved": stats.unresolved,
            "mappers": stats.mappers,
            "java_sql": stats.java_sql,
            "java_methods": stats.java_methods,
            "java_classes": stats.java_classes,
            "tables": stats.tables,
            "views": stats.views,
            "packages": stats.packages,
            "triggers": stats.triggers,
            "types": stats.types,
            "sequences": stats.sequences,
            "indexes": stats.indexes,
            "materialized_views": stats.materialized_views,
            "synonyms": stats.synonyms,
            "events": stats.events,
            "builtin_functions": stats.builtin_functions,
            "custom_nodes": stats.custom_nodes,
            "edges": stats.edges,
            "files": stats.files,
        });
        serde_json::to_string(&result).unwrap_or_default()
    }

    /// List graph nodes with optional filtering and pagination
    #[tool(
        description = "List graph nodes, optionally filtered by search string and node type, with pagination"
    )]
    fn codeweb_nodes(&self, Parameters(params): Parameters<NodesParams>) -> String {
        if self.graph_empty() {
            return self.empty_graph_response();
        }
        let summaries = self.store().node_summaries();
        let search_lower = params.search.map(|s| s.to_lowercase());
        let type_filter = params.node_type.map(|t| t.to_lowercase());

        let mut filtered: Vec<_> = summaries
            .iter()
            .filter(|s| {
                if let Some(ref tf) = type_filter {
                    if s.type_tag != *tf {
                        return false;
                    }
                }
                if let Some(ref sl) = search_lower {
                    if !s.key_lower.contains(sl) {
                        return false;
                    }
                }
                true
            })
            .collect();

        if let Some(ref sl) = search_lower {
            filtered.sort_by(|a, b| {
                use crate::graph::traverse::MatchRank;
                let rank_a = MatchRank::classify(sl, &a.key_lower);
                let rank_b = MatchRank::classify(sl, &b.key_lower);
                match rank_a.cmp(&rank_b) {
                    std::cmp::Ordering::Equal => a.key.cmp(&b.key),
                    other => other,
                }
            });
        }

        let total_count = filtered.len();
        let limit_val = params.limit.unwrap_or(100);
        let offset_val = params.offset.unwrap_or(0);

        let nodes: Vec<serde_json::Value> = filtered
            .into_iter()
            .skip(offset_val)
            .take(limit_val)
            .map(|s| {
                serde_json::json!({
                    "id": s.id,
                    "key": &s.key,
                    "type": &s.type_tag,
                    "in_degree": s.in_degree,
                    "out_degree": s.out_degree,
                })
            })
            .collect();

        let result = serde_json::json!({
            "total": total_count,
            "limit": limit_val,
            "offset": offset_val,
            "nodes": nodes,
        });
        serde_json::to_string(&result).unwrap_or_default()
    }

    /// Get detailed information about one or more nodes
    #[tool(
        description = "Get detailed information about one or more nodes by IDs, including their properties, callers, and callees. Set depth to control traversal (default 1 = direct only, 0 = unlimited, N = N hops)"
    )]
    fn codeweb_node_detail(&self, Parameters(params): Parameters<NodeDetailParams>) -> String {
        if self.graph_empty() {
            return self.empty_graph_response();
        }
        let graph = self.graph();
        let depth = params.depth.unwrap_or(1);
        let mut results: Vec<serde_json::Value> = Vec::new();

        for &id in &params.ids {
            let idx = NodeIndex::new(id);

            if idx.index() >= graph.node_count() {
                results.push(serde_json::json!({
                    "id": id,
                    "error": format!("Node {} not found", id),
                }));
                continue;
            }

            let node = &graph[idx];
            let key = NodeKey::from_node(node);

            let callers: Vec<serde_json::Value> =
                traverse::neighbors_at_depth(graph, idx, Direction::Incoming, depth)
                    .into_iter()
                    .map(|n| {
                        serde_json::json!({
                            "id": n.index(),
                            "key": NodeKey::from_node(&graph[n]).to_string(),
                            "type": node_sub_type_tag(&graph[n]),
                        })
                    })
                    .collect();

            let callees: Vec<serde_json::Value> =
                traverse::neighbors_at_depth(graph, idx, Direction::Outgoing, depth)
                    .into_iter()
                    .map(|n| {
                        serde_json::json!({
                            "id": n.index(),
                            "key": NodeKey::from_node(&graph[n]).to_string(),
                            "type": node_sub_type_tag(&graph[n]),
                        })
                    })
                    .collect();

            let in_deg = graph.neighbors_directed(idx, Direction::Incoming).count();
            let out_deg = graph.neighbors_directed(idx, Direction::Outgoing).count();

            let mut properties: Vec<serde_json::Value> = Vec::new();
            match node {
                Node::MappedStatement {
                    namespace,
                    statement_id,
                    kind,
                    xml_file,
                    line,
                    sql,
                    ..
                } => {
                    properties.push(serde_json::json!({"label": "namespace", "value": namespace}));
                    properties
                        .push(serde_json::json!({"label": "statement_id", "value": statement_id}));
                    properties.push(serde_json::json!({"label": "kind", "value": kind}));
                    properties.push(
                        serde_json::json!({"label": "file", "value": xml_file.to_string_lossy()}),
                    );
                    properties.push(serde_json::json!({"label": "line", "value": line}));
                    if let Some(sql_text) = sql {
                        properties.push(serde_json::json!({"label": "sql", "value": sql_text}));
                    }
                }
                Node::JavaSql {
                    class_name,
                    method_name,
                    extraction_method,
                    java_file,
                    line,
                    sql,
                    ..
                } => {
                    properties.push(
                        serde_json::json!({"label": "file", "value": java_file.to_string_lossy()}),
                    );
                    properties.push(serde_json::json!({"label": "line", "value": line}));
                    if let Some(c) = class_name {
                        properties.push(serde_json::json!({"label": "class", "value": c}));
                    }
                    if let Some(m) = method_name {
                        properties.push(serde_json::json!({"label": "method", "value": m}));
                    }
                    properties.push(
                        serde_json::json!({"label": "extraction", "value": extraction_method}),
                    );
                    if let Some(sql_text) = sql {
                        properties.push(serde_json::json!({"label": "sql", "value": sql_text}));
                    }
                }
                Node::Procedure {
                    id: proc_id,
                    location,
                    partial,
                    body_sql,
                    ..
                }
                | Node::Function {
                    id: proc_id,
                    location,
                    partial,
                    body_sql,
                    ..
                } => {
                    properties
                        .push(serde_json::json!({"label": "schema", "value": proc_id.schema}));
                    properties
                        .push(serde_json::json!({"label": "package", "value": proc_id.package}));
                    properties.push(serde_json::json!({"label": "name", "value": proc_id.name}));
                    properties.push(
                        serde_json::json!({"label": "file", "value": location.file.to_string_lossy()}),
                    );
                    properties.push(serde_json::json!({"label": "line", "value": location.line}));
                    if *partial {
                        properties.push(serde_json::json!({"label": "partial", "value": "true"}));
                    }
                    if !body_sql.is_empty() {
                        let body_sql_list: Vec<serde_json::Value> = body_sql
                            .iter()
                            .map(|s| {
                                serde_json::json!({
                                    "sql": s.sql_text,
                                    "kind": s.kind,
                                })
                            })
                            .collect();
                        properties
                            .push(serde_json::json!({"label": "body_sql", "value": body_sql_list}));
                    }
                }
                Node::JavaMethod {
                    fqn,
                    class_fqn,
                    name,
                    signature,
                    file,
                    line,
                    ..
                } => {
                    properties.push(serde_json::json!({"label": "fqn", "value": fqn}));
                    properties.push(serde_json::json!({"label": "class_fqn", "value": class_fqn}));
                    properties.push(serde_json::json!({"label": "name", "value": name}));
                    properties.push(serde_json::json!({"label": "signature", "value": signature}));
                    properties.push(
                        serde_json::json!({"label": "file", "value": file.to_string_lossy()}),
                    );
                    properties.push(serde_json::json!({"label": "line", "value": line}));
                }
                _ => {}
            }

            results.push(serde_json::json!({
                "id": idx.index(),
                "key": key.to_string(),
                "type": node_sub_type_tag(node),
                "in_degree": in_deg,
                "out_degree": out_deg,
                "callers": callers,
                "callees": callees,
                "properties": properties,
            }));
        }

        let result = serde_json::json!({
            "results": results,
            "total": results.len(),
        });
        serde_json::to_string(&result).unwrap_or_default()
    }

    /// Trace bidirectional call chain from a node
    #[tool(
        description = "Trace bidirectional call chain from a node by name, showing callers and callees as nested trees"
    )]
    fn codeweb_trace(&self, Parameters(params): Parameters<TraceParams>) -> String {
        if self.graph_empty() {
            return self.empty_graph_response();
        }
        let store = self.store();
        let graph = self.graph();
        let matches = store.search_nodes(&params.from);

        if matches.is_empty() {
            let err = serde_json::json!({"error": format!("No nodes matching '{}'", params.from)});
            return serde_json::to_string(&err).unwrap_or_default();
        }

        let (start_idx, _) = &matches[0];
        let depth = params.depth.unwrap_or(2).min(10);
        let max_nodes = params.max_nodes.unwrap_or(500);
        let (chain, visited) = traverse::trace_chain(graph, *start_idx, depth, max_nodes, false);

        let target_key = NodeKey::from_node(&graph[chain.target]);
        let result = serde_json::json!({
            "target": {
                "id": chain.target.index(),
                "key": target_key.to_string(),
                "type": node_sub_type_tag(&graph[chain.target]),
            },
            "callers": tree_nodes_to_json(&chain.callers, graph),
            "callees": tree_nodes_to_json(&chain.callees, graph),
            "caller_count": chain.callers.len(),
            "callee_count": chain.callees.len(),
            "truncated": visited >= max_nodes,
        });
        serde_json::to_string(&result).unwrap_or_default()
    }

    /// Search nodes by SQL text content
    #[tool(
        description = "Search MappedStatement and JavaSql nodes by SQL text content, with relevance scoring and detailed results"
    )]
    fn codeweb_search_sql(&self, Parameters(params): Parameters<SearchSqlParams>) -> String {
        if self.graph_empty() {
            return self.empty_graph_response();
        }
        let graph = self.graph();
        let results = self.store().search_by_sql(&params.sql);
        let nodes: Vec<serde_json::Value> = results
            .into_iter()
            .map(|(idx, display_key, score)| {
                let node = &graph[idx];
                let detail = node_sub_type_tag(node);
                let in_deg = graph.neighbors_directed(idx, Direction::Incoming).count();
                let out_deg = graph.neighbors_directed(idx, Direction::Outgoing).count();
                let body_sql = match node {
                    Node::Procedure { body_sql, .. } | Node::Function { body_sql, .. } => {
                        if body_sql.is_empty() {
                            None
                        } else {
                            Some(
                                body_sql
                                    .iter()
                                    .map(|s| {
                                        serde_json::json!({
                                            "sql": s.sql_text,
                                            "kind": s.kind,
                                        })
                                    })
                                    .collect::<Vec<_>>(),
                            )
                        }
                    }
                    _ => None,
                };
                serde_json::json!({
                    "id": idx.index(),
                    "key": display_key,
                    "type": detail,
                    "score": (score * 100.0).round() / 100.0,
                    "in_degree": in_deg,
                    "out_degree": out_deg,
                    "body_sql": body_sql,
                })
            })
            .collect();
        let result = serde_json::json!({
            "total": nodes.len(),
            "nodes": nodes,
        });
        serde_json::to_string(&result).unwrap_or_default()
    }

    /// Execute a declarative JSON QuerySpec
    #[tool(
        description = "Execute a declarative JSON query spec against the graph for complex multi-step traversals"
    )]
    fn codeweb_query(&self, Parameters(params): Parameters<QueryParams>) -> String {
        if self.graph_empty() {
            return self.empty_graph_response();
        }
        let spec: Result<QuerySpec, _> = serde_json::from_value(params.spec);
        let spec = match spec {
            Ok(s) => s,
            Err(e) => {
                let err = serde_json::json!({"error": format!("Invalid query spec: {}", e)});
                return serde_json::to_string(&err).unwrap_or_default();
            }
        };

        match spec.execute(self.store.as_ref()) {
            Ok(result) => serde_json::to_string(&result).unwrap_or_default(),
            Err(e) => {
                let err = serde_json::json!({"error": e});
                serde_json::to_string(&err).unwrap_or_default()
            }
        }
    }

    /// Aggregate per-procedure/per-package column analysis
    #[tool(
        description = "Aggregate hard filters, join conditions, SELECT INTO mappings, and enum/column mappings for a procedure or package — same JSON schema as `codeweb columns --format json` (issue #165). Provide exactly one of procedure or package; table optionally narrows to one table's diagnostics."
    )]
    fn codeweb_column_analysis(
        &self,
        Parameters(params): Parameters<ColumnAnalysisParams>,
    ) -> String {
        if self.graph_empty() {
            return self.empty_graph_response();
        }
        if params.procedure.is_some() == params.package.is_some() {
            let err = serde_json::json!({
                "error": "exactly one of 'procedure' or 'package' is required",
            });
            return serde_json::to_string(&err).unwrap_or_default();
        }

        let store = self.store();
        let graph = self.graph();
        let table_filter = params.table.as_deref();

        let result = if let Some(name) = &params.procedure {
            let idx = match resolve_node(store, name, true) {
                Ok(idx) => idx,
                Err(msg) => return msg,
            };
            if !matches!(&graph[idx], Node::Procedure { .. } | Node::Function { .. }) {
                let err = serde_json::json!({
                    "error": format!("'{}' is not a procedure or function", name),
                });
                return serde_json::to_string(&err).unwrap_or_default();
            }
            crate::graph::columns::column_analysis_of_routine(graph, idx, table_filter)
        } else {
            let name = params.package.as_ref().expect("checked exactly-one above");
            let idx = match resolve_node(store, name, true) {
                Ok(idx) => idx,
                Err(msg) => return msg,
            };
            if !matches!(&graph[idx], Node::Package { .. }) {
                let err = serde_json::json!({"error": format!("'{}' is not a package", name)});
                return serde_json::to_string(&err).unwrap_or_default();
            }
            crate::graph::columns::column_analysis_of_package(graph, idx, table_filter)
        };

        match result {
            Some(analysis) => serde_json::to_string(&analysis).unwrap_or_default(),
            None => {
                let err = serde_json::json!({"error": "failed to aggregate column analysis"});
                serde_json::to_string(&err).unwrap_or_default()
            }
        }
    }

    /// Table-level or column-level lineage for a target
    #[tool(
        description = "Table-level or column-level lineage for a target ('table', 'table.column', or a node key like 'table:schema.table') — same functions/JSON shape as `codeweb lineage --format json` (issue #165 P1). direction: upstream/downstream/both (default both); depth default 5."
    )]
    fn codeweb_lineage(&self, Parameters(params): Parameters<LineageParams>) -> String {
        if self.graph_empty() {
            return self.empty_graph_response();
        }
        let graph = self.graph();
        let store = self.store();
        let depth = params.depth.unwrap_or(5);
        let direction = params.direction.as_deref().unwrap_or("both");

        let dir_spec = match direction.to_lowercase().as_str() {
            "upstream" => Some(crate::graph::lineage::LineageDirection::Upstream),
            "downstream" => Some(crate::graph::lineage::LineageDirection::Downstream),
            "both" => None,
            _ => {
                let err = serde_json::json!({
                    "error": format!(
                        "Unknown direction: {}. Use 'upstream', 'downstream' or 'both'",
                        direction
                    ),
                });
                return serde_json::to_string(&err).unwrap_or_default();
            }
        };

        let parsed = match crate::graph::lineage::parse_lineage_target(graph, &params.target) {
            Ok(p) => p,
            Err(e) => {
                let err = serde_json::json!({"error": e});
                return serde_json::to_string(&err).unwrap_or_default();
            }
        };

        match parsed {
            crate::graph::lineage::ParsedLineageTarget::Column(table, column) => {
                let render = |dir: crate::graph::lineage::LineageDirection| {
                    let node =
                        crate::graph::lineage::lineage_column(graph, &table, &column, dir, depth);
                    crate::graph::lineage::format_column_lineage_json(&node, graph)
                };
                let json = match dir_spec {
                    Some(dir) => render(dir),
                    None => serde_json::json!({
                        "upstream": render(crate::graph::lineage::LineageDirection::Upstream),
                        "downstream": render(crate::graph::lineage::LineageDirection::Downstream),
                    }),
                };
                serde_json::to_string(&json).unwrap_or_default()
            }
            crate::graph::lineage::ParsedLineageTarget::Table(table_name) => {
                let table_idx = match resolve_node(store, &table_name, false) {
                    Ok(idx) => idx,
                    Err(msg) => return msg,
                };
                if !matches!(&graph[table_idx], Node::Table { .. } | Node::View { .. }) {
                    let err = serde_json::json!({
                        "error": format!("'{}' is not a table or view", table_name),
                    });
                    return serde_json::to_string(&err).unwrap_or_default();
                }

                let cfg = crate::graph::lineage::LineageConfig::default();
                let opts = crate::graph::lineage::DisplayOptions::new(
                    crate::graph::lineage::LineageView::Tree,
                    false,
                );
                let json = match dir_spec {
                    Some(dir) => {
                        let node = crate::graph::lineage::lineage_table(
                            graph, table_idx, dir, depth, &cfg,
                        );
                        crate::graph::lineage::format_lineage_json(&node, graph, &opts)
                    }
                    None => {
                        let up = crate::graph::lineage::lineage_table(
                            graph,
                            table_idx,
                            crate::graph::lineage::LineageDirection::Upstream,
                            depth,
                            &cfg,
                        );
                        let down = crate::graph::lineage::lineage_table(
                            graph,
                            table_idx,
                            crate::graph::lineage::LineageDirection::Downstream,
                            depth,
                            &cfg,
                        );
                        serde_json::json!({
                            "upstream": crate::graph::lineage::format_lineage_json(&up, graph, &opts),
                            "downstream": crate::graph::lineage::format_lineage_json(&down, graph, &opts),
                        })
                    }
                };
                serde_json::to_string(&json).unwrap_or_default()
            }
        }
    }
}

/// Resolve `name` (substring match, same as `trace`/CLI `columns`/`lineage`) to a single
/// node, or a pre-serialized `{"error": ...}` JSON string on empty/ambiguous match —
/// shared by `codeweb_column_analysis` and `codeweb_lineage`.
fn resolve_node(
    store: &GraphStore,
    name: &str,
    fail_on_multiple: bool,
) -> Result<NodeIndex, String> {
    match store.resolve_single_node(
        name,
        crate::graph::search::MatchMode::Substring,
        false,
        fail_on_multiple,
    ) {
        crate::graph::search::ResolveResult::Single(idx, _) => Ok(idx),
        crate::graph::search::ResolveResult::Empty => {
            let err = serde_json::json!({"error": format!("No nodes matching '{}'", name)});
            Err(serde_json::to_string(&err).unwrap_or_default())
        }
        crate::graph::search::ResolveResult::Ambiguous => {
            let count = store
                .search_nodes_with_mode(name, crate::graph::search::MatchMode::Substring)
                .len();
            let err = serde_json::json!({
                "error": format!("Ambiguous match: {} candidates for '{}'", count, name)
            });
            Err(serde_json::to_string(&err).unwrap_or_default())
        }
        crate::graph::search::ResolveResult::Multiple(_) => unreachable!("all_matches is false"),
    }
}

// ── ServerHandler implementation ──

use rmcp::tool_handler;
use rmcp::ServerHandler;

#[tool_handler(
    name = "codeweb",
    instructions = "Code graph analysis tools. ALWAYS call codeweb_stats first. \
        If it returns status='empty', the graph has not been built — tell the user to run `codeweb analyze` in the project directory then restart this MCP server, and stop. \
        If status='ready': use codeweb_nodes to find nodes (search + type filter), \
        codeweb_trace to follow call chains bidirectionally, \
        codeweb_search_sql to find SQL by text content, \
        codeweb_node_detail for properties + callers + callees, \
        codeweb_query for complex multi-step traversals via JSON QuerySpec, \
        codeweb_column_analysis for per-procedure/per-package hard filters, joins, and column mappings, \
        codeweb_lineage for table/column-level lineage tracing."
)]
impl ServerHandler for McpState {}
