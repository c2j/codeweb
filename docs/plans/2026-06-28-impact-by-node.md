# `impact --node` 子命令 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 为 `codeweb impact` 命令新增 `--node <name>` 入口,与现有 `--file <path>` 二选一,让用户可以直接按节点符号名查询其上下游影响,而不必先定位文件。

**Architecture:** 复用现有 `cmd_impact` 的双向遍历核心(`collect_impact_entries`),新增一条"节点名 → NodeIndex"的解析路径。节点名解析复用 `store.search_nodes(name)` —— 与 `trace` / `detail` 完全一致的多匹配模糊查找。JSON schema 从 v1 升级到 v2:`file` 和新增的 `node` 字段在 Rust 侧均为 `Option<String>`,通过 `skip_serializing_if = "Option::is_none"` 在序列化时互斥出现 —— `--file` 输出只含 `file` 字段(无 `node`),`--node` 输出只含 `node` 字段(无 `file`),二者均不为 `null`。v1 消费方(CodeRoughcollie 用 `--file`)只需更新 `schema_version` 校验,`file` 字段解析逻辑无需改动。

**Tech Stack:** Rust, clap(已有), serde_json(已有), petgraph(已有)。**无新依赖。**

---

## 背景与动机

### 现状

`codeweb impact` 当前只接受 `--file <path>` 入口(见 `docs/plans/2026-06-25-impact-by-file.md`)。该设计是为 CodeRoughcollie 子进程集成优化 —— 它的输入来自 `git diff`,自然是文件路径。

但其他所有查询入口(`trace --from <node>`、`detail <node>`、HTTP `/nodes/:id`、MCP `codeweb_node_detail`)都以**节点名**为输入。当用户/LLM 想问"这个存储过程被谁调用、调用了谁"时,强制先找文件是反人体工学的。

引擎层 `store.rs:1687 impact(node: NodeIndex, max_depth)` 本就按节点工作;`collect_impact_entries` 的签名是 `start_nodes: &[NodeIndex]`,单节点就是长度 1 的切片 —— **核心遍历逻辑零改动**。

### 设计原则

1. **`--file` 和 `--node` 互斥**:clap 层面强制二选一,同时给出或都不给出 → 报错退出
2. **节点名解析与 `trace`/`detail` 一致**:复用 `store.search_nodes(name)`,多匹配时打 stderr 警告 + 用第一个,零匹配时空结果 + stderr 警告 + 退出码 0
3. **JSON schema 平滑升级到 v2**:`file` 和 `node` 通过 `skip_serializing_if` 互斥出现(不为 `null`)。v1 消费方只需更新 `schema_version` 校验
4. **核心遍历逻辑零改动**:`collect_impact_entries` 签名不变

---

## 范围

### MVP 范围(必做)

| 能力 | 说明 |
|---|---|
| `--node <name>` 节点入口 | 与 `--file` 互斥,支持模糊匹配 |
| 节点名解析 | 复用 `search_nodes`,多匹配时用第一个 + stderr 警告 |
| JSON schema v2 | `file: Option<String>` + 新增 `node: Option<String>` |
| `--format text` 兼容 | text 模式下 `--node` 时显示 `Node: <name>` 而非 `File: <path>` |
| `--depth N` | 不变,沿用现有参数 |
| 节点不在图中 | 退出码 0 + 空数组 + stderr 提示(与 `--file` 行为对称) |

### 明确不做(Out of Scope)

- ❌ 多节点批量查询(`--node a,b,c`):YAGNI,需要时再加
- ❌ HTTP API `/api/v1/impact` 端点:现有 `/nodes/:id/callers` + `/nodes/:id/callees` 已覆盖该场景
- ❌ MCP tool `codeweb_impact`:同上,MCP 侧 `codeweb_node_detail` 已返回 callers/callees
- ❌ schema v1 兼容输出标志(`--schema-v1`):v2 是 v1 的超集,无需降级

---

## JSON 输出 Schema v2(契约)

