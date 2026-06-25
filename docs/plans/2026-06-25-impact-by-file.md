# `impact --file` 子命令 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 新增 `codeweb impact --file <path> --format json` 子命令，给定文件路径，输出该文件内所有节点的上游调用者与下游被调用者，以稳定的 JSON schema 返回，供 CodeRoughcollie 子进程集成。

**Architecture:** 复用 GraphStore 已有的 `file_nodes()` 索引和 `node_key_index()` 解析层，在 `src/main.rs` 新增 `Impact` CLI 变体和 `cmd_impact()` handler。handler 按 `file → NodeKey → NodeIndex` 链路定位节点，用 `graph.neighbors_directed()` 做深度 1 的双向遍历（Incoming = 上游 callers，Outgoing = 下游 callees），从连接边的 `location.line` 提取调用行号，聚合去重后 serde 序列化为 JSON。

**Tech Stack:** Rust, clap（已有）, serde_json（已有）, petgraph（已有）, 现有 GraphStore / CodeGraph。**无新依赖。**

---

## 背景与动机

### 现状

codeweb 的所有查询入口（`trace` / `detail` / `query` / HTTP API / MCP）都以**节点名或节点 ID**为起点。引擎层有 `impact()` 函数（`store.rs:1686`），但它：
1. 只走 `.incoming()` 方向（上游 callers），**不包含下游**
2. 未暴露为任何 CLI / HTTP / MCP 出口
3. 接受 `NodeIndex`，不接受文件路径

代码审核场景的天然输入是 `git diff` 产出的**变更文件路径**，不是节点名。下游消费方 CodeRoughcollie 需要一个面向机器、schema 稳定的 CLI JSON 出口。

### 对 Issue #52 描述的修正

> 以下 3 点在实施时必须注意，Issue 原文描述有偏差：

| # | Issue 原文 | 实际情况 | 影响 |
|---|---|---|---|
| 1 | "复用引擎已有的 `impact()` 能力" | `impact()` **只做上游**（`.incoming()`），不含下游 callees | 下游需额外遍历，但 `GraphTraversal::outgoing()` 基础设施齐全，工作量不大 |
| 2 | "`upstream[].line` = 调用发生行号" | 行号来自**边的 `location.line`**，不是节点定义行 | 实现时需从 `graph.edges_connecting(from, to)` 提取边位置 |
| 3 | 未提及深度/传递性 | `impact()` 接受 `max_depth`；审核场景默认深度 1 最合理 | CLI 加 `--depth N`，默认 1 |

### 设计原则

1. **直接遍历而非调用 `impact()`**：`impact()` 只返回 `Vec<NodeIndex>`，丢失边信息（行号）。本命令需要边上的 `location.line`，因此直接用 `graph.neighbors_directed()` + `graph.edges_connecting()` 做深度 1 遍历
2. **默认深度 1**：审核场景关心直接调用者/被调用者。`--depth N` 可选扩展
3. **Calls-only 边过滤**：只跟踪调用类边（`EdgeCategory::Call`），忽略 Composition / DataFlow / Reference / Inheritance
4. **契约冻结**：JSON schema 一次发布即冻结，加 `schema_version` 字段

---

## 范围

### MVP 范围（必做）

| 能力 | 说明 |
|---|---|
| `--file <path>` 文件入口 | 支持相对路径（相对 CWD 或项目根）和绝对路径 |
| 上游 callers 聚合 | 文件内所有节点的直接上游，去重 |
| 下游 callees 聚合 | 文件内所有节点的直接下游，去重 |
| `--format json` | 稳定 JSON schema，serde 序列化 |
| `--format text` | 人类可读文本输出 |
| `--depth N` | 遍历深度，默认 1 |
| 文件不在图中 | 退出码 0 + 空数组 + stderr 提示 |

### 明确不做（Out of Scope）

- ❌ 传递闭包的全量 blast radius（深度 >1 的聚合结果语义复杂，留后续）
- ❌ HTTP API `/api/v1/impact` 端点（如 CodeRoughcollie 需要 HTTP 模式再补）
- ❌ MCP tool `codeweb_impact`（同上）
- ❌ 按 edge 类型分桶输出（当前 `upstream/downstream` 数组扁平聚合）

