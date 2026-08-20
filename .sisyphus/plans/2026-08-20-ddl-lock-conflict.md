# DDL Lock-Level Tags + Cross-Procedure Conflict Detection

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.
>
> **Revision 3:** Momus REJECT on r2 — (1) this crate has **no lib target** (`src/lib.rs` does not exist); all unit tests run as `--bin codeweb`. Never use `--lib`. (2) rustc test filters are **substring**, not regex — no `|`. New extractor tests share the `ddl_lock_` prefix. (3) JSON edges are `#[serde(flatten)]` + `#[serde(tag = "type")]`; assert `e["type"] == "table_access"` and `e["modes"]` / `e["write_kinds"]` at the edge root (see `tests/regress_issue_140_subquery_reads.rs`).

**Goal:** Tag table-access edges with orthogonal `D` (DDL?) + `L1–L8` (openGauss lock level), and add `codeweb conflicts` to report cross-procedure lock conflicts on the same table.

**Architecture:** Reuse `AccessMode: u8` as the 8-level lock bitmask (existing 4 bits already *are* L1/L2/L3/L8; fill the high 4 bits). Derive the `D` tag from `WriteKind` (no extra storage). Extractor match-arms classify DDL/`LOCK TABLE` into bits + write-kinds. A pure-function 8×8 conflict matrix in `src/graph/conflict.rs` drives a new CLI. No new `Edge::TableAccess` fields; no `STORE_VERSION` bump.

**Tech Stack:** Rust, `bitflags`, `ogsql-parser` AST (`Statement::{AlterTable,Drop,CreateIndex,Lock,Vacuum,Analyze,Reindex,Cluster,Truncate}`), clap, serde JSON.

**Issue:** c2j/codeweb#144. Blocked-by #143/#320 is **downgraded**: `#148` already re-parses `PlStatement::Sql` via `parse_with_text()`, so inline DDL AST is reachable today. Phase 0 verifies that. Do **not** wait on ogsql-parser#320. Skip Phase 3 (MCP/HTTP) — YAGNI.

**QA convention (binary crate):**
- Unit tests in `src/**` → `cargo test --bin codeweb <substring> -- --nocapture`
- Integration tests in `tests/` → `cargo test --test <file_stem> -- --nocapture`
- Filter is a **substring**. Do not pass `|`, do not use `--lib`.
- Expected success: exit code 0, `0 failed`.

---

## Locked decisions (user-approved 2026-08-20)

1. **Lx display:** internal bitmask set for conflict checks; **render the highest level only** (`L8`, not `L1,L8`).
2. **R/W vs Lx:** keep existing `R` / `W:insert` / `lock` labels. Emit `Lx` **only when highest ≥ L4 or the edge has a DDL write-kind**.
3. **Rename:** `AccessMode::Truncate` → `AccessMode::AccessExclusive` (same `0b1000` bit). No alias. The word `truncate` comes only from `WriteKind::Truncate`.
4. **Out of scope:** MCP tool, HTTP endpoint, severity config file, transaction modeling, partition-granularity locks, waiting on #320.

### Bit map (do not invent a 9th bit)

| Level | openGauss | Statements | bit | constant |
|---|---|---|---|---|
| L1 | AccessShare | SELECT | `0b0001` | `Read` (existing) |
| L2 | RowShare | SELECT FOR UPDATE/SHARE | `0b0100` | `LockRead` (existing) |
| L3 | RowExclusive | INSERT/UPDATE/DELETE/MERGE | `0b0010` | `Write` (existing) |
| L4 | ShareUpdateExclusive | ANALYZE / VACUUM(non-FULL) / CREATE INDEX CONCURRENTLY | `0b0001_0000` | `ShareUpdateExclusive` |
| L5 | Share | CREATE INDEX (non-concurrent) | `0b0010_0000` | `Share` |
| L6 | ShareRowExclusive | LOCK TABLE … SHARE ROW EXCLUSIVE | `0b0100_0000` | `ShareRowExclusive` |
| L7 | Exclusive | LOCK TABLE … EXCLUSIVE | `0b1000_0000` | `Exclusive` |
| L8 | AccessExclusive | ALTER/DROP/TRUNCATE/REINDEX/CLUSTER/VACUUM FULL / LOCK default | `0b1000` | `AccessExclusive` (renamed from `Truncate`) |

### Label examples (after change)

```
[R]                         SELECT
[R,lock]                    SELECT FOR UPDATE
[W:insert]                  INSERT
[L8,D:truncate]             TRUNCATE
[L8,D:alter]                ALTER TABLE … RENAME
[L5,D:create_index]         CREATE INDEX
[L4,D:create_index]         CREATE INDEX CONCURRENTLY
[L6]                        LOCK TABLE IN SHARE ROW EXCLUSIVE   ← no D
[L8]                        LOCK TABLE (default AE)             ← no D
[R,W:insert,L8,D:alter]     mixed DML + DDL on same table
```

`W:` lists **DML** write-kinds only. `D:` lists **DDL** write-kinds only. Ops (`LockTable`, `Vacuum`, `Analyze`, `VacuumFull`) appear in neither — only as `Lx`.

### Conflict matrix (openGauss official; `X` = conflict)

Index 0..7 = L1..L8. Copy exactly:

```
         L1 L2 L3 L4 L5 L6 L7 L8
L1       -  -  -  -  -  -  -  X
L2       -  -  -  -  -  -  X  X
L3       -  -  -  -  X  X  X  X
L4       -  -  -  X  X  X  X  X
L5       -  -  X  X  -  X  X  X
L6       -  -  X  X  X  X  X  X
L7       -  X  X  X  X  X  X  X
L8       X  X  X  X  X  X  X  X
```

Severity: either side has L8 → `high`; any other matrix hit → `medium`; no hit → do not report. Default CLI filter: `high`.

### ALTER TABLE lock-level approximation

ogsql-parser `AlterColumnAction` has no `SET STATISTICS` variant. **All `ALTER TABLE` → L8.** Document in `conflict.rs` module docs. Conservative, not a follow-up in this PR.

---

## Task 1: Phase 0 — inline TRUNCATE regression

**Files:**
- Create: `tests/regress_issue_144_ddl_locks.rs`
- (no production code unless the test fails)

This is a **regression of existing behavior** (`#148` fallback). If it passes immediately, that is the correct outcome — keep the test.

**Step 1: Write the test**

