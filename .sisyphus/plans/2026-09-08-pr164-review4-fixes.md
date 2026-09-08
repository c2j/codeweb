# PR #164 第四轮 Review 修复计划：嵌套作用域继承 + 注释去历史（#158 追加 IV）

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 修复 PR #164 第四轮 review（c2j）：`AnchorExtractor` 嵌套例程臂的 `mem::take` 清空继承作用域（嵌套体引用外层/包级 cursor/参数名时伪造 `table*`）；store.rs 两处注释历史叙事回潮。

**Architecture:** 单点语义修正——嵌套臂 `take` 改 `clone`（词法继承 + 新声明隔离），注册嵌套参数顺序不变；注释去历史化。无新文件、无新依赖。

**Tech Stack:** 不变。基线：分支 `feat/issue-158` @ `0f22d3f`。

**参考:** PR #164 review（c2j，2 条，已逐条代码验证：extractor.rs:1265-1290 的 `std::mem::take`；store.rs:1322/:3802 的 commit-hash 叙事）。

---

## 0. 已验证事实（实现者必读）

1. **继承作用域被清空**（extractor.rs:1265-1290）：`NestedProcedure`/`NestedFunction` 两臂用 `std::mem::take` 保存 `cursor_names`/`var_names`——take **清空**原集，嵌套 walk 期间两集合为空。PL/SQL 嵌套子程序继承外层 DECLARE 局部名、外层例程参数（`collect_routine_anchor_edges` :1864-1866 注册）、包级名（:1858-1863 注入）。后果：嵌套体 `rec c%ROWTYPE`（c 为外层/包级 cursor）或 `v p_emp.empno%TYPE`（p_emp 为外层参数）不受守卫 → 伪 `table*` 挂外层节点。
2. **CallExtractor 的 take 先例不适用**：其 `local_vars` 是"调用解析 vs 标识符"集合，每例程清空重启在那边语义正确（:486-503）；`%TYPE`/`%ROWTYPE` 守卫集需要**词法继承**。
3. **现状正确部分（必须保持）**：嵌套参数注册（:1268-1270/:1281-1283）使嵌套参数在嵌套作用域内正确遮蔽外层名；`SkipChildren` 防默认递归双走；walk 后 restore 防嵌套新声明泄漏外层（`should_not_leak_nested_routine_locals_into_outer_scope` 锁定）。
4. **历史叙事回潮**（store.rs:1322 `regression fixed in commit following d667927...`、:3802 `Regression guard (#158, commit d667927)`、以及 `should_union_table_access_modes_across_stores_on_merge` 测试 doc 开头的 commit hash）：`c47fa7e`/`f4de1c9` 清理过两轮，本轮再犯。保留不变量陈述，删 commit hash 与"上次破坏"回顾。

## 语义决策（D-H，review 建议采纳）

**嵌套作用域 = 词法继承**：进入嵌套例程时 **clone** 两集合（外层/包级名在嵌套体内继续生效）→ 注册嵌套参数名（遮蔽外层同名，仅嵌套作用域内）→ walk → restore 为克隆前快照（嵌套内新声明不外泄）。与 D-F 的差别仅 take→clone；D-F 的"防泄漏外泄"目标不变。

---

## Task 1: 嵌套臂 take→clone（review bug）

**Files:**
- Modify: `src/parser/extractor.rs:1265-1290`（两臂）
- Test: extractor.rs tests

**Step 1: 失败测试**

