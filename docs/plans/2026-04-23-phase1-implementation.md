# Phase 1: SQL Stored Procedure Call Graph — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Parse GaussDB SQL files, extract stored procedure call relationships, build a directed graph, and export to DOT/JSON/Mermaid via CLI.

**Architecture:** Three-layer pipeline: `loader` (file discovery) → `extractor` (Visitor-based call extraction) → `graph::builder` (petgraph construction) → `export` (format rendering). CLI via clap.

**Tech Stack:** Rust, ogsql-parser (git dep, commit `8405b1a`), petgraph, thiserror, clap, serde/serde_json, walkdir

---

## Task 1: Initialize Cargo Project

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`

**Step 1: Initialize project**

```sh
cargo init --name codeweb
```

**Step 2: Write Cargo.toml**

```toml
[package]
name = "codeweb"
version = "0.1.0"
edition = "2021"
description = "Semantic code graph analyzer — call graphs for SQL stored procedures"

[dependencies]
ogsql-parser = { git = "https://github.com/c2j/ogsql-parser" }
petgraph = "0.7"
thiserror = "2"
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
walkdir = "2"

[dev-dependencies]
# none yet
```

**Step 3: Write minimal main.rs**

```rust
fn main() {
    println!("codeweb — semantic code graph analyzer");
}
```

**Step 4: Verify build**

Run: `cargo build`
Expected: SUCCESS (downloads ogsql-parser + all deps)

**Step 5: Commit**

```bash
git init && git add -A && git commit -m "feat: initialize codeweb cargo project with dependencies"
```

---

## Task 2: Error Types

**Files:**
- Create: `src/error.rs`

**Step 1: Write error types**

```rust
use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum CodeWebError {
    #[error("failed to read file {path}: {source}")]
    FileRead {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("SQL parse error in {file}:{line}: {message}")]
    ParseError {
        file: PathBuf,
        line: usize,
        message: String,
    },

    #[error("no SQL files found in {path}")]
    NoFilesFound { path: PathBuf },

    #[error("export error: {0}")]
    ExportError(String),
}

pub type Result<T> = std::result::Result<T, CodeWebError>;
```

**Step 2: Verify compilation**

Run: `cargo build`
Expected: SUCCESS

**Step 3: Commit**

```bash
git add src/error.rs && git commit -m "feat: add error types with thiserror"
```

---

## Task 3: Graph Model

**Files:**
- Create: `src/graph/mod.rs`

**Step 1: Write graph types**

```rust
use petgraph::stable_graph::{DiGraph, NodeIndex, EdgeIndex};
use serde::Serialize;
use std::path::PathBuf;

/// Unique identifier for a stored procedure: (schema, name).
#[derive(Debug, Clone, Hash, Eq, PartialEq, Serialize)]
pub struct ProcedureId {
    pub schema: Option<String>,
    pub name: String,
}

impl ProcedureId {
    pub fn new(schema: Option<String>, name: String) -> Self {
        Self { schema, name }
    }

    /// Parse "schema.proc" or just "proc" into a ProcedureId.
    pub fn from_qualified_name(name: &str) -> Self {
        if let Some((s, n)) = name.split_once('.') {
            Self {
                schema: Some(s.to_string()),
                name: n.to_string(),
            }
        } else {
            Self {
                schema: None,
                name: name.to_string(),
            }
        }
    }

    pub fn display_name(&self) -> String {
        match &self.schema {
            Some(s) => format!("{}.{}", s, self.name),
            None => self.name.clone(),
        }
    }
}

/// Source location in a file.
#[derive(Debug, Clone, Serialize)]
pub struct SourceLocation {
    pub file: PathBuf,
    pub line: usize,
}

/// Graph node: a stored procedure or an unresolved call target.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum Node {
    Procedure {
        id: ProcedureId,
        location: SourceLocation,
    },
    Unresolved {
        raw_expr: String,
        context: String,
    },
}

/// Edge kind between two nodes.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind")]
pub enum Edge {
    DirectCall {
        location: SourceLocation,
    },
    DynamicCall {
        raw_expr: String,
        location: SourceLocation,
    },
}