```rust
use std::fs;
use tempfile::TempDir;

fn run_codeweb(args: &[&str]) -> std::process::Output {
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) { "codeweb.exe" } else { "codeweb" };
    let entries = std::fs::read_dir(&base).unwrap_or_else(|_| panic!("no target dir"));
    for entry in entries.flatten() {
        let p = entry.path().join("debug").join(bin_name);
        if p.exists() {
            return std::process::Command::new(p).args(args).output().expect("run");
        }
    }
    std::process::Command::new(base.join("debug").join(bin_name))
        .args(args)
        .output()
        .expect("run")
}

fn analyze_json(sql: &str) -> serde_json::Value {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("test.sql"), sql).unwrap();
    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).unwrap()
}

fn node_id_by_name(json: &serde_json::Value, name: &str) -> Option<usize> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"].as_str() == Some(name))
        .and_then(|n| n["id"].as_u64())
        .map(|id| id as usize)
}

/// Real JSON shape (`src/export/json.rs`): `EdgeJson` flattens `EdgeKindJson`
/// which is `#[serde(tag = "type")]`. A table-access edge looks like:
/// `{ "source": 0, "target": 1, "type": "table_access", "flow_kind": "dml",
///    "modes": ["truncate"], "write_kinds": ["truncate"], "file": "...", "line": 1 }`
/// Same as `tests/regress_issue_140_subquery_reads.rs`.
fn has_truncate_table_access(json: &serde_json::Value, source: &str, target: &str) -> bool {
    let (Some(src_id), Some(dst_id)) =
        (node_id_by_name(json, source), node_id_by_name(json, target))
    else {
        return false;
    };
    json["edges"].as_array().unwrap().iter().any(|e| {
        e["source"].as_u64() == Some(src_id as u64)
            && e["target"].as_u64() == Some(dst_id as u64)
            && e["type"].as_str() == Some("table_access")
            && e["write_kinds"]
                .as_array()
                .map(|a| a.iter().any(|s| s.as_str() == Some("truncate")))
                .unwrap_or(false)
            && e["modes"].as_array().map(|a| {
                a.iter().any(|s| {
                    s.as_str() == Some("truncate") || s.as_str() == Some("access_exclusive")
                })
            }).unwrap_or(false)
    })
}

#[test]
fn inline_truncate_in_procedure_body_emits_table_access() {
    let json = analyze_json(
        r#"
        CREATE TABLE t_log (id int);
        CREATE OR REPLACE PROCEDURE p_clean IS
        BEGIN
            TRUNCATE TABLE t_log;
        END;
        /
        "#,
    );
    assert!(
        has_truncate_table_access(&json, "p_clean", "t_log"),
        "expected p_clean → t_log truncate TableAccess, edges: {:?}",
        json["edges"]
    );
}

#[test]
fn execute_immediate_truncate_literal_still_works() {
    let json = analyze_json(
        r#"
        CREATE TABLE t_log (id int);
        CREATE OR REPLACE PROCEDURE p_clean2 IS
        BEGIN
            EXECUTE IMMEDIATE 'TRUNCATE TABLE t_log';
        END;
        /
        "#,
    );
    assert!(
        has_truncate_table_access(&json, "p_clean2", "t_log"),
        "EXECUTE IMMEDIATE truncate path regressed, edges: {:?}",
        json["edges"]
    );
}
```

**Step 2: Run**

```
cargo test --test regress_issue_144_ddl_locks -- --nocapture
```

Expected:
- If **PASS**: `#148` already unblocked inline TRUNCATE. Keep tests. Note in the commit that #143 is likely fixed and can be closed after this PR lands the regression.
- If **FAIL**: stop. Inspect `procedure_body_infos_with_fallback` (`src/graph/builder.rs:2770`) and `PlStatement::Sql` handling. Fix the fallback (not the extractor Truncate arm — that already exists) before continuing. Do not start Task 2 until this test is green.

**Step 3: Commit**

```
git add tests/regress_issue_144_ddl_locks.rs
git commit -m "test: regress inline and EXECUTE IMMEDIATE TRUNCATE table-access (#143/#144)"
```

---

## Task 2: Rename Truncate bit + add L4–L7

**Files:**
- Modify: `src/graph/mod.rs:62-71` (bitflags), `:159-188` (`access_mode_label` — **do not change label logic yet**, only the bit name so it still compiles), `:801-808` (tests)
- Modify every `AccessMode::Truncate` site to `AccessMode::AccessExclusive` (6 files; grep to confirm):
  - `src/graph/mod.rs`
  - `src/parser/extractor.rs`
  - `src/export/dot.rs`
  - `src/export/json.rs`
  - `src/export/mermaid.rs`
  - `src/import/parser.rs`

**Step 1: Write failing tests in `src/graph/mod.rs` `tests` module**

```rust
#[test]
fn access_mode_eight_lock_levels_are_distinct() {
    let bits = [
        AccessMode::Read,                  // L1
        AccessMode::LockRead,              // L2
        AccessMode::Write,                 // L3
        AccessMode::ShareUpdateExclusive,  // L4
        AccessMode::Share,                 // L5
        AccessMode::ShareRowExclusive,     // L6
        AccessMode::Exclusive,             // L7
        AccessMode::AccessExclusive,       // L8
    ];
    for (i, a) in bits.iter().enumerate() {
        for (j, b) in bits.iter().enumerate() {
            if i == j {
                assert_eq!(*a, *b);
            } else {
                assert!(!a.intersects(*b), "{i} must not overlap {j}");
            }
        }
    }
    assert_eq!(AccessMode::AccessExclusive.bits(), 0b1000);
    assert_eq!(AccessMode::ShareUpdateExclusive.bits(), 0b0001_0000);
    assert_eq!(AccessMode::Share.bits(), 0b0010_0000);
    assert_eq!(AccessMode::ShareRowExclusive.bits(), 0b0100_0000);
    assert_eq!(AccessMode::Exclusive.bits(), 0b1000_0000);
}

#[test]
fn access_exclusive_u8_roundtrip_preserves_high_bits() {
    let modes = AccessMode::Read | AccessMode::Share | AccessMode::AccessExclusive;
    let json = serde_json::to_string(&modes).unwrap();
    let back: AccessMode = serde_json::from_str(&json).unwrap();
    assert_eq!(modes, back);
}
```

Also update `access_mode_bitflags_or` to use `AccessExclusive` instead of `Truncate`.

**Step 2: Run tests — expect compile failure** (`ShareUpdateExclusive` not found, `Truncate` gone).

**Step 3: Minimal implementation**

Replace the bitflags block:

```rust
bitflags! {
    /// Access mode for table references. Bits map 1:1 onto openGauss
    /// table-lock levels L1–L8 (see `graph::conflict`).
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct AccessMode: u8 {
        const Read                   = 0b0000_0001; // L1 AccessShare
        const Write                  = 0b0000_0010; // L3 RowExclusive
        const LockRead               = 0b0000_0100; // L2 RowShare
        const AccessExclusive        = 0b0000_1000; // L8 (was Truncate)
        const ShareUpdateExclusive   = 0b0001_0000; // L4
        const Share                  = 0b0010_0000; // L5
        const ShareRowExclusive      = 0b0100_0000; // L6
        const Exclusive              = 0b1000_0000; // L7
    }
}
```

Grep-replace `AccessMode::Truncate` → `AccessMode::AccessExclusive` in the 6 files. In `import/parser.rs` keep accepting CGEF string `"truncate"` **and** `"access_exclusive"` as the L8 bit (CGEF string compat, not a Rust alias). In `export/json.rs` emit `"access_exclusive"` for the L8 bit (not `"truncate"`).

JSON mode table becomes:

