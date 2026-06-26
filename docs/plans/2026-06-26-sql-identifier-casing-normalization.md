# SQL Identifier Case Normalization Fix Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Fix all case-sensitivity bugs where SQL identifiers (package, type, sequence, procedure/function, trigger, index, event) with different casing produce duplicate or orphan graph nodes.

**Architecture:** The root cause is that `ogsql_parser::Ident` preserves original case and codeweb does not consistently normalize to lowercase. Table/View/MaterializedView are already safe via `normalize_table_key`. The fix extends the same normalization pattern to all other SQL identifier types across two dimensions: (1) in-build dedup indices in `builder.rs`, and (2) cross-store merge keys in `key.rs`.

**Tech Stack:** Rust, codeweb graph builder (`src/graph/builder.rs`, `src/graph/key.rs`, `src/graph/mod.rs`)

---

## Background

`ogsql_parser::Ident` stores identifiers with original casing and a `quote_style: Option<char>` field. SQL semantics (PostgreSQL/openGauss) fold unquoted identifiers to lowercase, but ogsql-parser does NOT fold — this is codeweb's responsibility.

**Reference pattern** — already safe:
```rust
// builder.rs:3145
fn normalize_table_key(schema: Option<&str>, name: &str) -> String {
    match schema {
        Some(s) => format!("{}.{}", s.to_lowercase(), name.to_lowercase()),
        None => name.to_lowercase(),
    }
}
```

**Vulnerable indices** (builder.rs) — raw case keys:
| Index | Key type | Used by |
|-------|----------|---------|
| `package_index` | `String` | Package |
| `proc_index` | `RoutineId` | Procedure, Function, Package members |
| `type_index` | `String` | Type |
| `sequence_index` | `String` | Sequence |

**Vulnerable NodeKey variants** (key.rs) — raw case in `from_node()`:
Package, Trigger, Type, Sequence, Index, Event

---

**Existing regression tests** (all FAIL as expected until fix):
```sh
cargo test --test regress_package_casing
# FAILS: 6 tests (pkg name, proc name, call edge, standalone proc, type, sequence)
```

## Fix Approach

### Strategy: Normalize at insertion + lookup for String indices; normalize RoutineId construction for proc_index

- **String-keyed indices** (`package_index`, `type_index`, `sequence_index`): normalize at the `.entry()` call site (surgical, zero side effects).
- **Spec/body matching**: case-insensitive comparison (single line change).
- **`proc_index`** (`RoutineId`-keyed): more widespread. Create a `normalize_routine_id()` helper or normalize in `from_object_name`/`from_qualified_name`. Then update insertion AND lookup sites.
- **`key.rs` `from_node()`**: add `.to_lowercase()` to the 6 buggy variants.
- **`key.rs` `relaxed()`**: extend to cover Package/Type/Sequence/MaterializedView/Synonym (like Procedure/Function already do).

### Task 1: Fix `package_index` — `create_package_nodes`

**Files:**
- Modify: `src/graph/builder.rs:963-974`

**Problem:** `qualified` key uses raw-case `pkg_name_part` and `schema_part`.

**Fix:** Normalize `pkg_name_part` and `schema_part` to lowercase before building `qualified`.

```rust
// Before (lines 963-972):
let pkg_name_part = pkg_name.last().cloned().unwrap_or_default().to_string();
let schema_part: Option<String> = if pkg_name.len() > 1 {
    Some(pkg_name[..pkg_name.len() - 1].join("."))
} else { None };
let qualified = match &schema_part {
    Some(s) => format!("{}.{}", s, pkg_name_part),
    None => pkg_name_part.clone(),
};

// After:
let pkg_name_part = pkg_name.last().cloned().unwrap_or_default().to_lowercase();
let schema_part: Option<String> = if pkg_name.len() > 1 {
    Some(pkg_name[..pkg_name.len() - 1].iter().map(|i| i.to_lowercase()).collect::<Vec<_>>().join("."))
} else { None };
let qualified = match &schema_part {
    Some(ref s) => format!("{}.{}", s, pkg_name_part),
    None => pkg_name_part.clone(),
};
```

**Verify:** `regress_pkg_name_casing_mismatch` and `regress_pkg_call_edge_casing` should pass.

### Task 2: Fix `type_index` and `sequence_index` — `create_sql_nodes`

**Files:**
- Modify: `src/graph/builder.rs:454-460` (Type), `473-479` (Sequence)

**Problem:** `short_key` and `full_key` used as-index keys without lowercasing.

**Fix:** Add a helper `normalize_object_key(schema: Option<&str>, name: &str) -> String` (mirror of `normalize_table_key`) and route all non-table keys through it.

Add after `normalize_table_key` (line 3150):
```rust
fn normalize_object_key(schema: Option<&str>, name: &str) -> String {
    match schema {
        Some(s) => format!("{}.{}", s.to_lowercase(), name.to_lowercase()),
        None => name.to_lowercase(),
    }
}
```

