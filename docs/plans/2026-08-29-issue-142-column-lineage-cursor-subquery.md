# Issue #142 列级血缘穿透：游标 %ROWTYPE 记录变量与目标列表标量子查询

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 修复列级血缘（`codeweb lineage t.c --direction upstream`）在两类写入形态下无法穿透到真实源列的问题：(1) `%ROWTYPE` 记录变量写入的三种残留盲区，(2) INSERT..SELECT 目标列表中的标量子查询。

**Architecture:** 改动全部位于解析层 `src/parser/extractor.rs` 的 `ColumnAccessExtractor`，不触及 store 结构（无需 bump `STORE_VERSION`）：
- `column_source()`：`%ROWTYPE` 记录字段解析增加「表锚定」与「`SELECT *` 游标 catch-all」两个回退分支；
- `push_column_mapping()`：增加 `Expr::Subquery` 值源分流，子查询首表达式在**子查询自身 FROM 作用域**（临时 alias map + `scope_sole_table` save/restore）下解析；
- `visit_insert()`：`InsertSource::RecordVariable`（整记录 `INSERT INTO t (a,b) VALUES r`）按游标源列位置展开。

血缘 walker（`graph/lineage.rs`）与 CLI（`main.rs`）无需改动——修复消除的是 `table: None` 源与空 `sources`，walker 现有递归逻辑（仅对 `ColumnSource::Column{table: Some(t)}` 递归）即可穿透。

**Tech Stack:** Rust，ogsql-parser（git 依赖），现有测试 harness（`tests/regress_column_lineage.rs` 端到端 + `src/parser/extractor.rs` 单测模块）。

**现状基线（已实测，本分支 feat/issue-142）：**

| 场景 | 当前输出 | 目标 |
|---|---|---|
| `r cur%ROWTYPE` + 显式游标 + `VALUES (r.id, r.amt)` | `t_dst.id ← t_src.id` ✅（#148 已修） | 保持 + 特征测试锁定 |
| `r t_src%ROWTYPE`（表锚定）+ `VALUES (r.id, r.amt)` | `?.id` ❌ | `t_src.id` |
| `r cur%ROWTYPE` + `SELECT *` 游标 | `?.id` ❌ | `t_src.id`（列名取字段名） |
| 整记录 `INSERT INTO t (a,b) VALUES r`（游标锚定） | 无列映射 ❌ | 按游标源列位置映射 |
| INSERT..SELECT 目标列表标量子查询 | "No column lineage" ❌ | `t_ref.code` |
| 对照组：位置映射 `INSERT INTO t_out (id,code) SELECT s.id, s.amt FROM t_src s` | `t_src.id` ✅ | 保持 |

---

## 关键代码位置（当前实现，改动点）

`src/parser/extractor.rs`：

```rust
// L3085 — column_source()：%ROWTYPE 记录字段解析（#147 L2）
fn column_source(&self, names: &[ogsql_parser::Ident]) -> ColumnSource {
    let (alias_prefix, column) = split_alias_column(names);
    if let Some(record) = &alias_prefix {
        if let Some(cursor) = self.record_cursors.get(&record.to_lowercase()) {
            if let Some(cols) = self.cursor_sources.get(cursor) {
                if let Some(col) = cols.iter()
                    .find(|c| c.output_name.eq_ignore_ascii_case(&column))
                {
                    if !col.source_col.is_empty() {
                        return ColumnSource::Column { table: col.source_table.clone(), column: col.source_col.clone() };
                    }
                }
            }
        }
    }
    let table = match alias_prefix.as_ref() {
        Some(a) => self.resolve_alias(a).map(|ta| ta.table.clone()),
        None => self.scope_sole_table.clone(),
    };
    ColumnSource::Column { table, column }
}
```

```rust
// L2883 — visit_insert() 的 INSERT..VALUES/RecordVariable 分支
ogsql_parser::ast::InsertSource::DefaultValues
| ogsql_parser::ast::InsertSource::Set(_)
| ogsql_parser::ast::InsertSource::RecordVariable(_) => {}   // L2930-2932
```

```rust
// L3283 — push_column_mapping()：值源分发的唯一咽喉点（INSERT..SELECT 目标、
// INSERT..VALUES、UPDATE SET、MERGE 全部经此）
fn push_column_mapping(&mut self, target_table: Option<String>, target_column: String,
                       position: Option<usize>, value: &Expr) {
    let (sources, kind, expression) = self.classify_value_expr(value);
    self.column_mappings.push(ColumnMapping { target_table, target_column, position, sources, kind, expression });
}
```