```rust
[
    (AccessMode::Read, "read"),
    (AccessMode::Write, "write"),
    (AccessMode::LockRead, "lock_read"),
    (AccessMode::AccessExclusive, "access_exclusive"),
    (AccessMode::ShareUpdateExclusive, "share_update_exclusive"),
    (AccessMode::Share, "share"),
    (AccessMode::ShareRowExclusive, "share_row_exclusive"),
    (AccessMode::Exclusive, "exclusive"),
]
```

**Step 4: Run tests (two separate commands)**

```
cargo test --bin codeweb access_mode -- --nocapture
```

Expected: PASS. Tests `access_mode_eight_lock_levels_are_distinct`, `access_exclusive_u8_roundtrip_preserves_high_bits`, and `access_mode_bitflags_or` all ok. Exit code 0.

```
cargo test --bin codeweb truncate_table -- --nocapture
```

Expected: PASS. Existing extractor `truncate_table` still compiles against `AccessMode::AccessExclusive`. Exit code 0.

**Step 5: Commit**

```
git commit -m "refactor: rename AccessMode::Truncate to AccessExclusive; add L4–L7 bits"
```

---

## Task 3: WriteKind DDL/ops variants + label helpers

**Files:**
- Modify: `src/graph/mod.rs` `WriteKind` (`:126-138`), `write_kind_label` (`:142-154`), `access_mode_label` (`:159-188`)
- Modify: `src/import/parser.rs` `parse_write_kinds` (`:775-799`)
- Modify: `src/graph/lineage.rs` `produces_columns` (`:231-246`) — new kinds must **not** be treated as column-producing (fall through as false, like Truncate today)

**Step 1: Failing tests in `src/graph/mod.rs`**

```rust
#[test]
fn write_kind_label_covers_ddl_and_ops() {
    assert_eq!(write_kind_label(&WriteKind::AlterTable), "alter");
    assert_eq!(write_kind_label(&WriteKind::DropTable), "drop");
    assert_eq!(write_kind_label(&WriteKind::CreateIndex), "create_index");
    assert_eq!(write_kind_label(&WriteKind::CreateIndexConcurrent), "create_index_concurrent");
    assert_eq!(write_kind_label(&WriteKind::LockTable), "lock_table");
    assert_eq!(write_kind_label(&WriteKind::Reindex), "reindex");
    assert_eq!(write_kind_label(&WriteKind::Vacuum), "vacuum");
    assert_eq!(write_kind_label(&WriteKind::VacuumFull), "vacuum_full");
    assert_eq!(write_kind_label(&WriteKind::Analyze), "analyze");
    assert_eq!(write_kind_label(&WriteKind::Cluster), "cluster");
    assert_eq!(write_kind_label(&WriteKind::Truncate), "truncate");
}

#[test]
fn ddl_write_kind_predicate() {
    assert!(is_ddl_write_kind(&WriteKind::Truncate));
    assert!(is_ddl_write_kind(&WriteKind::AlterTable));
    assert!(is_ddl_write_kind(&WriteKind::DropTable));
    assert!(is_ddl_write_kind(&WriteKind::CreateIndex));
    assert!(is_ddl_write_kind(&WriteKind::CreateIndexConcurrent));
    assert!(is_ddl_write_kind(&WriteKind::Reindex));
    assert!(is_ddl_write_kind(&WriteKind::Cluster));
    assert!(!is_ddl_write_kind(&WriteKind::LockTable));
    assert!(!is_ddl_write_kind(&WriteKind::Vacuum));
    assert!(!is_ddl_write_kind(&WriteKind::Analyze));
    assert!(!is_ddl_write_kind(&WriteKind::VacuumFull));
    assert!(!is_ddl_write_kind(&WriteKind::Insert));
}

#[test]
fn access_mode_label_keeps_rw_hides_l1_l2_l3() {
    let empty = HashSet::new();
    assert_eq!(
        access_mode_label(AccessMode::Read, &empty).as_deref(),
        Some("R")
    );
    let mut wk = HashSet::new();
    wk.insert(WriteKind::Insert);
    assert_eq!(
        access_mode_label(AccessMode::Write, &wk).as_deref(),
        Some("W:insert")
    );
}

#[test]
fn access_mode_label_emits_l8_and_d_for_truncate() {
    let mut wk = HashSet::new();
    wk.insert(WriteKind::Truncate);
    assert_eq!(
        access_mode_label(AccessMode::AccessExclusive, &wk).as_deref(),
        Some("L8,D:truncate")
    );
}

#[test]
fn access_mode_label_lock_table_l6_has_no_d() {
    let mut wk = HashSet::new();
    wk.insert(WriteKind::LockTable);
    assert_eq!(
        access_mode_label(AccessMode::ShareRowExclusive, &wk).as_deref(),
        Some("L6")
    );
}

#[test]
fn access_mode_label_mixed_dml_ddl_highest_only() {
    let mut wk = HashSet::new();
    wk.insert(WriteKind::Insert);
    wk.insert(WriteKind::AlterTable);
    let modes = AccessMode::Read | AccessMode::Write | AccessMode::AccessExclusive;
    assert_eq!(
        access_mode_label(modes, &wk).as_deref(),
        Some("R,W:insert,L8,D:alter")
    );
}

#[test]
fn highest_lock_level_prefers_l8_over_l1() {
    let modes = AccessMode::Read | AccessMode::AccessExclusive;
    assert_eq!(highest_lock_level(modes), Some(8));
    assert_eq!(highest_lock_level(AccessMode::Write), Some(3));
    assert_eq!(highest_lock_level(AccessMode::empty()), None);
}
```

**Step 2: Run — FAIL** (missing variants / helpers).

**Step 3: Implementation**

Append to `WriteKind` (order matters for serde discriminant of **new** variants only; never reorder existing ones):

```rust
pub enum WriteKind {
    Insert,
    InsertSelect,
    Update,
    Delete,
    MergeInsert,
    MergeUpdate,
    MergeDelete,
    SelectInto,
    Truncate,
    // --- added for #144; append-only ---
    AlterTable,
    DropTable,
    CreateIndex,
    CreateIndexConcurrent,
    LockTable,
    Reindex,
    Vacuum,
    VacuumFull,
    Analyze,
    Cluster,
}
```

