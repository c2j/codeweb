# ogsql-parser Visitor 增强需求规格书

**目标仓库**: https://github.com/c2j/ogsql-parser
**基线版本**: commit `85ac383f`
**实现版本**: commit `8405b1a` ✅ **已全部实现**
**需求方**: codeweb 项目 — 基于 ogsql-parser 构建存储过程调用图分析工具

> **状态更新**: 本规格书中的所有 P0 和 P1 需求已由 ogsql-parser 维护者完整实现。详见下方验证清单。

---

## 1. 背景与动机

### 1.1 现状

当前 `Visitor` trait（`src/ast/visitor.rs`，337 行）仅覆盖 SQL 顶层语句的浅层遍历：

- **7 个 trait 方法**：`visit_statement`、`visit_expr`、`visit_select`、`visit_insert`、`visit_update`、`visit_delete`、`visit_create_table`
- **`walk_statement` 仅处理 5 个 `Statement` 变体**（Select/Insert/Update/Delete/CreateTable），其余 ~175 个变体落入 `_ => VisitorResult::Continue` 通配
- **完全不感知 PL/pgSQL**：无 `visit_pl_block`、`visit_pl_statement`、`visit_pl_declaration` 等方法
- **`walk_expr` 覆盖不完整**：跳过 `Like`、`Between`、`InList`、`InSubquery`、`Exists`、`ScalarSublink`、`TypeCast`、`Array`、`Subscript`、`FieldAccess` 等大量变体
- **`walk_select` 跳过 CTEs**（`with`）、`group_by`、`having`、`order_by`、`set_operation`

### 1.2 影响范围

以下 6 个承载 PL/pgSQL 块体的 `Statement` 变体，遍历时完全被忽略：

| Statement 变体 | PL/pgSQL 入口 | 源码位置 |
|---|---|---|
| `CreateFunction` | `.block: Option<PlBlock>` | mod.rs:1857 |
| `CreateProcedure` | `.block: Option<PlBlock>` | mod.rs:1866 |
| `Do` | `.block: Option<PlBlock>` | mod.rs:2141 |
| `AnonyBlock` | `.block: PlBlock` | mod.rs:1691 |
| `CreatePackage` | `.items → PackageProcedure/PackageFunction → .block` | mod.rs:1876-1912 |
| `CreatePackageBody` | 同上 | mod.rs:1885-1890 |

此外，`PlStatement` 的 25+ 变体中嵌入的 `Expr` 和 `Statement`（如 `PlExecuteStmt.parsed_query`、`PlForKind::Query.parsed_query`、`PlStatement::SqlStatement.statement`）也无法被遍历到。

### 1.3 目标

使 `Visitor` trait 能够**完整遍历 ogsql-parser 已有的全部 AST 类型**，包括 PL/pgSQL 块体，从而使下游用户（如调用图提取、代码分析、格式化等）无需自行编写遍历器。

---

## 2. 设计原则

1. **向后兼容**：现有 `Visitor` 实现必须无需修改即可编译通过。新增 trait 方法全部提供默认空实现。
2. **不引入外部依赖**：控制流机制使用 `core::ops::ControlFlow`（标准库）或保持现有 `VisitorResult`。
3. **关注点分离**：`Visitor` trait 负责通知，`walk_*` 函数负责驱动遍历。两者独立演进。
4. **覆盖完整性**：对 AST 类型做到无遗漏遍历。任何 `Expr`/`Statement`/`PlStatement` 的子节点均应被遍历到。
5. **可组合**：用户可以通过覆盖 `pre_visit_*` 返回 `SkipChildren` 来跳过子树。

---

## 3. 需求项

按优先级排列。P0 = 必须实现；P1 = 强烈建议；P2 = 建议实现。

> **实现状态**: 所有 P0 和 P1 需求 ✅ 已完成。P2 需求部分完成（VisitorResult 保持现有语义，新 walk 函数已导出）。

### 实现验证清单

