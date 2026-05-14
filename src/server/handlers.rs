use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use petgraph::graph::NodeIndex;
use serde_json::Value;
use tower_http::cors::CorsLayer;

use crate::graph::key::NodeKey;
use crate::graph::node_type_tag;
use crate::graph::query::spec::QuerySpec;
use crate::graph::traverse::{self, MatchRank, TreeNode};
use crate::graph::{CodeGraph, Node};

use super::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/stats", get(stats))
        .route("/api/v1/files", get(files))
        .route("/api/v1/nodes", get(nodes))
        .route("/api/v1/nodes/:id", get(node_detail))
        .route("/api/v1/nodes/:id/callers", get(node_callers))
        .route("/api/v1/nodes/:id/callees", get(node_callees))
        .route("/api/v1/nodes/search-sql", get(search_sql))
        .route("/api/v1/trace", get(trace))
        .route("/api/v1/query", post(execute_query))
        .route("/api/v1/export", get(export))
        .route("/api/v1/graph", get(graph_data))
        .layer(CorsLayer::permissive())
        .fallback(super::assets::serve_asset)
        .with_state(state)
}

async fn stats(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.store().stats())
}

#[derive(serde::Deserialize)]
struct NodesQuery {
    search: Option<String>,
    node_type: Option<String>,
    orphan: Option<bool>,
    low_degree: Option<usize>,
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn nodes(
    State(state): State<AppState>,
    Query(query): Query<NodesQuery>,
) -> impl IntoResponse {
    let max_degree = if query.orphan == Some(true) {
        Some(0)
    } else {
        query.low_degree
    };

    let type_filter = query.node_type.map(|t| t.to_lowercase());
    let search_lower = query.search.map(|s| s.to_lowercase());

    let summaries = state.store().node_summaries();

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
        .filter(|s| {
            if let Some(max) = max_degree {
                let total = s.in_degree + s.out_degree;
                if total > max {
                    return false;
                }
            }
            true
        })
        .collect();

    if let Some(ref sl) = search_lower {
        filtered.sort_by(|a, b| {
            let rank_a = MatchRank::classify(sl, &a.key_lower);
            let rank_b = MatchRank::classify(sl, &b.key_lower);
            match rank_a.cmp(&rank_b) {
                std::cmp::Ordering::Equal => a.key.cmp(&b.key),
                other => other,
            }
        });
    }

    let total_count = filtered.len();
    let limit_val = query.limit.unwrap_or(100);
    let offset_val = query.offset.unwrap_or(0);

    let result: Vec<Value> = filtered
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

    Json(serde_json::json!({
        "total": total_count,
        "limit": limit_val,
        "offset": offset_val,
        "nodes": result,
    }))
}

#[derive(serde::Deserialize)]
struct SearchSqlQuery {
    q: String,
}

async fn search_sql(
    State(state): State<AppState>,
    Query(query): Query<SearchSqlQuery>,
) -> impl IntoResponse {
    let graph = state.graph();
    let results = state.store().search_by_sql(&query.q);
    let nodes: Vec<Value> = results
        .into_iter()
        .map(|(idx, display_key)| {
            let node = &graph[idx];
            let detail = node_type_tag(node);
            let in_deg = graph
                .neighbors_directed(idx, petgraph::Direction::Incoming)
                .count();
            let out_deg = graph
                .neighbors_directed(idx, petgraph::Direction::Outgoing)
                .count();
            serde_json::json!({
                "id": idx.index(),
                "key": display_key,
                "type": detail,
                "in_degree": in_deg,
                "out_degree": out_deg,
            })
        })
        .collect();
    Json(serde_json::json!({
        "total": nodes.len(),
        "nodes": nodes,
    }))
}

async fn node_detail(
    State(state): State<AppState>,
    Path(id): Path<usize>,
) -> Result<impl IntoResponse, StatusCode> {
    let graph = state.graph();
    let idx = NodeIndex::new(id);

    if idx.index() >= graph.node_count() {
        return Err(StatusCode::NOT_FOUND);
    }

    let node = &graph[idx];
    let in_deg = graph
        .neighbors_directed(idx, petgraph::Direction::Incoming)
        .count();
    let out_deg = graph
        .neighbors_directed(idx, petgraph::Direction::Outgoing)
        .count();

    let callers: Vec<Value> = graph
        .neighbors_directed(idx, petgraph::Direction::Incoming)
        .map(|n| {
            let key = NodeKey::from_node(&graph[n]);
            serde_json::json!({
                "id": n.index(),
                "key": key.to_string(),
                "type": node_type_tag(&graph[n]),
            })
        })
        .collect();

    let callees: Vec<Value> = graph
        .neighbors_directed(idx, petgraph::Direction::Outgoing)
        .map(|n| {
            let key = NodeKey::from_node(&graph[n]);
            serde_json::json!({
                "id": n.index(),
                "key": key.to_string(),
                "type": node_type_tag(&graph[n]),
            })
        })
        .collect();

    let key = NodeKey::from_node(node);
    let mut properties: Vec<Value> = Vec::new();
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
            properties.push(serde_json::json!({"label": "statement_id", "value": statement_id}));
            properties.push(serde_json::json!({"label": "kind", "value": kind}));
            properties
                .push(serde_json::json!({"label": "file", "value": xml_file.to_string_lossy()}));
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
            properties
                .push(serde_json::json!({"label": "file", "value": java_file.to_string_lossy()}));
            properties.push(serde_json::json!({"label": "line", "value": line}));
            if let Some(c) = class_name {
                properties.push(serde_json::json!({"label": "class", "value": c}));
            }
            if let Some(m) = method_name {
                properties.push(serde_json::json!({"label": "method", "value": m}));
            }
            properties.push(serde_json::json!({"label": "extraction", "value": extraction_method}));
            if let Some(sql_text) = sql {
                properties.push(serde_json::json!({"label": "sql", "value": sql_text}));
            }
        }
        Node::Table {
            schema,
            name,
            location,
            columns,
            tablespace,
            temporary,
            unlogged,
            ddl_source,
            ..
        } => {
            if let Some(s) = schema {
                properties.push(serde_json::json!({"label": "schema", "value": s}));
            }
            properties.push(serde_json::json!({"label": "name", "value": name}));
            if let Some(loc) = location {
                properties.push(
                    serde_json::json!({"label": "file", "value": loc.file.to_string_lossy()}),
                );
                properties.push(serde_json::json!({"label": "line", "value": loc.line}));
            }
            if !columns.is_empty() {
                let col_summary: Vec<Value> = columns.iter().map(|c| {
                    serde_json::json!({"name": c.name, "type": c.data_type, "nullable": c.nullable, "pk": c.is_primary_key})
                }).collect();
                properties.push(serde_json::json!({"label": "columns", "value": col_summary}));
            }
            if *temporary {
                properties.push(serde_json::json!({"label": "temporary", "value": "true"}));
            }
            if *unlogged {
                properties.push(serde_json::json!({"label": "unlogged", "value": "true"}));
            }
            if let Some(ts) = tablespace {
                properties.push(serde_json::json!({"label": "tablespace", "value": ts}));
            }
            if let Some(ddl) = ddl_source {
                properties.push(serde_json::json!({"label": "ddl", "value": ddl.as_ref()}));
            }
        }
        Node::Procedure {
            id,
            location,
            partial,
            ..
        }
        | Node::Function {
            id,
            location,
            partial,
            ..
        } => {
            properties.push(serde_json::json!({"label": "schema", "value": id.schema}));
            properties.push(serde_json::json!({"label": "package", "value": id.package}));
            properties.push(serde_json::json!({"label": "name", "value": id.name}));
            properties.push(
                serde_json::json!({"label": "file", "value": location.file.to_string_lossy()}),
            );
            properties.push(serde_json::json!({"label": "line", "value": location.line}));
            if *partial {
                properties.push(serde_json::json!({"label": "partial", "value": "true"}));
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
            properties.push(serde_json::json!({"label": "file", "value": file.to_string_lossy()}));
            properties.push(serde_json::json!({"label": "line", "value": line}));
        }
        _ => {}
    }