```rust
// L3178 — collect_value_sources()：L3241 显式丢弃子查询
Expr::Exists(_) | Expr::Subquery(_) => {}   // ← 标量子查询目标列表零源根因
```

**AST 事实（ogsql-parser，已核实）**：目标列表裸标量子查询 `(SELECT ...)` 解析为 `Expr::Subquery(Box<SelectStatement>)`（ast/mod.rs:1258）；`Expr::ScalarSublink`（1259-1264）是 `expr OP ANY/ALL/SOME (subquery)` 谓词形态，非值源，不在本计划范围。

**测试基础设施（现有）**：
- 单测 helper：`column_mappings_of(sql)`（L4924）、`find_mapping`（L4947）、`sources_for`（L4953）、`col(table, column)`（L4960）。
- `ColumnAccessExtractor::new_with_context(&ProcedureVarContext)`（L2078）可注入游标/记录上下文——单测接缝。
- 端到端 harness：`tests/regress_column_lineage.rs` 的 `project_with_sql` + `lineage(root, target, dir, "tree")`。

---

## Task 1: 标量子查询目标解析（Case 2，主缺陷）

**Files:**
- Modify: `src/parser/extractor.rs`（`push_column_mapping` L3283 分流 + 新增 `push_subquery_column_mapping`）
- Test: `src/parser/extractor.rs` mod tests（新增单测）+ `tests/regress_column_lineage.rs`（新增端到端）

**Step 1: 写失败测试（单测，Red）**

在 `src/parser/extractor.rs` 测试模块（`insert_values_maps_literals_and_columns` L5095 附近）新增：

```rust
/// #142: a scalar subquery as an INSERT..SELECT target contributes the inner
/// select's FIRST expression as the source, resolved in the subquery's own FROM
/// scope. Correlated refs (`s.id` in WHERE) must NOT leak as sources.
#[test]
fn scalar_subquery_target_resolves_its_first_column() {
    let maps = column_mappings_of(
        "INSERT INTO t_out (id, code) \
         SELECT s.id, (SELECT r.code FROM t_ref r WHERE r.id = s.id) FROM t_src s",
    );
    let m = find_mapping(&maps, "code");
    assert_eq!(m.kind, MappingKind::Direct);
    assert_eq!(m.sources, vec![col(Some("t_ref"), "code")]);
}

/// #142: the choke point is push_column_mapping, so INSERT..VALUES subqueries
/// resolve too.
#[test]
fn scalar_subquery_in_insert_values_resolves() {
    let maps = column_mappings_of(
        "INSERT INTO t_out (code) VALUES ((SELECT r.code FROM t_ref r WHERE r.id = 1))",
    );
    assert_eq!(
        find_mapping(&maps, "code").sources,
        vec![col(Some("t_ref"), "code")]
    );
}
```

**Step 2: 运行确认失败**

Run: `cargo test --features full scalar_subquery_target_resolves_its_first_column`
Expected: FAIL — `find_mapping(&maps, "code")` 命中映射但 `sources == []`（`collect_value_sources` L3241 丢弃 `Expr::Subquery`）。

**Step 3: 写端到端失败测试（Red）**

`tests/regress_column_lineage.rs` 新增（放在 `cursor_fetch_insert_values_resolves_to_source_columns` 附近）：

```rust
/// #142: a scalar subquery in the INSERT..SELECT target list must resolve to the
/// subquery's source column, not report "No column lineage".
#[test]
fn scalar_subquery_in_insert_select_target_resolves() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE t_src(id NUMBER, amt NUMBER);
CREATE TABLE t_ref(id NUMBER, code VARCHAR2(10));
CREATE TABLE t_out(id NUMBER, code VARCHAR2(10));
CREATE PROCEDURE p_copy_subquery AS BEGIN
  INSERT INTO t_out (id, code)
  SELECT s.id, (SELECT r.code FROM t_ref r WHERE r.id = s.id) FROM t_src s;
END;
"#,
    );
    let out = lineage(&root, "t_out.code", "upstream", "tree");
    assert!(
        !out.contains("No column lineage"),
        "scalar subquery target must resolve:\n{out}"
    );
    assert!(
        out.contains("t_ref.code"),
        "subquery source column missing:\n{out}"
    );
}
```

