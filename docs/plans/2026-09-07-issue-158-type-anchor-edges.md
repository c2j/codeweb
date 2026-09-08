# %TYPE/%ROWTYPE 锚定边（AnchorsOn，#158）实施计划

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** 把 PL/SQL `%TYPE` / 表级 `%ROWTYPE` 编译期 schema 锚定建成 `Reference` 类 `AnchorsOn` 边，`detail`/`trace`/`impact` 可见（标签 `[T]`），`lineage`/`conflicts`/`--summarize-tables`/community 不受污染。

**Architecture:** 新增 `Edge::AnchorsOn` 变体（追加在 Edge 枚举末尾）+ 新 `AnchorExtractor` visitor（结构化 AST 直读 + 扁平字符串兜底解析）+ builder 建边（复用 `table_index` 的 inferred `table*` 创建路径）。8 处穷尽 match 补臂，`edge_label_for` 改为聚合同对节点平行边标签。`STORE_VERSION` 8→9。

**Tech Stack:** Rust (stable)，ogsql-parser v0.10.0（git tag），petgraph，serde/bincode。无新依赖、无新 feature flag。

**参考 issue:** #158。设计文档：`docs/plans/2026-04-26-db-object-type-support.md` §风险。

---

## 0. 背景事实（实现者必读）

### AST 层（ogsql-parser v0.10.0）

| 来源 | 表示 | 结构化？ |
|---|---|---|
| DECLARE / 包级变量 | `PlVarDecl.data_type: PlDataType` | ✅ |
| 游标参数/RETURN | `PlCursorArg.data_type` / `PlCursorDecl.return_type: Option<PlDataType>` | ✅ |
| 嵌套类型 | `PlTypeDecl::TableOf{elem_type, index_by}` / `VarrayOf{elem_type}` / `Record{fields: Vec<PlTypeField{data_type}>}` | ✅ |
| 函数/过程/包例程参数 | `RoutineParam.data_type: String` | ❌ 扁平串 |
| 函数 RETURN | `CreateFunctionStatement.return_type: Option<String>`；`PackageFunction.return_type: Option<String>` | ❌ 扁平串 |

`PlDataType` 变体：`TypeName(String)` / `PercentType { table: String, column: String }` / `PercentRowType(String)` / `Record` / `Cursor` / `RefCursor`。

扁平串来自 `parse_type_name()` token 拼接，形如 `par_sys_purchase. purchase_days% type`（含杂散空格、大小写不定）。

### codeweb 现状

- `TypeSequenceRefExtractor::visit_pl_declaration`（`src/parser/extractor.rs:890-906`）只处理 `TypeName` + known_types，跳过 `PercentType`。
- `ColumnAccessExtractor`（`extractor.rs:2648-2667`）仅用 `PercentRowType(cursor)` 填 `record_cursors`（#147），不建表边。
- 建边模板：`GraphBuilder` 的 `create_object_ref_edges`（`src/graph/builder.rs:1732`）—— CreateProcedure(:1746)/CreateFunction(:1803) 分支已遍历 `parameters` 与 `return_type`；CreatePackage/Body(:1874/:1886) → `collect_package_object_ref_edges`。**该函数当前不接收 `table_index`，需加参（`&mut`）。**
- inferred 表节点模式：`builder.rs:2881-2908`（`table_index.entry(key).or_insert_with(|| Node::Table { explicit: false, ... })`）。
- 表名归一化：`normalize_table_key(schema: Option<&str>, name: &str)`（`builder.rs:4331`），全小写。
- `Edge` 枚举 `src/graph/mod.rs:737-804`；`Edge::category()` :809-831（Reference 组 :822-826）。
- 8 处穷尽 match（加变体必改，漏一处编译失败）：
  1. `mod.rs:809-831` `Edge::category()`
  2. `src/graph/store.rs:1742-1772` `edge_type_tag()`
  3. `src/graph/cluster.rs:123-143` `edge_weight()`
  4. `src/export/json.rs:258-321` `EdgeKindJson` 枚举 + `:687-884` Edge→EdgeJson 映射
  5. `src/export/ndjson.rs:179-201` `edge_json_type()`
  6. `src/export/dot.rs:298-376` `edge_dot_attrs()`
  7. `src/export/mermaid.rs:148-176` 箭头样式 match
  8. `src/main.rs:4381-4404` `edge_location_line()`
- 行为性 match（有通配，编译不强制但必须补）：
  - `src/graph/traverse.rs:52-100` `edge_label_for()`（`_ => None`；且 `.edges_connecting(from,to).next()` 只取第一条边 —— 同对双标签必须改成聚合）
- `STORE_VERSION: u32 = 8`（`store.rs:22`）。
- 自动满足、无需改动（已核实过滤机制）：
  - `impact`：`EdgeFilter` 按 category，clap 默认 `--edge all` 不过滤 → AnchorsOn 自动纳入
  - `lineage`：只匹配 `TableAccess`/`DependsOn` 变体 → 自动排除
  - `conflicts`：只取 `TableAccess`+`DmlAccess`→table/view/mview → 自动排除
  - `--summarize-tables`：只取子例程 `TableAccess DmlAccess`→Table（`main.rs:2167-2264`）→ 自动排除

### 设计决策（D1–D4，取默认值；Momus 重点审查项）

