# PR #170 评审修复计划（#165–#169 跟随修复）

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 修复 PR #170 六条评审意见（4 bug + 2 suggestion，全部已对照代码核实成立）。核心目标：#167 谓词在 ELSIF/简单 CASE/函数包裹裸列/SELECT INTO 变量形态下不漏报不错报；`columns` 与 `predicates` 的过程身份可 join；机器入口不静默错配。

**Architecture:** 全部改动位于 `src/parser/predicates.rs`、`src/main.rs`、`src/mcp/tools.rs`、`src/server/handlers.rs`、`src/graph/lineage.rs`、`src/parser/extractor.rs`（仅注释）。无 store 布局变更——**不需要 bump `STORE_VERSION`（保持 12）**：F1/F2 改的是提取逻辑而非已序列化结构；F3 只改 JSON 输出字段来源；F4/F5 是解析参数与错误语义。

**已核实的评审发现（全部接受，无争议项）：**

| # | 发现 | 核实位置 |
|---|---|---|
| F1 | If 臂漏 `elsifs`；简单 CASE（`expression: Some`）把 WHEN 值当裸条件 | predicates.rs:277-287 |
| F2 | `condition_operand` 的 `expr_name(expr)?` 对 FunctionCall 断链，跳过 var_sources 与 sole-table fallback；Derived 臂硬编码 `transform: None` | predicates.rs:402, 409 |
| F3 | `cmd_predicates` 的 `procedure` 取自 NodeKey 展示串（包内过程得 `pkg.prc`），与 columns 的 `id.name`+`package` 不一致；无 `package` 字段 | main.rs:2026-2029 |
| F4 | 四处新调用点 `resolve_single_node(..., false, false)` → `Ambiguous` 臂不可达，多匹配静默取首个 | main.rs:1894/1990, tools.rs:715, handlers.rs:487 |
| F5 | resolved-but-empty 谓词非零退出，应返回 `predicates: []` | main.rs:2021-2025 |
| F6 | 注释复述控制流/带 issue 叙事；`cmd_lineage` 保留内联解析双份 | extractor.rs 多处, main.rs, lineage.rs |

---

## Fix 1 (F1): ELSIF 采集 + 简单 CASE 合成比较

**Files:** `src/parser/predicates.rs`（visitor 的 `PlStatement::If`/`PlStatement::Case` 臂）；测试同文件 tests 模块 + `tests/regress_predicates.rs`。

**Step 1 (Red):**
- 单测 `elsif_conditions_collected_as_predicates`：`IF r.x = '1' THEN ... ELSIF r.x = '2' THEN ... ELSIF r.x = '3' THEN ...`（record ctx）→ 3 条谓词，全部 `PredicateKind::If`、High、同表 clauses，id 递增；ELSIF 的 line 取各自 span（若 span 可得，否则 0——与现行主条件取法一致）。
- 单测 `simple_case_synthesizes_expression_comparison`：`CASE r.x WHEN '1' THEN ... WHEN '2' THEN ...`（`expression: Some`）→ 每条 WHEN 产出 `column: x, op: Eq, value: '1'/'2'` 的 High 谓词（合成 `expression = when.condition`），而非裸字面量 Low。
- e2e：`tests/regress_predicates.rs` 增补 fixture 断言 ELSIF 数量与简单 CASE 的 clauses。

**Step 2 (Green):**
- If 臂：主条件 push 后遍历 `spanned.elsifs`，逐个 `push_condition(&elsif.condition, PredicateKind::If, elsif 行号)`。
- Case 臂：`spanned.expression` 为 `Some` 时，对每个 when 合成比较表达式（构造 `Expr::BinaryOp { left: expression.clone(), op: "=".into(), right: when.condition.clone() }` 或等价内部表示——以 `push_condition` 现有输入类型为准，必要时新增 `push_equality(expression, when_value)` 内部路径），`expression: None`（搜索型 CASE）保持现行为。
- 跑 F1 既有测试确认不回归（搜索型 CASE 测试 `case_when_yields_predicates` 必须保持绿、语义不变）。

## Fix 2 (F2): condition_operand 断链修复 + Derived 携带 transform

**Files:** `src/parser/predicates.rs`。

**Step 1 (Red):**
- `naked_column_substr_resolves_via_sole_table`：单游标表 ctx + `IF substr(stock_kind,1,2) = '05'` → High 谓词，clause 带 `transform: Some(substr[1,2])`（当前实际：Low 无谓词）。
- `select_into_var_substr_resolves_via_var_source`：`SELECT kind_id INTO v_kind FROM swh_all_kind ...; IF substr(v_kind,1,2) = '05'` → Derived clause 指向 `swh_all_kind.kind_id` 且 **transform 携带**（当前：断链 Low）。

**Step 2 (Green):**
- `expr_name(expr)?` 改为可失败但不提前中断：将 `var_sources` 查找的键改为 `expr_name(expr)` **或** `column_transform_of(expr)` 的目标列名（裸列名，小写）；两键都查不到才落入 sole-table fallback（`names.len()==1 && tables.len()==1` 分支，现有 transform 透传已就绪）。
- Derived 臂的 `PredicateClause` 携带与 Direct 臂相同的 `transform`（删除硬编码 `None`；var_sources 命中的是变量名包裹形态时 transform 语义同样成立）。
- 注意 fallback 顺序保持：记录字段（`resolved_clause`）→ var_sources → sole-table；不改变记录字段路径的既有行为（`transformed_condition_clause_carries_transform` 等测试保持绿）。

