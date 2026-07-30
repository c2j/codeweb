# Plan: Fix Issues #113 & #112 (Round 2)

## Scope

Two independent enhancements building on Round 1 infrastructure:

| ID | Change | Files touched | LOC ~ |
|---|---|---|---|
| #113 | `--exact`/`--regex`/`--all-matches`/`--fail-on-multiple` for node matching | 7 | +150 |
| #112 | `--summarize-tables` flag on `detail` for package-level R/W table aggregation | 2 | +80 |

## #113: Node Matching Options

### Problem (confirmed)

4 CLI commands + 1 internal function share identical "first match wins" pattern:
- `cmd_trace` (main.rs:1010-1026)
- `detail_one` (main.rs:1382-1397)
- `resolve_node_target` / `cmd_impact` (main.rs:3290-3318)
- `cmd_inspect` (main.rs:3185-3199)
- `process_mark` (mark.rs:407-438)

All call `store.search_nodes(name)`, check `matches.len() > 1`, print list, take `matches[0]`, discard rest.

### Design

**New types in `src/graph/search/mod.rs`:**

```rust
#[derive(Clone, Copy)]
pub enum MatchMode {
    Substring,  // default, current behavior
    Exact,      // exact match, case-insensitive
    Regex,      // compiled regex match
}
```

**New helper in `src/graph/store.rs` (or `src/graph/search/mod.rs`):**

```rust
pub fn search_nodes_with_mode(&self, query: &str, mode: MatchMode)
    -> Vec<(NodeIndex, String)>
```

For `Exact`: binary search in name_index for exact lowercase key (O(log n) — already sorted).
For `Regex`: compile query as `Regex`, linear scan matching against `key_lower`.
For `Substring`: existing `search_nodes()`.

**New helper `resolve_single_node()` in `src/graph/store.rs`:**

Centralizes the 4 duplicate "first match" blocks:

```rust
pub fn resolve_single_node(
    &self,
    name: &str,
    mode: MatchMode,
    all_matches: bool,
    fail_on_multiple: bool,
) -> ResolveResult
```

Where `ResolveResult` is:
```rust
pub enum ResolveResult {
    Single(NodeIndex, String),           // one match
    Multiple(Vec<(NodeIndex, String)>),  // all_matches + multiple
    Empty,                                // no match
}
```

Callers then handle the 3 cases.

**CLI flags (added to Trace, Detail, Impact, Inspect Arg structs):**

```rust
#[arg(long)]
exact: bool,          // --exact: use MatchMode::Exact

#[arg(long, conflicts_with = "exact")]
regex: bool,          // --regex: use MatchMode::Regex

#[arg(long)]
all_matches: bool,    // --all-matches: process ALL matching nodes

#[arg(long)]
fail_on_multiple: bool, // --fail-on-multiple: non-zero exit on ambiguity
```

Match mode precedence: `--exact` > `--regex` > substring (default).

**Files changed:**
1. `Cargo.toml` — add `regex = "1"` dependency (v1.13.1 already in lockfile)
2. `src/graph/store.rs` — add `search_nodes_with_mode()`, `resolve_single_node()`
3. `src/graph/search/mod.rs` — add `MatchMode` enum
4. `src/main.rs` — update 4 Arg structs + 4 handler functions
5. `src/mark.rs` — update `process_mark`

### Regression Tests

- `search_nodes_with_mode` exact: test that only exact-cased keys match
- `search_nodes_with_mode` regex: test pattern matching
- `resolve_single_node` single match: returns Single
- `resolve_single_node` multiple + fail_on_multiple: error
- `resolve_single_node` multiple + all_matches: returns all
- Existing `search_nodes_*` tests must still pass

---

## #112: `--summarize-tables` for Detail

### Problem (confirmed)

`detail <pkg> -d 1` shows only `ContainsRoutine` edges (correct).
`detail <pkg> -d 2` shows every proc→table path with noise.
No quick way to answer "what tables does this package read/write?"

### Design

**New flag on `Detail` arg struct (main.rs:435-462):**

```rust
#[arg(long)]
summarize_tables: bool,
```

**When `summarize_tables` is true and the target node is a Package:**

1. Find all child procedures via `ContainsRoutine` edges
2. For each child proc, collect outgoing `TableAccess` edges
3. Group by table name, merging `modes` (OR) and `write_kinds` (union)
4. Print summary:

```
Table Access Summary for pkg:public.pkg_order_mgmt
  READ (2):  orders, customers
  WRITE (3):
    insert:           orders
    insert_select:    order_archive
    update:           customers
  READ+WRITE (1): audit_log
```

**Reuse existing logic:**
- `Edge::TableAccess { modes, write_kinds, .. }` already has the data
- Builder's mode-merge pattern (builder.rs:3046-3073) ORs modes, unions write_kinds
- `edge_label_for` (traverse.rs:65-110) decodes modes to human-readable text

**Files changed:**
1. `src/main.rs` — add `summarize_tables` to Detail + implement in `detail_one`
2. `src/graph/store.rs` or new helper — `get_package_table_summary()` aggregator

### Regression Tests

- Integration test: minimal fixture with package + 2 procs accessing tables with different modes
- Verify `detail --summarize-tables` output format
- Existing `detail` tests must still work (new flag is opt-in)

---

## Phase 0: Regression Tests (written FIRST)

1. Unit tests for `search_nodes_with_mode` in `src/graph/store.rs`
2. Unit tests for `resolve_single_node` in `src/graph/store.rs`
3. Integration test for `detail --summarize-tables` in `tests/`

## Phase 1: #113 Implementation

1. Add `regex` to `Cargo.toml`
2. Add `MatchMode`, `search_nodes_with_mode()`, `resolve_single_node()` in store.rs
3. Update 4 CLI arg structs + handler functions
4. Update `process_mark` in mark.rs

## Phase 2: #112 Implementation

1. Add `summarize_tables` flag to Detail
2. Implement package table aggregation in `detail_one`
3. Add integration test

## Phase 3: Verification

```sh
cargo build --features full
cargo test --features full
cargo clippy --features full -- -D warnings
cargo fmt -- --check
```