/// The main code graph.
pub type CodeGraph = DiGraph<Node, Edge>;
```

**Step 2: Add `mod graph;` to main.rs temporarily, verify build**

Update `src/main.rs`:
```rust
mod graph;

fn main() {
    println!("codeweb — semantic code graph analyzer");
}
```

Run: `cargo build`
Expected: SUCCESS

**Step 3: Commit**

```bash
git add src/graph/ src/main.rs && git commit -m "feat: add graph model types — Node, Edge, CodeGraph, ProcedureId"
```

---

## Task 4: Call Extractor — Core Visitor

**Files:**
- Create: `src/parser/mod.rs`
- Create: `src/parser/extractor.rs`

**Step 1: Write the call extractor**

This is the core component. It implements `ogsql_parser::Visitor` to walk SQL AST and extract procedure call edges.

```rust
// src/parser/extractor.rs
use std::path::PathBuf;

use ogsql_parser::{
    ast::{
        plpgsql::{PlProcedureCall, PlStatement},
        CallFuncStatement, Statement, VisitorResult,
    },
    Visitor,
};

use crate::graph::{Edge, ProcedureId, SourceLocation};

/// A call relationship extracted from SQL.
#[derive(Debug, Clone)]
pub struct CallEdge {
    pub caller: Option<ProcedureId>,
    pub callee_name: String,
    pub is_dynamic: bool,
    pub location: SourceLocation,
}

/// Extracts procedure call relationships by walking the SQL AST.
pub struct CallExtractor {
    /// Current procedure being visited (None when not inside a CREATE PROCEDURE/FUNCTION).
    current_procedure: Option<ProcedureId>,
    /// Collected call edges.
    pub edges: Vec<CallEdge>,
    /// Source file being analyzed.
    file: PathBuf,
}

impl CallExtractor {
    pub fn new(file: PathBuf) -> Self {
        Self {
            current_procedure: None,
            edges: Vec::new(),
            file,
        }
    }

    /// Extract procedure name from a CREATE PROCEDURE/FUNCTION statement.
    fn extract_procedure_name(stmt: &Statement) -> Option<ProcedureId> {
        match stmt {
            Statement::CreateProcedure(p) => {
                let name_str = p.name.to_string();
                Some(ProcedureId::from_qualified_name(&name_str))
            }
            Statement::CreateFunction(f) => {
                let name_str = f.name.to_string();
                Some(ProcedureId::from_qualified_name(&name_str))
            }
            _ => None,
        }
    }

    fn record_call(&mut self, callee_name: &str, is_dynamic: bool, line: usize) {
        self.edges.push(CallEdge {
            caller: self.current_procedure.clone(),
            callee_name: callee_name.to_string(),
            is_dynamic,
            location: SourceLocation {
                file: self.file.clone(),
                line,
            },
        });
    }
}

impl Visitor for CallExtractor {
    fn visit_statement(&mut self, stmt: &Statement) -> VisitorResult {
        // Track current procedure context
        if let Some(id) = Self::extract_procedure_name(stmt) {
            self.current_procedure = Some(id);
        }
        VisitorResult::Continue
    }

    fn visit_call(&mut self, call: &CallFuncStatement) -> VisitorResult {
        let name = call.func_name.to_string();
        self.record_call(&name, false, 0);
        VisitorResult::Continue
    }

    fn visit_procedure_call(&mut self, call: &PlProcedureCall) -> VisitorResult {
        let name = call.name.to_string();
        self.record_call(&name, false, 0);
        VisitorResult::Continue
    }

    fn visit_pl_statement(&mut self, stmt: &PlStatement) -> VisitorResult {
        // Handle EXECUTE with dynamic SQL
        if let PlStatement::Execute(exec_stmt) = stmt {
            if exec_stmt.parsed_query.is_none() {
                // Dynamic SQL that couldn't be parsed — record as dynamic call
                self.record_call(&exec_stmt.string_expr.to_string(), true, 0);
            }
        }
        VisitorResult::Continue
    }
}
```

**Step 2: Write parser module**

```rust
// src/parser/mod.rs
pub mod extractor;

pub use extractor::CallExtractor;
```

**Step 3: Update main.rs**

```rust
mod error;
mod graph;
mod parser;

