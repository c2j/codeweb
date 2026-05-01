# codeweb serve — HTTP Server + Browser UI Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a `codeweb serve` subcommand that starts an HTTP server exposing a JSON API for graph queries and serving a browser-based interactive UI that replaces the TUI for remote/non-terminal use.

**Architecture:** New `serve` feature gate (axum + tokio + rust-embed) wraps the existing synchronous `GraphStore` / `traverse` / `export` APIs behind REST endpoints. The browser UI is a single-page HTML/JS app using Cytoscape.js for graph visualization, embedded into the binary at compile time. No refactoring of existing code — all new code lives in `src/server/`.

**Tech Stack:** axum 0.7, tokio 1 (rt-multi-thread + macros), tower-http 0.6 (cors + trace), rust-embed 8, Cytoscape.js 3 (CDN or bundled), serde_json (already in tree).

---

## Prerequisites

- Existing `codeweb analyze` has been run and `.codeweb/store.bincode` exists for the target project.
- Rust stable toolchain (no MSRV change — axum 0.7 requires Rust 1.75+).

---

### Task 1: Feature Gate + Cargo.toml Setup

**Files:**
- Modify: `Cargo.toml`

**Step 1: Add `serve` feature and dependencies**

In `Cargo.toml`, add a new feature and its dependencies:

```toml
[features]
default = ["cli", "tui"]
cli = ["clap"]
tui = ["ratatui", "crossterm"]
serve = ["dep:axum", "dep:tokio", "dep:tower-http", "dep:rust-embed"]

[dependencies]
# ... existing deps unchanged ...
axum = { version = "0.7", optional = true }
tokio = { version = "1", features = ["rt-multi-thread", "macros"], optional = true }
tower-http = { version = "0.6", features = ["cors"], optional = true }
rust-embed = { version = "8", optional = true }
```

**Step 2: Verify default build is unaffected**

Run: `cargo build`
Expected: Compiles successfully with no new warnings. `axum`/`tokio` NOT compiled.

Run: `cargo build --features serve`
Expected: Compiles successfully. axum/tokio brought in.

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "feat(serve): add serve feature gate with axum/tokio dependencies"
```

---

### Task 2: Server Module Scaffold + CLI Integration

**Files:**
- Create: `src/server/mod.rs`
- Create: `src/server/state.rs`
- Create: `src/server/handlers.rs`
- Create: `src/server/assets.rs`
- Modify: `src/main.rs`

**Step 1: Create server module skeleton**

`src/server/mod.rs`:
```rust
pub mod assets;
pub mod handlers;
pub mod state;

use crate::error::Result;
use state::AppState;

pub fn run(project_path: &std::path::Path, addr: &str, open_browser: bool) -> Result<()> {
    // Load project and store synchronously before entering tokio runtime
    let mut proj = crate::project::Project::find(project_path)?;
    if proj.load_store().is_err() {
        eprintln!("No store found. Running initial analysis...");
        proj.analyze()?;
    }

    let state = AppState::new(proj);
    let listener_addr = addr.to_string();

    // Build the tokio runtime ourselves (not #[tokio::main])
    // because we're behind a feature gate
    let rt = tokio::runtime::Runtime::new().map_err(|e| crate::error::CodeWebError::ExportError {
        message: format!("failed to create tokio runtime: {}", e),
    })?;

    rt.block_on(async move {
        let app = handlers::router(state);

        let listener = tokio::net::TcpListener::bind(&listener_addr)
            .await
            .map_err(|e| crate::error::CodeWebError::ExportError {
                message: format!("failed to bind {}: {}", listener_addr, e),
            })?;

        eprintln!("codeweb serve listening on http://{}", listener_addr);

        if open_browser {
            let url = format!("http://{}", listener_addr);
            // Open browser in background, don't block or fail
            let _ = std::process::Command::new("open")
                .arg(&url)
                .spawn()
                .or_else(|_| std::process::Command::new("xdg-open").arg(&url).spawn())
                .or_else(|_| std::process::Command::new("cmd").args(["/c", "start", &url]).spawn());
        }

        axum::serve(listener, app)
            .await
            .map_err(|e| crate::error::CodeWebError::ExportError {
                message: format!("server error: {}", e),
            })
    })
}
```

`src/server/state.rs`:
```rust
use crate::project::Project;
use std::sync::Arc;

/// Shared application state. GraphStore is loaded once and wrapped in Arc
/// for safe concurrent access from all handlers.
pub struct AppState {
    pub project_name: String,
    pub project_root: std::path::PathBuf,
    store: Arc<crate::graph::store::GraphStore>,
}

impl AppState {
    pub fn new(project: Project) -> Self {
        let name = project.name().to_string();
        let root = project.root().to_path_buf();
        // Project::load_store() was already called before AppState::new()
        let store = project.store()
            .expect("store must be loaded before creating AppState")
            .clone(); // GraphStore derives Clone
        Self {
            project_name: name,
            project_root: root,
            store: Arc::new(store),
        }
    }

    pub fn store(&self) -> &crate::graph::store::GraphStore {
        &self.store
    }

