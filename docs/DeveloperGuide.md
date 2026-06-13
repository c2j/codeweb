# codeweb 开发指南

面向 MCP 集成、API 调用和功能扩展的开发者的架构参考文档。

## 目录

- [架构概览](#架构概览)
- [核心数据模型](#核心数据模型)
- [GraphStore 存储层](#graphstore-存储层)
- [HTTP API 设计](#http-api-设计)
- [声明式查询引擎（QuerySpec）](#声明式查询引擎queryspec)
- [CGEF 图谱交换格式](#cgef-图谱交换格式)
- [导出系统](#导出系统)
- [扩展指南](#扩展指南)
- [性能与索引](#性能与索引)
- [国际化（i18n）](#国际化i18n)
- [Feature Flags 与条件编译](#feature-flags-与条件编译)

---

## 架构概览

codeweb 采用分层架构，数据从源码文件流入，经过解析、图构建、索引化，最终通过 CLI / API / 导出对外暴露：

```
┌──────────────────────────────────────────────────────────┐
│                      对外接口层                           │
│   CLI (clap)    │   HTTP API (axum)   │   TUI (ratatui)  │
├──────────────────────────────────────────────────────────┤
│                      查询与分析层                         │
│   traverse.rs   │   query/spec.rs    │   search/         │
├──────────────────────────────────────────────────────────┤
│                      存储与索引层                         │
│   store.rs      │   GraphStore       │   多级索引         │
├──────────────────────────────────────────────────────────┤
│                      图模型层                             │
│   builder.rs    │   key.rs           │   resolver.rs     │
├──────────────────────────────────────────────────────────┤
│                      解析层                               │
│   extractor.rs  │   ibatis_loader.rs │   java_loader.rs  │
├──────────────────────────────────────────────────────────┤
│                      数据源                               │
│   .sql          │   .xml (MyBatis)   │   .java           │
└──────────────────────────────────────────────────────────┘
```

### 数据流

1. **扫描**：`parser/scanner.rs` 遍历源码目录，识别文件类型（SQL/Java/XML）
2. **指纹**：`parser/fingerprint.rs` 计算文件内容哈希，驱动增量分析
3. **解析**：
   - SQL：`ogsql-parser` 解析存储过程/函数定义，`extractor.rs` 提取 CALL 关系
   - XML：`ogsql-parser`（`ibatis` feature）解析 MyBatis 映射器
   - Java：`tree-sitter-java` 提取方法调用，`ogsql-parser`（`java` feature）提取内嵌 SQL
4. **建图**：`graph/builder.rs` 将解析结果构建为 `petgraph` 有向图
5. **存储**：`graph/store.rs` 序列化为 bincode/JSON，构建多级索引
6. **查询**：`graph/query/` 提供声明式遍历引擎

---

## 核心数据模型

### Node（节点）

定义在 `ogsql-parser` 库中，导入到 `graph` 模块。节点类型枚举（`Node`）：

| 变体 | 类型标签 | 说明 | 关键字段 |
|------|---------|------|---------|
| `Procedure` | `proc` / `proc*` | 存储过程 | `id`（NodeId）, `location`, `body_sql`, `partial` |
| `Function` | `func` / `func*` | 函数 | 同上 |
| `Table` | `table` | 数据库表 | `schema`, `name`, `columns`, `partition_by`, `distribute_by`, `temporary`, `unlogged` |
| `View` | `view` | 视图 | `schema`, `name` |
| `MaterializedView` | `mview` | 物化视图 | `schema`, `name` |
| `MappedStatement` | `mapper` | MyBatis 映射语句 | `namespace`, `statement_id`, `kind`, `xml_file`, `line`, `sql` |
| `JavaMethod` | `method` | Java 方法 | `fqn`, `class_fqn`, `name`, `signature`, `file`, `line` |
| `JavaClass` | `class` | Java 类/接口 | `fqn`, `name`, `package`, `file`, `line` |
| `JavaSql` | `sql` | Java 内嵌 SQL | `class_name`, `method_name`, `extraction_method`, `java_file`, `line`, `sql` |
| `Package` | `pkg` | 数据库包 | `schema`, `name` |
| `Trigger` | `trigger` | 触发器 | `name`, `table` |
| `Type` | `type` | 自定义类型 | `schema`, `name`, `type_kind` |
| `Sequence` | `seq` | 序列 | `schema`, `name` |
| `Index` | `index` | 索引 | `table_name`, `name`, `unique` |
| `Synonym` | `synonym` | 同义词 | `name`, `target_name`, `target_schema` |
| `Event` | `event` | 定时事件 | `name` |
| `Unresolved` | `unres` | 未解析引用 | `raw_expr`, `context` |
| `Custom` | *(自定义)* | CGEF 自定义节点 | `type_name`, `key_fields`, `properties` |

> `proc*` / `func*` 表示 **partial** 节点：存储过程/函数的 body 包含不支持的语法，仅提取了签名。

### Edge（边）

边类型定义及分类：

| 边类型 | 语义 | 分类 |
|--------|------|------|
| `DirectCall` | 直接静态调用（CALL proc） | `call` |
| `DynamicCall` | 动态调用（EXECUTE IMMEDIATE） | `call` |
| `CallsProcedure` | Mapper/JavaSql 调用存储过程 | `call` |
| `InvokesMapper` | Java 类通过 namespace.method 调用 Mapper | `call` |
| `CallsJava` | Java 方法间调用 | `call` |
| `TableAccess` | 表访问（含读写模式位掩码） | `dataflow` |
| `DependsOn` | 视图/物化视图依赖表 | `dataflow` |
| `ContainsRoutine` | 包包含例程（package→procedure） | `composition` |
| `ContainsMethod` | 类包含方法（class→method） | `composition` |
| `Extends` | 类继承 | `inheritance` |
| `Implements` | 接口实现 | `inheritance` |
| `TriggersRoutine` | 触发器调用例程 | `reference` |
| `ReferencesType` | 引用自定义类型 | `reference` |
| `UsesSequence` | 使用序列 | `reference` |
| `IndexesTable` | 索引表 | `reference` |
| `AliasesObject` | 同义词别名 | `reference` |
| `CustomEdge` | CGEF 自定义边 | `reference` |

### NodeKey（节点标识）

每个节点的唯一标识键，格式为 `<type_tag>:<路径>`：

| 节点类型 | 格式 | 示例 |
|---------|------|------|
| Procedure | `proc:<schema>.<pkg>.<name>` | `proc:public.pkg_order.create_order` |
| Table | `table:<schema>.<name>` | `table:public.orders` |
| MappedStatement | `mapper:<ns>.<id>` | `mapper:com.example.OrderMapper.selectById` |
| JavaMethod | `method:<fqn>` | `method:com.example.OrderService.findOrder(String)` |
| Unresolved | `unres:<expr>` | `unres:v_dynamic_sql` |
| Custom | `<type>:<keys>` | `esb_service:erp:placeOrder:v2` |

### CodeGraph（图结构）

```rust
// 基于 petgraph 的有向图
pub struct CodeGraph {
    graph: DiGraph<Node, Edge, NodeIndex>,
}

impl CodeGraph {
    pub fn node_count(&self) -> usize;
    pub fn edge_count(&self) -> usize;
    pub fn node_indices(&self) -> ...;
    pub fn neighbors_directed(&self, idx: NodeIndex, dir: Direction) -> ...;
}
```

---

## GraphStore 存储层

`GraphStore` 是图数据的主要入口，它封装了 `CodeGraph` 并添加了多级索引，支持快速的查询和搜索。

### 内部索引

| 索引 | 类型 | 用途 |
|------|------|------|
| `node_key_index` | `HashMap<NodeKey, NodeIndex>` | O(1) 节点查找 |
| `node_summaries` | `Vec<NodeSummary>` | 快速列表/过滤（预计算 key, type_tag, degree） |
| `type_tag_index` | `HashMap<String, Vec<NodeIndex>>` | 按类型标签过滤 |
| `name_index` | `Vec<(String, NodeIndex)>` | 名称子串搜索 |
| `schema_index` | `HashMap<String, Vec<NodeIndex>>` | Schema 过滤 |
| `edge_category_index` | `HashMap<String, Vec<EdgeIndex>>` | 边类别快速过滤 |
| `sql_fingerprint_index` | `HashMap<String, Vec<(NodeIndex, String)>>` | SQL 指纹 O(1) 查找（`search-sql-v2`） |
| `file_nodes` | `HashMap<PathBuf, Vec<NodeKey>>` | 文件→节点映射 |
| `reverse_deps` | `HashMap<PathBuf, HashSet<PathBuf>>` | 文件间反向依赖 |

### 序列化

GraphStore 支持两种序列化格式，通过 `--output` 扩展名自动选择：

| 格式 | 扩展名 | 特点 | 方法 |
|------|--------|------|------|
| Bincode | `.bincode` | 二进制，紧凑快速（推荐生产） | `save_bincode()` / `load_bincode()` |
| JSON | `.json` | 人类可读，便于调试 | `save_json()` / `load_json()` |

### 合并（Merge）

`GraphStore::merge()` 支持将多个 GraphStore 合并：

```rust
let merged = GraphStore::merge(vec![store_a, store_b], "merged-name");
```

合并规则：
- 节点按 `NodeKey` 去重（相同 key 合并）
- 边按 `(source, target, type)` 三元组去重
- `TableAccess` 边的 `modes` 集合取并集
- 存储过程采用三阶段匹配：精确 → 正向 relaxed → 反向 relaxed

### 去重（Dedup）

`GraphStore::dedup()` 清理因多次合并或增量分析累积的重复节点和边。

---

## HTTP API 设计

HTTP API 通过 `axum` 框架提供，所有端点以 `/api/v1/` 为前缀，启用 CORS。

### 端点列表

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/v1/stats` | 项目统计（各类节点/边数量） |
| GET | `/api/v1/files` | 已分析文件列表 |
| GET | `/api/v1/nodes` | 节点列表（支持 `search`, `node_type`, `orphan`, `low_degree`, `limit`, `offset`） |
| GET | `/api/v1/nodes/:id` | 节点详情（含 callers, callees, properties） |
| GET | `/api/v1/nodes/:id/callers` | 节点上游调用方（分页） |
| GET | `/api/v1/nodes/:id/callees` | 节点下游被调用方（分页） |
| GET | `/api/v1/nodes/search-sql` | 按 SQL 文本搜索（`q` 参数） |
| GET | `/api/v1/trace` | 双向调用链追踪（`from`, `depth`, `max_nodes`） |
| POST | `/api/v1/query` | 执行 QuerySpec 声明式查询 |
| GET | `/api/v1/export` | 导出图谱（`format` 参数：dot/json/mermaid） |
| GET | `/api/v1/graph` | 完整图谱 JSON 数据 |

### 响应格式

所有成功响应为 JSON，错误响应使用标准 HTTP 状态码：

| 状态码 | 说明 |
|--------|------|
| 200 | 成功 |
| 400 | 请求参数错误 |
| 404 | 资源不存在 |
| 500 | 服务器内部错误 |

### 浏览器 UI

嵌入的静态资源通过 `rust-embed` 编译进二进制文件，由 `axum` 的 fallback 处理器提供。UI 使用 Cytoscape.js 渲染图，da-gre 进行自动布局。

---

## 声明式查询引擎（QuerySpec）

QuerySpec 是 codeweb 最强大的查询接口，允许通过 JSON 描述复杂的多步图遍历。

### 结构

```json
{
  "start": { "type": "proc", "name": "calculate", "schema": "public" },
  "steps": [
    { "action": "outgoing", "edge_categories": ["call"], "max_depth": 3 },
    { "action": "filter", "type_tag": "table" }
  ],
  "collect": "nodes"
}
```

### StartSpec

选择起始节点，三个字段取交集：

```json
{ "type": "proc", "name": "order", "schema": "public" }
```

### StepSpec

支持四种操作：

| action | 说明 | 参数 |
|--------|------|------|
| `outgoing` | 沿出边遍历（下游） | `edge_categories`（可选，默认所有边）, `max_depth`（可选） |
| `incoming` | 沿入边遍历（上游） | `edge_categories`（可选）, `max_depth`（可选） |
| `filter` | 过滤当前节点集 | `type_tag`（可选）, `schema`（可选） |
| `until` | 遍历直到遇到指定类型 | `type_tag`（必填） |

### CollectMode

| 值 | 返回 |
|----|------|
| `nodes` | 去重后的节点列表 |
| `paths` | 所有遍历路径（节点序列数组） |
| `subgraph` | 子图模式 |

### 执行流程

1. `spec.rs` 解析 JSON → `QuerySpec` 结构体
2. `StartSpec` 匹配起始节点（通过 `GraphStore` 的索引）
3. 对每个起始节点，按 `steps` 顺序执行
4. 每步由 `traversal.rs` 执行 BFS 图遍历，`filter.rs` 进行节点/边过滤
5. 根据 `CollectMode` 收集结果
6. 返回 `QueryResult` JSON

---

## CGEF 图谱交换格式

CGEF（Codeweb Graph Exchange Format）是 codeweb 定义的 JSON 图谱交换格式，用于外部系统的语义关系数据导入。

### 文档结构

```json
{
  "format_version": 1,
  "metadata": { "source": "...", "generated_at": "...", "description": "..." },
  "node_schemas": { "custom_type": { "display_name": "...", "key_fields": [...], "properties": {...} } },
  "edge_schemas": { "custom_edge": { "display_name": "...", "source_types": [...], "target_types": [...] } },
  "nodes": [ ... ],
  "edges": [ ... ]
}
```

### 导入流程

1. **解析**：`import/parser.rs` 将 CGEF JSON 解析为中间表示
2. **校验**：`import/validator.rs` 检查格式版本、节点 ID 唯一性、边引用完整性、自定义类型声明
3. **路径映射**：`import/path_mapper.rs` 处理 `--prefix` 参数
4. **建图**：将验证通过的节点/边转换为 `CodeGraph`
5. **存储**：保存为 `GraphStore`

### 自定义类型

通过 `node_schemas` 和 `edge_schemas` 声明自定义节点/边类型。自定义节点按 `(type_name, key_fields 的 JSON 序列化)` 去重。

### MCP 集成场景

codeweb 提供四种 MCP/外部集成方式：

1. **原生 MCP 服务器**（推荐）：`codeweb mcp` 启动原生 MCP 服务器，通过 stdio JSON-RPC 通信。零配置，直接在 Claude Desktop / Cursor 等 MCP 客户端中使用。
2. **直接 HTTP API 调用**：`codeweb serve` 启动服务后，通过 HTTP 客户端调用 RESTful API
3. **CGEF 导入**：生成 CGEF JSON 文件，通过 `codeweb import` 导入
4. **程序化集成**：将 codeweb 作为 Rust 库依赖，直接调用 `GraphStore` API

#### 原生 MCP 服务器（`codeweb mcp`）

使用 `--features mcp` 构建，基于 [rmcp](https://crates.io/crates/rmcp) 实现。

**MCP 工具一览：**

| 工具 | 参数 | 说明 |
|------|------|------|
| `codeweb_stats` | 无 | 项目统计（各类节点/边/文件数量） |
| `codeweb_nodes` | `search`, `node_type`, `limit`, `offset` | 节点列表（搜索、类型过滤、分页） |
| `codeweb_node_detail` | `id` (usize) | 节点详情：属性 + callers + callees |
| `codeweb_trace` | `from`, `depth`, `max_nodes` | 双向调用链追踪 |
| `codeweb_search_sql` | `sql` | SQL 片段搜索 |
| `codeweb_query` | `spec` (QuerySpec JSON) | 声明式复杂遍历 |

**Claude Desktop 配置示例：**

```json
{
  "mcpServers": {
    "codeweb": {
      "command": "/path/to/codeweb",
      "args": ["mcp", "--project", "/path/to/your/project"]
    }
  }
}
```

**架构：**

- `src/mcp/tools.rs` — MCP 工具定义（`#[tool_router]` + `#[tool_handler]`）
- `src/mcp/server.rs` — 服务入口（加载 GraphStore → 启动 tokio runtime → stdio 传输）
- 复用 `GraphStore` 的全部索引和查询能力，与 HTTP API 共享后端

### 程序化 API 示例

```rust
use codeweb::graph::store::GraphStore;
use codeweb::graph::query::spec::QuerySpec;

// 加载存储
let store = GraphStore::load_bincode(path)?;

// 搜索节点
let matches = store.search_nodes("order");

// 按 SQL 搜索
let sql_matches = store.search_by_sql("SELECT * FROM orders");

// 执行 QuerySpec
let spec: QuerySpec = serde_json::from_str(&json)?;
let result = spec.execute(&store)?;
```

---

## 导出系统

支持三种导出格式：

| 格式 | Content-Type | 模块 | 用途 |
|------|-------------|------|------|
| DOT | `text/vnd.graphviz` | `export/dot.rs` | Graphviz 渲染图片 |
| JSON | `application/json` | `export/json.rs` | 程序化处理 |
| Mermaid | `text/plain` | `export/mermaid.rs` | Markdown 嵌入 |

---

## 扩展指南

### 添加新节点类型

1. **在 ogsql-parser 中定义**（如需新解析逻辑）
2. **更新 `graph/builder.rs`**：添加构建逻辑
3. **更新 `graph/key.rs`**：添加 `NodeKey` 变体
4. **更新 `main.rs` 中的 `node_type_tag()`**：添加类型标签
5. **更新统计**：`GraphStore::stats()`
6. **更新导出**：DOT/JSON/Mermaid 各文件
7. **更新 HTTP API**：`handlers.rs` 中的属性展示
8. **更新 TUI**：节点渲染逻辑
9. **更新文档**：所有文档中的节点类型表

### 添加新边类型

1. **在 ogsql-parser 中定义**（如需新解析逻辑）
2. **更新 `graph/builder.rs`**：添加边的构建
3. **更新查询过滤器**：`query/filter.rs` 中的 `EdgeCategory`
4. **更新导出**：DOT/JSON/Mermaid
5. **更新 HTTP API**：trace 中的 `edge_label`
6. **更新 CGEF**：如需支持导入/导出

### 添加新 CLI 命令

1. 在 `main.rs` 的 `Commands` 枚举中添加变体
2. 添加 `cmd_xxx()` 处理函数
3. 在 `run()` 的 `match` 中添加分支
4. 添加测试

### 添加新 HTTP API 端点

1. 在 `server/handlers.rs` 中添加处理函数
2. 在 `router()` 中注册路由
3. 更新 `serve-api-guide.md`

---

## 性能与索引

### 索引策略

GraphStore 构建多级索引，在加载时一次性构建：

- **节点查找**：`node_key_index` 提供 O(1) 查找
- **类型过滤**：`type_tag_index` 避免全图扫描
- **搜索**：`name_index` 支持子串匹配，结合 `MatchRank` 排序
- **SQL 搜索**：`sql_fingerprint_index`（feature: `search-sql-v2`）提供 O(1) 指纹查找

### 增量分析

通过 `fingerprint.rs` 计算文件内容的 BLAKE3 哈希，与上次分析的 `manifest` 比对：
- 未修改文件跳过解析
- 新增文件解析后加入图
- 删除文件的节点和边从图中移除
- 修改文件重新解析并更新

### 并发

- 文件扫描和解析使用 `rayon` 并行处理
- macOS 上自动设置工作线程 QoS 为 `QOS_CLASS_UTILITY`
- 线程数默认为 `max(4, CPU核心数 - 2)`

---

## 国际化（i18n）

使用 `rust-i18n` 框架，翻译文件位于 `locales/` 目录：

```
locales/
├── en.yml          # 英文翻译
└── zh-CN.yml       # 中文翻译
```

配置在 `Cargo.toml` 中：

```toml
[package.metadata.i18n]
available_locales = ["en", "zh-CN"]
default_locale = "zh-CN"
fallback_locale = "en"
```

使用方式：

```rust
// Rust 代码中
flt!("key_name");

// 或通过环境变量切换
// LANG=zh-CN codeweb ...
```

---

## Feature Flags 与条件编译

| Feature | 门控内容 | 启用时 | 用途 |
|---------|---------|--------|------|
| `cli` | `main.rs` 中 `Commands` 枚举及其处理逻辑 | `#[cfg(feature = "cli")]` | 命令行界面 |
| `tui` | `tui/` 模块 | `#[cfg(feature = "tui")]` | 终端 UI |
| `serve` | `server/` 模块，依赖 axum/tokio/tower-http/rust-embed/mime_guess | `#[cfg(feature = "serve")]` | HTTP 服务器 + 浏览器 UI |
| `mcp` | `mcp/` 模块，依赖 rmcp/schemars/tokio | `#[cfg(feature = "mcp")]` | MCP 服务器（LLM 集成） |
| `search-sql-v2` | `sql_fingerprint_index` 构建与查询逻辑 | `#[cfg(feature = "search-sql-v2")]` | 增强 SQL 搜索 |

### 条件编译模式

```rust
// 在代码中
#[cfg(feature = "serve")]
mod server;

// 或通过 Cargo.toml
[features]
default = ["cli", "tui"]
full = ["cli", "tui", "serve"]
```

---

## 相关文档

| 文档 | 说明 |
|------|------|
| [用户指南](user-guide.md) | 面向最终用户的使用手册 |
| [Serve API 指南](serve-api-guide.md) | HTTP API 完整参考 |
| [CGEF 用户指南](cgef-user-guide.md) | 图谱导入/合并格式说明 |
| [OP 血缘转 CGEF 指南](op2cgef-guide.md) | 企业 Excel 血缘数据转换 |
| [贡献指南](../CONTRIBUTION.md) | 面向合作开发者的贡献规范 |
| [README](../README.md) | 项目概览与快速开始 |
