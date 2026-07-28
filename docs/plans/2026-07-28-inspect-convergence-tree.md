# inspect 反向收敛树展示

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 为 `inspect --style tree` 新增反向收敛树展示：以汇聚节点为根，所有调用方为其子树，合并共享节点，边标注 `←` 方向。

**Architecture:** 格式化层改动 — `find_paths_between` 不变。新增 `format_paths_reverse_tree()` 从 `InspectResult.paths` 构建反向邻接表，递归渲染 `├──`/`└──`/`│` box-drawing 树。`InspectStyle` 增加 `Tree` 变体，CLI `--style` 增加 `"tree"` 选项。

**Tech Stack:** Rust, petgraph, std::collections::{BTreeMap, HashSet}

---

## 改动文件

| 文件 | 改动类型 | 说明 |
|---|---|---|
| `src/graph/inspect.rs` | 修改 | 新增 `Tree` 变体 + `format_paths_reverse_tree()` + 测试 |
| `src/main.rs` | 修改 | CLI `--style` value_parser 增加 `"tree"` + match arm |

---

### Task 1: InTree 方向标记解析

**Files:**
- 创建：无
- 修改：`src/graph/inspect.rs:31-36`（`InspectStyle` 枚举）
- 修改：`src/graph/inspect.rs:323-326`（PATHS section 调度）

**Step 1: 新增 `InspectStyle::Tree` 变体**

在 `InspectStyle` 枚举（行 31-36）末尾增加 `Tree` 变体：

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InspectStyle {
    Summary,
    Paths,
    Both,
    /// Reverse convergence tree: roots are destination nodes,
    /// children are callers. Edges marked with `←`.
    Tree,
}
```

**Step 2: 修改 PATHS section 调度逻辑**

修改 `format_inspect_result` 的 PATHS 渲染分支（行 323-326）：

```rust
if style != InspectStyle::Summary && !result.paths.is_empty() {
    lines.push("── PATHS ──".to_string());
    if style == InspectStyle::Tree {
        lines.extend(format_paths_reverse_tree(result, graph));
    } else {
        lines.extend(format_paths_tree(result, graph));
    }
}
```

**Step 3: 编译验证**

```bash
cargo build 2>&1
```

预期：`InspectStyle::Tree` 编译通过，但 `format_paths_reverse_tree` 未定义 → 编译错误（预期行为，下一步实现）。

---

### Task 2: 实现 `format_paths_reverse_tree()`

**Files:**
- 修改：`src/graph/inspect.rs:179` 之后（新增函数）
- 测试：`src/graph/inspect.rs:619` 之后（新增测试）

**Step 1: 写出反向收敛树的数据结构**

在 `format_paths_tree` 之后、`format_inspect_result` 之前插入两个新函数：

```rust
/// Build a reverse adjacency map from paths.
/// Key = node, Value = list of (caller_node, edge_label) pairs.
fn build_reverse_adjacency(
    paths: &[InspectPath],
) -> BTreeMap<NodeIndex, Vec<(NodeIndex, String)>> {
    let mut adj: BTreeMap<NodeIndex, Vec<(NodeIndex, String)>> = BTreeMap::new();

    for path in paths {
        let hops = &path.hops;
        let labels = &path.edge_labels;
        // For each consecutive pair (hops[i], hops[i+1]):
        //   hops[i] calls hops[i+1], so hops[i] is a child of hops[i+1] in reverse tree
        for i in 0..hops.len().saturating_sub(1) {
            let caller = hops[i];
            let callee = hops[i + 1];
            let label = if i < labels.len() {
                labels[i].clone()
            } else {
                String::new()
            };
            adj.entry(callee)
                .or_default()
                .push((caller, label));
        }
    }

    // Sort children by display name for deterministic output
    for children in adj.values_mut() {
        children.sort_by(|(a, _), (b, _)| {
            // We need graph here... Actually, pass graph to this function
            unimplemented!("will resolve in next iteration")
        });
    }

    adj
}
```

Wait — sorting needs graph reference. Adjust signature to take `graph: &CodeGraph`:

```rust
fn build_reverse_adjacency(
    graph: &CodeGraph,
    paths: &[InspectPath],
) -> BTreeMap<NodeIndex, Vec<(NodeIndex, String)>> {
    let mut adj: BTreeMap<NodeIndex, Vec<(NodeIndex, String)>> = BTreeMap::new();

    for path in paths {
        let hops = &path.hops;
        let labels = &path.edge_labels;
        for i in 0..hops.len().saturating_sub(1) {
            let caller = hops[i];
            let callee = hops[i + 1];
            let label = if i < labels.len() {
                labels[i].clone()
            } else {
                String::new()
            };
            let entry = adj.entry(callee).or_default();
            // dedup: same (caller, callee) edge from different paths
            if !entry.iter().any(|(n, _)| *n == caller) {
                entry.push((caller, label));
            }
        }
    }

    // Sort children by display name
    for children in adj.values_mut() {
        children.sort_by(|(a, _), (b, _)| {
            node_display_name(&graph[*a]).cmp(&node_display_name(&graph[*b]))
        });
    }

    adj
}
```

**Step 2: 实现递归渲染分支**

```rust
fn render_reverse_branch(
    graph: &CodeGraph,
    adj: &BTreeMap<NodeIndex, Vec<(NodeIndex, String)>>,
    node: NodeIndex,
    prefix: &str,
    is_last: bool,
    lines: &mut Vec<String>,
) {
    let name = node_display_name(&graph[*node]);
    let child_count = adj.get(&node).map(|c| c.len()).unwrap_or(0);

    let connector = if is_last {
        "└── "
    } else {
        "├── "
    };
    let caller_info = if child_count > 0 {
        format!("  (called by {})", child_count)
    } else {
        String::new()
    };

    // Edge label comes from parent→child relationship.
    // The label was stored on the child entry in adj[parent].
    // But here we're rendering the child node itself.
    // We need the label to be part of the parent's rendering of this child.
    // So the label should be passed from the parent level.
    // Actually — let's rethink.

    // Better approach: render_edge_to_child is called by parent,
    // then recurse into child.
}

