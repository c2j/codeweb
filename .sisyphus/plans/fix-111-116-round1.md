# Plan: Fix Issues #111 & #116 (Round 1)

## Scope

Three independent, low-risk changes targeting round 1 of `fix-issue-from-111-to-116` branch:

| ID | Change | Files touched | LOC ~ |
|---|---|---|---|
| #111 | JSON `_meta.type_mapping` + docs type mapping table | 2 | +30 |
| #116a | `search_nodes` binary search fast path for exact/prefix | 1 | +15 |
| #116b | NDJSON export format (`--format ndjson`) | 5 | +60 |

**Out of scope (for later rounds):** `--filter` subgraph export, n-gram substring index, `--limit` early-stop.

---

## Phase 0: Regression Tests (written FIRST, verified FAIL before changes)

### 0.1 Unit test: `search_nodes` correctness (`src/graph/store.rs#tests`)

Build a `GraphStore` with 5 known nodes:
- `"public.pkg_a.proc_alpha"` (Procedure, in package pkg_a)
- `"public.pkg_a.proc_alpha_beta"` (Procedure, in package pkg_a) — substring-ambiguous with above
- `"public.pkg_b.func_gamma"` (Function, in package pkg_b)
- `"orders"` (Table)
- `"some_irrelevant_name"` (Procedure)

Test cases:
1. `search_nodes("proc_alpha")` → returns both `proc_alpha` and `proc_alpha_beta`, ranked Exact first
2. `search_nodes("pkg_a")` → returns both pkg_a procedures (WordBoundary match on package portion)
3. `search_nodes("orders")` → returns orders table
4. `search_nodes("nonexistent")` → returns empty vec
5. `search_nodes("func_gamma")` → returns func_gamma
6. `search_nodes("Gamma")` → case-insensitive returns func_gamma

**Pass condition:** These tests MUST pass identically before AND after binary search optimization.

### 0.2 Integration test: JSON `_meta.type_mapping` (`tests/regress_json_type_mapping.rs`)

Create minimal fixture SQL file in `tests/fixtures/type_mapping_regress/` with:
- 1 procedure, 1 function, 1 package, 1 table
- Run `codeweb <fixture_dir> --format json`
- Assert `json["_meta"]["type_mapping"]` exists and contains expected keys:
  - `"proc"` → `"procedure"`
  - `"pkg"` → `"package"`
  - `"func"` → `"function"`
  - `"table"` → `"table"`
  - `"mapper"` → `"mapped_statement"`
  - `"sql"` → `"java_sql"`
  - `"method"` → `"java_method"`
  - `"class"` → `"java_class"`
  - `"mview"` → `"materialized_view"`
  - `"builtin"` → `"builtin_function"`
  - `"trigger"` → `"trigger"`
  - `"type"` → `"type"`
  - `"seq"` → `"sequence"`
  - `"index"` → `"index"`
  - `"synonym"` → `"synonym"`
  - `"event"` → `"event"`
  - `"unres"` → `"unresolved"`
- Assert existing `nodes[].type` values unchanged (regression guard: `procedure` stays `procedure`, not `proc`)

### 0.3 Integration test: NDJSON export (`tests/regress_ndjson_export.rs`)

Reuse the fixture from 0.2:
- Run `codeweb <fixture_dir> --format ndjson`
- Assert each line parses as valid JSON
- Collect all `{"type":"node",...}` lines → count matches fixture node count
- Collect all `{"type":"edge",...}` lines → count matches fixture edge count
- Assert NO line is empty or non-JSON
- Verify each node has `type_tag` field with CLI short name
- Verify each node has `type` field with JSON long name

### 0.4 Regression: `regress_node_format.rs` still passes unchanged

All existing format-key tests (`format_procedure_keys`, `format_table_keys`, etc.) must pass unchanged — they verify JSON `type` field names remain as-is (e.g. `"procedure"`, not `"proc"`).

---

## Phase 1: #111 — Type Mapping

### 1.1 Add `_meta.type_mapping` to JSON export (`src/export/json.rs`)

- Add `TypeMapping` struct to `GraphJson`:
  ```rust
  #[derive(Serialize)]
  struct GraphJson {
      #[serde(rename = "_meta")]
      meta: GraphMeta,
      nodes: Vec<NodeJson>,
      edges: Vec<EdgeJson>,
  }

  #[derive(Serialize)]
  struct GraphMeta {
      type_mapping: std::collections::BTreeMap<&'static str, &'static str>,
  }
  ```
- Fill mapping with all 17 CLI-tag → JSON-type pairs (from `node_type_tag()` → `NodeKindJson` serde names).
- **Existing `nodes` and `edges` arrays remain at top level unchanged.**

### 1.2 Add type mapping table to docs (`docs/user-guide.md`)

Add a table "CLI/JSON 类型标签对照表" under the existing node types table, listing all 17 mappings. Use the existing node types table as a reference.

---

