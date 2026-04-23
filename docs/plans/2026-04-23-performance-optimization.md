# Performance Optimization: Eliminate Redundant Parsing & Add Parallelism

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Reduce codeweb's processing time for large Java web projects by eliminating redundant directory scans (6→1), redundant file parsing (Java×3, XML×2 → each×1), adding rayon parallelism for file parsing, and replacing O(n×m) mapper matching with O(1) lookup.

**Architecture:** Restructure `AllParsedFiles` to retain all parsed results instead of discarding them. Merge the Java parsing pipeline so each file is read once and parsed by both ogsql-parser and tree-sitter in sequence. Pass pre-parsed data through to `GraphBuilder` so it never re-scans or re-parses. Add `rayon` for parallel file parsing and build a reverse-lookup index for mapper matching.

**Tech Stack:** Rust, rayon (new dep), tree-sitter 0.24, tree-sitter-java 0.23, ogsql-parser, petgraph.

---

## Problem Summary

Current execution for 300 Java files + 36 XML files:
- **6 WalkDir scans** of the same directory tree
- **Java files parsed 3×** (ogsql-parser twice, tree-sitter once)
- **XML files parsed 2×** (ogsql-parser twice)
- **All sequential** — single core, zero parallelism
- **O(n×m) mapper matching** with `format!` in inner loop

Target:
- **1 WalkDir scan** — single pass, bucket files by extension
- **Each file parsed exactly once**
- **rayon `par_iter`** for parallel parsing
- **O(1) mapper lookup** via reverse index

---

## Task 1: Single-Pass Directory Scanner

**Files:**
- Create: `src/parser/scanner.rs`
- Modify: `src/parser/mod.rs`

### Step 1: Create `src/parser/scanner.rs`

This replaces all 4 duplicate WalkDir scans with a single pass that buckets files by extension.

```rust
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

#[derive(Debug)]
pub struct ScannedFiles {
    pub sql_files: Vec<PathBuf>,
    pub java_files: Vec<PathBuf>,
    pub xml_files: Vec<PathBuf>,
}

pub fn scan_directory(input: &Path) -> ScannedFiles {
    if input.is_file() {
        return scan_single_file(input);
    }

    let mut sql_files = Vec::new();
    let mut java_files = Vec::new();
    let mut xml_files = Vec::new();

    for entry in WalkDir::new(input).into_iter().filter_map(|e| e.ok()) {
        let path = entry.into_path();
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        match ext {
            "sql" => sql_files.push(path),
            "java" => java_files.push(path),
            "xml" => xml_files.push(path),
            _ => {}
        }
    }

    ScannedFiles {
        sql_files,
        java_files,
        xml_files,
    }
}

fn scan_single_file(input: &Path) -> ScannedFiles {
    let mut scanned = ScannedFiles {
        sql_files: Vec::new(),
        java_files: Vec::new(),
        xml_files: Vec::new(),
    };
    let ext = input.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "sql" => scanned.sql_files.push(input.to_path_buf()),
        "java" => scanned.java_files.push(input.to_path_buf()),
        "xml" => scanned.xml_files.push(input.to_path_buf()),
        _ => {}
    }
    scanned
}
```

### Step 2: Add to `src/parser/mod.rs`

Add:
```rust
pub mod scanner;
pub use scanner::{scan_directory, ScannedFiles};
```

### Step 3: Run `cargo build`

Expected: Compiles.

---

## Task 2: Restructure AllParsedFiles to Retain All Parsed Data

**Files:**
- Modify: `src/parser/loader.rs`
- Modify: `src/parser/mod.rs`

### Step 1: Update `AllParsedFiles` in `src/parser/loader.rs`

Replace:
```rust
pub struct AllParsedFiles {
    pub sql_files: Vec<ParsedFile>,
    pub java_count: usize,
    pub ibatis_count: usize,
}
```

With:
```rust
pub struct AllParsedFiles {
    pub sql_files: Vec<ParsedFile>,
    pub java_files: Vec<crate::parser::java_loader::JavaParsedFile>,
    pub ibatis_files: Vec<crate::parser::ibatis_loader::IbatisParsedFile>,
    pub java_method_results: Vec<crate::parser::java_method::JavaParseResult>,
}
```