| # | 决策 | 内容 |
|---|---|---|
| D1 | 同对平行边标签聚合 | `edge_label_for` 收集该节点对**所有**边的标签，去重、保持首现顺序、`,` 连接、单括号：`[R]`+`[T]` → `[R,T]`；单边行为不变 |
| D2 | community 权重 | `edge_weight()` 对 AnchorsOn 返回 `None`（完全排除，符合 issue 字面「排除」） |
| D3 | 游标 RETURN 类型 | 不抽取（`CURSOR c RETURN t%ROWTYPE` 结构化可得，但 issue 优先级清单未列；记 follow-up） |
| D4 | CGEF import 白名单 | 不扩展（导出侧补臂是编译强制；回导 anchors_on 会按 unknown 处理，记 follow-up） |

### 提取范围与消歧义规则（issue §抽取范围）

1. DECLARE / 包级变量（`PercentType` → `site=Variable`，`column=Some`）
2. 函数/过程参数（`site=Param`）、RETURN（`site=ReturnType`）—— 扁平串解析
3. 嵌套 `TYPE t IS TABLE OF x.col%TYPE`、record 字段（`site=NestedType`）
4. `%ROWTYPE` 消歧义：名字命中当前 routine 或包级的 **cursor 名** → **不建边**；否则视为表锚定（`column=None`），目标按 `table_index` 解析（无 DDL → inferred `table*`）
   - 附加守卫：`%TYPE` 的首段标识符命中 cursor 名或 local 变量名 → 跳过（PL/SQL 允许 `v2 v1%TYPE` 锚定到变量）

边语义：同对象既有 DML 又有锚定时**保留两条独立边**（dedup 按 `edge_type_tag` 分组，类型不同不会合并，天然安全）。

---

## Task 1: 扁平类型串解析纯函数

**Files:**
- Modify: `src/parser/extractor.rs`（`TypeSequenceRefExtractor` 定义附近，:844 前后）
- Test: `src/parser/extractor.rs` `#[cfg(test)] mod tests`

**Step 1: 写失败测试**（测试函数命名按 AGENTS.md 行为风格）

```rust
#[test]
fn should_parse_flat_return_string_percent_type() {
    // 真实 parse_type_name 输出：杂散空格 + 大小写混乱
    let a = parse_anchor_from_type_string("par_sys_purchase. purchase_days% type")
        .expect("should parse");
    assert_eq!(a.object, "par_sys_purchase");
    assert_eq!(a.column.as_deref(), Some("purchase_days"));
    assert!(matches!(a.kind, AnchorKind::PercentType));
    // 纯函数不区分调用点，统一默认 Param 占位；
    // RETURN 场景由 builder 调用方覆盖为 ReturnType（后续 Task 6）
    assert!(matches!(a.site, AnchorSite::Param));
}

#[test]
fn should_parse_flat_param_string_percent_rowtype() {
    let a = parse_anchor_from_type_string("DAT_TRD_REPURCHASE%ROWTYPE")
        .expect("should parse");
    assert_eq!(a.object, "DAT_TRD_REPURCHASE");
    assert_eq!(a.column, None);
    assert!(matches!(a.kind, AnchorKind::PercentRowType));
}

#[test]
fn should_return_none_for_plain_type_names() {
    assert!(parse_anchor_from_type_string("INTEGER").is_none());
    assert!(parse_anchor_from_type_string("VARCHAR(100)").is_none());
    assert!(parse_anchor_from_type_string("my_pkg.my_record").is_none());
    assert!(parse_anchor_from_type_string("").is_none());
}
```

**Step 2: 跑测试确认失败**

Run: `cargo test should_parse_flat_return_string_percent_type`
Expected: 编译失败（`parse_anchor_from_type_string` / `AnchorKind` / `AnchorSite` 不存在）—— 合法 Red。

Run: `cargo test should_parse_flat_param_string_percent_rowtype`
Expected: 编译失败（同上）。

Run: `cargo test should_return_none_for_plain_type_names`
Expected: 编译失败（同上）。

**Step 3: 最小实现**（放在 extractor.rs 顶层，`TypeSequenceRefExtractor` 之前）