---

## JSON 输出 Schema（契约冻结）

```jsonc
{
  "schema_version": 1,
  "file": "src/main/java/com/example/Mapper.java",
  "upstream": [
    {
      "file_path": "src/main/java/com/example/OrderService.java",
      "symbol": "placeOrder",
      "line": 42
    }
  ],
  "downstream": [
    {
      "file_path": "sql/pkg_orders.sql",
      "symbol": "proc_create_order",
      "line": null
    }
  ]
}
```

| 字段 | 类型 | 说明 |
|---|---|---|
| `schema_version` | number | 固定 `1`，schema 变更时递增 |
| `file` | string | 回显输入的文件路径（规范化后） |
| `upstream` | array | 上游调用者（调用本文件内节点的那些节点），去重 |
| `downstream` | array | 下游被调用者（本文件内节点调用的那些节点），去重 |
| `*.file_path` | string | 调用方/被调用方所在文件路径（相对项目根），无位置信息时为 `null` |
| `*.symbol` | string | 调用方/被调用方符号名（NodeKey 的 Display） |
| `*.line` | number \| null | 调用发生的行号（来自边的 location），无则 `null` |

**去重规则**：以 `(file_path, symbol)` 二元组去重，保留第一次出现时的 `line`。

---

## 高层设计

### 数据流

```
 --file <path>
      │
      ▼
┌─────────────────────────────────┐
│ [1] 路径规范化                   │  ← canonicalize + strip_prefix
│     绝对路径 → 项目相对路径       │
└────────────┬────────────────────┘
             │
             ▼
┌─────────────────────────────────┐
│ [2] 文件→节点查找                 │  ← store.file_nodes().get(&path)
│     → Vec<NodeKey>               │
└────────────┬────────────────────┘
             │
             ▼
┌─────────────────────────────────┐
│ [3] NodeKey→NodeIndex 解析       │  ← store.node_key_index()
│     → Vec<NodeIndex>             │
└────────────┬────────────────────┘
             │
             ▼
┌─────────────────────────────────┐
│ [4] 双向遍历（深度 N，默认 1）    │
│     Incoming → upstream callers  │
│     Outgoing → downstream callees│
│     提取边 location.line         │
└────────────┬────────────────────┘
             │
             ▼
┌─────────────────────────────────┐
│ [5] 聚合去重 + serde JSON        │
│     按 (file_path, symbol) 去重  │
└─────────────────────────────────┘
```

### 路径匹配策略

`file_nodes` 的 key 是**绝对路径**（与 `manifest` 一致）。用户传入的 `--file` 可能是：

| 输入形式 | 处理方式 |
|---|---|
| 绝对路径 `/foo/src/Main.java` | 直接匹配 |
| 相对路径 `src/Main.java` | 先尝试 CWD 拼接为绝对路径 |
| 项目根相对路径 | 拼接 `proj.root()` 为绝对路径 |

匹配策略：先 `canonicalize()` 输入路径，在 `file_nodes` 中精确查找；若未命中，遍历 `file_nodes` 的 key 做 `ends_with(suffix)` 模糊匹配（取第一个命中），并在 stderr 提示使用了模糊匹配。

---

## 实现步骤

### Task 1: 定义 JSON 输出结构体

**Files:**
- Modify: `src/main.rs`（在文件顶部 `mod` 声明之后、`Commands` enum 之前添加结构体）

**Step 1: 定义 serde 结构体**

在 `src/main.rs` 中添加（紧跟现有的 `use` 块和 `struct Cli` 之后）：

```rust
use serde::Serialize;

/// JSON 输出 schema 的单个上游/下游条目
#[derive(Serialize, PartialEq, Eq, Hash, Clone)]
struct ImpactEntry {
    file_path: Option<String>,
    symbol: String,
    line: Option<usize>,
}

/// `impact --file` 的完整 JSON 输出
#[derive(Serialize)]
struct ImpactResult {
    schema_version: u32,
    file: String,
    upstream: Vec<ImpactEntry>,
    downstream: Vec<ImpactEntry>,
}
```