## Phase 2: #116a — Binary Search in `search_nodes` (`src/graph/store.rs`)

### 2.1 Add fast path for exact/prefix queries

In `search_nodes()` (L745), before the linear `for` loop:

```rust
// Fast path: if query looks like an exact or prefix match
// (no wildcards, no path separators that would benefit from substring),
// use binary search on the sorted name_index.
if is_simple_query(&lower) {
    // binary_search_by_key for exact match
    if let Ok(pos) = self.name_index.binary_search_by_key(&lower, |(k, _)| k.as_str()) {
        // Found exact match — collect all adjacent duplicates
        // (multiple nodes can share same lowercase key, e.g. same name different schemas)
        let mut results = Vec::new();
        // Scan left and right from pos for same key
        // ...
        return results;
    }
    // For prefix match: partition_point to find range
    // For exact match on key portion (e.g. "proc:public.do_work"):
    // try binary_search on full key, then fall through to substring
}
// Fall through to existing linear substring scan
```

`is_simple_query(lower: &str) -> bool`: return true if query contains no `*`, no `?` (simple wildcards). Regex would override this fast path later.

**Key invariants to preserve:**
- Results MUST be identical to current linear scan for any valid query
- Ranking order (Exact > WordBoundary > Substring) preserved
- Tie-breaking by degree preserved
- JspPage/JspSql/JavaSql fallback paths preserved

### 2.2 Performance benchmark

- Run `cargo test --features full` to verify correctness
- Manual benchmark on AAS project data (30K nodes) before/after for `nodes -s` query time

---

## Phase 3: #116b — NDJSON Export

### 3.1 Create `src/export/ndjson.rs`

```rust
use std::io::Write;
use crate::error::Result;
use crate::graph::CodeGraph;
use crate::graph::node_type_tag;  // reuse for type_tag field

pub fn to_ndjson(graph: &CodeGraph, writer: &mut impl Write) -> Result<()> {
    // For each node: write {"type":"node","id":0,"type_tag":"proc","type":"procedure",...}
    // For each edge: write {"type":"edge","source":0,"target":1,"edge_type":"direct",...}
    // Each record on its own line (no trailing comma, no wrapping array)
}
```

**NDJSON record format** (one JSON object per line):
```json
{"record":"node","id":0,"type_tag":"proc","type":"procedure","name":"do_work","schema":"public","file":"a.sql","line":1}
{"record":"node","id":1,"type_tag":"table","type":"table","name":"orders","schema":"public","file":"b.sql","line":3}
{"record":"edge","source":0,"target":1,"type":"table_access","modes":"R","flow":"dml"}
```

- Each NDJSON line is self-contained (no cross-line dependencies)
- Uses `type_tag` for CLI short name and `type` for JSON long name (solves #111 ambiguity per-record)
- Reuses `NodeKey` Display format for node identifiers

### 3.2 Register `ndjson` format in CLI

Add `"ndjson"` to format value_parser lists in **2 places**:
- `src/main.rs:190` (legacy CLI `--format`)
- `src/main.rs:239` (export subcommand `--format`)

### 3.3 Wire into `cmd_export` dispatch (`src/main.rs:912-925`)

Add match arm:
```rust
"ndjson" => {
    let mut writer: Box<dyn std::io::Write> = /* file or stdout */;
    export::ndjson::to_ndjson(store.graph(), &mut writer)?;
}
```

### 3.4 Register in `src/export/mod.rs`

```rust
pub mod ndjson;
```

### 3.5 Update HTTP export endpoint (`src/server/handlers.rs:598-606`)

Add `"ndjson"` match arm returning `Content-Type: application/x-ndjson`.

---

## Phase 4: Verification

Per AGENTS.md Definition of Done:

```sh
# Build all feature combinations
cargo build --features full

# Run all tests
cargo test --features full

# Lint
cargo clippy --features full -- -D warnings

# Format
cargo fmt -- --check
```

**Expected new test file count:** 4
- `tests/regress_json_type_mapping.rs`
- `tests/regress_ndjson_export.rs`
- `tests/fixtures/type_mapping_regress/` (fixture SQL)
- `src/graph/store.rs` test additions (5-6 test functions)

---

## Risks & Mitigations

| Risk | Mitigation |
|---|---|
| Binary search changes result ordering | Unit tests (Phase 0.1) assert exact results pre and post change |
| `_meta.type_mapping` breaks downstream JSON consumers | Added as new field, not modifying existing `nodes`/`edges` structure |
| NDJSON format is underspecified | Follow [NDJSON spec](http://ndjson.org/): utf-8, `\n` delimited, each line valid JSON |
| Format dispatch duplication grows | Accept for now; refactor into `src/export/mod.rs` factory in a follow-up PR |

## Dependencies

- Phases 1, 2, 3 are **fully independent** — can be worked in parallel
- Phase 0 tests must be written first and verified to FAIL before implementations
- Phase 4 (verification) gates merge