```rust
pub fn is_ddl_write_kind(kind: &WriteKind) -> bool {
    matches!(
        kind,
        WriteKind::Truncate
            | WriteKind::AlterTable
            | WriteKind::DropTable
            | WriteKind::CreateIndex
            | WriteKind::CreateIndexConcurrent
            | WriteKind::Reindex
            | WriteKind::Cluster
    )
}

/// Highest openGauss lock level present in `modes` (1–8).
pub fn highest_lock_level(modes: AccessMode) -> Option<u8> {
    if modes.contains(AccessMode::AccessExclusive) { Some(8) }
    else if modes.contains(AccessMode::Exclusive) { Some(7) }
    else if modes.contains(AccessMode::ShareRowExclusive) { Some(6) }
    else if modes.contains(AccessMode::Share) { Some(5) }
    else if modes.contains(AccessMode::ShareUpdateExclusive) { Some(4) }
    else if modes.contains(AccessMode::Write) { Some(3) }
    else if modes.contains(AccessMode::LockRead) { Some(2) }
    else if modes.contains(AccessMode::Read) { Some(1) }
    else { None }
}

pub fn access_mode_label(
    modes: AccessMode,
    write_kinds: &std::collections::HashSet<WriteKind>,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if modes.contains(AccessMode::Read) {
        parts.push("R".to_string());
    }
    if modes.contains(AccessMode::Write) {
        let mut wk: Vec<&str> = write_kinds
            .iter()
            .filter(|k| !is_ddl_write_kind(k) && !matches!(k,
                WriteKind::LockTable | WriteKind::Vacuum | WriteKind::VacuumFull | WriteKind::Analyze
            ))
            .map(write_kind_label)
            .collect();
        wk.sort_unstable();
        if wk.is_empty() {
            parts.push("W".to_string());
        } else {
            parts.push(format!("W:{}", wk.join(",")));
        }
    }
    if modes.contains(AccessMode::LockRead) {
        parts.push("lock".to_string());
    }
    // Lx only when highest ≥ 4 OR any DDL write-kind is present.
    let highest = highest_lock_level(modes);
    let has_ddl = write_kinds.iter().any(is_ddl_write_kind);
    if let Some(level) = highest {
        if level >= 4 || has_ddl {
            parts.push(format!("L{level}"));
        }
    }
    if has_ddl {
        let mut d: Vec<&str> = write_kinds.iter().filter(|k| is_ddl_write_kind(k)).map(write_kind_label).collect();
        d.sort_unstable();
        d.dedup(); // CreateIndex + CreateIndexConcurrent share label "create_index"
        parts.push(format!("D:{}", d.join(",")));
    }
    if parts.is_empty() { None } else { Some(parts.join(",")) }
}
```

`write_kind_label`: add the new match arms. `CreateIndex` and `CreateIndexConcurrent` **share** `"create_index"` (lock level distinguishes them).

`parse_write_kinds`: add the new string arms (`"alter"`, `"drop"`, `"create_index"` → `CreateIndex` on import is lossy for concurrent; acceptable — CGEF does not round-trip the concurrent flag separately unless we also accept `"create_index_concurrent"`). Prefer:

```
"create_index" => CreateIndex
"create_index_concurrent" => CreateIndexConcurrent
```

and have `write_kind_label(CreateIndexConcurrent)` return `"create_index"` for **display**, but JSON export uses the same `write_kind_label`. Conflict: display wants shared label, JSON wants distinct.

**Resolve:** `write_kind_label` returns `"create_index"` / `"create_index_concurrent"` (distinct, lossless). Display `D:` **dedups by stripping `_concurrent`** OR show `D:create_index` for both via a small `ddl_display_label()`:

```rust
fn ddl_display_label(kind: &WriteKind) -> &'static str {
    match kind {
        WriteKind::CreateIndex | WriteKind::CreateIndexConcurrent => "create_index",
        other => write_kind_label(other),
    }
}
```

Use `ddl_display_label` only inside `D:` rendering. JSON `write_kinds` stay lossless.

**Step 4: Run tests (two separate commands)**

```
cargo test --bin codeweb access_mode_label -- --nocapture
```

Expected: PASS. `access_mode_label_keeps_rw_hides_l1_l2_l3`, `access_mode_label_emits_l8_and_d_for_truncate`, `access_mode_label_lock_table_l6_has_no_d`, `access_mode_label_mixed_dml_ddl_highest_only` all ok. Exit code 0.

```
cargo test --bin codeweb write_kind -- --nocapture
```

Expected: PASS. `write_kind_label_covers_ddl_and_ops`, `ddl_write_kind_predicate`, `write_kind_serialization_roundtrip` all ok. Exit code 0.

```
cargo test --bin codeweb highest_lock_level -- --nocapture
```

Expected: PASS. `highest_lock_level_prefers_l8_over_l1` ok. Exit code 0.

**Step 5: Commit**

```
git commit -m "feat: WriteKind DDL/ops variants and D+Lx label rendering"
```

---

## Task 4: Extractor match-arms

**Files:**
- Modify: `src/parser/extractor.rs` `TableAccessExtractor::visit_statement` (`:1431-1488`)
- Test: same file `tests` module after `truncate_table` (`:3889`)

Add a private helper in the extractor impl (or `graph/mod.rs` if reused by conflict — keep mapping next to the visitor):

```rust
fn lock_mode_from_lock_table(mode: &str) -> AccessMode {
    let n: String = mode
        .split_whitespace()
        .map(|w| w.to_ascii_uppercase())
        .collect::<Vec<_>>()
        .join(" ");
    match n.as_str() {
        "" | "ACCESS EXCLUSIVE" => AccessMode::AccessExclusive,
        "ACCESS SHARE" => AccessMode::Read,
        "ROW SHARE" => AccessMode::LockRead,
        "ROW EXCLUSIVE" => AccessMode::Write,
        "SHARE UPDATE EXCLUSIVE" => AccessMode::ShareUpdateExclusive,
        "SHARE" => AccessMode::Share,
        "SHARE ROW EXCLUSIVE" => AccessMode::ShareRowExclusive,
        "EXCLUSIVE" => AccessMode::Exclusive,
        _ => AccessMode::AccessExclusive, // unknown → conservative L8
    }
}
```

**Step 1: Failing tests** (use existing `extract_accesses` / `find_access` helpers):

