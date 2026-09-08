# PR #164 第三轮 Review 修复计划：SPEC/BODY 重复边 + 嵌套作用域 + merge 不变量（#158 追加 III）

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 修复 PR #164 第三轮 review（c2j）：SPEC+BODY 双声明的重复签名锚定边、嵌套例程参数不注册与作用域泄漏、`merge` 路径重新引入 anchors_on 折叠、历史叙事注释回潮。

**Architecture:** 既有骨架收敛——锚定去重集上移为调用方持有（pass 级贯穿 SPEC+BODY）、`AnchorExtractor` 镜像 `CallExtractor` 的嵌套作用域 save/restore 先例、`merge` 边键对齐 `dedup` 的 `(kind, column, site)` 修正。无新文件、无新依赖。

**Tech Stack:** 不变。基线：分支 `feat/issue-158` @ `73b50c6`。

**参考:** PR #164 review id 5139218557（4 条，已逐条代码验证属实）。

---

## 0. 已验证事实（实现者必读）

1. **SPEC+BODY 重复边**（builder.rs:2240-2283）：`collect_package_object_ref_edges` 对 Procedure/Function 不区分 SPEC（block=None）/BODY；`collect_routine_anchor_edges` :2283 的 `let Some(block) = ... else { return }` 在**签名锚定发射之后**。`create_sql_nodes` 先于锚定 pass 完成全文件节点（SPEC+BODY 共享 `RoutineId` 节点）→ SPEC 与 BODY 各发一遍相同 Param/ReturnType 锚定边；`anchor_seen` 单次调用局部；`build()`/`analyze()` 不跑 `store.dedup()` → 消费者看到重复边。
2. **嵌套例程缺口**（extractor.rs）：`AnchorExtractor` 无 `NestedProcedure`/`NestedFunction` 臂（`CallExtractor` 在 :485-496 有 `begin_routine_scope` save/restore 先例；:483 有 SkipChildren+手动 walk 注释）。默认 walker 以同一 extractor 递归进嵌套块：嵌套参数不注册（`v p_emp.empno%TYPE` 伪造 `table* p_emp`）+ 嵌套局部名泄漏进外层跳过集。
3. **merge 折叠**（store.rs:1363）：`merge` 的 `seen_edges: HashSet<(NodeKey, NodeKey, String)>` 只用 edge_type_tag 键——`p1 emp.id%TYPE` + `p2 emp.name%TYPE` 合并时折叠，违反 `should_keep_distinct_anchor_edges_through_dedup` 为 dedup() 锁定的同一不变量（dedup 已修、merge 未修）。
4. **历史叙事注释回潮**：builder.rs:1828/2129/2143/2157/2210/5224/5268/5314 等 8 处 "PR #164 review round 2 ..." 措辞（c47fa7e 清理过一轮）。
5. `AnchorDedupKey` 类型别名已在 builder.rs 模块级（Task 7）；`VisitorResult::SkipChildren` 存在（:483 注释）。

## 语义决策（review 建议，已采纳）

- **D-E pass 级去重**：`collect_routine_anchor_edges` 的 `anchor_seen` 上移为调用方持有 `HashSet<(NodeIndex, AnchorDedupKey)>`（proc 维度入键）；顶层调用每次建新集（行为不变），`collect_package_object_ref_edges` 建一个**贯穿 SPEC+BODY** 的集（签名相同折叠、签名不同两条都保留——不偏向 SPEC）。
- **D-F 嵌套作用域**：镜像 CallExtractor——save `cursor_names`/`var_names` → 注册嵌套参数名 → 手动 walk 嵌套块 → restore → `SkipChildren` 防默认递归双走。嵌套 RETURN 类型锚定**不做**（嵌套例程无独立节点，维持 Task 7 语义；reviewer 未要求）。
- **D-G merge 键**：`anchors_on` 边在 merge 的 seen 判定中扩展 `(kind, 小写 column, site)`（对齐 dedup 修复）；非锚定边键不变。

## 任务依赖

Task 1（pass 级去重）独立；Task 2（嵌套作用域）独立；Task 3（merge 键）独立；Task 4 收尾。建议顺序 1 → 2 → 3 → 4。

---

