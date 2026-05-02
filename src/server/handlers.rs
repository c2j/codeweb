use std::borrow::Cow;

use axum::{
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use petgraph::graph::NodeIndex;
use serde_json::Value;
use tower_http::cors::CorsLayer;

use crate::graph::key::NodeKey;
use crate::graph::traverse::{self, TreeNode};
use crate::graph::{CodeGraph, Node};

use super::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/stats", get(stats))
        .route("/api/v1/files", get(files))
        .route("/api/v1/nodes", get(nodes))
        .route("/api/v1/nodes/:id", get(node_detail))
        .route("/api/v1/trace", get(trace))
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
}

async fn nodes(
    State(state): State<AppState>,
    Query(query): Query<NodesQuery>,
) -> impl IntoResponse {
    let graph = state.graph();
    let max_degree = if query.orphan == Some(true) {
        Some(0)
    } else {
        query.low_degree
    };

    let type_filter = query.node_type.map(|t| t.to_lowercase());

    let indices: Vec<NodeIndex> = if let Some(search) = query.search {
        let matches = traverse::find_nodes_by_name(graph, &search);
        matches.into_iter().map(|(idx, _)| idx).collect()
    } else {
        graph.node_indices().collect()
    };

    let filtered: Vec<_> = indices
        .into_iter()
        .filter(|idx| {
            if let Some(ref tf) = type_filter {
                let tag = node_type_tag(&graph[*idx]).to_lowercase();
                if tag != *tf {
                    return false;
                }
            }
            true
        })
        .filter_map(|idx| {
            let in_deg = graph
                .neighbors_directed(idx, petgraph::Direction::Incoming)
                .count();
            let out_deg = graph
                .neighbors_directed(idx, petgraph::Direction::Outgoing)
                .count();
            let total = in_deg + out_deg;

            if let Some(max) = max_degree {
                if total > max {
                    return None;
                }
            }

            Some((idx, in_deg, out_deg))
        })
        .collect();

    let result: Vec<Value> = filtered
        .into_iter()
        .map(|(idx, in_deg, out_deg)| {
            let key = NodeKey::from_node(&graph[idx]);
            serde_json::json!({
                "id": idx.index(),
                "key": key.to_string(),
                "type": node_type_tag(&graph[idx]),
                "in_degree": in_deg,
                "out_degree": out_deg,
            })
        })
        .collect();

    Json(result)
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
    let result = serde_json::json!({
        "id": idx.index(),
        "key": key.to_string(),
        "type": node_type_tag(node),
        "in_degree": in_deg,
        "out_degree": out_deg,
        "callers": callers,
        "callees": callees,
    });

    Ok(Json(result))
}

#[derive(serde::Deserialize)]
struct TraceQuery {
    from: String,
}

async fn trace(
    State(state): State<AppState>,
    Query(query): Query<TraceQuery>,
) -> Result<impl IntoResponse, StatusCode> {
    let graph = state.graph();
    let matches = traverse::find_nodes_by_name(graph, &query.from);

    if matches.is_empty() {
        return Err(StatusCode::NOT_FOUND);
    }

    let (start_idx, _) = &matches[0];
    let chain = traverse::trace_chain(graph, *start_idx);

    let target_key = NodeKey::from_node(&graph[chain.target]);
    let result = serde_json::json!({
        "target": {
            "id": chain.target.index(),
            "key": target_key.to_string(),
            "type": node_type_tag(&graph[chain.target]),
        },
        "callers": tree_nodes_to_json(&chain.callers, graph),
        "callees": tree_nodes_to_json(&chain.callees, graph),
    });

    Ok(Json(result))
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

fn node_type_tag(node: &Node) -> Cow<'static, str> {
    match node {
        Node::Procedure { partial: true, .. } => Cow::Borrowed("proc*"),
        Node::Procedure { .. } => Cow::Borrowed("proc"),
        Node::Function { partial: true, .. } => Cow::Borrowed("func*"),
        Node::Function { .. } => Cow::Borrowed("func"),
        Node::Unresolved { .. } => Cow::Borrowed("unres"),
        Node::MappedStatement { .. } => Cow::Borrowed("mapper"),
        Node::JavaSql { .. } => Cow::Borrowed("sql"),
        Node::JavaMethod { .. } => Cow::Borrowed("method"),
        Node::JavaClass { .. } => Cow::Borrowed("class"),
        Node::Table { .. } => Cow::Borrowed("table"),
        Node::View { .. } => Cow::Borrowed("view"),
        Node::Package { .. } => Cow::Borrowed("pkg"),
        Node::Trigger { .. } => Cow::Borrowed("trigger"),
        Node::Type { .. } => Cow::Borrowed("type"),
        Node::Sequence { .. } => Cow::Borrowed("seq"),
        Node::Index { .. } => Cow::Borrowed("index"),
        Node::MaterializedView { .. } => Cow::Borrowed("mview"),
        Node::Synonym { .. } => Cow::Borrowed("synonym"),
        Node::Event { .. } => Cow::Borrowed("event"),
        Node::Custom { type_name, .. } => Cow::Owned(type_name.clone()),
    }
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
