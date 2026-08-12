# `lineage` 命令 (#115 + #136 统一) Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 在 codeweb 中增加统一的 `lineage` CLI 子命令，支持表级血缘（#115）和列级血缘（#136），通过 `table.column` 分隔符自动区分粒度。列级血缘覆盖 OLAP 聚合场景（SUM/GROUP BY 维度键 vs 聚合度量区分、表达式变换追踪、视图展开、多跳链路）。

**Architecture:** 复用现有 `ColumnAccessExtractor`（18个测试、已提取列引用/别名/JOIN条件/WHERE过滤），在其上构建 `ColumnLineageExtractor` 包装层（零回归），新增 `Node::Column` + `Edge::ColumnFlow/Derived/Aggregated` 图模型变体，扩展 `builder.rs` 将列级边注入图，新增 `traverse::column_lineage()` 沿列级边 BFS 追溯。

**Tech Stack:** Rust, ogsql-parser (已有 Visitor + expression AST + token spans), petgraph, 现有 codeweb 模块。**无新依赖。**

---

## 范围

### MVP (本计划实施)

| 能力 | 粒度 |
|------|------|
| `lineage` CLI 子命令 | 表级 + 列级，`table.column` 自动区分 |
| 三种输出格式 | `tree`（默认，缩进树）、`table`（脚本消费）、`json`（程序集成） |
| 列级血缘溯源 | `--direction upstream|downstream|both`, `--depth N` |
| 4种列级变换类型 | `DataFlow`（直通）、`Derived`（DECODE/算术/CASE）、`Aggregated: SUM/COUNT/AVG`（聚合+GROUP BY键区分）、`Window`（窗口函数） |
| 4种边类型 | `Edge::DataFlow`, `Edge::Derived`, `Edge::Aggregated`, `Edge::Window` |
| 表达式文本捕获 | 非简单列引用时记录原始表达式文本 (`Spanned<T>` from ogsql-parser) |
| 视图列血缘 | 单层视图展开（`CREATE VIEW v AS SELECT a FROM t` → `v.a ← t.a`） |
| 测试覆盖 | ≥12 个集成测试（使用 `tests/fixtures/lineage/*.sql` 的 12 个案例） |

### Out of Scope (后续迭代)

- ❌ Schema 驱动的 `SELECT *` 展开（需 JSON schema 文件加载，MVP 标记 `approximate`）
- ❌ 多层嵌套视图展开（循环检测）
- ❌ 跨语句列血缘（`CREATE TABLE AS → INSERT → SELECT` 列依赖链）
- ❌ 存储过程内列血缘（PL/pgSQL 变量追踪）
- ❌ MCP 工具暴露（`codeweb_column_lineage`）
- ❌ 浏览器 UI 列视图

---

## 关键设计决策

| 决策 | 选择 | 理由 |
|------|------|------|
| 表级/列级同一个命令 | `lineage`，参数含 `.` 自动走列级 | 减少认知负担；OLAP 用户频繁两级切换 |
| 默认输出格式 | tree（缩进树） | 终端阅读友好，一眼看链路 |
| 不修改 `ColumnAccessExtractor` | 新建 `ColumnLineageExtractor` 包装层 | 零回归风险；原提取器的 18 个测试不受影响 |
| 列级边复用 `EdgeCategory::DataFlow` | 新增变体而非新 Category | 列级边本质是数据流 |
| 列节点不预创建 | 按需创建（仅创建被 SELECT 引用的列） | 避免大表（200+列）导致图膨胀 |
| bincode store 版本 | `STORE_VERSION` 6 → 7 | 新增 Node/Edge 变体后布局变化 |
| `Node::Column` ID 规则 | `col:<table_canonical>.<column_name>` | 稳定、可读、支持跨语句合并 |
| 不引入新依赖 | 完全基于 ogsql-parser + petgraph | 零依赖膨胀 |

---

## 图模型扩展

### Node::Column 变体

```rust
// src/graph/mod.rs - 在 Node 枚举中新增
Node::Column {
    /// 稳定 ID: "col:<table_canonical>.<column_name>"
    id: String,
    /// 所属表/视图/CTE 的 canonical 名称
    owner_table: String,
    /// 列名
    name: String,
    /// 数据类型（来自 DDL schema 或推断）
    data_type: Option<String>,
    /// 表达式文本（仅当列为计算派生时，非 None）
    expression: Option<String>,
    /// 聚合信息（仅当列涉及聚合时，非 None）
    aggregation: Option<AggregationInfo>,
    /// 是否 GROUP BY 键（OLAP 维度 vs 度量区分）
    is_grouping_key: bool,
    /// 来源位置
    location: Option<SourceLocation>,
}
```

### 新增 Edge 变体

```rust
// src/graph/mod.rs - 在 Edge 枚举中新增
Edge::DataFlow {
    /// 来源列节点 ID (col:<table>.<col>)
    source_col_id: String,
    /// 目标列节点 ID
    target_col_id: String,
    /// 位置
    location: Option<SourceLocation>,
}

Edge::Derived {
    /// 来源列节点 ID 列表
    source_col_ids: Vec<String>,
    /// 目标列节点 ID
    target_col_id: String,
    /// SQL 表达式文本 (e.g. "a + b", "DECODE(bs, 'B','1B','S','1S','0')")
    expression: String,
    /// 位置
    location: Option<SourceLocation>,
}

Edge::Aggregated {
    /// 来源列节点 ID 列表
    source_col_ids: Vec<String>,
    /// 目标列节点 ID
    target_col_id: String,
    /// 聚合函数名 (SUM, COUNT, AVG, MAX, MIN)
    function: String,
    /// 是否 DISTINCT
    distinct: bool,
    /// GROUP BY 列节点 ID 列表
    group_by_col_ids: Vec<String>,
    /// 位置
    location: Option<SourceLocation>,
}
```