### Step 2: Update `load_all_files` in `src/parser/loader.rs`

Replace the function body:

```rust
pub fn load_all_files(input: &Path) -> Result<AllParsedFiles> {
    let scanned = crate::parser::scanner::scan_directory(input);

    if scanned.sql_files.is_empty()
        && scanned.java_files.is_empty()
        && scanned.xml_files.is_empty()
    {
        return Err(CodeWebError::NoFilesFound {
            path: input.to_path_buf(),
        });
    }

    let sql_files = parse_sql_files(&scanned.sql_files);
    let java_files = crate::parser::java_loader::load_java_files_from_paths(&scanned.java_files);
    let ibatis_files = crate::parser::ibatis_loader::load_ibatis_files_from_paths(&scanned.xml_files);
    let java_method_results = crate::parser::java_method::parse_java_files_from_paths(&scanned.java_files);

    Ok(AllParsedFiles {
        sql_files,
        java_files,
        ibatis_files,
        java_method_results,
    })
}

fn parse_sql_files(paths: &[PathBuf]) -> Vec<ParsedFile> {
    let mut parsed = Vec::new();
    for path in paths {
        match parse_file(path) {
            Ok(stmts) => parsed.push(ParsedFile {
                path: path.clone(),
                statements: stmts,
            }),
            Err(e) => {
                eprintln!("warning: skipping {}: {}", path.display(), e);
            }
        }
    }
    parsed
}
```

Note: `load_sql_files_inner` is replaced by `parse_sql_files`. The old `collect_files_by_ext` is no longer needed in this file (moved to scanner.rs).

Keep `load_sql_files` for `--sql-only` mode:
```rust
pub fn load_sql_files(input: &Path) -> Result<Vec<ParsedFile>> {
    let scanned = crate::parser::scanner::scan_directory(input);
    if scanned.sql_files.is_empty() {
        return Err(CodeWebError::NoFilesFound {
            path: input.to_path_buf(),
        });
    }
    Ok(parse_sql_files(&scanned.sql_files))
}
```

Remove the now-dead `load_sql_files_inner` and `collect_files_by_ext` functions.

### Step 3: Add `from_paths` variants to java_loader, ibatis_loader, java_method

In `src/parser/java_loader.rs`, add:
```rust
pub fn load_java_files_from_paths(paths: &[PathBuf]) -> Vec<JavaParsedFile> {
    let mut parsed = Vec::new();
    for path in paths {
        match load_java_file(path) {
            Ok(result) => parsed.push(JavaParsedFile { result }),
            Err(e) => {
                eprintln!("warning: skipping {}: {}", path.display(), e);
            }
        }
    }
    parsed
}
```

Remove the old `collect_java_files` function and the `WalkDir` import. Keep `load_java_file`.

In `src/parser/ibatis_loader.rs`, add:
```rust
pub fn load_ibatis_files_from_paths(paths: &[PathBuf]) -> Vec<IbatisParsedFile> {
    let mut parsed = Vec::new();
    for path in paths {
        match load_ibatis_file(path) {
            Ok(result) => parsed.push(IbatisParsedFile { result }),
            Err(e) => {
                eprintln!("warning: skipping {}: {}", path.display(), e);
            }
        }
    }
    parsed
}
```

Remove old `collect_xml_files` and `WalkDir` import. Keep `load_ibatis_file`.

In `src/parser/java_method.rs`, add:
```rust
pub fn parse_java_files_from_paths(paths: &[PathBuf]) -> Vec<JavaParseResult> {
    let mut results = Vec::new();
    for path in paths {
        match parse_java_file(path) {
            Ok(result) => results.push(result),
            Err(e) => {
                eprintln!("warning: java method parse {}: {}", path.display(), e);
            }
        }
    }
    results
}
```

Remove `parse_java_directory` and `collect_java_files` functions, and the `walkdir` import. Keep `parse_java_file`.

### Step 4: Run `cargo build` and fix compilation errors