    pub fn graph(&self) -> &crate::graph::CodeGraph {
        self.store.graph()
    }
}
```

`src/server/handlers.rs`:
```rust
use axum::{routing::get, Router};
use crate::server::state::AppState;

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/v1/stats", get(handlers::stats))
        .route("/api/v1/files", get(handlers::files))
        .route("/api/v1/nodes", get(handlers::nodes))
        .route("/api/v1/nodes/{id}", get(handlers::node_detail))
        .route("/api/v1/trace", get(handlers::trace))
        .route("/api/v1/export", get(handlers::export))
        .route("/api/v1/graph", get(handlers::graph_data))
        // Static assets (catch-all) must be last
        .fallback(crate::server::assets::serve_asset)
        .with_state(state)
}

mod handlers {
    use axum::{
        extract::{Path, Query, State},
        http::StatusCode,
        response::IntoResponse,
    };
    use serde::Deserialize;
    use crate::server::state::AppState;

    // --- Handlers will be implemented in Task 3 ---
    pub async fn stats(State(state): State<AppState>) -> impl IntoResponse {
        let stats = state.store().stats();
        axum::Json(stats_json(&stats))
    }

    pub async fn files(State(state): State<AppState>) -> impl IntoResponse {
        axum::Json(serde_json::json!({ "files": [] }))
    }

    pub async fn nodes(
        State(state): State<AppState>,
        Query(params): Query<NodesQuery>,
    ) -> impl IntoResponse {
        axum::Json(serde_json::json!({ "nodes": [] }))
    }

    pub async fn node_detail(
        State(state): State<AppState>,
        Path(id): Path<usize>,
    ) -> impl IntoResponse {
        axum::Json(serde_json::json!({ "error": "not implemented" }))
    }

    pub async fn trace(
        State(state): State<AppState>,
        Query(params): Query<TraceQuery>,
    ) -> impl IntoResponse {
        axum::Json(serde_json::json!({ "error": "not implemented" }))
    }

    pub async fn export(
        State(state): State<AppState>,
        Query(params): Query<ExportQuery>,
    ) -> impl IntoResponse {
        (StatusCode::NOT_IMPLEMENTED, "not implemented")
    }

    pub async fn graph_data(State(state): State<AppState>) -> impl IntoResponse {
        axum::Json(serde_json::json!({ "nodes": [], "edges": [] }))
    }

    #[derive(Deserialize)]
    pub struct NodesQuery {
        pub search: Option<String>,
        pub node_type: Option<String>,
        pub orphan: Option<bool>,
        pub low_degree: Option<usize>,
    }

    #[derive(Deserialize)]
    pub struct TraceQuery {
        pub from: String,
        pub style: Option<String>,
    }

    #[derive(Deserialize)]
    pub struct ExportQuery {
        pub format: Option<String>,
    }