## Fix 3 (F3): predicates 输出身份对齐 columns

**Files:** `src/main.rs`（`cmd_predicates` + `PredicatesResult`）；`tests/regress_predicates.rs`。

**Step 1 (Red):** e2e `predicates_identity_matches_columns_for_packaged_procedure`：包内过程 fixture → `codeweb predicates --format json` 的 `procedure` == `columns` 的 `procedure`（均为裸名），且 predicates JSON 新增 `package` 字段 == 包名（columns 同名字段一致）。当前实际：`procedure == "pkg.prc"` 且无 package 字段 → FAIL。

**Step 2 (Green):**
- `PredicatesResult` 增 `#[serde(default, skip_serializing_if = "Option::is_none")] package: Option<String>`（纯 JSON 输出结构，非 bincode 持久化——skip 安全；仿 `AggregatedColumnAnalysis` 的 package 字段风格）。
- `cmd_predicates` 不再从 NodeKey 展示串 split：从图节点 `RoutineId` 取 `name` 与 `package`（对齐 `column_analysis_of_routine` 的取法）。
- 独立过程 `package: None`（JSON 省略），schema_version 不变。

## Fix 4 (F4): 歧义显式失败，消灭静默首匹配

**Files:** `src/main.rs`（`cmd_columns`/`cmd_predicates` 两处）、`src/mcp/tools.rs`（`resolve_node`）、`src/server/handlers.rs`（`resolve_node`）；测试 `tests/regress_columns.rs`、`tests/regress_predicates.rs`、`tests/mcp_test.rs`、`tests/serve_api.rs`。

**Step 1 (Red):**
- e2e：同前缀双过程 fixture（如 `prc_order` / `prc_order_header`）→ `codeweb columns --procedure prc_order` 非零退出且 stderr 提示歧义（当前实际：静默返回首个 + exit 0）；`codeweb predicates` 同理。
- serve_api：`GET /api/v1/columns?procedure=prc_order` → 409 或 400（按 handlers 既有错误约定选一个，报告所选）；mcp_test：`codeweb_column_analysis` 歧义名返回 error JSON（区分 Empty 的 "No nodes matching" 文案）。

**Step 2 (Green):**
- 四处调用第 4 参 `fail_on_multiple` 改 `true`；`cmd_*` 的 `ResolveResult::Ambiguous` 臂从死代码变为可达（保留现有非零错误路径）。
- MCP `resolve_node` 返回区分 `Empty`（"No nodes matching ..."）与 `Ambiguous`（"Ambiguous match: N candidates ..."）；HTTP 对应 404 vs 400（报告所选映射）。
- 不动 `trace`/`detail`/`impact` 等既有调用点的语义（它们本就交互式，首匹配+stderr 提示是既有契约）。

## Fix 5 (F5): resolved-empty 返回空数组

**Files:** `src/main.rs`；`tests/regress_predicates.rs`。

**Step 1 (Red):** `predicates_empty_branches_return_empty_array`：存在但无 IF/CASE 的过程 → exit 0、stdout 为 `{schema_version, procedure, predicates: []}`（当前实际：非零 + "No PL predicates found"）。

**Step 2 (Green):** `cmd_predicates` 中 store 侧表 miss/resolved-empty 不再 `?` 报错，改输出空 `predicates`；非零保留给：名称未解析（Empty）、歧义（F4 后可达）。注意与 F4 的歧义错误路径不冲突。

## Fix 6 (F6): 注释卫生 + cmd_lineage 共享 parse_lineage_target

**Files:** `src/parser/extractor.rs`（仅注释）、`src/parser/predicates.rs`（仅注释）、`src/graph/lineage.rs`、`src/main.rs`。

**内容：**
- 精简复述控制流的注释；保留并压缩非显性 WHY（HardFilter/PredicateClause 的 bincode 固定字段数约束一句话足够）；删除 issue 编号/评审轮次/"intentionally left untouched" 类叙事。
- `cmd_lineage` 改为调用 `graph::lineage::parse_lineage_target`（消除 T5 留下的内联双份及其叙事注释）；行为必须逐字不变——`tests/regress_lineage_table_upstream.rs`、`tests/regress_column_lineage.rs`、`tests/regress_issue_154_lineage_targets.rs` 全套保持绿不动即验证。

---

## 执行顺序与门禁

```
F1 → F2（同文件连续 Red→Green） → F3 → F4 → F5 → F6（纯清理收尾）
```

每项独立 Red→Green；每完成两项跑一次：
```bash
cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_
cargo clippy --features full -- -D warnings
cargo fmt --all -- --check
```
最终全量门禁 + `cargo build --features full`。

**Never 红线（AGENTS.md）**：不删/跳过/改写既有测试（F3/F4/F5 的新行为一律新增测试表达；若既有测试因 F4/F5 语义变化失败——如某测试断言了旧的静默首匹配——STOP 并报告，不得擅改）；不引入依赖/feature/unsafe/`#[allow]`；不动 `STORE_VERSION`；F6 不改任何行为语义（仅注释与等价重构，行为守护靠既有套件全绿）。