The key errors will be:
- `load_java_files(input)` calls in `builder.rs` need updating
- `load_ibatis_files(input)` calls in `builder.rs` need updating
- `parse_java_directory(input)` calls need updating

These will be fixed in Task 3.

### Step 5: Commit

```bash
git add -A
git commit -m "refactor: single-pass directory scanner, retain all parsed data in AllParsedFiles"
```

---

## Task 3: Rewrite build_all to Use Pre-Parsed Data

**Files:**
- Modify: `src/graph/builder.rs`

### Step 1: Update `build_all` signature and body

Replace the current `build_all`:

```rust
pub fn build_all(&self, all: &AllParsedFiles, input: &Path) -> CodeGraph {
```

With:
```rust
pub fn build_all(&self, all: &AllParsedFiles) -> CodeGraph {
```

Remove the `input: &Path` parameter. Update body to use `all.java_files`, `all.ibatis_files`, `all.java_method_results` directly instead of re-scanning:

```rust
pub fn build_all(&self, all: &AllParsedFiles) -> CodeGraph {
    let mut graph = CodeGraph::new();
    let mut proc_index: HashMap<ProcedureId, petgraph::graph::NodeIndex> = HashMap::new();
    let mut mapper_index: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();

    Self::create_procedure_nodes(&all.sql_files, &mut graph, &mut proc_index);
    let edges = Self::collect_call_edges(&all.sql_files);
    Self::create_edges(&edges, &mut graph, &mut proc_index);

    Self::add_ibatis_nodes_from_parsed(&all.ibatis_files, &mut graph, &mut proc_index, &mut mapper_index);
    Self::add_java_nodes_from_parsed(&all.java_files, &mut graph, &mut proc_index, &mapper_index);
    Self::add_java_method_nodes_from_parsed(&all.java_method_results, &mut graph, &mut proc_index, &mapper_index);

    graph
}
```

### Step 2: Add `_from_parsed` variants of add_ibatis_nodes, add_java_nodes, add_java_method_nodes

**`add_ibatis_nodes_from_parsed`** — same logic as `add_ibatis_nodes` but takes `&[IbatisParsedFile]` instead of scanning:

```rust
fn add_ibatis_nodes_from_parsed(
    ibatis_files: &[crate::parser::ibatis_loader::IbatisParsedFile],
    graph: &mut CodeGraph,
    proc_index: &mut HashMap<ProcedureId, petgraph::graph::NodeIndex>,
    mapper_index: &mut HashMap<String, petgraph::graph::NodeIndex>,
) {
    for ibatis_file in ibatis_files {
        // ... exact same body as current add_ibatis_nodes, but iterating ibatis_files parameter
    }
}
```

Copy the body of the current `add_ibatis_nodes` into this new method, just changing the data source.

**`add_java_nodes_from_parsed`** — same pattern, takes `&[JavaParsedFile]`:

```rust
fn add_java_nodes_from_parsed(
    java_files: &[crate::parser::java_loader::JavaParsedFile],
    graph: &mut CodeGraph,
    proc_index: &mut HashMap<ProcedureId, petgraph::graph::NodeIndex>,
    mapper_index: &HashMap<String, petgraph::graph::NodeIndex>,
) {
    // ... exact same body as current add_java_nodes
}
```

**`add_java_method_nodes_from_parsed`** — takes `&[JavaParseResult]`:

```rust
fn add_java_method_nodes_from_parsed(
    java_results: &[crate::parser::java_method::JavaParseResult],
    graph: &mut CodeGraph,
    proc_index: &mut HashMap<ProcedureId, petgraph::graph::NodeIndex>,
    mapper_index: &HashMap<String, petgraph::graph::NodeIndex>,
) {
    // ... exact same body as current add_java_method_nodes
}
```

### Step 3: Remove old methods and unused imports

Remove the old `add_ibatis_nodes(input, ...)`, `add_java_nodes(input, ...)`, `add_java_method_nodes(input, ...)` methods. Remove `use crate::parser::{load_ibatis_files, load_java_files, ...}` imports that are no longer needed.

Keep: `CallEdge`, `CallExtractor`, `ParsedFile`, `AllParsedFiles` imports.

### Step 4: Update main.rs call site