```jsonc
// --file 调用(file 字段存在,node 字段被 skip_serializing_if 省略 —— 不为 null)
{
  "schema_version": 2,
  "file": "src/main/java/com/example/Mapper.java",
  "upstream": [
    { "file_path": "src/main/java/com/example/OrderService.java", "symbol": "placeOrder", "line": 42 }
  ],
  "downstream": [
    { "file_path": "sql/pkg_orders.sql", "symbol": "proc_create_order", "line": null }
  ]
}

// --node 调用(node 字段存在,file 字段被 skip_serializing_if 省略 —— 不为 null)
{
  "schema_version": 2,
  "node": "proc_create_order",
  "upstream": [...],
  "downstream": [...]
}
```

| 字段 | v1 | v2 | 变更 |
|---|---|---|---|
| `schema_version` | `1` | `2` | 递增(唯一 breaking change:v1 消费方的版本校验需更新) |
| `file` | `string`(总非空) | `string`(`--file` 时存在,`--node` 时**字段省略**) | `--file` 路径下行为不变;`--node` 路径下字段被 `skip_serializing_if` 省略,不为 `null` |
| `node` | (不存在) | `string`(`--node` 时存在,`--file` 时**字段省略**) | **新增**;`--file` 路径下字段被省略,不为 `null` |
| `upstream` / `downstream` | array | array | 不变 |

**v1 → v2 消费方迁移**:CodeRoughcollie 当前只调 `--file`,其 JSON 输出里 `file` 字段**始终为字符串**(v1 行为不变),`node` 字段不出现。唯一需要适配的是 `schema_version` 从 `1` 变为 `2` —— 消费方将版本断言从 `== 1` 改为 `>= 1`(或 `== 2`)即可,`file` 字段解析逻辑无需改动。若消费方后续需要消费 `--node` 输出,则新增对 `node` 字段的处理(同样为非 null 字符串)。

---

## 高层设计

### 数据流

```
 --file <path>  ─┐                  ┌─ resolve_file_path()
                 │                   ├─ file_nodes[key]
                 ├─→ [路径解析] ──┐  │
 --node <name> ─┘                  │  ├─ search_nodes(name)[0]
                 │                  │  │   (多匹配时取第一个)
                 │                  │  │
                 ▼                  ▼  ▼
              ┌─────────────────────────────┐
              │ start_nodes: Vec<NodeIndex> │  ← 长度 1(--node)或 N(--file)
              └────────────┬────────────────┘
                           │
              ┌────────────▼────────────────┐
              │ collect_impact_entries()    │  ← **零改动**
              │ Incoming → upstream         │
              │ Outgoing → downstream       │
              └────────────┬────────────────┘
                           │
              ┌────────────▼────────────────┐
              │ ImpactResult { v2 schema }  │
              │ serde JSON or text output   │
              └─────────────────────────────┘
```

### `--node` 解析策略

直接调 `store.search_nodes(name)`(与 `cmd_trace` 一致):

| 匹配数 | 行为 | 退出码 |
|---|---|---|
| 0 | stderr `No nodes matching '<name>'`,stdout 空 schema v2 JSON | 0 |
| 1 | 直接用 | 0 |
| >1 | stderr 列出所有匹配 + `Using first match: <name>`,用第一个 | 0 |

---

## 实现步骤

### Task 1: 升级 `ImpactResult` / `ImpactEntry` schema 结构体

**Files:**
- Modify: `src/main.rs` — `ImpactResult` 结构体定义(约 line 56-71)

**Step 1: 修改结构体定义**

将 `src/main.rs:56-71` 附近:

```rust
#[derive(Serialize)]
struct ImpactResult {
    schema_version: u32,
    file: String,
    upstream: Vec<ImpactEntry>,
    downstream: Vec<ImpactEntry>,
}
```

改为:

```rust
#[derive(Serialize)]
struct ImpactResult {
    schema_version: u32,
    /// `--file` 入口时为 Some,`--node` 入口时为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    /// `--node` 入口时为 Some,`--file` 入口时为 None
    #[serde(skip_serializing_if = "Option::is_none")]
    node: Option<String>,
    upstream: Vec<ImpactEntry>,
    downstream: Vec<ImpactEntry>,
}
```

> **注意 `skip_serializing_if`**:用 `Option::is_none` 跳过未设置的字段,使 `--file` 时输出里不出现 `"node": null`,反之亦然 —— 输出更干净,且 v1 消费方解析 `--file` 输出时完全不受新字段影响。
>
> **如果希望显式输出 null**(更严格),去掉两个 `skip_serializing_if` 属性即可。本计划选择 skip 以获得更干净输出。