```rust
/// Schema anchor kind for `AnchorsOn` edges (issue #158).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorKind {
    PercentType,
    PercentRowType,
}

/// Where in the routine the anchor appears.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorSite {
    ReturnType,
    Param,
    Variable,
    NestedType,
}

/// One `%TYPE` / `%ROWTYPE` anchor parsed from a declaration or signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnchorRef {
    pub object: String,
    pub column: Option<String>,
    pub kind: AnchorKind,
    pub site: AnchorSite,
}

/// Parse a flat routine-signature type string (e.g. `par_sys_purchase.
/// purchase_days% type`) into an anchor. Returns `None` for plain type
/// names. Tolerates stray whitespace and case variation produced by
/// ogsql-parser's token concatenation.
pub fn parse_anchor_from_type_string(s: &str) -> Option<AnchorRef> {
    let site = AnchorSite::Param; // 调用方按需覆盖 site
    let lower = s.to_lowercase();
    let (kind, head) = if let Some(pos) = lower.find("%type") {
        let rest = &lower[pos + 5..];
        // 拒绝 "%ROWTYPE" 被误判为 "%TYPE" 前缀：%ROWTYPE 的 "%type" 后跟 "row"
        if rest.starts_with("row") {
            let pos = lower.find("%rowtype")?;
            (AnchorKind::PercentRowType, &s[..pos])
        } else {
            (AnchorKind::PercentType, &s[..pos])
        }
    } else {
        let pos = lower.find("%rowtype")?;
        (AnchorKind::PercentRowType, &s[..pos])
    };
    let idents: Vec<&str> = head
        .split('.')
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .collect();
    match (kind, idents.len()) {
        (AnchorKind::PercentType, n) if n >= 2 => {
            let column = idents[n - 1].to_string();
            let object = idents[..n - 1].join(".");
            Some(AnchorRef { object, column: Some(column), kind, site })
        }
        (AnchorKind::PercentRowType, n) if n >= 1 => {
            let object = idents.join(".");
            Some(AnchorRef { object, column: None, kind, site })
        }
        _ => None,
    }
}
```

注意：`%TYPE` 分支里 `lower.find("%type")` 会先命中 `%ROWTYPE` 中的 `%`——必须检查后续是否为 `row` 再回退到 `%rowtype`（上面代码已处理）。实现时若字面子串匹配无法容忍 `% type`（% 与 type 间空格），改为先定位 `%` 再 trim 后匹配前缀——以测试通过为准。

**Step 4: 跑测试确认通过**

Run: `cargo test should_parse_flat_return_string_percent_type`
Expected: PASS

Run: `cargo test should_parse_flat_param_string_percent_rowtype`
Expected: PASS

Run: `cargo test should_return_none_for_plain_type_names`
Expected: PASS

补充边界测试：`"t%ROWTYPE"` → PercentRowType（不被误判为 PercentType）。

**Step 5: 提交**

```bash
git add src/parser/extractor.rs
git commit -m "feat(parser): 扁平签名类型串解析 %TYPE/%ROWTYPE 锚定 (#158)"
```

---

## Task 2: AnchorExtractor —— 变量声明锚定（site=Variable）

**Files:**
- Modify: `src/parser/extractor.rs`（新 visitor，放 `TypeSequenceRefExtractor` 之后）
- Test: 同文件 `#[cfg(test)] mod tests`

**Step 1: 写失败测试**

先加测试辅助函数（放 tests 模块内、紧邻已有的 `extract_type_seq_refs` 辅助函数处）：

```rust
fn extract_anchors(sql: &str) -> Vec<AnchorRef> {
    let tokens = Tokenizer::new(sql).tokenize().unwrap();
    let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
    let stmts = parser.parse_with_text();
    let mut out = Vec::new();
    for info in &stmts {
        let mut ex = AnchorExtractor::new();
        walk_statement(&mut ex, &info.statement);
        out.extend(ex.anchors);
    }
    out
}
```

测试：

```rust
#[test]
fn should_collect_variable_percent_type_anchor() {
    let sql = "CREATE FUNCTION f() RETURN INTEGER AS \
        $$ DECLARE v_days par_sys_purchase.purchase_days%TYPE; BEGIN NULL; END; $$;";
    let anchors = extract_anchors(sql);
    assert_eq!(anchors.len(), 1, "got: {:?}", anchors);
    assert_eq!(anchors[0].object, "par_sys_purchase");
    assert_eq!(anchors[0].column.as_deref(), Some("purchase_days"));
    assert!(matches!(anchors[0].site, AnchorSite::Variable));
}

#[test]
fn should_collect_variable_table_rowtype_anchor() {
    let sql = "CREATE FUNCTION f() RETURN INTEGER AS \
        $$ DECLARE r dat_trd_repurchase%ROWTYPE; BEGIN NULL; END; $$;";
    let anchors = extract_anchors(sql);
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].object, "dat_trd_repurchase");
    assert_eq!(anchors[0].column, None);
    assert!(matches!(anchors[0].site, AnchorSite::Variable));
}
```

**Step 2: 跑测试确认失败**

Run: `cargo test should_collect_variable`
Expected: 编译失败（`AnchorExtractor` 不存在）—— 合法 Red。

**Step 3: 最小实现**（只做 Variable + Cursor 登记，嵌套 `PlDeclaration::Type` 分支留 Task 3——本 Task 保持最小）

```rust
/// Extracts `%TYPE` / table-level `%ROWTYPE` schema anchors (issue #158).
/// Cursor-anchored `%ROWTYPE` is deliberately skipped (issue #147/#142:
/// record fields resolve via cursor SELECT sources, not table edges).
pub struct AnchorExtractor {
    pub anchors: Vec<AnchorRef>,
    cursor_names: HashSet<String>,
}

impl AnchorExtractor {
    pub fn new() -> Self {
        Self { anchors: Vec::new(), cursor_names: HashSet::new() }
    }

    fn push_anchor(&mut self, object: String, column: Option<String>,
                   kind: AnchorKind, site: AnchorSite) {
        let obj_lower = object.to_lowercase();
        // 守卫：锚定目标是 cursor → 不建表锚（Task 4 将扩展变量名守卫）
        if self.cursor_names.contains(&obj_lower) {
            return;
        }
        self.anchors.push(AnchorRef { object, column, kind, site });
    }
}

impl Visitor for AnchorExtractor {
    fn visit_pl_declaration(&mut self, decl: &ogsql_parser::ast::plpgsql::PlDeclaration) -> VisitorResult {
        use ogsql_parser::ast::plpgsql::{PlDataType, PlDeclaration};
        match decl {
            PlDeclaration::Cursor(c) => {
                self.cursor_names.insert(c.name.to_lowercase());
            }
            PlDeclaration::Variable(v) => {
                if let PlDataType::PercentType { table, column } = &v.data_type {
                    self.push_anchor(table.clone(), Some(column.clone()),
                                     AnchorKind::PercentType, AnchorSite::Variable);
                } else if let PlDataType::PercentRowType(name) = &v.data_type {
                    self.push_anchor(name.clone(), None,
                                     AnchorKind::PercentRowType, AnchorSite::Variable);
                }
            }
            _ => {}
        }
        VisitorResult::Continue
    }
}
```

