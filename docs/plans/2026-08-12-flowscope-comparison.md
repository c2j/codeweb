# codeweb vs flowScope 功能与性能对比方案

> **Goal:** 系统性地对比 codeweb 与 flowScope 的功能覆盖与性能表现，产出可量化的对比报告。

**背景:** codeweb 是跨语言代码图谱分析工具（SQL + Java + MyBatis + JSP），flowScope 是纯 SQL 数据血缘分析引擎（WASM 浏览器端）。两者在 SQL 解析、图谱可视化、导出格式等维度有交集，但核心定位不同。对比需兼顾"同类功能横向对比"与"差异化能力定性分析"。

**对比范围:** 聚焦两者共同覆盖的 SQL 解析 / 图谱构建 / 查询 / 导出 / 可视化维度，同时对各自独有能力做定性描述。

## References

flowScope 所有能力声明均基于以下可验证来源：

| 来源 | URL |
|------|-----|
| GitHub 仓库 | https://github.com/pondpilot/flowscope |
| 官方文档 | https://docs.pondpilot.io/flowscope/ |
| 方言覆盖文档 | https://docs.pondpilot.io/flowscope/sql-dialects/ 或 `docs/dialect-coverage.md` (repo) |
| CLI 参考 | https://docs.pondpilot.io/flowscope/cli/ |
| API 参考 | https://docs.pondpilot.io/flowscope/api/ |
| crates.io | https://crates.io/crates/flowscope-core |
| NPM | https://www.npmjs.com/package/@pondpilot/flowscope-core |

codeweb 能力声明基于本仓库 README.md 与源码。

---

## Phase 1: 功能对比矩阵

### Task 1: SQL 解析能力对比

**对比维度：**

| 维度 | codeweb | flowScope | 对比方法 |
|------|---------|-----------|---------|
| SQL 方言 | openGauss/GaussDB（仅一种） | 13+ 种（PG, Snowflake, BigQuery, DuckDB, MySQL, SQLite, Redshift, Oracle, MSSQL, ClickHouse 等） | 定性分析 |
| 解析方式 | ogsql-parser（手写递归下降） | sqlparser-rs（通用 SQL 解析器） | 架构对比 |
| 支持语句类型 | CALL/EXECUTE/SELECT/INSERT/UPDATE/DELETE/MERGE/CREATE/DDL | SELECT/INSERT/UPDATE/DELETE/MERGE/CREATE/COPY/UNLOAD/ALTER | 文档对比 |
| 存储过程 Body 解析 | ✅ 完整 PL/pgSQL body 解析，提取嵌套调用 | ❌ 不解析存储过程 body | 定性分析 |
| CTE 支持 | 基础支持 | ✅ 完整 CTE 血缘追踪 | 定性分析 |
| 列级血缘 | ❌ 不支持 | ✅ 列级数据流追踪 | 定性分析 |
| 表级血缘 | 部分（TableAccess 边） | ✅ 完整表级血缘 | 定性分析 |
| dbt/Jinja 模板 | ❌ | ✅ ref(), source(), config(), var() | 定性分析 |
| Schema 感知 | ❌ | ✅ Schema DDL 文件 + 数据库连麦 | 定性分析 |
| 存储过程调用图 | ✅ CALL/EXECUTE 关系（核心能力） | ❌ | 定性分析 |
| PL/pgSQL 语法 | ✅ 完整支持（变量声明、游标、异常处理） | ❌ | 定性分析 |
| Package 支持 | ✅（openGauss package） | ❌ | 定性分析 |

**方法:** 
1. 整理 codeweb `ogsql-parser` 支持的语法范围（查阅 ogsql-parser 文档）
2. 整理 flowScope `flowscope-core` 支持的语法/方言范围（查阅 flowScope docs/dialect-coverage.md）
3. 输出对比表格 + 各工具适用场景分析

---

### Task 2: 图谱模型对比