```rust
#[test]
fn ddl_lock_alter_table_rename_is_l8() {
    let a = find_access(&extract_accesses("ALTER TABLE t_stage RENAME TO t_old"), "t_stage").unwrap();
    assert!(a.modes.contains(AccessMode::AccessExclusive));
    assert!(a.write_kinds.contains(&WriteKind::AlterTable));
}

#[test]
fn ddl_lock_drop_table_is_l8() {
    let a = find_access(&extract_accesses("DROP TABLE t_log"), "t_log").unwrap();
    assert!(a.modes.contains(AccessMode::AccessExclusive));
    assert!(a.write_kinds.contains(&WriteKind::DropTable));
}

#[test]
fn ddl_lock_drop_index_is_ignored() {
    let accesses = extract_accesses("DROP INDEX idx_t_log");
    assert!(accesses.is_empty(), "DROP INDEX is not a table access: {accesses:?}");
}

#[test]
fn ddl_lock_create_index_is_l5() {
    let a = find_access(&extract_accesses("CREATE INDEX idx ON t_log (id)"), "t_log").unwrap();
    assert!(a.modes.contains(AccessMode::Share));
    assert!(a.write_kinds.contains(&WriteKind::CreateIndex));
}

#[test]
fn ddl_lock_create_index_concurrently_is_l4() {
    let a = find_access(&extract_accesses("CREATE INDEX CONCURRENTLY idx ON t_log (id)"), "t_log").unwrap();
    assert!(a.modes.contains(AccessMode::ShareUpdateExclusive));
    assert!(a.write_kinds.contains(&WriteKind::CreateIndexConcurrent));
}

#[test]
fn ddl_lock_lock_table_share_row_exclusive_is_l6() {
    let a = find_access(&extract_accesses("LOCK TABLE t_log IN SHARE ROW EXCLUSIVE MODE"), "t_log").unwrap();
    assert!(a.modes.contains(AccessMode::ShareRowExclusive));
    assert!(a.write_kinds.contains(&WriteKind::LockTable));
    assert!(!a.write_kinds.iter().any(is_ddl_write_kind));
}

#[test]
fn ddl_lock_lock_table_default_is_l8() {
    let a = find_access(&extract_accesses("LOCK TABLE t_log"), "t_log").unwrap();
    assert!(a.modes.contains(AccessMode::AccessExclusive));
}

#[test]
fn ddl_lock_vacuum_full_is_l8_vacuum_is_l4() {
    let full = find_access(&extract_accesses("VACUUM FULL t_log"), "t_log").unwrap();
    assert!(full.modes.contains(AccessMode::AccessExclusive));
    assert!(full.write_kinds.contains(&WriteKind::VacuumFull));
    let v = find_access(&extract_accesses("VACUUM t_log"), "t_log").unwrap();
    assert!(v.modes.contains(AccessMode::ShareUpdateExclusive));
    assert!(v.write_kinds.contains(&WriteKind::Vacuum));
}

#[test]
fn ddl_lock_analyze_is_l4() {
    let a = find_access(&extract_accesses("ANALYZE t_log"), "t_log").unwrap();
    assert!(a.modes.contains(AccessMode::ShareUpdateExclusive));
    assert!(a.write_kinds.contains(&WriteKind::Analyze));
}

#[test]
fn ddl_lock_cluster_and_reindex_table_are_l8() {
    let c = find_access(&extract_accesses("CLUSTER t_log"), "t_log").unwrap();
    assert!(c.modes.contains(AccessMode::AccessExclusive));
    assert!(c.write_kinds.contains(&WriteKind::Cluster));
    let r = find_access(&extract_accesses("REINDEX TABLE t_log"), "t_log").unwrap();
    assert!(r.modes.contains(AccessMode::AccessExclusive));
    assert!(r.write_kinds.contains(&WriteKind::Reindex));
}

#[test]
fn ddl_lock_truncate_uses_access_exclusive_bit() {
    let t1 = find_access(&extract_accesses("TRUNCATE TABLE t1"), "t1").unwrap();
    assert!(t1.modes.contains(AccessMode::AccessExclusive));
    assert!(t1.write_kinds.contains(&WriteKind::Truncate));
}
```

Update the existing `truncate_table` test in the same change (it still says `AccessMode::Truncate`).

**Step 2: Run — FAIL** (empty accesses / missing bit).

**Step 3: Implementation** — extend `visit_statement` match. `walk_statement` does **not** recurse into these DDL nodes; the arm must read AST fields directly.

```rust
Statement::Truncate(truncate) => {
    for table in &truncate.tables {
        self.add_access(table, AccessMode::AccessExclusive, Some(WriteKind::Truncate));
    }
}
Statement::AlterTable(alter) => {
    self.add_access(&alter.name, AccessMode::AccessExclusive, Some(WriteKind::AlterTable));
}
Statement::Drop(drop) => {
    if matches!(drop.object_type, ogsql_parser::ast::ObjectType::Table) {
        for name in &drop.names {
            self.add_access(name, AccessMode::AccessExclusive, Some(WriteKind::DropTable));
        }
    }
}
Statement::CreateIndex(idx) => {
    let (mode, wk) = if idx.concurrent {
        (AccessMode::ShareUpdateExclusive, WriteKind::CreateIndexConcurrent)
    } else {
        (AccessMode::Share, WriteKind::CreateIndex)
    };
    self.add_access(&idx.table, mode, Some(wk));
}
Statement::CreateGlobalIndex(idx) => {
    let (mode, wk) = if idx.concurrent {
        (AccessMode::ShareUpdateExclusive, WriteKind::CreateIndexConcurrent)
    } else {
        (AccessMode::Share, WriteKind::CreateIndex)
    };
    self.add_access(&idx.table, mode, Some(wk));
}
Statement::Lock(lock) => {
    let mode = lock_mode_from_lock_table(&lock.mode);
    for table in &lock.tables {
        self.add_access(table, mode, Some(WriteKind::LockTable));
    }
}
Statement::Vacuum(vac) => {
    let (mode, wk) = if vac.full {
        (AccessMode::AccessExclusive, WriteKind::VacuumFull)
    } else {
        (AccessMode::ShareUpdateExclusive, WriteKind::Vacuum)
    };
    for t in &vac.tables {
        self.add_access(&t.name, mode, Some(wk));
    }
}
Statement::Analyze(an) => {
    for t in &an.tables {
        self.add_access(&t.name, AccessMode::ShareUpdateExclusive, Some(WriteKind::Analyze));
    }
}
Statement::Cluster(cl) => {
    if let Some(table) = &cl.table {
        self.add_access(table, AccessMode::AccessExclusive, Some(WriteKind::Cluster));
    }
}
Statement::Reindex(ri) => {
    if let ogsql_parser::ast::ReindexTarget::Table(name) = &ri.target {
        self.add_access(name, AccessMode::AccessExclusive, Some(WriteKind::Reindex));
    }
}
```

If a statement variant name differs in the pinned parser (`d88306a`), match the actual enum in `~/.cargo/git/checkouts/ogsql-parser-*/d88306a/src/ast/mod.rs` around the `Statement` enum (~line 777). Do not add parser work.

VACUUM/ANALYZE/CLUSTER with **empty** table list: no edge (correct — not table-scoped).

**Step 4: Run tests (substring prefix, not regex)**

```
cargo test --bin codeweb ddl_lock_ -- --nocapture
```

Expected: PASS. All `ddl_lock_*` tests (11) ok, 0 failed. Exit code 0.

```
cargo test --bin codeweb truncate_table -- --nocapture
```

Expected: PASS. Existing `truncate_table` still ok. Exit code 0.

If `CREATE INDEX CONCURRENTLY` / `VACUUM FULL` / `CLUSTER` / `LOCK TABLE IN … MODE` fail to parse, print `parser.parse_with_text()` errors in the test and check dialect. Fall back to the AST the parser actually produces; do not weaken assertions to `accesses.len() >= 0`.

**Step 5: Commit**

```
git commit -m "feat: extract DDL and LOCK TABLE access modes (L4–L8)"
```

---

## Task 5: Rendering / import / lineage / DOT / mermaid sync

**Files:**
- `src/export/json.rs:770-778` — already updated in Task 2; verify all 8 bits.
- `src/import/parser.rs:754-773` — all 8 mode strings; `"truncate"` still maps to `AccessExclusive`.
- `src/export/dot.rs:331` — `Write | AccessExclusive` stays red; also treat `Share`/`Exclusive`/`ShareRowExclusive`/`ShareUpdateExclusive` as red (DDL/lock-heavy). Simplest rule: **red if highest ≥ L4, else keep existing R/W/lock colors**.
- `src/export/mermaid.rs:161` — `==>` if highest ≥ L4 or Write; else `-.->`.
- `src/graph/lineage.rs:231-246` — `produces_columns` unchanged list (new kinds excluded automatically). Add a unit test that `WriteKind::AlterTable` returns false.