fn main() {
    println!("codeweb — semantic code graph analyzer");
}
```

**Step 4: Build and verify**

Run: `cargo build`
Expected: SUCCESS (ogsql-parser Visitor trait is implemented correctly)

**Step 5: Commit**

```bash
git add src/parser/ src/main.rs && git commit -m "feat: add CallExtractor — Visitor impl for procedure call extraction"
```

---

## Task 5: File Loader

**Files:**
- Create: `src/parser/loader.rs`

**Step 1: Write file loader**

```rust
// src/parser/loader.rs
use std::path::{Path, PathBuf};

use ogsql_parser::{Parser, Tokenizer};
use walkdir::WalkDir;

use crate::error::{CodeWebError, Result};

/// A parsed SQL file with its path and statements.
pub struct ParsedFile {
    pub path: PathBuf,
    pub statements: Vec<ogsql_parser::ast::StatementInfo>,
}

/// Load and parse all SQL files from a path (file or directory).
pub fn load_sql_files(path: &Path) -> Result<Vec<ParsedFile>> {
    let files = collect_sql_files(path)?;
    if files.is_empty() {
        return Err(CodeWebError::NoFilesFound {
            path: path.to_path_buf(),
        });
    }

    let mut parsed = Vec::new();
    for file_path in files {
        match parse_file(&file_path) {
            Ok(stmts) => parsed.push(ParsedFile {
                path: file_path,
                statements: stmts,
            }),
            Err(e) => {
                // Log error but continue with other files
                eprintln!("Warning: failed to parse {}: {}", file_path.display(), e);
            }
        }
    }

    Ok(parsed)
}

fn collect_sql_files(path: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    if path.is_file() {
        if is_sql_file(path) {
            files.push(path.to_path_buf());
        }
    } else if path.is_dir() {
        for entry in WalkDir::new(path).into_iter().filter_map(|e| e.ok()) {
            let p = entry.path();
            if p.is_file() && is_sql_file(p) {
                files.push(p.to_path_buf());
            }
        }
    }

    Ok(files)
}

fn is_sql_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("sql"))
}

fn parse_file(path: &Path) -> Result<Vec<ogsql_parser::ast::StatementInfo>> {
    let content = std::fs::read_to_string(path).map_err(|e| CodeWebError::FileRead {
        path: path.to_path_buf(),
        source: e,
    })?;

    let tokens = Tokenizer::new(&content)
        .tokenize()
        .map_err(|e| CodeWebError::ParseError {
            file: path.to_path_buf(),
            line: 0,
            message: e.to_string(),
        })?;

    let statements = Parser::new(tokens)
        .parse()
        .map_err(|e| CodeWebError::ParseError {
            file: path.to_path_buf(),
            line: 0,
            message: e.to_string(),
        })?;

    Ok(statements)
}
```

**Step 2: Update parser/mod.rs**

Add `pub mod loader;` and `pub use loader::{ParsedFile, load_sql_files};`

**Step 3: Build**

Run: `cargo build`
Expected: SUCCESS

**Step 4: Commit**

```bash
git add src/parser/ && git commit -m "feat: add SQL file loader — directory scanning and parsing"
```

---

## Task 6: Graph Builder

**Files:**
- Create: `src/graph/builder.rs`

**Step 1: Write graph builder**

```rust
// src/graph/builder.rs
use std::collections::HashMap;

use crate::parser::loader::ParsedFile;
use crate::parser::CallExtractor;
use ogsql_parser::{walk_statement, VisitorResult};

use super::{CodeGraph, Edge, Node, ProcedureId, SourceLocation};