**Step 2: 编译验证**

Run: `cargo build`
Expected: 编译通过（结构体未被使用，可能触发 `dead_code` warning，后续 Task 消除）

**Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat(impact): add JSON output schema structs for impact --file"
```

---

### Task 2: 添加 `Impact` CLI 变体

**Files:**
- Modify: `src/main.rs` — `Commands` enum（约 line 104）和 `match cli.command` 分发（约 line 453）

**Step 1: 在 `Commands` enum 末尾（`Partition` 之后）添加 `Impact` 变体**

```rust
    /// Show upstream callers and downstream callees for all nodes in a file
    ///
    /// Aggregates impact analysis across all nodes (methods, mappers, procedures)
    /// defined in the given file. Designed for subprocess integration (e.g.
    /// CodeRoughcollie) — outputs stable JSON by default.
    Impact {
        /// File path to analyze (relative to CWD or absolute)
        #[arg(short, long)]
        file: PathBuf,

        /// Project directory (default: current directory)
        #[arg(short, long, default_value = ".")]
        project: PathBuf,

        /// Output format (json for integration, text for human reading)
        #[arg(short, long, default_value = "json", value_parser = ["json", "text"])]
        format: String,

        /// Traversal depth (1 = direct callers/callees only)
        #[arg(short, long, default_value = "1")]
        depth: usize,
    },
```

**Step 2: 在 `match cli.command` 分发中添加分支**

在 `Some(Commands::Partition { ... }) => ...` 之后添加：

```rust
        Some(Commands::Impact {
            file,
            project,
            format,
            depth,
        }) => cmd_impact(&file, &project, &format, depth),
```

**Step 3: 添加空的 handler 占位**

在 `cmd_partition()` 之后添加：

```rust
fn cmd_impact(file: &Path, project: &Path, format: &str, depth: usize) -> Result<()> {
    let _ = (file, project, format, depth);
    todo!("impact --file implementation")
}
```

**Step 4: 编译验证**

Run: `cargo build`
Expected: 编译通过

Run: `cargo run -- impact --help`
Expected: 显示 impact 子命令的帮助信息

**Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat(impact): add Impact CLI variant and clap args"
```

---

### Task 3: 实现路径查找辅助函数

**Files:**
- Modify: `src/main.rs` — 在 `cmd_impact()` 之前

**Step 1: 实现 `resolve_file_path` 函数**

```rust
/// 将用户输入的文件路径规范化，匹配 file_nodes 中的绝对路径 key。
///
/// 尝试顺序：
/// 1. canonicalize 输入路径，精确匹配
/// 2. 以 ends_with 模糊匹配（处理相对路径输入）
fn resolve_file_path<'a>(
    input: &Path,
    file_nodes: &'a HashMap<PathBuf, Vec<crate::graph::key::NodeKey>>,
) -> Option<&'a PathBuf> {
    use std::path::Path;

    // 策略 1：canonicalize 后精确匹配
    if let Ok(canon) = input.canonicalize() {
        if file_nodes.contains_key(canon.as_path()) {
            return file_nodes.get_key_value(canon.as_path()).map(|(k, _)| k);
        }
    }

    // 策略 2：输入已是绝对路径，直接查
    if input.is_absolute() && file_nodes.contains_key(input) {
        return file_nodes.get_key_value(input).map(|(k, _)| k);
    }

    // 策略 3：ends_with 模糊匹配（取第一个命中）
    let input_str = input.to_string_lossy();
    for key in file_nodes.keys() {
        if key.to_string_lossy().ends_with(input_str.as_ref()) {
            return Some(key);
        }
    }

    None
}
```

**Step 2: 编译验证**

Run: `cargo build`
Expected: 编译通过

**Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat(impact): add file path resolution helper"
```

---

### Task 4: 实现 `cmd_impact()` 核心逻辑

**Files:**
- Modify: `src/main.rs` — 替换 Task 2 中的 `todo!()` 占位

**Step 1: 实现完整的 `cmd_impact()`**

```rust
fn cmd_impact(file: &Path, project: &Path, format: &str, depth: usize) -> Result<()> {
    use crate::graph::query::filter::EdgeFilter;
    use petgraph::Direction;

    let mut proj = project::Project::find(project)?;
    let store = proj.load_store()?;

    let store = match store {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Project not analyzed. Run `codeweb analyze` first.");
            return Ok(());
        }
    };

    let graph = store.graph();
    let file_nodes = store.file_nodes();
    let key_index = store.node_key_index();

    // 1. 定位文件
    let matched_path = match resolve_file_path(file, file_nodes) {
        Some(p) => p,
        None => {
            // 文件不在图中：空数组 + 退出码 0 + stderr 提示
            let result = ImpactResult {
                schema_version: 1,
                file: file.to_string_lossy().to_string(),
                upstream: vec![],
                downstream: vec![],
            };
            let json = serde_json::to_string_pretty(&result).map_err(|e| {
                error::CodeWebError::ExportError {
                    message: format!("JSON serialization: {}", e),
                }
            })?;
            println_stdout!("{}", json);
            eprintln!("Warning: file '{}' not found in graph (no nodes analyzed for this file)", file.display());
            return Ok(());
        }
    };

    // 2. 文件 → NodeKey → NodeIndex
    let node_keys = file_nodes.get(matched_path);
    let file_node_indices: Vec<NodeIndex> = match node_keys {
        Some(keys) => keys
            .iter()
            .filter_map(|k| key_index.get(k).copied())
            .collect(),
        None => vec![],
    };

    if file_node_indices.is_empty() {
        let result = ImpactResult {
            schema_version: 1,
            file: matched_path.to_string_lossy().to_string(),
            upstream: vec![],
            downstream: vec![],
        };
        let json = serde_json::to_string_pretty(&result)
            .map_err(|e| error::CodeWebError::ExportError {
                message: format!("JSON serialization: {}", e),
            })?;
        if format == "json" {
            println_stdout!("{}", json);
        } else {
            print_impact_text(&result);
        }
        eprintln!("Warning: file '{}' has no graph nodes", matched_path.display());
        return Ok(());
    }

    // 3. 双向遍历 + 边位置提取
    let calls_filter = EdgeFilter::calls_only();
    let mut upstream_set: std::collections::HashSet<ImpactEntry> = std::collections::HashSet::new();
    let mut downstream_set: std::collections::HashSet<ImpactEntry> = std::collections::HashSet::new();

    collect_impact_entries(
        graph,
        &file_node_indices,
        Direction::Incoming,
        depth,
        &calls_filter,
        &mut upstream_set,
    );
    collect_impact_entries(
        graph,
        &file_node_indices,
        Direction::Outgoing,
        depth,
        &calls_filter,
        &mut downstream_set,
    );

    // 4. 排序 + 输出（按 file_path 然后 symbol 排序，保证输出稳定）
    let mut upstream: Vec<ImpactEntry> = upstream_set.into_iter().collect();
    let mut downstream: Vec<ImpactEntry> = downstream_set.into_iter().collect();
    upstream.sort_by(|a, b| (&a.file_path, &a.symbol).cmp(&(&b.file_path, &b.symbol)));
    downstream.sort_by(|a, b| (&a.file_path, &a.symbol).cmp(&(&b.file_path, &b.symbol)));

    let result = ImpactResult {
        schema_version: 1,
        file: matched_path.to_string_lossy().to_string(),
        upstream,
        downstream,
    };

    if format == "json" {
        let json = serde_json::to_string_pretty(&result)
            .map_err(|e| error::CodeWebError::ExportError {
                message: format!("JSON serialization: {}", e),
            })?;
        println_stdout!("{}", json);
    } else {
        print_impact_text(&result);
    }

    Ok(())
}
```

**Step 2: 实现 `collect_impact_entries` 辅助函数**

```rust
/// 从一组节点出发，沿指定方向遍历，收集影响条目。
///
/// - `direction = Incoming`：收集上游调用者
/// - `direction = Outgoing`：收集下游被调用者
fn collect_impact_entries(
    graph: &crate::graph::CodeGraph,
    start_nodes: &[NodeIndex],
    direction: petgraph::Direction,
    depth: usize,
    edge_filter: &crate::graph::query::filter::EdgeFilter,
    out: &mut std::collections::HashSet<ImpactEntry>,
) {
    use crate::graph::key::NodeKey;

    let mut visited: std::collections::HashSet<NodeIndex> = start_nodes.iter().copied().collect();

    // BFS 层序遍历
    let mut frontier: Vec<NodeIndex> = start_nodes.to_vec();

    for _depth in 0..depth {
        let mut next_frontier: Vec<NodeIndex> = vec![];

        for &node in &frontier {
            let neighbors: Vec<_> = graph
                .neighbors_directed(node, direction)
                .collect();

            for neighbor in neighbors {
                if visited.contains(&neighbor) {
                    continue;
                }

                // 检查边是否通过过滤器
                let (from, to) = match direction {
                    petgraph::Direction::Outgoing => (node, neighbor),
                    petgraph::Direction::Incoming => (neighbor, node),
                };

                // 找到第一条通过过滤的边，提取其 location.line
                let edge_line = graph
                    .edges_connecting(from, to)
                    .find(|e| edge_filter.matches(e.weight()))
                    .and_then(|e| edge_location_line(e.weight()));

                let edge_matches = graph
                    .edges_connecting(from, to)
                    .any(|e| edge_filter.matches(e.weight()));

                if !edge_matches {
                    continue;
                }

                visited.insert(neighbor);

                // 提取 neighbor 的 file_path 和 symbol
                let neighbor_node = &graph[neighbor];
                let file_path = node_source_file(neighbor_node)
                    .map(|p| p.to_string_lossy().to_string());
                let symbol = NodeKey::from_node(neighbor_node)
                    .map(|k| k.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string());

                out.insert(ImpactEntry {
                    file_path,
                    symbol,
                    line: edge_line,
                });

                next_frontier.push(neighbor);
            }
        }

        frontier = next_frontier;
        if frontier.is_empty() {
            break;
        }
    }
}
```

**Step 3: 实现 `edge_location_line` 和 `print_impact_text` 辅助函数**

```rust
/// 从边上提取行号（调用发生行）
fn edge_location_line(edge: &crate::graph::Edge) -> Option<usize> {
    use crate::graph::Edge;
    match edge {
        Edge::DirectCall { location, .. } => Some(location.line),
        Edge::DynamicCall { location, .. } => Some(location.line),
        Edge::CallsProcedure { location, .. } => Some(location.line),
        Edge::InvokesMapper { location, .. } => Some(location.line),
        Edge::CallsJava { location, .. } => Some(location.line),
        Edge::Extends { location, .. } => Some(location.line),
        Edge::Implements { location, .. } => Some(location.line),
        Edge::TableAccess { location, .. } => Some(location.line),
        Edge::DependsOn { location, .. } => Some(location.line),
        Edge::TriggersRoutine { location, .. } => Some(location.line),
        Edge::ReferencesType { location, .. } => Some(location.line),
        Edge::UsesSequence { location, .. } => Some(location.line),
        Edge::IndexesTable { location, .. } => Some(location.line),
        Edge::AliasesObject { location, .. } => Some(location.line),
        Edge::CustomEdge { location, .. } => location.as_ref().map(|l| l.line),
        // 无 location 的边类型
        Edge::ContainsMethod | Edge::ContainsSql | Edge::ContainsRoutine => None,
    }
}