**Step 4: 运行确认失败**

Run: `cargo test --features full --test regress_column_lineage scalar_subquery_in_insert_select_target_resolves`
Expected: FAIL（"No column lineage" 出现在输出中）。

**Step 5: 最小实现（Green）**

改 `push_column_mapping`（L3283）为值源咽喉点分流，并新增 `push_subquery_column_mapping`（置于 `push_column_mapping` 之后、`visit_merge_statement` L3302 之前）：

```rust
    fn push_column_mapping(
        &mut self,
        target_table: Option<String>,
        target_column: String,
        position: Option<usize>,
        value: &Expr,
    ) {
        // #142: a scalar subquery as a value (`INSERT .. SELECT (SELECT ...)`,
        // `VALUES ((SELECT ...))`, `SET x = (SELECT ...)`, MERGE values) contributes
        // the inner select's FIRST output expression as the source, resolved in the
        // subquery's own FROM scope.
        if let Expr::Subquery(select) = peel_parenthesized(value) {
            self.push_subquery_column_mapping(target_table, target_column, position, select);
            return;
        }
        let (sources, kind, expression) = self.classify_value_expr(value);
        self.column_mappings.push(ColumnMapping {
            target_table,
            target_column,
            position,
            sources,
            kind,
            expression,
        });
    }

    /// Column mapping for `target = (SELECT first_expr FROM ...)`: resolve the
    /// subquery's first select-list expression against the subquery's own FROM
    /// aliases, then restore the enclosing statement's scope. Correlated
    /// references (`s.id` in the subquery's WHERE) are not value sources and are
    /// intentionally not collected — only the first select-list expression feeds
    /// the written column.
    fn push_subquery_column_mapping(
        &mut self,
        target_table: Option<String>,
        target_column: String,
        position: Option<usize>,
        select: &SelectStatement,
    ) {
        let saved_alias_map = self.alias_map.clone();
        self.collect_aliases_from_table_refs(&select.from);
        let new_scope = self.scope_sole_table_of(&select.from);
        let saved_scope = std::mem::replace(&mut self.scope_sole_table, new_scope);

        let mut sources = Vec::new();
        let mut kind = MappingKind::Derived;
        let mut expression: Option<String> = None;
        if let Some(SelectTarget::Expr(first, _)) = select.targets.first() {
            let first = peel_parenthesized(first);
            self.collect_value_sources(first, &mut sources);
            // An entirely-literal first target (`(SELECT 'x' FROM dual)`) is a
            // constant; collect_value_sources skips literals by design, so record
            // it here as a Literal source rather than leaving the mapping empty.
            if sources.is_empty() {
                if let Expr::Literal(lit) = first {
                    sources.push(ColumnSource::Literal {
                        value: format_literal_short(lit),
                    });
                }
            }
            if matches!(sources.as_slice(), [ColumnSource::Column { .. }]) {
                kind = MappingKind::Direct;
            }
            expression = Some(format_expr_short(first));
        }

        self.scope_sole_table = saved_scope;
        self.alias_map = saved_alias_map;

        self.column_mappings.push(ColumnMapping {
            target_table,
            target_column,
            position,
            sources,
            kind,
            // A plain copy needs no expression text (mirrors `insert_select_maps_columns_by_position`).
            expression: if matches!(kind, MappingKind::Direct) {
                None
            } else {
                expression
            },
        });
    }
```

**Step 6: 运行确认通过**

Run: `cargo test --features full scalar_subquery`
Expected: PASS（两个单测）。

Run: `cargo test --features full --test regress_column_lineage scalar_subquery_in_insert_select_target_resolves`
Expected: PASS（`t_ref.code` 出现在输出，无 "No column lineage"）。

**Step 7: 回归现有单测**

Run: `cargo test --features full insert_values_maps_literals_and_columns`
Run: `cargo test --features full insert_select_maps_columns_by_position`
Expected: PASS（子查询分流不影响普通值）。

**Step 8: Commit**

```bash
git add src/parser/extractor.rs tests/regress_column_lineage.rs
git commit -m "feat(lineage): 标量子查询作为 INSERT 值源时解析其首表达式 (fix #142 部分)"
```

---

## Task 2: 表锚定 %ROWTYPE 记录字段（Case 1a）

