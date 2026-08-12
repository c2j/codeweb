# codeweb vs flowScope 对比报告

> **日期**: 2026-08-12 | **测试环境**: macOS Apple Silicon, Rust stable, `--release`

---

## 1. 执行摘要

**codeweb** 和 **flowScope** 虽然都在"SQL 分析 + 图谱可视化"领域，但核心定位截然不同：

| | codeweb | flowScope |
|---|---|---|
| **核心定位** | 跨语言代码调用图（存储过程调用链） | 纯 SQL 数据血缘（列级数据流） |
| **一句话** | "谁调用了哪个存储过程？Java 方法经由哪个 Mapper 最终到达哪个 Procedure？" | "这个 SELECT 的数据从哪些表的哪些列来，经过什么变换？" |
| **目标用户** | 遗留系统维护者、存储过程重构者 | 数据工程师、数据分析师 |
| **核心差异** | 支持 PL/pgSQL 存储过程 body 解析 + Java/MyBatis 桥接 | 支持 14 种 SQL 方言列级血缘 + dbt/Jinja |

**核心结论**: 两者不是竞争关系，而是互补关系。codeweb 擅长"调用链"维度（过程间关系），flowScope 擅长"数据流"维度（列间关系）。实际企业场景可能需要两者结合。

---

## 2. 工具概述

### codeweb

