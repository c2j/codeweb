# Copilot instructions for codeweb

Purpose
- Give future Copilot sessions concise, repo-specific guidance: how to build/test/lint, the big-picture architecture, and non-obvious conventions.

Quick commands
- Build (default features):
  - cargo build
- Build with features:
  - cargo build --features serve
  - cargo build --features mcp
  - cargo build --features full
- Tests:
  - Run all tests: cargo test
  - Run tests with feature: cargo test --features serve
  - Run a single unit test by name: cargo test <test_name>
  - Run a single integration test file: cargo test --test <integration_test_name>
- Lint & format checks (CI uses these):
  - cargo fmt -- --check
  - cargo clippy --features full -- -D warnings
  - cargo fmt (to autoformat)

CI notes
- CI runs clippy and tests with --features full. Some long/host-bound tests are skipped in CI via -- --skip test_path_mapping_applied --skip test_serve_.
- Follow the project's Definition of Done: verify builds/tests/clippy/format under relevant feature combos (especially --features full when touching feature-gated code).

High-level architecture (big picture)
- Purpose: build a semantic directed graph linking Java methods, MyBatis mappers, SQL, and stored procedures/tables.
- Layers:
  - CLI (src/main.rs) — clap commands (init, analyze, trace, export, serve, mcp, tui)
  - Parser layer (src/parser/*) — ogsql-parser for SQL; tree-sitter-java for Java; ibatis XML loader for mappers; JSP preprocessing when jsp feature enabled
  - Graph model (src/graph/*) — GraphStore, builder, resolver, traversal and declarative QuerySpec
  - Export/import (src/export, src/import) — DOT/JSON/Mermaid and CGEF import/merge
  - Runtime surfaces:
    - TUI (feature: tui)
    - HTTP server + Browser UI (feature: serve)
    - MCP server (feature: mcp) — exposes MCP tools for LLM clients
- Incremental analysis: file fingerprinting (blake3) used to avoid re-parsing unchanged files; GraphStore serialized with bincode.
- Exports: DOT, JSON, Mermaid. Imports: CGEF JSON.

Key repo-specific conventions and mapping rules
- Feature flags matter: default = [cli, tui, jsp]. Use `--features full` when making changes that touch multiple gated areas.
- Java <-> Mapper <-> SQL mapping rules (important for trace tasks):
  - Java interface FQN == mapper namespace
  - Java method name == mapper statement id
  - Calls like sqlSession.selectList("namespace.id") map to a MappedStatement
- JSP feature: jsp preprocesses JSP into synthetic Java and extracts SQL via ogsql-parser's Java extraction; JDBC escape `{call ...}` may require post-processing
- Node types & tags: procedures (proc/ proc*), functions (func/func*), table/table*, mapper, method, jsp/jspsql, etc. Many commands accept node-type filters (e.g., --type proc)
- Incremental test/dev workflow:
  - Run targeted unit tests during development: cargo test <test_name>
  - For integration tests use: cargo test --test <name>
  - CI may skip long tests — be aware when reproducing CI locally (remove --skip args to run everything)
- Code quality gates:
  - No any/anyhow in libraries; prefer thiserror for error types
  - Use cargo fmt and cargo clippy with -D warnings before PR
- Git workflow: never push directly to main; branch naming: feature/<desc> or fix/<desc> (kebab-case); create PRs and ensure CI passes
- Commit trailer: include Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com> when a Copilot-made commit is created (project policy enforced by tooling in this workspace)

Where to look for deeper context
- README.md, CONTRIBUTION.md, AGENTS.md, docs/DeveloperGuide.md, docs/getting-started.md, docs/serve-api-guide.md
- parser/ and graph/ folders for how relationships are extracted and represented

Existing AI-agent configs
- AGENTS.md exists and documents phased goals and conventions for automated agents (useful for MCP/tool integration). No CLAUDE.md or other assistant-specific config files were found.

MCP server integration snippet (from README)
- Claude Desktop example:
  {
    "mcpServers": {
      "codeweb": {
        "command": "/path/to/codeweb",
        "args": ["mcp", "--project", "/path/to/your/project"]
      }
    }
  }

Notes for Copilot sessions
- When reasoning about call chains prefer starting from parser outputs (parser/*) and GraphStore APIs rather than searching ad-hoc across the repo.
- When editing or adding feature-gated code, run cargo build/test/clippy with --features full locally before opening PR.
- Use exported node types and mapping rules to resolve cross-language edges (Java → mapper → SQL → proc)

Files used to compose this guidance
- README.md, CONTRIBUTION.md, AGENTS.md, Cargo.toml, .github/workflows/ci.yml

---

If you'd like, configure MCP servers (Claude/Desktop, Cursor, VS Code Copilot Chat) for this repo now — say which server(s) to add and Copilot will prepare suggested config snippets and a short setup checklist.