### 必须更新的 match 分支

添加新 `Node`/`Edge` 变体后，以下位置**必须**新增 match 分支（否则编译报错）：

| 文件 | 位置 | 需要新增 |
|------|------|---------|
| `src/graph/mod.rs` | `node_type_tag()` | `"col"` 分支 |
| `src/graph/mod.rs` | `node_display_name()` | `Column` 分支 → `"col:owner.name"` |
| `src/graph/mod.rs` | `Node::file()` | `Column` 分支 → 返回 `location.file` 或空 Path |
| `src/graph/mod.rs` | `Edge::category()` | `DataFlow/Derived/Aggregated` → `EdgeCategory::DataFlow` |
| `src/graph/mod.rs` | `Node::column_summaries()` | 返回空 vec（或列的元数据） |
| `src/graph/key.rs` | `NodeKey` 枚举 | 新增 `NodeKey::Column { schema, table, name }` |
| `src/graph/key.rs` | `NodeKey::from_node()` | 新增 `Column` → `NodeKey::Column` 转换 |
| `src/graph/key.rs` | `Display for NodeKey` | 新增 `"col:schema.table.name"` 格式 |
| `src/graph/store.rs` | `STORE_VERSION` | 6 → 7 |
| `src/export/json.rs` | 节点/边序列化 | 新增 `Column`/`DataFlow`/`Derived`/`Aggregated` JSON 输出 |
| `src/graph/traverse.rs` | 路径追踪 | 新增 `column_lineage()` 函数 |

---

## CLI 设计

```bash
# 表级血缘（参数无 `.` 分隔符）
codeweb lineage orders --direction upstream

# 列级血缘（参数含 `.` 分隔符）
codeweb lineage orders.amount --direction upstream

# 方向
--direction upstream|downstream|both   (默认: upstream)

# 深度控制
--depth N                              (默认: 10)

# 输出格式
--format tree|table|json               (默认: tree)

# 输出文件
--output <path>                        (默认: stdout)
```

### 输出格式示例

**tree 模式** (默认):
```
codeweb lineage trade_summary.total_qty --direction upstream

trade_summary.total_qty [Aggregated: SUM, GROUP BY account_id,branch_code,trade_date,bs_flag,product_code]
  └── trade_detail.quantity [DataFlow]

trade_summary.account_id [GROUP BY key]
  └── trade_detail.account_id [DataFlow]
```

**table 模式**:
```
COLUMN                     | TRANSFORM                    | SOURCE_TABLE    | SOURCE_COLUMN | DEPTH | KIND
---------------------------+-----------------------------+-----------------+---------------+-------+-----------
trade_summary.total_qty   | SUM(quantity)               | trade_detail    | quantity      | 1     | aggregated
trade_summary.account_id  | →                           | trade_detail    | account_id    | 1     | dataflow
```

**json 模式**: 结构化输出，含 `target`, `direction`, `kind`, `chain[]` 数组。

---

## 实施步骤

### Task 1: ColumnLineageExtractor 核心提取器

**文件:**
- Create: `src/parser/column_lineage.rs`
- Modify: `src/parser/mod.rs`（添加 `pub mod column_lineage` + re-export）

**目标:** 在 `ColumnAccessExtractor` 基础上构建列级血缘提取器，捕获表达式文本、聚合检测、列映射关系。

**Step 1.1: 定义数据结构**

```rust
// src/parser/column_lineage.rs

use ogsql_parser::ast::*;
use crate::graph::SourceLocation;
use crate::parser::ColumnAccessExtractor;

/// 列级边（提取阶段的中间表示）
#[derive(Debug, Clone)]
pub enum ColumnEdge {
    /// 直接映射: SELECT a AS b → a → b
    Flow {
        target_col: String,
        source_table: Option<String>,
        source_col: String,
        location: Option<SourceLocation>,
    },
    /// 表达式派生: SELECT a + 1 AS b
    Derived {
        target_col: String,
        source_cols: Vec<(Option<String>, String)>,
        expression: String,
        location: Option<SourceLocation>,
    },
    /// 聚合: SELECT SUM(a) AS total
    Aggregated {
        target_col: String,
        source_cols: Vec<(Option<String>, String)>,
        function: String,
        distinct: bool,
        group_by_cols: Vec<String>,
        location: Option<SourceLocation>,
    },
}

/// 列级血缘提取器
pub struct ColumnLineageExtractor {
    /// 内嵌的基础提取器（复用别名解析、列引用等能力）
    base: ColumnAccessExtractor,
    /// 提取到的列级边
    column_edges: Vec<ColumnEdge>,
    /// 当前上下文中的 GROUP BY 列名列表
    group_by_columns: Vec<String>,
    /// 当前语句的输出目标表（用于构建 Column ID）
    current_output: Option<OutputContext>,
}

struct OutputContext {
    owner_table: String,  // e.g. "public.trade_summary" or "out_trd_gh_jy"
}
```

