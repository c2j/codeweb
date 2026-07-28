# codeweb

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
