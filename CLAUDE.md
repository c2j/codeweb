# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**codeweb** is a semantic code graph analyzer written in Rust that builds traversable call/reference graphs across SQL stored procedures, MyBatis/iBatis XML mappers, and Java methods. The core insight: connect `Java Method → MyBatis Mapper → SQL → Stored Procedure` chains to enable impact analysis and call tracing.

### Key Graph Concepts

The graph is built from multiple semantic layers:
- **SQL Layer**: Stored procedures, functions, tables, views, packages, triggers extracted from SQL DDL/DML via [ogsql-parser](https://github.com/c2j/ogsql-parser) (hand-written recursive descent parser for openGauss/GaussDB dialect)
- **Mapper Layer**: MyBatis/iBatis XML `<select>/<update>/<insert>/<delete>` statements linking to SQL layer via callable lookup
- **Java Layer**: Java method calls extracted via [tree-sitter-java](https://github.com/tree-sitter/tree-sitter-java), bridging to mappers via mapper.xml references or `@Sql` annotations

Edges: `CallsProcedure`, `InvokesMapper`, `DirectCall`, `TableAccess`, etc.

## Build & Development Commands

```bash
# Build with default features (CLI + TUI)
cargo build

# Build with serve feature (HTTP server + browser UI)
cargo build --features serve

# Build with MCP server (Model Context Protocol for LLM integration)
cargo build --features mcp

# Build all features
cargo build --features full

# Check formatting
cargo fmt -- --check

# Fix formatting
cargo fmt

# Lint (CI-gate level)
cargo clippy --features full -- -D warnings

# Run specific features
cargo clippy --features serve -- -D warnings
```

## Testing

```bash
# Run all tests (feature-gated some flaky tests)
cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_

# Run tests with specific features
cargo test --features full
cargo test --features serve
cargo test --features mcp

# Run a single test file
cargo test --test integration_test

# Run with output
cargo test -- --nocapture --test-threads=1
```

### Test Structure

Tests live in `tests/` directory. Key patterns:
- `regress_*` tests check for regression fixes (e.g., `regress_lineage_table_upstream.rs` for issue #115-136)
- Feature-gated tests skip in CI when features are disabled
- Some tests marked with `#[ignore]` or skipped in CI (see `.github/workflows/ci.yml`)

## Architecture & Module Organization

### src/main.rs
CLI entry point. Routes commands via clap to submodules:
- `analyze` → parser layer + graph construction
- `trace`/`detail` → graph traversal
- `export` → export layer
- `serve` → HTTP server (feature-gated)
- `mcp` → MCP server (feature-gated)
- `tui` → terminal UI (feature-gated)

### src/parser/
Extracts call relationships from source files:
- `extractor.rs`: Main relationship extraction logic; uses ogsql-parser for SQL, tree-sitter-java for Java, XML parsing for MyBatis
- `java_method.rs`: Java method extraction via tree-sitter; handles annotations (`@Sql`, mapper references)
- `ibatis_loader.rs`: MyBatis XML parsing; extracts `<select>` / `<update>` / `<insert>` / `<delete>` statements
- `jsp_loader.rs` / `jsp_preprocessor.rs`: JSP embedded SQL extraction (feature-gated via `jsp` flag)
- `fingerprint.rs`: File hash tracking for incremental analysis
- `scanner.rs`: File discovery via globset patterns

### src/graph/
Core graph model and operations:
- `store.rs`: GraphStore — persistent graph representation with serialization (bincode), merging, deduplication, indexing. **Large file** (~250KB) — contains node/edge storage, query indexing, CGEF export preparation.
- `builder.rs`: Graph construction from parse results; reconciles multi-phase analysis (incremental re-parsing)
- `key.rs`: NodeKey types — node identity model (name + type + schema for SQL nodes, fqn for Java methods)
- `traverse.rs`: Graph traversal APIs — callers(), callees(), bidirectional trace()
- `inspect.rs`: Node detail extraction, impact analysis queries
- `lineage.rs` / `cluster.rs`: Specialized traversal for lineage/clustering operations
- `mod.rs`: Public GraphStore interface & command implementations (`analyze`, `trace`, `detail`, etc.)
- `query/`: Declarative query engine (QuerySpec JSON format)
- `search/`: Fuzzy search + SQL fragment search with optional fingerprint-based indexing (search-sql-v2 feature)

### src/export/
Multi-format export:
- `dot.rs`: Graphviz DOT format (visualizable in online tools)
- `json.rs`: JSON export for programmatic access
- `mermaid.rs`: Mermaid flowchart (markdown-embeddable)

### src/server/ (feature-gated: serve)
HTTP API + browser UI:
- `mod.rs`: Server entry & Axum router setup
- `handlers.rs`: `/api/v1/*` endpoint implementations (stats, nodes, trace, query, search-sql, export)
- `state.rs`: Shared application state (GraphStore + config)
- `assets.rs`: Embedded browser UI (Cytoscape.js + dagre layout via rust-embed)
- `access_log.rs`: HTTP request/response logging

### src/mcp/ (feature-gated: mcp)
MCP (Model Context Protocol) server for LLM integration:
- `server.rs`: MCP server entry & stdio JSON-RPC transport setup
- `tools.rs`: MCP tool definitions (`codeweb_stats`, `codeweb_nodes`, `codeweb_trace`, etc.)

### src/tui/ (feature-gated: tui)
Terminal UI (ratatui + crossterm):
- `app.rs`: TUI state machine (search, navigation, detail view)
- `theme.rs`: Color theme configuration
- `mod.rs`: TUI entry point & event loop

### src/project/
Project lifecycle management:
- `mod.rs`: codeweb project initialization, configuration loading
- `config.rs`: codeweb.toml parsing (project metadata, directory includes/excludes)

## Feature Flags

| Flag | Purpose | Default |
|------|---------|---------|
| `cli` | CLI via clap | ✅ |
| `tui` | Interactive terminal UI | ✅ |
| `jsp` | JSP embedded SQL extraction | ✅ |
| `serve` | HTTP API + browser UI (Axum + Cytoscape.js) | ❌ |
| `mcp` | MCP server for LLM integration | ❌ |
| `search-sql-v2` | Enhanced SQL search with fingerprint indexing | ❌ |
| `full` | All flags combined (cli + tui + serve + mcp + jsp + search-sql-v2) | ❌ |

**Why this matters**: Import statements, dependencies, and code paths vary by feature flag. Use `--features serve` or `--features full` to test server-gated functionality.

## Development Workflow

### Adding a New Command
1. Add clap subcommand in `main.rs`
2. Implement business logic in `graph/mod.rs` (query on GraphStore)
3. Format output via `export/` if needed
4. Add tests in `tests/`

### Adding Parsing for a New Language/Format
1. Add loader in `parser/` (e.g., `parser/new_lang_loader.rs`)
2. Extract relationships → Vec<(caller, callee, EdgeKind)> via ogsql-parser or tree-sitter
3. Feed to `GraphBuilder::add_edges()` in `graph/builder.rs`
4. Test incremental re-parse via fingerprint logic

### Performance Patterns
- **Incremental analysis**: Fingerprints (blake3) track file changes; re-parse only changed files via `parser/fingerprint.rs`
- **Parallelization**: rayon used for multi-threaded parsing (`parser/extractor.rs`)
- **Graph storage**: petgraph StableGraph with serde; bincode serialization for persistence
- **Search indexing**: Optional fingerprint-based SQL content indexing (search-sql-v2 feature)

## Common Issues & Patterns

### Graph Panics on NodeIndex/EdgeIndex
Issue #133 addresses stale index panics after `petgraph::StableGraph::swap_remove()`. Recent commit (acecaf2) prevents this via careful index management in `graph/builder.rs` during incremental updates.

### Deduplication Edge Cases
SQL nodes can appear as multiple types (`table`, `table*`, `view`, `view*`) across analysis phases. Dedup logic in `graph/store.rs` reconciles these; see `regress_dedup_table_view_cross_phase.rs`.

### XML Mapper Resolution
MyBatis mappers are identified by namespace + id (e.g., `com.example.UserMapper.selectById`). Must resolve to Java class methods for the Java→Mapper→SQL bridge. See `parser/extractor.rs` namespace resolution.

## i18n & Localization

UI strings via [rust-i18n](https://docs.rs/rust-i18n/). Locales: `en` (English), `zh-CN` (Chinese). Strings defined in `locales/*.yml`, loaded at compile time. Switch locale via environment or runtime API.

## Dependencies to Be Aware Of

- **ogsql-parser** (git dependency, tag v0.8.31): Hand-written recursive descent parser for openGauss/GaussDB SQL; custom dialect support via feature flags (ibatis, java)
- **petgraph** 0.7: Directed graph library; StableGraph used for node/edge storage with stable indices
- **tree-sitter-java** 0.23: Java source parsing; called via tree-sitter bindings
- **Axum** 0.7 + Tokio (feature-gated): Async HTTP server framework
- **Ratatui** 0.29 (feature-gated): TUI widget library

## CI/CD

- **Linting** (ci.yml): `cargo fmt --all -- --check` + `cargo clippy --features full -- -D warnings`
- **Tests** (ci.yml): `cargo test --features full` (with some tests skipped due to flakiness)
- **Build** (build.yml): Multi-platform build artifacts (likely)

Check `.github/workflows/ci.yml` for exact CI commands; CI gates on lint + test pass.

## Incremental Development Tips

1. **Feature-toggle testing**: Use `cargo test --features full` vs. `cargo test` to catch feature-specific regressions
2. **Single test**: `cargo test --test integration_test graph_builder` runs one test file; narrow with `-- --exact test_name`
3. **Graph debugging**: `codeweb detail <node>` or `codeweb query` (QuerySpec JSON) to inspect graph state
4. **Incremental re-parse**: Delete `.codeweb/fingerprints` to force full re-analysis (otherwise only changed files re-parsed)
5. **Export for inspection**: `codeweb export --format json` outputs graph as JSON for external analysis
