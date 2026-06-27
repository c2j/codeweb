# Built-in Function Tracking Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Make built-in SQL functions (COUNT, SUBSTR, DBE_OUTPUT.PUT_LINE, ...) first-class nodes in the code graph, queryable by name with reverse-reference lookup, while keeping `detail`/`trace` CLI output clean by default (hidden unless `--builtfunc` or the target itself is a built-in).

**Architecture:** Two-layer design — (1) graph layer fully models `Node::BuiltinFunction` + `Edge::UsesBuiltinFunction` as first-class citizens, auto-inheriting all 6 query entry points (search_nodes, impact, GraphTraversal, QuerySpec, CLI, HTTP, MCP); (2) presentation layer adds a `skip_builtins` filter to `trace_chain`, controlled by `--builtfunc` flag on `detail`/`trace` commands.

**Tech Stack:** Rust, petgraph, ogsql-parser (provides `BuiltinFuncMeta { category, domain }` via `Expr::FunctionCall.builtin`), clap.

---

## Background Context

### Current behavior (what changes)
- `src/parser/extractor.rs:617-632` — `visit_expr` only processes `Expr::FunctionCall { builtin: None, .. }`. Built-ins are silently dropped.
- `src/graph/builder.rs:2878-2879` — `noise_rule()` calls `lookup_builtin_meta()` and returns `Some("builtin-function")`, causing Unresolved nodes for built-ins to be deleted.
- Test `builtin_function_not_captured_as_call` (builder.rs:4509) pins the "exclude" contract.

### Key data structures
```rust
// extractor.rs:150-155 — call record pushed by push_call()
pub struct CallEdge {
    pub caller: Option<RoutineId>,
    pub callee_name: String,
    pub is_dynamic: bool,
    pub location: SourceLocation,
}

// ogsql-parser ast/mod.rs:47-50 — metadata carried on FunctionCall
pub struct BuiltinFuncMeta {
    pub category: String,   // "Scalar", "Aggregate", "Window", ...
    pub domain: String,     // "String", "Math", "DbeOutput", ...
}

// ogsql-parser ast/mod.rs:1068-1075 — AST node
Expr::FunctionCall { name: ObjectName, args, alias, column_defs, builtin: Option<BuiltinFuncMeta> }
```

### Design decisions (locked)
- **Option B** (nodes + edges), not edge-only. Rationale: all query entry points auto-inherit.
- **On-demand node creation** — create BuiltinFunction nodes only for builtins actually called in source code (not pre-load all 756).
- **detail/trace default: hide builtins** — `skip_builtins = !builtfunc_flag && !target_is_builtin`.
- **trace command also gets `--builtfunc`** — consistency with detail.

---

## Phase 1: Data Model + Match Arms (compile-pass foundation)

Adding an enum variant in Rust breaks compilation until ALL match sites are updated. This phase touches every file that matches on `Node` or `NodeKind`, with minimal/trivial arm bodies. Logic is filled in Phases 2-3.

### Task 1.1: Add Node::BuiltinFunction and Edge::UsesBuiltinFunction

**Files:**
- Modify: `src/graph/mod.rs`

**Step 1: Add Node variant** (insert after `Node::Event { .. }`, before `Node::Custom { .. }`, ~L400)

```rust
/// A built-in SQL function (COUNT, SUBSTR, DBE_OUTPUT.PUT_LINE, ...).
///
/// Created on-demand when a FunctionCall tagged `builtin: Some(..)` is
/// encountered during extraction. Deduplication key: lowercased `name`.
BuiltinFunction {
    name: String,
    category: String,
    domain: String,
    location: SourceLocation,
},
```

**Step 2: Add Edge variant** (insert after `Edge::CallsJava { .. }`, before `Edge::ContainsMethod`, ~L480)

```rust
/// A procedure/function calls a built-in SQL function.
UsesBuiltinFunction {
    location: SourceLocation,
},
```

**Step 3: Update `node_type_tag()`** (~L429-455, add before `Node::Custom`)

```rust
Node::BuiltinFunction { .. } => "builtin",
```

**Step 4: Update `Edge::category()`** (~L528-545, add `UsesBuiltinFunction` to the Call group)