/// Render a child node entry: its name, the edge label `←`, and optional caller count.
/// Then recurse into its own children.
fn render_child(
    graph: &CodeGraph,
    adj: &BTreeMap<NodeIndex, Vec<(NodeIndex, String)>>,
    child: NodeIndex,
    edge_label: &str,
    prefix: &str,
    is_last: bool,
    lines: &mut Vec<String>,
) {
    let name = node_display_name(&graph[*child]);
    let grandchild_count = adj.get(&child).map(|c| c.len()).unwrap_or(0);

    let connector = if is_last { "└── " } else { "├── " };
    let label_str = if edge_label.is_empty() {
        String::new()
    } else {
        format!("  ← {}", edge_label)
    };
    let caller_info = if grandchild_count > 0 {
        format!("  (called by {})", grandchild_count)
    } else {
        String::new()
    };

    lines.push(format!(
        "{}{}{}{}{}",
        prefix, connector, name, label_str, caller_info
    ));

    // Recurse into grandchildren
    let extension = if is_last { "    " } else { "│   " };
    let new_prefix = format!("{}{}", prefix, extension);

    if let Some(grandchildren) = adj.get(&child) {
        let count = grandchildren.len();
        for (gi, (gc, gc_label)) in grandchildren.iter().enumerate() {
            render_child(
                graph,
                adj,
                *gc,
                gc_label,
                &new_prefix,
                gi == count - 1,
                lines,
            );
        }
    }
}
```

**Step 3: 实现顶层入口 `format_paths_reverse_tree()`**

```rust
/// Format PATHS as reverse convergence trees.
/// Groups paths by destination (to), then builds a tree with
/// the destination as root and all callers as children.
/// Edges are marked with `←` to indicate caller→callee direction.
fn format_paths_reverse_tree(
    result: &InspectResult,
    graph: &CodeGraph,
) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();

    // Group paths by destination (to)
    let mut by_dest: BTreeMap<NodeIndex, Vec<&InspectPath>> = BTreeMap::new();
    for p in &result.paths {
        by_dest.entry(p.to).or_default().push(p);
    }

    for (root, paths) in &by_dest {
        let root_name = node_display_name(&graph[*root]);
        // Count unique direct callers
        let mut direct_callers: HashSet<NodeIndex> = HashSet::new();
        for p in paths {
            if p.hops.len() >= 2 {
                // hops[hops.len()-2] is the direct caller of root
                direct_callers.insert(p.hops[p.hops.len() - 2]);
            }
        }
        let caller_count = direct_callers.len();

        lines.push(String::new());
        lines.push(format!(
            "── {} (root, called by {}) ──",
            root_name, caller_count
        ));

        // Build reverse adjacency just for these paths
        let adj = build_reverse_adjacency(graph, paths);

        // Render direct children of root
        if let Some(children) = adj.get(root) {
            let count = children.len();
            for (ci, (child, label)) in children.iter().enumerate() {
                render_child(
                    graph,
                    &adj,
                    *child,
                    label,
                    "    ",
                    ci == count - 1,
                    &mut lines,
                );
            }
        }
    }

    lines
}
```

**Step 4: 更新 `src/main.rs` 的 CLI 和 match**

`src/main.rs:614` — 增加 `"tree"`：
```rust
#[arg(short, long, default_value = "both", value_parser = ["summary", "paths", "both", "tree"])]
```

`src/main.rs:2962-2966` — 增加 match arm：
```rust
let style_enum = match style {
    "summary" => InspectStyle::Summary,
    "paths" => InspectStyle::Paths,
    "tree" => InspectStyle::Tree,
    _ => InspectStyle::Both,
};
```

**Step 5: 编译验证**

```bash
cargo build 2>&1
```

预期：编译通过，无 warning。

---

### Task 3: 回归测试

**Files:**
- 修改：`src/graph/inspect.rs:619` 之后（新增 3 个测试）

**Step 1: 测试 — 基本分叉 DAG 反向树**

```rust
#[test]
fn format_reverse_tree_forked_dag() {
    let mut graph = CodeGraph::new();
    let a1 = graph.add_node(make_proc("a1", None));
    let b1 = graph.add_node(make_proc("b1", None));
    let c = graph.add_node(make_proc("c", None));
    let b2 = graph.add_node(make_proc("b2", None));
    let a2 = graph.add_node(make_proc("a2", None));

    make_direct_call(&mut graph, a1, b1);
    make_direct_call(&mut graph, b1, c);
    make_direct_call(&mut graph, a2, b2);
    make_direct_call(&mut graph, b2, c);

    let result = find_paths_between(&graph, &[a1, a2, c], &InspectOptions::default());
    let output = format_inspect_result(
        &result,
        &graph,
        &["a1".into(), "a2".into(), "c".into()],
        InspectStyle::Tree,
        false,
    );

    // 验证根节点
    assert!(output.contains("── proc:c (root, called by 2) ──"));
    // 验证 b1 分支（含方向标记和 caller count）
    assert!(output.contains("├── proc:b1"));
    assert!(output.contains("← [cross]"));
    assert!(output.contains("(called by 1)"));
    // a1 是 b1 的子节点、叶子（无 called by 标注）
    assert!(output.contains("└── proc:a1"));
    // 验证 b2 分支
    assert!(output.contains("└── proc:b2"));
    // a2 是 b2 的子节点
    assert!(output.contains("└── proc:a2"));
    // 验证 PATHS 存在（Tree 风格应在 PATHS 区块）
    assert!(output.contains("── PATHS ──"));
    // 验证 CONNECTIONS 也存在（Tree != Paths）
    assert!(output.contains("── CONNECTIONS ──"));
}
```

**Step 2: 测试 — 共享中间节点合并**

```rust
#[test]
fn format_reverse_tree_shared_intermediate_merged() {
    // a1 → b → c, a2 → b → c
    let mut graph = CodeGraph::new();
    let a1 = graph.add_node(make_proc("a1", None));
    let a2 = graph.add_node(make_proc("a2", None));
    let b = graph.add_node(make_proc("b", None));
    let c = graph.add_node(make_proc("c", None));

    make_direct_call(&mut graph, a1, b);
    make_direct_call(&mut graph, a2, b);
    make_direct_call(&mut graph, b, c);

    let result = find_paths_between(&graph, &[a1, a2, c], &InspectOptions::default());
    let output = format_inspect_result(
        &result,
        &graph,
        &["a1".into(), "a2".into(), "c".into()],
        InspectStyle::Tree,
        false,
    );

    // b 只出现一次（合并），显示 called by 2
    assert!(output.contains("── proc:c (root, called by 1) ──"));
    assert!(output.contains("proc:b  ← [cross]  (called by 2)"));
    // a1 和 a2 都在 b 下
    assert!(output.contains("├── proc:a1"));
    assert!(output.contains("└── proc:a2"));
}
```

**Step 3: 测试 — 无路径时不崩溃**

```rust
#[test]
fn format_reverse_tree_no_paths_empty() {
    let mut graph = CodeGraph::new();
    let a = graph.add_node(make_proc("a", None));
    let b = graph.add_node(make_proc("b", None));

    let result = find_paths_between(&graph, &[a, b], &InspectOptions::default());
    let output = format_inspect_result(
        &result,
        &graph,
        &["a".into(), "b".into()],
        InspectStyle::Tree,
        false,
    );

    // 无 PATHS section（paths 为空）
    assert!(!output.contains("── PATHS ──"));
    // CONNECTIONS 存在但显示 "(no paths found between any pair)"
    assert!(output.contains("(no paths found between any pair)"));
}
```

**Step 4: 运行全部 inspect 测试**

```bash
cargo test graph::inspect -- --nocapture
```

预期：16 passed（11 原有 + 2 forked_dag 新增 + 3 tree 新增）。

**Step 5: 运行 lint + fmt**

```bash
cargo clippy -- -D warnings
cargo fmt -- --check
```

---

### Task 4: 提交

**Step 1: 提交**

```bash
git add src/graph/inspect.rs src/main.rs docs/plans/2026-07-28-inspect-convergence-tree.md
git commit -m "feat(inspect): add --style tree for reverse convergence tree display

Edges are marked with ← to indicate caller→callee direction.
Root nodes show (called by N) count. Shared intermediate nodes
are merged into a single tree branch."
```