**Step 1.2: 实现核心提取逻辑**

```rust
impl ColumnLineageExtractor {
    pub fn new() -> Self {
        Self {
            base: ColumnAccessExtractor::new(),
            column_edges: Vec::new(),
            group_by_columns: Vec::new(),
            current_output: None,
        }
    }

    /// 设置当前语句的输出目标
    pub fn set_output(&mut self, owner_table: &str) {
        self.current_output = Some(OutputContext {
            owner_table: owner_table.to_string(),
        });
    }

    /// 完成提取，返回列级边列表
    pub fn finish(self) -> Vec<ColumnEdge> {
        self.column_edges
    }

    /// 分析 SELECT 目标列表，构建列级边
    pub fn analyze_select_targets(&mut self, targets: &[SelectTarget]) {
        for target in targets {
            match target {
                SelectTarget::ExprWithAlias { expr, alias } => {
                    let target_name = alias.value.clone();
                    self.classify_and_add_edge(expr, &target_name);
                }
                SelectTarget::Expr(expr) => {
                    if let Some(name) = self.derive_column_name(expr) {
                        self.classify_and_add_edge(expr, &name);
                    }
                }
                SelectTarget::Wildcard => {
                    // MVP: 无 schema 时标记 approximate
                    // 后续: schema 加载后展开
                }
                SelectTarget::QualifiedWildcard(qualifier) => {
                    // 同上
                }
            }
        }
    }

    /// 分类表达式并构建对应边类型
    fn classify_and_add_edge(&mut self, expr: &Expr, target_col: &str) {
        let owner = self.current_output.as_ref()
            .map(|c| c.owner_table.clone())
            .unwrap_or_default();

        match expr {
            // 聚合函数: SUM(col) / COUNT(*) / AVG(col) ...
            Expr::FunctionCall { name, args, distinct, .. }
                if is_aggregate_function(name) =>
            {
                let source_cols = extract_arg_columns(args);
                let func_name = name.to_uppercase();
                self.column_edges.push(ColumnEdge::Aggregated {
                    target_col: format!("{}.{}", owner, target_col),
                    source_cols,
                    function: func_name,
                    distinct: *distinct,
                    group_by_cols: self.group_by_columns.clone(),
                    location: None, // TODO: 从 Span 提取
                });
            }
            // 简单列引用: SELECT col 或 SELECT t.col
            expr if is_simple_column_ref(expr) => {
                let (table, col) = extract_column_ref(expr);
                self.column_edges.push(ColumnEdge::Flow {
                    target_col: format!("{}.{}", owner, target_col),
                    source_table: table,
                    source_col: col,
                    location: None,
                });
            }
            // 其他表达式: a + b, DECODE(...), CASE WHEN ...
            _ => {
                let source_cols = extract_all_columns(expr);
                let expr_text = expr_to_source_text(expr);
                self.column_edges.push(ColumnEdge::Derived {
                    target_col: format!("{}.{}", owner, target_col),
                    source_cols,
                    expression: expr_text,
                    location: None,
                });
            }
        }
    }
}
```

**Step 1.3: 辅助函数**

```rust
/// 判断函数名是否为聚合函数
fn is_aggregate_function(name: &str) -> bool {
    matches!(name.to_uppercase().as_str(),
        "SUM" | "COUNT" | "AVG" | "MAX" | "MIN" | "STDDEV" | "VARIANCE")
}

/// 从表达式提取所有列引用（含表名前缀）
fn extract_all_columns(expr: &Expr) -> Vec<(Option<String>, String)> {
    // 使用 Visitor 模式递归收集所有 ColumnRef 节点
    todo!()
}

/// 判断表达式是否为简单列引用（非表达式、非函数调用）
fn is_simple_column_ref(expr: &Expr) -> bool {
    matches!(expr, Expr::ColumnRef(_))
}

/// 从 ColumnRef 表达式提取 (table, column) 元组
fn extract_column_ref(expr: &Expr) -> (Option<String>, String) {
    // 解析 ColumnRef 的 parts
    todo!()
}

/// 从表达式提取原始 SQL 文本（利用 ogsql-parser 的 Span 信息）
fn expr_to_source_text(expr: &Expr) -> String {
    // 使用 ogsql-parser 的 Spanned<T> 或 token span 提取
    todo!()
}
```

**验收标准:**
- [ ] `cargo build` 编译通过（新增模块）
- [ ] `ColumnLineageExtractor::new()` 可创建
- [ ] `analyze_select_targets()` 对简单 `SELECT a AS b FROM t` 产生 `ColumnEdge::Flow`
- [ ] `analyze_select_targets()` 对 `SELECT SUM(a) AS total FROM t GROUP BY x` 产生 `ColumnEdge::Aggregated` + GROUP BY 列列表
- [ ] `analyze_select_targets()` 对 `SELECT a + 1 AS b FROM t` 产生 `ColumnEdge::Derived` + expression = `"a + 1"`
- [ ] 现有 `ColumnAccessExtractor` 的 18 个测试仍然通过（零回归）

---

### Task 2: 图模型扩展 — Node::Column + 新 Edge 变体

