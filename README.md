# codeweb

**Semantic Code Graph Analyzer** — build traversable call/reference graphs across SQL, Java, and MyBatis.

[English](#english) | [中文](#中文)

---

<a id="english"></a>

## Overview

codeweb analyzes semantic relationships in source code and produces a directed call/reference graph. It starts from SQL stored procedure call relationships (openGauss / GaussDB dialect), extends through MyBatis/iBatis XML mappers, and bridges to Java method calls — forming a complete `Java Method → Mapper → SQL → Stored Procedure` chain.

```
Java Method ──InvokesMapper──▶ MappedStatement ──CallsProcedure──▶ Procedure ──DirectCall──▶ Procedure
                                                                    │
                                                                    └──TableAccess──▶ Table
```

## Features

- **SQL Stored Procedure Call Graph** — Parse SQL files to extract `CALL` / `EXECUTE` relationships between stored procedures, functions, and packages
- **MyBatis/iBatis XML Mapper Chain** — Link `MappedStatement → SQL → Stored Procedure` via XML mapper files
- **Java Method Calls + Bridge** — Parse Java source to extract method calls and bridge `Java Method → Mapper → SQL → Stored Procedure`
- **Bidirectional Query Engine** — `callers()`, `callees()`, `trace()` (bidirectional), `impact()` (change analysis)
- **Incremental Analysis** — Only re-parse changed files for fast iteration
- **Multiple Export Formats** — DOT (Graphviz), JSON, Mermaid
- **CGEF Import/Merge** — Import external graph data via [CGEF](docs/cgef-user-guide.md) format and merge with local analysis; supports custom node/edge types
- **Interactive TUI** — Terminal-based graph explorer with keyboard navigation, search, and node detail
- **HTTP Server + Browser UI** — Cytoscape.js-based interactive visualization with RESTful API (feature-gated behind `serve`)
- **Declarative Query API** — JSON QuerySpec for complex multi-step traversals with filter, path collection, and subgraph extraction
- **SQL Fragment Search** — Find MappedStatement / JavaSql nodes by SQL content and trace back to Java callers
- **i18n** — Chinese and English interface (via rust-i18n)

## Stack

| Component | Technology |
|-----------|-----------|
| Language | Rust (latest stable) |
| SQL Parsing | [ogsql-parser](https://github.com/c2j/ogsql-parser) (hand-written recursive descent parser for openGauss/GaussDB) |
| Graph | [petgraph](https://crates.io/crates/petgraph) (in-memory directed graph) |
| Java Parsing | [tree-sitter-java](https://crates.io/crates/tree-sitter-java) |
| XML Parsing | via ogsql-parser `ibatis` feature |
| CLI | [clap](https://crates.io/crates/clap) |
| TUI | [ratatui](https://crates.io/crates/ratatui) + [crossterm](https://crates.io/crates/crossterm) |
| HTTP Server | [axum](https://crates.io/crates/axum) + [tokio](https://crates.io/crates/tokio) (feature-gated) |
| Browser UI | Cytoscape.js + dagre layout (embedded via [rust-embed](https://crates.io/crates/rust-embed)) |
| i18n | [rust-i18n](https://crates.io/crates/rust-i18n) (en, zh-CN) |
| Progress | [indicatif](https://crates.io/crates/indicatif) |
| Hashing | [blake3](https://crates.io/crates/blake3) (file fingerprinting) |
| MCP | [rmcp](https://crates.io/crates/rmcp) (Model Context Protocol server, feature-gated) |

## Installation

### Build from Source

```bash
git clone https://github.com/c2j/cobweb.git
cd cobweb

# Build CLI + TUI (default features)
cargo build

# Build with HTTP server + browser UI
cargo build --features serve

# Build all features
cargo build --features full
```

### Feature Flags

| Feature | Description | Default |
|---------|-------------|---------|
| `cli` | CLI via clap | ✅ |
| `tui` | Interactive terminal UI | ✅ |
| `serve` | HTTP server + browser UI | ❌ |
| `mcp` | MCP server for LLM integration (Claude Desktop, Cursor, etc.) | ❌ |
| `jsp` | JSP embedded SQL extraction (scriptlet JDBC, declarations) | ❌ |
| `full` | All features (cli + tui + serve + mcp + jsp + search-sql-v2) | ❌ |
| `search-sql-v2` | Enhanced SQL search with fingerprint-based indexing | ❌ |

## Quick Start

```bash
# Initialize a new project and analyze
codeweb init my-project -d ./src/main/java -d ./src/main/resources/mapper -d ./sql

# Or analyze an existing project
cd my-project
codeweb analyze

# View project statistics
codeweb stats

# List analyzed files
codeweb files

# Show changes since last analysis
codeweb diff

# Trace call chain from a node
codeweb trace --from "process_order"

# Show node details with callers/callees
codeweb detail "calculate_total"

# Search nodes by SQL fragment and trace to Java callers
codeweb trace-sql "SELECT * FROM orders WHERE"

# List nodes with filtering
codeweb nodes -s order -t proc
codeweb nodes --orphan
codeweb nodes --has-partition

# Deduplicate graph nodes and edges
codeweb dedup --dry-run
codeweb dedup

# Execute declarative JSON QuerySpec
codeweb query --spec '{"start":{"type":"proc","name":"order"},"collect":"nodes"}'

# Export graph
codeweb export --format dot --output graph.dot
codeweb export --format mermaid --output graph.mmd
codeweb export --format json --output graph.json

# Interactive TUI
codeweb tui

# HTTP server with browser UI
codeweb serve --addr 127.0.0.1:3000 --open

# MCP server for LLM integration
codeweb mcp

# Import external CGEF graph and merge
codeweb import --file enterprise-graph.json --output erp-store.bincode
codeweb merge -o full-graph.bincode my-project.bincode erp-store.bincode
```

## CLI Reference

| Command | Description |
|---------|-------------|
| `codeweb init <name> -d <dirs>` | Initialize and analyze a new project |
| `codeweb analyze` | Analyze project (full or incremental) |
| `codeweb diff` | Show changes since last analysis |
| `codeweb export` | Export graph to DOT/JSON/Mermaid |
| `codeweb trace --from <node>` | Trace complete call chain from a node |
| `codeweb detail <node>` | Show callers/callees detail for a node |
| `codeweb stats` | Show project statistics |
| `codeweb files` | List analyzed files with node counts |
| `codeweb nodes` | List graph nodes with filtering |
| `codeweb trace-sql <sql>` | Search by SQL fragment and trace to Java methods |
| `codeweb query` | Execute declarative JSON QuerySpec |
| `codeweb import` | Import CGEF JSON graph file |
| `codeweb merge` | Merge multiple graph stores |
| `codeweb dedup` | Deduplicate graph nodes and edges |
| `codeweb tui` | Open interactive TUI |
| `codeweb serve` | Start HTTP server with browser UI |
| `codeweb mcp` | Start MCP server (stdio JSON-RPC for LLM clients) |

## Node Types

| Type | Tag | Description |
|------|-----|-------------|
| Procedure | `proc` | Stored procedure |
| Function | `func` | Function |
| Table | `table` | Database table |
| View | `view` | View |
| MappedStatement | `mapper` | MyBatis/iBatis mapped statement |
| JavaMethod | `method` | Java method |
| JavaClass | `class` | Java class |
| JavaSql | `sql` | SQL embedded in Java (annotations, JDBC) |
| JspPage | `jsp` | JSP page (only with `jsp` feature) |
| JspSql | `jsql` | SQL embedded in JSP (only with `jsp` feature) |
| Package | `pkg` | Database package |
| Trigger | `trigger` | Database trigger |
| Type | `type` | Custom type |
| Sequence | `seq` | Sequence |
| Index | `index` | Index |
| MaterializedView | `mview` | Materialized view |
| Synonym | `synonym` | Synonym |
| Event | `event` | Event |
| Unresolved | `unres` | Unresolved reference |

## HTTP API (serve mode)

When built with `--features serve`, codeweb provides a RESTful API:

| Method | Path | Description |
|--------|------|-------------|
| GET | `/api/v1/stats` | Project statistics |
| GET | `/api/v1/nodes` | Node list (search, filter, paginate) |
| GET | `/api/v1/nodes/:id` | Node detail (callers, callees, properties) |
| GET | `/api/v1/nodes/:id/callers` | Upstream callers |
| GET | `/api/v1/nodes/:id/callees` | Downstream callees |
| GET | `/api/v1/nodes/search-sql` | Search nodes by SQL fragment |
| GET | `/api/v1/trace` | Bidirectional call chain tracing |
| POST | `/api/v1/query` | Execute declarative QuerySpec |
| GET | `/api/v1/export` | Export graph (DOT/JSON/Mermaid) |

See [docs/serve-api-guide.md](docs/serve-api-guide.md) for full API documentation.

## MCP Integration (mcp mode)

When built with `--features mcp`, codeweb can run as an MCP (Model Context Protocol) server, allowing LLM clients (Claude Desktop, Cursor, VS Code Copilot Chat, etc.) to query the code graph directly via stdio JSON-RPC.

### Quick Start

```bash
# Build with MCP support
cargo build --features mcp

# Run MCP server (stdio JSON-RPC)
codeweb mcp --project /path/to/your/project
```

### Claude Desktop Configuration

Add to `claude_desktop_config.json`:

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

### MCP Tools

| Tool | Description |
|------|-------------|
| `codeweb_stats` | Project statistics (node/edge/file counts by type) |
| `codeweb_nodes` | List nodes with search, type filter, pagination |
| `codeweb_node_detail` | Node properties, callers, and callees by ID |
| `codeweb_trace` | Bidirectional call chain tracing from a node name |
| `codeweb_search_sql` | Search nodes by SQL text content with scoring |
| `codeweb_query` | Execute declarative JSON QuerySpec for complex traversals |

## Project Structure

```
src/
├── main.rs              # CLI entry point
├── error.rs             # Error types (thiserror)
├── parse_log.rs         # Parse warning/error logging
├── parser/              # SQL/Java/XML parsing layer
│   ├── extractor.rs     # Call relationship extractor
│   ├── loader.rs        # File loading
│   ├── scanner.rs       # Source file scanner
│   ├── ibatis_loader.rs # MyBatis XML mapper loading
│   ├── java_loader.rs   # Java source loading
│   ├── java_method.rs   # Java method extraction (tree-sitter)
│   └── fingerprint.rs   # File fingerprinting for incremental analysis
├── graph/               # Graph model layer
│   ├── store.rs         # GraphStore (serialization, merge, dedup, indexing)
│   ├── builder.rs       # Graph construction
│   ├── traverse.rs      # Traversal & chain formatting
│   ├── resolver.rs      # Node resolution
│   ├── key.rs           # Node key types
│   ├── search/          # Fuzzy search & SQL search
│   └── query/           # Declarative query engine (QuerySpec)
├── export/              # Export layer
│   ├── dot.rs           # Graphviz DOT format
│   ├── json.rs          # JSON serialization
│   └── mermaid.rs       # Mermaid flowchart
├── import/              # CGEF import layer
│   ├── format.rs        # CGEF document model
│   ├── parser.rs        # CGEF parser
│   ├── validator.rs     # Validation
│   ├── schema.rs        # Custom schema registry
│   └── path_mapper.rs   # Path prefix mapping
├── server/              # HTTP server (feature-gated)
│   ├── mod.rs           # Server entry & startup
│   ├── handlers.rs      # API route handlers
│   ├── state.rs         # Application state
│   ├── assets.rs        # Embedded browser UI
│   └── access_log.rs    # HTTP access logging
├── mcp/                 # MCP server for LLM integration (feature-gated)
│   ├── mod.rs           # Module declarations
│   ├── server.rs        # Server entry & startup
│   └── tools.rs         # MCP tool definitions & handlers
├── tui/                 # Terminal UI (feature-gated)
│   ├── mod.rs           # TUI entry
│   ├── app.rs           # Application logic
│   └── theme.rs         # Color theme
└── project/             # Project management
    ├── mod.rs           # Project lifecycle
    └── config.rs        # codeweb.toml parsing
```

## Development

```bash
cargo build                  # build
cargo build --features serve # build with HTTP server + browser UI
cargo build --features mcp   # build with MCP server
cargo test                   # run all tests
cargo test --features serve  # run all tests including serve integration tests
cargo test --features mcp    # run all tests including MCP integration tests
cargo clippy -- -D warnings  # lint (CI-gating level)
cargo clippy --features serve -- -D warnings
cargo fmt -- --check         # format check
```

## Documentation

- [User Guide](docs/user-guide.md) — Complete user manual (Chinese)
- [Roadmap](docs/plans/roadmap.md) — Implementation roadmap (Chinese)
- [CGEF User Guide](docs/cgef-user-guide.md) — Graph import/merge format (Chinese)
- [OP to CGEF Guide](docs/op2cgef-guide.md) — Enterprise Excel lineage to CGEF (Chinese)
- [Serve API Guide](docs/serve-api-guide.md) — HTTP API reference (Chinese)
- [Developer Guide](docs/DeveloperGuide.md) — Architecture & extension guide for MCP/API developers (Chinese)
- [Contribution Guide](CONTRIBUTION.md) — Contribution guidelines for collaborators (Chinese)
- [CGEF JSON Schema](docs/cgef-schema.json) — JSON Schema for CGEF validation

## License

This project is licensed under the terms specified in the [LICENSE](LICENSE) file.

---

<a id="中文"></a>

## 概述

codeweb 分析源代码中的语义关系，构建可遍历的调用/引用有向图。从 SQL 存储过程调用关系起步（openGauss / GaussDB 方言），扩展到 MyBatis/iBatis XML Mapper，再桥接到 Java 方法调用 — 形成完整的 `Java 方法 → Mapper → SQL → 存储过程` 调用链。

```
Java方法 ──InvokesMapper──▶ MappedStatement ──CallsProcedure──▶ 存储过程 ──DirectCall──▶ 存储过程
                                                                   │
                                                                   └──TableAccess──▶ 表
```

## 功能特性

- **SQL 存储过程调用图** — 解析 SQL 文件，提取存储过程、函数、包之间的 `CALL` / `EXECUTE` 调用关系
- **MyBatis/iBatis XML Mapper 链路** — 关联 `MappedStatement → SQL → 存储过程`
- **Java 方法调用 + 桥接** — 解析 Java 源码，提取方法调用，桥接 `Java 方法 → Mapper → SQL → 存储过程`
- **双向查询引擎** — 支持 `callers()`、`callees()`、`trace()`（双向）、`impact()`（变更影响分析）
- **增量分析** — 仅重新解析变更的文件，快速迭代
- **多种导出格式** — DOT（Graphviz）、JSON、Mermaid
- **CGEF 导入/合并** — 通过 [CGEF](docs/cgef-user-guide.md) 格式导入外部图谱数据并与本地分析结果合并；支持自定义节点/边类型
- **交互式 TUI** — 终端图形浏览器，支持键盘导航、搜索和节点详情
- **HTTP 服务器 + 浏览器 UI** — 基于 Cytoscape.js 的交互式可视化，提供 RESTful API（`serve` feature gate）
- **声明式查询 API** — JSON QuerySpec，支持多步骤遍历、过滤、路径收集和子图提取
- **SQL 片段搜索** — 按 SQL 内容搜索 MappedStatement / JavaSql 节点，并追溯到 Java 调用方
- **国际化** — 中英文界面（通过 rust-i18n）

## 技术栈

| 组件 | 技术 |
|------|------|
| 语言 | Rust（最新稳定版） |
| SQL 解析 | [ogsql-parser](https://github.com/c2j/ogsql-parser)（手写递归下降解析器，支持 openGauss/GaussDB） |
| 图引擎 | [petgraph](https://crates.io/crates/petgraph)（内存有向图） |
| Java 解析 | [tree-sitter-java](https://crates.io/crates/tree-sitter-java) |
| XML 解析 | 通过 ogsql-parser `ibatis` feature |
| CLI | [clap](https://crates.io/crates/clap) |
| TUI | [ratatui](https://crates.io/crates/ratatui) + [crossterm](https://crates.io/crates/crossterm) |
| HTTP 服务器 | [axum](https://crates.io/crates/axum) + [tokio](https://crates.io/crates/tokio)（feature gate） |
| 浏览器 UI | Cytoscape.js + dagre 布局（通过 [rust-embed](https://crates.io/crates/rust-embed) 嵌入） |
| 国际化 | [rust-i18n](https://crates.io/crates/rust-i18n)（en, zh-CN） |
| 进度条 | [indicatif](https://crates.io/crates/indicatif) |
| 哈希 | [blake3](https://crates.io/crates/blake3)（文件指纹） |

## 安装

### 从源码构建

```bash
git clone https://github.com/c2j/cobweb.git
cd cobweb

# 构建 CLI + TUI（默认 features）
cargo build

# 构建 HTTP 服务器 + 浏览器 UI
cargo build --features serve

# 构建全部功能
cargo build --features full
```

### Feature Flags

| Feature | 说明 | 默认启用 |
|---------|------|---------|
| `cli` | 命令行界面（clap） | ✅ |
| `tui` | 交互式终端 UI | ✅ |
| `serve` | HTTP 服务器 + 浏览器 UI | ❌ |
| `mcp` | MCP 服务器，用于 LLM 集成（Claude Desktop、Cursor 等） | ❌ |
| `jsp` | JSP 内嵌 SQL 抽取（scriptlet JDBC、declaration） | ❌ |
| `full` | 全部功能（cli + tui + serve + mcp + jsp + search-sql-v2） | ❌ |
| `search-sql-v2` | 增强 SQL 搜索（基于指纹索引） | ❌ |

## 快速开始

```bash
# 初始化新项目并分析
codeweb init my-project -d ./src/main/java -d ./src/main/resources/mapper -d ./sql

# 或分析已有项目
cd my-project
codeweb analyze

# 查看项目统计
codeweb stats

# 列出已分析文件
codeweb files

# 显示自上次分析以来的变更
codeweb diff

# 从节点追踪调用链
codeweb trace --from "process_order"

# 查看节点详情（含上游/下游）
codeweb detail "calculate_total"

# 按 SQL 片段搜索并追踪到 Java 调用方
codeweb trace-sql "SELECT * FROM orders WHERE"

# 列出节点（支持过滤）
codeweb nodes -s order -t proc
codeweb nodes --orphan
codeweb nodes --has-partition

# 去重图谱节点和边
codeweb dedup --dry-run
codeweb dedup

# 执行声明式 JSON QuerySpec
codeweb query --spec '{"start":{"type":"proc","name":"order"},"collect":"nodes"}'

# 导出图谱
codeweb export --format dot --output graph.dot
codeweb export --format mermaid --output graph.mmd
codeweb export --format json --output graph.json

# 交互式 TUI
codeweb tui

# HTTP 服务器 + 浏览器 UI
codeweb serve --addr 127.0.0.1:3000 --open

# 导入外部 CGEF 图谱并合并
codeweb import --file enterprise-graph.json --output erp-store.bincode
codeweb merge -o full-graph.bincode my-project.bincode erp-store.bincode
```

## CLI 命令参考

| 命令 | 说明 |
|------|------|
| `codeweb init <name> -d <dirs>` | 初始化并分析新项目 |
| `codeweb analyze` | 分析项目（全量或增量） |
| `codeweb diff` | 显示自上次分析以来的变更 |
| `codeweb export` | 导出图谱为 DOT/JSON/Mermaid |
| `codeweb trace --from <node>` | 从节点追踪完整调用链 |
| `codeweb detail <node>` | 查看节点的调用方/被调用方详情 |
| `codeweb stats` | 查看项目统计 |
| `codeweb files` | 列出已分析文件及节点数 |
| `codeweb nodes` | 列出图节点（支持过滤） |
| `codeweb trace-sql <sql>` | 按 SQL 片段搜索并追踪到 Java 方法 |
| `codeweb query` | 执行声明式 JSON QuerySpec |
| `codeweb import` | 导入 CGEF JSON 图谱文件 |
| `codeweb merge` | 合并多个图谱存储 |
| `codeweb dedup` | 去重图谱节点和边 |
| `codeweb tui` | 打开交互式 TUI |
| `codeweb serve` | 启动 HTTP 服务器 + 浏览器 UI |
| `codeweb mcp` | 启动 MCP 服务器（stdio JSON-RPC，供 LLM 客户端调用） |

## 节点类型

| 类型 | 标签 | 说明 |
|------|------|------|
| 存储过程 | `proc` | Stored Procedure |
| 函数 | `func` | Function |
| 表 | `table` | Database Table |
| 视图 | `view` | View |
| 映射语句 | `mapper` | MyBatis/iBatis MappedStatement |
| Java 方法 | `method` | Java Method |
| Java 类 | `class` | Java Class |
| Java 内嵌 SQL | `sql` | Java 中的 SQL（注解、JDBC） |
| JSP 页面 | `jsp` | JSP 页面（需启用 `jsp` feature） |
| JSP 内嵌 SQL | `jsql` | JSP 中的 SQL（需启用 `jsp` feature） |
| 包 | `pkg` | Database Package |
| 触发器 | `trigger` | Database Trigger |
| 自定义类型 | `type` | Custom Type |
| 序列 | `seq` | Sequence |
| 索引 | `index` | Index |
| 物化视图 | `mview` | Materialized View |
| 同义词 | `synonym` | Synonym |
| 事件 | `event` | Event |
| 未解析引用 | `unres` | Unresolved Reference |

## HTTP API（serve 模式）

使用 `--features serve` 构建时，codeweb 提供 RESTful API：

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/api/v1/stats` | 项目统计信息 |
| GET | `/api/v1/nodes` | 节点列表（搜索、过滤、分页） |
| GET | `/api/v1/nodes/:id` | 节点详情（属性、上游、下游） |
| GET | `/api/v1/nodes/:id/callers` | 上游调用方 |
| GET | `/api/v1/nodes/:id/callees` | 下游被调用方 |
| GET | `/api/v1/nodes/search-sql` | 按 SQL 文本搜索节点 |
| GET | `/api/v1/trace` | 双向调用链追踪 |
| POST | `/api/v1/query` | 执行声明式 QuerySpec |
| GET | `/api/v1/export` | 导出图谱（DOT/JSON/Mermaid） |

完整 API 文档见 [docs/serve-api-guide.md](docs/serve-api-guide.md)。

## MCP 集成（mcp 模式）

使用 `--features mcp` 构建时，codeweb 可作为 MCP（Model Context Protocol）服务器运行，允许 LLM 客户端（Claude Desktop、Cursor、VS Code Copilot Chat 等）通过 stdio JSON-RPC 直接查询代码图谱。

### 快速开始

```bash
# 构建 MCP 支持
cargo build --features mcp

# 运行 MCP 服务器（stdio JSON-RPC）
codeweb mcp --project /path/to/your/project
```

### Claude Desktop 配置

在 `claude_desktop_config.json` 中添加：

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

### MCP 工具

| 工具 | 说明 |
|------|------|
| `codeweb_stats` | 项目统计（各类型节点/边/文件数量） |
| `codeweb_nodes` | 节点列表（搜索、类型过滤、分页） |
| `codeweb_node_detail` | 节点详情：属性 + 上游调用方 + 下游被调用方 |
| `codeweb_trace` | 从节点名双向追踪调用链 |
| `codeweb_search_sql` | 按 SQL 文本搜索节点（含相关性评分） |
| `codeweb_query` | 执行声明式 JSON QuerySpec，支持复杂多步遍历 |

## 项目结构

```
src/
├── main.rs              # CLI 入口
├── error.rs             # 错误类型（thiserror）
├── parse_log.rs         # 解析警告/错误日志
├── parser/              # SQL/Java/XML 解析层
│   ├── extractor.rs     # 调用关系提取器
│   ├── loader.rs        # 文件加载
│   ├── scanner.rs       # 源文件扫描器
│   ├── ibatis_loader.rs # MyBatis XML Mapper 加载
│   ├── java_loader.rs   # Java 源码加载
│   ├── java_method.rs   # Java 方法提取（tree-sitter）
│   └── fingerprint.rs   # 文件指纹（增量分析）
├── graph/               # 图模型层
│   ├── store.rs         # GraphStore（序列化、合并、去重、索引）
│   ├── builder.rs       # 图构建
│   ├── traverse.rs      # 遍历 & 链路格式化
│   ├── resolver.rs      # 节点解析
│   ├── key.rs           # 节点标识类型
│   ├── search/          # 模糊搜索 & SQL 搜索
│   └── query/           # 声明式查询引擎（QuerySpec）
├── export/              # 导出层
│   ├── dot.rs           # Graphviz DOT 格式
│   ├── json.rs          # JSON 序列化
│   └── mermaid.rs       # Mermaid 流程图
├── import/              # CGEF 导入层
│   ├── format.rs        # CGEF 文档模型
│   ├── parser.rs        # CGEF 解析器
│   ├── validator.rs     # 校验
│   ├── schema.rs        # 自定义类型注册表
│   └── path_mapper.rs   # 路径前缀映射
├── server/              # HTTP 服务器（feature gate）
│   ├── mod.rs           # 服务入口和启动
│   ├── handlers.rs      # API 路由处理
│   ├── state.rs         # 应用状态
│   ├── assets.rs        # 嵌入式浏览器 UI
│   └── access_log.rs    # HTTP 访问日志
├── mcp/                 # MCP 服务器，LLM 集成（feature gate）
│   ├── mod.rs           # 模块声明
│   ├── server.rs        # 服务入口和启动
│   └── tools.rs         # MCP 工具定义和处理
├── tui/                 # 终端 UI（feature gate）
│   ├── mod.rs           # TUI 入口
│   ├── app.rs           # 应用逻辑
│   └── theme.rs         # 颜色主题
└── project/             # 项目管理
    ├── mod.rs           # 项目生命周期
    └── config.rs        # codeweb.toml 解析
```

## 开发

```bash
cargo build                  # 构建
cargo build --features serve # 构建 HTTP 服务器 + 浏览器 UI
cargo build --features mcp   # 构建 MCP 服务器
cargo test                   # 运行全部测试
cargo test --features serve  # 运行包含 serve 的全部测试
cargo test --features mcp    # 运行包含 MCP 的全部测试
cargo clippy -- -D warnings  # 代码检查（CI 级别）
cargo clippy --features serve -- -D warnings
cargo fmt -- --check         # 格式检查
```

## 文档

- [用户指南](docs/user-guide.md) — 完整的用户使用手册
- [实施路线图](docs/plans/roadmap.md) — 完整的实施计划
- [CGEF 用户指南](docs/cgef-user-guide.md) — 图谱导入/合并格式说明
- [OP 血缘转 CGEF 指南](docs/op2cgef-guide.md) — 企业 Excel 血缘数据转 CGEF 格式
- [Serve API 指南](docs/serve-api-guide.md) — HTTP API 参考
- [开发指南](docs/DeveloperGuide.md) — 面向 MCP/API 等开发者的架构与扩展指南
- [贡献指南](CONTRIBUTION.md) — 面向合作开发者的贡献规范
- [CGEF JSON Schema](docs/cgef-schema.json) — CGEF 格式校验 Schema

## 许可证

本项目采用 [LICENSE](LICENSE) 文件中指定的许可条款。
