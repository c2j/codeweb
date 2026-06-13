# MCP Server 实施计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add native MCP (Model Context Protocol) server mode to codeweb, allowing LLM clients (Claude Desktop, Cursor, etc.) to query code graphs directly via stdio JSON-RPC.

**Architecture:** Add a `mcp` feature flag and `src/mcp/` module. The MCP server wraps the existing `GraphStore` API, exposing 6 tools (stats, nodes, detail, trace, search_sql, query) via the `rmcp` crate's declarative macro system. The `codeweb mcp` CLI subcommand starts the server over stdio — no HTTP, no ports, zero config.

**Tech Stack:** Rust, `rmcp` v1.7 (official MCP SDK), `tokio` (async runtime), `schemars` (JSON Schema generation via rmcp's re-export)

---

## 设计决策

### 为什么用 rmcp 而不是手写 JSON-RPC？

| 方案 | 代码量 | 依赖 | 维护成本 |
|---|---|---|---|
| 手写 JSON-RPC | ~600 行 | 0 | 需跟进 MCP 协议变更 |
| rmcp（官方 SDK） | ~350 行 | rmcp + schemars | 协议兼容性由 SDK 保证 |

rmcp 是 MCP 官方 Rust SDK（12M+ 下载），提供：
- 自动 JSON-RPC 消息序列化/反序列化
- `initialize` 握手处理
- `tools/list` 自动生成（从 `#[tool]` 宏）
- stdio 传输层
- JSON Schema 自动生成（从 `schemars::JsonSchema` derive）

### Tool 设计（6 个工具）

| Tool | 对应能力 | 参数 |
|---|---|---|
| `codeweb_stats` | `GET /api/v1/stats` | 无 |
| `codeweb_nodes` | `GET /api/v1/nodes` | `search`, `node_type`, `limit`, `offset` |
| `codeweb_node_detail` | `GET /api/v1/nodes/:id` | `id` (NodeIndex) |
| `codeweb_trace` | `GET /api/v1/trace` | `from`, `depth`, `max_nodes` |
| `codeweb_search_sql` | `GET /api/v1/nodes/search-sql` | `sql` |
| `codeweb_query` | `POST /api/v1/query` | `spec` (QuerySpec JSON) |

### Feature Flag 设计

```toml
[features]
mcp = ["dep:rmcp", "dep:tokio", "dep:schemars"]
```

- 复用 `tokio`（项目已有，在 `serve` feature 中）
- `rmcp` 需要新引入
- `schemars` 由 rmcp 内部使用，但 tool 参数类型需要 derive `JsonSchema`

---

## Task 1: 添加 rmcp 依赖和 feature flag

**Files:**
- Modify: `Cargo.toml`

**Step 1: 添加 rmcp 和 schemars 依赖**

在 `[features]` 中添加 `mcp` feature，在 `[dependencies]` 中添加 rmcp 和 schemars。

```toml
# 在 [features] 部分添加
mcp = ["dep:rmcp", "dep:schemars"]

# 在 [dependencies] 部分添加
rmcp = { version = "1.7", features = ["server", "transport-io"], optional = true }
schemars = { version = "0.8", optional = true }
```

注意：
- `tokio` 不需要加到 mcp feature 里，因为 rmcp 自己会带 tokio。但我们的 main 函数中需要创建 tokio runtime。检查 rmcp 的 tokio feature：如果 rmcp 自带 tokio，我们可以直接用。否则需要在 mcp feature 中也加入 `dep:tokio`。
- 实际上，项目已经有 `tokio = { version = "1", features = ["rt-multi-thread", "macros"], optional = true }` 在 serve feature 中。mcp 也需要 tokio，所以应该让 mcp feature 依赖 tokio：`mcp = ["dep:rmcp", "dep:schemars", "dep:tokio"]`。

**Step 2: 验证 Cargo.toml 语法正确**

Run: `cargo check --features mcp 2>&1 | head -20`
Expected: 可能有编译错误（没有 mcp 模块），但 Cargo.toml 解析不应报错

**Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add rmcp + schemars dependencies behind mcp feature flag"
```

---

## Task 2: 创建 mcp 模块骨架

**Files:**
- Create: `src/mcp/mod.rs`
- Create: `src/mcp/server.rs`
- Create: `src/mcp/tools.rs`
- Modify: `src/main.rs`

**Step 1: 创建 `src/mcp/mod.rs`**

```rust
pub mod server;
pub mod tools;
```

**Step 2: 创建 `src/mcp/tools.rs` — Tool 参数类型和 handler**

这是核心文件。定义 MCP tool 参数类型和 tool 实现。

```rust
use rmcp::{
    handler::server::wrapper::{Json, Parameters},
    schemars,
    tool, tool_router,
};
use serde::{Deserialize, Serialize};

use crate::graph::key::NodeKey;
use crate::graph::node_type_tag;
use crate::graph::query::spec::QuerySpec;
use crate::graph::traverse::{self, TreeNode};
use crate::graph::{CodeGraph, Node};
use crate::graph::store::GraphStore;

use petgraph::graph::NodeIndex;
use petgraph::Direction;
use std::sync::Arc;

/// MCP server state: holds a reference to the loaded GraphStore.
pub struct McpState {
    store: Arc<GraphStore>,
}

impl McpState {
    pub fn new(store: GraphStore) -> Self {
        Self {
            store: Arc::new(store),
        }
    }
}

// ── Tool parameter types ──────────────────────────────────────

#[derive(Deserialize, schemars::JsonSchema)]
pub struct NodesParams {
    /// Search nodes by name (substring match, case-insensitive).
    #[serde(default)]
    pub search: Option<String>,
    /// Filter by node type tag (proc, func, table, mapper, method, class, etc.).
    #[serde(default)]
    pub node_type: Option<String>,
    /// Maximum number of nodes to return (default 50, max 200).
    #[serde(default)]
    pub limit: Option<usize>,
    /// Number of nodes to skip (for pagination).
    #[serde(default)]
    pub offset: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct NodeDetailParams {
    /// Node index (from codeweb_nodes or codeweb_trace results).
    pub id: usize,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct TraceParams {
    /// Node name to search for (substring match).
    pub from: String,
    /// Maximum traversal depth (default 3, max 10).
    #[serde(default)]
    pub depth: Option<usize>,
    /// Maximum number of nodes to visit (default 500).
    #[serde(default)]
    pub max_nodes: Option<usize>,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct SearchSqlParams {
    /// SQL fragment to search for (case-insensitive substring match).
    pub sql: String,
}

#[derive(Deserialize, schemars::JsonSchema)]
pub struct QueryParams {
    /// QuerySpec JSON object for complex multi-step graph traversal.
    /// See codeweb documentation for QuerySpec format.
    pub spec: serde_json::Value,
}

// ── Helper functions ──────────────────────────────────────────

fn node_to_json(idx: NodeIndex, graph: &CodeGraph) -> serde_json::Value {
    let node = &graph[idx];
    let key = NodeKey::from_node(node);
    let tag = node_type_tag(node);
    let in_deg = graph.neighbors_directed(idx, Direction::Incoming).count();
    let out_deg = graph.neighbors_directed(idx, Direction::Outgoing).count();
    serde_json::json!({
        "id": idx.index(),
        "key": key.to_string(),
        "type": tag,
        "in_degree": in_deg,
        "out_degree": out_deg,
    })
}

fn tree_nodes_to_json(nodes: &[TreeNode], graph: &CodeGraph) -> Vec<serde_json::Value> {
    nodes.iter().map(|n| {
        let key = NodeKey::from_node(&graph[n.idx]);
        serde_json::json!({
            "id": n.idx.index(),
            "key": key.to_string(),
            "type": node_type_tag(&graph[n.idx]),
            "edge_label": n.edge_label,
            "children": tree_nodes_to_json(&n.children, graph),
        })
    }).collect()
}

// ── Tool implementations ──────────────────────────────────────

#[tool_router]
impl McpState {
    #[tool(description = "Get project statistics — node counts by type, edge count, file count.")]
    fn codeweb_stats(&self) -> Json<serde_json::Value> {
        let stats = self.store.stats();
        Json(serde_json::to_value(stats).unwrap_or_else(|_| serde_json::json!({})))
    }

    #[tool(description = "List graph nodes with optional filtering by name and type. Returns node id, key, type, and degree info.")]
    fn codeweb_nodes(
        &self,
        Parameters(params): Parameters<NodesParams>,
    ) -> Json<serde_json::Value> {
        let graph = self.store.graph();
        let summaries = self.store.node_summaries();

        let search_lower = params.search.map(|s| s.to_lowercase());
        let type_filter = params.node_type.map(|t| t.to_lowercase());
        let limit = params.limit.unwrap_or(50).min(200);
        let offset = params.offset.unwrap_or(0);

        let filtered: Vec<_> = summaries
            .iter()
            .filter(|s| {
                if let Some(ref tf) = type_filter {
                    if s.type_tag != *tf { return false; }
                }
                if let Some(ref sl) = search_lower {
                    if !s.key_lower.contains(sl) { return false; }
                }
                true
            })
            .skip(offset)
            .take(limit)
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
            "nodes": filtered,
            "count": filtered.len(),
        }))
    }

    #[tool(description = "Get detailed information about a specific node — properties, callers (upstream), callees (downstream).")]
    fn codeweb_node_detail(
        &self,
        Parameters(params): Parameters<NodeDetailParams>,
    ) -> Json<serde_json::Value> {
        let graph = self.store.graph();
        let idx = NodeIndex::new(params.id);

        if idx.index() >= graph.node_count() {
            return Json(serde_json::json!({
                "error": format!("Node {} not found (graph has {} nodes)", params.id, graph.node_count())
            }));
        }

        let node = &graph[idx];
        let key = NodeKey::from_node(node);
        let tag = node_type_tag(node);
        let in_deg = graph.neighbors_directed(idx, Direction::Incoming).count();
        let out_deg = graph.neighbors_directed(idx, Direction::Outgoing).count();

        let callers: Vec<serde_json::Value> = graph
            .neighbors_directed(idx, Direction::Incoming)
            .map(|n| node_to_json(n, graph))
            .collect();

        let callees: Vec<serde_json::Value> = graph
            .neighbors_directed(idx, Direction::Outgoing)
            .map(|n| node_to_json(n, graph))
            .collect();

        // Build properties map based on node type
        let mut properties = serde_json::Map::new();
        match node {
            Node::Procedure { id, location, partial, body_sql, .. }
            | Node::Function { id, location, partial, body_sql, .. } => {
                properties.insert("schema".into(), serde_json::Value::String(id.schema.clone().unwrap_or_default()));
                properties.insert("package".into(), serde_json::Value::String(id.package.clone().unwrap_or_default()));
                properties.insert("name".into(), serde_json::Value::String(id.name.clone()));
                properties.insert("file".into(), serde_json::Value::String(location.file.to_string_lossy().into()));
                properties.insert("line".into(), serde_json::Value::Number(location.line.into()));
                if *partial {
                    properties.insert("partial".into(), serde_json::Value::Bool(true));
                }
                if !body_sql.is_empty() {
                    let sql_list: Vec<serde_json::Value> = body_sql.iter().map(|s| {
                        serde_json::json!({ "sql": s.sql_text, "kind": s.kind })
                    }).collect();
                    properties.insert("body_sql".into(), serde_json::Value::Array(sql_list));
                }
            }
            Node::MappedStatement { namespace, statement_id, kind, xml_file, line, sql, .. } => {
                properties.insert("namespace".into(), serde_json::Value::String(namespace.clone()));
                properties.insert("statement_id".into(), serde_json::Value::String(statement_id.clone()));
                properties.insert("kind".into(), serde_json::Value::String(kind.clone()));
                properties.insert("file".into(), serde_json::Value::String(xml_file.to_string_lossy().into()));
                properties.insert("line".into(), serde_json::Value::Number((*line).into()));
                if let Some(sql_text) = sql {
                    properties.insert("sql".into(), serde_json::Value::String(sql_text.clone()));
                }
            }
            Node::JavaMethod { fqn, class_fqn, name, signature, file, line, .. } => {
                properties.insert("fqn".into(), serde_json::Value::String(fqn.clone()));
                properties.insert("class_fqn".into(), serde_json::Value::String(class_fqn.clone()));
                properties.insert("name".into(), serde_json::Value::String(name.clone()));
                properties.insert("signature".into(), serde_json::Value::String(signature.clone()));
                properties.insert("file".into(), serde_json::Value::String(file.to_string_lossy().into()));
                properties.insert("line".into(), serde_json::Value::Number((*line).into()));
            }
            Node::JavaSql { class_name, method_name, extraction_method, java_file, line, sql, .. } => {
                if let Some(c) = class_name { properties.insert("class".into(), serde_json::Value::String(c.clone())); }
                if let Some(m) = method_name { properties.insert("method".into(), serde_json::Value::String(m.clone())); }
                properties.insert("extraction".into(), serde_json::Value::String(extraction_method.clone()));
                properties.insert("file".into(), serde_json::Value::String(java_file.to_string_lossy().into()));
                properties.insert("line".into(), serde_json::Value::Number((*line).into()));
                if let Some(sql_text) = sql { properties.insert("sql".into(), serde_json::Value::String(sql_text.clone())); }
            }
            Node::Table { schema, name, columns, .. } => {
                if let Some(s) = schema { properties.insert("schema".into(), serde_json::Value::String(s.clone())); }
                properties.insert("name".into(), serde_json::Value::String(name.clone()));
                if !columns.is_empty() {
                    let cols: Vec<serde_json::Value> = columns.iter().map(|c| {
                        serde_json::json!({ "name": c.name, "type": c.data_type })
                    }).collect();
                    properties.insert("columns".into(), serde_json::Value::Array(cols));
                }
            }
            _ => {}
        }

        Json(serde_json::json!({
            "id": idx.index(),
            "key": key.to_string(),
            "type": tag,
            "in_degree": in_deg,
            "out_degree": out_deg,
            "properties": properties,
            "callers": callers,
            "callees": callees,
        }))
    }

    #[tool(description = "Trace the call chain from a node — shows both upstream (callers) and downstream (callees) relationships. Use this to understand the full impact path of a stored procedure, mapper, or Java method.")]
    fn codeweb_trace(
        &self,
        Parameters(params): Parameters<TraceParams>,
    ) -> Json<serde_json::Value> {
        let matches = self.store.search_nodes(&params.from);
        let graph = self.store.graph();

        if matches.is_empty() {
            return Json(serde_json::json!({
                "error": format!("No nodes matching '{}'", params.from),
                "suggestion": "Use codeweb_nodes to search for the correct node name."
            }));
        }

        let (start_idx, start_name) = &matches[0];
        let depth = params.depth.unwrap_or(3).min(10);
        let max_nodes = params.max_nodes.unwrap_or(500);

        let (chain, visited) = traverse::trace_chain(graph, *start_idx, depth, max_nodes);

        let target_key = NodeKey::from_node(&graph[chain.target]);
        Json(serde_json::json!({
            "target": {
                "id": chain.target.index(),
                "key": target_key.to_string(),
                "type": node_type_tag(&graph[chain.target]),
            },
            "matched_name": start_name,
            "callers": tree_nodes_to_json(&chain.callers, graph),
            "callees": tree_nodes_to_json(&chain.callees, graph),
            "caller_count": chain.callers.len(),
            "callee_count": chain.callees.len(),
            "truncated": visited >= max_nodes,
        }))
    }

    #[tool(description = "Search for nodes containing specific SQL text (MappedStatement, JavaSql, Procedure). Traces back to Java callers when applicable.")]
    fn codeweb_search_sql(
        &self,
        Parameters(params): Parameters<SearchSqlParams>,
    ) -> Json<serde_json::Value> {
        let graph = self.store.graph();
        let results = self.store.search_by_sql(&params.sql);

        if results.is_empty() {
            return Json(serde_json::json!({
                "matches": [],
                "count": 0,
            }));
        }

        let nodes: Vec<serde_json::Value> = results.into_iter().map(|(idx, display_key, score)| {
            let node = &graph[idx];
            let tag = node_type_tag(node);
            let score_pct = (score * 100.0).round() as u8;

            // Extract SQL text based on node type
            let sql_text = match node {
                Node::MappedStatement { sql, .. } => sql.clone(),
                Node::JavaSql { sql, .. } => sql.clone(),
                Node::Procedure { body_sql, .. } | Node::Function { body_sql, .. } => {
                    if body_sql.is_empty() { None } else {
                        Some(body_sql.iter().map(|s| s.sql_text.as_str()).collect::<Vec<_>>().join("\n"))
                    }
                }
                _ => None,
            };

            // Find Java callers (for mapper/sql nodes)
            let callers: Vec<serde_json::Value> = graph
                .neighbors_directed(idx, Direction::Incoming)
                .filter_map(|n| match &graph[n] {
                    Node::JavaMethod { fqn, .. } => Some(serde_json::json!({
                        "fqn": fqn,
                        "id": n.index(),
                    })),
                    _ => None,
                })
                .collect();

            serde_json::json!({
                "id": idx.index(),
                "key": display_key,
                "type": tag,
                "score": score_pct,
                "sql": sql_text,
                "callers": callers,
            })
        }).collect();

        Json(serde_json::json!({
            "matches": nodes,
            "count": nodes.len(),
        }))
    }

    #[tool(description = "Execute a declarative QuerySpec for complex multi-step graph traversals. Supports outgoing/incoming traversal, filtering, path collection, and subgraph extraction. This is the most powerful query interface.")]
    fn codeweb_query(
        &self,
        Parameters(params): Parameters<QueryParams>,
    ) -> Json<serde_json::Value> {
        let spec: QuerySpec = match serde_json::from_value(params.spec) {
            Ok(s) => s,
            Err(e) => {
                return Json(serde_json::json!({
                    "error": format!("Invalid QuerySpec: {}", e)
                }));
            }
        };

        match spec.execute(self.store.as_ref()) {
            Ok(result) => Json(result),
            Err(e) => Json(serde_json::json!({
                "error": e,
            })),
        }
    }
}
```

**Step 3: 创建 `src/mcp/server.rs` — MCP server 入口**

```rust
use crate::error::Result;
use crate::project::Project;

use super::tools::McpState;

pub fn run(project_path: &std::path::Path) -> Result<()> {
    let mut proj = Project::find(project_path)?;
    let _ = proj.load_store()?;
    let mut store = proj.take_store().unwrap_or_else(|| {
        crate::graph::store::GraphStore::new(proj.name())
    });
    store.ensure_consistency_with_progress();

    let state = McpState::new(store);

    let runtime = tokio::runtime::Runtime::new().map_err(|e| {
        crate::error::CodeWebError::ExportError {
            message: format!("failed to create tokio runtime: {}", e),
        }
    })?;

    use rmcp::ServiceExt;

    runtime.block_on(async {
        let transport = rmcp::transport::io::stdio();
        let server = state.serve(transport).await.map_err(|e| {
            crate::error::CodeWebError::ExportError {
                message: format!("MCP server error: {}", e),
            }
        })?;
        server.waiting().await.map_err(|e| {
            crate::error::CodeWebError::ExportError {
                message: format!("MCP server wait error: {}", e),
            }
        })
    })
}
```

**Step 4: 在 `src/main.rs` 中注册模块和子命令**

添加 module 声明（在 `mod server;` 后面）：
```rust
#[cfg(feature = "mcp")]
mod mcp;
```

添加 CLI 子命令（在 `Serve` 变体后）：
```rust
/// Start MCP server for LLM integration (stdio JSON-RPC)
///
/// Starts an MCP server that allows LLM clients (Claude Desktop, Cursor, etc.)
/// to query the code graph. Communicates over stdin/stdout using JSON-RPC.
///
/// Requires the "mcp" feature flag: cargo run --features mcp -- mcp
#[cfg(feature = "mcp")]
Mcp {
    /// Project directory (default: current directory)
    #[arg(short, long, default_value = ".")]
    project: PathBuf,
},
```

添加 match 分支（在 `Serve` 分支后）：
```rust
#[cfg(feature = "mcp")]
Some(Commands::Mcp { project }) => mcp::run(&project),
```

**Step 5: 验证编译**

Run: `cargo check --features mcp 2>&1 | tail -30`
Expected: 编译成功，可能有少量 warning

**Step 6: Commit**

```bash
git add src/mcp/ src/main.rs
git commit -m "feat: add MCP server module with 6 tools behind mcp feature flag"
```

---

## Task 3: 添加 ServerHandler trait 实现

**Files:**
- Modify: `src/mcp/tools.rs`

**Step 1: 添加 `tool_handler` impl**

在 `tools.rs` 文件末尾添加：

```rust
use rmcp::handler::server::ServerHandler;
use rmcp::tool_handler;

#[tool_handler(name = "codeweb", version = env!("CARGO_PKG_VERSION"), instructions = "Code graph analysis tools. Use codeweb_stats for overview, codeweb_nodes to find nodes, codeweb_trace to follow call chains, codeweb_search_sql to find SQL, codeweb_node_detail for deep inspection, codeweb_query for complex traversals.")]
impl ServerHandler for McpState {}
```

**Step 2: 验证编译**

Run: `cargo check --features mcp 2>&1 | tail -20`
Expected: 编译成功

**Step 3: Commit**

```bash
git add src/mcp/tools.rs
git commit -m "feat: add ServerHandler impl with tool_handler macro"
```

---

## Task 4: 集成测试

**Files:**
- Create: `tests/mcp_test.rs`

**Step 1: 编写 MCP 集成测试**

测试通过子进程启动 `codeweb mcp`，发送 JSON-RPC 请求到 stdin，读取 stdout 响应。

```rust
#[cfg(feature = "mcp")]
mod tests {
    use std::io::{BufRead, Write};
    use std::process::{Child, Command, Stdio};

    fn codeweb_bin() -> std::path::PathBuf {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
        let bin_name = if cfg!(windows) { "codeweb.exe" } else { "codeweb" };
        let entries = std::fs::read_dir(&base).unwrap_or_else(|_| panic!("no target dir"));
        for entry in entries.flatten() {
            let p = entry.path().join("debug").join(bin_name);
            if p.exists() {
                return p;
            }
        }
        base.join("debug").join(bin_name)
    }

    struct McpChild {
        child: Child,
    }

    impl McpChild {
        fn start() -> Self {
            let child = Command::new(codeweb_bin())
                .args(["mcp", "--project", env!("CARGO_MANIFEST_DIR")])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn()
                .expect("failed to start codeweb mcp");
            Self { child }
        }

        fn send(&mut self, json: &str) {
            let stdin = self.child.stdin.as_mut().expect("no stdin");
            // MCP uses newline-delimited JSON
            writeln!(stdin, "{}", json).expect("write to stdin");
            stdin.flush().expect("flush stdin");
        }

        fn recv(&mut self) -> serde_json::Value {
            let stdout = self.child.stdout.as_mut().expect("no stdout");
            let mut line = String::new();
            let reader = std::io::BufReader::new(stdout);
            // Read one line (JSON-RPC response)
            reader.lines().next().expect("no response line").expect("read error");
            // MCP may send notifications first, so read until we get a response with "id"
            // Actually, let's just read lines until we find one with "result" or "error"
            loop {
                line.clear();
                let mut reader2 = std::io::BufReader::new(self.child.stdout.as_mut().unwrap());
                reader2.read_line(&mut line).expect("read error");
                if line.contains("\"result\"") || line.contains("\"error\"") {
                    return serde_json::from_str(&line).expect(&format!("parse JSON: {}", line));
                }
            }
        }
    }

    impl Drop for McpChild {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    #[test]
    fn test_mcp_initialize() {
        let mut child = McpChild::start();

        // Send initialize request
        child.send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}}}"#);

        let resp = child.recv();
        assert_eq!(resp["jsonrpc"], "2.0");
        assert_eq!(resp["id"], 1);
        assert!(resp["result"]["serverInfo"]["name"].is_string());
        assert!(resp["result"]["capabilities"]["tools"].is_object());
    }

    #[test]
    fn test_mcp_tools_list() {
        let mut child = McpChild::start();

        // Initialize first
        child.send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}}}"#);
        let _ = child.recv();

        // Send initialized notification
        child.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);

        // List tools
        child.send(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#);
        let resp = child.recv();
        assert_eq!(resp["id"], 2);
        let tools = resp["result"]["tools"].as_array().expect("tools array");
        let tool_names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(tool_names.contains(&"codeweb_stats"));
        assert!(tool_names.contains(&"codeweb_nodes"));
        assert!(tool_names.contains(&"codeweb_trace"));
        assert!(tool_names.contains(&"codeweb_search_sql"));
        assert!(tool_names.contains(&"codeweb_node_detail"));
        assert!(tool_names.contains(&"codeweb_query"));
    }

    #[test]
    fn test_mcp_call_stats() {
        let mut child = McpChild::start();

        child.send(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}}}"#);
        let _ = child.recv();
        child.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);

        // Call codeweb_stats
        child.send(r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"codeweb_stats","arguments":{}}}"#);
        let resp = child.recv();
        assert_eq!(resp["id"], 3);
        assert!(resp["result"]["content"].is_array());
        let content = resp["result"]["content"].as_array().unwrap();
        assert!(!content.is_empty());
        assert_eq!(content[0]["type"], "text");
        let text = content[0]["text"].as_str().unwrap();
        let stats: serde_json::Value = serde_json::from_str(text).unwrap_or_else(|_| panic!("stats parse: {}", text));
        assert!(stats["edges"].is_number());
    }
}
```

**注意：** 上面的 `recv()` 实现可能需要根据 rmcp 实际的 stdio 行为调整。rmcp 使用 JSON-RPC over stdio，每行一个 JSON 消息。但 rmcp 可能在内部有 buffering 行为。如果测试中遇到问题，可能需要：
1. 使用 `--features mcp` 构建
2. 调整 `recv()` 的读取逻辑
3. 或者改为单元测试（直接调用 `McpState` 的方法）

**Step 2: 运行测试**

Run: `cargo test --features mcp --test mcp_test 2>&1`
Expected: 3 个测试通过

**Step 3: Commit**

```bash
git add tests/mcp_test.rs
git commit -m "test: add MCP server integration tests"
```

---

## Task 5: 更新文档和 feature flag 完善

**Files:**
- Modify: `Cargo.toml` — 更新 `full` feature 包含 `mcp`
- Modify: `README.md` — 添加 MCP 使用说明
- Modify: `docs/DeveloperGuide.md` — 更新 MCP 集成部分
- Modify: `CONTRIBUTION.md` — 添加 MCP 相关的测试命令

**Step 1: 更新 Cargo.toml 的 `full` feature**

```toml
full = ["cli", "tui", "serve", "mcp"]
```

**Step 2: 在 README.md 中添加 MCP 相关内容**

在 Feature Flags 表中添加：
```
| `mcp` | MCP server for LLM integration | ❌ |
```

在 CLI 命令参考表中添加：
```
| `codeweb mcp` | Start MCP server (stdio JSON-RPC for LLM clients) |
```

在 Quick Start 中添加 MCP 配置示例：
```markdown
### MCP Integration (for LLM clients)