    let result = serde_json::json!({
        "id": idx.index(),
        "key": key.to_string(),
        "type": node_type_tag(node),
        "in_degree": in_deg,
        "out_degree": out_deg,
        "callers": callers,
        "callees": callees,
        "properties": properties,
    });

    Ok(Json(result))
}

#[derive(serde::Deserialize)]
struct TraceQuery {
    from: String,
    depth: Option<usize>,
    max_nodes: Option<usize>,
}

async fn trace(
    State(state): State<AppState>,
    Query(query): Query<TraceQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let store = state.store();
    let matches = store.search_nodes(&query.from);
    let graph = state.graph();

    if matches.is_empty() {
        return Err(StatusCode::NOT_FOUND);
    }

    let (start_idx, _) = &matches[0];
    let depth = query.depth.unwrap_or(2).min(10);
    let max_nodes = query.max_nodes.unwrap_or(500);
    let (chain, visited) = traverse::trace_chain(graph, *start_idx, depth, max_nodes);

    let target_key = NodeKey::from_node(&graph[chain.target]);
    let result = serde_json::json!({
        "target": {
            "id": chain.target.index(),
            "key": target_key.to_string(),
            "type": node_type_tag(&graph[chain.target]),
        },
        "callers": tree_nodes_to_json(&chain.callers, graph),
        "callees": tree_nodes_to_json(&chain.callees, graph),
        "caller_count": chain.callers.len(),
        "callee_count": chain.callees.len(),
        "truncated": visited >= max_nodes,
    });

    Ok(Json(result))
}

async fn execute_query(
    State(state): State<AppState>,
    Json(spec): Json<QuerySpec>,
) -> Result<impl IntoResponse, (StatusCode, String)> {
    let store = state.store();
    match spec.execute(store) {
        Ok(result) => Ok(Json(result)),
        Err(e) => Err((StatusCode::BAD_REQUEST, e)),
    }
}

