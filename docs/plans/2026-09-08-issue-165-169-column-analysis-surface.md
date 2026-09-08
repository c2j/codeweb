# Issue #165–169 列级分析查询面与解析增强（columns / predicates / transform / 跨表键 / 文档）

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 打通「列级分析 → 造数/mock 机器可读入口」主线，覆盖五个 issue：
1. **#169** — 函数包裹列的字面量过滤纳入 `HardFilter`（substr/nvl/trim 白名单 + `transform` 字段）；
2. **方案A（用户已拍板）** — `merge_table_access_edges` 合并全部诊断字段（当前只合并 `column_mappings`/`read_tables`，其余「保留第一条」，导致同过程多语句同表时 hard_filters/join_conditions 丢失）+ `STORE_VERSION` 9→10；
3. **#165 P0** — `codeweb columns --procedure X --format json` 按过程聚合导出 ColumnAnalysis；
4. **#168** — WHERE/JOIN ON/SELECT INTO 中 `%ROWTYPE` 记录字段解析为跨表等值键；
5. **#165 P1** — MCP `codeweb_column_analysis`/`codeweb_lineage` + HTTP `GET /api/v1/columns`/`GET /api/v1/lineage`；
6. **#167** — PL IF/CASE 条件解析为表列谓词（置信度分级 + param_table_hint）；
7. **#166** — 文档补齐（lineage CLI、ColumnAnalysis 字段、新命令）。

**Architecture:** 全部改动在单 crate 内，无新外部依赖、无新 feature flag：
- **解析层** `src/parser/extractor.rs`：T1 白名单 transform、T4 记录字段等值键、T6 新谓词提取 pass；
- **图构建层** `src/graph/builder.rs`：T2 合并诊断字段（`merge_table_access_edges` L3330-3421）；
- **查询层** `src/graph/`（新增聚合函数）+ `src/main.rs`（新 CLI 子命令）+ `src/mcp/tools.rs` + `src/server/handlers.rs`；
- **存储** `src/graph/store.rs`：`STORE_VERSION` 9→10（T2，含 D4 合并 bump）；T6 再 bump 至 11（D6 已锁定存储方案）。

**Tech Stack:** Rust stable、ogsql-parser v0.10.0（git 依赖，checkout `~/.cargo/git/checkouts/ogsql-parser-9b270b8f87a071f2/28b5b4b`）、现有测试 harness（extractor.rs `#[cfg(test)]` 单测 + `tests/regress_*.rs` 端到端 + `tests/serve_api.rs` + `tests/mcp_test.rs`）。

---

## 决策记录（全部锁定：D1 用户拍板；D2–D6 依用户委托由 Momus 审核裁决）

> **裁决说明**：Momus 第一轮审核（2026-09-08）确认 D2–D6 实质方向无异议、要求正式锁定以免实施阻塞。以下裁决即为最终结论，Task 4/6 按此执行，不再保留「建议」状态。

| # | 问题 | 选项 | 裁决与理由 |
|---|---|---|---|
| **D1** | #165 store 合并丢数据 | A: 合并诊断字段+bump / B: 接受少报 | **✅ 用户已拍板：方案A** |
| **D2** | #168 `JoinConditionSource` | 新增 `RecordField` 变体 / 复用 `ImplicitWhere` | **✅ 锁定：新增 `RecordField` 变体**。store 文件兼容由版本门禁隔离（旧二进制读不了新 store，无枚举反序列化问题）；JSON export 是单向输出，codeweb 自己不回读；唯一消费者 fastaas 是共建中的新代码，可同步适配。语义价值：下游需区分「隐式等值 JOIN」与「记录字段推导键」（置信度不同） |
| **D3** | #167 输出形态 | 独立 `codeweb predicates` / 并入 `columns` JSON | **✅ 锁定：独立 `codeweb predicates` 命令**，schema 复用 `FilterOperator`/`FilterValue`（issue 允许）。`columns` 保持聚焦列约束面；两 issue 解耦交付，TDD 分层清晰。MCP/HTTP 的 predicates 入口**暂缓**（issue 验收未强制，YAGNI） |
| **D4** | #169 是否 bump 版本 | 单独 bump / 与 T2 合并一次 | **✅ 锁定：与 T2 合并为一次 v9→10**。`transform` 是 serde-default 新字段，技术上无需 bump，但按 v7→v8 先例（加 `read_tables` 即 bump）+ 借 `store_is_current()` 促使用户重跑 analyze |
| **D5** | #169 白名单边界 | 仅逗号语法 FunctionCall / 双变体；白名单集合 | **✅ 锁定：同时处理 `FunctionCall` + `SpecialFunction`**（ogsql 文档明言 dual-variant：`SUBSTR(x FROM 1 FOR 2)` 走 SpecialFunction，只处理逗号语法会留下「换写法就漏」的坑）；白名单 **{substr, substring, nvl, trim, upper, lower}**（lower 与 upper 对称，成本≈0）。封闭白名单，不开放任意函数 |
| **D6** | #167 谓词存哪 | (a) analyze 期存入 GraphStore（`procedure_predicates` 侧表，serde default，bump v11）/ (b) 查询期重解析源文件 | **✅ 锁定：(a) 存储方案**。PL IF/CASE 分支结构在 `extract_body_sql` 摊平后即丢失，查询期重解析依赖源文件未变，脆弱且与「分析结果进 store」的既有架构一致。代价是多一次 bump（v11） |