**Step 2: cargo build 查看编译错误**

Run: `cargo build 2>&1 | head -50`

Expected: 多处 `ImpactResult { file: "..." }` 构造点报类型不匹配错误(分布在 `cmd_impact` 内)。**此时不修**,Task 4 统一处理。

**Step 3: Commit**

```bash
git add src/main.rs
git commit -m "feat(impact): upgrade ImpactResult schema to v2 (file nullable + node field)"
```

> Task 1 单独提交会让中间状态编译失败。如果希望每步可编译,改为:在本 Task 内把所有 `ImpactResult { file: "..." }` 构造点改为 `file: Some("...".to_string()), node: None`,再提交。

---

### Task 2: 添加 `--node` CLI 参数

**Files:**
- Modify: `src/main.rs` — `Commands::Impact` enum 变体(约 line 435-451)

**Step 1: 改 `file` 为 `Option<PathBuf>` 并新增 `node: Option<String>`**

将 line 435-451 的 `Impact { ... }` 变体改为:

```rust
    /// Show upstream callers and downstream callees for a file or a node
    ///
    /// Pass `--file <path>` to aggregate impact across all nodes defined in
    /// that file (useful for subprocess integration with `git diff`).
    /// Pass `--node <name>` to query a single symbol (e.g. a procedure,
    /// Java method, or mapper id). The two flags are mutually exclusive.
    Impact {
        /// File path to analyze (relative to CWD or absolute).
        /// Mutually exclusive with --node.
        #[arg(long)]
        file: Option<PathBuf>,

        /// Node symbol name to analyze (e.g. "proc_create_order").
        /// Supports fuzzy match like `trace`/`detail`. Mutually exclusive with --file.
        #[arg(long)]
        node: Option<String>,

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

**Step 2: 更新 `match cli.command` 分发(约 line 634)**

将:

```rust
        Some(Commands::Impact {
            file,
            project,
            format,
            depth,
        }) => cmd_impact(file, project, format, depth),
```

改为:

```rust
        Some(Commands::Impact {
            file,
            node,
            project,
            format,
            depth,
        }) => {
            // 互斥校验:恰好一个必须是 Some
            match (file, node) {
                (Some(_), Some(_)) => {
                    eprintln!("Error: --file and --node are mutually exclusive. Pass exactly one.");
                    std::process::exit(2);
                }
                (None, None) => {
                    eprintln!("Error: must pass exactly one of --file <path> or --node <name>.");
                    std::process::exit(2);
                }
                _ => cmd_impact(file.as_deref(), node.as_deref(), project, format, depth),
            }
        }
```

**Step 3: 更新 `cmd_impact` 签名(临时占位,先让编译过)**

将 `cmd_impact` 签名(line ~2237)改为:

```rust
fn cmd_impact(
    file: Option<&Path>,
    node: Option<&str>,
    project: &Path,
    format: &str,
    depth: usize,
) -> Result<()> {
    let _ = (file, node, project, format, depth);
    todo!("impact dispatch — implemented in Task 4")
}
```

**Step 4: 编译 + 验证 clap 帮助**

Run: `cargo build`
Expected: 编译通过(`todo!` 不会在 build 时触发)

Run: `cargo run -- impact --help`
Expected: 帮助文本同时显示 `--file` 和 `--node`,且说明互斥

Run: `cargo run -- impact`
Expected: 退出码 2 + stderr `Error: must pass exactly one of --file <path> or --node <name>.`

Run: `cargo run -- impact --file x --node y`
Expected: 退出码 2 + stderr `Error: --file and --node are mutually exclusive...`

**Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat(impact): add --node CLI flag with mutual exclusion vs --file"
```

---

### Task 3: 实现 `cmd_impact` 分发与 `--node` 路径

**Files:**
- Modify: `src/main.rs` — `cmd_impact` 函数体(替换 Task 2 的 `todo!()`)

**Step 1: 实现完整的 `cmd_impact` 分发**

替换 Task 2 中的 `todo!()` 占位为:

