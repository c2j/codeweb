# 存储过程 SQL 搜索 — 实施计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**目标:** 扩展 `search_by_sql` / `trace-sql` 支持搜索存储过程体内的 SQL 语句，使搜索结果从仅覆盖 Mapper + JavaSql 扩展到 Mapper + JavaSql + Procedure/Function 体内 SQL。

**架构:** 在现有 `Procedure`/`Function` 节点上新增 `body_sql` 字段存储过程体内提取的 SQL 文本列表。在 `GraphStore::from_graph()` 索引构建阶段遍历这些 SQL 并加入 `sql_fingerprint_index`。`search_by_sql()` 和 `cmd_trace_sql()` 增加 `Procedure`/`Function` 分支。

**技术栈:** Rust, ogsql-parser (`walk_pl_block` + `PlStatement::SqlStatement`/`PlStatement::Sql`), petgraph, blake3

---

## 验证基础

ogsql-parser 能力已通过 17 个验证测试确认（`parser::extractor::procedure_sql_extraction`）：

- `PlStatement::SqlStatement { sql_text, statement, span }` — 解析成功的 SQL，含原始文本 + AST + 行号
- `PlStatement::Sql(String)` — 未完全解析的 SQL 文本
- `walk_pl_block()` — 遍历 body + exception handler + 嵌套 block
- 支持 SELECT/INSERT/UPDATE/DELETE/MERGE，IF/LOOP/CASE 嵌套，三种方言

**关键修正:** `SqlStatement` 有 `span: Option<SourceSpan>` 字段（含 `line`/`column`/`offset`），行号信息可获取。

---

## 方案选择：在 Procedure/Function 节点上增加 `body_sql` 字段

**选择理由（而非新增 ProcedureSql 节点类型）：**

1. **最小改动原则** — 不引入新节点类型、新边类型、新 NodeKey 变体，避免序列化兼容性风险
2. **搜索粒度足够** — 过程体内每条 SQL 的 `sql_text` 独立参与索引，搜索命中时展示所属过程 + 匹配的具体 SQL
3. **与 Mapper/JavaSql 模式对齐** — `MappedStatement.sql` 是单条 SQL，`Procedure.body_sql` 是 SQL 列表，索引时逐条展开

---

## Task 1: 扩展 Node 数据模型

**文件:**
- 修改: `src/graph/mod.rs` — `Node::Procedure` / `Node::Function` 增加字段
- 修改: `src/graph/key.rs` — 无需修改（Procedure/Function 的 NodeKey 不变）

**Step 1: 在 `ProcedureBodySql` 结构体（新增）和 `Node::Procedure`/`Node::Function` 上增加 `body_sql` 字段**

在 `src/graph/mod.rs` 的 `Node` 枚举定义处：

```rust
/// 一条从存储过程体中提取的 SQL 语句
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcedureBodySql {
    pub sql_text: String,
    pub kind: String,  // "SELECT" / "INSERT" / "UPDATE" / "DELETE" / "MERGE" / "SQL" / "EXECUTE"
    pub line: Option<usize>,
}

// Node::Procedure 增加字段：
Procedure {
    id: RoutineId,
    location: SourceLocation,
    #[serde(default)]
    partial: bool,
    #[serde(default)]
    body_sql: Vec<ProcedureBodySql>,  // 新增
}

// Node::Function 同样增加：
Function {
    id: RoutineId,
    location: SourceLocation,
    #[serde(default)]
    partial: bool,
    #[serde(default)]
    body_sql: Vec<ProcedureBodySql>,  // 新增
}
```

注意 `#[serde(default)]` 保证向后兼容——旧 bincode 文件反序列化时 `body_sql` 为空 Vec。

**Step 2: 运行编译验证**

```sh
cargo build 2>&1
```

需修复所有因新增字段导致的编译错误（主要是 `builder.rs` 中构造 `Node::Procedure`/`Node::Function` 的地方，加上 `body_sql: Vec::new()`）。

**Step 3: 提交**

```sh
git add -A && git commit -m "feat: add body_sql field to Procedure/Function nodes"
```

---

## Task 2: 实现 ProcedureSqlExtractor

**文件:**
- 修改: `src/parser/extractor.rs` — 新增 `ProcedureSqlExtractor` 和 `extract_body_sql` 函数