```bash
# Build with MCP support
cargo build --features mcp

# Configure in Claude Desktop's claude_desktop_config.json:
{
  "mcpServers": {
    "codeweb": {
      "command": "/path/to/codeweb",
      "args": ["mcp", "--project", "/path/to/your/project"]
    }
  }
}
```
```

**Step 3: 更新 DeveloperGuide.md**

将现有的"MCP 集成场景"部分扩展，添加原生 MCP server 模式说明。

**Step 4: 更新 CONTRIBUTION.md**

在开发命令部分添加：
```bash
cargo test --features mcp           # run including MCP tests
cargo clippy --features mcp -- -D warnings  # lint with MCP
```

**Step 5: 验证构建**

Run: `cargo build --features full 2>&1 | tail -5`
Expected: 编译成功

Run: `cargo clippy --features full -- -D warnings 2>&1 | tail -10`
Expected: 无 warning

**Step 6: Commit**

```bash
git add Cargo.toml README.md docs/DeveloperGuide.md CONTRIBUTION.md
git commit -m "docs: add MCP feature documentation and update feature flags"
```

---

## Task 6: 端到端验证

**Files:** 无新文件

**Step 1: 运行完整测试套件**

Run: `cargo test --features full 2>&1`
Expected: 所有测试通过

**Step 2: 运行 clippy**

Run: `cargo clippy --features full -- -D warnings 2>&1`
Expected: 无 warning

**Step 3: 运行格式检查**

Run: `cargo fmt -- --check 2>&1`
Expected: 无输出（已格式化）

**Step 4: 手动 smoke test**

```bash
# 构建
cargo build --features mcp