```rust
fn cmd_impact(
    file: Option<&Path>,
    node: Option<&str>,
    project: &Path,
    format: &str,
    depth: usize,
) -> Result<()> {
    use crate::graph::query::filter::EdgeFilter;
    use petgraph::Direction;

    let mut proj = project::Project::find(project)?;
    // Note: `load_store()` returns single-layer `Result<&GraphStore>` (not nested).
    // See project/mod.rs:420. Match without `?` to handle "not yet analyzed" gracefully.
    let store = match proj.load_store() {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Project not analyzed. Run `codeweb analyze` first.");
            return Ok(());
        }
    };

    let graph = store.graph();
    let calls_filter = EdgeFilter::calls_only();

    // ── 解析起点节点 ───────────────────────────────────────────────
    // 返回 (start_nodes, ImpactTarget)
    let (start_nodes, target) = match (file, node) {
        (Some(path), None) => {
            let file_nodes = store.file_nodes();
            let key_index = store.node_key_index();
            resolve_file_target(graph, file_nodes, key_index, path)?
        }
        (None, Some(name)) => {
            resolve_node_target(store, name)?
        }
        // 不可达:clap 分发层已校验互斥
        _ => unreachable!("clap layer guarantees exactly one of --file/--node is set"),
    };

    if start_nodes.is_empty() {
        emit_empty_result(&target, format)?;
        return Ok(());
    }

    // ── 双向遍历 ──────────────────────────────────────────────────
    let mut upstream_map: HashMap<(Option<String>, String), ImpactEntry> = HashMap::new();
    let mut downstream_map: HashMap<(Option<String>, String), ImpactEntry> = HashMap::new();

    collect_impact_entries(
        graph,
        &start_nodes,
        Direction::Incoming,
        depth,
        &calls_filter,
        &mut upstream_map,
    );
    collect_impact_entries(
        graph,
        &start_nodes,
        Direction::Outgoing,
        depth,
        &calls_filter,
        &mut downstream_map,
    );

    let mut upstream: Vec<ImpactEntry> = upstream_map.into_values().collect();
    let mut downstream: Vec<ImpactEntry> = downstream_map.into_values().collect();
    upstream.sort_by(|a, b| (&a.file_path, &a.symbol).cmp(&(&b.file_path, &b.symbol)));
    downstream.sort_by(|a, b| (&a.file_path, &a.symbol).cmp(&(&b.file_path, &b.symbol)));

    let result = build_impact_result(&target, upstream, downstream);
    emit_result(&result, format)?;
    Ok(())
}

/// `cmd_impact` 的目标解析结果
enum ImpactTarget {
    File { path: String },
    Node { name: String },
}

/// 从文件入口解析出起始节点列表。
/// 文件不在图中或无节点时返回 Ok((vec![], ImpactTarget::File{...})),由调用方走空结果路径。
fn resolve_file_target(
    _graph: &crate::graph::CodeGraph,
    file_nodes: &HashMap<PathBuf, Vec<crate::graph::key::NodeKey>>,
    key_index: &HashMap<crate::graph::key::NodeKey, NodeIndex>,
    path: &Path,
) -> Result<(Vec<NodeIndex>, ImpactTarget)> {
    let target = ImpactTarget::File {
        path: path.to_string_lossy().to_string(),
    };

    let Some((matched_path, was_fuzzy)) = resolve_file_path(path, file_nodes) else {
        eprintln!(
            "Warning: file '{}' not found in graph (no nodes analyzed for this file)",
            path.display()
        );
        return Ok((vec![], target));
    };

    if was_fuzzy {
        eprintln!(
            "Warning: '{}' resolved via fuzzy match to '{}'",
            path.display(),
            matched_path.display()
        );
    }

    let nodes: Vec<NodeIndex> = file_nodes
        .get(matched_path)
        .map(|keys| {
            keys.iter()
                .filter_map(|k| key_index.get(k).copied())
                .collect()
        })
        .unwrap_or_default();

    // 注意:这里把 matched_path 的字符串回填到 target,以便 JSON 里的 file 字段显示规范化后的路径
    let target = ImpactTarget::File {
        path: matched_path.to_string_lossy().to_string(),
    };
    Ok((nodes, target))
}

/// 从节点名入口解析出起始节点列表(单个节点)。
/// 复用 store.search_nodes(),与 cmd_trace / cmd_detail 一致。
fn resolve_node_target(
    store: &crate::graph::store::GraphStore,
    name: &str,
) -> Result<(Vec<NodeIndex>, ImpactTarget)> {
    let matches = store.search_nodes(name);

    if matches.is_empty() {
        eprintln!("No nodes matching '{}'", name);
        return Ok((vec![], ImpactTarget::Node { name: name.to_string() }));
    }

    if matches.len() > 1 {
        eprintln!("Multiple matches found for '{}':", name);
        for (i, (_, n)) in matches.iter().enumerate() {
            eprintln!("  {}: {}", i + 1, n);
        }
        eprintln!("Using first match: {}", matches[0].1);
    } else {
        eprintln!("Impact from: {}", matches[0].1);
    }

    let start_idx = matches[0].0;
    Ok((vec![start_idx], ImpactTarget::Node { name: matches[0].1.clone() }))
}

fn build_impact_result(
    target: &ImpactTarget,
    upstream: Vec<ImpactEntry>,
    downstream: Vec<ImpactEntry>,
) -> ImpactResult {
    let (file, node) = match target {
        ImpactTarget::File { path } => (Some(path.clone()), None),
        ImpactTarget::Node { name } => (None, Some(name.clone())),
    };
    ImpactResult {
        schema_version: 2,
        file,
        node,
        upstream,
        downstream,
    }
}

fn emit_result(result: &ImpactResult, format: &str) -> Result<()> {
    if format == "json" {
        let json = serde_json::to_string_pretty(result).map_err(|e| {
            error::CodeWebError::ExportError {
                message: format!("JSON serialization: {}", e),
            }
        })?;
        println_stdout!("{}", json);
    } else {
        print_impact_text(result);
    }
    Ok(())
}

fn emit_empty_result(target: &ImpactTarget, format: &str) -> Result<()> {
    let result = build_impact_result(target, vec![], vec![]);
    emit_result(&result, format)
}
```