**Step 1:** Add a focused test for `access_mode_label` already done. Add:

```rust
// in lineage.rs tests or graph/mod.rs
#[test]
fn ddl_write_kinds_do_not_produce_columns() {
    for k in [
        WriteKind::Truncate, WriteKind::AlterTable, WriteKind::DropTable,
        WriteKind::CreateIndex, WriteKind::LockTable, WriteKind::Vacuum,
    ] {
        let mut s = HashSet::new();
        s.insert(k);
        assert!(!produces_columns(&s), "{k:?}");
    }
}
```

`produces_columns` is private in `lineage.rs` — put the test in `lineage.rs`'s `#[cfg(test)]` module.

**Step 2: Run the new lineage test (expect FAIL — test not compiled / `produces_columns` not visible, or assertion if already in module)**

```
cargo test --bin codeweb ddl_write_kinds_do_not_produce_columns -- --nocapture
```

Expected: FAIL (test missing or `produces_columns` still treats unknown kinds as column-producing if the HashSet-empty shortcut is hit incorrectly). Do not proceed until this is a real assertion failure or compile error on the new test.

**Step 3:** Add the test to `src/graph/lineage.rs` `#[cfg(test)]`. Confirm `produces_columns` does **not** list the new kinds (no code change needed if the allow-list is unchanged). Update DOT (`src/export/dot.rs:327-335`) and mermaid (`src/export/mermaid.rs:160-165`) to use `highest_lock_level(modes)`: red / `==>` when `highest >= Some(4)` or `Write`; keep existing Read/LockRead colors otherwise.

**Step 4: Re-run**

```
cargo test --bin codeweb ddl_write_kinds_do_not_produce_columns -- --nocapture
```

Expected: PASS. Exit code 0.

```
cargo test --bin codeweb access_mode_label -- --nocapture
```

Expected: PASS. Exit code 0.

```
cargo build
```

Expected: exit code 0. DOT/mermaid/json/import compile with the renamed bit and `highest_lock_level`.

**Step 5: Commit**

```
git commit -m "feat: export/import/DOT/mermaid lock-level rendering"
```

---

## Task 6: Conflict matrix pure functions

**Files:**
- Create: `src/graph/conflict.rs`
- Modify: `src/graph/mod.rs` add `pub mod conflict;`

**Step 1: Write tests in `conflict.rs`**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::AccessMode;

    #[test]
    fn l8_conflicts_with_everything_including_l1() {
        assert_eq!(
            conflict_severity(AccessMode::AccessExclusive, AccessMode::Read),
            Some(ConflictSeverity::High)
        );
        assert_eq!(
            conflict_severity(AccessMode::Read, AccessMode::AccessExclusive),
            Some(ConflictSeverity::High)
        );
    }

    #[test]
    fn two_inserts_do_not_conflict() {
        assert_eq!(
            conflict_severity(AccessMode::Write, AccessMode::Write),
            None
        );
    }

    #[test]
    fn create_index_l5_vs_insert_l3_is_medium() {
        assert_eq!(
            conflict_severity(AccessMode::Share, AccessMode::Write),
            Some(ConflictSeverity::Medium)
        );
    }

    #[test]
    fn two_selects_do_not_conflict() {
        assert_eq!(conflict_severity(AccessMode::Read, AccessMode::Read), None);
    }

    #[test]
    fn l7_conflicts_with_l2_but_l6_does_not() {
        assert_eq!(
            conflict_severity(AccessMode::Exclusive, AccessMode::LockRead),
            Some(ConflictSeverity::Medium)
        );
        assert_eq!(
            conflict_severity(AccessMode::ShareRowExclusive, AccessMode::LockRead),
            None
        );
    }

    #[test]
    fn mixed_bits_use_any_pair() {
        let a = AccessMode::Read | AccessMode::AccessExclusive;
        assert_eq!(
            conflict_severity(a, AccessMode::Read),
            Some(ConflictSeverity::High)
        );
    }

    #[test]
    fn vacuum_l4_self_conflicts_medium() {
        assert_eq!(
            conflict_severity(AccessMode::ShareUpdateExclusive, AccessMode::ShareUpdateExclusive),
            Some(ConflictSeverity::Medium)
        );
    }
}
```

**Step 2: FAIL** (module missing).

**Step 3: Implementation**

```rust
//! Cross-procedure table-lock conflict detection (openGauss 8-level matrix).
//!
//! Static analysis: "would conflict IF executed concurrently". No transaction
//! model. Default reporters should filter to High.

use crate::graph::{highest_lock_level, AccessMode, CodeGraph, Edge, Node};
use petgraph::graph::NodeIndex;
use petgraph::visit::EdgeRef;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ConflictSeverity {
    Medium,
    High,
}

/// openGauss lock-mode conflict matrix. `true` = conflict.
/// Rows/cols 0..7 = L1..L8.
const CONFLICT: [[bool; 8]; 8] = [
    //        L1    L2    L3    L4    L5    L6    L7    L8
    /* L1 */ [false,false,false,false,false,false,false,true ],
    /* L2 */ [false,false,false,false,false,false,true, true ],
    /* L3 */ [false,false,false,false,true, true, true, true ],
    /* L4 */ [false,false,false,true, true, true, true, true ],
    /* L5 */ [false,false,true, true, false,true, true, true ],
    /* L6 */ [false,false,true, true, true, true, true, true ],
    /* L7 */ [false,true, true, true, true, true, true, true ],
    /* L8 */ [true, true, true, true, true, true, true, true ],
];

fn levels_in(modes: AccessMode) -> impl Iterator<Item = usize> {
    (1u8..=8).filter_map(move |lvl| {
        let present = match lvl {
            1 => modes.contains(AccessMode::Read),
            2 => modes.contains(AccessMode::LockRead),
            3 => modes.contains(AccessMode::Write),
            4 => modes.contains(AccessMode::ShareUpdateExclusive),
            5 => modes.contains(AccessMode::Share),
            6 => modes.contains(AccessMode::ShareRowExclusive),
            7 => modes.contains(AccessMode::Exclusive),
            8 => modes.contains(AccessMode::AccessExclusive),
            _ => false,
        };
        present.then_some((lvl - 1) as usize)
    })
}

pub fn locks_conflict(a: AccessMode, b: AccessMode) -> bool {
    for i in levels_in(a) {
        for j in levels_in(b) {
            if CONFLICT[i][j] {
                return true;
            }
        }
    }
    false
}