#[derive(serde::Deserialize)]
struct NeighborsQuery {
    limit: Option<usize>,
    offset: Option<usize>,
}

async fn node_callers(
    State(state): State<AppState>,
    Path(id): Path<usize>,
    Query(query): Query<NeighborsQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let graph = state.graph();
    let idx = NodeIndex::new(id);

    if idx.index() >= graph.node_count() {
        return Err(StatusCode::NOT_FOUND);
    }

    let all: Vec<Value> = graph
        .neighbors_directed(idx, petgraph::Direction::Incoming)
        .map(|n| {
            let key = NodeKey::from_node(&graph[n]);
            serde_json::json!({
                "id": n.index(),
                "key": key.to_string(),
                "type": node_type_tag(&graph[n]),
            })
        })
        .collect();

    let total = all.len();
    let limit_val = query.limit.unwrap_or(50);
    let offset_val = query.offset.unwrap_or(0);

    let nodes: Vec<Value> = all.into_iter().skip(offset_val).take(limit_val).collect();

    Ok(Json(serde_json::json!({
        "total": total,
        "limit": limit_val,
        "offset": offset_val,
        "nodes": nodes,
    })))
}

async fn node_callees(
    State(state): State<AppState>,
    Path(id): Path<usize>,
    Query(query): Query<NeighborsQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let graph = state.graph();
    let idx = NodeIndex::new(id);

    if idx.index() >= graph.node_count() {
        return Err(StatusCode::NOT_FOUND);
    }

    let all: Vec<Value> = graph
        .neighbors_directed(idx, petgraph::Direction::Outgoing)
        .map(|n| {
            let key = NodeKey::from_node(&graph[n]);
            serde_json::json!({
                "id": n.index(),
                "key": key.to_string(),
                "type": node_type_tag(&graph[n]),
            })
        })
        .collect();

    let total = all.len();
    let limit_val = query.limit.unwrap_or(50);
    let offset_val = query.offset.unwrap_or(0);

    let nodes: Vec<Value> = all.into_iter().skip(offset_val).take(limit_val).collect();

    Ok(Json(serde_json::json!({
        "total": total,
        "limit": limit_val,
        "offset": offset_val,
        "nodes": nodes,
    })))
}

async fn graph_data(State(state): State<AppState>) -> Result<impl IntoResponse, StatusCode> {
    let graph = state.graph();
    match crate::export::json::to_json(graph) {
        Ok(json) => Ok(([(header::CONTENT_TYPE, "application/json")], json)),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(serde::Deserialize)]
struct ExportQuery {
    format: String,
}

async fn export(
    State(state): State<AppState>,
    Query(query): Query<ExportQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let graph = state.graph();

    let (content_type, body) = match query.format.as_str() {
        "dot" => ("text/vnd.graphviz", crate::export::dot::to_dot(graph)),
        "json" => match crate::export::json::to_json(graph) {
            Ok(json) => ("application/json", json),
            Err(_) => return Err(StatusCode::INTERNAL_SERVER_ERROR),
        },
        "mermaid" => ("text/plain", crate::export::mermaid::to_mermaid(graph)),
        _ => return Err(StatusCode::BAD_REQUEST),
    };

    Ok(([(header::CONTENT_TYPE, content_type)], body))
}

async fn files(State(state): State<AppState>) -> impl IntoResponse {
    let store = state.store();
    let manifest = store.manifest();
    let file_nodes = store.file_nodes();
    let root = state.project_root();

    let mut entries: Vec<_> = manifest.iter().collect();
    entries.sort_by(|a, b| a.0.cmp(b.0));

    let result: Vec<Value> = entries
        .into_iter()
        .map(|(path, record)| {
            let rel = path.strip_prefix(root).unwrap_or(path);
            let type_tag = match record.file_type {
                crate::parser::fingerprint::FileType::Sql => "SQL",
                crate::parser::fingerprint::FileType::Java => "Java",
                crate::parser::fingerprint::FileType::Xml => "XML",
            };
            let node_count = file_nodes
                .get(path as &std::path::Path)
                .map(|v| v.len())
                .unwrap_or(0);
            serde_json::json!({
                "path": rel.to_string_lossy(),
                "type": type_tag,
                "nodes": node_count,
            })
        })
        .collect();

    Json(result)
}

fn tree_nodes_to_json(nodes: &[TreeNode], graph: &CodeGraph) -> Vec<Value> {
    nodes
        .iter()
        .map(|node| {
            let key = NodeKey::from_node(&graph[node.idx]);
            serde_json::json!({
                "id": node.idx.index(),
                "key": key.to_string(),
                "type": node_type_tag(&graph[node.idx]),
                "edge_label": node.edge_label,
                "children": tree_nodes_to_json(&node.children, graph),
            })
        })
        .collect()
}