/// text 格式输出
fn print_impact_text(result: &ImpactResult) {
    println_stdout!("File: {}", result.file);
    println_stdout!();
    println_stdout!("── UPSTREAM ({}) ──", result.upstream.len());
    if result.upstream.is_empty() {
        println_stdout!("  (none)");
    } else {
        for entry in &result.upstream {
            let line_tag = entry.line.map(|l| format!(":{}", l)).unwrap_or_default();
            let file = entry.file_path.as_deref().unwrap_or("<unknown>");
            println_stdout!("  {}  {}{}", entry.symbol, file, line_tag);
        }
    }
    println_stdout!();
    println_stdout!("── DOWNSTREAM ({}) ──", result.downstream.len());
    if result.downstream.is_empty() {
        println_stdout!("  (none)");
    } else {
        for entry in &result.downstream {
            let line_tag = entry.line.map(|l| format!(":{}", l)).unwrap_or_default();
            let file = entry.file_path.as_deref().unwrap_or("<unknown>");
            println_stdout!("  {}  {}{}", entry.symbol, file, line_tag);
        }
    }
}
```

**Step 4: 编译验证**

Run: `cargo build`
Expected: 编译通过，无 warning

Run: `cargo clippy -- -D warnings`
Expected: 无 clippy 错误

**Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat(impact): implement cmd_impact with bidirectional traversal"
```