**Step 1: 实现 ProcedureSqlExtractor Visitor**

复用验证测试中已验证的模式，将其从测试代码提升为正式组件：

```rust
pub struct ProcedureBodySql {
    pub sql_text: String,
    pub kind: String,
    pub line: Option<usize>,
}

pub struct ProcedureSqlExtractor {
    results: Vec<ProcedureBodySql>,
}

impl ProcedureSqlExtractor {
    pub fn new() -> Self { Self { results: Vec::new() } }
    pub fn finish(self) -> Vec<ProcedureBodySql> { self.results }
}

impl Visitor for ProcedureSqlExtractor {
    fn visit_pl_statement(&mut self, stmt: &PlStatement) -> VisitorResult {
        match stmt {
            PlStatement::SqlStatement { sql_text, statement, span, .. } => {
                let kind = statement_kind_name(statement);
                let line = span.as_ref().map(|s| s.start.line);
                self.results.push(ProcedureBodySql {
                    sql_text: sql_text.clone(),
                    kind,
                    line,
                });
            }
            PlStatement::Sql(sql_text) => {
                self.results.push(ProcedureBodySql {
                    sql_text: sql_text.clone(),
                    kind: "SQL".to_string(),
                    line: None,
                });
            }
            PlStatement::Perform { query, parsed_query } => {
                if let Some(ref stmt) = parsed_query {
                    let kind = statement_kind_name(stmt);
                    self.results.push(ProcedureBodySql {
                        sql_text: query.clone(),
                        kind,
                        line: None,
                    });
                }
            }
            _ => {}
        }
        VisitorResult::Continue
    }
}

fn statement_kind_name(stmt: &Statement) -> String {
    match stmt {
        Statement::Select(_) => "SELECT",
        Statement::Insert(_) => "INSERT",
        Statement::Update(_) => "UPDATE",
        Statement::Delete(_) => "DELETE",
        Statement::Merge(_) => "MERGE",
        _ => "SQL",
    }.to_string()
}

/// 从 Procedure/Function 的 block 中提取所有 SQL 语句
pub fn extract_body_sql(block: &ogsql_parser::ast::plpgsql::PlBlock) -> Vec<ProcedureBodySql> {
    let mut extractor = ProcedureSqlExtractor::new();
    ogsql_parser::walk_pl_block(&mut extractor, block);
    extractor.finish()
}
```

**Step 2: 在 graph/mod.rs 中增加 From 转换**

```rust
impl From<crate::parser::ProcedureBodySql> for ProcedureBodySql {
    fn from(v: crate::parser::ProcedureBodySql) -> Self {
        Self {
            sql_text: v.sql_text,
            kind: v.kind,
            line: v.line,
        }
    }
}
```

或直接在 `graph::mod.rs` 中使用 parser 模块定义的类型（取决于模块可见性设计）。

**Step 3: 运行已有测试**

```sh
cargo test procedure_sql_extraction 2>&1
cargo test 2>&1 | grep "test result:"
```

所有已有测试（含 17 个验证测试）应通过。

**Step 4: 提交**

```sh
git add -A && git commit -m "feat: add ProcedureSqlExtractor for body SQL extraction"
```

---

## Task 3: 在 GraphBuilder 中填充 body_sql

**文件:**
- 修改: `src/graph/builder.rs` — 构造 `Node::Procedure`/`Node::Function` 时从 AST block 提取 SQL

**Step 1: 在构建 Procedure/Function 节点时提取 body SQL**

在 `builder.rs` 中找到所有构造 `Node::Procedure` 和 `Node::Function` 的位置，调用 `extract_body_sql`：

```rust
// 原来的代码（大约 line 170）：
let node = Node::Procedure {
    id,
    location: SourceLocation { file: file_arc.clone(), line: info.start_line },
    partial: false,
    body_sql: Vec::new(),  // Task 1 已加
};

// 改为：
Statement::CreateProcedure(p) => {
    let id = RoutineId::from_object_name(&p.name, RoutineKind::Procedure);
    let body_sql = p.block.as_ref()
        .map(|b| crate::parser::extract_body_sql(b)
            .into_iter().map(Into::into).collect())
        .unwrap_or_default();
    proc_index.entry(id.clone()).or_insert_with(|| {
        let node = Node::Procedure {
            id,
            location: SourceLocation { file: file_arc.clone(), line: info.start_line },
            partial: false,
            body_sql,
        };
        graph.add_node(node)
    });
}
```