---

## 关键代码位置（当前实现，改动点）

`src/parser/extractor.rs`：

```rust
// L1930 — HardFilter（T1 加 transform 字段）
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HardFilter {
    pub table: Option<String>,
    pub column: String,
    pub operator: FilterOperator,
    pub value: FilterValue,
}

// L2355-2505 — process_expr_for_joins_and_filters（T1 六个比较分支加白名单 arm；T4 等值分支加记录字段解析）
"=" => {
    if let (Some(l), Some(r)) = (as_column_ref(left), as_column_ref(right)) { /* equi-join L2370 */ }
    else if let Some(col) = as_column_ref(left) { /* col = literal → HardFilter L2385 */ }
    else if let Some(col) = as_column_ref(right) { /* literal = col → HardFilter L2389 */ }
    // ← FunctionCall/SpecialFunction 包裹列目前三条路全不匹配，静默丢弃
}

// L2561 — add_hard_filter（table 只经 resolve_alias 解析，无 transform）
// L3149 — column_source()（T4 复用其记录字段解析规则：精确 output_name 匹配 → 游标源列；
//         catch-all（SELECT */动态SQL）→ 游标锚表+字段名；表锚定 %ROWTYPE → 锚表+字段名）
// L3149 所在 impl 已持有 record_cursors / cursor_sources（ProcedureVarContext，L2000）
```

`src/graph/builder.rs`：

```rust
// L3330-3421 — merge_table_access_edges（T2：除 column_mappings/read_tables 外，
// 其余诊断字段 join_conditions/hard_filters/enum_mappings/select_into/
// insert_columns/update_columns/column_refs/alias_map 当前「保留第一条」，改为集合并集去重）
```

`src/graph/store.rs`：L22 `STORE_VERSION: u32 = 9`（T2 → 10；T6 若走存储方案 → 11）。
`src/graph/lineage.rs`：L1480 `mappings_of_routine`（T3 聚合函数的范本——HashSet 去重、扫入边+出边）。
`src/main.rs`：L379-411 `Lineage` variant（T3/T6 新子命令的克隆范本）；L1558-1565 v7 软提示范本；L148-179 `ImpactResult`（`schema_version` 字段房屋风格）。
`src/mcp/tools.rs`：L124-532 六工具注册（`#[tool(description=...)]`）；L539-549 `tool_handler` instructions。
`src/server/handlers.rs`：L24-41 `router()`；L435-479 `trace` handler（Query-struct GET 范本）。
`tests/mcp_test.rs`：L185-199 `test_mcp_tools_list` 硬编码 6 工具名，加工具必改。

**AST 事实（ogsql-parser v0.10.0，已核实）**：
- `Expr::FunctionCall { name: ObjectName, args: Vec<Expr>, ... }`（ast/mod.rs:1221，逗号语法）；
- `Expr::SpecialFunction { name, args, ... }`（ast/mod.rs:1384，关键字语法——`SUBSTRING(x FROM 1 FOR 3)`、`TRIM(LEADING ... FROM ...)`）。文档要求 dual-variant 处理；
- `PlIfStmt { condition: Expr, then_stmts, elsifs: Vec<PlElsif>, else_stmts }`（ast/plpgsql.rs:237）、`PlCaseStmt { expression, whens: Vec<PlCaseWhen>, else_stmts }`（L251）；`walk_pl_statement` 自动递归条件+分支体；
- WHERE 表达式：`Expr::BinaryOp{left,op:String,right}`、`Between`、`InList`、`Like`、`Case`、`FieldAccess{object,field}`（L1302）、`PlVariable`（L1415）；
- 记录字段在 SQL 中解析为多段 `ColumnRef`（`r.security_id` → 2 Idents），`split_alias_column`（extractor.rs L3872）已按此形状处理。