**Files:**
- Modify: `src/parser/extractor.rs`（`column_source` L3085 的 record 分支加 else 回退）
- Test: 单测 + `tests/regress_column_lineage.rs`

**Step 1: 写失败测试（单测，Red）**

```rust
/// #142: a `rec t%ROWTYPE` record (anchor is a TABLE, not a registered cursor)
/// resolves its fields to that table's columns.
#[test]
fn table_rowtype_record_field_resolves_to_table_column() {
    let mut ctx = ProcedureVarContext::default();
    ctx.record_cursors.insert("r".to_string(), "t_src".to_string());
    let maps = column_mappings_of_with_context(
        "INSERT INTO t_dst (id, amt) VALUES (r.id, r.amt)",
        &ctx,
    );
    assert_eq!(
        find_mapping(&maps, "id").sources,
        vec![col(Some("t_src"), "id")]
    );
    assert_eq!(
        find_mapping(&maps, "amt").sources,
        vec![col(Some("t_src"), "amt")]
    );
}
```

新增测试 helper（放在 `column_mappings_of` L4929 之后）：

```rust
    /// Column mappings with a seeded procedure variable context (#142): lets a
    /// standalone INSERT walk see cursor/record bindings that in real procedures
    /// come from the DECLARE block.
    fn column_mappings_of_with_context(sql: &str, ctx: &ProcedureVarContext) -> Vec<ColumnMapping> {
        let tokens = Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let stmts = parser.parse_with_text();
        let mut result = Vec::new();
        for info in &stmts {
            let mut extractor = ColumnAccessExtractor::new_with_context(ctx);
            walk_statement(&mut extractor, &info.statement);
            result.extend(extractor.finish().column_mappings);
        }
        result
    }
```

**Step 2: 运行确认失败**

Run: `cargo test --features full table_rowtype_record_field_resolves_to_table_column`
Expected: FAIL — `sources == [col(None, "id")]`（anchor 查 `cursor_sources` 落空，走兜底 `table: None` → `?.id`）。

**Step 3: 写端到端失败测试（Red）**

```rust
/// #142: a table-anchored %ROWTYPE record (`r t_src%ROWTYPE`) written via
/// `VALUES (r.id, r.amt)` must resolve to t_src columns, not "?.id".
#[test]
fn table_rowtype_record_insert_values_resolves_to_table() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE t_src(id NUMBER, amt NUMBER);
CREATE TABLE t_dst(id NUMBER, amt NUMBER);
CREATE PROCEDURE p_table_rowtype AS
  r t_src%ROWTYPE;
  CURSOR cur IS SELECT id, amt FROM t_src;
BEGIN
  OPEN cur;
  LOOP
    FETCH cur INTO r;
    EXIT WHEN cur%NOTFOUND;
    INSERT INTO t_dst (id, amt) VALUES (r.id, r.amt);
  END LOOP;
  CLOSE cur;
END;
"#,
    );
    let out = lineage(&root, "t_dst.id", "upstream", "tree");
    assert!(
        out.contains("t_src.id"),
        "table-anchored record field must resolve:\n{out}"
    );
    assert!(
        !out.contains("?.id"),
        "table-anchored record field must not stay unattributed:\n{out}"
    );
}
```

**Step 4: 运行确认失败**

Run: `cargo test --features full --test regress_column_lineage table_rowtype_record_insert_values_resolves_to_table`
Expected: FAIL（输出含 `?.id`）。

**Step 5: 最小实现（Green）**

改 `column_source`（L3085）的 record 分支——在 `cursor_sources.get(cursor)` 的 `Some` 分支之外加 `else` 回退：

```rust
    fn column_source(&self, names: &[ogsql_parser::Ident]) -> ColumnSource {
        let (alias_prefix, column) = split_alias_column(names);

        // `%ROWTYPE` record field (issue #147 L2): `rec.id` where rec is a record
        // resolves to the cursor's source column by output name.
        if let Some(record) = &alias_prefix {
            if let Some(cursor) = self.record_cursors.get(&record.to_lowercase()) {
                if let Some(cols) = self.cursor_sources.get(cursor) {
                    if let Some(col) = cols
                        .iter()
                        .find(|c| c.output_name.eq_ignore_ascii_case(&column))
                    {
                        if !col.source_col.is_empty() {
                            return ColumnSource::Column {
                                table: col.source_table.clone(),
                                column: col.source_col.clone(),
                            };
                        }
                    }
                } else {
                    // #142: the `%ROWTYPE` anchor is a TABLE, not a registered
                    // cursor (`rec t_src%ROWTYPE`): the record's fields are that
                    // table's columns. (A custom record TYPE anchor is rare; it
                    // would attribute the type name as a table — the field is
                    // still attributable, unlike the old `?.field`.)
                    return ColumnSource::Column {
                        table: Some(cursor.clone()),
                        column: column.clone(),
                    };
                }
            }
        }

        let table = match alias_prefix.as_ref() {
            Some(a) => self.resolve_alias(a).map(|ta| ta.table.clone()),
            None => self.scope_sole_table.clone(),
        };
        ColumnSource::Column { table, column }
    }
```