（`HashSet` 确认在 extractor.rs 已 import。嵌套 `PlDeclaration::Type` 分支留待 Task 3——本 Task 只做变量，保持最小实现。）

**Step 4: 跑测试确认通过**

Run: `cargo test should_collect_variable`
Expected: PASS（两个测试）

**Step 5: 提交**

```bash
git add src/parser/extractor.rs
git commit -m "feat(parser): AnchorExtractor 抽取变量 %TYPE/%ROWTYPE 锚定 (#158)"
```

---

## Task 3: 嵌套类型锚定（site=NestedType）

**Files:** 同 Task 2。

**Step 1: 写失败测试**

```rust
#[test]
fn should_collect_nested_table_of_percent_type() {
    let sql = "CREATE FUNCTION f() RETURN INTEGER AS $$ \
        DECLARE TYPE t_list IS TABLE OF par_sys_purchase.purchase_days%TYPE; \
        BEGIN NULL; END; $$;";
    let anchors = extract_anchors(sql);
    assert_eq!(anchors.len(), 1, "got: {:?}", anchors);
    assert!(matches!(anchors[0].site, AnchorSite::NestedType));
    assert_eq!(anchors[0].column.as_deref(), Some("purchase_days"));
    assert_eq!(anchors[0].object, "par_sys_purchase");
}

#[test]
fn should_collect_record_field_percent_type() {
    let sql = "CREATE FUNCTION f() RETURN INTEGER AS $$ \
        DECLARE TYPE t_rec IS RECORD (d dat_trd_repurchase.purchase_date%TYPE); \
        BEGIN NULL; END; $$;";
    let anchors = extract_anchors(sql);
    assert_eq!(anchors.len(), 1);
    assert_eq!(anchors[0].object, "dat_trd_repurchase");
    assert_eq!(anchors[0].column.as_deref(), Some("purchase_date"));
    assert!(matches!(anchors[0].site, AnchorSite::NestedType));
    assert!(matches!(anchors[0].kind, AnchorKind::PercentType));
}
```

**Step 2: 跑测试确认失败**

Run: `cargo test should_collect_nested_table_of_percent_type`
Expected: 断言失败——`anchors` 为空（Task 2 的最小实现未处理 `PlDeclaration::Type`，嵌套锚定被静默跳过）。

Run: `cargo test should_collect_record_field_percent_type`
Expected: 断言失败（同上）。

**Step 3: 最小实现**（给 `AnchorExtractor` 补 `Type` 分支与辅助方法）

```rust
// impl Visitor for AnchorExtractor 的 match 中追加：
            PlDeclaration::Type(t) => match t {
                PlTypeDecl::TableOf { elem_type, index_by, .. } => {
                    self.visit_pl_data_type(elem_type, AnchorSite::NestedType);
                    if let Some(ib) = index_by { self.visit_pl_data_type(ib, AnchorSite::NestedType); }
                }
                PlTypeDecl::VarrayOf { elem_type, .. } => {
                    self.visit_pl_data_type(elem_type, AnchorSite::NestedType);
                }
                PlTypeDecl::Record { fields, .. } => {
                    for f in fields { self.visit_pl_data_type(&f.data_type, AnchorSite::NestedType); }
                }
                _ => {}
            },

// 另加固有 impl：
impl AnchorExtractor {
    fn visit_pl_data_type(&mut self, dt: &ogsql_parser::ast::plpgsql::PlDataType, site: AnchorSite) {
        use ogsql_parser::ast::plpgsql::PlDataType;
        match dt {
            PlDataType::PercentType { table, column } => {
                self.push_anchor(table.clone(), Some(column.clone()), AnchorKind::PercentType, site);
            }
            PlDataType::PercentRowType(name) => {
                self.push_anchor(name.clone(), None, AnchorKind::PercentRowType, site);
            }
            _ => {}
        }
    }
}
```

（`use` 行扩展为 `PlDataType, PlDeclaration, PlTypeDecl`。）

**Step 3a: 跑测试确认通过**

Run: `cargo test should_collect_nested_table_of_percent_type`
Expected: PASS

Run: `cargo test should_collect_record_field_percent_type`
Expected: PASS

**Step 4: 提交**

```bash
git add src/parser/extractor.rs
git commit -m "feat(parser): 嵌套 TYPE/record 字段锚定抽取 (#158)"
```

---

## Task 4: %ROWTYPE 消歧义 —— cursor 名不产表锚

**Files:** 同 Task 2。

**Step 1: 写失败测试**