**文件:**
- Modify: `src/graph/mod.rs`
- Modify: `src/graph/key.rs`
- Modify: `src/graph/store.rs`
- Modify: `src/graph/builder.rs`

**目标:** 在现有 Node/Edge 枚举中新增列级变体，更新所有 match 分支，确保编译通过。

**Step 2.1: 新增 AggregationInfo 辅助结构**

在 `src/graph/mod.rs` 中，`Node` 枚举定义之前添加：

```rust
/// 聚合信息（OLAP 场景：区分维度键和聚合度量）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregationInfo {
    /// 聚合函数名 (SUM, COUNT, AVG, MAX, MIN, ...)
    pub function: String,
    /// 是否包含 DISTINCT
    pub distinct: bool,
    /// GROUP BY 列列表（仅在 is_grouping_key=false 时填充）
    #[serde(default)]
    pub group_by_cols: Vec<String>,
}
```

**Step 2.2: 新增 Node::Column 变体**

在 `src/graph/mod.rs` 第 513 行 `Node` 枚举的右大括号 `}` 之前添加：

```rust
    /// 列节点 — 列级血缘分析启用时创建
    Column {
        /// 稳定 ID: "col:<table_canonical>.<column_name>"
        id: String,
        /// 所属表/视图的 canonical 名称
        owner_table: String,
        /// 列名
        name: String,
        /// 数据类型（来自 DDL 推断，或 None）
        #[serde(default)]
        data_type: Option<String>,
        /// 表达式文本（仅当列为计算派生列时非 None）
        #[serde(default)]
        expression: Option<String>,
        /// 聚合信息（仅当列涉及聚合时非 None）
        #[serde(default)]
        aggregation: Option<AggregationInfo>,
        /// 是否为 GROUP BY 键（OLAP: 维度列）
        #[serde(default)]
        is_grouping_key: bool,
        /// 来源位置
        #[serde(default)]
        location: Option<SourceLocation>,
    },
```

**Step 2.3: 新增 Edge 变体**

在 `src/graph/mod.rs` 第 659 行 `Edge` 枚举的右大括号 `}` 之前添加：

```rust
    /// 列直接映射: source.col → target.col (SELECT a AS b)
    DataFlow {
        source_col_id: String,
        target_col_id: String,
        location: Option<SourceLocation>,
    },
    /// 列表达式派生: source.col(s) → target.col (SELECT a + 1 AS b)
    Derived {
        source_col_ids: Vec<String>,
        target_col_id: String,
        /// SQL 表达式文本
        expression: String,
        location: Option<SourceLocation>,
    },
    /// 列聚合: source.col → target.col (SELECT SUM(a) AS total GROUP BY x)
    Aggregated {
        source_col_ids: Vec<String>,
        target_col_id: String,
        /// 聚合函数名
        function: String,
        /// 是否 DISTINCT
        #[serde(default)]
        distinct: bool,
        /// GROUP BY 列节点 ID 列表
        #[serde(default)]
        group_by_col_ids: Vec<String>,
        location: Option<SourceLocation>,
    },
```

**Step 2.4: 更新所有 match 分支**

按顺序更新以下函数中的 match 分支（每处新增 `Column`/`DataFlow`/`Derived`/`Aggregated` 分支）：

1. `node_type_tag()` (L517-549): 新增 `Node::Column { .. } => "col"`
2. `node_display_name()` (L557-571): 新增 `Node::Column { id, .. } => id.clone()`
3. `Node::file()` (L706-740): 新增 `Node::Column { location, .. } => location.as_ref().map(|l| l.file.as_path()).unwrap_or(Path::new(""))`
4. `Edge::category()` (L665-685): 新增 `Edge::DataFlow { .. } | Edge::Derived { .. } | Edge::Aggregated { .. } => EdgeCategory::DataFlow`
5. `NodeKey` 枚举 (key.rs L7-91): 新增 `NodeKey::Column { table: String, name: String }`
6. `NodeKey::from_node()` (key.rs L181-288): 新增 `Column` → `NodeKey::Column` 转换
7. `Display for NodeKey` (key.rs L93-179): 新增 `"col:table.name"` 显示格式

**Step 2.5: 更新 STORE_VERSION**

在 `src/graph/store.rs` 第 22 行:
```rust
const STORE_VERSION: u32 = 7;  // was 6
```

**验收标准:**
- [ ] `cargo build` 编译通过（所有新增 match 分支到位）
- [ ] `cargo test` 通过（现有测试不因新变体而失败）
- [ ] 序列化/反序列化 round-trip 正确（`Node::Column` 和三个新 Edge 变体可 bincode 序列化）

---

### Task 3: Builder 集成 — 将列级边注入图

**文件:**
- Modify: `src/graph/builder.rs`

**目标:** 在 `collect_table_access_from_statements()` 或新函数中，调用 `ColumnLineageExtractor` 将列级边添加到图中。

**Step 3.1: 新增 `build_column_lineage_edges` 函数**