**Type fix** (lines 454-460):
```rust
// Before:
let short_key = name.clone();
let full_key = match &schema { Some(s) => format!("{}.{}", s, name), None => name.clone() };

// After:
let short_key = normalize_object_key(None, &name);
let full_key = normalize_object_key(schema.as_deref(), &name);
```

**Sequence fix** (lines 473-479): same pattern.

**Verify:** `regress_type_casing_mismatch` and `regress_sequence_casing_mismatch` should pass.

### Task 3: Fix spec/body matching — `create_sql_nodes`

**Files:**
- Modify: `src/graph/builder.rs:905`

**Problem:** Case-sensitive `==` comparison between head-declared routine name and body-implemented name.

**Fix:** Use `eq_ignore_ascii_case()`.

```rust
// Before (line 905):
.any(|(pn, rn)| pn == pkg_name && rn == routine_name);

// After:
.any(|(pn, rn)| pn.eq_ignore_ascii_case(pkg_name) && rn.eq_ignore_ascii_case(routine_name));
```

**Also fix normalization of strings pushed to spec_decls/body_impls:**
The `pkg_name` pushed at lines 286 and 309 uses `pkg.name.last().cloned().unwrap_or_default().to_string()` — should be `.to_lowercase()`.
The `name` uses `p.name.join(".")` — should be lowercased too.

```rust
// Lines 273 (spec):
let pkg_name = pkg.name.last().cloned().unwrap_or_default().to_lowercase();
let name = match item {
    PackageItem::Procedure(p) => p.name.iter().map(|i| i.to_lowercase()).collect::<Vec<_>>().join("."),
    PackageItem::Function(f) => f.name.iter().map(|i| i.to_lowercase()).collect::<Vec<_>>().join("."),
    ...
};

// Lines 300 (body): same pattern for pkg_name and name
```

**Verify:** `regress_proc_name_casing_mismatch` should pass.

### Task 4: Fix `proc_index` keys — `RoutineId` normalization

**Files:**
- Modify: `src/graph/mod.rs:152-173` (RoutineId constructors)
- Modify: `src/graph/builder.rs:206, 235, 325, 822-829, 907-912, 1008-1012` (insertion sites)
- Modify: `src/graph/builder.rs:1447-1528` (lookup sites in create_edges)

**Problem:** `RoutineId` fields use raw case, so `proc_index` has case-sensitive dedup. Call resolution in `create_edges` also uses raw case for lookup.

**Fix Strategy A (recommended — highest quality):**
Add a `normalize()` method on `RoutineId` that returns a new RoutineId with all string fields lowercased. Call it at every insertion and lookup site.

Add to `src/graph/mod.rs` (after line 173):
```rust
impl RoutineId {
    pub fn normalized(&self) -> Self {
        Self {
            schema: self.schema.as_ref().map(|s| s.to_lowercase()),
            package: self.package.as_ref().map(|p| p.to_lowercase()),
            name: self.name.to_lowercase(),
            kind: self.kind,
        }
    }
}
```

Then in `builder.rs`:

**Insertion sites:**
- Line 221: `proc_index.entry(id.normalized()).or_insert_with(|| {`
- Line 250: `proc_index.entry(id.normalized()).or_insert_with(|| {`
- Line 356: `proc_index.insert(func_id.normalized(), idx);`
- Line 943: `proc_index.insert(routine_id.normalized(), idx);`
- Line 1014: `proc_index.entry(proc_id.normalized()).or_insert_with(|| {`
- Line 325: `proc_index.get(&func_id.normalized())` (lookup)
- Line 913: `proc_index.contains_key(&routine_id.normalized())` (lookup)

**Lookup sites in `create_edges`:**
- Lines 1447-1461: caller lookup — normalize before each `proc_index.get()` call
- Lines 1463-1472: callee direct lookup — lowercase `edge.callee_name` via `from_qualified_name` or normalize result
- Lines 1474-1485: schema-as-package fallback — normalize alt_id
- Lines 1486-1528: pkg_member_lower is already case-insensitive (lowercased keys) — no change needed

```rust
// Around lines 1463-1472:
let callee_id =
    RoutineId::from_qualified_name(&edge.callee_name, RoutineKind::Procedure).normalized();
let callee_idx = proc_index
    .get(&callee_id)
    .copied()
    .or_else(|| {
        let func_id =
            RoutineId::from_qualified_name(&edge.callee_name, RoutineKind::Function).normalized();
        proc_index.get(&func_id).copied()
    })
    // ... rest unchanged
```