```rust
#[test]
fn should_skip_cursor_rowtype_anchor() {
    let sql = "CREATE FUNCTION f() RETURN INTEGER AS $$ \
        DECLARE CURSOR c IS SELECT id FROM t_main; \
        rec c%ROWTYPE; \
        BEGIN NULL; END; $$;";
    let anchors = extract_anchors(sql);
    assert!(anchors.is_empty(), "cursor%ROWTYPE must not become a table anchor: {:?}", anchors);
}

#[test]
fn should_keep_table_rowtype_when_cursor_exists_elsewhere() {
    // 同 routine 内：cursor c 与 表锚 rec2 互不影响
    let sql = "CREATE FUNCTION f() RETURN INTEGER AS $$ \
        DECLARE CURSOR c IS SELECT id FROM t_main; \
        rec c%ROWTYPE; rec2 dat_trd_repurchase%ROWTYPE; \
        BEGIN NULL; END; $$;";
    let anchors = extract_anchors(sql);
    assert_eq!(anchors.len(), 1, "got: {:?}", anchors);
    assert_eq!(anchors[0].object.to_lowercase(), "dat_trd_repurchase");
}
```

**Step 2:**

Run: `cargo test should_skip_cursor_rowtype_anchor`
Expected: PASS（Task 2 的 `cursor_names` 守卫已覆盖；若失败按失败信息修实现，不改测试）。

Run: `cargo test should_keep_table_rowtype_when_cursor_exists_elsewhere`
Expected: PASS

**Step 3: 提交**

```bash
git add src/parser/extractor.rs
git commit -m "test(parser): cursor%ROWTYPE 消歧义回归锁定 (#158)"
```

---

## Task 5: Edge::AnchorsOn 变体 + 8 处穷尽 match + STORE_VERSION bump

这是唯一一个「一次引入多文件」的 Task——变体加入即触发穷尽 match 编译强制，8 处必须同批补齐才能编译。每处臂的内容本身就是可断言行为。

**Files:**
- Modify: `src/graph/mod.rs`（Edge 枚举末尾 :803 后追加变体；`Edge::category()` :809-831）
- Modify: `src/graph/store.rs:22`（`STORE_VERSION` 8→9）、`:1742-1772`（`edge_type_tag()`）
- Modify: `src/graph/cluster.rs:123-143`（`edge_weight()`）
- Modify: `src/export/json.rs`（`EdgeKindJson` :258-321 + 映射 :687-884）
- Modify: `src/export/ndjson.rs:179-201`、`src/export/dot.rs:298-376`、`src/export/mermaid.rs:148-176`
- Modify: `src/main.rs:4381-4404`（`edge_location_line()`）
- Modify: `src/graph/traverse.rs:52-100`（`edge_label_for()` 加 `[T]` 臂；聚合留 Task 8）
- Test: `src/graph/store.rs` tests（roundtrip）、`src/graph/mod.rs` tests（category）

**Step 1: 写失败测试**（store.rs tests 内）

```rust
#[test]
fn should_roundtrip_anchors_on_edge_through_bincode_store() {
    // 构造含 AnchorsOn 边的最小 graph → save_bincode → load_bincode → 断言变体与字段
    // 断言：Edge::AnchorsOn { kind: PercentType, column: Some("purchase_days"),
    //        site: Variable, .. } 存在且 category() == EdgeCategory::Reference
}

#[test]
fn should_reject_store_with_stale_version() {
    // 仿 src/project/mod.rs:615-649 既有测试模式：
    // 手写 version=8 header 的 payload → load_bincode 报错要求重建
}
```

**Step 2: 跑测试确认失败**

Run: `cargo test should_roundtrip_anchors_on_edge_through_bincode_store`
Expected: 编译失败（`Edge::AnchorsOn` / `AnchorKind` 在 graph 层不存在）—— 合法 Red。

Run: `cargo test should_reject_store_with_stale_version`
Expected: 编译失败（同上）。

**Step 3: 实现**（全部同批，否则编译不过）

`src/graph/mod.rs` —— `use` 引入 `crate::parser::{AnchorKind, AnchorSite}`（或 re-export）：

```rust
// Edge 枚举末尾（CustomEdge 之后）追加——保持既有变体 bincode 序号不变：
/// Compile-time schema anchor: `%TYPE` / table-level `%ROWTYPE` (issue #158).
/// Category = Reference. Visible in detail/trace/impact; excluded from
/// lineage, conflicts, --summarize-tables, and community weighting.
AnchorsOn {
    kind: AnchorKind,
    column: Option<String>,
    site: AnchorSite,
    location: SourceLocation,
},

// Edge::category() 的 Reference 臂追加：
| Edge::AnchorsOn { .. } => EdgeCategory::Reference,
```

`store.rs`：

```rust
pub const STORE_VERSION: u32 = 9;  // was 8 — new Edge variant (issue #158)

// edge_type_tag() 追加：
Edge::AnchorsOn { .. } => "anchors_on",
```

`cluster.rs` `edge_weight()` 追加（community 完全排除，决策 D2）：

```rust
Edge::AnchorsOn { .. } => None,
```

`traverse.rs` `edge_label_for()` 追加（聚合在 Task 8）：

```rust
Edge::AnchorsOn { .. } => Some("[T]".into()),
```

`export/json.rs`：`EdgeKindJson` 追加变体（对齐现有风格，如 TableAccess 的 `#[serde(skip_serializing_if=...)]` 用法）：