# 手动测试 MCP 协议交互（发送 JSON-RPC 到 stdin）
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}}}' | cargo run --features mcp -- mcp --project .
```

Expected: 输出包含 initialize 响应 JSON，包含 `serverInfo.name = "codeweb"` 和 `capabilities.tools`。

**Step 5: Commit（如有修正）**

```bash
git add -A
git commit -m "fix: address clippy warnings and test issues from MCP integration"
```

---

## 总计

| Task | 内容 | 预估时间 |
|---|---|---|
| Task 1 | 添加依赖和 feature flag | 10 分钟 |
| Task 2 | 创建 mcp 模块骨架（tools + server + main.rs 集成） | 60 分钟 |
| Task 3 | 添加 ServerHandler impl | 15 分钟 |
| Task 4 | 集成测试 | 30 分钟 |
| Task 5 | 文档更新 | 20 分钟 |
| Task 6 | 端到端验证 | 15 分钟 |
| **总计** | | **~2.5 小时** |

## 风险

| 风险 | 影响 | 应对 |
|---|---|---|
| rmcp API 与文档不一致 | 需要额外调试 | Task 2 中先 `cargo check` 验证 |
| rmcp 的 tokio 版本与项目冲突 | 编译失败 | 检查 rmcp 依赖的 tokio 版本 |
| 集成测试中 stdio 交互不稳定 | 测试偶发失败 | 简化为单元测试或增加等待逻辑 |
| `schemars` derive 与现有类型不兼容 | 编译错误 | 为 tool 参数定义独立的参数类型 |