In `src/main.rs`, change:
```rust
builder.build_all(&all, &cli.input)
```
To:
```rust
builder.build_all(&all)
```

### Step 5: Run `cargo build`, `cargo clippy -- -D warnings`

Expected: Clean compilation.

### Step 6: Run `cargo test`

Expected: All 12 tests pass.

### Step 7: Commit

```bash
git add -A
git commit -m "refactor: build_all uses pre-parsed data, eliminates redundant scans and parsing"
```

---

## Task 4: Add rayon Parallelism

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/parser/loader.rs`
- Modify: `src/parser/java_method.rs`

### Step 1: Add rayon to Cargo.toml

```toml
rayon = "1.10"
```

### Step 2: Parallelize SQL file parsing in `src/parser/loader.rs`

Replace `parse_sql_files`:
```rust
fn parse_sql_files(paths: &[PathBuf]) -> Vec<ParsedFile> {
    use rayon::prelude::*;

    paths
        .par_iter()
        .filter_map(|path| {
            parse_file(path).ok().map(|statements| ParsedFile {
                path: path.clone(),
                statements,
            })
        })
        .collect()
}
```

### Step 3: Parallelize Java method parsing in `src/parser/java_method.rs`

Replace `parse_java_files_from_paths`:
```rust
pub fn parse_java_files_from_paths(paths: &[PathBuf]) -> Vec<JavaParseResult> {
    use rayon::prelude::*;

    paths
        .par_iter()
        .filter_map(|path| parse_java_file(path).ok())
        .collect()
}
```

### Step 4: Parallelize Java SQL extraction in `src/parser/java_loader.rs`

Replace `load_java_files_from_paths`:
```rust
pub fn load_java_files_from_paths(paths: &[PathBuf]) -> Vec<JavaParsedFile> {
    use rayon::prelude::*;

    paths
        .par_iter()
        .filter_map(|path| load_java_file(path).ok().map(|result| JavaParsedFile { result }))
        .collect()
}
```

### Step 5: Parallelize iBatis loading in `src/parser/ibatis_loader.rs`

Replace `load_ibatis_files_from_paths`:
```rust
pub fn load_ibatis_files_from_paths(paths: &[PathBuf]) -> Vec<IbatisParsedFile> {
    use rayon::prelude::*;

    paths
        .par_iter()
        .filter_map(|path| load_ibatis_file(path).ok().map(|result| IbatisParsedFile { result }))
        .collect()
}
```

### Step 6: Run `cargo build`, `cargo test`

Expected: All 12 tests pass. Faster wall-clock time on multi-core machines.

### Step 7: Commit

```bash
git add -A
git commit -m "perf: add rayon parallelism for file parsing"
```

---

## Task 5: Parser Reuse + Timeout Protection

**Files:**
- Modify: `src/parser/java_method.rs`

### Step 1: Add thread-local parser caching

At the top of `java_method.rs`, add:

```rust
use std::cell::RefCell;