同理处理 `CreateFunction`、Package 中的 Procedure/Function、CreatePackageBody 中的实现体。

**关键位置**（需要修改的 builder.rs 代码段）：
- Line ~167: `Statement::CreateProcedure(p)` — 独立存储过程
- Line ~181: `Statement::CreateFunction(f)` — 独立函数
- Line ~940+: package spec/package body 中的 procedure/function 处理
- Line ~1026+: `walk_pl_block` 调用处（package body 内 routine 的 block）

**Step 2: 运行编译 + 全部测试**

```sh
cargo test 2>&1 | grep "test result:"
```

**Step 3: 提交**

```sh
git add -A && git commit -m "feat: populate body_sql during graph construction"
```

---

## Task 4: 扩展 search_by_sql 索引 + 搜索逻辑

**文件:**
- 修改: `src/graph/store.rs` — `from_graph()` 索引构建 + `search_by_sql()` 搜索逻辑

**Step 1: 在 from_graph() 索引构建中加入 Procedure/Function**

在 `store.rs` 的 `from_graph()` 方法中，`sql_fingerprint_index` 构建循环内增加分支：

```rust
Node::Procedure { id, body_sql, .. } => {
    for sql in body_sql {
        let fp = sql_fingerprint(&sql.sql_text);
        let display_key = format!("proc:{}", id);
        sql_fingerprint_index.entry(fp).or_default().push((idx, display_key.clone()));
    }
}
Node::Function { id, body_sql, .. } => {
    for sql in body_sql {
        let fp = sql_fingerprint(&sql.sql_text);
        let display_key = format!("func:{}", id);
        sql_fingerprint_index.entry(fp).or_default().push((idx, display_key.clone()));
    }
}
```

**Step 2: 在 search_by_sql() fallback 中增加 Procedure/Function 分支**

在 `search_by_sql()` 的线性扫描循环中，增加：

```rust
Node::Procedure { id, body_sql, .. } => {
    for sql in body_sql {
        if prepared.matches(&sql.sql_text) {
            results.push((idx, format!("proc:{}", id)));
            break;  // 一个过程只需匹配一次
        }
    }
}
Node::Function { id, body_sql, .. } => {
    for sql in body_sql {
        if prepared.matches(&sql.sql_text) {
            results.push((idx, format!("func:{}", id)));
            break;
        }
    }
}
```

**Step 3: 添加单元测试**

```rust
#[test]
fn search_by_sql_finds_procedure_body_sql() {
    let mut graph = CodeGraph::new();
    graph.add_node(Node::Procedure {
        id: RoutineId { schema: None, package: None, name: "get_users".into(), kind: RoutineKind::Procedure },
        location: SourceLocation { file: Arc::new(PathBuf::from("test.sql")), line: 1 },
        partial: false,
        body_sql: vec![ProcedureBodySql {
            sql_text: "SELECT * FROM users WHERE status = 'ACTIVE'".to_string(),
            kind: "SELECT".to_string(),
            line: Some(3),
        }],
    });
    let store = GraphStore::from_graph("test", graph);
    let results = store.search_by_sql("select * from users where status");
    assert_eq!(results.len(), 1);
    assert!(results[0].1.contains("get_users"));
}

#[test]
fn search_by_sql_finds_function_body_sql() {
    // 类似测试，用 Function 节点
}

#[test]
fn search_by_sql_proc_multiple_sqls_match_one() {
    // 过程体有多条 SQL，只匹配其中一条
}

#[test]
fn search_by_sql_proc_rejects_unrelated() {
    // 搜索 INSERT 但过程体只有 SELECT，不应匹配
}
```

**Step 4: 运行测试**

```sh
cargo test search_by_sql 2>&1
```

**Step 5: 提交**

```sh
git add -A && git commit -m "feat: extend search_by_sql to cover Procedure/Function body SQL"
```

---

## Task 5: 扩展 CLI trace-sql 输出

**文件:**
- 修改: `src/main.rs` — `cmd_trace_sql()` 增加 `Node::Procedure`/`Node::Function` 输出分支

**Step 1: 在 match 中增加 Procedure/Function 分支**

