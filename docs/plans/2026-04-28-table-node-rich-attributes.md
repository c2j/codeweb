# Table Node Rich Attributes — Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Extend the `Table` node from bare `schema+name` to a rich object with source location, column metadata, partition/distribution info, and DDL source. Make these attributes visible in `cmd_detail`, JSON export, TUI detail panel, and queryable via CLI filters. Also add `global` field to `Index` node for partition/global index distinction.

**Architecture:** Three-layer change: (1) Model layer — `Node::Table` gains new fields; new helper types `ColumnSummary`, `PartitionInfo`, `DistributeInfo`. (2) Builder layer — `GraphBuilder` handles `Statement::CreateTable` from ogsql-parser, merging with implicit table nodes. (3) Presentation layer — `cmd_detail`, JSON export, TUI detail panel, and CLI filter all updated to show/use the new attributes.

**Tech Stack:** Rust, ogsql-parser (already has `CreateTableStatement` with full column/partition/distribution AST), petgraph, serde, ratatui.

**Prerequisite verified:** ogsql-parser `CreateTableStatement` provides: `columns: Vec<ColumnDef>`, `constraints: Vec<TableConstraint>`, `partition_by: Option<PartitionClause>`, `subpartition_by: Option<PartitionClause>`, `distribute_by: Option<DistributeClause>`, `tablespace: Option<String>`, `temporary: bool`, `unlogged: bool`. All needed data is available.

---

## Phase 0 (P0): Model + Builder + Basic Visibility

### Task 1: Add new types and extend `Node::Table` in graph model

**Files:**
- Modify: `src/graph/mod.rs` (Node enum, new types)

**Step 1: Write the failing tests**

Add tests in `src/graph/mod.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn table_node_with_location_and_columns() {
    let file = Arc::new(PathBuf::from("create_tables.sql"));
    let table = Node::Table {
        schema: Some("public".to_string()),
        name: "orders".to_string(),
        location: Some(SourceLocation {
            file: file.clone(),
            line: 10,
        }),
        columns: vec![
            ColumnSummary {
                name: "id".to_string(),
                data_type: "INTEGER".to_string(),
                nullable: false,
                is_primary_key: true,
                default_value: None,
                comment: None,
            },
            ColumnSummary {
                name: "amount".to_string(),
                data_type: "NUMERIC(10,2)".to_string(),
                nullable: true,
                is_primary_key: false,
                default_value: Some("0".to_string()),
                comment: Some("order amount".to_string()),
            },
        ],
        partition_by: Some(PartitionInfo::Range {
            columns: vec!["created_at".to_string()],
            partitions: vec!["p_2024".to_string(), "p_2025".to_string()],
        }),
        distribute_by: Some(DistributeInfo::Hash(vec!["id".to_string()])),
        tablespace: Some("pg_default".to_string()),
        temporary: false,
        unlogged: false,
        ddl_source: Some("CREATE TABLE public.orders (...)".to_string()),
    };
    assert_eq!(table.file(), Path::new("create_tables.sql"));
}

#[test]
fn table_node_backward_compatible_minimal() {
    // Old-style Table with only schema+name should still work (all new fields Optional/defaulted)
    let table = Node::Table {
        schema: None,
        name: "my_table".to_string(),
        location: None,
        columns: vec![],
        partition_by: None,
        distribute_by: None,
        tablespace: None,
        temporary: false,
        unlogged: false,
        ddl_source: None,
    };
    assert_eq!(table.file(), Path::new("")); // No file → empty path
}

#[test]
fn partition_info_serialization_roundtrip() {
    let info = PartitionInfo::Range {
        columns: vec!["created_at".to_string()],
        partitions: vec!["p_2024".to_string()],
    };
    let json = serde_json::to_string(&info).unwrap();
    let de: PartitionInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(info, de);
}

#[test]
fn distribute_info_serialization_roundtrip() {
    let info = DistributeInfo::Hash(vec!["user_id".to_string()]);
    let json = serde_json::to_string(&info).unwrap();
    let de: DistributeInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(info, de);
}

#[test]
fn column_summary_serialization_roundtrip() {
    let col = ColumnSummary {
        name: "id".to_string(),
        data_type: "BIGINT".to_string(),
        nullable: false,
        is_primary_key: true,
        default_value: Some("nextval('seq')".to_string()),
        comment: None,
    };
    let json = serde_json::to_string(&col).unwrap();
    let de: ColumnSummary = serde_json::from_str(&json).unwrap();
    assert_eq!(col, de);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --lib graph::tests::table_node_with_location -- --nocapture`