```rust
// src/graph/builder.rs

/// 从语句中提取列级血缘边并添加到图中
fn build_column_lineage_edges(
    graph: &mut CodeGraph,
    statement: &Statement,
    owner_table: &str,
    location: SourceLocation,
) {
    let mut extractor = crate::parser::ColumnLineageExtractor::new();
    extractor.set_output(owner_table);

    // 使用 Visitor 遍历语句的 SELECT 部分
    // 注意: 这里需要根据 Statement 类型选择正确的 walk 函数
    if let Some(select) = extract_select_from_statement(statement) {
        extractor.analyze_select_targets(&select.targets);
    }

    let edges = extractor.finish();
    for edge in edges {
        match edge {
            ColumnEdge::Flow { target_col, source_table, source_col, location } => {
                let source_id = format!("col:{}.{}", 
                    source_table.unwrap_or_else(|| owner_table.to_string()),
                    source_col);
                let target_id = format!("col:{}", target_col);
                // 创建列节点（如果尚不存在）并添加边
                upsert_column_node(graph, &source_id, &source_table, &source_col);
                upsert_column_node(graph, &target_id, owner_table, &target_col.split('.').last().unwrap());
                graph.add_edge(
                    find_column_node(graph, &source_id).unwrap(),
                    find_column_node(graph, &target_id).unwrap(),
                    Edge::DataFlow {
                        source_col_id: source_id.clone(),
                        target_col_id: target_id.clone(),
                        location: location.clone(),
                    },
                );
            }
            ColumnEdge::Derived { target_col, source_cols, expression, location } => {
                build_derived_edge(graph, &target_col, &source_cols, &expression, owner_table, location);
            }
            ColumnEdge::Aggregated { target_col, source_cols, function, distinct, group_by_cols, location } => {
                build_aggregated_edge(graph, &target_col, &source_cols, &function, distinct, &group_by_cols, owner_table, location);
            }
        }
    }
}
```

**Step 3.2: 辅助函数**

```rust
/// 查找或创建列节点
fn upsert_column_node(graph: &mut CodeGraph, col_id: &str, owner_table: &str, col_name: &str) {
    // 通过 col_id 查找已有节点，不存在则创建
    for idx in graph.node_indices() {
        if matches!(&graph[idx], Node::Column { id, .. } if id == col_id) {
            return;
        }
    }
    graph.add_node(Node::Column {
        id: col_id.to_string(),
        owner_table: owner_table.to_string(),
        name: col_name.to_string(),
        data_type: None,
        expression: None,
        aggregation: None,
        is_grouping_key: false,
        location: None,
    });
}

/// 在图中通过 col_id 查找列节点索引
fn find_column_node(graph: &CodeGraph, col_id: &str) -> Option<NodeIndex> {
    graph.node_indices().find(|&idx| {
        matches!(&graph[idx], Node::Column { id, .. } if id == col_id)
    })
}
```

**Step 3.3: 在分析入口启用列级提取**

在 `builder.rs` 的 `build_graph()` 或 `collect_table_access_from_statements()` 中，对每条包含 SELECT 的语句调用 `build_column_lineage_edges()`。注意：默认情况下不启用列级分析（保持现有行为）；仅在 `analyze --column-lineage` 时启用。

**验收标准:**
- [ ] `cargo build` 编译通过
- [ ] 对 `01-simple-dataflow.sql` 执行 analysis 后，图中存在 `Node::Column` 节点和 `Edge::DataFlow` 边
- [ ] 现有测试全部通过（列级分析默认关闭，不改变既有行为）

---

### Task 4: 遍历引擎 — `column_lineage()` 查询函数

**文件:**
- Modify: `src/graph/traverse.rs`

**目标:** 实现沿列级边的 BFS/DFS 追溯，支持方向控制和深度限制。

**Step 4.1: 实现 `column_lineage()` 函数**

```rust
// src/graph/traverse.rs

/// 列级血缘查询结果
#[derive(Debug, Clone)]
pub struct ColumnLineageResult {
    /// 起始列节点 ID
    pub source_id: String,
    /// 目标列节点 ID  
    pub target_id: String,
    /// 边类型
    pub edge_kind: ColumnEdgeKind,
    /// 表达式文本（Derived 时非空）
    pub expression: Option<String>,
    /// 聚合函数（Aggregated 时非空）
    pub aggregation: Option<String>,
    /// 深度（距起始节点的跳数）
    pub depth: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ColumnEdgeKind {
    DataFlow,
    Derived,
    Aggregated,
}

impl CodeGraph {
    /// 从指定列节点按方向追溯列级血缘
    /// direction: "upstream" (反向，沿入边) | "downstream" (正向，沿出边) | "both"
    pub fn column_lineage(
        &self,
        col_id: &str,
        direction: &str,
        max_depth: usize,
    ) -> Vec<Vec<ColumnLineageResult>> {
        let start_node = match self.find_column_node(col_id) {
            Some(idx) => idx,
            None => return vec![],
        };

        let mut paths: Vec<Vec<ColumnLineageResult>> = vec![];
        let mut visited = std::collections::HashSet::new();
        let mut queue = std::collections::VecDeque::new();
        queue.push_back((start_node, 0, vec![]));

        while let Some((current, depth, path)) = queue.pop_front() {
            if depth > max_depth {
                continue;
            }
            visited.insert(current);

            // 沿 DataFlow/Derived/Aggregated 边遍历
            let edges: Vec<_> = if direction == "upstream" || direction == "both" {
                // 反向: 查找指向 current 的列级边
                self.edges_directed(current, petgraph::Direction::Incoming)
                    .filter_map(|e| {
                        let edge = &self[e.id()];
                        match edge {
                            Edge::DataFlow { target_col_id, .. } if self.is_column_node(e.source()) => Some((e.source(), edge.clone(), depth)),
                            Edge::Derived { target_col_id, .. } if self.is_column_node(e.source()) => Some((e.source(), edge.clone(), depth)),
                            Edge::Aggregated { target_col_id, .. } if self.is_column_node(e.source()) => Some((e.source(), edge.clone(), depth)),
                            _ => None,
                        }
                    })
                    .collect()
            } else {
                vec![] // TODO: forward direction
            };

            for (neighbor, edge, d) in edges {
                if !visited.contains(&neighbor) {
                    let result = edge_to_lineage_result(&self[neighbor], &edge, d);
                    let mut new_path = path.clone();
                    new_path.push(result);
                    paths.push(new_path.clone());
                    queue.push_back((neighbor, d + 1, new_path));
                }
            }
        }

        paths
    }
}
```