**Step 6: 运行确认通过**

Run: `cargo test --features full table_rowtype_record_field_resolves_to_table_column`
Expected: PASS。

Run: `cargo test --features full --test regress_column_lineage table_rowtype_record_insert_values_resolves_to_table`
Expected: PASS。

**Step 7: 回归 Task 1 与游标路径**

Run: `cargo test --features full scalar_subquery`
Run: `cargo test --features full cursor_fetch_insert_values_resolves_to_source_columns`
Expected: PASS（`else` 回退不影响已注册游标路径——`cursor_sources.get` 命中时走原逻辑）。

**Step 8: Commit**

```bash
git add src/parser/extractor.rs tests/regress_column_lineage.rs
git commit -m "feat(lineage): 表锚定 %ROWTYPE 记录字段解析为表列 (fix #142 部分)"
```

---

## Task 3: SELECT * 游标 + 记录字段（Case 1b）

**Files:**
- Modify: `src/parser/extractor.rs`（`column_source` L3085 record 分支内加 catch-all 匹配）
- Test: 单测 + `tests/regress_column_lineage.rs`

**Step 1: 写失败测试（单测，Red）**

```rust
/// #142: a `SELECT *` cursor produces a single catch-all cursor source (empty
/// output name, table attributed). Record fields over it attribute to the
/// cursor's table under the field's own name.
#[test]
fn star_cursor_rowtype_record_field_attributes_to_cursor_table() {
    let mut ctx = ProcedureVarContext::default();
    ctx.cursor_sources.insert(
        "cur".to_string(),
        vec![CursorColumn {
            output_name: String::new(),
            source_table: Some("t_src".to_string()),
            source_col: String::new(),
        }],
    );
    ctx.record_cursors.insert("r".to_string(), "cur".to_string());
    let maps = column_mappings_of_with_context(
        "INSERT INTO t_dst (id, amt) VALUES (r.id, r.amt)",
        &ctx,
    );
    assert_eq!(
        find_mapping(&maps, "id").sources,
        vec![col(Some("t_src"), "id")]
    );
    assert_eq!(
        find_mapping(&maps, "amt").sources,
        vec![col(Some("t_src"), "amt")]
    );
}
```

**Step 2: 运行确认失败**

Run: `cargo test --features full star_cursor_rowtype_record_field_attributes_to_cursor_table`
Expected: FAIL — `sources == [col(None, "id")]`（catch-all 的 `output_name` 为空，`find` 按 output_name 匹配不到）。

**Step 3: 写端到端失败测试（Red）**

```rust
/// #142: `SELECT *` cursor + `%ROWTYPE` record fields must resolve to the
/// cursor's table (columns attributed under the field names), not "?.id".
#[test]
fn star_cursor_rowtype_record_resolves_to_cursor_table() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE t_src(id NUMBER, amt NUMBER);
CREATE TABLE t_dst(id NUMBER, amt NUMBER);
CREATE PROCEDURE p_star_cursor AS
  CURSOR cur IS SELECT * FROM t_src;
  r cur%ROWTYPE;
BEGIN
  OPEN cur;
  LOOP
    FETCH cur INTO r;
    EXIT WHEN cur%NOTFOUND;
    INSERT INTO t_dst (id, amt) VALUES (r.id, r.amt);
  END LOOP;
  CLOSE cur;
END;
"#,
    );
    let out = lineage(&root, "t_dst.id", "upstream", "tree");
    assert!(
        out.contains("t_src.id"),
        "star-cursor record field must resolve:\n{out}"
    );
    assert!(
        !out.contains("?.id"),
        "star-cursor record field must not stay unattributed:\n{out}"
    );
}
```

**Step 4: 运行确认失败**