/// Builds a CodeGraph from parsed SQL files.
pub struct GraphBuilder {
    graph: CodeGraph,
    /// Maps ProcedureId to node index for deduplication.
    node_map: HashMap<ProcedureId, petgraph::graph::NodeIndex>,
    /// Maps unresolved call target strings to node index.
    unresolved_map: HashMap<String, petgraph::graph::NodeIndex>,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self {
            graph: CodeGraph::new(),
            node_map: HashMap::new(),
            unresolved_map: HashMap::new(),
        }
    }

    /// Build the graph from a list of parsed SQL files.
    pub fn build(mut self, parsed_files: &[ParsedFile]) -> CodeGraph {
        // Phase 1: Extract all call edges from all files
        let mut all_edges = Vec::new();
        for parsed_file in parsed_files {
            let mut extractor = CallExtractor::new(parsed_file.path.clone());
            for stmt_info in &parsed_file.statements {
                walk_statement(&mut extractor, &stmt_info.statement);
            }
            all_edges.push((parsed_file.path.clone(), extractor.edges));
        }

        // Phase 2: Create nodes for all procedures that have definitions
        // (we need to do a first pass to find all defined procedures)
        for parsed_file in parsed_files {
            for stmt_info in &parsed_file.statements {
                if let Some(id) = get_procedure_id(&stmt_info.statement) {
                    self.get_or_create_procedure_node(
                        &id,
                        SourceLocation {
                            file: parsed_file.path.clone(),
                            line: 0,
                        },
                    );
                }
            }
        }

        // Phase 3: Create edges from extracted calls
        for (_file, edges) in all_edges {
            for edge in edges {
                let caller_idx = edge.caller.as_ref().and_then(|id| self.node_map.get(id).copied());

                let callee_id = ProcedureId::from_qualified_name(&edge.callee_name);
                let callee_idx = if edge.is_dynamic {
                    self.get_or_create_unresolved_node(&edge.callee_name, &edge.callee_name)
                } else {
                    self.get_or_create_procedure_node(
                        &callee_id,
                        SourceLocation {
                            file: edge.location.file.clone(),
                            line: 0,
                        },
                    )
                };

                if let Some(caller_idx) = caller_idx {
                    let graph_edge = if edge.is_dynamic {
                        Edge::DynamicCall {
                            raw_expr: edge.callee_name.clone(),
                            location: edge.location,
                        }
                    } else {
                        Edge::DirectCall {
                            location: edge.location,
                        }
                    };
                    self.graph.add_edge(caller_idx, callee_idx, graph_edge);
                }
            }
        }

        self.graph
    }

    fn get_or_create_procedure_node(
        &mut self,
        id: &ProcedureId,
        location: SourceLocation,
    ) -> petgraph::graph::NodeIndex {
        if let Some(&idx) = self.node_map.get(id) {
            return idx;
        }
        let idx = self.graph.add_node(Node::Procedure {
            id: id.clone(),
            location,
        });
        self.node_map.insert(id.clone(), idx);
        idx
    }

    fn get_or_create_unresolved_node(
        &mut self,
        raw_expr: &str,
        context: &str,
    ) -> petgraph::graph::NodeIndex {
        if let Some(&idx) = self.unresolved_map.get(raw_expr) {
            return idx;
        }
        let idx = self.graph.add_node(Node::Unresolved {
            raw_expr: raw_expr.to_string(),
            context: context.to_string(),
        });
        self.unresolved_map.insert(raw_expr.to_string(), idx);
        idx
    }
}

fn get_procedure_id(stmt: &ogsql_parser::ast::Statement) -> Option<ProcedureId> {
    use ogsql_parser::ast::Statement;
    match stmt {
        Statement::CreateProcedure(p) => {
            Some(ProcedureId::from_qualified_name(&p.name.to_string()))
        }
        Statement::CreateFunction(f) => {
            Some(ProcedureId::from_qualified_name(&f.name.to_string()))
        }
        Statement::Do(d) => {
            // DO blocks are anonymous — create a synthetic node if needed
            None
        }
        Statement::AnonyBlock(_) => None,
        _ => None,
    }
}
```

**Step 2: Update graph/mod.rs**

Add `pub mod builder;` and `pub use builder::GraphBuilder;`

**Step 3: Build**

Run: `cargo build`
Expected: SUCCESS

**Step 4: Commit**

```bash
git add src/graph/ && git commit -m "feat: add GraphBuilder — constructs petgraph from extracted call edges"
```

---

## Task 7: Export — DOT Format

**Files:**
- Create: `src/export/mod.rs`
- Create: `src/export/dot.rs`

**Step 1: Write DOT exporter**

```rust
// src/export/dot.rs
use crate::graph::{CodeGraph, Edge, Node};