**Step 2: 更新 `print_impact_text` 适配 v2**

将现有 `print_impact_text`(line ~2482)的 `File: {}` 行改为根据 target 类型显示:

```rust
fn print_impact_text(result: &ImpactResult) {
    // 头部:File 或 Node 二选一显示
    if let Some(file) = &result.file {
        println_stdout!("File: {}", file);
    } else if let Some(node) = &result.node {
        println_stdout!("Node: {}", node);
    }
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

**Step 3: 删除 `cmd_impact` 旧版本中已合并到 helper 的代码**

Task 3 Step 1 的 `cmd_impact` 替换了原 `cmd_impact` 全部函数体。原函数中 `resolve_file_path` 调用、`file_nodes` / `key_index` 加载等代码已搬到 `resolve_file_target`,需删除旧版本中的重复定义。

具体:对比新旧 `cmd_impact` 函数体,确保旧的 file 解析逻辑(原 line ~2247-2313)已被新版本替代,没有残留。

`resolve_file_path`、`collect_impact_entries`、`edge_location_line` 三个辅助函数**保持不变**。

**Step 4: 编译验证**

Run: `cargo build`
Expected: 编译通过,无 warning

Run: `cargo clippy -- -D warnings`
Expected: 无 clippy 错误

**Step 5: Commit**

```bash
git add src/main.rs
git commit -m "feat(impact): implement --node dispatch reusing search_nodes + shared traversal"
```

---

### Task 4: 集成测试 `--node`

**Files:**
- Modify: `tests/impact_test.rs`(在文件末尾追加新测试)

**Step 1: 追加 `--node` 集成测试**

在 `tests/impact_test.rs` 末尾追加:

```rust
// ────────────────────────────────────────────────────────────────────
// --node 入口测试
// ────────────────────────────────────────────────────────────────────