**验收标准:**
- [ ] 给定图中存在 `col:src_raw.trade_qty → col:std_intermediate.trade_qty [DataFlow]`，调用 `column_lineage("col:std_intermediate.trade_qty", "upstream", 5)` 返回包含该边的路径
- [ ] depth=0 返回空
- [ ] depth=1 只返回直接上游
- [ ] 循环检测正常（不无限递归）

---

### Task 5: CLI 集成 — `lineage` 子命令

**文件:**
- Modify: `src/main.rs`

**目标:** 新增 `Commands::Lineage` 变体，实现 `cmd_lineage()` 处理函数。

**Step 5.1: 新增 CLI 参数定义**

在 `Commands` 枚举中（约 L217 之后）新增：

```rust
/// 查询表级或列级血缘
Lineage {
    /// 起始节点: "table_name" (表级) 或 "table.column" (列级)
    #[arg(value_name = "TARGET")]
    target: String,

    /// 追踪方向
    #[arg(short = 'd', long, default_value = "upstream",
          value_parser = ["upstream", "downstream", "both"])]
    direction: String,

    /// 最大追溯深度
    #[arg(long, default_value = "10")]
    depth: usize,

    /// 输出格式
    #[arg(short = 'f', long, default_value = "tree",
          value_parser = ["tree", "table", "json"])]
    format: String,

    /// 输出文件（默认 stdout）
    #[arg(short = 'o', long)]
    output: Option<PathBuf>,

    /// 项目目录
    #[arg(short = 'p', long, default_value = ".")]
    project: PathBuf,
},
```

**Step 5.2: 新增 dispatch arm**

在 `run()` 函数中（约 L815 之后）新增：

```rust
Some(Commands::Lineage { target, direction, depth, format, output, project }) => {
    cmd_lineage(&target, &direction, depth, &format, output.as_deref(), &project)
}
```

**Step 5.3: 实现 `cmd_lineage()`**

```rust
fn cmd_lineage(
    target: &str,
    direction: &str,
    depth: usize,
    format: &str,
    output: Option<&Path>,
    project: &Path,
) -> Result<()> {
    let proj = load_project(project)?;

    // 判断表级还是列级: 含 '.' 且后有内容为列级
    if target.contains('.') && target.split('.').nth(1).map_or(false, |s| !s.is_empty()) {
        // 列级血缘
        cmd_column_lineage(&proj, target, direction, depth, format, output)
    } else {
        // 表级血缘
        cmd_table_lineage(&proj, target, direction, depth, format, output)
    }
}

fn cmd_column_lineage(
    proj: &Project,
    col_target: &str,  // "table.column"
    direction: &str,
    depth: usize,
    format: &str,
    output: Option<&Path>,
) -> Result<()> {
    let graph = proj.load_graph()?;

    // 构造列节点 ID
    let col_id = format!("col:public.{}", col_target);

    let paths = graph.column_lineage(&col_id, direction, depth);

    if paths.is_empty() {
        eprintln!("No lineage found for column: {}", col_target);
        return Ok(());
    }

    let output_str = match format {
        "tree" => format_lineage_tree(&paths, col_target),
        "table" => format_lineage_table(&paths),
        "json" => serde_json::to_string_pretty(&paths)?,
        _ => unreachable!(),
    };

    if let Some(path) = output {
        std::fs::write(path, output_str)?;
    } else {
        println!("{}", output_str);
    }

    Ok(())
}
```

**Step 5.4: 实现输出格式化函数**

```rust
/// Tree 格式输出
fn format_lineage_tree(paths: &[Vec<ColumnLineageResult>], target: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("{} [{}]\n", target, lineage_kind_label(paths)));

    for (i, path) in paths.iter().enumerate() {
        let is_last = i == paths.len() - 1;
        for (j, step) in path.iter().enumerate() {
            let prefix = if is_last && j == path.len() - 1 { "  └── " } else { "  ├── " };
            let indent = "    ".repeat(j);
            out.push_str(&format!("{}{}{} [{}]\n",
                indent, prefix, step.target_id, step.edge_kind_label()));
        }
    }
    out
}

/// Table 格式输出
fn format_lineage_table(paths: &[Vec<ColumnLineageResult>]) -> String {
    let mut out = String::from("SOURCE_COLUMN | TRANSFORM | TARGET_COLUMN | DEPTH | KIND\n");
    out.push_str("-------------+-----------+---------------+-------+------\n");
    for path in paths {
        for step in path {
            out.push_str(&format!("{} | {} | {} | {} | {}\n",
                step.source_id,
                step.expression.as_deref().unwrap_or("→"),
                step.target_id,
                step.depth,
                step.edge_kind_label(),
            ));
        }
    }
    out
}
```

