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

## Conventions

- Module per concern: parsing, graph model, analysis, output/export.
- Prefer `thiserror` for error types; no `anyhow` in library crates.
- CLI entrypoint via `clap`.
- Tests alongside source files (`#[cfg(test)]` modules), integration tests in `tests/`.
- `cargo fmt` and `cargo clippy` must pass before commit.

## Commands

```sh
cargo build                  # build
cargo test                   # run all tests
cargo test --test <name>     # run single integration test
cargo clippy -- -D warnings  # lint (CI-gating level)
cargo fmt -- --check         # format check
```