```rust
#[serde(rename = "anchors_on")]
AnchorsOn {
    file: String,
    line: usize,
    kind: crate::parser::AnchorKind,
    column: Option<String>,
    site: crate::parser::AnchorSite,
},
```

并在 Edge→EdgeJson 映射 match 追加对应臂。`ndjson.rs` `edge_json_type()` 追加 `"anchors_on"`；`dot.rs` `edge_dot_attrs()` 追加（样式对齐 `ReferencesType`，label `anchors_on`）；`mermaid.rs` 追加（虚线，同 Reference 组现状）；`main.rs` `edge_location_line()` 追加 `Some(location.line)`。

**Step 4: 跑测试**

Run: `cargo test should_roundtrip_anchors_on_edge_through_bincode_store`
Expected: PASS

Run: `cargo test should_reject_store_with_stale_version`
Expected: PASS

Run: `cargo build --features full`（跨 feature 编译强制：jsp 的 ContainsSql 臂与本次改动共存）—— 0 错误。

**Step 5: 提交**

```bash
git add src/graph src/export src/main.rs
git commit -m "feat(graph): Edge::AnchorsOn 变体 + 全消费点补臂 + STORE_VERSION 9 (#158)"
```

---

## Task 6: builder 建边 —— 签名（Param/RETURN）扁平串

**Files:**
- Modify: `src/graph/builder.rs:1732` `create_object_ref_edges`（加 `table_index: &mut HashMap<String, NodeIndex>` 参数；调用点 :1603-1709 区间的传递链同步加参）
- Test: `src/graph/builder.rs` `#[cfg(test)] mod tests`

**Step 1: 写失败测试**

```rust
#[test]
fn should_create_anchor_edge_from_function_return_type() {
    // CREATE FUNCTION f(...) RETURN par_sys_purchase.purchase_days%TYPE ...
    // build 后断言：存在 Edge::AnchorsOn { kind: PercentType,
    //   column: Some("purchase_days"), site: ReturnType, .. }，目标为 Table 节点
    // 且该表无 DDL → 节点为 inferred（explicit: false）
}

#[test]
fn should_create_anchor_edge_from_param_type() {
    // 参数 p_in DAT_TRD_REPURCHASE%ROWTYPE → AnchorsOn { kind: PercentRowType,
    //   column: None, site: Param }
}
```

**Step 2:** Run: `cargo test should_create_anchor_edge` —— 失败（无边）。

**Step 3: 最小实现**（CreateProcedure / CreateFunction 分支内，紧邻现有 `ReferencesType` 参数循环）

```rust
// 签名参数（扁平串兜底，issue #158）
for param in &p.parameters {
    if let Some(mut a) = crate::parser::parse_anchor_from_type_string(&param.data_type) {
        a.site = AnchorSite::Param;
        Self::add_anchor_edge(graph, proc_idx, &a, file_arc.clone(), info.start_line, table_index);
    }
}
// RETURN
if let Some(rt) = &f.return_type {
    if let Some(mut a) = crate::parser::parse_anchor_from_type_string(rt) {
        a.site = AnchorSite::ReturnType;
        Self::add_anchor_edge(graph, proc_idx, &a, file_arc.clone(), info.start_line, table_index);
    }
}
```

共享 helper（照抄 :2881-2908 的解析/创建模式）：

```rust
fn add_anchor_edge(
    graph: &mut CodeGraph,
    proc_idx: NodeIndex,
    anchor: &crate::parser::AnchorRef,
    file: Arc<PathBuf>,
    line: usize,
    table_index: &mut HashMap<String, NodeIndex>,
) {
    // anchor.object 可能是 "schema.table" 或裸表名：取末段为表名、前段为 schema
    let (schema, table) = anchor.object.rsplit_once('.')
        .map(|(s, t)| (Some(s), t))
        .unwrap_or((None, anchor.object.as_str()));
    let key = normalize_table_key(schema, table);
    let table_idx = *table_index.entry(key).or_insert_with(|| {
        // Node::Table { explicit: false, ... } 照 :2893-2907
    });
    graph.add_edge(proc_idx, table_idx, Edge::AnchorsOn {
        kind: anchor.kind.clone(),
        column: anchor.column.clone(),
        site: anchor.site.clone(),
        location: SourceLocation { file, line },
    });
}
```

（实现时以 `:2881-2913` 的 schema 归一化为准，勿重新发明。）

**Step 4:** Run: `cargo test should_create_anchor_edge` —— PASS。

**Step 5: 提交**

```bash
git add src/graph/builder.rs
git commit -m "feat(graph): 签名 Param/RETURN 锚定建 AnchorsOn 边（含 inferred table*） (#158)"
```

---

## Task 7: builder 建边 —— 变量/嵌套锚定 + 双边共存

**Files:**
- Modify: `src/graph/builder.rs`（`create_object_ref_edges` 各分支 walk `AnchorExtractor`；CreatePackage/Body → `collect_package_object_ref_edges` 同步处理 `PackageItem::{Variable, Cursor}` 与例程签名）
- Test: `src/graph/builder.rs` tests + `tests/regress_issue_158_type_anchor_edges.rs`（新建）

**Step 1: 写失败测试**