**验收标准:**
- [ ] `codeweb lineage trade_summary --direction upstream` 执行表级血缘查询
- [ ] `codeweb lineage trade_summary.total_qty --direction upstream` 执行列级血缘查询
- [ ] `--format tree` 输出缩进树格式
- [ ] `--format table` 输出表格格式
- [ ] `--format json` 输出有效 JSON
- [ ] `--output /tmp/result.json` 写入文件
- [ ] `--depth 2` 限制输出深度

---

### Task 6: 测试 + 验证

**文件:**
- Create: `tests/integration_lineage.rs`
- Fixtures: `tests/fixtures/lineage/*.sql` (已创建 12 个)

**Step 6.1: 编写集成测试**

```rust
// tests/integration_lineage.rs

#[cfg(test)]
mod lineage_tests {
    use super::*;

    /// 测试 01: 简单 INSERT SELECT 直通
    #[test]
    fn test_lineage_simple_dataflow() {
        let sql = include_str!("fixtures/lineage/01-simple-dataflow.sql");
        let graph = parse_and_build_graph(sql);
        let paths = graph.column_lineage("col:public.trade_normalized.account_id", "upstream", 5);
        assert!(!paths.is_empty(), "Should find lineage for account_id");
        let first_path = &paths[0];
        assert!(first_path.iter().any(|r| r.edge_kind == ColumnEdgeKind::DataFlow));
    }

    /// 测试 02: 聚合 + GROUP BY
    #[test]
    fn test_lineage_aggregation_groupby() {
        let sql = include_str!("fixtures/lineage/02-aggregation-groupby.sql");
        let graph = parse_and_build_graph(sql);
        let paths = graph.column_lineage("col:public.trade_summary.total_qty", "upstream", 5);
        assert!(!paths.is_empty());
        let first_path = &paths[0];
        assert!(first_path.iter().any(|r| r.edge_kind == ColumnEdgeKind::Aggregated));
        assert!(first_path.iter().any(|r| r.aggregation.as_deref() == Some("SUM")));
    }

    /// 测试 03: DECODE 表达式变换
    #[test]
    fn test_lineage_decode_derived() {
        let sql = include_str!("fixtures/lineage/03-decode-derived.sql");
        let graph = parse_and_build_graph(sql);
        let paths = graph.column_lineage("col:public.normalized_trade.bs_flag", "upstream", 5);
        assert!(!paths.is_empty());
        let first_path = &paths[0];
        assert!(first_path.iter().any(|r| r.edge_kind == ColumnEdgeKind::Derived));
        assert!(first_path.iter().any(|r| r.expression.as_ref().map_or(false, |e| e.contains("DECODE"))));
    }

    /// 测试 04: 算术表达式
    #[test]
    fn test_lineage_arithmetic_derived() {
        let sql = include_str!("fixtures/lineage/04-arithmetic-derived.sql");
        let graph = parse_and_build_graph(sql);
        let paths = graph.column_lineage("col:public.bond_deal_output.trade_amount", "upstream", 5);
        assert!(!paths.is_empty());
    }

    /// 测试 05: 视图列直通
    #[test]
    fn test_lineage_view_passthrough() {
        let sql = include_str!("fixtures/lineage/05-view-passthrough.sql");
        let graph = parse_and_build_graph(sql);
        // 通过视图引用
        let paths = graph.column_lineage("col:public.instruction_base.inst_num", "upstream", 5);
        assert!(!paths.is_empty());
    }

    /// 测试 06: 多表 JOIN 视图
    #[test]
    fn test_lineage_view_join_transform() {
        let sql = include_str!("fixtures/lineage/06-view-join-transform.sql");
        let graph = parse_and_build_graph(sql);
        let paths = graph.column_lineage("col:public.output_deal.trade_qty", "upstream", 5);
        assert!(!paths.is_empty());
    }

    /// 测试 07: UNION ALL 视图
    #[test]
    fn test_lineage_view_unionall() {
        let sql = include_str!("fixtures/lineage/07-view-unionall.sql");
        let graph = parse_and_build_graph(sql);
        let paths = graph.column_lineage("col:public.bond_summary.product_code", "upstream", 5);
        assert!(!paths.is_empty());
        // 应有多条路径（3 个 UNION ALL 分支）
        assert!(paths.len() >= 1, "Should have lineage from UNION ALL sources");
    }

    /// 测试 08: 多层管道 (4 跳)
    #[test]
    fn test_lineage_multi_hop() {
        let sql = include_str!("fixtures/lineage/08-multi-hop-pipeline.sql");
        let graph = parse_and_build_graph(sql);
        let paths = graph.column_lineage("col:public.instruction_final.trade_qty", "upstream", 5);
        assert!(!paths.is_empty());
        // 应该有 3 跳以上
        let max_depth = paths.iter()
            .flat_map(|p| p.iter().map(|r| r.depth))
            .max()
            .unwrap_or(0);
        assert!(max_depth >= 3, "Should trace through at least 3 hops");
    }

    /// 测试 09: 跨表列重命名
    #[test]
    fn test_lineage_cross_table_rename() {
        let sql = include_str!("fixtures/lineage/09-column-rename-cross-table.sql");
        let graph = parse_and_build_graph(sql);
        let paths = graph.column_lineage("col:public.target_unified.account_id", "upstream", 5);
        assert!(!paths.is_empty());
        // 应有 3 个源（zqzh/gddm/account → account_id）
        assert!(paths.len() >= 3, "Should trace to 3 source columns");
    }

    /// 测试 10: UNION ALL 多源 INSERT
    #[test]
    fn test_lineage_unionall_multisource() {
        let sql = include_str!("fixtures/lineage/10-unionall-multisource.sql");
        let graph = parse_and_build_graph(sql);
        let paths = graph.column_lineage("col:public.trade_consolidated.bs_side", "upstream", 5);
        assert!(!paths.is_empty());
    }

    /// 测试 11: 匿名块 + 存储过程
    #[test]
    fn test_lineage_anonymous_block() {
        let sql = include_str!("fixtures/lineage/11-anonymous-block.sql");
        let graph = parse_and_build_graph(sql);
        let paths = graph.column_lineage("col:public.daily_output.total_qty", "upstream", 5);
        assert!(!paths.is_empty());
    }

    /// 测试 12: UPDATE 自引用
    #[test]
    fn test_lineage_self_update() {
        let sql = include_str!("fixtures/lineage/12-self-update.sql");
        let graph = parse_and_build_graph(sql);
        let paths = graph.column_lineage("col:public.bond_positions.position_qty", "upstream", 5);
        assert!(!paths.is_empty());
    }
}
```