---

### Task 5: 集成测试

**Files:**
- Create: `tests/impact_test.rs`

**Step 1: 创建测试 fixture 和集成测试**

```rust
use assert_cmd::Command;
use predicates::str::contains;
use serde_json::Value;

/// 辅助：在临时目录创建一个最小的 codeweb 项目并分析
fn setup_test_project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();

    // 创建 SQL 文件：存储过程 A 调用存储过程 B
    std::fs::write(
        dir.path().join("proc_a.sql"),
        r#"
CREATE OR REPLACE PROCEDURE proc_a() AS
BEGIN
    CALL proc_b();
END;
/
"#,
    )
    .unwrap();

    std::fs::write(
        dir.path().join("proc_b.sql"),
        r#"
CREATE OR REPLACE PROCEDURE proc_b() AS
BEGIN
    NULL;
END;
/
"#,
    )
    .unwrap();

    // 初始化并分析项目
    Command::cargo_bin("codeweb")
        .unwrap()
        .current_dir(dir.path())
        .args(["init", "test-project", "-d", "."])
        .assert()
        .success();

    dir
}

#[test]
fn test_impact_file_json_output_schema() {
    let dir = setup_test_project();

    // proc_a.sql 调用了 proc_b，所以对 proc_a.sql 查询：
    // - upstream 应为空（没人调用 proc_a）
    // - downstream 应包含 proc_b
    let output = Command::cargo_bin("codeweb")
        .unwrap()
        .current_dir(dir.path())
        .args(["impact", "--file", "proc_a.sql", "--format", "json"])
        .output()
        .unwrap();

    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: Value = serde_json::from_str(&stdout).expect("output must be valid JSON");

    assert_eq!(json["schema_version"], 1);
    assert!(json["file"].as_str().unwrap().ends_with("proc_a.sql"));
    assert!(json["upstream"].is_array());
    assert!(json["downstream"].is_array());

    // downstream 应包含 proc_b
    let downstream = json["downstream"].as_array().unwrap();
    assert!(
        downstream
            .iter()
            .any(|e| e["symbol"].as_str().unwrap().contains("proc_b")),
        "downstream should contain proc_b: {:?}",
        downstream
    );
}

#[test]
fn test_impact_file_not_in_graph() {
    let dir = setup_test_project();

    let output = Command::cargo_bin("codeweb")
        .unwrap()
        .current_dir(dir.path())
        .args(["impact", "--file", "nonexistent.sql", "--format", "json"])
        .output()
        .unwrap();

    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: Value = serde_json::from_str(&stdout).expect("output must be valid JSON");

    assert_eq!(json["schema_version"], 1);
    assert!(json["upstream"].as_array().unwrap().is_empty());
    assert!(json["downstream"].as_array().unwrap().is_empty());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("not found in graph"),
        "stderr should warn about missing file: {}",
        stderr
    );
}

#[test]
fn test_impact_file_text_format() {
    let dir = setup_test_project();

    let output = Command::cargo_bin("codeweb")
        .unwrap()
        .current_dir(dir.path())
        .args(["impact", "--file", "proc_b.sql", "--format", "text"])
        .output()
        .unwrap();

    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    // proc_b 被 proc_a 调用，所以 upstream 应包含 proc_a
    assert!(stdout.contains("UPSTREAM"), "should have UPSTREAM section");
    assert!(
        stdout.contains("proc_a"),
        "upstream should contain proc_a: {}",
        stdout
    );
}

#[test]
fn test_impact_reverse_direction() {
    let dir = setup_test_project();

    // 对 proc_b.sql 查询：proc_b 被 proc_a 调用
    // - upstream 应包含 proc_a
    // - downstream 应为空
    let output = Command::cargo_bin("codeweb")
        .unwrap()
        .current_dir(dir.path())
        .args(["impact", "--file", "proc_b.sql", "--format", "json"])
        .output()
        .unwrap();

    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: Value = serde_json::from_str(&stdout).expect("output must be valid JSON");

    let upstream = json["upstream"].as_array().unwrap();
    assert!(
        upstream
            .iter()
            .any(|e| e["symbol"].as_str().unwrap().contains("proc_a")),
        "upstream of proc_b should contain proc_a: {:?}",
        upstream
    );
}
```