Run: `cargo test --features full --test regress_column_lineage star_cursor_rowtype_record_resolves_to_cursor_table`
Expected: FAIL（输出含 `?.id`）。

**Step 5: 最小实现（Green）**

在 `column_source` record 分支的 `Some(cols)` 内、`find` 匹配失败之后追加 catch-all 匹配：

```rust
                if let Some(cols) = self.cursor_sources.get(cursor) {
                    if let Some(col) = cols
                        .iter()
                        .find(|c| c.output_name.eq_ignore_ascii_case(&column))
                    {
                        if !col.source_col.is_empty() {
                            return ColumnSource::Column {
                                table: col.source_table.clone(),
                                column: col.source_col.clone(),
                            };
                        }
                    }
                    // #142: a single catch-all cursor source (empty output name —
                    // `SELECT *` cursor, or dynamic-SQL attribution) covers every
                    // record field: the exact column is unknown, attribute to the
                    // cursor's table under the field's own name (same philosophy as
                    // `resolve_cursor_flows`).
                    if let [single] = cols.as_slice() {
                        if single.output_name.is_empty() {
                            if let Some(ref t) = single.source_table {
                                return ColumnSource::Column {
                                    table: Some(t.clone()),
                                    column: column.clone(),
                                };
                            }
                        }
                    }
                } else {
                    // #142: table-anchored %ROWTYPE (Task 2)
                    return ColumnSource::Column {
                        table: Some(cursor.clone()),
                        column: column.clone(),
                    };
                }
```

**Step 6: 运行确认通过**

Run: `cargo test --features full star_cursor_rowtype_record_field_attributes_to_cursor_table`
Expected: PASS。

Run: `cargo test --features full --test regress_column_lineage star_cursor_rowtype_record_resolves_to_cursor_table`
Expected: PASS。

**Step 7: 回归**

Run: `cargo test --features full table_rowtype_record_field_resolves_to_table_column`
Expected: PASS。

**Step 8: Commit**

```bash
git add src/parser/extractor.rs tests/regress_column_lineage.rs
git commit -m "feat(lineage): SELECT * 游标 + %ROWTYPE 记录字段归因到游标表 (fix #142 部分)"
```

---

## Task 4: 整记录写入 INSERT ... VALUES r（Case 1c，游标锚定）

**Files:**
- Modify: `src/parser/extractor.rs`（`visit_insert` L2928-2932，`RecordVariable` 分支拆出）
- Test: 单测 + `tests/regress_column_lineage.rs`

**Step 1: 写失败测试（单测，Red）**

```rust
/// #142: `INSERT INTO t (a, b) VALUES r` with a cursor-anchored %ROWTYPE record
/// expands the record's fields positionally through the cursor's SELECT sources.
#[test]
fn whole_record_insert_expands_cursor_rowtype_fields() {
    let mut ctx = ProcedureVarContext::default();
    ctx.cursor_sources.insert(
        "cur".to_string(),
        vec![
            CursorColumn {
                output_name: "id".to_string(),
                source_table: Some("t_src".to_string()),
                source_col: "id".to_string(),
            },
            CursorColumn {
                output_name: "amt".to_string(),
                source_table: Some("t_src".to_string()),
                source_col: "amt".to_string(),
            },
        ],
    );
    ctx.record_cursors.insert("r".to_string(), "cur".to_string());
    let maps = column_mappings_of_with_context(
        "INSERT INTO t_dst (id, amt) VALUES r",
        &ctx,
    );
    assert_eq!(
        find_mapping(&maps, "id").sources,
        vec![col(Some("t_src"), "id")]
    );
    assert_eq!(
        find_mapping(&maps, "amt").sources,
        vec![col(Some("t_src"), "amt")]
    );
}
```

> 实现时先验证 `INSERT INTO t (a,b) VALUES r` 解析为 `InsertSource::RecordVariable(Expr::PlVariable(["r"]))`（可用 `dbg!` 临时打印或先跑此测试看失败形态；若实际为 `Values([PlVariable])` 则改动点移到 Values 分支，逻辑相同——按实测调整）。

**Step 2: 运行确认失败**

Run: `cargo test --features full whole_record_insert_expands_cursor_rowtype_fields`
Expected: FAIL — `find_mapping` panic "no mapping for id"（RecordVariable 分支当前被跳过，产生零映射）。

**Step 3: 写端到端失败测试（Red）**