```rust
Edge::DirectCall { .. }
| Edge::DynamicCall { .. }
| Edge::CallsProcedure { .. }
| Edge::InvokesMapper { .. }
| Edge::CallsJava { .. }
| Edge::UsesBuiltinFunction { .. } => EdgeCategory::Call,   // ← add this line
```

### Task 1.2: Update NodeKey — `src/graph/key.rs`

Read the existing `Node::Function` arm in `NodeKey::from_node()` (or the `match node` at ~L180) and mirror it:

```rust
Node::BuiltinFunction { name, .. } => NodeKey::BuiltinFunction {
    name: name.to_lowercase(),
},
```

Add the corresponding `NodeKey::BuiltinFunction { name: String }` enum variant. Check what `NodeKey` uses for serialization/hash — mirror `Function`'s pattern exactly.

### Task 1.3: Update NodeKind — `src/graph/cluster.rs`

Add variant to `NodeKind` enum (~L15) and arm to `from_node()` (~L42):

```rust
// enum
BuiltinFunction,

// from_node
Node::BuiltinFunction { .. } => NodeKind::BuiltinFunction,
```

Also update any `NodeKind::tag()` or display method to return `"builtin"`.

### Task 1.4: Update all remaining match sites (trivial arms)

For each file below, find every `match node {}` / `match NodeKind {}` and add a `BuiltinFunction` arm. Use the **same pattern as `Function`** unless noted. These are mechanical — no logic, just preventing non-exhaustive match errors.

| File | Match sites (line ~) | Pattern notes |
|---|---|---|
| `src/graph/store.rs` | L1762, L1775 | Mirror `Function` arm (indexing/stats) |
| `src/graph/builder.rs` | multiple | Phase 2 fills real logic; for now mirror `Unresolved` as placeholder |
| `src/export/dot.rs` | 1 site | Color/shape: use a distinct color (e.g. same as `Function` but dashed) |
| `src/export/json.rs` | 1 site | Serialize all fields |
| `src/export/mermaid.rs` | 1 site | Mirror `Function` node style |
| `src/import/parser.rs` | L696 | Return error or skip — CGEF doesn't produce builtin nodes from parser |
| `src/tui/app.rs` | L878, L952 | Mirror `Function` display |
| `src/main.rs` | L870, L1126, L1333 | Mirror `Function` display formatting |
| `src/server/handlers.rs` | L159, L241 | Mirror `Function` JSON serialization |
| `src/mcp/tools.rs` | L238, L405 | Mirror `Function` serialization |

**Step: Verify compilation**

```sh
cargo build
```
Expected: clean compile (0 errors). If errors remain, find missed match sites via compiler errors — they list every location.

```sh
cargo build --features full
```
Expected: clean compile under all features.

---

## Phase 2: Extraction + Graph Building (functional core)

### Task 2.1: Extend CallEdge with builtin metadata

**File:** `src/parser/extractor.rs`

**Step 1:** Add `builtin_meta` field to `CallEdge` (L150-155):

```rust
#[derive(Debug, Clone)]
pub struct CallEdge {
    pub caller: Option<RoutineId>,
    pub callee_name: String,
    pub is_dynamic: bool,
    pub location: SourceLocation,
    pub builtin_meta: Option<ogsql_parser::ast::BuiltinFuncMeta>,  // ← NEW
}
```

**Step 2:** Update `push_call()` (L244-251) to set `builtin_meta: None`:

```rust
pub fn push_call(&mut self, callee: &str, is_dynamic: bool, line: usize) {
    self.edges.push(CallEdge {
        caller: self.current_procedure.clone(),
        callee_name: callee.to_string(),
        is_dynamic,
        location: self.make_location(line),
        builtin_meta: None,
    });
}
```

**Step 3:** Add `push_builtin_call()`:

```rust
pub fn push_builtin_call(
    &mut self,
    callee: &str,
    meta: ogsql_parser::ast::BuiltinFuncMeta,
    line: usize,
) {
    self.edges.push(CallEdge {
        caller: self.current_procedure.clone(),
        callee_name: callee.to_string(),
        is_dynamic: false,
        location: self.make_location(line),
        builtin_meta: Some(meta),
    });
}
```

