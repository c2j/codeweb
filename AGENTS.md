# codeweb

## TDD 工作流（Red → Green → Refactor）

本仓库采用测试驱动开发。一次循环只锁定一个行为。探索草稿不得直接合入，必须按本文件用 TDD 重写。

### 先读再改

1. 本仓库是**单 crate**（根 `Cargo.toml` 只有 `[package] codeweb`，没有 `[workspace]`）；功能靠 feature 切分（`cli` / `tui` / `serve` / `mcp` / `jsp` / `search-sql-v2`，`full` 为全开）。先确认改动落在哪个模块、哪个 feature 下。
2. 只用本文件列出的 cargo 命令；不要发明裸 `cargo update`。本仓库**没有** `rust-toolchain.toml`，toolchain 以 CI 使用的 `stable` 为准，不要擅自切换或加 `+nightly`。
3. 先跑与改动相关的最小测试；提交前再跑全量门禁（fmt + clippy + test，见「命令」）。
4. 完成一个循环后按「完成标准与汇报」汇报，不要只说「做完了」。

### Never / Ask first / Always

**Never（不必请示，直接禁止）**
- 删除、注释、跳过已有测试：`#[ignore]`、注释掉 `#[test]`、把断言改成 `is_ok()` / `unwrap()` 了事
- 修改人类已有测试的断言来迁就实现
- 先提交无测试的业务行为，再「回头补」
- 写永真测试：无断言、只检查 `is_some()`、只 verify 调用次数不查参数与状态
- 用全量端到端测试覆盖本可单测完成的改动
- 提交半成品；每次对人类可见的结果必须能构建且相关测试为绿
- 把探索草稿、临时脚本、调试 `dbg!`/`println!` 留在主代码

**Ask first**
- 改人类已有测试（含断言、fixture、golden 期望值）
- 新增运行时依赖、`unsafe`、新的 feature flag、新的外部服务
- 为不可测代码做超出当前改动路径的重构
- 接受/更新 golden file 或固定 fixture 的期望值，且行为含义发生变化
- 关闭 clippy lint、新增 `#[allow]`

**Always**
- 改遗留路径前：先写特征测试，锁定当前可观察行为（允许丑，必须可重复）
- 新行为：先有会失败的行为断言，再写最少实现
- 难以测试时：先造接缝，再写测试
- 测试名描述行为：`should_reject_negative_amount`
- 现有测试因你的改动失败：修实现，不修测试（除非人类明确要求）

测试权限：

| 测试来源 | 权限 |
|---|---|
| 人类已有测试 | 只读 |
| 本任务新建测试 | 可改，直到该行为稳定 |
| 过时或环境偶发失败 | 只报告，不擅自跳过 |

### 工作流

**Red** — 写生产行为之前先写测试；测试必须能被收集且必须失败（断言失败，或因缺失 API 导致编译失败，二者都算合法 Red）。修改已有功能先写特征测试锁定当前输出。一次只加一个行为的测试。

**Green** — 只写让当前失败测试通过的最少代码。禁止删掉/改掉失败测试、一次引入多个未验证变更、用更宽断言或 `unwrap()` 换绿。

**Refactor** — 相关测试全绿后才重构；重构后立刻跑同一组测试；范围限于当前改动路径。

**探索 vs 实现** — 需求或方案不清可写草稿验证；草稿不得合并；方案确定后必须走 TDD 重写。

### 遗留代码与接缝

**特征测试** — 锁定现有行为，不是证明它正确。本仓库**未引入 `insta`**，用固定 fixture + 显式断言，或与 golden 文件逐字比对。更新期望值必须在汇报里写清 diff 含义；默认不接受「看起来差不多」。

**接缝（优先顺序，靠后的更差）**
1. trait + 泛型或 `impl Trait`，测试用假类型
2. 用类型去掉非法状态（enum / newtype），而不是在测试里补分支
3. 时钟、ID、熵、文件系统做成可注入依赖；测试用 `tempfile` / 内存实现
4. `unsafe` 不是接缝。新增 `unsafe` 必须 Ask first，并写 `SAFETY` 注释

只给即将修改的代码路径补测试，不要一次性「补全覆盖率」。

### 测试分层

| 层级 | 位置 | 测什么 |
|---|---|---|
| 单元 | `src` 内 `#[cfg(test)] mod tests` | 模块不变量、错误类型、状态转换 |
| 集成 | `tests/*.rs` | 公共 API；不可访问私有项 |
| 文档测试 | `///` 示例 | 公共 API 必须可运行；禁止滥用 `no_run` |
| CLI/二进制 | 项目惯用方式（如 `assert_cmd`） | 退出码与 stdout 契约 |
| 不变量 | `proptest`（项目已用时） | 往返解析、幂等、单调性 |
| 特征/golden | 固定 fixture（本仓库未引入 `insta`） | 遗留输出；更新期望值必须说明 |