    fn stats_json(stats: &crate::graph::store::StoreStats) -> serde_json::Value {
        serde_json::json!({
            "procedures": stats.procedures,
            "functions": stats.functions,
            "tables": stats.tables,
            "views": stats.views,
            "mappers": stats.mappers,
            "java_methods": stats.java_methods,
            "java_classes": stats.java_classes,
            "packages": stats.packages,
            "triggers": stats.triggers,
            "types": stats.types,
            "sequences": stats.sequences,
            "indexes": stats.indexes,
            "materialized_views": stats.materialized_views,
            "synonyms": stats.synonyms,
            "events": stats.events,
            "edges": stats.edges,
            "files": stats.files,
        })
    }
}
```

`src/server/assets.rs`:
```rust
/// Static asset serving. Will use rust-embed once frontend is added (Task 6).
/// For now returns 404 for all non-API routes.
pub async fn serve_asset(
    axum::extract::Request req: axum::extract::Request,
) -> impl axum::response::IntoResponse {
    (
        axum::http::StatusCode::NOT_FOUND,
        "not found",
    )
}
```

**Step 2: Register module + CLI subcommand in main.rs**

Add module declaration (conditional):
```rust
// In src/main.rs, after the tui module declaration:
#[cfg(feature = "serve")]
mod server;
```

Add `Serve` variant to `Commands` enum:
```rust
// In the Commands enum:
/// Start HTTP server with browser UI
#[cfg(feature = "serve")]
Serve {
    /// Project directory (default: current directory)
    #[arg(short, long, default_value = ".")]
    project: PathBuf,

    /// Listen address
    #[arg(short, long, default_value = "127.0.0.1:3000")]
    addr: String,

    /// Open browser automatically
    #[arg(long)]
    open: bool,
},
```

Add handler in `run()`:
```rust
// In the match block:
#[cfg(feature = "serve")]
Some(Commands::Serve { project, addr, open }) => server::run(&project, &addr, open),
```

**Step 3: Verify it compiles**

Run: `cargo build --features serve`
Expected: Compiles. Warnings about unused code in handlers are fine — we'll implement them next.

Run: `cargo build` (without serve feature)
Expected: Still compiles cleanly.

**Step 4: Commit**

```bash
git add src/server/ src/main.rs
git commit -m "feat(serve): scaffold server module with CLI integration"
```

---

### Task 3: Implement API Handlers (Data Layer)

**Files:**
- Modify: `src/server/handlers.rs`
- Modify: `src/server/state.rs`

**Step 1: Add Serialize derive to StoreStats**

In `src/graph/store.rs`, add `Serialize` to `StoreStats`:

```rust
#[derive(Debug, Default, Serialize)]  // Add Serialize
pub struct StoreStats {
```

Add `serde::Serialize` to the `use` imports at the top of `store.rs` if not already present.

**Step 2: Implement `stats` handler fully**

Replace the stub in `handlers::stats`:
```rust
pub async fn stats(State(state): State<AppState>) -> impl IntoResponse {
    let stats = state.store().stats();
    axum::Json(serde_json::to_value(stats).unwrap_or_default())
}
```

**Step 3: Implement `nodes` handler with search and filters**

```rust
pub async fn nodes(
    State(state): State<AppState>,
    Query(params): Query<NodesQuery>,
) -> impl IntoResponse {
    let graph = state.graph();

    // Get node indices based on search or list all
    let indices: Vec<petgraph::graph::NodeIndex> = if let Some(query) = &params.search {
        crate::graph::traverse::find_nodes_by_name(graph, query)
            .into_iter()
            .map(|(idx, _)| idx)
            .collect()
    } else if params.orphan == Some(true) {
        crate::graph::traverse::low_degree_nodes(graph, 0)
            .into_iter()
            .map(|d| d.idx)
            .collect()
    } else if let Some(max) = params.low_degree {
        crate::graph::traverse::low_degree_nodes(graph, max)
            .into_iter()
            .map(|d| d.idx)
            .collect()
    } else {
        graph.node_indices().collect()
    };

    // Filter by node type
    let type_filter = params.node_type.as_ref().map(|t| t.to_lowercase());
    let nodes: Vec<serde_json::Value> = indices
        .into_iter()
        .filter(|idx| {
            if let Some(ref tf) = type_filter {
                let tag = node_type_tag(&graph[*idx]).to_lowercase();
                tag == *tf
            } else {
                true
            }
        })
        .map(|idx| {
            let node = &graph[idx];
            let key = crate::graph::key::NodeKey::from_node(node).to_string();
            let tag = node_type_tag(node).to_string();
            let in_deg = graph.neighbors_directed(idx, petgraph::Direction::Incoming).count();
            let out_deg = graph.neighbors_directed(idx, petgraph::Direction::Outgoing).count();
            serde_json::json!({
                "id": idx.index(),
                "key": key,
                "type": tag,
                "in_degree": in_deg,
                "out_degree": out_degree,
            })
        })
        .collect();

    axum::Json(serde_json::json!({ "nodes": nodes }))
}

fn node_type_tag(node: &crate::graph::Node) -> std::borrow::Cow<'static, str> {
    match node {
        crate::graph::Node::Procedure { .. } => std::borrow::Cow::Borrowed("proc"),
        crate::graph::Node::Function { .. } => std::borrow::Cow::Borrowed("func"),
        crate::graph::Node::Unresolved { .. } => std::borrow::Cow::Borrowed("unres"),
        crate::graph::Node::MappedStatement { .. } => std::borrow::Cow::Borrowed("mapper"),
        crate::graph::Node::JavaSql { .. } => std::borrow::Cow::Borrowed("sql"),
        crate::graph::Node::JavaMethod { .. } => std::borrow::Cow::Borrowed("method"),
        crate::graph::Node::JavaClass { .. } => std::borrow::Cow::Borrowed("class"),
        crate::graph::Node::Table { .. } => std::borrow::Cow::Borrowed("table"),
        crate::graph::Node::View { .. } => std::borrow::Cow::Borrowed("view"),
        crate::graph::Node::Package { .. } => std::borrow::Cow::Borrowed("pkg"),
        crate::graph::Node::Trigger { .. } => std::borrow::Cow::Borrowed("trigger"),
        crate::graph::Node::Type { .. } => std::borrow::Cow::Borrowed("type"),
        crate::graph::Node::Sequence { .. } => std::borrow::Cow::Borrowed("seq"),
        crate::graph::Node::Index { .. } => std::borrow::Cow::Borrowed("index"),
        crate::graph::Node::MaterializedView { .. } => std::borrow::Cow::Borrowed("mview"),
        crate::graph::Node::Synonym { .. } => std::borrow::Cow::Borrowed("synonym"),
        crate::graph::Node::Event { .. } => std::borrow::Cow::Borrowed("event"),
        crate::graph::Node::Custom { type_name, .. } => {
            std::borrow::Cow::Owned(type_name.clone())
        }
    }
}
```

**Step 4: Implement `trace` handler**

```rust
pub async fn trace(
    State(state): State<AppState>,
    Query(params): Query<TraceQuery>,
) -> impl IntoResponse {
    let graph = state.graph();
    let matches = crate::graph::traverse::find_nodes_by_name(graph, &params.from);

    if matches.is_empty() {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({ "error": format!("no nodes matching '{}'", params.from) })),
        ).into_response();
    }

    // Use first match
    let (start_idx, start_name) = &matches[0];
    let chain = crate::graph::traverse::trace_chain(graph, *start_idx);

    // Serialize the call chain as JSON
    let target_key = crate::graph::key::NodeKey::from_node(&graph[*start_idx]).to_string();
    let callers_json = tree_nodes_to_json(&chain.callers, graph);
    let callees_json = tree_nodes_to_json(&chain.callees, graph);

    axum::Json(serde_json::json!({
        "target": {
            "id": start_idx.index(),
            "key": target_key,
            "type": node_type_tag(&graph[*start_idx]).to_string(),
        },
        "callers": callers_json,
        "callees": callees_json,
    })).into_response()
}