Expected: FAIL — types `ColumnSummary`, `PartitionInfo`, `DistributeInfo` don't exist yet; `Node::Table` doesn't have new fields.

**Step 3: Implement the model changes**

In `src/graph/mod.rs`, add the new types BEFORE the `Node` enum:

```rust
/// Lightweight column summary for Table nodes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnSummary {
    pub name: String,
    pub data_type: String,
    pub nullable: bool,
    pub is_primary_key: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Partition strategy for a table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum PartitionInfo {
    Range {
        columns: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        partitions: Vec<String>,
    },
    List {
        columns: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        partitions: Vec<String>,
    },
    Hash {
        columns: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        partitions_count: Option<u32>,
    },
}

/// Distribution strategy for a distributed table (openGauss/GaussDB).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "strategy", rename_all = "snake_case")]
pub enum DistributeInfo {
    Hash(Vec<String>),
    Replication,
    RoundRobin(Vec<String>),
    Modulo(Vec<String>),
}
```

Change `Node::Table` variant from:

```rust
Table {
    schema: Option<String>,
    name: String,
},
```

to:

```rust
Table {
    schema: Option<String>,
    name: String,
    /// Source location of the CREATE TABLE statement.
    /// None when table node was created implicitly (referenced but not parsed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    location: Option<SourceLocation>,
    /// Column summaries extracted from CREATE TABLE.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    columns: Vec<ColumnSummary>,
    /// Partition strategy (RANGE/LIST/HASH).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    partition_by: Option<PartitionInfo>,
    /// Distribution strategy (openGauss/GaussDB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    distribute_by: Option<DistributeInfo>,
    /// Tablespace name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    tablespace: Option<String>,
    /// Whether this is a temporary table.
    #[serde(default, skip_serializing_if = "is_false")]
    temporary: bool,
    /// Whether this is an unlogged table.
    #[serde(default, skip_serializing_if = "is_false")]
    unlogged: bool,
    /// Original DDL text (CREATE TABLE statement).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ddl_source: Option<String>,
},
```

Update `Node::file()` for Table:

```rust
Node::Table { location, .. } => location
    .as_ref()
    .map(|l| l.file.as_path())
    .unwrap_or(Path::new("")),
```

Update `Node::View` similarly (add `location: Option<SourceLocation>` — View already has `CreateView` handling in builder):

```rust
View {
    schema: Option<String>,
    name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    location: Option<SourceLocation>,
},
```

And its `file()`:
```rust
Node::View { location, .. } => location
    .as_ref()
    .map(|l| l.file.as_path())
    .unwrap_or(Path::new("")),
```

Add `is_false` helper if not already present in `graph/mod.rs`:
```rust
fn is_false(v: &bool) -> bool {
    !v
}
```

**Step 4: Run tests to verify they pass**