```rust
/// #142: whole-record insert `INSERT INTO t_dst (id, amt) VALUES r` (cursor-anchored
/// %ROWTYPE) must resolve positionally through the cursor's sources.
#[test]
fn whole_record_insert_values_r_resolves_through_cursor() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE t_src(id NUMBER, amt NUMBER);
CREATE TABLE t_dst(id NUMBER, amt NUMBER);
CREATE PROCEDURE p_rec_insert AS
  CURSOR cur IS SELECT id, amt FROM t_src;
  r cur%ROWTYPE;
BEGIN
  OPEN cur;
  LOOP
    FETCH cur INTO r;
    EXIT WHEN cur%NOTFOUND;
    INSERT INTO t_dst (id, amt) VALUES r;
  END LOOP;
  CLOSE cur;
END;
"#,
    );
    let out = lineage(&root, "t_dst.amt", "upstream", "tree");
    assert!(
        out.contains("t_src.amt"),
        "whole-record insert must resolve through the cursor:\n{out}"
    );
}
```

**Step 4: 运行确认失败**

Run: `cargo test --features full --test regress_column_lineage whole_record_insert_values_r_resolves_through_cursor`
Expected: FAIL（无映射 → "No column lineage"）。

**Step 5: 最小实现（Green）**

改 `visit_insert` L2928-2932，把 `RecordVariable` 拆出独立分支：

```rust
                // DEFAULT VALUES has no sources; `SET` is handled as assignments.
                ogsql_parser::ast::InsertSource::DefaultValues
                | ogsql_parser::ast::InsertSource::Set(_) => {}
                // #142: `INSERT INTO t (a, b) VALUES r` — expand the record's
                // fields through its `%ROWTYPE` anchor. Cursor-anchored records
                // resolve positionally through the cursor's SELECT sources; a
                // table-anchored record needs the table's column order (DDL),
                // which is unavailable here, so it is left unresolved (documented
                // limitation).
                ogsql_parser::ast::InsertSource::RecordVariable(expr) => {
                    if let Expr::PlVariable(names) = peel_parenthesized(expr) {
                        let rec = names.join(".").to_lowercase();
                        if let Some(anchor) = self.record_cursors.get(&rec) {
                            if let Some(cols) = self.cursor_sources.get(anchor) {
                                for (position, column) in insert.columns.iter().enumerate() {
                                    let col = cols.get(position).or(match cols.as_slice() {
                                        [single] if single.output_name.is_empty() => Some(single),
                                        _ => None,
                                    });
                                    let source = col.and_then(|c| {
                                        if !c.source_col.is_empty() {
                                            Some(ColumnSource::Column {
                                                table: c.source_table.clone(),
                                                column: c.source_col.clone(),
                                            })
                                        } else if c.source_table.is_some() {
                                            // Catch-all (`SELECT *` cursor): attribute
                                            // under the target column's own name.
                                            Some(ColumnSource::Column {
                                                table: c.source_table.clone(),
                                                column: column.clone(),
                                            })
                                        } else {
                                            None
                                        }
                                    });
                                    if let Some(source) = source {
                                        self.column_mappings.push(ColumnMapping {
                                            target_table: Some(table_name.clone()),
                                            target_column: column.clone(),
                                            position: Some(position),
                                            sources: vec![source],
                                            kind: MappingKind::Direct,
                                            expression: None,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
```

**Step 6: 运行确认通过**

Run: `cargo test --features full whole_record_insert_expands_cursor_rowtype_fields`
Expected: PASS。

Run: `cargo test --features full --test regress_column_lineage whole_record_insert_values_r_resolves_through_cursor`
Expected: PASS。

**Step 7: 回归**

Run: `cargo test --features full scalar_subquery`
Run: `cargo test --features full table_rowtype_record_field`
Run: `cargo test --features full star_cursor_rowtype_record`
Expected: PASS。

**Step 8: Commit**

```bash
git add src/parser/extractor.rs tests/regress_column_lineage.rs
git commit -m "feat(lineage): 整记录 INSERT VALUES r 按游标源列位置展开 (fix #142 部分)"
```

---

## Task 5: 特征测试锁定已修复的游标 %ROWTYPE 形态

**Files:**
- Test: `tests/regress_column_lineage.rs`（仅新增，无实现改动）

**Step 1: 写特征测试（当前已 PASS，锁定 #148 行为防回归）**