#[test]
fn test_impact_node_json_schema() {
    let dir = setup_project();

    // proc_a 调用 proc_b → 对 proc_a 查 node:
    //   upstream 为空(没人调用 proc_a)
    //   downstream 含 proc_b
    let output = run_in_dir(
        &dir,
        &["impact", "--node", "proc_a", "--format", "json"],
    );
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");

    assert_eq!(json["schema_version"], 2);
    assert!(
        json["node"].as_str().unwrap().contains("proc_a"),
        "node field should contain proc_a: {:?}",
        json["node"]
    );
    // file 字段在 --node 路径下应不存在(skip_serializing_if)或为 null
    assert!(
        json.get("file").map(|v| v.is_null()).unwrap_or(true),
        "file should be null or absent for --node: {:?}",
        json.get("file")
    );

    let downstream = json["downstream"].as_array().unwrap();
    assert!(
        downstream
            .iter()
            .any(|e| e["symbol"].as_str().unwrap().contains("proc_b")),
        "downstream should contain proc_b: {:?}",
        downstream
    );

    let upstream = json["upstream"].as_array().unwrap();
    assert!(
        upstream.is_empty(),
        "proc_a has no upstream: {:?}",
        upstream
    );
}

#[test]
fn test_impact_node_upstream_direction() {
    let dir = setup_project();

    // 对 proc_b 查 node:被 proc_a 调用 → upstream 含 proc_a,downstream 为空
    let output = run_in_dir(
        &dir,
        &["impact", "--node", "proc_b", "--format", "json"],
    );
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");

    let upstream = json["upstream"].as_array().unwrap();
    assert!(
        upstream
            .iter()
            .any(|e| e["symbol"].as_str().unwrap().contains("proc_a")),
        "upstream of proc_b should contain proc_a: {:?}",
        upstream
    );

    let downstream = json["downstream"].as_array().unwrap();
    assert!(
        downstream.is_empty(),
        "proc_b has no downstream: {:?}",
        downstream
    );
}

#[test]
fn test_impact_node_not_found() {
    let dir = setup_project();

    let output = run_in_dir(
        &dir,
        &["impact", "--node", "does_not_exist", "--format", "json"],
    );

    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("must be valid JSON");

    assert_eq!(json["schema_version"], 2);
    assert!(json["upstream"].as_array().unwrap().is_empty());
    assert!(json["downstream"].as_array().unwrap().is_empty());

    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("No nodes matching"),
        "stderr should warn about missing node: {}",
        stderr
    );
}

#[test]
fn test_impact_node_text_format() {
    let dir = setup_project();

    let output = run_in_dir(
        &dir,
        &["impact", "--node", "proc_b", "--format", "text"],
    );
    assert!(output.status.success());

    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(
        stdout.contains("Node:"),
        "text output should show 'Node:' header: {}",
        stdout
    );
    assert!(
        stdout.contains("proc_a"),
        "upstream should contain proc_a: {}",
        stdout
    );
}

#[test]
fn test_impact_mutual_exclusion_error() {
    let dir = setup_project();

    // 同时给 --file 和 --node → 退出码 2
    let output = run_in_dir(
        &dir,
        &[
            "impact",
            "--file",
            "proc_a.sql",
            "--node",
            "proc_a",
        ],
    );
    assert!(
        !output.status.success(),
        "should fail when both --file and --node given"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("mutually exclusive"),
        "stderr should mention mutual exclusion: {}",
        stderr
    );
}

#[test]
fn test_impact_neither_flag_error() {
    let dir = setup_project();

    // 都不给 → 退出码 2
    let output = run_in_dir(&dir, &["impact"]);
    assert!(
        !output.status.success(),
        "should fail when neither --file nor --node given"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("exactly one of"),
        "stderr should mention required flag: {}",
        stderr
    );
}
```

**Step 2: 更新现有 v1 测试以适配 schema v2**

现有 4 个测试(`test_impact_json_schema`、`test_impact_file_not_in_graph`、`test_impact_text_format`、`test_impact_upstream_direction`)断言 `schema_version == 1`,需全部改为 `== 2`。

具体修改 `tests/impact_test.rs`:

- `test_impact_json_schema`:`assert_eq!(json["schema_version"], 1)` → `assert_eq!(json["schema_version"], 2)`
- `test_impact_file_not_in_graph`:同上
- (其他 2 个测试不断言 schema_version,无需改)

同时验证:`--file` 路径下 `node` 字段在 JSON 中应不存在(skip_serializing_if)或为 null。可在 `test_impact_json_schema` 中加一条断言:

```rust
    // --file 路径下 node 字段应为 null 或不存在
    assert!(
        json.get("node").map(|v| v.is_null()).unwrap_or(true),
        "node should be null or absent for --file: {:?}",
        json.get("node")
    );