| 维度 | codeweb | flowScope | 对比方法 |
|------|---------|-----------|---------|
| 节点类型 | 21 种（proc/func/table/view/mapper/method/class/sql/jsp/jspsql/pkg/trigger/type/seq/index/mview/synonym/event/builtin/unres） | ~5 种（Table/CTE/Output/Source/Target） | 文档对比 |
| 边类型 | DirectCall/CallsProcedure/InvokesMapper/TableAccess/ContainsSql + CGEF 自定义 | 数据流边（source→target，含列级映射） | 文档对比 |
| 图引擎 | petgraph（内存有向图） | 自研（WASM 兼容） | 架构对比 |
| 序列化 | bincode（二进制，blake3 指纹） | JSON（通过 WASM 桥接） | 定性分析 |
| 增量更新 | ✅（文件指纹，仅重新解析变更文件） | ❌（每次全量分析） | 定性分析 |
| 外部图导入 | ✅ CGEF 格式导入/合并 | ❌ | 定性分析 |
| 图去重 | ✅ `codeweb dedup` | ❌ | 定性分析 |
| 跨语言桥接 | ✅ Java→Mapper→SQL→Procedure 链路 | ❌（仅 SQL） | 定性分析 |

---

### Task 3: 查询与分析能力对比

| 维度 | codeweb | flowScope |
|------|---------|-----------|
| callers() | ✅ `codeweb detail <node>` | ❌（血缘方向相反） |
| callees() | ✅ `codeweb detail <node>` | ❌ |
| trace() 双向 | ✅ `codeweb trace <node>` | 部分（正向血缘遍历） |
| impact() 影响分析 | ✅ `codeweb impact --node/--file` | ❌ |
| SQL 片段搜索 | ✅ `codeweb trace-sql <sql>` | ❌ |
| 声明式查询 | ✅ JSON QuerySpec（多步遍历） | ❌ |
| 节点过滤/排序 | ✅ `codeweb nodes -s/-t/--sort-by` | ❌ |
| SQL Linting | ❌ | ✅ 72 规则 9 类别 + 自动修复 |
| SQL 补全 | ❌ | ✅ Completion API |
| 项目统计 | ✅ `codeweb stats` | ❌ |
| Diff 变更 | ✅ `codeweb diff` | ❌ |

---

### Task 4: 导出与可视化对比

| 维度 | codeweb | flowScope |
|------|---------|-----------|
| DOT/Graphviz | ✅ | ❌ |
| JSON | ✅ | ✅ |
| Mermaid | ✅ | ✅ |
| CSV | ❌ | ✅ |
| XLSX/Excel | ❌ | ✅ |
| HTML（自包含交互式） | ❌ | ✅ |
| DuckDB | ❌ | ✅ |
| Dali | ❌ | ✅ |
| 浏览器 UI | ✅ Cytoscape.js + dagre | ✅ React + dagre/ELK |
| 终端 TUI | ✅ ratatui + crossterm | ❌ |
| VS Code 扩展 | ❌ | ✅ |
| NPM/TypeScript SDK | ❌ | ✅ @pondpilot/flowscope-core |
| MCP 服务器 | ✅ | ❌ |
| REST API | ✅ axum (9 endpoints) | ✅ serve mode (7+ endpoints) |

---

### Task 5: 部署与集成对比

| 维度 | codeweb | flowScope |
|------|---------|-----------|
| 运行环境 | 原生二进制（macOS/Linux/Windows） | 浏览器 WASM / 原生 CLI |
| 隐私模型 | 本地文件系统 | 浏览器端（SQL 不出设备） |
| 安装方式 | `cargo build` 源码编译 | npm install / cargo install / Web App |
| Feature Gate | ✅ 6 features（cli/tui/serve/mcp/jsp/search-sql-v2） | ❌ serve feature gate |
| 二进制大小 | 待测量 | 待测量 |

---

## Phase 2: 性能对比方案

### Task 6: 基准测试环境搭建

**目标:** 准备统一的测试环境和 SQL 测试语料库。

**环境要求:**
- 硬件: 统一机器（macOS，Apple Silicon）
- Rust 版本: stable（记录具体版本号，如 `rustc 1.85.0`）
- 构建模式: `--release`
- 预热: 每个测试运行 3 次取中位数

**性能测量工具:**