## Task 1: SPEC+BODY pass 级锚定去重（review bug）

**Files:**
- Modify: `src/graph/builder.rs`（`collect_routine_anchor_edges` 签名：`anchor_seen` 改为调用方持有并带 proc 维度；`collect_package_object_ref_edges` 建包级集；顶层调用点适配）
- Test: builder.rs tests

**Step 1: 失败测试**

```rust
#[test]
fn should_dedupe_signature_anchors_across_spec_and_body() {
    // 两条语句：CREATE PACKAGE ... PROCEDURE p(t t%ROWTYPE);
    //           CREATE PACKAGE BODY ... PROCEDURE p(t t%ROWTYPE) IS BEGIN ... END;
    // （SPEC 无 body，BODY 有）
    // 断言：p → 目标表 恰好 1 条 AnchorsOn（site=Param），不是 2 条
    // 附加：BODY 再放一个 SPEC 没有的 DECLARE 变量锚（site=Variable）→ 仍产生（不同 site 不折叠）
}
```

**Step 2:** Run: `cargo test should_dedupe_signature_anchors_across_spec_and_body` → Red（2 条相同边）。

**Step 3: 最小实现**

- `collect_routine_anchor_edges` 签名：删本地 `anchor_seen`，新参 `anchor_seen: &mut HashSet<(petgraph::graph::NodeIndex, AnchorDedupKey)>`；三处发射检查改为 `anchor_seen.insert((proc_idx, Self::anchor_dedup_key(a)))`
- 顶层 CreateProcedure/CreateFunction 调用点：各自 `let mut seen = HashSet::new();` 传入（单例程语义不变）
- `collect_package_object_ref_edges`：函数顶建一个集，**SPEC 项与 BODY 项的全部调用共享传入**

**Step 4:** Run: 新测试 PASS；回归 `cargo test should_skip_signature_anchor` + `should_keep_table_access_and_anchor_edges_separate` + `should_dedupe_signature_anchors_across_spec_and_body` + `cargo test --test regress_issue_158_type_anchor_edges` 全绿。

**Step 5: Commit**

```bash
git commit -m "fix(graph): SPEC/BODY 双声明签名锚定 pass 级去重 (#158)"
```

---

## Task 2: 嵌套例程作用域（review suggestion 1）

**Files:**
- Modify: `src/parser/extractor.rs`（`AnchorExtractor::visit_pl_declaration` 加 Nested 臂）
- Test: extractor.rs tests

**Step 1: 失败测试**

```rust
#[test]
fn should_skip_type_anchored_to_nested_proc_param() {
    // 外层函数体内嵌套 PROCEDURE inner(p_emp VARCHAR2) IS v p_emp.empno%TYPE; ...
    // 断言：anchors 为空（p_emp 是嵌套参数，伪造 table* 被守卫）
}

#[test]
fn should_restore_scope_after_nested_routine() {
    // 嵌套例程声明局部名 orders（INTEGER），嵌套之后外层再 DECLARE v2 orders%TYPE 不可行
    // （DECLARE 顺序），改为：嵌套块内声明 cursor/orders，外层后续语句锚定 orders
    // —— 由于声明顺序限制，改为断言：嵌套内注册的参数名不泄漏到嵌套之后
    //   的任何锚定（构造：嵌套后无声明可行锚定点时，用同 SQL 的 BEGIN 体锚定；
    //   若 SQL 无法构造该顺序，改为直接断言 walk 前后 cursor_names/var_names 快照相等
    //   —— 通过 extractor 公共字段 anchors + 既有断言路径实现，或在测试内访问
    //   #[cfg(test)] 可见状态。以最简可行为准，报告说明选择。）
}
```

**Step 2:** Run: 第一个测试 Red（伪造 `table* p_emp`）。第二个按实际可行形态写（作用域恢复是 Task 的正确性核心）。

**Step 3: 最小实现**（镜像 CallExtractor :485-496 先例；先读它）