**Step 4:** Fix any other `CallEdge { ... }` construction sites (search for `CallEdge {` — any struct literal must now include `builtin_meta`).

### Task 2.2: Split visit_expr to handle builtins

**File:** `src/parser/extractor.rs:617-632`

Replace the current `visit_expr`:

```rust
fn visit_expr(&mut self, expr: &Expr) -> VisitorResult {
    if let Expr::FunctionCall { name, builtin, .. } = expr {
        if name.is_empty() {
            return VisitorResult::Continue;
        }
        let first = name[0].to_lowercase();
        if self.local_vars.contains(&first) || self.known_types.contains(&first) {
            return VisitorResult::Continue;
        }
        match builtin {
            None => {
                // User-defined function — existing behavior
                self.push_call(&name.join("."), false, 0);
            }
            Some(meta) => {
                // Built-in function — new path
                self.push_builtin_call(&name.join("."), meta.clone(), 0);
            }
        }
    }
    VisitorResult::Continue
}
```

### Task 2.3: Graph builder consumes builtin_meta

**File:** `src/graph/builder.rs`

Find where `CallEdge` records are consumed and converted to graph nodes/edges (search for `.edges` or `CallEdge` in builder.rs). Add a branch for `builtin_meta.is_some()`:

```rust
// Pseudocode — adapt to actual consumption site structure
for edge in &extractor.edges {
    if let Some(meta) = &edge.builtin_meta {
        // Find or create BuiltinFunction node (dedup by lowercased name)
        let builtin_idx = find_or_create_builtin(
            graph,
            &edge.callee_name,
            &meta.category,
            &meta.domain,
            &edge.location,
        );
        // Connect caller → builtin with UsesBuiltinFunction edge
        if let Some(caller_idx) = /* resolve caller */ {
            graph.add_edge(caller_idx, builtin_idx, Edge::UsesBuiltinFunction {
                location: edge.location.clone(),
            });
        }
        continue;  // skip normal DirectCall path
    }
    // ... existing DirectCall/DynamicCall logic for user functions ...
}
```

Implement `find_or_create_builtin` with a HashMap<lowercase_name, NodeIndex> for dedup (mirror how `proc_index` deduplicates procedures).

### Task 2.4: noise_rule — keep as defensive backstop

**File:** `src/graph/builder.rs:2878-2879`