```rust
Node::Procedure { id, location, body_sql, .. } => {
    println_stdout!("  Procedure: {}", id);
    println_stdout!("    file:  {}:{}", location.file.to_string_lossy(), location.line);
    // 显示匹配的 SQL（最多 5 行）
    for sql in body_sql.iter().take(5) {
        for l in sql.sql_text.lines().take(3) {
            println_stdout!("    sql:   {} [{}]", l, sql.kind);
        }
    }
    let total = body_sql.len();
    if total > 5 {
        println_stdout!("    sql:   ... +{} more SQL statements", total - 5);
    }

    // 显示调用方（与其他方法 trace 一致）
    let callers: Vec<_> = graph
        .neighbors_directed(*idx, petgraph::Direction::Incoming)
        .collect();
    if !callers.is_empty() {
        println_stdout!("    called by:");
        for ci in &callers {
            let key = NodeKey::from_node(&graph[*ci]);
            println_stdout!("      {}", key);
        }
    }
    println_stdout!();
}
```

类似地处理 `Node::Function`。

**Step 2: 更新 "No match" 消息**

```rust
// 原来：
eprintln!("No MappedStatement or JavaSql nodes contain the SQL fragment: '{}'", fragment);
// 改为：
eprintln!("No matching SQL found for fragment: '{}'", fragment);
```

**Step 3: 运行测试**

```sh
cargo test 2>&1 | grep "test result:"
```

**Step 4: 提交**

```sh
git add -A && git commit -m "feat: extend trace-sql CLI output for Procedure/Function"
```

---

## Task 6: 扩展 HTTP API search-sql 输出

**文件:**
- 修改: `src/server/handlers.rs` — `search_sql()` 无需修改（已使用通用 node 格式），但验证 JSON 输出包含正确 type tag
- 修改: `src/graph/store.rs` — `node_type_tag()` 或类似的 helper，确保 Procedure/Function 有正确的 tag

**Step 1: 检查并确保 node type tag 正确**

`server/handlers.rs` 中的 `search_sql()` 已经使用 `node_type_tag(node)` 返回类型标签。确认 `Procedure` 返回 `"proc"`，`Function` 返回 `"func"`（这应该已经工作）。

**Step 2: 运行 serve 测试**

```sh
cargo test --features serve 2>&1 | grep "test result:"
```

**Step 3: 提交**

```sh
git add -A && git commit -m "feat: extend search-sql HTTP API to cover Procedure/Function"
```

---

## Task 7: 端到端集成测试

**文件:**
- 修改: `tests/search_sql_test.rs` — 新增 procedure SQL 搜索测试

**Step 1: 添加端到端测试**

```rust
fn write_sql_file(dir: &TempDir, filename: &str, sql: &str) -> std::path::PathBuf {
    let path = dir.path().join(filename);
    fs::write(&path, sql).unwrap();
    path
}

#[test]
fn trace_sql_finds_matching_procedure() {
    let dir = TempDir::new().unwrap();
    write_sql_file(&dir, "procs.sql", r#"
        CREATE OR REPLACE PROCEDURE get_active_users()
        AS BEGIN
            SELECT * FROM t_users WHERE status = 'ACTIVE';
        END;
        /
    "#);

    let init_out = init_project(&dir, "test-proc-search");
    assert!(init_out.status.success());

    let output = run_codeweb(&[
        "trace-sql",
        "select * from t_users where status",
        "--project",
        dir.path().to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("get_active_users") || stdout.contains("Procedure"),
        "trace-sql should find procedure body SQL. stdout: {}", stdout
    );
}
```

**Step 2: 运行测试**

```sh
cargo test --test search_sql_test 2>&1
```

**Step 3: 提交**

```sh
git add -A && git commit -m "test: add E2E test for procedure SQL search"
```

---

## Task 8: clippy + fmt + 最终验证

**Step 1: 格式检查**

```sh
cargo fmt -- --check
cargo clippy -- -D warnings 2>&1
cargo clippy --features serve -- -D warnings 2>&1
```

**Step 2: 全量测试**

```sh
cargo test 2>&1
cargo test --features serve 2>&1
```

**Step 3: 修正所有问题后提交**

```sh
git add -A && git commit -m "chore: clippy + fmt fixes"
```