```rust
#[test]
fn should_skip_nested_body_anchor_using_outer_cursor() {
    // 外层 CURSOR c + 嵌套例程体内 rec c%ROWTYPE
    // 断言：anchors 为空（c 在嵌套作用域内仍被守卫——词法继承）
    let sql = "CREATE FUNCTION f() RETURN INTEGER AS \
        CURSOR c IS SELECT id FROM t_main; \
        PROCEDURE inner IS rec c%ROWTYPE; BEGIN NULL; END inner; \
        BEGIN NULL; END;";
    assert!(extract_anchors(sql).is_empty(), "outer cursor must stay guarded inside nested body: {:?}", extract_anchors(sql));
}

#[test]
fn should_skip_nested_function_body_anchor_using_outer_param() {
    // 外层函数参数 p_emp + 嵌套 FUNCTION 体内 v p_emp.empno%TYPE
    // 断言：anchors 为空（参数名词法继承）
    let sql = "CREATE FUNCTION f(p_emp INTEGER) RETURN INTEGER AS \
        FUNCTION inner_f RETURN INTEGER IS v p_emp%TYPE; BEGIN RETURN v; END inner_f; \
        BEGIN RETURN NULL; END;";
    assert!(extract_anchors(sql).is_empty(), "outer param must stay guarded inside nested body: {:?}", extract_anchors(sql));
}
```

（若 ogsql-parser 对嵌套块内混合顺序/语法解析有出入，微调 SQL 字面量保持断言语义；报告说明。）

**Step 2:** Run: `cargo test should_skip_nested_body_anchor_using_outer_cursor` 与 `cargo test should_skip_nested_function_body_anchor_using_outer_param` → Red（take 清空导致伪锚产生，anchors 非空）。

**Step 3: 最小实现**（两臂同改）

```rust
let saved_cursors = self.cursor_names.clone();
let saved_vars = self.var_names.clone();
// ... 注册嵌套参数 + walk 不变 ...
self.cursor_names = saved_cursors;
self.var_names = saved_vars;
```

**Step 4:** Run: 两个新测试 PASS；**回归重点**：`should_skip_type_anchored_to_nested_proc_param`（嵌套参数遮蔽仍生效）、`should_not_leak_nested_routine_locals_into_outer_scope`（克隆后 walk 的新声明不污染克隆前快照——restore 语义不变，必须仍绿）、`should_collect`/`should_skip` 全集、`cargo test --test regress_issue_158_type_anchor_edges`(10)。

**Step 5: Commit**

```bash
git commit -m "fix(parser): 嵌套例程作用域改词法继承（take→clone），外层/包级名守卫贯穿嵌套体 (#158)"
```

---

## Task 2: store.rs 注释去历史化（review suggestion）

**Files:**
- Modify: `src/graph/store.rs:1322` 附近（`anchor_merge_key` doc）、`:3802` 附近（`should_union_table_access_modes_across_stores_on_merge` doc）及其它 grep 命中

**Step 1:** grep `d667927|regression fixed in commit|Regression guard.*commit` 全部命中改写为当前不变量陈述，例如：
- `anchor_merge_key` doc → "AnchorsOn edges use a merge-spanning (src, dst, kind, column, site) key: identical anchors collapse across stores while distinct columns survive. Every other edge type keeps the per-store (src, dst, tag) key so `merge_duplicate_table_access_edges` can still union TableAccess modes across stores."
- 测试 doc → "Regression guard (#158): a whole-merge generic key would drop the second store's TableAccess edge before mode union; per-store keys + the dedicated AnchorsOn key above keep both behaviors."（去掉 commit hash）

**Step 2:** Run: `cargo build --features full` → 0 错误（纯注释）；`grep -rn "d667927" src/` → 零命中。

**Step 3: Commit**

```bash
git commit -m "docs(store): merge 键注释去历史化，陈述当前不变量 (#158)"
```

---

## Task 3: 全量门禁 + push + 回复

**Step 1: 门禁**

```bash
cargo fmt --all -- --check
cargo clippy --features full -- -D warnings
cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_
```

**Step 2:** push + 逐条回复 2 条 review 意见（引用 commit）。

---

## Non-goals（维持）

- 嵌套 RETURN 类型锚定、cursor earlier-only、line 精度、D3/D4

## 完成标准

- [ ] 2 条意见 1:1 闭环；既有嵌套/守卫测试零回归（尤其 `should_not_leak_nested_routine_locals_into_outer_scope` 与 `should_skip_type_anchored_to_nested_proc_param`）
- [ ] 全量门禁三连绿；PR push + 回复