pub fn to_dot(graph: &CodeGraph) -> String {
    let mut out = String::from("digraph G {\n");
    out.push_str("  rankdir=LR;\n");
    out.push_str("  node [shape=box, style=filled, fillcolor=white];\n\n");

    // Nodes
    for idx in graph.node_indices() {
        let node = &graph[idx];
        let dot_id = dot_node_id(idx);
        match node {
            Node::Procedure { id, .. } => {
                let label = id.display_name();
                out.push_str(&format!(
                    "  {} [label=\"{}\"];\n",
                    dot_id,
                    dot_escape(&label)
                ));
            }
            Node::Unresolved { raw_expr, .. } => {
                out.push_str(&format!(
                    "  {} [label=\"{}\", style=\"dashed\", fillcolor=lightyellow];\n",
                    dot_id,
                    dot_escape(&format!("unresolved: {}", raw_expr))
                ));
            }
        }
    }

    out.push('\n');

    // Edges
    for edge_idx in graph.edge_indices() {
        let (src, dst) = graph.edge_endpoints(edge_idx).unwrap();
        let edge = &graph[edge_idx];
        let label = match edge {
            Edge::DirectCall { .. } => "CALL".to_string(),
            Edge::DynamicCall { .. } => "EXECUTE".to_string(),
        };
        let style = match edge {
            Edge::DirectCall { .. } => "solid",
            Edge::DynamicCall { .. } => "dashed",
        };
        out.push_str(&format!(
            "  {} -> {} [label=\"{}\", style={}];\n",
            dot_node_id(src),
            dot_node_id(dst),
            label,
            style,
        ));
    }

    out.push_str("}\n");
    out
}

fn dot_node_id(idx: petgraph::graph::NodeIndex) -> String {
    format!("n{}", idx.index())
}

fn dot_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
}
```

**Step 2: Write export module**

```rust
// src/export/mod.rs
pub mod dot;

pub use dot::to_dot;
```

**Step 3: Build**

Run: `cargo build`
Expected: SUCCESS

**Step 4: Commit**

```bash
git add src/export/ && git commit -m "feat: add DOT (Graphviz) export"
```

---

## Task 8: Export — JSON Format

**Files:**
- Create: `src/export/json.rs`

**Step 1: Write JSON exporter**

```rust
// src/export/json.rs
use serde::Serialize;

use crate::graph::{CodeGraph, Edge, Node};

#[derive(Serialize)]
struct JsonGraph {
    nodes: Vec<JsonNode>,
    edges: Vec<JsonEdge>,
}

#[derive(Serialize)]
struct JsonNode {
    index: usize,
    #[serde(flatten)]
    node: Node,
}

#[derive(Serialize)]
struct JsonEdge {
    from: usize,
    to: usize,
    #[serde(flatten)]
    edge: Edge,
}

pub fn to_json(graph: &CodeGraph) -> serde_json::Result<String> {
    let nodes: Vec<JsonNode> = graph
        .node_indices()
        .map(|idx| JsonNode {
            index: idx.index(),
            node: graph[idx].clone(),
        })
        .collect();

    let edges: Vec<JsonEdge> = graph
        .edge_indices()
        .map(|idx| {
            let (src, dst) = graph.edge_endpoints(idx).unwrap();
            JsonEdge {
                from: src.index(),
                to: dst.index(),
                edge: graph[idx].clone(),
            }
        })
        .collect();

    serde_json::to_string_pretty(&JsonGraph { nodes, edges })
}
```

**Step 2: Update export/mod.rs**

Add `pub mod json;` and `pub use json::to_json;`

**Step 3: Build**

Run: `cargo build`
Expected: SUCCESS

**Step 4: Commit**

```bash
git add src/export/ && git commit -m "feat: add JSON export"
```

---

## Task 9: Export — Mermaid Format

**Files:**
- Create: `src/export/mermaid.rs`

**Step 1: Write Mermaid exporter**

```rust
// src/export/mermaid.rs
use crate::graph::{CodeGraph, Edge, Node};