```rust
/// #142 characteristic test: cursor-anchored %ROWTYPE record written via
/// `VALUES (r.id, r.amt)` resolves to the cursor's source columns (fixed by #148;
/// this locks the behavior so later extraction changes cannot regress it).
#[test]
fn cursor_rowtype_record_insert_values_resolves_to_cursor_source() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE t_src(id NUMBER, amt NUMBER);
CREATE TABLE t_dst(id NUMBER, amt NUMBER);
CREATE PROCEDURE p_copy_cursor AS
  CURSOR cur IS SELECT id, amt FROM t_src;
  r cur%ROWTYPE;
BEGIN
  OPEN cur;
  LOOP
    FETCH cur INTO r;
    EXIT WHEN cur%NOTFOUND;
    INSERT INTO t_dst (id, amt) VALUES (r.id, r.amt);
  END LOOP;
  CLOSE cur;
END;
"#,
    );
    let out = lineage(&root, "t_dst.amt", "upstream", "tree");
    assert!(
        out.contains("t_src.amt"),
        "cursor %ROWTYPE record field must resolve:\n{out}"
    );
}
```

**Step 2: 运行确认通过**

Run: `cargo test --features full --test regress_column_lineage cursor_rowtype_record_insert_values_resolves_to_cursor_source`
Expected: PASS（当前行为基线）。

**Step 3: Commit**

```bash
git add tests/regress_column_lineage.rs
git commit -m "test(lineage): 锁定游标 %ROWTYPE 记录字段写入的穿透行为 (fix #142)"
```

---

## Task 6: 全量门禁与收尾

**Step 1: fmt**

Run: `cargo fmt --all -- --check`
Expected: PASS。若有格式问题：`cargo fmt` 后重跑。

**Step 2: clippy（full）**

Run: `cargo clippy --features full -- -D warnings`
Expected: PASS（零警告）。

**Step 3: 全量测试（full，跳过环境相关）**

Run: `cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_`
Expected: PASS（含新增 4 个单测 + 5 个端到端；无与本改动相关的既有失败）。

**Step 4: 检查 diff 范围**

Run: `git diff --stat HEAD` 与 `git status`
Expected: 仅 `src/parser/extractor.rs`、`tests/regress_column_lineage.rs`、本计划文档；无调试输出/草稿。

**Step 5: 汇报**

按 AGENTS.md「每个 TDD 循环汇报」格式输出：每个任务测试的行为（测试函数名）、最小实现改的文件、是否重构及边界、实际执行的命令与结果。

---

## 已知局限（有意不处理，记录备查）

1. **整记录写入无列清单**：`INSERT INTO t VALUES r`（无 `(a,b)`）无法命名目标列——需要目标表 DDL 列序，超出静态解析能力，维持现状（不产生列映射）。
2. **表锚定 + `SELECT *` 组合**：`r t_src%ROWTYPE` 且通过 `SELECT *` 游标 FETCH 填充——`record_cursors` 锚定为表名，`cursor_sources` 无对应条目 → 走 Task 2 的表锚定回退（`table: Some(t_src)`，列名取字段名），行为可接受。
3. **子查询首表达式为记录字段**（真实项目 `v_fund_acnt_all.CLIENT_ACNT_ID`）：`FieldAccess` → `collect_value_sources` 递归到 `PlVariable` → `ColumnSource::Variable{v_fund_acnt_all}`——比 "No column lineage" 好（显示变量名），但不穿透到列；完整解析需在子查询内再做记录字段展开，属后续增强。
4. **`Expr::ScalarSublink`（`expr OP ANY/ALL/SOME (subquery)`）作为值源**：非本计划场景（谓词形态），`collect_value_sources` 无分支 → 保持零源。
5. **自定义 TYPE `%ROWTYPE` 锚定**：按 Task 2 回退归因到类型名（视为表名）——罕见形态，比 `?.field` 可归因。

## 验收标准

- [ ] Task 1-4 各有失败→通过的测试（单测 + 端到端）
- [ ] Task 5 特征测试锁定 #148 已修复行为
- [ ] 未删除/跳过/改写人类已有测试
- [ ] `cargo fmt`、`cargo clippy --features full -- -D warnings` 干净
- [ ] `cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_` 全绿
- [ ] 仅改动 `src/parser/extractor.rs`、`tests/regress_column_lineage.rs`；无 store 结构变化（不 bump `STORE_VERSION`）