不要把本该测公共契约的内容塞进 `#[cfg(test)]` 去读私有字段。

Rust 的 Red 允许是：测试引用了尚不存在的类型/函数导致编译失败。不要为了先编译而写空 `todo!()` 再补测试——可以留 `todo!()` 仅作为 Green 的最小占位，且下一步必须替换。

### Rust Never 补遗
- 库代码（非 main/example/测试）用 `unwrap` / `expect` / `panic!` 做控制流
- 无必要 `unsafe`；有则必须 `SAFETY` 注释
- 一次性 `cargo update` 整个 lockfile
- 用 `#[allow(...)]` 静默应修复的 lint
- 为绿而改 golden/期望值却不解释行为是否应该变

### 命令

```bash
# 单测（循环内）
cargo test --features <feature> <test_name>

# 当前 feature 组合
cargo test --features <feature>
cargo test --features full                # 全 feature（抓交叉 feature 回归）

# 提交前门禁（与 .github/workflows/ci.yml 一致）
cargo fmt --all -- --check
cargo clippy --features full -- -D warnings
cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_
```

> CI **主动跳过** `test_path_mapping_applied` 与 `test_serve_*`（环境相关）。裸跑 `cargo test --features full` 会看到这些用例失败——那是既有的环境限制，不是你改坏了；不要为了让它们变绿去改实现。

> 每加一个新 feature flag，完成前必须跑 `cargo build --features full` + `cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_`。若 `--features full` 存在与本次改动无关的既有失败，需在提交信息里显式说明，并确认本次改动未使其更糟。

### 完成标准与汇报

提交或交还人类前，确认：
- [ ] 新行为有失败→通过的测试
- [ ] 修改的遗留路径有特征测试
- [ ] 未删除、跳过、改写人类已有测试
- [ ] 已跑与改动匹配的门禁（fmt + clippy + test）
- [ ] `cargo fmt` 与 clippy 干净
- [ ] 没有把草稿、调试输出、无主 lockfile 大面积变更带上

每个 TDD 循环汇报：
1. 测试了什么行为（测试函数名）
2. 最小实现改了哪些文件
3. 是否重构、边界在哪
4. 实际执行的命令和结果（通过 / 失败原因；不要只写「测过了」）

### 质量判断（自我检查）
- 这条测试在实现写错时会失败吗？
- 我是否在测行为，而不是私有实现细节？
- 我是否用 skip、更宽断言、unwrap、golden 盲收换绿？
- 命令是否来自本文件，而不是我编的？


Name is a play on "cobweb" (spider web) — the project builds a semantic **code graph**.

## Goal

Analyze semantic relationships in source code and produce a traversable call/reference graph.

### Phase 1: SQL Stored Procedure Call Graph

Parse SQL (openGauss / GaussDB dialect) to extract stored procedure call relationships: which procedure calls which, producing a directed graph.

Key crate: `ogsql-parser` — hand-written recursive descent parser for openGauss/GaussDB with built-in AST visitor (`Visitor` trait + `walk_statement`), full PL/pgSQL support, and serde on all AST types. Repo: https://github.com/c2j/ogsql-parser

**Status**: Planned — see `docs/plans/roadmap.md`

### Phase 2: + iBatis XML Mapper Chain

Extend the graph to include MyBatis/iBatis XML mappers: `MappedStatement → SQL → Stored Procedure`.

ogsql-parser already provides the `ibatis` feature (`parse_mapper_bytes()`) that parses XML mappers, resolves `<include>` fragments, flattens dynamic SQL, and produces the same `StatementInfo` AST as direct SQL parsing.

**Status**: Planned

### Phase 3: + Java Method Calls + Bridge

Parse Java source code to extract method calls and bridge `Java Method → Mapper → SQL → Stored Procedure`.

ogsql-parser already provides the `java` feature (`extract_sql_from_java()`) that extracts SQL from annotations (`@Query`), JDBC calls (`prepareStatement`), and string constants. Combined with `tree-sitter-java` for method call extraction.

Bridge rules:
- Java interface FQN == mapper `namespace`
- Java method name == mapper statement `id`
- `sqlSession.selectList("namespace.id")` → `MappedStatement`

**Status**: Planned

### Phase 3b: + JSP Scriptlet SQL Extraction