- **版本**: v0.8.10
- **语言**: Rust
- **仓库**: https://github.com/c2j/cobweb
- **核心能力**: 解析 openGauss/GaussDB SQL 存储过程、MyBatis XML mapper、Java 源码，构建 `Java → Mapper → SQL → Stored Procedure` 调用链
- **SQL 解析引擎**: [ogsql-parser](https://github.com/c2j/ogsql-parser) v0.8.33 — 手写递归下降解析器，1980+ 单元测试，1409/1409 openGauss 回归测试通过

### flowScope

- **版本**: v0.8.0
- **语言**: Rust + WASM
- **仓库**: https://github.com/pondpilot/flowscope
- **核心能力**: 解析 SQL 查询语句（14 种方言），提取表级和列级数据血缘关系
- **SQL 解析引擎**: sqlparser-rs — 通用 SQL 解析器

---

## 3. 功能对比

### 3.1 SQL 解析能力

| 维度 | codeweb (ogsql-parser) | flowScope (sqlparser-rs) |
|------|----------------------|--------------------------|
| **SQL 方言** | openGauss/GaussDB（1 种） | 14 种（generic, ansi, bigquery, clickhouse, databricks, duckdb, hive, mssql, mysql, oracle, postgres, redshift, snowflake, sqlite） |
| **解析方式** | 手写递归下降（单方言深度优化） | 通用 parser（多方言广度覆盖） |
| **关键字数量** | 724 | sqlparser-rs 通用关键字集 |
| **AST 类型** | 219+ | sqlparser-rs AST |
| **PL/pgSQL** | ✅ 完整（变量声明/游标/循环/异常处理/动态SQL） | ❌ 不解析存储过程 body |
| **存储过程调用图** | ✅ CALL/EXECUTE 关系 + 嵌套调用链 | ❌ |
| **Package 支持** | ✅ Oracle 兼容 Package | ❌ |
| **SELECT/INSERT/UPDATE/DELETE/MERGE** | ✅ | ✅ |
| **CTE (WITH)** | ✅ 基础支持 | ✅ 完整血缘追踪 |
| **列级血缘** | ❌ | ✅ 核心能力（追踪 `SUM(o.amount)→total`） |
| **表级血缘** | ✅ 部分（TableAccess 边） | ✅ 完整 |
| **dbt/Jinja 模板** | ❌ | ✅ ref(), source(), config(), var() |
| **Schema 感知** | ❌ | ✅ DDL 文件 + 数据库直连 |
| **DDL 支持** | ✅ 50+ CREATE/ALTER/DROP 类型 | ✅ CREATE TABLE/VIEW, DROP |
| **格式器** | ✅ 双阶段：AST 结构化 + Token 级可配置 | ❌ |
| **JSON 往返** | ✅ SQL→JSON→SQL 无损 | ❌ |
| **注释保留** | ✅ 可配置 | ✅ COMMENT 描述提取 |
| **回归测试** | 1409/1409 openGauss 官方 | 基于 sqlparser-rs 测试集 |
| **Lint 规则** | 53 条（反模式检测，4 严重级别） | 72 条（9 类别，含自动修复） |

**结论**: codeweb 在存储过程/PL/pgSQL/DDL 领域有压倒性优势（flowScope 完全不支持）；flowScope 在多方言覆盖、列级血缘、dbt 生态上取胜。

---

### 3.2 图谱模型

| 维度 | codeweb | flowScope |
|------|---------|-----------|
| **节点类型数量** | 21 种 | ~5 种 |
| **节点类型** | Procedure, Function, Table, View, MappedStatement, JavaMethod, JavaClass, JavaSql, JspPage, JspSql, Package, Trigger, Type, Sequence, Index, MaterializedView, Synonym, Event, BuiltinFunction, Unresolved, Custom | Table（源）, CTE（中间）, Output（目标）, Source, Target |
| **边类型** | DirectCall, CallsProcedure, InvokesMapper, TableAccess, ContainsSql + CGEF 自定义 | 数据流边（含列级映射） |
| **图引擎** | petgraph（Rust 内存有向图） | 自研（WASM 兼容） |
| **序列化格式** | bincode（二进制，带 blake3 指纹） | JSON（WASM 桥接） |
| **增量更新** | ✅ 文件指纹，仅重解析变更文件 | ❌ 每次全量分析 |
| **外部图导入** | ✅ CGEF 格式（JSON Schema 校验） | ❌ |
| **图去重** | ✅ `codeweb dedup` | ❌ |
| **跨语言桥接** | ✅ Java→Mapper→SQL→Procedure 完整链路 | ❌ 仅 SQL |

**结论**: codeweb 的图模型更丰富（涵盖数据库对象 + Java 代码实体），flowScope 的图模型更专注（SQL 数据流）。

---

### 3.3 查询与分析能力

| 维度 | codeweb | flowScope |
|------|---------|-----------|
| callers() 上游 | ✅ `detail <node>` | ❌ |
| callees() 下游 | ✅ `detail <node>` | ❌ |
| trace() 双向追踪 | ✅ `trace <node>` | 部分（正向血缘遍历） |
| impact() 影响分析 | ✅ `impact --node/--file` | ❌ |
| SQL 片段搜索→调用链 | ✅ `trace-sql <sql>` | ❌ |
| 声明式查询 | ✅ JSON QuerySpec（多步遍历/过滤/子图） | ❌ |
| 节点过滤/排序 | ✅ `nodes -s/-t/--sort-by` | ❌ |
| 项目统计 | ✅ `stats` | ❌ |
| Diff 变更 | ✅ `diff` | ❌ |
| SQL Linting | 53 规则（反模式） | 72 规则 + 自动修复 |
| SQL 补全 | ❌ | ✅ Completion API |
| AI 集成 | MCP 服务器（LLM 可查询图谱） | Librarian AI 聊天面板 |

**结论**: codeweb 的查询引擎更强大（双向遍历 + impact + QuerySpec），flowScope 在 SQL 质量和 AI 交互上有优势。

---

### 3.4 导出与可视化

| 维度 | codeweb | flowScope |
|------|---------|-----------|
| DOT/Graphviz | ✅ | ❌ |
| JSON | ✅ | ✅ |
| Mermaid | ✅ | ✅ |
| CSV | ❌ | ✅（ZIP 包） |
| XLSX/Excel | ❌ | ✅ |
| HTML（交互式） | ❌ | ✅（自包含 React 组件） |
| DuckDB | ❌ | ✅ |
| Dali | ❌ | ✅（企业血缘互操作） |
| **格式数量** | 3 | 8 |
| 浏览器 UI | ✅ Cytoscape.js + dagre | ✅ React + dagre/ELK |
| 终端 TUI | ✅ ratatui + crossterm | ❌ |
| VS Code 扩展 | ❌ | ✅ |
| NPM/TypeScript SDK | ❌ | ✅ @pondpilot/flowscope-core + React |
| MCP 服务器 | ✅ | ❌ |
| REST API | ✅ axum (9 endpoints) | ✅ serve mode (7+ endpoints) |

**结论**: flowScope 导出格式更丰富（8 vs 3），且有 VS Code + NPM SDK 生态；codeweb 有 TUI 和 MCP 独特优势。

---

### 3.5 部署与集成

| 维度 | codeweb | flowScope |
|------|---------|-----------|
| 运行环境 | 原生二进制（macOS/Linux/Windows） | 浏览器 WASM / 原生 CLI |
| 隐私模型 | 本地文件系统 | 浏览器端（SQL 不出设备） |
| 安装方式 | `cargo build` 源码编译 | `npm install` / `cargo install` / Web App |
| Feature Gate | 6 features | 1（serve） |
| 二进制大小 | **15 MB** | **63 MB**（含 WASM + React UI） |

---

## 4. 性能对比

### 4.1 测试环境

| 项目 | 详情 |
|------|------|
| 硬件 | macOS Apple Silicon (M-series) |
| Rust | stable |
| 构建模式 | `--release` |
| 测量工具 | hyperfine (`--runs 3 --warmup 1`) |
| 内存测量 | `/usr/bin/time -l` (BSD, bytes→MB) |
| codeweb 版本 | v0.8.10 (full features) |
| flowScope 版本 | v0.8.0 |

### 4.2 共享语料解析性能（ANSI SQL）

| 场景 | 行数 | codeweb | flowScope | 比值 |
|------|------|---------|-----------|------|
| shared-small (10 files) | 467 | **13.1 ms** | 17.4 ms | 1.33× |
| shared-medium (50 files) | 5,618 | **22.3 ms** | 108.6 ms | **4.87×** |
| shared-large (200 files) | 20,543 | **36.3 ms** | 689.7 ms | **19.0×** |

| 场景 | codeweb (lines/s) | flowScope (lines/s) |
|------|-------------------|---------------------|
| shared-small | 35,649 | 26,839 |
| shared-medium | 251,928 | 51,731 |
| shared-large | **565,923** | 29,785 |

> **注意**: small 场景下 codeweb `init` 的项目初始化开销（~12ms）占主导，导致吞吐量看上去偏低。medium/large 场景更能反映真实解析吞吐量。codeweb 在大规模场景下吞吐量是 flowScope 的 **19 倍**。
>
> 此差异与 ogsql-parser 官方 benchmark 一致（ogsql-parser 是 sqlparser-rs 的 2.4 倍，此处差距更大因为 codeweb 还有增量序列化、并行解析等优化）。

### 4.3 codeweb PL/pgSQL 解析性能（flowScope 不可比）

| 场景 | 行数 | 耗时 | lines/s |
|------|------|------|---------|
| plpgsql-medium (50 files) | 4,831 | 19.7 ms | 245,228 |
| plpgsql-large (100 files) | 19,762 | 49.7 ms | **397,626** |

> codeweb 在其核心场景（PL/pgSQL 存储过程）下吞吐量高达 **~40 万行/秒**。flowScope 完全不支持此场景。

### 4.4 导出性能（shared-large, 20,543 行）

| 格式 | codeweb | flowScope |
|------|---------|-----------|
| JSON | **6.3 ms** | 278.1 ms |
| Mermaid | **6.5 ms** | 369.0 ms |

> codeweb 导出速度极快（bincode 已缓存图谱结构，导出只是格式转换）。flowScope 每次重新解析 + 分析。

### 4.5 资源消耗（shared-large, 20,543 行）

| 指标 | codeweb | flowScope |
|------|---------|-----------|
| 内存峰值 (RSS) | **35.0 MB** | 175.7 MB |
| 二进制大小 | **15 MB** | 63 MB |

> flowScope 内存消耗是 codeweb 的 **5 倍**，可能与 WASM 运行时开销、JSON 序列化路径有关。codeweb 的 bincode 二进制序列化路径更轻量。

---

## 5. 适用场景推荐

### 场景 A: 存储过程调用链分析 → **codeweb** ✅

> "这个 openGauss 项目有 500+ 个存储过程，我需要知道 `proc_create_order` 调用了哪些过程，以及谁调用了它。"
>
> codeweb 是唯一选择 — flowScope 完全不解析存储过程 body。

### 场景 B: Java → Mapper → SQL 全链路追踪 → **codeweb** ✅

> "这个 Java 接口方法最终访问了哪个存储过程？经过哪些 MyBatis Mapper？"
>
> codeweb 的跨语言桥接是独有能力。

### 场景 C: 多方言 SQL 数据血缘 → **flowScope** ✅

> "公司用 PostgreSQL、Snowflake、BigQuery，我需要统一查看数据从源表到报表的列级血缘。"
>
> flowScope 的 14 种方言 + 列级血缘是核心优势。

### 场景 D: dbt 项目 SQL 质量 → **flowScope** ✅

> "我们的 dbt 模型需要 linting、自动修复、列级血缘可视化。" 
>
> flowScope 的 dbt/Jinja 支持 + 72 lint 规则 + VS Code 集成是最佳选择。

### 场景 E: 遗留系统存储过程重构 → **codeweb** ✅

> "需要理解 10 年前的 openGauss 存储过程系统，梳理调用关系，评估重构影响面。"
>
> codeweb 的 `impact` 分析 + 增量更新 + TUI 是最佳工具。

### 场景 F: 隐私敏感环境 SQL 分析 → **flowScope** ✅

> "SQL 不能离开用户设备，需要在浏览器里分析。"
>
> flowScope 的 WASM 架构是唯一选择。

### 场景 G: LLM 驱动的代码理解 → **codeweb** ✅

> "让 Claude/Cursor 能直接查询代码调用图谱，回答'这个修改会影响哪些存储过程？'"
>
> codeweb 的 MCP 服务器是独有能力。

### 场景 H: 企业数据治理血缘 → **视需求组合** 🔀

> "需要同时管理存储过程依赖 + 表级数据血缘。"
>
> 可通过 codeweb 的 CGEF 导入功能将 flowScope 的 SQL 血缘结果合并到 codeweb 图谱中。

---

## 6. 优劣势总结

### codeweb

| 优势 ✅ | 不足 ❌ |
|---------|--------|
| 存储过程调用图（独有能力） | 仅支持 openGauss/GaussDB 一种方言 |
| 跨语言桥接 Java→Mapper→SQL→Proc | 无列级血缘 |
| PL/pgSQL 完整语法支持 | 无 dbt/Jinja 支持 |
| 双向图查询（callers/callees/trace/impact） | 导出格式较少（3 vs 8） |
| 增量分析（快速迭代） | 无 Schema 感知（通配符展开） |
| CGEF 外部图谱导入/合并 | 无 VS Code 扩展 / NPM SDK |
| MCP 服务器（LLM 集成） | 无 SQL Linting 自动修复 |
| 解析性能极快（19× 于 flowScope） | 无浏览器 WASM 版本 |
| 内存/二进制极小（35MB / 15MB） | 仅源码编译安装 |

### flowScope

| 优势 ✅ | 不足 ❌ |
|---------|--------|
| 14 种 SQL 方言列级血缘 | 不解析存储过程 body |
| dbt/Jinja 模板支持 | 无跨语言桥接 |
| 浏览器 WASM（隐私优先） | 无增量分析 |
| SQL Linting 72 规则 + 自动修复 | 无双向图查询（仅正向血缘） |
| VS Code 扩展 + NPM SDK 生态 | 内存消耗较大（176MB） |
| 8 种导出格式 | 无外部图谱导入 |
| Schema 感知（DDL/数据库直连） | 无 MCP 服务器 |
| AI Librarian 自然语言查询 | 二进制较大（63MB） |
| Completion API（SQL 补全） | |

---

## 7. 改进建议（针对 codeweb）

基于 flowScope 的能力，codeweb 可考虑以下增强：

| 优先级 | 建议 | 来源 |
|--------|------|------|
| P1 | 扩展 SQL 方言支持（至少 PG/MySQL 通用子集） | flowScope 的多方言优势 |
| P1 | 引入列级血缘追踪 | flowScope 核心差异化能力 |
| P2 | 增加 CSV/XLSX/HTML 导出格式 | flowScope 的 8 种格式 |
| P2 | Schema DDL 感知（通配符展开） | flowScope 的 schema 感知 |
| P3 | dbt/Jinja 模板预处理 | flowScope 的数据工程支持 |
| P3 | NPM/TypeScript SDK 封装 | flowScope 的开发者生态 |
| P4 | WASM 浏览器版本 | flowScope 的隐私优势 |

---

## 8. 数据来源与复现

### 测试语料

| 语料集 | 路径 | 文件数 | 行数 |
|--------|------|--------|------|
| shared-small | `/tmp/flowscope-bench/corpus/shared-small/` | 10 | 467 |
| shared-medium | `/tmp/flowscope-bench/corpus/shared-medium/` | 50 | 5,618 |
| shared-large | `/tmp/flowscope-bench/corpus/shared-large/` | 200 | 20,543 |
| plpgsql-medium | `/tmp/flowscope-bench/corpus/plpgsql-medium/` | 50 | 4,831 |
| plpgsql-large | `/tmp/flowscope-bench/corpus/plpgsql-large/` | 100 | 19,762 |

### 复现命令

```bash
# 构建 codeweb
cargo build --release --features full

# 安装 flowScope
cargo install flowscope-cli

# 安装测量工具
brew install hyperfine

# 运行 benchmarks（详见 docs/plans/2026-08-12-flowscope-comparison.md Task 8）
```

### 原始数据

所有 hyperfine JSON 结果保存在 `/tmp/flowscope-bench/results/`。

---

*报告基于 2026-08-12 实测数据。codeweb v0.8.10, flowScope v0.8.0。*
