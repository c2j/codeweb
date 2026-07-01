# codeweb 用户手册

> **语义代码图谱分析器** — 为 SQL 存储过程、Java 方法、MyBatis Mapper 构建可遍历的调用/引用关系图。

---

## 目录

- [1. 什么是 codeweb](#1-什么是-codeweb)
- [2. 安装](#2-安装)
- [3. 核心概念](#3-核心概念)
- [4. 快速上手](#4-快速上手)
- [5. 项目配置](#5-项目配置)
- [6. CLI 命令详解](#6-cli-命令详解)
  - [6.1 初始化项目：`init`](#61-初始化项目init)
  - [6.2 分析项目：`analyze`](#62-分析项目analyze)
  - [6.3 查看变更：`diff`](#63-查看变更diff)
  - [6.4 项目统计：`stats`](#64-项目统计stats)
  - [6.5 文件列表：`files`](#65-文件列表files)
  - [6.6 调用链追踪：`trace`](#66-调用链追踪trace)
  - [6.7 节点详情：`detail`](#67-节点详情detail)
  - [6.8 节点列表：`nodes`](#68-节点列表nodes)
  - [6.9 SQL 搜索与追踪：`trace-sql`](#69-sql-搜索与追踪trace-sql)
  - [6.10 影响分析：`impact`](#610-影响分析impact)
  - [6.11 图谱导出：`export`](#611-图谱导出export)
  - [6.12 声明式查询：`query`](#612-声明式查询query)
  - [6.13 图谱去重：`dedup`](#613-图谱去重dedup)
  - [6.14 系统分解：`partition`](#614-系统分解partition)
  - [6.15 CGEF 导入：`import`](#615-cgef-导入import)
  - [6.16 多项目合并：`merge`](#616-多项目合并merge)
  - [6.17 交互式终端：`tui`](#617-交互式终端tui)
  - [6.18 HTTP 服务：`serve`](#618-http-服务serve)
  - [6.19 MCP 服务：`mcp`](#619-mcp-服务mcp)
- [7. 典型使用场景](#7-典型使用场景)
- [8. 常见问题](#8-常见问题)
- [附录：节点类型与边类型](#附录节点类型与边类型)

---

## 1. 什么是 codeweb

codeweb 是一款**语义代码图谱分析工具**。它能自动解析你的代码仓库（SQL、Java、XML Mapper 等），提取其中的调用关系、引用关系，构建成一个**有向图**，让你可以：

- 🔍 **追踪调用链**：某个存储过程被谁调用？它又调用了谁？一键追溯完整链路。
- 📊 **影响分析**：修改一个 Java 方法或存储过程，会影响哪些上下游？
- 🗂️ **系统分解**：自动将大型系统按调用内聚度聚合成模块（社区发现算法）。
- 🔗 **跨层桥接**：打通 Java → MyBatis Mapper → SQL → 存储过程，形成端到端的调用链。
- 📤 **导出可视化**：导出为 DOT（Graphviz）、Mermaid、JSON 格式，在文档或可视化工具中使用。
- 🌐 **浏览器交互**：启动内置 HTTP 服务，在浏览器中以 Cytoscape.js 图可视化方式探索图谱。

### 适用对象

- **后端/数据库开发者**：理解复杂 PL/SQL 存储过程间的调用关系。
- **Java 企业应用维护者**：理清 MyBatis Mapper 与存储过程的映射。
- **架构师/技术负责人**：评估变更影响范围、规划系统拆分。
- **AI/LLM 使用者**：通过 MCP 协议让 LLM 直接查询代码图谱。

---

## 2. 安装

### 2.1 从源码构建（推荐）

需要安装 [Rust](https://www.rust-lang.org/) 工具链（1.70+）。

```bash
# 克隆仓库
git clone https://github.com/c2j/codeweb.git
cd codeweb

# 构建默认功能（CLI + 终端 TUI）
cargo build --release

# 将编译产物安装到系统路径（可选）
cargo install --path .
```

编译产物位于 `target/release/codeweb`，可将其复制到 `PATH` 中的目录。

### 2.2 启用可选功能

```bash
# 构建并安装 HTTP 服务器 + 浏览器 UI（serve 模式）
cargo install --path . --features serve

# 构建并安装 MCP 服务器（用于 LLM 集成）
cargo install --path . --features mcp

# 构建并安装 JSP 内嵌 SQL 提取
cargo install --path . --features jsp

# 构建全部功能
cargo install --path . --features full
```

### 2.3 功能标志说明

| 功能标志 | 说明 | 默认 |
|---------|------|:---:|
| `cli` | 命令行界面（clap） | ✅ |
| `tui` | 交互式终端 UI | ✅ |
| `serve` | HTTP 服务器 + 浏览器图可视化 | ❌ |
| `mcp` | MCP 服务器（LLM 客户端直接查询图谱） | ❌ |
| `jsp` | JSP 文件内嵌 SQL 提取 | ❌ |
| `search-sql-v2` | 增强 SQL 搜索（指纹索引） | ❌ |

---

## 3. 核心概念

### 3.1 项目

codeweb 以「项目」为单位管理分析。每个项目对应一个 `codeweb.toml` 配置文件，记录分析目录、存储路径等信息。分析结果保存在 `.codeweb/store.bincode` 文件中。

### 3.2 图谱（Graph）

分析结果是一个**有向图**，由节点（Node）和边（Edge）组成：

```
Java方法 ──InvokesMapper──▶ MappedStatement ──CallsProcedure──▶ 存储过程 ──DirectCall──▶ 存储过程
                                                                   │
                                                                   └──TableAccess──▶ 表
```

- **节点（Node）**：代表代码中的实体——存储过程、函数、表、视图、Java 方法、MyBatis Mapper 等。
- **边（Edge）**：代表节点之间的关系——调用、引用、表访问、继承等。

### 3.3 节点标识（Node Key）

每个节点有一个唯一标识，格式为 `<类型>:<限定名>`，例如：
- `proc:public.create_order` — 存储过程
- `method:com.example.OrderService.createOrder` — Java 方法
- `mapper:com.example.OrderMapper.insert` — MyBatis MappedStatement
- `table:public.orders` — 数据库表

### 3.4 增量分析

codeweb 采用**基于文件摘要（指纹）的增量分析**。首次运行 `analyze` 时会全量解析所有文件；之后再次运行只会重新解析**变更过**的文件，未变更的文件直接复用上次分析结果。这使得日常迭代非常快速。

---

## 4. 快速上手

### 4.1 初始化项目

```bash
# 假设你的项目结构如下：
# my-app/
#   src/main/java/        — Java 源码
#   src/main/resources/   — MyBatis XML Mapper
#   sql/                  — 数据库存储过程 SQL 脚本

cd my-app
codeweb init my-app -d src/main/java -d src/main/resources/mapper -d sql
```

这会：
1. 创建 `codeweb.toml` 配置文件
2. 扫描指定的目录，解析所有 SQL、Java、XML 文件
3. 构建代码调用图谱，保存到 `.codeweb/store.bincode`

如果项目已有 `codeweb.toml`（比如团队中有人已初始化），直接：

```bash
codeweb analyze
```

### 4.2 日常工作流

```
            ┌─────────────────────────────────────────┐
            │                                         │
            ▼                                         │
    ┌──────────────┐   修改代码    ┌──────────────┐   │
    │ codeweb      │─────────────▶│ codeweb      │───┘
    │ init -d ...  │              │ analyze      │ (增量分析)
    └──────────────┘              └──────┬───────┘
                                         │
                              ┌──────────┼──────────┐
                              ▼          ▼          ▼
                         trace     detail      impact
                        (追踪链)   (节点详情)   (影响分析)
```

### 4.3 基础命令速览

```bash
codeweb stats              # 查看项目统计：多少节点、多少边
codeweb nodes -s order     # 搜索名称包含 "order" 的节点
codeweb trace "create_order"           # 追踪 create_order 的完整调用链
codeweb detail "OrderService"          # 查看节点详情（含上下游）
codeweb impact --node "create_order"   # 影响分析
codeweb export --format dot --output graph.dot   # 导出为 DOT 格式
```

---

## 5. 项目配置

### 5.1 codeweb.toml 结构

```toml
[project]
name = "my-project"          # 项目名称
description = ""              # 项目描述（可选）

[analysis]
paths = [                     # 要分析的源代码目录
    "src/main/java",
    "src/main/resources/mapper",
    "sql"
]
exclude = [                   # 排除的文件模式（glob 格式，可选）
    "**/test/**",
    "**/generated/**"
]

[analysis.java]
# 扩展 SQL 提取：将额外的方法名也视为 SQL 调用
extra_sql_methods = ["doQuery", "runSql"]
# 扩展 SQL 提取：将包含这些关键词的变量名也视为 SQL 变量
extra_sql_var_patterns = ["QUERY", "CMD"]

[store]
path = ".codeweb/store.bincode"   # 图谱存储路径
format = "bincode"                # 存储格式：bincode 或 json
```

### 5.2 配置项详解

#### `[project]`

| 字段 | 类型 | 说明 |
|------|------|------|
| `name` | string | 项目名称 |
| `description` | string | 项目描述（可选） |

#### `[analysis]`

| 字段 | 类型 | 说明 |
|------|------|------|
| `paths` | string[] | 要分析的源代码目录列表 |
| `exclude` | string[] | 排除的文件 glob 模式（可选） |
| `encoding` | map | 文件编码指定（可选，key 为文件后缀如 `.sql`，value 为编码名如 `GBK`） |

#### `[analysis.java]` — Java SQL 提取调优

codeweb 会从 Java 源码中自动识别 SQL——包括 `@Query` 注解、`prepareStatement()`、`createNativeQuery()` 等方法中的 SQL 字符串常量。

如果你的项目使用了自定义的 SQL 执行封装方法（例如 `doQuery(sql, params)`），可以通过以下配置让 codeweb 识别：

```toml
[analysis.java]
extra_sql_methods = ["doQuery", "executeSQL"]
extra_sql_var_patterns = ["QUERY_TEMPLATE", "SQL_CMD"]
```

- `extra_sql_methods`：追加的方法名，第一个字符串参数会被当作 SQL。
- `extra_sql_var_patterns`：追加的变量名关键词（不区分大小写），包含这些关键词的变量赋值会被当作 SQL。

#### `[store]`

| 字段 | 类型 | 说明 |
|------|------|------|
| `path` | string | 图谱存储文件路径，默认 `.codeweb/store.bincode` |
| `format` | string | 存储格式：`bincode`（二进制，快速）或 `json`（可读，较大） |

---

## 6. CLI 命令详解

> 💡 **提示**：所有命令都支持 `--lang zh-CN` 或 `--lang en` 切换界面语言。默认语言为 `zh-CN`。

### 6.1 初始化项目：`init`

创建并分析一个新项目。

```bash
codeweb init <项目名> -d <目录1> [-d <目录2> ...]
```

| 参数 | 说明 |
|------|------|
| `<项目名>` | 项目名称，会写入 `codeweb.toml` |
| `-d, --dir <路径>` | 源代码目录，可多次指定 |

**示例**：

```bash
codeweb init erp-system -d src/main/java -d sql/procedures -d sql/functions
```

执行后会在当前目录创建 `codeweb.toml` 并立即进行首次全量分析。

---

### 6.2 分析项目：`analyze`

分析（或重新分析）当前项目。会**自动检测变更**并执行增量分析——仅重新解析已修改、新增、删除的文件。

```bash
codeweb analyze [-p <项目目录>]
```

| 参数 | 说明 |
|------|------|
| `-p, --project <路径>` | 项目目录，默认当前目录 `.` |

**输出示例**：

```
incremental build: 156 files (140 unchanged, 12 changed, 3 added, 1 deleted) → 423 nodes, 891 edges (2.3s)
  ⚠ 2 warnings, 0 errors — see .codeweb/parse.log
```

- `full build` — 首次分析或项目配置变更时的全量构建。
- `incremental build` — 增量分析，仅处理变更文件。
- 解析警告/错误会记录在 `.codeweb/parse.log` 中。

---

### 6.3 查看变更：`diff`

显示自上次分析以来的文件变更情况，**不会执行分析**。

```bash
codeweb diff [-p <项目目录>]
```

**输出示例**：

```
3 modified, 1 added, 0 deleted, 140 unchanged
  M src/main/java/com/example/OrderService.java
  M src/main/resources/mapper/OrderMapper.xml
  M sql/procedures/create_order.sql
  + sql/procedures/update_order.sql
```

---

### 6.4 项目统计：`stats`

显示项目的统计概览。

```bash
codeweb stats [-p <项目目录>]
```

**输出示例**：

```
Project: erp-system

            42  procedures
            15  functions
             5  packages
             2  triggers
             1  types
             3  sequences
             7  indexes
             8  views
             0  materialized views
             0  synonyms
             0  events
            35  tables
            28  mappers
            85  java methods
            12  java classes
            10  java sql sources
             3  unresolved

           156  edges
            48  files
```

---

### 6.5 文件列表：`files`

列出项目中所有已分析的文件，以及每个文件包含的节点数量。

```bash
codeweb files [-p <项目目录>]
```

**输出示例**：

```
TYPE  NODES  PATH
SQL       5  sql/procedures/order_procedures.sql
Java      3  src/main/java/com/example/OrderService.java
XML       4  src/main/resources/mapper/OrderMapper.xml
Java      2  src/main/java/com/example/UserService.java
...

48 files total
```

---

### 6.6 调用链追踪：`trace`

从指定节点出发，追踪完整的上下游调用链。这是**最常用的命令之一**。

```bash
codeweb trace <节点名称> [-p <项目目录>] [-s <展示风格>] [--builtfunc]
```

| 参数 | 说明 |
|------|------|
| `<节点名称>` | 节点名称（位置参数，子串匹配，不区分大小写） |
| `-p, --project <路径>` | 项目目录，默认 `.` |
| `-s, --style <风格>` | 展示风格：`tree`（树形，默认）或 `path`（路径列表） |
| `--builtfunc` | 显示内建函数调用（默认隐藏） |

**示例**：

```bash
# 追踪 create_order 的调用链
codeweb trace "create_order"

# 以路径列表方式展示
codeweb trace "processPayment" --style path

# 显示内建函数（如 pg_sleep, now() 等）
codeweb trace "calculate_total" --builtfunc
```

**输出示例（tree 风格）**：

```
Tracing from: proc:create_order

── CALLERS ──
  (none)

── TARGET ──
  proc:create_order

── CALLEES ──
  ├── proc:send_notification
  │   └── table:notifications [W:insert]
  ├── proc:validate_order
  │   └── table:users [R]
  └── table:orders [W:insert]
```

**输出示例（path 风格）**：

```
Tracing from: proc:create_order

── CALLERS (0 paths) ──
  (none)

── TARGET ──
  proc:create_order

── CALLEES (3 paths) ──
Path 1      2 hops
→ proc:send_notification
    → table:notifications [W:insert]

Path 2      2 hops
→ proc:validate_order
    → table:users [R]

Path 3      1 hops
→ table:orders [W:insert]
```

---

### 6.7 节点详情：`detail`

查看指定节点的完整详情——包括属性信息、直接上下游、完整调用链。

```bash
codeweb detail <节点名称> [-p <项目目录>] [-s <风格>] [-d <深度>] [--files] [--builtfunc]
```

| 参数 | 说明 |
|------|------|
| `<节点名称>` | 节点名称（子串匹配） |
| `-s, --style <风格>` | `tree`（默认）或 `path` |
| `-d, --depth <深度>` | 遍历深度，1=仅直接上下游，0=无限制（默认 1） |
| `--files` | 同时列出调用链涉及的文件 |
| `--builtfunc` | 显示内建函数调用 |

**输出注意事项**：

- `proc*` / `func*` 标签表示部分解析节点 —— `⚠ partial node` 警告
- `table*` / `view*` 标签表示推测型节点 —— `⚠ inferred node — no DDL definition found` 警告
- 系统对象 —— `⚙ system object — belongs to a known system schema` 提示

**示例**：

```bash
codeweb detail "create_order"
codeweb detail "OrderMapper.insert" --depth 3 --files
```

---

### 6.8 节点列表：`nodes`

列出和筛选图中的节点。

```bash
codeweb nodes [-s <搜索词>] [-t <类型>] [--orphan] [--low-degree <N>]
              [--has-partition] [--has-distribute] [--inferred] [--system]
              [--sort-by <规则>] [-p <项目目录>]
```

| 参数 | 说明 |
|------|------|
| `-s, --search <词>` | 按名称搜索（子串匹配） |
| `-t, --node-type <类型>` | 按类型过滤：`proc`、`func`、`table`、`view`、`mapper`、`method`、`class`、`sql`、`pkg`、`trigger`、`unres` 等 |
| `--orphan` | 只显示孤立节点（没有任何连接） |
| `--low-degree <N>` | 只显示总连接度 ≤ N 的节点 |
| `--has-partition` | 只显示分区表 |
| `--has-distribute` | 只显示分布式表 |
| `--inferred` | 只显示推测型节点（在 DML 中引用但无 DDL 定义的表/视图） |
| `--system` | 只显示系统对象（`pg_catalog`、`sys`、`dbe_*` 等 schema 下的表/视图，或 `dual`/`sys_dummy` 等已知系统名） |
| `--sort-by <规则>` | 排序规则，逗号分隔。键：`name`、`type`、`in`、`out`、`total`。方向：`asc`（默认）/`desc` |

**示例**：

```bash
# 搜索包含 "order" 的存储过程
codeweb nodes -s order -t proc

# 找出所有孤立节点
codeweb nodes --orphan

# 按总调用度降序排列
codeweb nodes --sort-by total:desc

# 多级排序：先按入度降序，再按出度升序
codeweb nodes -t method --sort-by in:desc,out:asc
```

**输出示例**：

```
TYPE     IN OUT TOT  NAME
proc      5   3   8  proc:public.create_order
proc      3   2   5  proc:public.update_order
table*    2   0   2  table:public.legacy_temp
table     0   1   1  table:public.orders

4 nodes
```

> Note: `table*` 和 `view*` 标签表示推测型节点——在 DML 中被引用但源文件中没有对应的 `CREATE TABLE` / `CREATE VIEW` 定义。

---

### 6.9 SQL 搜索与追踪：`trace-sql`

在 MappedStatement 和 Java 内嵌 SQL 中搜索包含指定 SQL 片段的节点，并自动追溯到 Java 调用方。

```bash
codeweb trace-sql <SQL片段> [-p <项目目录>]
codeweb trace-sql -f <SQL文件路径> [-p <项目目录>]
```

| 参数 | 说明 |
|------|------|
| `<SQL片段>` | 要在 SQL 文本中搜索的片段（子串匹配，不区分大小写） |
| `-f, --file <路径>` | 从文件读取 SQL 片段（使用 `-` 从标准输入读取） |

**示例**：

```bash
# 直接搜索
codeweb trace-sql "SELECT * FROM orders WHERE"

# 从文件读取（避免 shell 转义问题）
codeweb trace-sql -f query.sql

# 从管道读取
echo "UPDATE inventory SET" | codeweb trace-sql -f -
```

**输出示例**：

```
SQL fragment: 'SELECT * FROM orders WHERE'
Found 2 matching node(s)

  MappedStatement: com.example.OrderMapper.selectByStatus  [92%]
    kind:  select
    file:  src/main/resources/mapper/OrderMapper.xml:24
    sql:   SELECT * FROM orders WHERE status = __XML_PARAM_status__
    invoked by:
      JavaMethod: com.example.OrderService.findOrdersByStatus
        file:     src/main/java/com/example/OrderService.java:45
```

---

### 6.10 影响分析：`impact`

评估修改某个文件或节点的上下游影响范围。支持 **JSON 输出**，适合集成到 CI/CD 流程。

```bash
# 文件级影响分析——分析某个文件中所有节点的上下游
codeweb impact --file <文件路径> [--format json|text] [-d <深度>] [-p <项目目录>]

# 节点级影响分析——分析单个节点的上下游
codeweb impact --node <节点名称> [--format json|text] [-d <深度>] [-p <项目目录>]
```

| 参数 | 说明 |
|------|------|
| `--file <路径>` | 文件路径（与 `--node` 互斥） |
| `--node <名称>` | 节点名称（与 `--file` 互斥） |
| `-f, --format <格式>` | 输出格式：`json`（默认，适合集成）或 `text`（适合阅读） |
| `-d, --depth <深度>` | 遍历深度，1=直接上下游（默认） |

**JSON 输出示例**：

```json
{
  "schema_version": 2,
  "node": "proc:create_order",
  "upstream": [
    {
      "file_path": "src/main/java/com/example/OrderService.java",
      "symbol": "method:com.example.OrderService.createOrder",
      "line": 42
    }
  ],
  "downstream": [
    {
      "file_path": "sql/procedures/order_procedures.sql",
      "symbol": "proc:public.validate_order",
      "line": 120
    },
    {
      "file_path": "sql/procedures/order_procedures.sql",
      "symbol": "table:public.orders",
      "line": 135
    }
  ]
}
```

**集成场景**：

```bash
# 在 CI 中，获取 git diff 涉及文件的变更影响
git diff --name-only HEAD~1 | while read file; do
  codeweb impact --file "$file" --format json
done
```

---

### 6.11 图谱导出：`export`

将代码图谱导出为不同格式，用于可视化或集成。

```bash
codeweb export [--format <格式>] [-o <输出文件>] [-p <项目目录>]
```

| 参数 | 说明 |
|------|------|
| `-f, --format <格式>` | 导出格式：`dot`（默认）、`json`、`mermaid` |
| `-o, --output <文件>` | 输出文件路径（不指定则输出到标准输出） |
| `-p, --project <路径>` | 项目目录，默认 `.` |

**导出格式说明**：

| 格式 | 说明 | 用途 |
|------|------|------|
| `dot` | Graphviz DOT 语言 | 用 `dot -Tpng graph.dot -o graph.png` 渲染为图片 |
| `mermaid` | Mermaid 流程图语法 | 嵌入 Markdown 文档直接渲染 |
| `json` | 完整图谱 JSON 序列化 | 程序化处理、数据交换 |

**示例**：

```bash
# 导出 DOT 并渲染为 PNG
codeweb export --format dot --output graph.dot
dot -Tpng graph.dot -o graph.png

# 导出 Mermaid 嵌入文档
codeweb export --format mermaid --output graph.mmd

# 导出 JSON
codeweb export --format json --output graph.json
```

---

### 6.12 声明式查询：`query`

通过 JSON QuerySpec 执行复杂的多步图遍历查询。这是最灵活的查询接口。

```bash
codeweb query --spec '<JSON>' [-p <项目目录>]
codeweb query -f <查询文件.json> [-p <项目目录>]
```

| 参数 | 说明 |
|------|------|
| `-s, --spec <JSON>` | 内联 QuerySpec JSON 字符串 |
| `-f, --file <路径>` | QuerySpec JSON 文件路径（`-` 从标准输入读取） |

#### QuerySpec 结构

```json
{
  "start": { "type": "proc", "name": "order" },
  "steps": [
    { "action": "outgoing", "edge_categories": ["call"], "max_depth": 3 }
  ],
  "collect": "nodes"
}
```

**start（起始节点）**：
| 字段 | 说明 |
|------|------|
| `type` | 节点类型标签（可选） |
| `name` | 节点名称，子串匹配（可选） |
| `schema` | Schema 过滤（可选） |

**steps（遍历步骤）**：
| action | 说明 |
|--------|------|
| `outgoing` | 沿出边方向遍历 |
| `incoming` | 沿入边方向遍历 |
| `filter` | 按 `type_tag` 或 `schema` 过滤当前节点集 |
| `until` | 遍历直到遇到指定 `type_tag` 的节点 |

边类别（`edge_categories`）可选值：`call`、`composition`、`dataflow`、`reference`、`inheritance`

**collect（收集模式）**：`nodes`、`paths`、`subgraph`

**示例**：

```bash
# 从 "order" 相关存储过程出发，沿调用边追踪 3 层
codeweb query --spec '{"start":{"type":"proc","name":"order"},"steps":[{"action":"outgoing","edge_categories":["call"],"max_depth":3}],"collect":"nodes"}'

# 反向追踪：谁依赖了 orders 表
codeweb query --spec '{"start":{"type":"table","name":"orders"},"steps":[{"action":"incoming","max_depth":3}],"collect":"nodes"}'

# 从 Java 方法追踪到存储过程的完整路径
codeweb query --spec '{"start":{"type":"method","name":"PaymentService"},"steps":[{"action":"outgoing","max_depth":5}],"collect":"paths"}'
```

---

### 6.13 图谱去重：`dedup`

对图谱中的重复节点和边执行去重，减少冗余。

```bash
codeweb dedup [-p <项目目录>] [-o <输出文件>] [--dry-run]
```

| 参数 | 说明 |
|------|------|
| `--dry-run` | 预览模式——只显示即将去重的内容，不实际修改 |
| `-o, --output <文件>` | 输出去重后的图谱到新文件 |
| `-p, --project <路径>` | 项目目录，默认 `.` |

**示例**：

```bash
# 预览去重效果
codeweb dedup --dry-run

# 执行去重
codeweb dedup
```

---

### 6.14 系统分解：`partition`

通过社区发现算法（Louvain/CNM）将图谱节点自动分组为若干个聚类，用于系统模块化分析。

```bash
codeweb partition [--k <聚类数>] [--gamma <γ>] [--auto] [--max-iterations <N>]
                  [--min-delta-q <ΔQ>] [--table-projection [tau:lambda:k]]
                  [-o <输出文件>] [-p <项目目录>]
```

| 参数 | 说明 |
|------|------|
| `-k, --k <数量>` | 目标聚类数量（不指定则自动发现） |
| `--gamma <γ>` | 分辨率参数 γ，越小聚类越大（默认 1.0） |
| `--auto` | 自动探索最佳聚类数和 γ 值 |
| `--max-iterations <N>` | CNM 迭代上限 |
| `--min-delta-q <ΔQ>` | 最小模块度增量阈值（自然模式） |
| `--min-component-size <N>` | 参与聚类的最小连通分量大小（默认 1） |
| `--table-projection [tau:lambda:k]` | 启用 TF-IDF 表访问投影，桥接共享表的存储过程 |
| `-o, --output <文件>` | 导出聚类后的 DOT 文件（含 `cluster_*` 子图块） |

**示例**：

```bash
# 自动探索最佳聚类
codeweb partition --auto

# 指定聚类数为 5
codeweb partition -k 5

# 启用表访问投影（使用默认参数）
codeweb partition --auto --table-projection

# 自定义表投影参数并导出
codeweb partition --auto --table-projection 0.2:0.4:15 -o clusters.dot
```

---

### 6.15 CGEF 导入：`import`

导入外部 CGEF（Code Graph Exchange Format）格式的图谱数据。

```bash
codeweb import -f <CGEF文件> -o <输出文件> [--prefix <路径前缀>] [--name <项目名>] [--force]
```

| 参数 | 说明 |
|------|------|
| `-f, --file <路径>` | CGEF JSON 文件路径 |
| `-o, --output <路径>` | 输出的 GraphStore 文件（`.bincode` 或 `.json`） |
| `--prefix <前缀>` | 为所有文件路径添加前缀 |
| `--name <名称>` | 导入后的项目名称 |
| `--force` | 即使有校验错误也强制导入 |

**示例**：

```bash
codeweb import -f enterprise-graph.json -o erp-store.bincode --name "erp-enterprise"
```

详见 [CGEF 用户指南](cgef-user-guide.md)。

---

### 6.16 多项目合并：`merge`

将多个项目的 GraphStore 合并为一个。

```bash
codeweb merge <存储文件1> <存储文件2> ... -o <输出文件> [--name <项目名>]
```

| 参数 | 说明 |
|------|------|
| `<存储文件...>` | 要合并的 GraphStore 文件（`.bincode` 或 `.json`） |
| `-o, --output <文件>` | 合并后的输出文件 |
| `--name <名称>` | 合并后的项目名称（默认 `merged`） |

**示例**：

```bash
codeweb merge module-a.bincode module-b.bincode module-c.bincode -o full-graph.bincode --name "monolith"
```

---

### 6.17 交互式终端：`tui`

启动终端交互式图谱浏览器。

> **前提**：使用默认 features（包含 `tui`）构建。

```bash
codeweb tui [-p <项目目录>]
```

TUI 支持键盘导航、节点搜索、节点详情查看等功能。使用 `q` 退出，`↑↓` 导航，`/` 搜索。

---

### 6.18 HTTP 服务：`serve`

启动本地 HTTP 服务器，提供 RESTful API 和基于 Cytoscape.js 的浏览器图可视化界面。

> **前提**：使用 `--features serve` 构建。

```bash
codeweb serve [-p <项目目录>] [-a <监听地址>] [--open]
```

| 参数 | 说明 |
|------|------|
| `-p, --project <路径>` | 项目目录，默认 `.` |
| `-a, --addr <地址>` | 监听地址，默认 `127.0.0.1:3000` |
| `--open` | 启动后自动打开浏览器 |

**示例**：

```bash
# 本地访问
codeweb serve --addr 127.0.0.1:3000 --open

# 局域网内共享
codeweb serve --addr 0.0.0.0:8080
```

启动后访问 `http://127.0.0.1:3000` 即可使用浏览器 UI。API 详细文档见 [Serve API 指南](serve-api-guide.md)。

---

### 6.19 MCP 服务：`mcp`

以 MCP（Model Context Protocol）服务器模式运行，允许 LLM 客户端（Claude Desktop、Cursor、VS Code Copilot Chat 等）直接通过 stdio JSON-RPC 查询代码图谱。

> **前提**：使用 `--features mcp` 构建。

```bash
codeweb mcp [-p <项目目录>]
```

**Claude Desktop 配置**（`claude_desktop_config.json`）：

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

配置后，Claude 可以直接调用以下 MCP 工具：

| 工具 | 说明 |
|------|------|
| `codeweb_stats` | 项目统计 |
| `codeweb_nodes` | 节点列表（搜索、过滤、分页） |
| `codeweb_node_detail` | 节点详情（属性 + 上下游） |
| `codeweb_trace` | 双向调用链追踪 |
| `codeweb_search_sql` | 按 SQL 文本搜索 |
| `codeweb_query` | 声明式 QuerySpec 查询 |

---

## 7. 典型使用场景

### 场景 1：新人接手项目，理解调用关系

```bash
# 1. 初始化分析
codeweb init my-project -d src -d sql

# 2. 查看全局概览
codeweb stats

# 3. 搜索感兴趣的存储过程
codeweb nodes -s "order" -t proc

# 4. 追踪完整调用链
codeweb trace "create_order"

# 5. 查看节点详情（含文件信息）
codeweb detail "create_order" --depth 3 --files
```

### 场景 2：修改代码前的影响评估

```bash
# 方法 1：文件级影响分析
codeweb impact --file "sql/procedures/order_procedures.sql" --format text

# 方法 2：节点级影响分析
codeweb impact --node "create_order" --depth 3

# 方法 3：JSON 输出，集成到脚本
codeweb impact --node "create_order" --format json | jq .
```

### 场景 3：从 SQL 反查 Java 调用方

有时你只知道数据库中的某条 SQL，想找到是哪个 Java 方法调用了它：

```bash
codeweb trace-sql "SELECT * FROM orders WHERE order_id"
```

### 场景 4：系统模块化拆分规划

```bash
# 自动发现模块边界
codeweb partition --auto

# 指定目标模块数
codeweb partition -k 8 -o modules.dot

# 导出 DOT 并渲染为图片
dot -Tpng modules.dot -o modules.png
```

### 场景 5：导出图谱供文档使用

```bash
# 导出 Mermaid 格式嵌入 Markdown
codeweb export --format mermaid --output architecture.mmd

# 导出 DOT 并渲染高质量图片
codeweb export --format dot --output graph.dot
dot -Tsvg graph.dot -o architecture.svg
```

### 场景 6：在 CI/CD 中集成变更影响分析

```bash
#!/bin/bash
# .github/scripts/impact-check.sh

# 获取变更的文件
CHANGED_FILES=$(git diff --name-only origin/main...HEAD)

for file in $CHANGED_FILES; do
  echo "=== Impact for: $file ==="
  codeweb impact --file "$file" --format json
  echo
done
```

### 场景 7：通过 MCP 让 AI 助手理解代码

在 Claude Desktop 中配置后，你可以直接问：

> "帮我看看 create_order 这个存储过程被哪些 Java 方法调用"

Claude 会自动调用 `codeweb_trace` 工具获取调用链并分析。

---

## 8. 常见问题

### Q: 分析速度慢怎么办？

1. 确保使用 `--release` 构建：`cargo build --release`
2. 启用并行：codeweb 自动利用多核 CPU
3. 使用排除规则排除测试文件和生成代码：

```toml
[analysis]
exclude = ["**/test/**", "**/target/**", "**/generated/**"]
```

4. 日常开发中利用增量分析——只执行 `codeweb analyze`，不要每次都用 `init`。

### Q: 为什么某些存储过程显示为 `proc*`（partial）？

表示该存储过程的 body 代码解析失败，可能是使用了 codeweb 不支持的语法。请检查 `.codeweb/parse.log` 查看具体错误。

### Q: 为什么某些节点显示为 `unres`（未解析）？

某个地方引用了该节点（例如 `CALL unresolved_proc()`），但在被分析的代码中没有找到其定义。可能原因：
- 该存储过程定义在其他未分析的目录中
- 名称大小写/引号导致的匹配失败

### Q: 可以分析哪些类型的项目？

codeweb 专为以下技术栈优化：
- **数据库**：openGauss / GaussDB 方言的存储过程、函数、包、触发器
- **Java**：通过 tree-sitter 解析方法调用关系
- **MyBatis/iBatis**：XML Mapper 中的 SQL 语句
- **JSP**（需要 `jsp` feature）：JSP 文件中内嵌的 SQL

对于其他 SQL 方言（MySQL、PostgreSQL 标准语法）可能部分兼容，但完整支持仅限 openGauss/GaussDB 方言。

### Q: 图谱存储文件很大怎么办？

- 默认使用 `bincode` 格式，已做二进制压缩
- 可定期执行 `codeweb dedup` 去除重复节点和边
- 如果需要可读格式，使用 `format = "json"`（文件会显著增大）

### Q: 支持哪些操作系统？

Linux、macOS、Windows 均支持。

### Q: 如何切换界面语言？

```bash
codeweb --lang en stats      # 英文
codeweb --lang zh-CN stats   # 中文（默认）
```

或在 `codeweb.toml` 中不支持配置语言，通过命令行 `--lang` 参数指定。

### Q: 可以集成到 VS Code 吗？

可以。通过 MCP 模式运行 codeweb，然后在 VS Code 的 Copilot Chat 或 Continue 中配置 MCP 连接即可。

### Q: 分析结果保存在哪里？

默认保存在 `.codeweb/` 目录下：
- `store.bincode` — 图谱数据
- `parse.log` — 解析日志（警告和错误）
- `fingerprint.bin` — 文件指纹缓存（用于增量分析）

---

## 附录：节点类型与边类型

### 节点类型

| 类型 | 标签 | 说明 |
|------|------|------|
| 存储过程 | `proc` | Stored Procedure |
| 存储过程（partial） | `proc*` | 存储过程 body 未完整解析 |
| 函数 | `func` | Function |
| 函数（partial） | `func*` | 函数 body 未完整解析 |
| 表 | `table` | Database Table（有 DDL 定义） |
| 表（推测型） | `table*` | 在 DML 中引用，无对应 DDL |
| 视图 | `view` | View（有 DDL 定义） |
| 视图（推测型） | `view*` | 在 DML 中引用，无对应 DDL |
| 物化视图 | `mview` | Materialized View |
| 包 | `pkg` | Database Package |
| 触发器 | `trigger` | Database Trigger |
| 自定义类型 | `type` | Custom Type |
| 序列 | `seq` | Sequence |
| 索引 | `index` | Index |
| 同义词 | `synonym` | Synonym |
| 事件 | `event` | Event |
| 映射语句 | `mapper` | MyBatis/iBatis MappedStatement |
| Java 方法 | `method` | Java Method |
| Java 类 | `class` | Java Class |
| Java 内嵌 SQL | `sql` | Java 中的 SQL（注解、JDBC） |
| JSP 页面 | `jsp` | JSP 页面（需 `jsp` feature） |
| JSP 内嵌 SQL | `jspsql` | JSP 中的 SQL（需 `jsp` feature） |
| 内建函数 | `builtin` | Built-in Function |
| 未解析引用 | `unres` | Unresolved Reference |
| 自定义节点 | *自定义* | 通过 CGEF 导入的自定义节点类型 |

### 边类型

| 边类型 | 说明 |
|--------|------|
| `DirectCall` | **同包内**存储过程之间的直接调用 |
| `CrossCall` | **跨包**存储过程之间的调用 |
| `DynamicCall` | 动态 SQL 调用（通过变量间接调用） |
| `CallsProcedure` | XML Mapper 中的 SQL 调用存储过程 |
| `InvokesMapper` | Java 方法调用 MyBatis MappedStatement |
| `CallsJava` | Java 方法之间的调用 |
| `TableAccess` | 存储过程或 SQL 访问表 |
| `UsesBuiltinFunction` | 调用数据库内建函数 |
| `ContainsMethod` | Java 类包含方法 |
| `ContainsRoutine` | 包包含存储过程/函数 |
| `ContainsSql` | JSP 页面包含 SQL（需 `jsp` feature） |
| `Extends` | Java 类继承 |
| `Implements` | Java 类实现接口 |
| `DependsOn` | 其他依赖关系 |
| `TriggersRoutine` | 触发器触发存储过程 |
| `ReferencesType` | 引用自定义类型 |
| `UsesSequence` | 使用序列 |
| `IndexesTable` | 索引关联表 |
| `AliasesObject` | 同义词指向对象 |
| `CustomEdge` | 通过 CGEF 导入的自定义边类型 |

---

> **更多文档**：
> - [实施路线图](plans/roadmap.md) — 功能规划和进度
> - [Serve API 指南](serve-api-guide.md) — HTTP API 详细参考
> - [CGEF 用户指南](cgef-user-guide.md) — 图谱导入/合并格式说明
> - [开发指南](DeveloperGuide.md) — 面向开发者的架构与扩展指南