```

**Step 3: 运行测试**

Run: `cargo test --test impact_test`
Expected: 全部 10 个测试通过(原 4 + 新 6)

**Step 4: Commit**

```bash
git add tests/impact_test.rs
git commit -m "test(impact): add --node integration tests + bump v1 tests to schema v2"
```

---

### Task 5: 更新 README

**Files:**
- Modify: `README.md`(中英两版)

**Step 1: 英文版 CLI Reference 表**

找到 `codeweb impact --file <path>` 行,改为:

```markdown
| `codeweb impact --file <path>` | Show upstream/downstream impact for all nodes in a file |
| `codeweb impact --node <name>` | Show upstream/downstream impact for a single node symbol |
```

**Step 2: 英文版 Quick Start**

在现有 `codeweb impact --file ...` 示例之后加一行:

```bash
# Show impact analysis for a single node (by symbol name)
codeweb impact --node "proc_create_order" --format json
```

**Step 3: 中文版同步**

中文 CLI 命令参考表加:

```markdown
| `codeweb impact --node <name>` | 显示单个节点符号的上下游影响 |
```

中文快速开始加:

```bash
# 按节点名查询影响分析
codeweb impact --node "proc_create_order" --format json
```

**Step 4: Commit**

```bash
git add README.md
git commit -m "docs: add impact --node to CLI reference"
```

---

### Task 6: 最终验证

**Step 1: 全 feature 编译**

Run: `cargo build --features full`
Expected: 编译通过

**Step 2: 全 feature 测试**

Run: `cargo test --features full`
Expected: 所有测试通过(含 impact_test 的 10 个测试)

**Step 3: Clippy**

Run: `cargo clippy --features full -- -D warnings`
Expected: 无错误

**Step 4: 格式检查**

Run: `cargo fmt -- --check`
Expected: 无差异

**Step 5: 手动冒烟测试**

```bash
# 在已有 codeweb 项目中
codeweb impact --node "某个已知 proc" --format json | jq .
codeweb impact --node "某个已知 proc" --format text
codeweb impact --node "不存在的名字" --format json   # → 空 + stderr 警告
codeweb impact --file x.sql --node y                 # → 退出码 2
codeweb impact                                       # → 退出码 2

# 对称性检查:同一节点的 --node 结果应等于其所在文件的 --file 结果的子集
codeweb impact --node "proc_a" --format json > node.json
codeweb impact --file "proc_a.sql" --format json > file.json
# 手工 diff:node.json 的 upstream+downstream ⊆ file.json 的 upstream+downstream
```

---

## 验收标准

- [ ] `codeweb impact --node <name> --format json` 输出合法 JSON,符合 schema v2(`schema_version: 2`,`node` 字段为节点名,`file` 字段被 `skip_serializing_if` 省略)
- [ ] `codeweb impact --file <path> --format json` 输出 schema v2,`file` 为路径,`node` 字段被省略 —— 与 v1 行为对称,仅 version 号从 1 变 2
- [ ] `--file` 和 `--node` 同时给出 → 退出码 2 + stderr 互斥错误
- [ ] 都不给 → 退出码 2 + stderr 缺参错误
- [ ] 节点名零匹配 → 退出码 0 + 空数组 + stderr 警告(与 `trace` 行为一致)
- [ ] 节点名多匹配 → stderr 列出 + 用第一个 + 退出码 0(与 `trace` 行为一致)
- [ ] `--format text` 在 `--node` 模式下显示 `Node: <name>` 而非 `File: ...`
- [ ] `cargo fmt` / `cargo clippy --features full -- -D warnings` / `cargo test --features full` 全绿
- [ ] README 中英文 CLI Reference 表均补充 `impact --node`

---

## 关联

- 前置:`docs/plans/2026-06-25-impact-by-file.md`(已实现)
- 本计划将 schema 从 v1 升级到 v2;下游消费方 CodeRoughcollie(`--file` 路径)只需更新 `schema_version` 校验(`1` → `2`),`file` 字段在 `--file` 路径下仍为非 null 字符串,解析逻辑不变
- 引擎层 `store.rs:1687 impact()` 未改动 —— 它本来就按节点工作