pub fn to_mermaid(graph: &CodeGraph) -> String {
    let mut out = String::from("graph LR\n");

    // Nodes
    for idx in graph.node_indices() {
        let node = &graph[idx];
        let mermaid_id = mermaid_node_id(idx);
        match node {
            Node::Procedure { id, .. } => {
                out.push_str(&format!(
                    "  {}[\"{}\"]\n",
                    mermaid_id,
                    id.display_name()
                ));
            }
            Node::Unresolved { raw_expr, .. } => {
                out.push_str(&format!(
                    "  {}{{\"unresolved: {}\"}}\n",
                    mermaid_id,
                    mermaid_escape(raw_expr)
                ));
            }
        }
    }

    // Edges
    for edge_idx in graph.edge_indices() {
        let (src, dst) = graph.edge_endpoints(edge_idx).unwrap();
        let edge = &graph[edge_idx];
        let label = match edge {
            Edge::DirectCall { .. } => "CALL",
            Edge::DynamicCall { .. } => "EXECUTE",
        };
        let style = match edge {
            Edge::DynamicCall { .. } => " -.-> ",
            _ => " --> ",
        };
        out.push_str(&format!(
            "  {}{}{}|\"{}\"|{}\n",
            mermaid_node_id(src),
            style,
            mermaid_node_id(dst),
            label,
            "" // mermaid doesn't need trailing comma
        ));
    }

    out
}

fn mermaid_node_id(idx: petgraph::graph::NodeIndex) -> String {
    format!("N{}", idx.index())
}

fn mermaid_escape(s: &str) -> String {
    s.replace('"', "&quot;")
}
```

**Step 2: Update export/mod.rs**

Add `pub mod mermaid;` and `pub use mermaid::to_mermaid;`

**Step 3: Build**

Run: `cargo build`
Expected: SUCCESS

**Step 4: Commit**

```bash
git add src/export/ && git commit -m "feat: add Mermaid export"
```

---

## Task 10: CLI Integration

**Files:**
- Rewrite: `src/main.rs`

**Step 1: Write CLI with clap**

```rust
use std::path::PathBuf;

use clap::{Parser, ValueEnum};

mod error;
mod export;
mod graph;
mod parser;

#[derive(Parser)]
#[command(name = "codeweb")]
#[command(about = "Semantic code graph analyzer")]
struct Cli {
    /// Input file or directory containing SQL files
    input: PathBuf,

    /// Output format
    #[arg(short, long, default_value = "dot")]
    format: OutputFormat,

    /// Output file path (stdout if not specified)
    #[arg(short, long)]
    output: Option<PathBuf>,

    /// Include unresolved/dynamic call targets in the graph
    #[arg(long)]
    include_unresolved: bool,
}

#[derive(Clone, ValueEnum)]
enum OutputFormat {
    Dot,
    Json,
    Mermaid,
}