**Step 6.2: 运行测试**

```bash
cargo test --test integration_lineage
cargo test --features full  # 确保所有既有测试不受影响
cargo clippy --features full -- -D warnings
cargo fmt -- --check
```

**验收标准:**
- [ ] ≥12 个新增测试全部通过
- [ ] `cargo test --features full` 通过（0 failures）
- [ ] `cargo clippy --features full -- -D warnings` 无警告
- [ ] `cargo fmt -- --check` 通过
- [ ] 现有 ColumnAccessExtractor 的 18 个测试仍然通过（零回归）

---

## 汇总时间表

| Task | 内容 | 工期 |
|------|------|------|
| Task 1 | ColumnLineageExtractor 核心 | 1-1.5 天 |
| Task 2 | 图模型扩展 (Node/Edge + match 分支) | 0.5-1 天 |
| Task 3 | Builder 集成 | 0.5-1 天 |
| Task 4 | 遍历引擎 column_lineage() | 0.5 天 |
| Task 5 | CLI 集成 | 0.5-1 天 |
| Task 6 | 测试 + 验证 | 0.5-1 天 |

**总计:** 约 3.5-5.5 个工作日（单人全职）。

可并行: Task 1 + Task 5（CLI 壳不依赖提取器），Task 2（图模型可提前定义），Task 4（遍历逻辑可基于 mock 图先写）。

---

## 风险与应对

| 风险 | 影响 | 应对 |
|------|------|------|
| `ogsql-parser` 表达式文本提取不可用 | `Derived` 的 `expression` 字段为空 | MVP 标记 `expression: None`，后续从原始 SQL 提取 |
| 大表列节点爆炸 | 图膨胀、查询变慢 | 按需创建节点（仅 SELECT 实际引用的列），不预创建 |
| 视图定义跨文件 | 视图展开失败 | MVP 仅处理同文件内视图；跨文件留后续 |
| 新增 Node/Edge 变体后遗漏 match 分支 | 编译错误 | Rust 编译器强制覆盖所有 match 分支，不会遗漏 |
| bincode store 版本不兼容 | 旧数据无法加载 | `STORE_VERSION=7`，旧版本数据提示重新分析 |
| 列级分析拖慢 `analyze` | 污染默认体验 | 列级提取仅 `analyze --column-lineage` 时启用 |

---

## 依赖关系

```
Task 1 (ColumnLineageExtractor)
  │
  ├──→ Task 2 (图模型) ──┐
  │                       ├──→ Task 3 (Builder集成)
  │                       │      │
  │                       │      └──→ Task 4 (遍历引擎)
  │                       │             │
  │                       │             └──→ Task 6 (测试)
  │                       │
  └──→ Task 5 (CLI) ──────┘
```

Task 1 和 Task 5 可并行开始（CLI 先做壳，Task 4 完成后再接数据）。

---

## 测试策略

| 测试类型 | 内容 | 对应文件 |
|----------|------|---------|
| 单元测试 | `ColumnLineageExtractor` 对每种 SELECT 模式产生正确的 `ColumnEdge` | `src/parser/column_lineage.rs` (inline `#[cfg(test)]`) |
| 集成测试 | 完整 SQL → 图 → `column_lineage()` 查询 → 验证结果 | `tests/integration_lineage.rs` |
| Fixture 测试 | 12 个 `tests/fixtures/lineage/*.sql` 逐文件验证 | 集成测试中 `include_str!()` |
| 回归测试 | 不启用列级时既有功能零影响 | `cargo test --features full` |
| CLI 测试 | `codeweb lineage` 各参数组合 | 集成测试中调用 CLI 命令 |