**测试基础设施（现有）**：`column_mappings_of(sql)` 等 helper（extractor.rs tests，L4924 起）；`ColumnAccessExtractor::new_with_context(&ProcedureVarContext)`（L2078，单测接缝）；`tests/regress_column_lineage.rs` 的 `project_with_sql` + `lineage()` harness；`run_codeweb_in`（tests/regress_lineage_table_upstream.rs L28）。**注意**：`par_sys_purchase`/`r_get_purchase`/STEP3 样例仓内不存在，T3/T4/T6 需自建 fixture。

---

## Task 1 (T2): 方案A — merge_table_access_edges 合并全部诊断字段 + STORE_VERSION 10

**Files:**
- Modify: `src/graph/builder.rs`（`merge_table_access_edges` L3330-3421）
- Modify: `src/graph/store.rs`（L22 `STORE_VERSION` 9→10；版本注释）
- Modify: `src/parser/extractor.rs`（若 `JoinCondition`/`HardFilter`/`EnumMapping`/`SelectIntoMapping`/`InsertColumnInfo`/`UpdateColumnInfo`/`ColumnRef` 缺 `Hash`，补 derive——所有字段均为 String/枚举/Vec<String>，可哈希）
- Test: `src/graph/builder.rs` `#[cfg(test)]`（若无测试模块则在 store.rs 或新建 `tests/regress_column_analysis_merge.rs`）

**Step 1: 写失败测试（Red）**

单测：同一过程两条语句写同一张表、各带不同 `hard_filters` 与 `join_conditions`，经 builder 构建后该 `(proc, table)` 边的 `column_analysis` 应为并集：

```rust
/// 方案A (issue #165): merged TableAccess edges must UNION diagnostic fields,
/// not keep only the first edge's. Two statements → same proc/table pair with
/// distinct hard filters must both survive.
#[test]
fn merge_table_access_unions_hard_filters_and_joins() {
    // 构建：CREATE TABLE t(a NUMBER, b NUMBER); CREATE PROCEDURE p AS BEGIN
    //   INSERT INTO t SELECT x.a FROM s x WHERE x.a = 1;
    //   INSERT INTO t SELECT y.b FROM s y JOIN u z ON y.id = z.id WHERE y.b = 2;
    // END;
    // 断言：该 proc→t 边 column_analysis.hard_filters 同时含 a=1 与 b=2；
    //       join_conditions 含 s.id = u.id；column_mappings 仍正确去重。
}
```

（实现时按 builder 现有测试范式落位；若 builder 无 `#[cfg(test)]`，用 `tests/regress_column_analysis_merge.rs` 端到端 + `export --format json` 断言。）

**Step 2: 运行确认失败**

Run: `cargo test --features full merge_table_access_unions_hard_filters_and_joins`
Expected: FAIL — 只有第一条语句的 hard_filters 幸存。

**Step 3: 最小实现（Green）**

- 为上述类型补 `Hash` derive（`FilterValue::Float(String)` 可哈希，无 f64 阻碍）；
- `merge_table_access_edges`：仿照 `column_mappings` 的 HashSet 去重模式，对 `join_conditions`、`hard_filters`、`enum_mappings`、`select_into`、`insert_columns`、`update_columns`、`column_refs` 做集合并集；`alias_map` 做 BTreeMap extend（同 key 首见优先）；删除/改写「remaining diagnostic fields keep the first」注释（L3377-3379）；
- 读边（`AccessMode::Write` 不含）继续清空 `column_mappings` 的既有行为不变；
- `STORE_VERSION` 9→10，更新邻近注释（v10 = merge 诊断字段并集 + HardFilter.transform 预留，关联 #165/#169）。

**Step 4: 验证**

Run: `cargo test --features full` + `cargo clippy --features full -- -D warnings` + `cargo fmt --all -- --check`
Expected: 新测试绿；store.rs 版本拒绝测试（`load_bincode_rejects_previous_layout_version` L2403、`load_bincode_rejects_pre_issue_159_version` L2425）依旧绿（它们写旧版本文件断言被拒，不受新版本号影响）；既有 full 套件除已知环境跳过项（`test_path_mapping_applied`、`test_serve_*`）外全绿。

---

## Task 2 (T1): #169 — 函数包裹列的字面量过滤纳入 HardFilter（白名单 + transform）

**Files:**
- Modify: `src/parser/extractor.rs`（`HardFilter` L1930 加字段；新 struct `FilterTransform`；新 helper `column_transform_of`；`process_expr_for_joins_and_filters` 六个比较分支各加 arm；新 `add_hard_filter_with_transform`）
- Test: `src/parser/extractor.rs` tests 模块（filter 测试群 L4814-5038 旁）

**Step 1: 写失败测试（Red）**

