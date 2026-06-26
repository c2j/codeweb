# Unresolved Reference Diagnostics — Implementation Plan

> **Goal:** Make unresolved-reference warnings **reproducible** (file:line + source snippet) and **self-diagnosable** (strategy trace + candidate structure), so that resolver logic bugs can be root-caused from the log alone.

## Problem

`resolve_unresolved_nodes()` and `try_resolve_routine()` (src/graph/builder.rs) run **zero logging** — all 7 resolution strategies execute silently. When a reference survives, the developer sees only a creation-time warn with no clue *why* it wasn't matched. Most unresolved cases are resolver logic bugs (systematic schema/package asymmetry in `from_qualified_name`), not genuinely missing objects — but current diagnostics can't distinguish the two.

## Root Cause (verified)

`RoutineId::from_qualified_name` (mod.rs:134) parses `PKG.METHOD` → `{schema:Some("PKG"), package:None}`, while package-defined routines are stored as `{schema:None, package:Some("PKG")}`. The creation-time `proc_index` (keyed by full `RoutineId` struct) misses; the post-pass `lower_qualified` (keyed by `Display` string) usually rescues it. True survivors are subtler (e.g. schema-as-package fallback index only registers when `package.is_none()` — builder.rs:2292).

## Design

Two layers that compose via `raw_expr` as the log join key:

- **Layer 1 (creation-time, reproducibility anchor):** enrich the 6 existing `parse_log::warn` calls with `file:line` + source snippet (read-on-demand) + the parsed `callee_id` structure.
- **Layer 2 (post-pass, root-cause):** `try_resolve_routine` emits a `StrategyTrace` on miss; survivors log the trace + nearest candidates with **full `RoutineId`** (schema/package/name/kind), not just Display.

### New types (in builder.rs near `try_resolve_routine`)

```rust
pub(crate) enum ResolveOutcome {
    Resolved(petgraph::graph::NodeIndex),
    Miss(StrategyTrace),
}

pub(crate) struct StrategyTrace {
    pub parsed: RoutineId,                 // from_qualified_name result
    pub s1_qualified_key: String,
    pub s1_hit: bool,
    pub s3_pkg_lookup: Option<(String, String)>,  // None if strategy N/A
    pub s3_hit: bool,
    pub bare_candidates: Vec<(Option<String>, String)>, // (schema, display) at looked-up bare name
    pub caller_schemas: Vec<Option<String>>,
}
```

### New utility (src/parser/snippet.rs)

```rust
/// Read file line `line` ± `context` lines, return an annotated snippet.
/// Returns None on missing file / out-of-range; never panics.
pub fn read_snippet(file: &Path, line: usize, context: usize) -> Option<String>
```

### Survivor log format (target)

```
[WARN] ts svc.sql:88: unresolved(post-pass): 'S.M' survived
  parsed = {schema:Some(S), package:None, name:M, kind:Procedure}
  S1 lower_qualified['s.m'] -> miss
  S3 pkg_member_lower[('s','m')] -> miss
  bare 'm' -> 2 candidates: [(schema:None,'P.M'), (schema:Some(S),'S.P.M')]
  nearest: 'S.P.M' {schema:S,package:P,name:M,kind:Procedure}(d=1)
```

### Summary line (INFO, end of `resolve_unresolved_nodes`)

```
[INFO] ts resolve_unresolved_nodes: created=150 resolved=120 noise=25 survivors=5
```

---

## Tasks (TDD)

### Task 1 — `read_snippet()` utility
- **Files:** create `src/parser/snippet.rs`; add `pub mod snippet;` to `src/parser/mod.rs`.
- **Tests:** normal (line ± context), line at file boundary, line out-of-range → None, missing file → None, empty file.
- **Done:** `cargo test snippet --lib` green; pure function, no panic paths.

### Task 2 — `try_resolve_routine` emits `ResolveOutcome` (enhancement A)
- **Files:** `src/graph/builder.rs` (`try_resolve_routine` L2725 + its single caller in `resolve_unresolved_nodes` L2333).
- **Change:** return `ResolveOutcome` instead of `Option<NodeIndex>`; build `StrategyTrace` only on the miss path (zero hot-path cost). Caller matches `Resolved`/`Miss`.
- **Tests:** existing resolver behavior preserved (resolved cases still resolve); a deliberate-miss case returns `Miss` with `parsed` + `s1_qualified_key` populated correctly; ambiguous bare-name case populates `bare_candidates`.
- **Done:** `cargo test --lib` green; `try_resolve_routine` signature change ripples to 1 caller only.

### Task 3 — Survivor diagnostics consume trace + full-RoutineId candidates (enhancement A+B)
- **Files:** `src/graph/builder.rs` (`resolve_unresolved_nodes` Miss branch).
- **Change:** on `Miss`, format the target log above; compute nearest candidates via existing `SqlIdentifierMatcher::rank_candidates` over `lower_qualified` keys, take top-3 with distance ≤ 3, display each candidate's full `RoutineId` (read from `graph[idx]`).
- **Verify `SqlIdentifierMatcher` is not behind `search-sql-v2` feature gate; if it is, inline `strsim::levenshtein` (strsim is already a dependency).**
- **Tests:** synthetic graph with a survivor + a near candidate asserts the log line contains both names + the candidate's package field.
- **Done:** real-project `codeweb analyze` produces readable survivor lines.

### Task 4 — Creation-site enrichment: `:line` + snippet + `parsed as {...}` (Layer 1 + enhancement C)
- **Files:** `src/graph/builder.rs` 6 sites (trigger L327, synonym L822, sql-call L1516, mapper L1624, java L1776, jsp L2022).
- **Change:** each warn gains `:line` (vars already in scope — see site table), a `read_snippet(file, line, 1)` block, and a `parsed as {schema,package,name}` line (for sites using `from_qualified_name`, format the constructed `callee_id`/`func_id`/`target_key`).
- **Tests:** unit test that a missing call target produces a warn containing the file basename + line number (assert via `parse_log` test seam or by capturing the format string builder).
- **Done:** all 6 warn formats uniform; `cargo test --lib` green.

### Task 5 — Noise-filter trace + summary line (enhancement D)
- **Files:** `src/graph/builder.rs` (`is_noise_unresolved` call site L2316 + end of `resolve_unresolved_nodes`).
- **Change:** when `is_noise_unresolved` returns true, emit `parse_log::info` with `raw_expr` + matched rule category; at function end emit the summary INFO line with created/resolved/noise/survivor counts.
- **Tests:** a noise raw_expr produces an info entry; counts are accurate in summary.
- **Done:** summary line quantifies resolver health.

### Task 6 — Verification matrix (AGENTS.md DoD)
```sh
cargo build
cargo build --features full
cargo test --lib
cargo test --features full
cargo clippy --features full -- -D warnings
cargo fmt -- --check
codeweb analyze <real project>   # inspect .codeweb/parse.log survivor + summary lines
```

## Dependency / Ordering

- T1 (new file) ∥ T2 (builder.rs) — **no file overlap, parallel-safe.**
- T3 after T2 (consumes `ResolveOutcome`).
- T4 after T1 (needs `read_snippet`) and after T2/T3 (same file).
- T5 after T3 (same region).
- T6 last.

T2→T3→T4→T5 are all `builder.rs` → **strictly sequential** (file-write conflict avoidance).