**Fix Strategy B (minimal-risk alternative):**
Skip `proc_index` changes entirely. Accept that standalone Procedure/Function with different casing in the same build produce duplicate nodes (extremely rare in practice — you can't define the same procedure twice in the same schema). The package member path is already handled by the case-insensitive `pkg_member_lower` in `create_edges`.

**Verify:** `regress_procedure_casing_mismatch` should pass.

### Task 5: Fix `key.rs` — `NodeKey::from_node()` normalization

**Files:**
- Modify: `src/graph/key.rs:209-236`

**Problem:** 6 NodeKey variants use `.clone()` instead of `.to_lowercase()`.

**Fix:** Add `.to_lowercase()` / `.map(|s| s.to_lowercase())` to each variant.

```rust
// Line 209-212 — Package (BEFORE):
super::Node::Package { schema, name, .. } => NodeKey::Package {
    schema: schema.clone(),
    name: name.clone(),
},
// AFTER:
super::Node::Package { schema, name, .. } => NodeKey::Package {
    schema: schema.as_ref().map(|s| s.to_lowercase()),
    name: name.to_lowercase(),
},

// Line 213 — Trigger (BEFORE):
super::Node::Trigger { name, .. } => NodeKey::Trigger { name: name.clone() },
// AFTER:
super::Node::Trigger { name, .. } => NodeKey::Trigger { name: name.to_lowercase() },

// Lines 214-217 — Type (BEFORE):
super::Node::Type { schema, name, .. } => NodeKey::Type {
    schema: schema.clone(),
    name: name.clone(),
},
// AFTER:
super::Node::Type { schema, name, .. } => NodeKey::Type {
    schema: schema.as_ref().map(|s| s.to_lowercase()),
    name: name.to_lowercase(),
},

// Lines 218-221 — Sequence (same pattern as Type)

// Lines 222-227 — Index:
// BEFORE: name: name.clone(), table_name: table_name.clone()
// AFTER:  name: name.as_ref().map(|n| n.to_lowercase()), table_name: table_name.to_lowercase()

// Line 236 — Event:
// BEFORE: name: name.clone()
// AFTER:  name: name.to_lowercase()
```

**Verify:** Existing `merge_case_insensitive_procedure_keys` test should still pass. Add a similar test for package/type/sequence.

### Task 6: Extend `relaxed()` to cover all DB object types (optional)

**Files:**
- Modify: `src/graph/key.rs:287-309`

**Problem:** `relaxed()` only handles Procedure/Function. Package/Type/Sequence/MaterializedView/Synonym also have schema fields that could differ between stores during merge.

**Fix:** Add match arms for all DB object types that have a schema field.

**Note:** This is a separate concern from case normalization. The `relaxed()` drops schema, not normalize case. But it's worth fixing while in this file.

**Verify:** Integration test in store.rs.

### Task 7: Bump `fixed_in` in regression test metadata

**Files:**
- Modify: `tests/regress/package_casing_mismatch/cases.toml`

Change `fixed_in = "v0.X.Y"` to `fixed_in = "v0.8.0"` (or next release version).

---

## Verification Matrix

```sh
# Must pass:
cargo test --test regress_package_casing  # 6 tests (was failing, now passing)
cargo test --test regress_function_call_edges  # 7 tests (regression check)
cargo test --test regress_local_var_not_call_edges  # 11 tests (regression check)
cargo test --test regress_execute_immediate_paren_plvar  # 2 tests
cargo test --test regress_func_schema_resolution  # 4 tests

# Build matrix:
cargo build --features full
cargo test --features full
cargo clippy --features full -- -D warnings
cargo fmt -- --check
```

## Risk Assessment

| Change | Risk | Mitigation |
|--------|------|------------|
| `package_index` normalization | Low — String key, isolated | Only affects package dedup |
| `type_index` normalization | Low — String key, isolated | Only affects type dedup |
| `sequence_index` normalization | Low — String key, isolated | Only affects sequence dedup |
| Spec/body `eq_ignore_ascii_case` | Low — comparison only, no hash | Only affects partial node detection |
| `RoutineId::normalized()` at insertion | Medium — RoutineId used in many paths | All existing tests must pass unchanged |
| `create_edges` lookup normalization | Medium — edge resolution logic | Verify with `regress_pkg_call_edge_casing` |
| `key.rs from_node()` normalization | Low — only affects cross-store merge | Existing merge tests must pass |
| `relaxed()` extension | Low — new arms, no functional change | Only adds new merge paths |

---

## Execution Order

1. **Task 2** (type_index, sequence_index) — create `normalize_object_key` helper first (reused later)
2. **Task 1** (package_index) — also uses normalization
3. **Task 3** (spec/body matching) — single line change
4. **Task 4** (proc_index) — most involved, implement `RoutineId::normalized()` first
5. **Task 5** (key.rs) — standalone file, independent
6. **Task 6** (relaxed) — optional, after key.rs
7. **Full verification matrix**
8. **Task 7** (cases.toml metadata)