```rust
/// #169: a whitelisted pure column transform compared against a literal yields a
/// HardFilter on the underlying column, with a transform descriptor.
#[test]
fn substr_wrapped_column_literal_becomes_hard_filter_with_transform() {
    // WHERE substr(qs.stock_kind, 1, 2) = '05'  (qs 为表别名)
    // 断言：hard_filters 含 { table: Some(..), column: "stock_kind", Eq, String("05"),
    //        transform: Some(FilterTransform { fn_: "substr", args: [Integer(1), Integer(2)] }) }
}

/// #169: the STEP3 mixed-cursor case — transformed and plain filters coexist.
#[test]
fn step3_cursor_mixed_filters_all_captured() {
    // WHERE substr(qs.stock_kind,1,2)='05' AND qs.stock_kind <> '0509'
    //   AND qs.scdm = '001' AND qs.cjsl > 0
    // 断言：4 条 HardFilter，第一条带 transform，后三条 transform == None
}

/// #169: non-literal extra args exclude the filter (PL variable in args).
#[test]
fn substr_with_variable_length_arg_is_excluded() {
    // WHERE substr(col, 1, v_len) = '05'  → 不产出
}

/// #169: non-whitelisted function or func-vs-func comparisons stay excluded.
#[test]
fn non_whitelisted_or_double_sided_function_is_excluded() {
    // WHERE fnc_x(col) = '1' → 不产出；WHERE nvl(a,1) = nvl(b,2) → 不产出
}

/// #169: SpecialFunction (keyword syntax) is covered too.
#[test]
fn substr_keyword_syntax_produces_transform() {
    // WHERE substring(col FROM 1 FOR 2) = '05' → 产出（D5 双变体）
}
```

**Step 2: 运行确认失败**

Run: `cargo test --features full substr_wrapped_column_literal_becomes_hard_filter_with_transform step3_cursor_mixed_filters_all_captured`
Expected: FAIL — 现在什么都不产出。

**Step 3: 最小实现（Green）**

```rust
/// #169: descriptor of a whitelisted pure column transform in a filter.
/// Serialized as {"fn": "substr", "args": [1, 2]} per issue schema.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct FilterTransform {
    #[serde(rename = "fn")]
    pub fn_name: String,          // 小写规范化
    pub args: Vec<FilterValue>,   // 除目标列外的全部实参（均为字面量）
}
```

- `HardFilter` 增加 `#[serde(default, skip_serializing_if = "Option::is_none")] pub transform: Option<FilterTransform>`（满足验收「JSON 无 transform 或 null」；旧 store 反序列化得 None）；
- helper `column_transform_of(expr) -> Option<(&[Ident] /*列*/, FilterTransform)>`：
  - 匹配 `Expr::FunctionCall` 与 `Expr::SpecialFunction`（D5 双变体），name 小写 ∈ {substr, substring, nvl, trim, upper, lower}；
  - args 中恰好一个 `Expr::ColumnRef`，其余全部 `literal_to_filter_value` 成功（PL 变量→None→自动排除，天然满足「substr(col,1,v_len) 不产出」）；
  - `substring` 与 `substr` 归一化为 `"substr"`；
- 六个比较分支（`=` `<>` `!=` `>` `>=` `<` `<=`）在 col-vs-literal 判断后各加对称 arm：一侧 `column_transform_of` 命中且另一侧 `literal_to_filter_value` 命中 → `add_hard_filter_with_transform`；
- `Like/Between/InList/IsNull` 侧不处理函数包裹（范围外）；
- 既有 `add_hard_filter` 保持签名，内部 `transform: None`（10 个调用点零改动）。

**Step 4: 验证**

Run: `cargo test --features full` （重点回归 `test_join_with_alias_and_hard_filter` L4814、`test_pl_variable_not_hard_filter` L4895）+ clippy + fmt。
Expected: 新旧全绿；既有 `col='x'` filter 的 `transform` 序列化后不出现（skip_serializing_if）。

---

## Task 3 (T3): #165 P0 — `codeweb columns` CLI（按过程聚合 ColumnAnalysis）

**Files:**
- Add: `src/graph/columns.rs`（聚合函数 `pub fn column_analysis_of_routine(...) -> AggregatedColumnAnalysis`；模块注册 `src/graph/mod.rs`）
- Modify: `src/main.rs`（`Commands::Columns` variant + dispatch + `cmd_columns`；旧 store 软提示）
- Test: 新增 `tests/regress_columns.rs`（harness 仿 `regress_column_lineage.rs` 的 `project_with_sql`）+ graph 层单测

**Step 1: 写失败测试（Red）**