| ID | 需求 | 状态 | 代码位置 |
|---|---|---|---|
| P0-1 | PL/pgSQL Visitor trait 方法 (4 个) | ✅ | [visitor.rs:46–68](https://github.com/c2j/ogsql-parser/blob/8405b1a8d7d893aa11ddcf0772989b2548be2573/src/ast/visitor.rs#L46-L68) |
| P0-2 | PL/pgSQL walk 函数 (3 个) | ✅ | [visitor.rs:72–422](https://github.com/c2j/ogsql-parser/blob/8405b1a8d7d893aa11ddcf0772989b2548be2573/src/ast/visitor.rs#L72-L422) |
| P0-3 | walk_statement PL/pgSQL 载体处理 (6 个变体) | ✅ | [visitor.rs:434–513](https://github.com/c2j/ogsql-parser/blob/8405b1a8d7d893aa11ddcf0772989b2548be2573/src/ast/visitor.rs#L434-L513) |
| P0-4 | visit_call + visit_procedure_call | ✅ | [visitor.rs:62–67](https://github.com/c2j/ogsql-parser/blob/8405b1a8d7d893aa11ddcf0772989b2548be2573/src/ast/visitor.rs#L62-L67) |
| P1-1 | walk_expr 增强 | ✅ | [visitor.rs:784–1086](https://github.com/c2j/ogsql-parser/blob/8405b1a8d7d893aa11ddcf0772989b2548be2573/src/ast/visitor.rs#L784-L1086) |
| P1-2 | walk_select 增强 | ✅ | [visitor.rs:521–687](https://github.com/c2j/ogsql-parser/blob/8405b1a8d7d893aa11ddcf0772989b2548be2573/src/ast/visitor.rs#L521-L687) |
| P1-3 | walk_table_ref 增强 | ✅ | [visitor.rs:1088–1167](https://github.com/c2j/ogsql-parser/blob/8405b1a8d7d893aa11ddcf0772989b2548be2573/src/ast/visitor.rs#L1088-L1167) |
| P2-2 | 导出新类型 | ✅ | [lib.rs:9–15](https://github.com/c2j/ogsql-parser/blob/8405b1a8d7d893aa11ddcf0772989b2548be2573/src/lib.rs#L9-L15) |
| 测试 | 综合测试覆盖 | ✅ | visitor.rs 内联测试 + visitor_tests.rs (650 行)

---

### P0-1: PL/pgSQL Visitor Trait 方法

在 `Visitor` trait 中新增以下方法（均提供默认空实现，返回 `VisitorResult::Continue`）：

```rust
// PlBlock 入口
fn visit_pl_block(&mut self, _block: &PlBlock) -> VisitorResult {
    VisitorResult::Continue
}

// PL/pgSQL 语句
fn visit_pl_statement(&mut self, _stmt: &PlStatement) -> VisitorResult {
    VisitorResult::Continue
}

// PL/pgSQL 声明
fn visit_pl_declaration(&mut self, _decl: &PlDeclaration) -> VisitorResult {
    VisitorResult::Continue
}

// PL/pgSQL 异常处理器
fn visit_pl_exception_handler(&mut self, _handler: &PlExceptionHandler) -> VisitorResult {
    VisitorResult::Continue
}
```

**验收标准**：
- 所有方法均有默认实现，现有 `Visitor` impl 无需修改
- `PlBlock`、`PlStatement`、`PlDeclaration`、`PlExceptionHandler` 类型已正确 import

---

### P0-2: PL/pgSQL Walk 函数

新增以下 `pub` walk 函数，由 `walk_statement` 在遇到承载 PL/pgSQL 块的 `Statement` 变体时调用：

```rust
/// 遍历 PlBlock：declarations → body → exception_block
pub fn walk_pl_block(visitor: &mut dyn Visitor, block: &PlBlock) -> VisitorResult

/// 遍历 PlStatement：先调用 visit_pl_statement，再根据变体递归遍历子节点
pub fn walk_pl_statement(visitor: &mut dyn Visitor, stmt: &PlStatement) -> VisitorResult

/// 遍历 PlDeclaration：先调用 visit_pl_declaration，再遍历子 Expr/Statement/PlBlock
pub fn walk_pl_declaration(visitor: &mut dyn Visitor, decl: &PlDeclaration) -> VisitorResult
```

**遍历顺序**（`walk_pl_block`）：
1. 调用 `visitor.visit_pl_block(block)` → 检查返回值
2. 遍历 `block.declarations` → 每个调用 `walk_pl_declaration`
3. 遍历 `block.body` → 每个调用 `walk_pl_statement`
4. 如果存在 `block.exception_block` → 遍历 `handlers`，每个 handler：
   a. 调用 `visitor.visit_pl_exception_handler(handler)`
   b. 遍历 `handler.statements` → 每个调用 `walk_pl_statement`

**验收标准**：
- `walk_pl_statement` 能递归进入所有包含子 `PlStatement` 的变体（`If.then_stmts/elsifs/else_stmts`、`Case.whens/else_stmts`、`Loop.body`、`While.body`、`For.body`、`ForEach.body`、`Block(PlBlock)`）
- `walk_pl_statement` 能遍历所有包含 `Expr` 的变体（调用 `walk_expr`）
- `walk_pl_statement` 能遍历所有包含 `Statement` 的变体（调用 `walk_statement`）：
  - `PlStatement::SqlStatement { statement }` → `walk_statement`
  - `PlStatement::Perform { parsed_query }` → 若存在则 `walk_statement`
  - `PlStatement::Execute(PlExecuteStmt).parsed_query` → 若存在则 `walk_statement`
- `walk_pl_declaration` 能进入嵌套的 `NestedProcedure`/`NestedFunction` 的 `block`

---

### P0-3: 修正 `walk_statement` 对 PL/pgSQL 载体变体的处理

当前 `walk_statement` 的 `_ => VisitorResult::Continue` 通配分支忽略了 PL/pgSQL 载体。需改为显式匹配：

```rust
Statement::CreateFunction(s) => {
    let result = visitor.visit_statement(stmt);  // 已在外层调用
    // 若 Continue，遍历 block
    if let Some(ref block) = s.block {
        walk_pl_block(visitor, block)?;
    }
    VisitorResult::Continue
}
Statement::CreateProcedure(s) => { /* 同上模式 */ }
Statement::Do(s) => { /* 同上模式 */ }
Statement::AnonyBlock(s) => { /* block 不是 Option，直接遍历 */ }
Statement::CreatePackage(s) => { /* 遍历 items 中 PackageProcedure/PackageFunction 的 block */ }
Statement::CreatePackageBody(s) => { /* 同上 */ }
```

**注意**：`visit_statement` 已在外层 `walk_statement` 入口调用，内层不应重复调用。内层只负责遍历子节点。

**验收标准**：
- 解析包含 `CREATE FUNCTION ... AS $$ ... $$` 的 SQL 并用 `walk_statement` 遍历，能触发 `visit_pl_block` 和 `visit_pl_statement`
- 解析 `CREATE PACKAGE BODY` 并遍历，能进入包内过程/函数的 `block`

---

### P0-4: 新增 `visit_call` 和 `visit_procedure_call` 方法

调用图提取的核心需求——对过程调用和函数调用提供专门的 hook：

```rust
/// 顶层 CALL 语句（Statement::Call）
fn visit_call(&mut self, _call: &CallFuncStatement) -> VisitorResult {
    VisitorResult::Continue
}

/// PL/pgSQL 块内的过程调用（PlStatement::ProcedureCall）
fn visit_procedure_call(&mut self, _call: &PlProcedureCall) -> VisitorResult {
    VisitorResult::Continue
}
```

**遍历行为**：
- `walk_statement` 遇到 `Statement::Call(s)` → 调用 `visitor.visit_call(&s)`
- `walk_pl_statement` 遇到 `PlStatement::ProcedureCall(c)` → 调用 `visitor.visit_procedure_call(&c)`，然后遍历 `c.arguments` 中的 `Expr`

**验收标准**：
- 遍历 `CALL schema.proc_name(arg1, arg2)` 能触发 `visit_call`，并可在回调中读取 `call.func_name`
- 遍历 PL/pgSQL 块体内的 `schema.proc_name(arg1)` 能触发 `visit_procedure_call`，并可在回调中读取 `call.name`

---

### P1-1: 增强 `walk_expr` 覆盖率

当前 `walk_expr` 跳过大量 `Expr` 变体。应补全以下遍历：

| 被跳过的 Expr 变体 | 应遍历的子节点 |
|---|---|
| `Like { expr, pattern }` | `expr`、`pattern` |
| `NotLike { expr, pattern }` | `expr`、`pattern` |
| `Between { expr, low, high }` | `expr`、`low`、`high` |
| `NotBetween { expr, low, high }` | `expr`、`low`、`high` |
| `InList { expr, list }` | `expr`、`list` 中每个 |
| `NotInList { expr, list }` | `expr`、`list` 中每个 |
| `InSubquery { expr, subquery }` | `expr`、`subquery` → `walk_select` |
| `NotInSubquery { expr, subquery }` | `expr`、`subquery` → `walk_select` |
| `Exists { subquery }` | `subquery` → `walk_select` |
| `ScalarSublink { ... }` | 其中的 `select`/`expr` 子节点 |
| `IsNull { expr }` | `expr` |
| `IsNotNull { expr }` | `expr` |
| `IsNotDistinctFrom { expr, right }` | `expr`、`right` |
| `IsDistinctFrom { expr, right }` | `expr`、`right` |
| `TypeCast { expr, ... }` | `expr` |
| `Treat { expr, ... }` | `expr` |
| `Array { elems }` | `elems` 中每个 |
| `Subscript { object, ... }` | `object` |
| `FieldAccess { expr, ... }` | `expr` |
| `Parenthesized { expr }` | `expr` |
| `RowConstructor { exprs }` | `exprs` 中每个 |
| `CollationFor { expr }` | `expr` |

**验收标准**：
- 补全后 `walk_expr` 的 match 分支应覆盖 `Expr` 枚举的全部变体（`_` 通配分支可保留用于未来新增变体，但当前所有已知变体均需显式处理）
- 每个 `Expr` 子节点均被递归遍历

---

### P1-2: 增强 `walk_select` 覆盖率

当前 `walk_select` 跳过 CTEs、`group_by`、`having`、`order_by`、`set_operation`。应补全：

| 被跳过的 Select 字段 | 应遍历的子节点 |
|---|---|
| `with` (CTEs) | 每个 CTE 的 `query` → `walk_select`；`column_names` 无需遍历 |
| `group_by` | 每个 `Expr` → `walk_expr` |
| `having` | `Expr` → `walk_expr` |
| `order_by` | 每个 `OrderByItem.expr` → `walk_expr` |
| `set_operation` (UNION/INTERSECT/EXCEPT) | 左右两侧的 `SelectStatement` → `walk_select` |
| `limit` / `offset` | 若为 `Expr` → `walk_expr` |

**验收标准**：
- 含 CTE 的 `WITH cte AS (...) SELECT ...` 语句能被完整遍历
- 含 `UNION ALL SELECT` 的复合查询能递归遍历两侧

---

### P1-3: 增强 `walk_table_ref` 覆盖率

当前 `walk_table_ref` 仅处理 `TableRef::Subquery`。应补全：

| 被跳过的 TableRef 变体 | 应遍历的子节点 |
|---|---|
| `TableRef::Join { left, right, condition }` | `left` → `walk_table_ref`、`right` → `walk_table_ref`、`condition` 中 Expr → `walk_expr` |
| `TableRef::FunctionCall { ... }` | 参数中的 Expr → `walk_expr` |
| `TableRef::Values { ... }` | 每行每列 Expr → `walk_expr` |
| `TableRef::Pivot { ... }` / `Unpivot { ... }` | 聚合 Expr、列值 Expr → `walk_expr` |

---

### P2-1: 提升 `VisitorResult` 为三态

当前 `VisitorResult` 仅有 `Continue`、`SkipChildren`、`Stop`。建议增加第四种状态或重命名为三态语义：

```rust
pub enum VisitorResult {
    Continue,       // 继续遍历当前节点的子节点
    Skip,           // 跳过当前节点的子节点，但继续遍历兄弟节点
    Stop,           // 终止整个遍历
}
```

**注意**：当前 `SkipChildren` 语义实际等同于上述 `Skip`。如果现有语义已满足需求，此项可跳过。但建议检查 `walk_statement` 中 `SkipChildren` 的处理是否在所有 `walk_*` 函数中行为一致。

---

### P2-2: 导出新增类型

在 `src/lib.rs` 中导出新增的公开类型：

```rust
pub use ast::visitor::{
    walk_statement,
    walk_pl_block,       // 新增
    walk_pl_statement,   // 新增
    walk_pl_declaration, // 新增
    Visitor, VisitorResult,
};
```

---

## 4. 不在范围内

以下内容**明确排除**，建议不在本次增强中实现：

- **可变遍历（VisitorMut / Fold）**：当前需求为只读遍历，可变遍历是独立的大型特性
- **AST 变换 / 重写**：不属于 Visitor trait 的职责
- **Proc macro 自动派生**：手动编写 walk 函数已足够，codegen 是后续优化
- **新的控制流机制**：保持现有 `VisitorResult`，不引入 `ControlFlow` 或 `Result` 包装
- **性能优化**（如 arena 分配、零拷贝）：不影响功能正确性

---

## 5. 验收测试建议

### 5.1 P0 验收用例

**TC-P0-01**: `CREATE FUNCTION` 体遍历

```sql
CREATE OR REPLACE FUNCTION foo(a INTEGER) RETURNS INTEGER AS $$
BEGIN
    PERFORM bar(a);
    RETURN SELECT baz(a);
END;
$$ LANGUAGE plpgsql;
```

**预期**：`walk_statement` 触发 → `visit_pl_block` → `visit_pl_statement`（Perform → walk_statement 触及 baz 的 FunctionCall）、`visit_pl_statement`（Return）。

**TC-P0-02**: `CREATE PROCEDURE` 体遍历 + 过程调用

```sql
CREATE PROCEDURE load_data(p_id INT) AS $$
BEGIN
    insert_log(p_id, 'start');
    data_core.process(p_id);
    insert_log(p_id, 'done');
EXCEPTION
    WHEN OTHERS THEN
        log_error(SQLERRM);
END;
$$;
```

**预期**：`visit_procedure_call` 被调用 3 次（`insert_log`、`data_core.process`、`insert_log`），异常块中的 `log_error` 也能被遍历到。

**TC-P0-03**: `CREATE PACKAGE BODY` 包内过程遍历

```sql
CREATE OR REPLACE PACKAGE BODY pkg_api AS
    PROCEDURE inner_proc IS
    BEGIN
        helper.do_stuff();
    END;

    FUNCTION get_val RETURN NUMBER IS
    BEGIN
        RETURN compute_val();
    END;
END pkg_api;
```

**预期**：`walk_statement` 能进入 `pkg_api` 的 `items`，对 `inner_proc` 和 `get_val` 的 `block` 各触发 `visit_pl_block` 和对应的 `visit_pl_statement` / `visit_procedure_call`。

**TC-P0-04**: 顶层 `CALL` 语句

```sql
CALL schema.run_maintenance();
```

**预期**：`visit_call` 被触发，`call.func_name` 为 `schema.run_maintenance`。

**TC-P0-05**: 嵌套块递归

```sql
BEGIN
    BEGIN
        inner_call();
    END;
    outer_call();
END;
```

**预期**：`visit_procedure_call` 先 `inner_call` 后 `outer_call`。

**TC-P0-06**: `EXECUTE` 动态 SQL

```sql
BEGIN
    EXECUTE IMMEDIATE 'CALL ' || v_proc || '()';
END;
```

**预期**：`visit_pl_statement` 触发（`PlStatement::Execute`），`string_expr` 可被遍历。`parsed_query` 为 None（动态 SQL 无法解析）时不崩溃。

### 5.2 P1 验收用例

**TC-P1-01**: `walk_expr` 完整性 — `BETWEEN` 表达式

```sql
SELECT * FROM t WHERE x BETWEEN 1 AND 10;
```

**预期**：`visit_expr` 对 `BETWEEN` 的 `expr`、`low`、`high` 三个子节点均触发。

**TC-P1-02**: `walk_select` CTE 遍历

```sql
WITH cte AS (SELECT id FROM users WHERE active = 1)
SELECT * FROM cte WHERE id > 100;
```

**预期**：CTE 内的 `SELECT` 和外层 `SELECT` 的 `WHERE` 均被遍历。

**TC-P1-03**: `walk_table_ref` JOIN 遍历

```sql
SELECT * FROM a JOIN b ON a.id = b.aid WHERE b.val > 0;
```

**预期**：`walk_table_ref` 递归进入 JOIN 的左右表和 ON 条件。

---

## 6. 实现影响评估

| 需求项 | 修改文件 | 预估工作量 |
|---|---|---|
| P0-1: PL/pgSQL trait 方法 | `visitor.rs` | ~20 行新增 |
| P0-2: PL/pgSQL walk 函数 | `visitor.rs` | ~150-200 行新增（walk_pl_block ~30 行, walk_pl_statement ~80-100 行, walk_pl_declaration ~40-50 行） |
| P0-3: 修正 walk_statement | `visitor.rs` | ~30 行修改（替换通配分支为显式匹配） |
| P0-4: visit_call / visit_procedure_call | `visitor.rs` | ~15 行新增 |
| P1-1: 增强 walk_expr | `visitor.rs` | ~80-100 行新增（补全 ~20 个 Expr 变体） |
| P1-2: 增强 walk_select | `visitor.rs` | ~30-40 行新增 |
| P1-3: 增强 walk_table_ref | `visitor.rs` | ~20-30 行新增 |
| P2-1: VisitorResult 三态 | `visitor.rs`, 所有 walk 函数 | ~10 行修改 + 全量回归 |
| P2-2: 导出新类型 | `lib.rs` | ~3 行修改 |

**总预估**：`visitor.rs` 从 337 行增长至 ~700-800 行。无其他文件需大规模修改。