Run: `cargo test --lib graph::tests`
Expected: ALL PASS (including new tests and existing tests — existing Table construction sites will need `..Default-like fields` — BUT Rust enum variants don't support `Default`. All existing construction sites must be updated).

**Step 5: Fix all existing `Node::Table` construction sites**

Every `Node::Table { schema, name }` in the codebase must be updated to include the new fields. There are ~10 sites:

- `src/graph/builder.rs` — multiple sites creating implicit Table nodes (lines ~272, ~364, ~406, ~1375)
- `src/graph/resolver.rs` — `make_table_node` helper (line ~427)
- `src/graph/mod.rs` — tests (lines ~531+)
- `src/import/parser.rs` — import path (line ~203)

For **implicit** Table nodes (created when referenced, not from DDL), set all new fields to `None`/`false`/`vec![]`:

```rust
Node::Table {
    schema: access.schema.clone(),
    name: access.name.clone(),
    location: None,
    columns: vec![],
    partition_by: None,
    distribute_by: None,
    tablespace: None,
    temporary: false,
    unlogged: false,
    ddl_source: None,
}
```

**Step 6: Run full test suite**

Run: `cargo test`
Expected: ALL PASS

**Step 7: Run clippy**

Run: `cargo clippy -- -D warnings`
Expected: CLEAN

---

### Task 2: Add `Statement::CreateTable` handling in GraphBuilder

**Files:**
- Modify: `src/graph/builder.rs` (new match arm in `create_sql_nodes`)

**Step 1: Write the failing integration test**

Add in `src/graph/builder.rs` `#[cfg(test)] mod tests`:

```rust
#[test]
fn create_table_produces_rich_table_node() {
    use ogsql_parser::parse_sql;

    let sql = r#"
        CREATE TABLE public.orders (
            id BIGINT NOT NULL PRIMARY KEY,
            amount NUMERIC(10,2) DEFAULT 0,
            status VARCHAR(20),
            created_at TIMESTAMP NOT NULL
        ) PARTITION BY RANGE (created_at)
        DISTRIBUTE BY HASH (id)
        TABLESPACE pg_default;
    "#;

    let stmts = parse_sql(sql).unwrap();
    let parsed = crate::parser::ParsedFile {
        path: std::path::PathBuf::from("test.sql"),
        statements: stmts,
        source_hash: [0u8; 32],
    };

    let graph = GraphBuilder::new().build(&[parsed]);

    // Find the Table node
    let table_nodes: Vec<_> = graph.node_indices()
        .filter(|i| matches!(&graph[*i], Node::Table { .. }))
        .collect();

    assert_eq!(table_nodes.len(), 1, "should have exactly one table node");
    let table_node = &graph[table_nodes[0]];

    if let Node::Table { schema, name, columns, partition_by, distribute_by, tablespace, location, .. } = table_node {
        assert_eq!(schema.as_deref(), Some("public"));
        assert_eq!(name, "orders");
        assert!(location.is_some(), "should have source location");
        assert_eq!(columns.len(), 4);
        assert!(columns[0].is_primary_key);
        assert_eq!(columns[0].name, "id");
        assert!(partition_by.is_some());
        assert!(distribute_by.is_some());
        assert_eq!(tablespace.as_deref(), Some("pg_default"));
    } else {
        panic!("expected Table node");
    }
}

#[test]
fn create_table_merges_with_implicit_table_from_reference() {
    use ogsql_parser::parse_sql;

    // File 1: CREATE TABLE
    let sql1 = "CREATE TABLE public.my_table (id INT PRIMARY KEY);";
    // File 2: Procedure that references the table
    let sql2 = "CREATE PROCEDURE do_stuff() AS BEGIN INSERT INTO my_table(id) VALUES(1); END;";

    let stmts1 = parse_sql(sql1).unwrap();
    let stmts2 = parse_sql(sql2).unwrap();

    let parsed1 = crate::parser::ParsedFile {
        path: std::path::PathBuf::from("create.sql"),
        statements: stmts1,
        source_hash: [0u8; 32],
    };
    let parsed2 = crate::parser::ParsedFile {
        path: std::path::PathBuf::from("proc.sql"),
        statements: stmts2,
        source_hash: [0u8; 32],
    };

    let graph = GraphBuilder::new().build(&[parsed1, parsed2]);

    let table_nodes: Vec<_> = graph.node_indices()
        .filter(|i| matches!(&graph[*i], Node::Table { name, .. } if name == "my_table"))
        .collect();

    assert_eq!(table_nodes.len(), 1, "should merge into single table node");
    if let Node::Table { columns, location, .. } = &graph[table_nodes[0]] {
        assert!(!columns.is_empty(), "merged node should keep columns from CREATE TABLE");
        assert!(location.is_some(), "merged node should have location from CREATE TABLE");
    }
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --lib builder::tests::create_table_produces_rich_table_node`
Expected: FAIL — `Statement::CreateTable` not matched in builder.

**Step 3: Implement `Statement::CreateTable` in builder**

In `src/graph/builder.rs`, inside `create_sql_nodes`, add a new match arm after `Statement::CreateIndex`:

```rust
Statement::CreateTable(t) => {
    let (schema, name) = split_object_name(&t.name);
    let columns: Vec<ColumnSummary> = t.columns.iter().map(|c| {
        let is_pk = c.constraints.iter().any(|cc| matches!(cc, ColumnConstraint::PrimaryKey));
        let nullable = !c.constraints.iter().any(|cc| matches!(cc, ColumnConstraint::NotNull));
        let default_value = c.constraints.iter().find_map(|cc| {
            if let ColumnConstraint::Default(expr) = cc {
                Some(format!("{:?}", expr))
            } else {
                None
            }
        });
        ColumnSummary {
            name: c.name.clone(),
            data_type: format!("{:?}", c.data_type),
            nullable,
            is_primary_key: is_pk,
            default_value,
            comment: c.comment.clone(),
        }
    }).collect();

    let partition_by = t.partition_by.as_ref().map(|p| match p {
        PartitionClause::Range { columns, partitions, .. } => PartitionInfo::Range {
            columns: columns.iter().map(|c| c.join(".")).collect(),
            partitions: partitions.iter().map(|pd| pd.name.clone()).collect(),
        },
        PartitionClause::List { columns, partitions, .. } => PartitionInfo::List {
            columns: columns.iter().map(|c| c.join(".")).collect(),
            partitions: partitions.iter().map(|pd| pd.name.clone()).collect(),
        },
        PartitionClause::Hash { columns, partitions_count, .. } => PartitionInfo::Hash {
            columns: columns.iter().map(|c| c.join(".")).collect(),
            partitions_count: *partitions_count,
        },
    });

    let distribute_by = t.distribute_by.as_ref().map(|d| match d {
        DistributeClause::Hash { columns } => DistributeInfo::Hash(columns.clone()),
        DistributeClause::Replication => DistributeInfo::Replication,
        DistributeClause::RoundRobin { columns } => DistributeInfo::RoundRobin(columns.clone()),
        DistributeClause::Modulo { columns } => DistributeInfo::Modulo(columns.clone()),
    });

    let table_node = Node::Table {
        schema: schema.clone(),
        name: name.clone(),
        location: Some(SourceLocation {
            file: file_arc.clone(),
            line: info.start_line,
        }),
        columns,
        partition_by,
        distribute_by,
        tablespace: t.tablespace.clone(),
        temporary: t.temporary,
        unlogged: t.unlogged,
        ddl_source: None, // Not storing full DDL by default — can be added later
    };

    let idx = graph.add_node(table_node);
    let key = normalize_table_key(schema.as_deref(), &name);
    table_index.entry(key).or_insert(idx);
    if schema.is_some() {
        table_index.entry(name.to_lowercase()).or_insert(idx);
    }
}
```

**Important:** Add required imports at top of builder.rs:
```rust
use ogsql_parser::ast::{ColumnConstraint, DistributeClause, PartitionClause};
use crate::graph::{ColumnSummary, DistributeInfo, PartitionInfo};
```

**Step 4: Update `dedup_table_view_nodes` to merge attributes**

When an explicit Table node (from CREATE TABLE) and an implicit one (from table reference) coexist, the merge needs to keep the rich attributes. In the merge logic (`dedup_table_view_nodes`), the explicit node should win.

Update the merge to check if the target (view/explicit) node should get columns from the source:

```rust
// In the merge phase, if a Table node with location replaces one without,
// ensure the rich attributes are preserved.
```

The existing logic already keeps the View/MaterializedView node and removes the Table node. For Table→Table merges (two Table nodes for same table), we need new logic. Add a `dedup_table_nodes` step after `dedup_table_view_nodes`:

```rust
fn dedup_table_nodes(graph: &mut CodeGraph) {
    let mut best: HashMap<String, petgraph::graph::NodeIndex> = HashMap::new();
    let mut duplicates: Vec<petgraph::graph::NodeIndex> = Vec::new();

    for idx in graph.node_indices() {
        if let Node::Table { schema, name, location, columns, .. } = &graph[idx] {
            let key = normalize_table_key(schema.as_deref(), name);
            let has_rich_data = location.is_some() || !columns.is_empty();
            match best.get(&key) {
                None => { best.entry(key).or_insert(idx); }
                Some(&existing) => {
                    // Keep the richer node
                    let existing_rich = match &graph[existing] {
                        Node::Table { location, columns, .. } => location.is_some() || !columns.is_empty(),
                        _ => false,
                    };
                    if has_rich_data && !existing_rich {
                        duplicates.push(existing);
                        best.insert(key, idx);
                    } else {
                        duplicates.push(idx);
                    }
                }
            }
        }
    }

    // Reconnect edges from removed nodes
    for dup in &duplicates {
        let incoming: Vec<_> = graph.edges_directed(*dup, petgraph::Direction::Incoming)
            .map(|e| (e.source(), e.weight().clone()))
            .collect();
        let outgoing: Vec<_> = graph.edges_directed(*dup, petgraph::Direction::Outgoing)
            .map(|e| (e.target(), e.weight().clone()))
            .collect();

        if let Some(&keep) = None.or_else(|| None) {
            // find the kept node
        }
        // Simplified: just use best map to find replacement
        if let Node::Table { schema, name, .. } = &graph[*dup] {
            let key = normalize_table_key(schema.as_deref(), name);
            if let Some(&keep) = best.get(&key) {
                for (src, weight) in &incoming {
                    if !graph.edges_connecting(*src, keep).any(|e| std::mem::discriminant(e.weight()) == std::mem::discriminant(weight)) {
                        graph.add_edge(*src, keep, weight.clone());
                    }
                }
                for (tgt, weight) in &outgoing {
                    if !graph.edges_connecting(keep, *tgt).any(|e| std::mem::discriminant(e.weight()) == std::mem::discriminant(weight)) {
                        graph.add_edge(keep, *tgt, weight.clone());
                    }
                }
            }
        }
    }

    graph.retain_nodes(|_, idx| !duplicates.contains(&idx));
}
```

Call `Self::dedup_table_nodes(&mut graph)` after `Self::dedup_table_view_nodes(&mut graph)` in `build_graph_internal`.

**Step 5: Run tests**

Run: `cargo test --lib builder::tests::create_table`
Expected: ALL PASS

**Step 6: Run full suite + clippy**

Run: `cargo test && cargo clippy -- -D warnings`
Expected: ALL PASS, CLEAN

---

### Task 3: Update `cmd_detail` to show Table attributes

**Files:**
- Modify: `src/main.rs` (`cmd_detail` function)

**Step 1: Write the failing test**

```rust
// In main.rs tests or as a CLI integration test
// This is tested via the cmd_detail function output
```

Since `cmd_detail` prints to stdout, test via integration test or by extracting the formatting logic.

**Step 2: Implement**

In `cmd_detail`, after printing `tag + name + degree`, add a section for node-specific attributes:

```rust
fn print_node_details(node: &Node) {
    match node {
        Node::Table { schema, name, location, columns, partition_by, distribute_by, tablespace, temporary, unlogged, .. } => {
            if let Some(loc) = location {
                println!("  file: {}:{}", loc.file.to_string_lossy(), loc.line);
            } else {
                println!("  file: (implicit — no CREATE TABLE parsed)");
            }
            if *temporary { println!("  temporary: true"); }
            if *unlogged { println!("  unlogged: true"); }
            if let Some(ts) = tablespace {
                println!("  tablespace: {}", ts);
            }
            if !columns.is_empty() {
                println!("  columns ({}):", columns.len());
                for col in columns {
                    let pk = if col.is_primary_key { " [PK]" } else { "" };
                    let null = if col.nullable { "NULL" } else { "NOT NULL" };
                    let def = col.default_value.as_deref().map(|d| format!(" DEFAULT {}", d)).unwrap_or_default();
                    println!("    {} {} {}{}{}", col.name, col.data_type, null, pk, def);
                }
            }
            if let Some(part) = partition_by {
                let strategy = match part {
                    PartitionInfo::Range { columns, .. } => format!("RANGE({})", columns.join(", ")),
                    PartitionInfo::List { columns, .. } => format!("LIST({})", columns.join(", ")),
                    PartitionInfo::Hash { columns, .. } => format!("HASH({})", columns.join(", ")),
                };
                println!("  partition: {}", strategy);
            }
            if let Some(dist) = distribute_by {
                let strategy = match dist {
                    DistributeInfo::Hash(cols) => format!("HASH({})", cols.join(", ")),
                    DistributeInfo::Replication => "REPLICATION".to_string(),
                    DistributeInfo::RoundRobin(cols) => format!("ROUNDROBIN({})", cols.join(", ")),
                    DistributeInfo::Modulo(cols) => format!("MODULO({})", cols.join(", ")),
                };
                println!("  distribute: {}", strategy);
            }
        }
        // Other node types can be enhanced similarly later
        _ => {}
    }
}
```

Call `print_node_details(&graph[*start_idx])` after the degree line in `cmd_detail`.

**Step 3: Run tests + clippy**

Run: `cargo test && cargo clippy -- -D warnings`

---

### Task 4: Update JSON export/import for new Table fields

**Files:**
- Modify: `src/export/json.rs` (`NodeKindJson::Table`)
- Modify: `src/import/parser.rs` (Table import)

**Step 1: Write failing test for JSON export**

In `src/export/json.rs` tests:

```rust
#[test]
fn table_node_json_includes_columns_and_partition() {
    let file = Arc::new(PathBuf::from("test.sql"));
    let table = Node::Table {
        schema: Some("public".to_string()),
        name: "orders".to_string(),
        location: Some(SourceLocation { file, line: 5 }),
        columns: vec![ColumnSummary {
            name: "id".to_string(),
            data_type: "BIGINT".to_string(),
            nullable: false,
            is_primary_key: true,
            default_value: None,
            comment: None,
        }],
        partition_by: Some(PartitionInfo::Range {
            columns: vec!["created_at".to_string()],
            partitions: vec![],
        }),
        distribute_by: None,
        tablespace: None,
        temporary: false,
        unlogged: false,
        ddl_source: None,
    };

    let mut graph = CodeGraph::new();
    graph.add_node(table);

    let json_str = export_json(&graph).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();

    let nodes = parsed["nodes"].as_array().unwrap();
    assert_eq!(nodes.len(), 1);
    let node = &nodes[0];
    assert_eq!(node["type"], "table");
    assert_eq!(node["name"], "orders");
    assert!(node["columns"].is_array());
    assert_eq!(node["columns"].as_array().unwrap().len(), 1);
    assert_eq!(node["columns"][0]["name"], "id");
    assert!(node["partition_by"].is_object());
}
```

**Step 2: Update `NodeKindJson::Table`**

```rust
Table {
    schema: Option<String>,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<usize>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    columns: Vec<ColumnSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    partition_by: Option<PartitionInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    distribute_by: Option<DistributeInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tablespace: Option<String>,
    #[serde(skip_serializing_if = "is_false")]
    temporary: bool,
    #[serde(skip_serializing_if = "is_false")]
    unlogged: bool,
},
```

Update the Node→NodeKindJson conversion for Table:

```rust
Node::Table { schema, name, location, columns, partition_by, distribute_by, tablespace, temporary, unlogged, ddl_source: _ } => NodeJson {
    id: idx.index(),
    kind: NodeKindJson::Table {
        schema: schema.clone(),
        name: name.clone(),
        file: location.as_ref().map(|l| l.file.to_string_lossy().to_string()),
        line: location.as_ref().map(|l| l.line),
        columns: columns.clone(),
        partition_by: partition_by.clone(),
        distribute_by: distribute_by.clone(),
        tablespace: tablespace.clone(),
        temporary: *temporary,
        unlogged: *unlogged,
    },
},
```

**Step 3: Update import parser**

In `src/import/parser.rs`, update the `"table"` match arm to parse new fields from JSON, with defaults for backward compatibility.

**Step 4: Update `src/graph/store.rs` `node_source_file`**

```rust
Node::Table { location, .. } => location.as_ref().map(|l| l.file.to_path_buf()),
Node::View { location, .. } => location.as_ref().map(|l| l.file.to_path_buf()),
```

**Step 5: Run full suite**

Run: `cargo test && cargo clippy -- -D warnings`

---

## Phase 1 (P1): TUI Detail Panel

### Task 5: Add node attribute display to TUI Graph screen

**Files:**
- Modify: `src/tui/app.rs` (Graph screen Node Detail panel)

In `draw_graph`, after rendering the target node line, add attribute lines before callers/callees:

```rust
// After the TARGET display section, add attributes
let attr_lines = format_node_attributes(&graph[node_idx]);
for line in attr_lines {
    lines.push(Line::from(Span::styled(
        format!("  {}", line),
        Style::default().fg(Color::DarkGray),
    )));
}
```

Add helper function:

```rust
fn format_node_attributes(node: &Node) -> Vec<String> {
    let mut attrs = Vec::new();
    match node {
        Node::Table { schema, name, location, columns, partition_by, distribute_by, tablespace, temporary, unlogged, .. } => {
            if let Some(loc) = location {
                attrs.push(format!("file: {}:{}", loc.file.to_string_lossy(), loc.line));
            }
            if *temporary { attrs.push("temporary: true".to_string()); }
            if *unlogged { attrs.push("unlogged: true".to_string()); }
            if let Some(ts) = tablespace {
                attrs.push(format!("tablespace: {}", ts));
            }
            if !columns.is_empty() {
                attrs.push(format!("columns: {}", columns.len()));
            }
            if let Some(part) = partition_by {
                attrs.push(format!("partition: {:?}", part));
            }
            if let Some(dist) = distribute_by {
                attrs.push(format!("distribute: {:?}", dist));
            }
        }
        _ => {}
    }
    attrs
}
```

Run: `cargo test && cargo clippy -- -D warnings`

---

## Phase 2 (P2): Query Filters + Index Global Field

### Task 6: Add `--has-partition` and `--has-distribute` filter to `cmd nodes`

**Files:**
- Modify: `src/main.rs` (Nodes subcommand args, `cmd_nodes`)
- Modify: `src/graph/traverse.rs` (filter function)

Add CLI args:

```rust
/// Show only partitioned tables
#[arg(long)]
has_partition: bool,

/// Show only distributed tables
#[arg(long)]
has_distribute: bool,
```

In `cmd_nodes`, add filtering:

```rust
if has_partition || has_distribute {
    filtered = filtered.into_iter().filter(|idx| {
        let matches = match &graph[*idx] {
            Node::Table { partition_by, distribute_by, .. } => {
                (!has_partition || partition_by.is_some())
                    && (!has_distribute || distribute_by.is_some())
            }
            _ => !has_partition && !has_distribute,
        };
        matches
    }).collect();
}
```

Run: `cargo test && cargo clippy -- -D warnings`

### Task 7: Add `global` field to `Node::Index`

**Files:**
- Modify: `src/graph/mod.rs` (Index variant)
- Modify: `src/graph/builder.rs` (Index construction from CreateIndex)
- Modify: `src/export/json.rs`, `src/import/parser.rs`, `src/tui/app.rs`

Add to `Node::Index`:

```rust
/// Whether this is a global index (vs local/partition index).
#[serde(default, skip_serializing_if = "is_false")]
global: bool,
```

ogsql-parser's `CreateIndexStatement` likely has a `global` field — check and wire it through.

Run: `cargo test && cargo clippy -- -D warnings`

---

## Summary of File Changes by Phase

| Phase | Files Changed |
|-------|--------------|
| P0 Task 1 | `src/graph/mod.rs` |
| P0 Task 2 | `src/graph/builder.rs`, all files with `Node::Table { .. }` construction |
| P0 Task 3 | `src/main.rs` |
| P0 Task 4 | `src/export/json.rs`, `src/import/parser.rs`, `src/graph/store.rs` |
| P1 Task 5 | `src/tui/app.rs` |
| P2 Task 6 | `src/main.rs`, `src/graph/traverse.rs` |
| P2 Task 7 | `src/graph/mod.rs`, `src/graph/builder.rs`, `src/export/json.rs`, `src/import/parser.rs`, `src/tui/app.rs`, `src/export/dot.rs`, `src/export/mermaid.rs` |