**Do NOT remove** the `builtin-function` noise_rule branch. It now serves as a defensive filter: if a built-in somehow still reaches `Node::Unresolved` (e.g. from dynamic SQL that wasn't resolved), it gets filtered instead of polluting the graph. Log a warning so it's visible.

No code change needed here — but verify behavior: builtins from expressions now flow through Task 2.3, never reaching Unresolved.

### Task 2.5: Verify extraction works

**Step 1:** Write a new test in `src/graph/builder.rs` test module:

```rust
#[test]
fn builtin_function_captured_as_node() {
    // SQL: CREATE OR REPLACE FUNCTION use_count() ... PERFORM COUNT(*) ...
    // Assert: graph contains Node::BuiltinFunction { name: "count", category: "Aggregate", domain: "Aggregate" }
    // Assert: a UsesBuiltinFunction edge connects the caller to the builtin node
}
```

**Step 2:** Run and verify pass:

```sh
cargo test builtin_function_captured_as_node -- --nocapture
```

---

## Phase 3: Presentation Layer (skip_builtins filter)

### Task 3.1: Add skip_builtins to traverse.rs

**File:** `src/graph/traverse.rs`

**Step 1:** Add `skip_builtins: bool` parameter to `build_tree_dfs` (L117-175):

```rust
#[allow(clippy::too_many_arguments)]
fn build_tree_dfs(
    graph: &crate::graph::CodeGraph,
    start: NodeIndex,
    direction: Direction,
    ancestors: &mut HashSet<NodeIndex>,
    depth: usize,
    max_depth: usize,
    max_nodes: usize,
    visited: &mut usize,
    skip_builtins: bool,   // ← NEW
) -> Vec<TreeNode> {
```

Update the neighbor filter (L141-144):

```rust
let neighbors: Vec<NodeIndex> = graph
    .neighbors_directed(start, direction)
    .filter(|n| !ancestors.contains(n))
    .filter(|n| !skip_builtins || !matches!(graph[*n], Node::BuiltinFunction { .. }))
    .collect();
```

Pass `skip_builtins` through the recursive call (L157-166).

**Step 2:** Add `skip_builtins: bool` to `trace_chain` (L177-219):

```rust
pub fn trace_chain(
    graph: &crate::graph::CodeGraph,
    start: NodeIndex,
    max_depth: usize,
    max_nodes: usize,
    skip_builtins: bool,   // ← NEW
) -> (CallChain, usize) {
```

Pass to both `build_tree_dfs` calls (callers + callees).

**Step 3:** Add `UsesBuiltinFunction` arm to `edge_label_for` (L52-115):

```rust
Edge::UsesBuiltinFunction { .. } => Some("[builtin]".into()),
```

### Task 3.2: Add --builtfunc flag to detail and trace commands

**File:** `src/main.rs`

**Step 1:** Add flag to `Commands::Detail` (L305-320):

```rust
Detail {
    name: String,
    #[arg(short, long, default_value = ".")]
    project: PathBuf,
    #[arg(short, long, default_value = "tree", value_parser = ["tree", "path"])]
    style: String,
    #[arg(short, long)]
    files: bool,
    /// Show built-in function calls in the chain (default: hidden)
    #[arg(long)]
    builtfunc: bool,   // ← NEW
},
```

**Step 2:** Add same flag to `Commands::Trace` (L223-234):

```rust
Trace {
    from: String,
    #[arg(short, long, default_value = ".")]
    project: PathBuf,
    #[arg(short, long, default_value = "tree", value_parser = ["tree", "path"])]
    style: String,
    /// Show built-in function calls in the chain (default: hidden)
    #[arg(long)]
    builtfunc: bool,   // ← NEW
},
```

**Step 3:** Update dispatch arms (~L572, ~L757 area) to pass `builtfunc`.

**Step 4:** Update `cmd_detail` signature and logic (L1008):

```rust
fn cmd_detail(
    name: &str,
    project: &Path,
    style: &str,
    show_files: bool,
    show_builtins: bool,   // ← NEW
) -> Result<()> {
    // ... existing search logic ...

    let target_is_builtin = matches!(graph[*start_idx], Node::BuiltinFunction { .. });
    let skip_builtins = !show_builtins && !target_is_builtin;

    let (chain, _) = graph::traverse::trace_chain(graph, *start_idx, 50, usize::MAX, skip_builtins);
    // ... rest unchanged ...
}
```

**Step 5:** Update `cmd_trace` signature and logic similarly (find it near L757):

```rust
fn cmd_trace(
    from: &str,
    project: &Path,
    style: &str,
    show_builtins: bool,   // ← NEW
) -> Result<()> {
    // ...
    let target_is_builtin = matches!(graph[*start_idx], Node::BuiltinFunction { .. });
    let skip_builtins = !show_builtins && !target_is_builtin;
    let (chain, _) = graph::traverse::trace_chain(graph, *start_idx, 50, usize::MAX, skip_builtins);
    // ...
}
```

### Task 3.3: Update non-CLI trace_chain callers

These callers pass `false` (show everything — they're programmatic APIs).

| File | Site | Change |
|---|---|---|
| `src/server/handlers.rs` | trace handler (~L401) | Add `false` arg to `trace_chain` call |
| `src/mcp/tools.rs` | codeweb_trace tool | Add `false` arg to `trace_chain` call |

Search for all `trace_chain(` calls: `grep -rn "trace_chain(" src/` — every call site gets the new `bool` argument.

### Task 3.4: Verify presentation filter

```sh
# detail of a procedure should NOT show builtins in callees tree
cargo run -- detail "some_proc" 

# detail with --builtfunc should show them
cargo run -- detail "some_proc" --builtfunc

# detail of a builtin itself should show its callers regardless
cargo run -- detail "count"
```

---

## Phase 4: Test Flip + Full Verification

### Task 4.1: Flip existing exclusion tests

**File:** `src/graph/builder.rs`

**Test `builtin_function_not_captured_as_call` (~L4509):**
- Rename to `builtin_function_captured_as_builtin_node`
- Change assertions: COUNT now creates a `BuiltinFunction` node + `UsesBuiltinFunction` edge (NOT a `DirectCall`)
- Assert NO `DirectCall` edge still exists (builtins use their own edge type)

**Test `noise_filter_recognizes_dbe_xmldom_builtins` (~L4162):**
- These builtins now become BuiltinFunction nodes instead of being filtered
- Update assertions to check for node existence rather than absence

### Task 4.2: Add new tests

**File:** `src/graph/builder.rs` test module

```rust
#[test]
fn detail_hides_builtins_by_default() {
    // Build graph with a proc that calls COUNT
    // trace_chain with skip_builtins=true → callees tree excludes BuiltinFunction
}

#[test]
fn detail_shows_builtins_with_flag() {
    // trace_chain with skip_builtins=false → callees tree includes BuiltinFunction
}

#[test]
fn detail_of_builtin_shows_callers() {
    // trace_chain from a BuiltinFunction node → skip_builtins=false regardless
    // callers tree shows the calling procedures
}

#[test]
fn builtin_node_dedup() {
    // Two procedures calling SUBSTR → one BuiltinFunction node, two edges
}
```

### Task 4.3: Full verification matrix (AGENTS.md Definition of Done)

```sh
# Compilation under every feature combination
cargo build
cargo build --features serve
cargo build --features mcp
cargo build --features jsp
cargo build --features full

# Tests
cargo test
cargo test --features full

# Lint
cargo clippy -- -D warnings
cargo clippy --features full -- -D warnings

# Format
cargo fmt -- --check
```

ALL must pass with 0 errors. Document any pre-existing failures unrelated to this change.

---

## File Change Summary

| File | Phase | Change type |
|---|---|---|
| `src/graph/mod.rs` | 1 | Add Node variant, Edge variant, type_tag, category |
| `src/graph/key.rs` | 1 | Add NodeKey variant + from_node arm |
| `src/graph/cluster.rs` | 1 | Add NodeKind variant + from_node arm |
| `src/graph/store.rs` | 1 | 2 match arms (stats/indexing) |
| `src/graph/builder.rs` | 1+2 | match arm + builtin_meta consumption + dedup |
| `src/export/dot.rs` | 1 | 1 match arm |
| `src/export/json.rs` | 1 | 1 match arm |
| `src/export/mermaid.rs` | 1 | 1 match arm |
| `src/import/parser.rs` | 1 | 1 match arm |
| `src/tui/app.rs` | 1 | 2 match arms |
| `src/main.rs` | 1+3 | 3 match arms + detail/trace --builtfunc + cmd signatures |
| `src/server/handlers.rs` | 1+3 | 2 match arms + trace_chain call |
| `src/mcp/tools.rs` | 1+3 | 2 match arms + trace_chain call |
| `src/parser/extractor.rs` | 2 | CallEdge field + visit_expr split + push_builtin_call |
| `src/graph/traverse.rs` | 3 | skip_builtins param + edge_label_for |

**Total: ~15 files, ~25 distinct edit sites.**

---

## Risk Notes

1. **CallEdge struct literal sites**: Adding `builtin_meta` field breaks all `CallEdge { ... }` literals. Compiler will list every site — fix each by adding `builtin_meta: None`.
2. **trace_chain signature change**: Every call site must be updated atomically. Use `grep -rn "trace_chain(" src/` to find all of them.
3. **Test fixtures**: `tests/regress_function_call_edges/cases/builtin_not_captured.sql` may need review — its expectation flips.
4. **Store serialization**: BuiltinFunction nodes must serialize/deserialize correctly (bincode store). The `Node` enum derives `Serialize/Deserialize` — adding a variant is backward-incompatible for old store files. Document this as a breaking change in commit message.
5. **extract_func_from_table_ref** (extractor.rs:636): FROM-clause functions have NO `builtin: None` guard. Consider whether builtins in FROM clause (rare, e.g. `generate_series`) should also get `push_builtin_call`. Low priority — can be a follow-up.