```rust
/// #165: per-procedure column analysis export aggregates all TableAccess edges.
#[test]
fn columns_json_lists_hard_filters_and_joins_without_duplicates() {
    // fixture（仿 STEP3 驱动游标 + 维表）：
    //   CREATE TABLE mid_yjqs_detail(...); CREATE TABLE par_fund_partner(...);
    //   CREATE PROCEDURE prc_trd_hz_byfund AS BEGIN
    //     -- 两条语句写同一张输出表，各带不同 hard_filter / join_condition
    //     INSERT INTO mid_yjqs_detail SELECT f.partner_no FROM par_fund_partner f
    //       WHERE f.fund_code = c.fund_code AND c.scdm = '001' ...;
    //   END;
    // 断言 `codeweb columns --procedure prc_trd_hz_byfund --format json`：
    //   - schema_version == 1；procedure/package 字段正确
    //   - hard_filters 含 scdm='001'；join_conditions 含 par_fund_partner.fund_code ↔ ...
    //   - 同一 filter/join 不重复出现（多边聚合去重）
}

/// #165: --table narrows to one table's constraints.
#[test]
fn columns_json_table_filter_narrows_output() { /* --table mid_yjqs_detail 只出该表相关 */ }

/// #165: unknown procedure → clear error, exit != 0.
#[test]
fn columns_unknown_procedure_errors_cleanly() { /* 不静默空数组 */ }
```

graph 层单测：聚合函数对合成边去重（两条边各含相同 `scdm='001'` → 只出现一次）。

**Step 2: 运行确认失败**

Run: `cargo test --features full --test regress_columns`
Expected: FAIL — 子命令不存在（编译失败即为合法 Red）。

**Step 3: 最小实现（Green）**

- `AggregatedColumnAnalysis`（serde struct，首字段 `schema_version: u32 = 1`，房屋风格仿 `ImpactResult` main.rs:148-179）：
  `{ schema_version, procedure, package: Option<String>, tables: Vec<String>, join_conditions, hard_filters, select_into, enum_mappings, column_mappings, insert_columns, update_columns, read_tables }`——字段名与 `ColumnAnalysis` 1:1（issue 要求「不要再包一层展示用树」）；
- `column_analysis_of_routine`：仿 `mappings_of_routine`（lineage.rs:1480）——扫该 routine 节点入边+出边的 `Edge::TableAccess.column_analysis`，逐字段 HashSet 去重；`read_tables` 合并；`--table` 过滤在聚合层做（保留与目标表相关的边；join/filter 若涉及其它表仍保留——语句级隔离需要 read_tables）；
- CLI：`Commands::Columns { #[arg(long)] procedure: Option<String>, #[arg(long)] package: Option<String>, #[arg(long)] table: Option<String>, #[arg(long, default_value="json", value_parser=["json"])] format: String, #[arg(short, long, default_value=".")] project: PathBuf }`；procedure/package 二选一必填（clap `group.required = true` + `conflicts_with`）；
- 旧 store 软提示：`store.version < 10` → `eprintln!("note: store version {} predates full column-analysis diagnostics (v10) — run `codeweb analyze` to rebuild.", ...)`（仿 main.rs:1560 范本）；
- 过程定位复用 `store.resolve_single_node(name, MatchMode::Substring, ...)` + 校验 Procedure/Function 节点（仿 cmd_lineage L1670-1696）。

**Step 4: 验证**

Run: `cargo test --features full --test regress_columns` + 全套门禁。README/user-guide 文档在 T7 统一补。

---

## Task 4 (T4): #168 — WHERE/JOIN ON 记录字段解析为跨表等值键

**Files:**
- Modify: `src/parser/extractor.rs`（新 helper `resolve_record_field(&self, names) -> Option<(String, String)>` 复用 `column_source` 的三段规则；`extract_join_condition` 增加记录字段对侧路径；`JoinConditionSource` **新增 `RecordField` 变体【D2 已锁定】**，serde 序列化为 `"RecordField"`）
- Test: extractor.rs tests + `tests/regress_column_lineage.rs`（新 e2e）

**Step 1: 写失败测试（Red）**