**Step 2: 检查 dev-dependencies**

确认 `Cargo.toml` 的 `[dev-dependencies]` 中已有 `assert_cmd`、`tempfile`、`serde_json`。若缺失则添加：

```toml
[dev-dependencies]
assert_cmd = "2"
tempfile = "3"
serde_json = "1"
```

**Step 3: 运行测试**

Run: `cargo test --test impact_test`
Expected: 4 个测试全部通过

**Step 4: Commit**

```bash
git add tests/impact_test.rs Cargo.toml
git commit -m "test(impact): add integration tests for impact --file command"
```

---

### Task 6: 更新 README

**Files:**
- Modify: `README.md`

**Step 1: 在 CLI Reference 表中添加 `impact` 行**

在英文版 "CLI Reference" 表中（`codeweb dedup` 行之后）添加：

```markdown
| `codeweb impact --file <path>` | Show upstream/downstream impact for all nodes in a file |
```

在中文版 "CLI 命令参考" 表中对应位置添加：

```markdown
| `codeweb impact --file <path>` | 显示文件内所有节点的上下游影响 |
```

**Step 2: 在英文版 Quick Start 中添加示例**

在 `codeweb dedup` 示例之后添加：

```bash
# Show impact analysis for a file (JSON output for integration)
codeweb impact --file src/main/java/com/example/Mapper.java --format json
```

**Step 3: Commit**

```bash
git add README.md
git commit -m "docs: add impact --file to CLI reference"
```

---

### Task 7: 最终验证

**Step 1: 全 feature 编译**

Run: `cargo build --features full`
Expected: 编译通过

**Step 2: 全 feature 测试**

Run: `cargo test --features full`
Expected: 所有测试通过（含新增 impact_test 的 4 个测试）

**Step 3: Clippy**

Run: `cargo clippy --features full -- -D warnings`
Expected: 无错误

**Step 4: 格式检查**

Run: `cargo fmt -- --check`
Expected: 无差异

**Step 5: 手动冒烟测试**

```bash
# 在已有项目中测试
codeweb impact --file <某个已知文件> --format json | jq .
codeweb impact --file <某个已知文件> --format text
codeweb impact --file /nonexistent --format json
```

---

## 验收标准

- [ ] `codeweb impact --file <path> --format json` 输出合法 JSON，符合上述 schema
- [ ] 文件不在图中时退出码 0 + 空数组 + stderr 提示（不 panic、不非零退出）
- [ ] `--format text` 提供人类可读输出
- [ ] `--depth N` 控制遍历深度，默认 1
- [ ] 上游 callers 和下游 callees 均正确（双向）
- [ ] 单测 + 集成测试覆盖
- [ ] `cargo fmt` / `cargo clippy --features full -- -D warnings` / `cargo test --features full` 全绿
- [ ] README 的 CLI Reference 表补充 `impact` 条目

---

## 关联

- Issue: #52
- 下游集成方：CodeRoughcollie `.sisyphus/plans/codeweb-impact-integration.md`（Task 1 依赖本 issue 落地后 bump submodule）