```rust
PlDeclaration::NestedProcedure(p) | PlDeclaration::NestedFunction(f) => {
    // 嵌套例程有自己的参数与作用域（镜像 CallExtractor 的 begin_routine_scope）：
    // save → 注册嵌套参数 → 手动 walk 嵌套块 → restore；SkipChildren 防默认递归双走
    let saved_cursors = std::mem::take(&mut self.cursor_names);
    let saved_vars = std::mem::take(&mut self.var_names);
    let (params, block) = match decl { /* 解构 NestedProcedure/NestedFunction 的 parameters/block */ };
    for param in params { self.register_var_name(&param.name); }
    if let Some(b) = block { walk_pl_block(self, b); }
    self.cursor_names = saved_cursors;
    self.var_names = saved_vars;
    VisitorResult::SkipChildren
}
```

（`parameters`/`block` 字段名以 `PackageProcedure`/真实嵌套声明结构为准核对；`std::mem::take` 需要 Default 或用 clone/restore——`HashSet<String>` 有 Default，直接 take。）

**Step 4:** Run: 新测试 PASS；回归 `cargo test should_collect` + `should_skip` 全集 + `cargo test --test regress_issue_158_type_anchor_edges`。

**Step 5: Commit**

```bash
git commit -m "fix(parser): 嵌套例程参数注册 + 作用域 save/restore（镜像 CallExtractor） (#158)"
```

---

## Task 3: merge 路径 anchors_on 键（review suggestion 2）

**Files:**
- Modify: `src/graph/store.rs`（`merge` 的 seen 判定 :1363 附近）
- Test: store.rs tests

**Step 1: 失败测试**（平行于 `should_keep_distinct_anchor_edges_through_dedup`）

```rust
#[test]
fn should_keep_distinct_anchor_edges_through_merge() {
    // store_a：proc → emp 两条 AnchorsOn（column Some("id") / Some("name")，site 同）
    // store_b：最小空/无关图
    // merge(a, b) → 断言合并结果仍有 2 条不同列的 AnchorsOn
    // （若 merge 语义是累加器模式，按真实实现构造：以实际代码为准，报告说明）
}
```

**Step 2:** Run: `cargo test should_keep_distinct_anchor_edges_through_merge` → Red（折叠成 1 条）。

**Step 3: 最小实现**

merge 循环（:1363-1380）读边时：非 `Edge::AnchorsOn` 维持原 `(src, dst, tag)` 键；`Edge::AnchorsOn` 边键扩展为 `(src, dst, "anchors_on", kind, column 小写, site)`。实现形态：新增平行集合 `seen_anchor_keys: HashSet<(NodeKey, NodeKey, AnchorKind, Option<String>, AnchorSite)>`（`AnchorKind`/`AnchorSite` 从 crate::parser 引入，已 Copy+Eq+Hash），命中任一集合即视为重复。**同时检查累加器中已存在的边**（reviewer 明示）——读 merge 对 accumulator 已有边的处理点，套用同键判定。

**Step 4:** Run: 新测试 PASS；回归 `should_keep_distinct_anchor_edges_through_dedup` + `should_roundtrip_anchors_on_edge_through_bincode_store` + store 全部锚定测试。

**Step 5: Commit**

```bash
git commit -m "fix(store): merge 路径保留不同列锚定边（对齐 dedup 键语义） (#158)"
```

---

## Task 4: 历史叙事注释清理 + 全量门禁（review suggestion 3）

**Step 1:** 清理 builder.rs:1828/2129/2143/2157/2210/5224/5268/5314 及 grep `review round\|#164 review\|review Issue` 的全部命中——保留规则语义、删 "PR #164 review..." 引用（含 extractor.rs 同类，若有）。

**Step 2: 全量门禁**

```bash
cargo fmt --all -- --check
cargo clippy --features full -- -D warnings
cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_
```

**Step 3: Commit**

```bash
git commit -m "docs: 清理锚定注释的历史叙事措辞 (#158)"
```

---

## Non-goals（维持）

- 嵌套例程 RETURN 类型锚定（嵌套例程无独立图节点，维持 Task 7 语义）
- cursor 名 earlier-only、line 精度、游标 RETURN 锚定（D3）、CGEF 白名单（D4）

## 完成标准

- [ ] 4 条意见 1:1 闭环；`should_dedupe_signature_anchors_across_spec_and_body` 恰 1 条边
- [ ] 既有全部锚定测试零回归；全量门禁三连
- [ ] PR #164 push + 逐条回复（引用 commit）