```rust
/// #168: record field on one side of an equi-comparison resolves to the cursor's
/// source column, producing a cross-table JoinCondition.
#[test]
fn record_field_in_where_resolves_to_cross_table_join() {
    // 上下文：CURSOR c_get_data IS SELECT security_id, fund_code FROM mid_yjqs_detail ...;
    //         r_get_purchase c_get_data%ROWTYPE;
    // SQL: SELECT t.purchase_days INTO v_purchase_days FROM par_sys_purchase t
    //      WHERE t.security_id = r_get_purchase.security_id
    // 断言：join_conditions 含 par_sys_purchase.security_id ↔ mid_yjqs_detail.security_id，
    //       source == RecordField【D2 已锁定】
}

/// #168: plain equi-joins regress unchanged.
#[test]
fn plain_on_equi_join_unchanged() { /* ON a.id = b.id → ImplicitWhere/ExplicitOn 如旧 */ }

/// #168: record-vs-procedure-param and unregistered records produce nothing.
#[test]
fn record_vs_param_or_unregistered_produces_no_join() {
    // WHERE r.col = p_i_date（参数侧）→ 不产出；未注册记录变量 → 不产出（不猜表名）
}

/// #168: table-anchored %ROWTYPE and SELECT * cursor catch-all follow #142 rules.
#[test]
fn table_anchored_rowtype_and_star_cursor_resolve() { /* 两种锚定形态各一断言 */ }
```

e2e：`tests/regress_column_lineage.rs` 新增「STEP3 维表 JOIN」用例（fixture 自建 `par_sys_purchase` 风格）。

**Step 2: 运行确认失败** → **Step 3: 最小实现（Green）**

- `resolve_record_field`：抽取 `column_source`（L3149）中「记录字段 → 游标源列」分支为独立函数（精确 output_name 匹配 → `(source_table, source_col)`；catch-all → `(cursor锚表, 字段名)`；表锚定 → `(锚表, 字段名)`），`column_source` 改为调用它（消除重复，Refactor 步骤内聚）；
- `process_expr_for_joins_and_filters` 的 `=` 分支：两侧 `as_column_ref` 双成功 → 现路径；**一侧列、一侧记录字段** → `extract_record_field_join`，产出 `JoinCondition { left/right 表列, source: RecordField }`【D2 已锁定】；去重逻辑复用现有反向查重（L2550-2555）；
- 记录字段一侧同时 `add_column_ref(..., JoinCondition)`（与现路径对齐）；
- `p_i_date` 参数经 `record_cursors` 查不到 → None → 不产出（负例免费）。

**Step 4: 验证**：全套门禁；`test_join_with_alias_and_hard_filter` 等既有 join 单测全绿。

---

## Task 5 (T5): #165 P1 — MCP `codeweb_column_analysis`/`codeweb_lineage` + HTTP `/api/v1/columns`/`/lineage`

**Files:**
- Modify: `src/mcp/tools.rs`（两个新 `#[tool]` 方法 + 参数结构；`tool_handler` instructions 补两句）；`tests/mcp_test.rs`（tools list 断言 6→8）
- Modify: `src/server/handlers.rs`（router 两条 route + 两个 handler，仿 `trace` L435-479）
- Modify: `docs/serve-api-guide.md`、README 两表（亦可留 T7，此处至少改代码侧）
- 共享后端：T3 的 `graph::columns::column_analysis_of_routine` 与 lineage 既有函数，三个面共用同一 serde 结构，**不另发明 schema**

**Step 1: 写失败测试（Red）**

- `tests/mcp_test.rs`：`test_mcp_tools_list` 改为断言 8 个工具名（含 `codeweb_column_analysis`、`codeweb_lineage`）；新增 `test_mcp_call_column_analysis`（仿 `test_mcp_call_stats`，断言返回 JSON 与 CLI `columns --format json` 字段一致）；
- `tests/serve_api.rs`：`test_serve_columns_endpoint`、`test_serve_lineage_endpoint`（启动 serve、请求 `/api/v1/columns?procedure=...`、断言 200 + JSON 字段；404 场景）。

**Step 2: 运行确认失败** → **Step 3: 最小实现（Green）**

- MCP `codeweb_column_analysis`：`ColumnAnalysisParams { procedure: Option<String>, package: Option<String>, table: Option<String> }`；空图守卫复用 `graph_empty()`；返回 T3 同一 JSON 字符串；
- MCP `codeweb_lineage`：`LineageParams { target: String, direction: Option<String>, depth: Option<usize> }`；复用 lineage_table/lineage_column + `format_lineage_json`/`format_column_lineage_json`，direction 缺省 both（与 CLI 一致）；
- HTTP `GET /api/v1/columns`：`ColumnsQuery { procedure: Option<String>, package: Option<String>, table: Option<String> }`；`GET /api/v1/lineage`：`LineageQuery { target, direction: Option<String>, depth: Option<usize> }`；错误约定与现有一致（缺参/未命中 → 400/404，无 envelope）；
- instructions 字符串（tools.rs:539）追加两工具用途说明。

**Step 4: 验证**：`cargo test --features full`（含 serve/mcp 集成测试；CI 跳过项除外）+ clippy + fmt。