```rust
// builder tests 内：
#[test]
fn should_keep_table_access_and_anchor_edges_separate() {
    // 函数体：SELECT ... FROM par_sys_purchase + DECLARE v par_sys_purchase.purchase_days%TYPE
    // 断言：两节点间 TableAccess（含 Read）与 AnchorsOn 各一条，互不合并
}

#[test]
fn should_not_create_anchor_edge_for_cursor_rowtype() {
    // cursor c + rec c%ROWTYPE → 无 AnchorsOn 边（端到端回归，验收项5）
}

// tests/regress_issue_158_type_anchor_edges.rs（新建，仿既有 regress_issue_* 的 setup）：
#[test]
fn issue_158_anchor_edges_end_to_end() {
    // issue 实测样例：FNC_GET_PURCHASE_JS_DAYS
    //   RETURN par_sys_purchase.purchase_days%TYPE
    //   v_purchase_days par_sys_purchase.purchase_days%TYPE
    //   v_repurchase_date dat_trd_repurchase.purchase_date%TYPE
    //   + SELECT ... FROM par_sys_purchase（无 dat_trd_repurchase DML）
    // 断言：
    // 1. f → par_sys_purchase：TableAccess[Read] 与 AnchorsOn 各一条
    // 2. f → dat_trd_repurchase：仅 AnchorsOn（inferred table*）
    // 3. lineage 不含因锚定产生的 hop（lineage 只认 TableAccess/DependsOn）
    // 4. find_conflicts 不含 AnchorsOn
}
```

**Step 2:**

Run: `cargo test --test regress_issue_158_type_anchor_edges`
Expected: 失败（`issue_158_anchor_edges_end_to_end` 断言不满足）。

Run: `cargo test should_keep_table_access_and_anchor_edges_separate`
Expected: 失败（双边共存未实现）。

Run: `cargo test should_not_create_anchor_edge_for_cursor_rowtype`
Expected: 失败（builder 尚未对包级/块级 cursor 消歧义）。

**Step 3: 实现**：各分支 `walk_pl_block(&mut anchor_extractor, block)`（**每 statement 新实例**，对齐 `TypeSequenceRefExtractor` 现有调用点模式）；`collect_package_object_ref_edges` 处理 `PackageItem::Variable`（直接 push_anchor 语义）、`PackageItem::Cursor`（登记 cursor 名）、包级例程签名（`PackageFunction.return_type`/`parameters`）。同 routine 内以 `HashSet<(object_lower, column, kind, site)>` 去重，避免同列多变量产生重复边。

**Step 4:**

Run: `cargo test --test regress_issue_158_type_anchor_edges`
Expected: PASS

Run: `cargo test should_keep_table_access_and_anchor_edges_separate`
Expected: PASS

Run: `cargo test should_not_create_anchor_edge_for_cursor_rowtype`
Expected: PASS

**Step 5: 提交**

```bash
git add src/graph/builder.rs tests/regress_issue_158_type_anchor_edges.rs
git commit -m "feat(graph): 变量/嵌套/包级锚定建边，DML+锚定双边共存 (#158)"
```

---

## Task 8: edge_label_for 平行边标签聚合（决策 D1）

**Files:**
- Modify: `src/graph/traverse.rs:52-100`
- Test: `src/graph/traverse.rs` tests（或相邻 `#[cfg(test)]`）

**Step 1: 写失败测试**

```rust
#[test]
fn should_aggregate_parallel_edge_labels_into_one_bracket() {
    // proc → table 同时有 TableAccess[Read] 与 AnchorsOn → 标签 "[R,T]"
}

#[test]
fn should_keep_single_edge_label_unchanged() {
    // 仅 TableAccess[Read] → "[R]"；仅 DirectCall → "[intra]"（回归锁定）
}

#[test]
fn should_dedupe_and_keep_first_seen_order() {
    // 两条边产出相同标签 → 只出现一次
}
```

**Step 2:** Run: `cargo test should_aggregate_parallel` —— 失败（当前 `.next()` 只取一条）。

**Step 3: 实现**：

```rust
pub(crate) fn edge_label_for(
    graph: &crate::graph::CodeGraph,
    from: NodeIndex,
    to: NodeIndex,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for e in graph.edges_connecting(from, to) {
        if let Some(label) = edge_label_part(e.weight()) {   // 原 match 体抽成 per-edge 函数
            if !parts.contains(&label) {
                parts.push(label);
            }
        }
    }
    if parts.is_empty() { None } else { Some(format!("[{}]", parts.join(","))) }
}
```

（`edge_label_part` 即原 match 全体，含 `ContainsRoutine|ContainsMethod => None` 语义不变。整标签去重，不做段级拆分——YAGNI。注意 petgraph `edges_connecting` 对平行边是 LIFO 迭代——若需按创建顺序输出，收集后 `.rev()` 再去重，以测试 `[R,T]` 为准。）

**Step 4:** Run: `cargo test should_aggregate_parallel_edge_labels_into_one_bracket` —— PASS。

Run: `cargo test should_keep_single_edge_label_unchanged` —— PASS。

Run: `cargo test should_dedupe_and_keep_first_seen_order` —— PASS。

**Step 5: 提交**

```bash
git add src/graph/traverse.rs
git commit -m "feat(graph): edge_label_for 聚合同对平行边标签 [R,T] (#158)"
```

---

## Task 9: 验收矩阵端到端 + 全量门禁

**Files:**
- Test: `tests/regress_issue_158_type_anchor_edges.rs`（Task 7 已建，本任务补齐验收断言）