fn main() {
    let cli = Cli::parse();

    if let Err(e) = run(cli) {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> error::Result<()> {
    // Load and parse SQL files
    let parsed_files = parser::load_sql_files(&cli.input)?;

    // Build graph
    let builder = graph::GraphBuilder::new();
    let graph = builder.build(&parsed_files);

    // Export
    let output = match cli.format {
        OutputFormat::Dot => export::to_dot(&graph),
        OutputFormat::Json => export::to_json(&graph)
            .map_err(|e| error::CodeWebError::ExportError(e.to_string()))?,
        OutputFormat::Mermaid => export::to_mermaid(&graph),
    };

    // Write output
    match cli.output {
        Some(path) => {
            std::fs::write(&path, &output).map_err(|e| error::CodeWebError::FileRead {
                path: path.clone(),
                source: e,
            })?;
            eprintln!("Graph written to {}", path.display());
        }
        None => println!("{}", output),
    }

    // Print stats
    let proc_count = graph.node_indices().filter(|idx| matches!(&graph[*idx], graph::Node::Procedure { .. })).count();
    let edge_count = graph.edge_indices().count();
    eprintln!("Nodes: {}, Edges: {}", proc_count, edge_count);

    Ok(())
}
```

**Step 2: Build**

Run: `cargo build`
Expected: SUCCESS

**Step 3: Commit**

```bash
git add src/main.rs && git commit -m "feat: add CLI with clap — analyze, export to dot/json/mermaid"
```

---

## Task 11: Integration Test

**Files:**
- Create: `tests/integration_test.rs`

**Step 1: Write integration test with sample SQL**

```rust
use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn create_test_sql(dir: &std::path::Path) -> PathBuf {
    let sql = r#"
CREATE OR REPLACE PROCEDURE pkg_users.get_user(p_id INT)
AS $$
BEGIN
    pkg_users.log_access(p_id);
    PERFORM pkg_users.validate_id(p_id);
    RETURN QUERY SELECT * FROM users WHERE id = p_id;
END;
$$;

CREATE OR REPLACE PROCEDURE pkg_users.log_access(p_id INT)
AS $$
BEGIN
    INSERT INTO access_log (user_id, ts) VALUES (p_id, NOW());
END;
$$;

CREATE OR REPLACE PROCEDURE pkg_users.validate_id(p_id INT)
AS $$
BEGIN
    IF p_id IS NULL THEN
        RAISE EXCEPTION 'invalid id';
    END IF;
END;
$$;
"#;
    let file_path = dir.join("test_procedures.sql");
    fs::write(&file_path, sql).unwrap();
    file_path
}

// Note: This test requires ogsql-parser to parse the SQL correctly.
// If the parser doesn't support certain GaussDB syntax, simplify the SQL.
#[test]
fn test_full_pipeline() {
    let dir = tempfile::tempdir().unwrap();
    create_test_sql(dir.path());

    let parsed = codeweb::parser::load_sql_files(dir.path()).unwrap();
    assert_eq!(parsed.len(), 1);

    let builder = codeweb::graph::GraphBuilder::new();
    let graph = builder.build(&parsed);

    // Should have 3 procedure nodes
    let proc_nodes: Vec<_> = graph
        .node_indices()
        .filter(|idx| matches!(&graph[*idx], codeweb::graph::Node::Procedure { .. }))
        .collect();
    assert!(proc_nodes.len() >= 2, "Expected at least 2 procedure nodes, got {}", proc_nodes.len());

    // Should have call edges
    let edge_count = graph.edge_indices().count();
    assert!(edge_count >= 1, "Expected at least 1 call edge, got {}", edge_count);

    // DOT export should produce valid output
    let dot = codeweb::export::to_dot(&graph);
    assert!(dot.starts_with("digraph G"));
    assert!(dot.contains("pkg_users"));
}
```

**Step 2: Add tempfile dev-dependency to Cargo.toml**

```toml
[dev-dependencies]
tempfile = "3"
```

**Step 3: Run tests**

Run: `cargo test`
Expected: PASS (or clarify parser limitations and adjust SQL)

**Step 4: Commit**

```bash
git add tests/ Cargo.toml && git commit -m "test: add integration test for full pipeline"
```

---

## Task 12: Verify and Polish

**Step 1: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: PASS — fix any warnings

**Step 2: Run fmt check**

Run: `cargo fmt -- --check`
Expected: PASS — run `cargo fmt` if needed

**Step 3: Run all tests**

Run: `cargo test`
Expected: ALL PASS

**Step 4: Manual end-to-end test**

```bash
# Create a test SQL file and run
echo "CREATE PROCEDURE hello() AS \$\$ BEGIN world(); END; \$\$;
CREATE PROCEDURE world() AS \$\$ BEGIN NULL; END; \$\$;" > /tmp/test.sql
cargo run -- /tmp/test.sql --format dot
cargo run -- /tmp/test.sql --format json
cargo run -- /tmp/test.sql --format mermaid
```

Expected: Each format produces valid output with nodes and edges.

**Step 5: Final commit**

```bash
git add -A && git commit -m "chore: clippy + fmt + final cleanup for Phase 1"
```

---

## Summary

| Task | Content | Est. Time |
|------|---------|-----------|
| 1 | Initialize Cargo project | 5 min |
| 2 | Error types | 5 min |
| 3 | Graph model | 10 min |
| 4 | Call extractor (Visitor) | 20 min |
| 5 | File loader | 10 min |
| 6 | Graph builder | 15 min |
| 7 | DOT export | 10 min |
| 8 | JSON export | 10 min |
| 9 | Mermaid export | 10 min |
| 10 | CLI integration | 15 min |
| 11 | Integration test | 15 min |
| 12 | Verify and polish | 10 min |
| **Total** | | **~2 hours** |