---

## Task 6 (T6): #167 — PL IF/CASE 条件解析为表列谓词

**Files:**
- Add: `src/parser/predicates.rs`（`PredicateExtractor`：branch-aware Visitor pass + 谓词 AST）
- Modify: `src/graph/builder.rs`（过程构建期调用新 pass，产出挂入 store）；`src/graph/store.rs`（**加 `procedure_predicates` 侧表 + bump v11【D6 已锁定：存储方案】**）
- Modify: `src/main.rs`（`Commands::Predicates` + `cmd_predicates`，**独立命令【D3 已锁定】**）
- Test: `src/parser/predicates.rs` tests + `tests/regress_predicates.rs`

**设计要点（D3/D6 均已锁定，直接按此实施）**：

- 新 AST（全部 serde，schema 复用 `FilterOperator`/`FilterValue`）：
  `PlPredicate { id: String /* B001… */, line: usize, origin: String, kind: PredicateKind(If|CaseWhen), confidence: Confidence(High|Medium|Low), table_predicate: Option<TablePredicate>, needs_review: Option<String>, param_table_hint: Option<ParamTableHint> }`；
  `TablePredicate { table, clauses: Vec<PredicateClause { column, op, value }> }`；
  `ParamTableHint { table, filters: Vec<PredicateClause>, set: Vec<(String, FilterValue)> }`；
- pass 形态仿 `CallExtractor` 的 PL 走树（L441-799 证可行）：`impl Visitor for PredicateExtractor`，拦截 `PlStatement::If`/`Case`（读 `condition`/`whens[].condition`），条件表达式经「条件→clauses 转换器」解析——该转换器**复用 T1 的 `column_transform_of` + T4 的 `resolve_record_field` + `ProcedureVarContext`**；
- 置信度规则（issue 表格逐条落地，单测各锁一条）：
  | 模式 | confidence |
  |---|---|
  | `r.field` 且 `record_cursors` 命中，比较字面量 | high |
  | 裸列且 `scope_sole_table` 唯一 | high |
  | `SELECT col INTO v` 后 `IF v = literal`，col 来自主表 | medium（主表谓词） |
  | 同上但 col 来自维表 | low + `param_table_hint` |
  | 函数调用/动态 SQL/GOTO | low / skip，保留 `origin` |
- 过程内 `SELECT INTO` 变量源追踪：pass 内自建 `HashMap<var, (table, column)>`（走 `PlStatement::SqlStatement` 的 into_targets + targets，游标源解析复用 `ProcedureVarContext`）；
- IF 分支下语句归属：`then_stmts`/`else_stmts` 递归时携带当前条件上下文（分支内语句不重复产出谓词，谓词只来自条件本身）。

**Step 1: 写失败测试（Red）**

```rust
/// #167: STEP3 star_market IF resolves to high-confidence table predicate.
#[test]
fn star_market_if_resolves_high_confidence() {
    // IF r_get_data.stock_kind = '0100' AND r_get_data.zqdm BETWEEN '609100' AND '609999'
    // → predicate { confidence: High, table: mid_yjqs_detail,
    //   clauses: [stock_kind eq '0100', zqdm between [609100,609999]] }
}

/// #167: SELECT-INTO-derived var yields low confidence + param_table_hint.
#[test]
fn select_into_var_condition_yields_param_table_hint() {
    // SELECT kind_id INTO v_kind FROM swh_all_kind WHERE operation_kind='COMMISSION_SWITCH';
    // IF v_kind = '1'  →  low + hint{ swh_all_kind, filters:[operation_kind eq ...], set:{kind_id:'1'} }
    // 断言：不误写成主表谓词
}

/// #167: cursor WHERE hard filters do NOT leak into the IF predicate list.
#[test]
fn cursor_hard_filters_not_in_predicates() { /* 游标 WHERE 的 HardFilter 不出现在 predicates */ }

/// #167: function-call conditions keep origin, low/skip confidence.
#[test]
fn function_condition_degrades_confidence() { /* IF fnc_x(a) = 1 → low + origin 保留 */ }
```

**Step 2: 运行确认失败** → **Step 3: 最小实现（Green）** → **Step 4: 验证**

- CLI：`codeweb predicates --procedure X --format json`，输出 `{ schema_version: 1, procedure, predicates: [...] }`；
- store 增加 `procedure_predicates: HashMap<String, Vec<PlPredicate>>`（`#[serde(default)]`）【D6 已锁定】，`STORE_VERSION` → 11，`cmd_predicates` 直接读 store；旧 store < 11 软提示重跑 analyze；
- 全套门禁。

---

## Task 7 (T7): #166 — 文档补齐