pub fn conflict_severity(a: AccessMode, b: AccessMode) -> Option<ConflictSeverity> {
    if !locks_conflict(a, b) {
        return None;
    }
    if a.contains(AccessMode::AccessExclusive) || b.contains(AccessMode::AccessExclusive) {
        Some(ConflictSeverity::High)
    } else {
        Some(ConflictSeverity::Medium)
    }
}
```

**Step 4: Run**

```
cargo test --bin codeweb conflict -- --nocapture
```

Expected: PASS. `l8_conflicts_with_everything_including_l1`, `two_inserts_do_not_conflict`, `create_index_l5_vs_insert_l3_is_medium`, `two_selects_do_not_conflict`, `l7_conflicts_with_l2_but_l6_does_not`, `mixed_bits_use_any_pair`, `vacuum_l4_self_conflicts_medium` all ok. Exit code 0.

**Step 5: Commit**

```
git commit -m "feat: openGauss 8-level lock conflict matrix"
```

---

## Task 7: `find_conflicts` over the graph

**Files:**
- Modify: `src/graph/conflict.rs`

Do **not** reuse `ProcTableMatrix` (it is a boolean matrix and drops modes).

**Step 1: Failing test** — build a tiny `CodeGraph` by hand (same style as `builder.rs` unit tests around line 7273):

```rust
#[test]
fn truncate_vs_select_is_high_two_inserts_are_silent() {
    use crate::graph::{DataFlowKind, Edge, Node, RoutineId, RoutineKind, SourceLocation, WriteKind};
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::path::PathBuf;

    let mut g = CodeGraph::new();
    let loc = SourceLocation { file: Arc::new(PathBuf::from("t.sql")), line: 1, column: 0 };
    let p_trunc = g.add_node(Node::Procedure {
        id: RoutineId { schema: None, package: None, name: "p_clean".into(), kind: RoutineKind::Procedure },
        // fill remaining fields with whatever the Node::Procedure variant requires —
        // copy a constructor from an existing test. If the variant is large, prefer
        // GraphBuilder::build() on two ParsedFile fixtures instead.
        .. /* see existing tests */ 
    });
    // ... too brittle if Node::Procedure has many fields.
}
```

**Prefer `GraphBuilder` + parser** over hand-built nodes. Pattern from extractor tests is not enough (no edges). Use the integration `analyze_json` helper **or** a unit-level builder:

Look up `GraphBuilder::build` (`builder.rs:174`). In `conflict.rs` tests:

```rust
fn graph_from_sql(sql: &str) -> CodeGraph {
    let tokens = ogsql_parser::Tokenizer::new(sql).tokenize().unwrap();
    let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
    let stmts = parser.parse_with_text();
    let file = crate::parser::ParsedFile {
        // copy the struct fields from parser/loader.rs / an existing builder test
    };
    crate::graph::builder::GraphBuilder::new().build(&[file])
}
```

If `ParsedFile` is awkward to construct, put the pair-wise tests in `tests/regress_issue_144_ddl_locks.rs` instead (CLI `conflicts` comes in Task 8). For Task 7, keep tests in `conflict.rs` by constructing **only edges** if `Node` is heavy: inspect `Node::Procedure` and `Node::Table` field lists first; if >8 fields, skip hand-built graphs and test `find_conflicts` via a helper that takes `Vec<(proc_name, table_name, AccessMode)>` and does not need a real `CodeGraph`:

```rust
pub struct ProcTableLock {
    pub proc: NodeIndex,
    pub table: NodeIndex,
    pub modes: AccessMode,
}

pub fn conflicts_among(locks: &[ProcTableLock]) -> Vec<LockConflict> { ... }
```

Then `find_conflicts(graph)` is a thin adapter that groups `Edge::TableAccess` with `flow_kind == DmlAccess` whose endpoints are (Procedure|Function) → (Table|View|MaterializedView).

This split is the required design: **pure grouping function + graph adapter**. Test the pure function without Node construction.

```rust
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LockConflict {
    pub table: NodeIndex,
    pub proc_a: NodeIndex,
    pub proc_b: NodeIndex,
    pub modes_a: AccessMode,
    pub modes_b: AccessMode,
    pub severity: ConflictSeverity,
}

pub fn conflicts_among(locks: &[ProcTableLock]) -> Vec<LockConflict> {
    // group by table
    // for each table, unique procs (OR modes if a proc has multiple edges)
    // pairs i<j, skip if proc_a == proc_b
    // if let Some(sev) = conflict_severity(ma, mb) { push }
    // sort by severity desc, then table index, then proc names later at CLI
}

pub fn find_conflicts(graph: &CodeGraph) -> Vec<LockConflict> {
    let mut locks = Vec::new();
    for edge in graph.edge_references() {
        if let Edge::TableAccess { flow_kind, modes, .. } = edge.weight() {
            if *flow_kind != crate::graph::DataFlowKind::DmlAccess {
                continue;
            }
            let src = edge.source();
            let dst = edge.target();
            if !matches!(graph[src], Node::Procedure { .. } | Node::Function { .. }) {
                continue;
            }
            if !matches!(graph[dst], Node::Table { .. } | Node::View { .. } | Node::MaterializedView { .. }) {
                continue;
            }
            locks.push(ProcTableLock { proc: src, table: dst, modes: *modes });
        }
    }
    conflicts_among(&locks)
}
```

When merging multiple edges from the same `(proc, table)`, **OR the modes**.

Add these tests in `conflict.rs` (NodeIndex is a `u32` newtype — use `NodeIndex::new(n)`):

```rust
#[test]
fn conflicts_among_truncate_vs_select_is_high() {
    let locks = vec![
        ProcTableLock { proc: NodeIndex::new(1), table: NodeIndex::new(10), modes: AccessMode::AccessExclusive },
        ProcTableLock { proc: NodeIndex::new(2), table: NodeIndex::new(10), modes: AccessMode::Read },
    ];
    let out = conflicts_among(&locks);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].severity, ConflictSeverity::High);
}

#[test]
fn conflicts_among_two_inserts_empty() {
    let locks = vec![
        ProcTableLock { proc: NodeIndex::new(1), table: NodeIndex::new(10), modes: AccessMode::Write },
        ProcTableLock { proc: NodeIndex::new(2), table: NodeIndex::new(10), modes: AccessMode::Write },
    ];
    assert!(conflicts_among(&locks).is_empty());
}

#[test]
fn conflicts_among_skips_same_proc() {
    let locks = vec![
        ProcTableLock { proc: NodeIndex::new(1), table: NodeIndex::new(10), modes: AccessMode::AccessExclusive },
        ProcTableLock { proc: NodeIndex::new(1), table: NodeIndex::new(10), modes: AccessMode::Read },
    ];
    assert!(conflicts_among(&locks).is_empty());
}