**Step 1: 补齐 issue 验收项断言**（对应 issue §验收，逐条落测试）：

1. `detail` CALLEES 同时含 `par_sys_purchase` 的 `[R]` 与 `[T]`（经 `edge_label_for` 聚合为 `[R,T]`，断言包含两个标签段）
2. `dat_trd_repurchase` 以 AnchorsOn 出现（无 DML）
3. `lineage`：锚定边不产生 hop；`find_conflicts` / summarize 路径不把 AnchorsOn 计为 READ（conflicts 断言在 Task 7 测试内，此处复核）
4. `impact`（`--edge` 默认 all）从 `dat_trd_repurchase` 可达该函数（`EdgeFilter::new()` 全边遍历验证）
5. `cursor%ROWTYPE` 无表边（Task 7 已断言）
6. 旧 store：version=8 payload 拒载并提示重建（Task 5 已断言）

**Step 2: 全量门禁**（AGENTS.md 提交前矩阵，与 CI 一致）

```bash
cargo fmt --all -- --check
cargo clippy --features full -- -D warnings
cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_
```

Expected: 全绿。（CI 主动跳过的 `test_path_mapping_applied`/`test_serve_*` 为既有环境限制，勿因它们改实现。）

**Step 3: 提交**

```bash
git add tests/regress_issue_158_type_anchor_edges.rs
git commit -m "test: #158 验收矩阵端到端回归（锚定边可见性与隔离性）"
```

---

## 执行备注（2026-09-08 实施后补记）

实际执行与计划的偏差（均经 subagent 双阶段审查确认）：

- Task 1：`parse_anchor_from_type_string` 最终参数化为 `(s: &str, site: AnchorSite)`（质量审查建议，编译期强制调用方决定 site）；发现计划参考实现对 `% type`（% 与 type 间空格）字面匹配失败，改为定位 `%` + trim；修 Unicode 小写变宽字节偏移；补 3 段 schema、Unicode 回归测试。
- Task 2/3：cursor 负路径测试提前到 Task 2；Task 3 顺手统一 Variable 分支复用 `visit_pl_data_type` + 补 VarrayOf 特征测试。
- Task 4：真实 AST 对 `v2 v1%TYPE` 产出 `column: Some("")`（空串非 None），守卫语义不受影响；混合场景测试即绿（Task 2 守卫已覆盖），作为特征测试保留。
- Task 5：STORE_VERSION 保持模块私有 `const`（无外部引用）；mermaid 箭头对齐 ReferencesType 的实线（视觉家族一致性审查后统一）；json `column` 加 `skip_serializing_if` 对齐 TableAccess 先例。
- Task 6：`parse_anchor_from_type_string` 此前未从 parser/mod.rs re-export（Task 1 计划遗漏），本任务补；质量审查发现 store.dedup() 会静默折叠同 (proc,table) 对上不同列的锚定边——加 `"anchors_on"` 专分支按 `(kind, column, site)` 去重保留不同组合；fixture 升级为 3 段 schema 限定。
- Task 7：包级 Variable 此前完全被 `continue`（无既有锚定主体先例）→ 锚定到 Package 节点；包成员例程签名锚定此前缺失 → 补齐；提取 `collect_routine_anchor_edges` 消除三处 ~35 行重复；`AnchorKind`/`AnchorSite` 补 `Hash` derive（去重键需要）。
- Task 8：petgraph `edges_connecting` 平行边为 LIFO 迭代，计划参考代码会产生 `[T,R]` —— 收集后 `.rev()` 还原创建顺序（经实证：比按 EdgeIndex 排序更稳健，remove_edge 的 swap_remove 会重用索引）。
- Task 9：crate 为 bin-only（无 lib target），集成测试一律走编译后 CLI 二进制（与既有 tests/ 全部一致）；4 个验收缺口（lineage 排除 / conflicts 排除 / impact 可达 / detail 双标签）全部以 CLI 等价验证 + 变异法证明测试有效性。
- 外部 review 修复：局部 TYPE/RECORD 声明名纳入 var_names 守卫（防伪表锚）；dedup 键 column 小写归一（对齐 openGauss 标识符折叠语义）；记录已知近似——锚定边 line 取 routine 起始行（AST 无 span，结构化签名类型是 ogsql-parser follow-up）。

## Non-goals（本期不做）

- 游标 `RETURN t%ROWTYPE` 锚定（D3，follow-up）
- CGEF import 白名单扩展 `anchors_on`（D4，follow-up）
- ogsql-parser 把签名类型结构化成 `PlDataType`（issue 明示 follow-up）
- impact `find_edge()` 平行边单边取样的既有缺陷（默认 all 下无影响；只记录）
- 不改任何人类已有测试断言；不新增 feature flag / 依赖

## 完成标准（AGENTS.md Definition of Done）

- [ ] `cargo build` 与 `cargo build --features full` 均 0 错误
- [ ] 新行为：每 Task 先失败后通过的测试（函数名列出）
- [ ] `cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_` 全绿
- [ ] `cargo clippy --features full -- -D warnings` 干净；`cargo fmt --all -- --check` 干净
- [ ] `STORE_VERSION` 8→9，旧 store 拒载有测试
- [ ] 汇报按 AGENTS.md 格式：测试行为 / 改动文件 / 重构边界 / 实际命令与结果