**Files（纯文档，无代码）:**
- `README.md`（中英两份表格）：CLI 表加 `lineage`、`columns`、`predicates`；HTTP 表加 `/columns`、`/lineage`；MCP 工具表加两个新工具
- `docs/user-guide.md`：§6 新增 `lineage` 子节（table vs table.column、--direction/--view/--flow-only、store v7+ 提示、与 trace 的区别）+ `columns`/`predicates` 子节
- `docs/DeveloperGuide.md`：`ColumnAnalysis` 字段表（join/hard_filter/select_into/mapping kind/transform）+ 消费场景（mock 造数）+ MCP/HTTP 表更新
- `docs/getting-started.md` + `_zh`：10 行 INSERT..SELECT 的 `codeweb lineage t_out.amt --direction upstream` 示例
- `docs/serve-api-guide.md`：`/columns`、`/lineage` 端点文档（若 T5 未覆盖）

**Step 1: 可执行 QA 场景（文档的「失败测试」——先跑通核对清单再动笔，列出当前缺失项）**

```bash
# QA-1 README 命令表与 --help 一致性（中英两份表都要核对）
codeweb --help
# 预期缺失（写文档前应确认 grep 全部落空， documenting 后应 ≥2：英文表 + 中文表各一行）：
grep -c '| `codeweb lineage'   README.md   # 现在 0 → 目标 ≥ 2
grep -c '| `codeweb columns'   README.md   # 现在 0 → 目标 ≥ 2
grep -c '| `codeweb predicates' README.md  # 现在 0 → 目标 ≥ 2

# QA-2 user-guide 出现可照跑的小节（写前 0 命中，写后各 ≥1 个 §6.x 标题）
grep -n '^#\{2,3\} .*lineage'   docs/user-guide.md
grep -n '^#\{2,3\} .*columns'   docs/user-guide.md
grep -n '^#\{2,3\} .*predicates' docs/user-guide.md

# QA-3 serve-api-guide 端点存在且字段与实际输出一致
grep -n 'api/v1/columns\|api/v1/lineage' docs/serve-api-guide.md   # 目标 ≥ 1 处/端点
# 字段一致性核对：文档响应示例顶层键 == 实际输出顶层键（对 T3 fixture 项目执行）
codeweb columns --procedure prc_trd_hz_byfund --format json | jq -S 'keys'
codeweb serve & curl -s 'http://127.0.0.1:3000/api/v1/columns?procedure=prc_trd_hz_byfund' | jq -S 'keys'
# 两次 jq keys 输出必须相同，且与 serve-api-guide 文档示例逐键一致

# QA-4 DeveloperGuide ColumnAnalysis 字段说明
grep -n 'ColumnAnalysis' docs/DeveloperGuide.md   # 目标：字段表出现（含 transform 行）
grep -n 'codeweb_column_analysis\|codeweb_lineage' docs/DeveloperGuide.md  # MCP 表 8 工具

# QA-5 getting-started 示例可照跑（10 行 INSERT..SELECT fixture）
# 按文档步骤在 /tmp 临时项目逐字执行，预期输出含：
codeweb lineage t_out.amt --direction upstream
#   → 树中出现源列 t_src.amt（或 fixture 对应源表列），非 "No column lineage"
```

**Step 2: 依清单撰写/修订文档**（上面每条 grep 由 0 → 目标值；QA-3/QA-5 的实际命令输出与文档示例逐字一致）

**验收（照 issue #166 + Momus 要求的可执行核对）**：QA-1~QA-5 全部通过；`codeweb --help` 的每个子命令在 README 两份 CLI 表各有且仅有一行；serve-api-guide 响应示例键集与 `jq keys` 实测一致。

---

## 执行顺序与门禁

```
T2（方案A合并+bump v10） → T1（#169 transform） → T3（#165 P0 CLI）
→ T4（#168 跨表键） → T5（#165 P1 MCP/HTTP） → T6（#167 谓词，bump v11【D6 已锁定】）
→ T7（#166 文档） → 全量门禁
```

每个 Task 独立 Red→Green→Refactor 循环，完成即跑：
```bash
cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_
cargo clippy --features full -- -D warnings
cargo fmt --all -- --check
```
最终门禁另跑 `cargo build --features full` + `cargo test --features full`。

**Never 红线（AGENTS.md）**：不删/跳过/改写人类已有测试断言；`test_join_with_alias_and_hard_filter`、`test_pl_variable_not_hard_filter`、`test_mcp_tools_list`（改 6→8 属新增工具的必要同步，在汇报中显式说明）、store 版本拒绝测试为只读基线；每个行为先有失败测试；不引入新依赖/feature flag。