| 用途 | 工具 | macOS 命令 |
|------|------|-----------|
| 执行时间（中位数） | [hyperfine](https://github.com/sharkdp/hyperfine) | `hyperfine --runs 3 --warmup 1 '<cmd>'` |
| 内存峰值（最大 RSS） | `/usr/bin/time -l` | `/usr/bin/time -l <cmd> 2>&1 \| grep 'maximum resident'` |
| 二进制大小 | `ls -lh` | `ls -lh target/release/codeweb` |

> **说明**: macOS `time` (bash builtin) 不报告 RSS；需使用 `/usr/bin/time -l`（BSD 版本）获取 `maximum resident set size`。**注意**: BSD `/usr/bin/time -l` 输出单位为 **bytes**，需 `÷ 1048576` 转换为 MB。所有测量使用 `hyperfine` 统一收集时间数据，内存单独用 `/usr/bin/time -l` 测量。

**SQL 测试语料设计:**

语料分为两类：**共享语料**（通用 ANSI SQL，两者均可分析）和 **codeweb 专有语料**（PL/pgSQL 存储过程，仅 codeweb 可测）。

**A. 共享语料（ANSI SQL，两者可比）：**

| 语料集 | 描述 | 文件数 | 预估总行数 | 方言 |
|--------|------|--------|-----------|------|
| shared-small | 简单 SELECT + 少量 JOIN | 10 | ~500 | ANSI SQL (通用) |
| shared-medium | 中等复杂度（CTE + 子查询 + 多表 JOIN + UNION） | 50 | ~5,000 | ANSI SQL (通用) |
| shared-large | 大量查询语句（多文件批处理场景） | 200 | ~20,000 | ANSI SQL (通用) |

> flowScope 使用 `--dialect generic` 运行这些语料（generic/ansi 方言均可解析 ANSI SQL）。

**B. codeweb 专有语料（PL/pgSQL，仅 codeweb 可测）：**

| 语料集 | 描述 | 文件数 | 预估总行数 | 方言 |
|--------|------|--------|-----------|------|
| plpgsql-medium | 存储过程（含 CALL/EXECUTE + 嵌套调用） | 50 | ~5,000 | openGauss |
| plpgsql-large | 复杂存储过程（含游标 + 异常处理 + 动态 SQL） | 100 | ~20,000 | openGauss |

> 这些语料 flowScope **无法解析**（不解析存储过程 body），仅用于测量 codeweb 在核心场景下的性能上限，**不出现在共享对比表中**，而是在独立章节呈现。

**C. 真实项目语料（混合）：**

| 语料集 | 描述 | 来源 |
|--------|------|------|
| real-world | 从 codeweb `tests/fixtures/` 选取的混合 SQL 项目（含查询 + 存储过程） | 已有 fixtures |

> 真实项目语料中的存储过程部分仅 codeweb 处理。对比时按文件类型分类统计。

---

### Task 7: 性能指标定义

| 指标 | 测量方法 | 单位 | 优先级 |
|------|---------|------|--------|
| 解析吞吐量 | `hyperfine` 测量全量分析总耗时 → lines/sec | lines/sec | P0 |
| 解析吞吐量（文件） | 文件数 / `hyperfine` 测量耗时 → files/sec | files/sec | P0 |
| 图谱构建时间 | 从 parse log 提取解析/构建阶段耗时 | ms | P0 |
| 内存峰值 | `/usr/bin/time -l` 获取 maximum resident set size | MB | P1 |
| 查询延迟（trace） | `hyperfine --runs 3 'codeweb trace <node>'` | ms | P1 |
| 查询延迟（impact） | `hyperfine --runs 3 'codeweb impact --node <node>'` | ms | P1 |
| 二进制大小 | `ls -lh target/release/codeweb` | MB | P2 |
| 冷启动时间 | `hyperfine --runs 5 '<cmd> --help'` | ms | P2 |
| 导出时间 | `hyperfine` 测量导出大图为各种格式 | ms | P2 |
| 增量分析加速比 | 全量耗时 / 增量耗时（均用 `hyperfine`） | 比值 | P2 |

---

### Task 8: 性能测试脚本

**前置条件:** 安装 `hyperfine`（`brew install hyperfine`）。

**codeweb 性能测量:**

```bash
# === 共享语料测试（与 flowScope 可比） ===

# 全量分析（shared-large = 200 files, 20,000 lines ANSI SQL）
hyperfine --runs 3 --warmup 1 \
  'codeweb init bench-shared-large -d ./sql-corpus/shared-large'

# 内存峰值
/usr/bin/time -l codeweb init bench-shared-large -d ./sql-corpus/shared-large 2>&1 | grep 'maximum resident'

# 增量分析（二次运行）
hyperfine --runs 3 --warmup 1 \
  'codeweb analyze'

# 导出（JSON, Mermaid, DOT）
hyperfine --runs 3 --warmup 1 \
  'codeweb export --format json --output /dev/null'
hyperfine --runs 3 --warmup 1 \
  'codeweb export --format mermaid --output /dev/null'
hyperfine --runs 3 --warmup 1 \
  'codeweb export --format dot --output /dev/null'

# 查询延迟
hyperfine --runs 3 --warmup 1 \
  'codeweb trace "target_node"'
hyperfine --runs 3 --warmup 1 \
  'codeweb impact --node "target_node" --format json'

# === codeweb 专有语料测试（仅 codeweb，不出现在共享对比表中） ===

# PL/pgSQL 存储过程
hyperfine --runs 3 --warmup 1 \
  'codeweb init bench-plpgsql -d ./sql-corpus/plpgsql-large'
```

```bash
# === flowScope 性能测量 ===
# flowScope CLI 选项来源: https://docs.pondpilot.io/flowscope/cli/
# flowScope v0.7.0, 安装: cargo install flowscope-cli

# 全量分析（shared-large, 使用 generic 方言兼容 ANSI SQL）
hyperfine --runs 3 --warmup 1 \
  'flowscope -d generic sql-corpus/shared-large/*.sql'

# 内存峰值
/usr/bin/time -l flowscope -d generic sql-corpus/shared-large/*.sql 2>&1 | grep 'maximum resident'

# 导出（JSON, Mermaid, HTML）
hyperfine --runs 3 --warmup 1 \
  'flowscope -d generic -f json sql-corpus/shared-large/*.sql > /dev/null'
hyperfine --runs 3 --warmup 1 \
  'flowscope -d generic -f mermaid sql-corpus/shared-large/*.sql > /dev/null'
hyperfine --runs 3 --warmup 1 \
  'flowscope -d generic -f html -o /tmp/lineage.html sql-corpus/shared-large/*.sql'

# 冷启动
hyperfine --runs 5 'flowscope --help'
```

**注意事项:**
- flowScope CLI 方言参数使用 `-d generic`（覆盖通用 ANSI SQL，与 codeweb 的 openGauss 子集最大交集）
- flowScope 不支持存储过程 body 解析，`plpgsql-*` 语料仅对 codeweb 有效，不出现在共享对比中
- 两者都用 `--release` 构建
- `hyperfine` 自动计算中位数、标准差，并做统计检验

---

### Task 9: 性能数据收集与可视化

**输出格式:** 性能对比表格（共享语料 + codeweb 专有语料分表呈现）

**A. 共享语料对比表（两者均可运行）：**

```
| 测试场景 | codeweb (lines/s) | flowScope (lines/s) | 比值 |
|----------|-------------------|---------------------|------|
| shared-small (500 lines, 10 files)   | xxx | xxx | x.xx |
| shared-medium (5,000 lines, 50 files) | xxx | xxx | x.xx |
| shared-large (20,000 lines, 200 files)| xxx | xxx | x.xx |
```

```
| 测试场景 | codeweb 内存峰值 (MB) | flowScope 内存峰值 (MB) | 比值 |
|----------|----------------------|------------------------|------|
| shared-large | xxx | xxx | x.xx |
```

```
| 测试场景 | codeweb JSON 导出 (ms) | flowScope JSON 导出 (ms) |
|----------|-----------------------|-------------------------|
| shared-large | xxx | xxx |
```

**B. codeweb 专有语料表（仅 codeweb，标注 PL/pgSQL）：**

```
| 测试场景 | codeweb (lines/s) | 内存峰值 (MB) | 备注 |
|----------|-------------------|--------------|------|
| plpgsql-medium (5,000 lines) | xxx | xxx | 存储过程 + CALL/EXECUTE |
| plpgsql-large (20,000 lines)  | xxx | xxx | 存储过程 + 游标 + 动态 SQL |
| real-world (mixed)            | xxx | xxx | 混合项目（查询 + 存储过程） |
```

> **关键**: 共享对比表仅包含两者均可解析的 ANSI SQL 语料。PL/pgSQL 语料不出现在 codeweb vs flowScope 并排对比中，以避免误导性比较。

---

## Phase 3: 定位与适用场景分析

### Task 10: 差异化能力总结

**codeweb 独有优势:**
1. **存储过程调用图** — 核心差异化能力，flowScope 完全不支持
2. **跨语言桥接** — Java → Mapper → SQL → Procedure 完整链路
3. **PL/pgSQL 完整语法** — 变量声明、游标、异常处理、动态 SQL
4. **双向图查询** — callers/callees/trace/impact（flowScope 只有正向血缘）
5. **增量分析** — 变更文件指纹，大幅加速迭代
6. **CGEF 导入/合并** — 与企业血缘系统对接
7. **MCP 服务器** — LLM 可直接查询代码图谱
8. **TUI 终端 UI** — 无浏览器环境可用

**flowScope 独有优势:**
1. **多方言 SQL 血缘** — 13+ 种数据库方言，列级数据流追踪
2. **浏览器端运行** — WASM，SQL 不出设备，零部署
3. **SQL Linting** — 72 规则 + 自动修复，提升 SQL 质量
4. **dbt/Jinja 支持** — 数据工程工作流必备
5. **VS Code 扩展 + NPM SDK** — 开发者生态完善
6. **AI Librarian** — 自然语言查询数据血缘
7. **Schema 感知** — DDL 文件或数据库直连，通配符展开
8. **列级血缘** — 追踪 `SUM(o.amount) → total` 等转换

**重叠领域（可直接对比）:**
- SQL 解析能力（查询语句）
- 图谱可视化（Web UI）
- 导出格式（JSON, Mermaid）
- CLI 工具链
- REST API

---

## Phase 4: 对比报告输出

### Task 11: 编写对比报告

**报告结构:**

```markdown
# codeweb vs flowScope 对比报告

## 1. 执行摘要
- 一句话定位差异
- 核心结论

## 2. 工具概述
- codeweb 简介
- flowScope 简介

## 3. 功能对比
- 3.1 SQL 解析能力
- 3.2 图谱模型
- 3.3 查询与分析
- 3.4 导出与可视化
- 3.5 部署与集成

## 4. 性能对比
- 4.1 测试环境
- 4.2 解析性能
- 4.3 查询性能
- 4.4 资源消耗

## 5. 适用场景推荐
- 场景 A: 存储过程调用链分析 → codeweb
- 场景 B: 多方言 SQL 数据血缘 → flowScope
- 场景 C: Java + SQL 全链路追踪 → codeweb
- 场景 D: dbt 项目 SQL 质量 → flowScope
- 场景 E: 企业数据治理血缘 → 视需求组合

## 6. 优劣势总结
- codeweb 优势 / 不足
- flowScope 优势 / 不足

## 7. 改进建议（针对 codeweb）
- 可借鉴 flowScope 的特性
```

---

## 执行计划总览

| Phase | Task | 预估工作量 | 依赖 |
|-------|------|-----------|------|
| Phase 1 | Task 1: SQL 解析对比 | 1h | - |
| Phase 1 | Task 2: 图谱模型对比 | 0.5h | - |
| Phase 1 | Task 3: 查询分析对比 | 0.5h | - |
| Phase 1 | Task 4: 导出可视化对比 | 0.5h | - |
| Phase 1 | Task 5: 部署集成对比 | 0.5h | - |
| Phase 2 | Task 6: 基准测试环境搭建 | 2h | - |
| Phase 2 | Task 7: 性能指标定义 | 0.5h | Task 6 |
| Phase 2 | Task 8: 性能测试脚本 | 1h | Task 6, 7 |
| Phase 2 | Task 9: 数据收集与可视化 | 1h | Task 8 |
| Phase 3 | Task 10: 差异化总结 | 1h | Task 1-5 |
| Phase 4 | Task 11: 对比报告编写 | 2h | 全部 |

**总预估:** ~10h

---

## 关键注意事项

1. **方言不匹配问题** — flowScope 不支持 openGauss/GaussDB 方言，codeweb 不支持 PG/Snowflake 等。SQL 解析对比需使用两者共同支持的 SQL 子集（通用 ANSI SQL 查询语句）。
2. **存储过程不可比** — flowScope 不解析存储过程 body，涉及 `CALL` / `EXECUTE` / PL/pgSQL 的测试仅对 codeweb 有效。
3. **flowScope 需要 Node.js 或浏览器环境**（NPM 包），CLI 为原生二进制。性能对比统一使用 CLI。
4. **codeweb 需要 ogsql-parser git 依赖**，构建前需确保网络可访问。