fn tree_nodes_to_json(
    nodes: &[crate::graph::traverse::TreeNode],
    graph: &crate::graph::CodeGraph,
) -> Vec<serde_json::Value> {
    nodes
        .iter()
        .map(|node| {
            let key = crate::graph::key::NodeKey::from_node(&graph[node.idx]).to_string();
            serde_json::json!({
                "id": node.idx.index(),
                "key": key,
                "type": node_type_tag(&graph[node.idx]).to_string(),
                "edge_label": node.edge_label,
                "children": tree_nodes_to_json(&node.children, graph),
            })
        })
        .collect()
}
```

**Step 5: Implement `graph_data` handler (full graph for visualization)**

```rust
pub async fn graph_data(State(state): State<AppState>) -> impl IntoResponse {
    // Reuse existing export::json module
    let graph = state.graph();
    match crate::export::json::to_json(graph) {
        Ok(json_str) => {
            // Parse back to Value to set correct content-type
            let val: serde_json::Value = serde_json::from_str(&json_str).unwrap_or_default();
            axum::Json(val)
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            axum::Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}
```

**Step 6: Implement `node_detail` handler**

```rust
pub async fn node_detail(
    State(state): State<AppState>,
    Path(id): Path<usize>,
) -> impl IntoResponse {
    let graph = state.graph();
    let idx = petgraph::graph::NodeIndex::new(id);

    if idx.index() >= graph.node_count() {
        return (
            StatusCode::NOT_FOUND,
            axum::Json(serde_json::json!({ "error": "node not found" })),
        ).into_response();
    }

    let node = &graph[idx];
    let key = crate::graph::key::NodeKey::from_node(node).to_string();
    let tag = node_type_tag(node).to_string();
    let in_deg = graph.neighbors_directed(idx, petgraph::Direction::Incoming).count();
    let out_deg = graph.neighbors_directed(idx, petgraph::Direction::Outgoing).count();

    // Neighbors
    let callers: Vec<serde_json::Value> = graph
        .neighbors_directed(idx, petgraph::Direction::Incoming)
        .map(|n| {
            let k = crate::graph::key::NodeKey::from_node(&graph[n]).to_string();
            serde_json::json!({ "id": n.index(), "key": k, "type": node_type_tag(&graph[n]).to_string() })
        })
        .collect();
    let callees: Vec<serde_json::Value> = graph
        .neighbors_directed(idx, petgraph::Direction::Outgoing)
        .map(|n| {
            let k = crate::graph::key::NodeKey::from_node(&graph[n]).to_string();
            serde_json::json!({ "id": n.index(), "key": k, "type": node_type_tag(&graph[n]).to_string() })
        })
        .collect();

    axum::Json(serde_json::json!({
        "id": idx.index(),
        "key": key,
        "type": tag,
        "in_degree": in_deg,
        "out_degree": out_deg,
        "callers": callers,
        "callees": callees,
    })).into_response()
}
```

**Step 7: Implement `export` handler**

```rust
pub async fn export(
    State(state): State<AppState>,
    Query(params): Query<ExportQuery>,
) -> impl IntoResponse {
    let graph = state.graph();
    let fmt = params.format.as_deref().unwrap_or("json");

    match fmt {
        "json" => match crate::export::json::to_json(graph) {
            Ok(content) => (
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                content,
            ).into_response(),
            Err(e) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
        },
        "dot" => {
            let content = crate::export::dot::to_dot(graph);
            (
                [(axum::http::header::CONTENT_TYPE, "text/vnd.graphviz")],
                content,
            ).into_response()
        }
        "mermaid" => {
            let content = crate::export::mermaid::to_mermaid(graph);
            (
                [(axum::http::header::CONTENT_TYPE, "text/plain")],
                content,
            ).into_response()
        }
        _ => StatusCode::BAD_REQUEST.into_response(),
    }
}
```

**Step 8: Implement `files` handler**

```rust
pub async fn files(State(state): State<AppState>) -> impl IntoResponse {
    let store = state.store();
    let root = state.project_root();
    let manifest = store.manifest();
    let file_nodes = store.file_nodes();

    let files: Vec<serde_json::Value> = manifest
        .iter()
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

    axum::Json(serde_json::json!({ "files": files }))
}
```

**Step 9: Wire up CORS for development**

In `handlers::router`, add CORS layer:
```rust
use tower_http::cors::CorsLayer;

pub fn router(state: AppState) -> Router {
    let cors = CorsLayer::permissive();

    Router::new()
        // ... routes ...
        .layer(cors)
        .with_state(state)
}
```

**Step 10: Verify all handlers compile**

Run: `cargo build --features serve`
Expected: Clean compile, no errors.

**Step 11: Manual smoke test**

```bash
# In a project that has been analyzed
cargo run --features serve -- serve --project /path/to/project --addr 127.0.0.1:3000

# In another terminal
curl http://127.0.0.1:3000/api/v1/stats
curl http://127.0.0.1:3000/api/v1/nodes?search=get_user
curl http://127.0.0.1:3000/api/v1/graph
curl http://127.0.0.1:3000/api/v1/trace?from=get_user
```

**Step 12: Commit**

```bash
git add src/server/ src/graph/store.rs
git commit -m "feat(serve): implement all REST API handlers"
```

---

### Task 4: StoreStats Serialization + Error Handling

**Files:**
- Modify: `src/graph/store.rs`
- Modify: `src/server/handlers.rs`

**Step 1: Make StoreStats serializable**

Add `#[derive(Serialize)]` to `StoreStats` in `src/graph/store.rs` (if not done in Task 3).

Ensure `serde::Serialize` is available — add to imports:
```rust
use serde::{Deserialize, Serialize};
```

Change StoreStats derivation:
```rust
#[derive(Debug, Default, Serialize)]
pub struct StoreStats {
```

**Step 2: Verify roundtrip**

Run: `cargo test --lib graph::store --features serve`
Expected: All existing tests pass. New serialization works.

**Step 3: Commit**

```bash
git add src/graph/store.rs
git commit -m "feat(serve): make StoreStats serializable for API"
```

---

### Task 5: Browser UI — HTML Shell + Search Panel

**Files:**
- Create: `assets/index.html`
- Create: `assets/app.js`
- Create: `assets/style.css`
- Modify: `src/server/assets.rs`

**Step 1: Create the HTML shell**

`assets/index.html`:
```html
<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>codeweb</title>
  <link rel="stylesheet" href="/style.css">
  <script src="https://unpkg.com/cytoscape@3.28/dist/cytoscape.min.js"></script>
  <script src="https://unpkg.com/dagre@0.8/dist/dagre.min.js"></script>
  <script src="https://unpkg.com/cytoscape-dagre@2.5/dist/cytoscape-dagre.min.js"></script>
</head>
<body>
  <div id="app">
    <!-- Header bar -->
    <div id="header">
      <span id="title">codeweb</span>
      <span id="project-name"></span>
      <div id="search-bar">
        <input type="text" id="search-input" placeholder="Search nodes... (press /)">
        <span id="node-count"></span>
      </div>
    </div>

    <!-- Main content -->
    <div id="content">
      <!-- Left panel: node list -->
      <div id="node-panel">
        <div id="node-list"></div>
      </div>

      <!-- Right panel: graph visualization -->
      <div id="graph-panel">
        <div id="cy"></div>
      </div>
    </div>

    <!-- Detail panel (hidden by default) -->
    <div id="detail-panel" class="hidden">
      <div id="detail-header">
        <button id="detail-close">&times;</button>
        <span id="detail-title"></span>
      </div>
      <div id="detail-content"></div>
    </div>

    <!-- Stats bar -->
    <div id="stats-bar"></div>
  </div>
  <script src="/app.js"></script>
</body>
</html>
```

**Step 2: Create the CSS**

`assets/style.css`:
```css
* { margin: 0; padding: 0; box-sizing: border-box; }
body { font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif; background: #1a1a2e; color: #e0e0e0; }

#app { display: flex; flex-direction: column; height: 100vh; }

/* Header */
#header { display: flex; align-items: center; padding: 8px 16px; background: #16213e; border-bottom: 1px solid #0f3460; }
#title { font-weight: bold; font-size: 16px; color: #e94560; margin-right: 12px; }
#project-name { color: #a0a0a0; font-size: 14px; margin-right: 20px; }
#search-bar { flex: 1; display: flex; align-items: center; gap: 8px; }
#search-input { width: 300px; padding: 6px 10px; background: #0f3460; border: 1px solid #1a1a5e; border-radius: 4px; color: #e0e0e0; font-size: 14px; }
#search-input:focus { outline: none; border-color: #e94560; }
#node-count { color: #a0a0a0; font-size: 12px; }

/* Content layout */
#content { display: flex; flex: 1; overflow: hidden; }

/* Node list panel */
#node-panel { width: 320px; overflow-y: auto; border-right: 1px solid #0f3460; background: #16213e; }
#node-list { padding: 4px 0; }
.node-item { display: flex; align-items: center; padding: 4px 12px; cursor: pointer; font-size: 13px; }
.node-item:hover { background: #1a1a5e; }
.node-item.active { background: #0f3460; }
.node-tag { width: 60px; font-size: 11px; font-weight: bold; margin-right: 8px; text-transform: uppercase; }
.node-key { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.node-degree { font-size: 11px; color: #666; margin-left: 8px; }

/* Graph panel */
#graph-panel { flex: 1; position: relative; }
#cy { width: 100%; height: 100%; }

/* Detail panel */
#detail-panel { position: fixed; right: 0; top: 41px; bottom: 28px; width: 400px; background: #16213e; border-left: 1px solid #0f3460; overflow-y: auto; z-index: 10; transition: transform 0.2s; }
#detail-panel.hidden { transform: translateX(100%); }
#detail-header { display: flex; align-items: center; padding: 12px 16px; background: #0f3460; }
#detail-close { background: none; border: none; color: #e94560; font-size: 20px; cursor: pointer; margin-right: 12px; }
#detail-title { font-weight: bold; }
#detail-content { padding: 16px; }
#detail-content pre { background: #0f3460; padding: 8px; border-radius: 4px; overflow-x: auto; font-size: 12px; }

/* Stats bar */
#stats-bar { display: flex; gap: 16px; padding: 4px 16px; background: #16213e; border-top: 1px solid #0f3460; font-size: 12px; color: #a0a0a0; }

/* Node type colors */
.tag-proc { color: #4caf50; }
.tag-func { color: #8bc34a; }
.tag-table { color: #ff9800; }
.tag-mapper { color: #2196f3; }
.tag-method { color: #00bcd4; }
.tag-class { color: #ff5722; }
.tag-view { color: #2196f3; }
.tag-pkg { color: #ffeb3b; }
.tag-unres { color: #f44336; }
```

**Step 3: Create the main JavaScript**

`assets/app.js`:
```javascript
// codeweb browser UI
let cy = null;
let allNodes = [];
let selectedNodeId = null;

const TAG_COLORS = {
  proc: '#4caf50', func: '#8bc34a', table: '#ff9800', mapper: '#2196f3',
  method: '#00bcd4', class: '#ff5722', view: '#2196f3', pkg: '#ffeb3b',
  unres: '#f44336', trigger: '#f44336', type: '#ffeb3b', seq: '#8bc34a',
  index: '#9e9e9e', mview: '#00bcd4', synonym: '#9c27b0', event: '#ff5722',
  sql: '#9c27b0',
};

async function api(path) {
  const res = await fetch(`/api/v1${path}`);
  return res.json();
}

async function init() {
  // Load stats
  const stats = await api('/stats');
  document.getElementById('project-name').textContent = '';
  const statsBar = document.getElementById('stats-bar');
  statsBar.innerHTML = `
    <span>${stats.procedures} procs</span>
    <span>${stats.functions} funcs</span>
    <span>${stats.tables} tables</span>
    <span>${stats.mappers} mappers</span>
    <span>${stats.java_methods} methods</span>
    <span>${stats.edges} edges</span>
    <span>${stats.files} files</span>
  `;

  // Load all nodes
  await loadNodes();

  // Initialize Cytoscape
  initGraph();

  // Setup search
  const searchInput = document.getElementById('search-input');
  let searchTimeout;
  searchInput.addEventListener('input', () => {
    clearTimeout(searchTimeout);
    searchTimeout = setTimeout(() => loadNodes(searchInput.value), 200);
  });

  // Keyboard shortcut: / to focus search
  document.addEventListener('keydown', (e) => {
    if (e.key === '/' && document.activeElement !== searchInput) {
      e.preventDefault();
      searchInput.focus();
    }
    if (e.key === 'Escape') {
      hideDetail();
      searchInput.blur();
    }
  });

  // Detail close
  document.getElementById('detail-close').addEventListener('click', hideDetail);
}

async function loadNodes(search = '') {
  const params = search ? `?search=${encodeURIComponent(search)}` : '';
  const data = await api(`/nodes${params}`);
  allNodes = data.nodes;
  renderNodeList();
  document.getElementById('node-count').textContent = `${allNodes.length} nodes`;
}

function renderNodeList() {
  const list = document.getElementById('node-list');
  list.innerHTML = allNodes.map(n => `
    <div class="node-item ${n.id === selectedNodeId ? 'active' : ''}"
         onclick="selectNode(${n.id})" data-id="${n.id}">
      <span class="node-tag tag-${n.type}" style="color:${TAG_COLORS[n.type] || '#999'}">${n.type}</span>
      <span class="node-key" title="${n.key}">${n.key}</span>
      <span class="node-degree">${n.in_degree}/${n.out_degree}</span>
    </div>
  `).join('');
}

async function selectNode(id) {
  selectedNodeId = id;
  renderNodeList();

  // Load trace for this node
  const node = allNodes.find(n => n.id === id);
  if (!node) return;

  const trace = await api(`/trace?from=${encodeURIComponent(node.key)}`);
  renderTraceGraph(trace);

  // Show detail
  const detail = await api(`/nodes/${id}`);
  showDetail(detail);
}

function initGraph() {
  cy = cytoscape({
    container: document.getElementById('cy'),
    style: [
      { selector: 'node', style: {
        'label': 'data(label)',
        'text-valign': 'center',
        'text-halign': 'center',
        'font-size': '10px',
        'color': '#e0e0e0',
        'background-color': 'data(color)',
        'width': 24, 'height': 24,
        'text-wrap': 'ellipsis',
        'text-max-width': '80px',
      }},
      { selector: 'node:selected', style: { 'border-width': 3, 'border-color': '#e94560' }},
      { selector: 'edge', style: {
        'width': 1.5,
        'line-color': '#555',
        'target-arrow-color': '#555',
        'target-arrow-shape': 'triangle',
        'curve-style': 'bezier',
        'arrow-scale': 0.8,
      }},
      { selector: '.caller', style: { 'line-color': '#2196f3', 'target-arrow-color': '#2196f3' }},
      { selector: '.callee', style: { 'line-color': '#4caf50', 'target-arrow-color': '#4caf50' }},
    ],
    layout: { name: 'preset' },
  });

  cy.on('tap', 'node', (evt) => {
    const node = evt.target;
    selectNode(parseInt(node.id()));
  });
}

function renderTraceGraph(trace) {
  if (!cy) return;
  cy.elements().remove();

  const elements = [];
  const seen = new Set();

  // Target node
  const targetColor = TAG_COLORS[trace.target.type] || '#999';
  elements.push({
    data: { id: String(trace.target.id), label: trace.target.key, color: targetColor },
    classes: 'target',
  });
  seen.add(trace.target.id);

  // Add caller/callee tree nodes
  function addTreeNodes(nodes, edgeClass) {
    for (const n of nodes) {
      if (!seen.has(n.id)) {
        const color = TAG_COLORS[n.type] || '#999';
        elements.push({
          data: { id: String(n.id), label: n.key, color: color },
        });
        seen.add(n.id);
      }
      // Find parent: find the nearest ancestor in the tree
      // For callers: edge from n.id → parent
      // For callees: edge from parent → n.id
      // We need to track the parent; use a stack approach
    }
  }

  // Better approach: recursive edge building
  function addTreeEdges(nodes, parentId, direction, edgeClass) {
    for (const n of nodes) {
      if (!seen.has(n.id)) {
        const color = TAG_COLORS[n.type] || '#999';
        elements.push({
          data: { id: String(n.id), label: n.key, color: color },
        });
        seen.add(n.id);
      }
      if (direction === 'caller') {
        // caller → target direction: caller calls target
        elements.push({
          data: { source: String(n.id), target: String(parentId) },
          classes: edgeClass,
        });
      } else {
        // target → callee direction
        elements.push({
          data: { source: String(parentId), target: String(n.id) },
          classes: edgeClass,
        });
      }
      addTreeEdges(n.children, n.id, direction, edgeClass);
    }
  }

  addTreeEdges(trace.callers, trace.target.id, 'caller', 'caller');
  addTreeEdges(trace.callees, trace.target.id, 'callee', 'callee');

  cy.add(elements);

  // Layout
  cy.layout({
    name: 'dagre',
    rankDir: 'TB',
    spacingFactor: 1.2,
    nodeSep: 30,
    rankSep: 60,
  }).run();

  // Fit to view
  cy.fit(undefined, 40);
}

function showDetail(detail) {
  const panel = document.getElementById('detail-panel');
  const title = document.getElementById('detail-title');
  const content = document.getElementById('detail-content');

  title.textContent = `${detail.type} ${detail.key}`;
  content.innerHTML = `
    <div style="margin-bottom:12px">
      <strong>Degree:</strong> in:${detail.in_degree} out:${detail.out_degree} total:${detail.in_degree + detail.out_degree}
    </div>
    ${detail.callers && detail.callers.length > 0 ? `
      <div style="margin-bottom:8px"><strong>Callers (${detail.callers.length})</strong></div>
      <div style="margin-bottom:12px">${detail.callers.map(c =>
        `<div class="node-item" onclick="selectNode(${c.id})" style="cursor:pointer">
          <span class="node-tag tag-${c.type}" style="color:${TAG_COLORS[c.type] || '#999'}">${c.type}</span>
          <span class="node-key">${c.key}</span>
        </div>`
      ).join('')}</div>
    ` : ''}
    ${detail.callees && detail.callees.length > 0 ? `
      <div style="margin-bottom:8px"><strong>Callees (${detail.callees.length})</strong></div>
      <div>${detail.callees.map(c =>
        `<div class="node-item" onclick="selectNode(${c.id})" style="cursor:pointer">
          <span class="node-tag tag-${c.type}" style="color:${TAG_COLORS[c.type] || '#999'}">${c.type}</span>
          <span class="node-key">${c.key}</span>
        </div>`
      ).join('')}</div>
    ` : ''}
  `;

  panel.classList.remove('hidden');
}

function hideDetail() {
  document.getElementById('detail-panel').classList.add('hidden');
  selectedNodeId = null;
  renderNodeList();
}

// Initialize on load
document.addEventListener('DOMContentLoaded', init);
```

**Step 4: Wire up rust-embed in assets.rs**

```rust
use rust_embed::Embed;

#[derive(Embed)]
#[folder = "assets/"]
struct Assets;

pub async fn serve_asset(
    axum::extract::Request req: axum::extract::Request,
) -> impl axum::response::IntoResponse {
    let path = req.uri().path().trim_start_matches('/');

    // Default to index.html for SPA routing
    let path = if path.is_empty() || path == "/" {
        "index.html"
    } else {
        path
    };

    match Assets::get(path) {
        Some(content) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            let body = content.data.to_vec();
            (
                axum::http::StatusCode::OK,
                [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
                body,
            ).into_response()
        }
        None => axum::http::StatusCode::NOT_FOUND.into_response(),
    }
}
```

Add `mime_guess` to `Cargo.toml` under serve feature:
```toml
mime_guess = { version = "2", optional = true }
```

Update serve feature:
```toml
serve = ["dep:axum", "dep:tokio", "dep:tower-http", "dep:rust-embed", "dep:mime_guess"]
```

**Step 5: Verify everything compiles and serves**

Run: `cargo build --features serve`
Expected: Clean compile.

Run: `cargo run --features serve -- serve --open`
Expected: Browser opens, shows dark-themed UI with search panel and graph area.

**Step 6: Commit**

```bash
git add assets/ src/server/assets.rs Cargo.toml Cargo.lock
git commit -m "feat(serve): add browser UI with Cytoscape.js graph visualization"
```

---

### Task 6: Integration Test

**Files:**
- Create: `tests/serve_api.rs`

**Step 1: Write integration tests**

`tests/serve_api.rs`:
```rust
//! Integration tests for the serve HTTP API.
//! Only compiled when the `serve` feature is enabled.

#[cfg(feature = "serve")]
mod tests {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt; // for oneshot

    fn test_app() -> axum::Router {
        // Create a minimal GraphStore with a few nodes for testing
        let mut graph = codeweb::graph::CodeGraph::new();
        use std::sync::Arc;
        use std::path::PathBuf;

        let proc = codeweb::graph::Node::Procedure {
            id: codeweb::graph::RoutineId {
                schema: Some("pkg".to_string()),
                package: None,
                name: "do_work".to_string(),
                kind: codeweb::graph::RoutineKind::Procedure,
            },
            location: codeweb::graph::SourceLocation {
                file: Arc::new(PathBuf::from("test.sql")),
                line: 1,
            },
            partial: false,
        };
        let idx = graph.add_node(proc);
        let store = codeweb::graph::store::GraphStore::from_graph("test", graph);

        let state = codeweb::server::state::AppState::from_store(store, PathBuf::from("/tmp"));
        codeweb::server::handlers::router(state)
    }

    #[tokio::test]
    async fn test_stats_endpoint() {
        let app = test_app();
        let req = Request::builder()
            .uri("/api/v1/stats")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_nodes_endpoint() {
        let app = test_app();
        let req = Request::builder()
            .uri("/api/v1/nodes")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_graph_endpoint() {
        let app = test_app();
        let req = Request::builder()
            .uri("/api/v1/graph")
            .body(Body::empty())
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }
}
```

This requires making `AppState::from_store` public. Add to `state.rs`:
```rust
/// Constructor for testing — creates AppState from an existing store.
pub fn from_store(store: crate::graph::store::GraphStore, root: std::path::PathBuf) -> Self {
    let store = Arc::new(store);
    Self {
        project_name: "test".to_string(),
        project_root: root,
        store,
    }
}
```

**Step 2: Run tests**

Run: `cargo test --features serve --test serve_api`
Expected: All tests pass.

**Step 3: Verify existing tests still pass**

Run: `cargo test`
Expected: All existing tests still pass (serve feature not enabled by default).

**Step 4: Commit**

```bash
git add tests/serve_api.rs src/server/state.rs
git commit -m "test(serve): add integration tests for HTTP API endpoints"
```

---

### Task 7: Final Polish — Clippy + Format + Verify

**Files:**
- Modify: any files with clippy warnings

**Step 1: Run clippy on serve feature**

Run: `cargo clippy --features serve -- -D warnings`
Expected: Zero warnings. Fix any that appear.

**Step 2: Run format check**

Run: `cargo fmt -- --check`
Expected: All files formatted. Fix any that aren't.

**Step 3: Run full test suite**

Run: `cargo test --features serve`
Expected: All tests pass.

**Step 4: Final commit**

```bash
git add -A
git commit -m "chore(serve): clippy + fmt pass"
```

---

## Summary

| Task | Description | Key Deliverables |
|---|---|---|
| 1 | Feature gate + deps | `Cargo.toml` with `serve` feature |
| 2 | Server scaffold + CLI | `src/server/` module, `Serve` subcommand |
| 3 | API handlers | 7 REST endpoints wrapping GraphStore/traverse/export |
| 4 | StoreStats serde | JSON serialization for stats |
| 5 | Browser UI | HTML/CSS/JS with Cytoscape.js graph visualization |
| 6 | Integration tests | API endpoint tests |
| 7 | Polish | clippy clean, fmt clean, all tests green |

**Total: 7 tasks, estimated 5-8 days of work.**