thread_local! {
    static JAVA_PARSER: RefCell<Parser> = RefCell::new({
        let mut p = Parser::new();
        let _ = p.set_language(&tree_sitter_java::LANGUAGE.into());
        p.set_timeout_micros(5_000_000);
        p
    });
}
```

### Step 2: Update `parse_java_file` to use cached parser

Replace:
```rust
pub fn parse_java_file(path: &Path) -> Result<JavaParseResult, String> {
    let source = std::fs::read_to_string(path).map_err(|e| format!("read error: {}", e))?;
    let source_bytes = source.as_bytes();

    let mut parser = Parser::new();
    parser
        .set_language(&tree_sitter_java::LANGUAGE.into())
        .map_err(|e| format!("language error: {}", e))?;

    let tree = parser
        .parse(source_bytes, None)
        .ok_or_else(|| "parse returned None".to_string())?;
    ...
```

With:
```rust
pub fn parse_java_file(path: &Path) -> Result<JavaParseResult, String> {
    let source = std::fs::read_to_string(path).map_err(|e| format!("read error: {}", e))?;
    let source_bytes = source.as_bytes();

    let tree = JAVA_PARSER.with(|p| {
        let mut parser = p.borrow_mut();
        parser.parse(source_bytes, None)
    }).ok_or_else(|| format!("parse timeout or failure: {}", path.display()))?;
    ...
```

Remove the `use tree_sitter::Parser;` import at the top (now only used in tests and thread_local).

### Step 3: Run `cargo build`, `cargo test`

Expected: All 12 tests pass.

### Step 4: Commit

```bash
git add -A
git commit -m "perf: thread-local tree-sitter parser reuse with 5s timeout"
```

---

## Task 6: O(n×m) → O(1) Mapper Lookup Optimization

**Files:**
- Modify: `src/graph/builder.rs`

### Step 1: Build reverse method-name → mapper index

In `add_java_method_nodes_from_parsed`, before the method processing loop, add:

```rust
let mut method_to_mappers: HashMap<String, Vec<(String, petgraph::graph::NodeIndex)>> = HashMap::new();
for (key, &idx) in mapper_index.iter() {
    if let Some((_, method)) = key.rsplit_once('.') {
        method_to_mappers
            .entry(method.to_string())
            .or_default()
            .push((key.clone(), idx));
    }
}
```

### Step 2: Replace O(n×m) mapper_index.iter() loops with O(1) lookup

Replace both copies of the heuristic mapper matching block:

```rust
// OLD: O(n*m)
for (key, &mapper_idx) in mapper_index.iter() {
    if key.ends_with(&format!(".{}", call.method)) {
        if let Some((ns, _)) = key.rsplit_once('.') {
            let ns_simple = ns.rsplit('.').next().unwrap_or(ns);
            if names_match(obj, ns_simple) { ... }
        }
    }
}
```

With:
```rust
// NEW: O(m) where m = mappers with matching method name (usually 1-3)
if let Some(candidates) = method_to_mappers.get(&call.method) {
    for (key, &mapper_idx) in candidates {
        if let Some((ns, _)) = key.rsplit_once('.') {
            let ns_simple = ns.rsplit('.').next().unwrap_or(ns);
            if names_match(obj, ns_simple) {
                graph.add_edge(
                    method_idx,
                    mapper_idx,
                    Edge::InvokesMapper { location: location.clone() },
                );
                found_mapper = true;
                break;
            }
        }
    }
}
```

This replaces **both** copies of the O(n×m) loop (the one in `resolve_fqn` success branch and the one in `resolve_fqn` failure branch).

### Step 3: Run `cargo build`, `cargo clippy -- -D warnings`, `cargo test`

Expected: All 12 tests pass, no warnings.

### Step 4: Commit

```bash
git add -A
git commit -m "perf: O(1) mapper lookup via method-name reverse index"
```

---

## Task 7: Final Verification

**Files:** None (validation only)

### Step 1: Run full test suite + linting

```bash
cargo test
cargo clippy -- -D warnings
cargo fmt -- --check
```

Expected: 12/12 tests pass, no clippy warnings, fmt clean.

### Step 2: E2E validation

```bash
cargo build
./target/debug/codeweb /tmp/codeweb-e2e-test/ --format json 2>&1
```

Expected: Same output structure as before optimization (same nodes, same edges), but faster.

### Step 3: Verify no regression in output

The JSON output should contain the same node types and edge types as pre-optimization:
- `java_class`, `java_method`, `mapped_statement` nodes
- `extends`, `implements`, `invokes_mapper`, `calls_java`, `contains_method` edges

---

## Summary of Deliverables

| Task | What | Impact |
|---|---|---|
| 1 | Single-pass directory scanner (new `scanner.rs`) | WalkDir 6→1 |
| 2 | Retain parsed data in `AllParsedFiles` | Foundation for Task 3 |
| 3 | `build_all` uses pre-parsed data | Eliminates all redundant parsing |
| 4 | rayon `par_iter` for all file parsing | 4-8x wall-clock on multi-core |
| 5 | thread-local parser + 5s timeout | Reuse allocations, prevent hangs |
| 6 | Reverse mapper lookup index | O(n×m) → O(n) |
| 7 | Final verification | No regressions |

**Expected overall improvement:** 5-10x faster for large projects (300+ files).