Extend SQL extraction to JSP files. Legacy Java Web systems often embed SQL directly in JSP via scriptlets (`<% %>`), declarations (`<%! %>`), and expressions (`<%= %>`). The `jsp` Cargo feature preprocesses JSP into synthetic Java and reuses ogsql-parser's `extract_sql_from_java()` — zero ogsql-parser changes.

Bridge rules:
- `JspPage → JspSql → Procedure/Table` (via reused `CallsProcedure`/`TableAccess` edges)
- `JspPage → JavaClass/JavaMethod` (constructor calls detected via tree-sitter on synthesized Java)
- `JavaMethod → JavaSql` (via `contains_sql` edge, linking method to its embedded SQL)
- `JspPage` `display_name` uses WEB-INF-relative path, with `line` pointing to first scriptlet/declaration

Limitation: JDBC escape syntax `{call pkg.x(...)}` is filtered by ogsql-parser's keyword gate (only SELECT/INSERT/UPDATE/DELETE/MERGE/WITH pass through). Direct stored procedure calls from JSP require a follow-up post-processor.

**Status**: Implemented behind `jsp` feature flag.

### Phase 4: + Bidirectional Query + CLI

Query engine: `callers()`, `callees()`, `trace()` (bidirectional), `impact()` (change analysis).

CLI: `codeweb analyze`, `codeweb trace --from <node> --direction forward|backward`, `codeweb stats`.

**Status**: Planned

## Stack

- **Language**: Rust (latest stable)
- **Build**: Cargo
- **SQL parsing**: `ogsql-parser` (git dependency, not on crates.io)
- **Graph**: `petgraph` (in-memory), export to DOT/JSON/Mermaid
- **Java parsing**: `tree-sitter-java` (ogsql-parser already depends on it)
- **XML parsing**: `quick-xml` via ogsql-parser `ibatis` feature
- **HTTP serve**: `axum` + `tokio` + `tower-http` (feature-gated behind `serve`)
- **Browser UI**: Cytoscape.js + dagre layout (embedded via `rust-embed`)

## Conventions

- Module per concern: parsing, graph model, analysis, output/export.
- Prefer `thiserror` for error types; no `anyhow` in library crates.
- CLI entrypoint via `clap`.
- Tests alongside source files (`#[cfg(test)]` modules), integration tests in `tests/`.
- `cargo fmt` and `cargo clippy` must pass before commit.

## Definition of Done

A task is **not complete** until ALL of the following pass:

1. **Compilation** — clean `cargo build` under EVERY feature combination the change touches:
   - At minimum: `cargo build` (default) and `cargo build --features <feature-being-added>`
   - If the change could affect any feature-gated code path (e.g. adding an enum variant that other features match against): `cargo build --features full` MUST also compile
   - Rule of thumb: when adding a new feature flag, run `cargo build --features full` before declaring done
2. **Unit tests** — `cargo test --features <feature>` passes (0 failures)
3. **Guard cases** — `cargo clippy --features <feature> -- -D warnings` is clean
4. **Formatting** — `cargo fmt -- --check` is clean

**Mandatory pre-completion verification matrix** (adapt to features touched):

```sh
cargo build --features full         # catches cross-feature regressions
cargo test --features full          # or per-feature if --features full has pre-existing failures
cargo clippy --features full -- -D warnings
cargo fmt -- --check
```

If `--features full` has **pre-existing** failures unrelated to the change, document them explicitly in the PR/commit message and verify the change doesn't make them worse.

## Git Workflow

**NEVER push directly to `main`.** All changes must go through a pull request.

1. Create a feature branch from `main`:
   ```sh
   git checkout -b <branch-name>
   ```
2. Commit changes to the feature branch.
3. Push the feature branch and create a PR:
   ```sh
   git push -u origin <branch-name>
   gh pr create --title "..." --body-file /tmp/pr-body.md
   ```
4. Merge via PR (squash or rebase preferred). Delete the feature branch after merge.

## Commands

```sh
cargo build                              # build (default features)
cargo build --features serve             # build with HTTP server + browser UI
cargo build --features mcp               # build with MCP server
cargo build --features jsp               # build with JSP SQL extraction
cargo build --features full              # build all features (catches cross-feature regressions)
cargo test                               # run all tests (default features)
cargo test --features serve              # run all tests including serve integration tests
cargo test --features jsp                # run all tests including jsp integration tests
cargo test --test <name>                 # run single integration test
cargo clippy -- -D warnings              # lint (CI-gating level)
cargo clippy --features serve -- -D warnings  # lint with serve feature
cargo clippy --features full -- -D warnings   # lint with all features
cargo fmt -- --check                     # format check
```