#[test]
fn conflicts_among_ors_modes_per_proc_table() {
    let locks = vec![
        ProcTableLock { proc: NodeIndex::new(1), table: NodeIndex::new(10), modes: AccessMode::Read },
        ProcTableLock { proc: NodeIndex::new(1), table: NodeIndex::new(10), modes: AccessMode::AccessExclusive },
        ProcTableLock { proc: NodeIndex::new(2), table: NodeIndex::new(10), modes: AccessMode::Read },
    ];
    let out = conflicts_among(&locks);
    assert_eq!(out.len(), 1);
    assert_eq!(out[0].severity, ConflictSeverity::High);
}
```

**Step 4: Run**

```
cargo test --bin codeweb conflicts_among -- --nocapture
```

Expected: PASS. Tests covering (1) L8 vs L1 → one High pair, (2) two L3 → empty vec, (3) same proc skipped, (4) modes OR-merged per (proc, table) all ok. Exit code 0.

Also run the matrix tests to ensure no regression:

```
cargo test --bin codeweb conflict -- --nocapture
```

Expected: PASS. Exit code 0.

**Step 5: Commit**

```
git commit -m "feat: group per-table procedure locks and emit conflict pairs"
```

---

## Task 8: `codeweb conflicts` CLI

**Files:**
- Modify: `src/main.rs` `Commands` enum (after `Inspect`, ~line 733)
- Modify: `src/main.rs` `match` dispatcher (~line 848)
- Modify: `tests/regress_issue_144_ddl_locks.rs` — add CLI tests

**CLI shape:**

```
codeweb conflicts [--project .] [--severity high|medium] [--format json|text] [--table NAME]
```

- `--severity` default `high`. `medium` includes High+Medium.
- `--table` optional substring filter on table display name.
- JSON schema (schema_version=1):

```json
{
  "schema_version": 1,
  "severity_filter": "high",
  "conflicts": [
    {
      "severity": "high",
      "table": "t_log",
      "proc_a": "p_clean",
      "proc_b": "p_read",
      "lock_a": "L8",
      "lock_b": "L1",
      "modes_a": ["access_exclusive"],
      "modes_b": ["read"]
    }
  ]
}
```

Text:

```
HIGH  table t_log: p_clean [L8] vs p_read [L1]
```

Header comment on the JSON struct: static analysis, not runtime certainty; no transaction model.

Load store the same way `cmd_impact` / `cmd_detail` does (read `.codeweb/store` via project dir). Reuse that helper; do not invent a second loader.

**Step 1: Failing CLI tests** in `tests/regress_issue_144_ddl_locks.rs`:

```rust
#[test]
fn conflicts_reports_truncate_vs_select_not_two_inserts() {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("t.sql"), r#"
        CREATE TABLE t_log (id int);
        CREATE OR REPLACE PROCEDURE p_clean IS
        BEGIN
            TRUNCATE TABLE t_log;
        END;
        /
        CREATE OR REPLACE PROCEDURE p_read IS
        BEGIN
            PERFORM * FROM t_log;
        END;
        /
        CREATE OR REPLACE PROCEDURE p_ins1 IS
        BEGIN
            INSERT INTO t_log VALUES (1);
        END;
        /
        CREATE OR REPLACE PROCEDURE p_ins2 IS
        BEGIN
            INSERT INTO t_log VALUES (2);
        END;
        /
    "#).unwrap();
    let init = run_codeweb(&["init", "t", "-d", dir.path().to_str().unwrap()]);
    // If init needs a project dir, follow tests/integration_test.rs / impact_test.rs exactly.
    // Alternative: legacy `codeweb <dir> --format json` cannot run `conflicts`.
    // Preferred: `codeweb init` into TempDir then `codeweb conflicts -p <proj>`.
}
```

**Read `tests/impact_test.rs` first** and copy its project-init pattern. Do not invent a new init flow.

Assertions:
1. `p_clean` vs `p_read` on `t_log` is present, severity high.
2. `p_ins1` vs `p_ins2` is **absent** at default (high) and also absent at `--severity medium` (L3 vs L3 does not conflict).
3. Old `.codeweb/store` still loads: run `codeweb stats -p <proj>` after analyze (already implied by conflicts loading the store). No `STORE_VERSION` bump.

**Step 2: FAIL** (`unrecognized subcommand conflicts`).

**Step 3: Implement subcommand + `cmd_conflicts`.** Keep JSON structs in `main.rs` next to `ImpactResult` (same pattern). Do not add MCP/HTTP.

**Step 4: Run**

```
cargo test --test regress_issue_144_ddl_locks -- --nocapture
```

Expected: PASS. `inline_truncate_in_procedure_body_emits_table_access`, `execute_immediate_truncate_literal_still_works`, and `conflicts_reports_truncate_vs_select_not_two_inserts` all ok. Exit code 0.

```
cargo test --bin codeweb conflict -- --nocapture
```

Expected: PASS. Exit code 0.

**Step 5: Commit**

```
git commit -m "feat: codeweb conflicts — cross-procedure lock conflict report"
```

---

## Task 9: End-to-end acceptance + verification matrix

**Files:** none new unless a test gap remains.

Acceptance from #144 (updated labels):

- [ ] Procedure with `ALTER TABLE t RENAME` shows `[L8,D:alter]` in `detail`/`trace` (via `access_mode_label`) and JSON `write_kinds` contains `alter`, modes contain `access_exclusive`.
- [ ] `CREATE INDEX` vs `CREATE INDEX CONCURRENTLY` differ (L5 vs L4).
- [ ] `LOCK TABLE … IN <mode> MODE` maps; default is L8; no `D` tag.
- [ ] `codeweb conflicts`: TRUNCATE vs SELECT → HIGH; two INSERTs → silent.
- [ ] Old store loads (`STORE_VERSION` still 8).
- [ ] Inline TRUNCATE (Task 1) still green.
- [ ] EXECUTE IMMEDIATE truncate still green.

**Commands (mandatory, AGENTS.md Definition of Done):**

```
cargo build --features full
cargo test --features full
cargo clippy --features full -- -D warnings
cargo fmt -- --check
```

If `--features full` has **pre-existing** failures unrelated to this change, document them in the PR body and show the change does not add new ones. Do not "fix" unrelated failures in this PR.

**Commit** only if the last task left uncommitted formatting/clippy fixes:

```
git commit -m "chore: clippy/fmt for DDL lock-level feature"
```

---

## Non-goals / do not

- Do not add fields to `Edge::TableAccess`.
- Do not bump `STORE_VERSION`.
- Do not reuse `ProcTableMatrix`.
- Do not wait for ogsql-parser#320.
- Do not implement MCP/HTTP (Phase 3).
- Do not print `L1`/`L2`/`L3` on ordinary DML edges.
- Do not keep `AccessMode::Truncate` as an alias.
- Do not treat VACUUM/ANALYZE/LOCK TABLE as `D`.
- Do not merge L6 and L7 (they have different conflict rows).

## Risks

| Risk | Handling |
|---|---|
| Parser cannot parse `CREATE INDEX CONCURRENTLY` / `VACUUM FULL` / `LOCK TABLE IN …` in this pin | Fail the test loudly; inspect AST; only then narrow the SQL to what the parser accepts |
| Inline DDL re-parse loses span/line | Accept (known #320 gap); edges still exist |
| `PERFORM * FROM t` may not be a table read | If `p_read` test has no Read edge, use `SELECT * FROM t_log` inside the procedure (or `v_x := (SELECT …)`) |
| CGEF export never wrote modes | Import already optional; adding strings is backward compatible |

## Suggested PR title

`feat: DDL lock-level tags (D + L1–L8) and codeweb conflicts (#144)`
