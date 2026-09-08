# Issue #159: 无 CREATE SEQUENCE 时为 seq.nextval 建 inferred seq* + UsesSequence

Issue: https://github.com/c2j/codeweb/issues/159
Branch: feat-issue-159（当前 worktree）

## 1. 目标

`seq.nextval` 被引用但分析范围内无 `CREATE SEQUENCE` DDL 时，当前 builder 静默丢弃 `UsesSequence` 边。改为对齐 `table*` 模式：创建 inferred `seq*` 节点并挂 `UsesSequence` 边，使 `detail` / `impact` 在缺 DDL 时仍能看到真实依赖。

**不做的事（issue 明确范围外）**：
- 不改 `sys_dummy`/`dual` 是否出现在 detail 默认 CALLEES
- 不把 sequence 编进 lineage 数据流（`UsesSequence` 已是 Reference）
- 不把 `seq.nextval` 建成 `TableAccess`
- 不动现有人类测试（`procedure_using_nextval_creates_uses_sequence_edge`、`procedure_using_dot_nextval_creates_uses_sequence_edge` 保持只读）

## 2. 设计决策

### 2.1 推测标记：加 `explicit: bool` 字段（而非 `location.is_none()` 哨兵）

镜像 `Node::Table`/`Node::View` 既有模式（mod.rs L505-532）：

```rust
/// A database SEQUENCE.
Sequence {
    schema: Option<String>,
    name: String,
    /// true when sequence has a DDL definition (CREATE SEQUENCE), false when
    /// only inferred from seq.nextval / currval / setval references.
    #[serde(default)]
    explicit: bool,
    /// None when sequence node was created implicitly (referenced but not parsed from DDL).
    #[serde(default)]
    location: Option<SourceLocation>,
},
```

理由：
1. `node_type_tag` 的 `"table*"`/`"view*"` 分支（mod.rs L670-677）可直接复制为 `"seq*"`；`location.is_none()` 方案语义不显式且无先例。
2. 无论选哪种方案，`location` 都必须改成 `Option`（推测节点无 DDL 文件可指）。
3. 与 Table/View 在 `is_inferred_node`（main.rs L2036）、export、merge 等处的处理方式保持一致。

### 2.2 Store 版本：`STORE_VERSION` 8 → 9

`Node::Sequence` 变体形状变化会破坏 bincode 位置式反序列化。仓库已有版本门禁机制（store.rs L1172 `stored_ver != STORE_VERSION` → 报错；L1225 `peek_version` → analyze fast path 强制重建，见 commit 0b636ac）。因此：

- `STORE_VERSION: u32 = 8` → `9`（store.rs L22）
- 「旧 store 可加载」验收 = 旧版本 store 被检测为过期并触发重建，不 panic、不死循环（沿用 0b636ac 的既有路径，已有测试覆盖）
- store.rs L2325-2342 附近的版本测试使用 `STORE_VERSION` 常量，自动跟随

## 3. TDD 步骤

### 3.1 Red — 新建测试（全部先写，确认失败/编译失败）

**单元测试**（`src/graph/builder.rs` `#[cfg(test)] mod tests`，复用现有 `build_from_sql` helper）：

1. `procedure_using_nextval_without_ddl_creates_inferred_sequence_node`
   - SQL: 仅 `CREATE PROCEDURE`（内含 `SELECT nextval('seq_batch_payment') INTO v FROM sys_dummy`），无 CREATE SEQUENCE
   - 断言: 恰好 1 条 `UsesSequence` 边；目标节点是 `Node::Sequence { explicit: false, location: None, .. }`
2. `select_dot_nextval_into_from_sys_dummy_creates_edge_with_ddl`
   - SQL: `CREATE SEQUENCE seq_batch_payment` + `SELECT seq_batch_payment.nextval INTO v FROM sys_dummy`
   - 断言: 恰好 1 条 `UsesSequence` 边（**不重复**）；目标节点 `explicit: true`
3. `dot_nextval_assignment_without_ddl_creates_inferred_sequence_node`
   - SQL: `v_id := my_seq.NEXTVAL` 赋值，无 DDL
   - 断言: 1 条边 + inferred 节点
4. `insert_values_nextval_without_ddl_creates_inferred_sequence_node`
   - SQL: `INSERT INTO t(id) VALUES(my_seq.NEXTVAL)`，无 DDL
   - 断言: 1 条边 + inferred 节点
5. `inferred_sequence_schema_qualified_ref_resolves`（schema 回退）
   - SQL: `SELECT s1.my_seq.nextval INTO v FROM sys_dummy`，无 DDL
   - 断言: 1 条边 + inferred 节点名为 `my_seq`

**tag/显示测试**（`src/graph/mod.rs` tests）：

6. `node_type_tag_inferred_sequence_is_seq_star`
   - `Node::Sequence { explicit: false, location: None, .. }` → `"seq*"`；`explicit: true` → `"seq"`

**store 版本测试**（`src/graph/store.rs` tests）：

7. `load_bincode_rejects_pre_issue_159_version`（完全跟随既有 `load_bincode_rejects_previous_layout_version`（约 L2348，version=7 场景）的模式）
   - 构造字节：`STORE_MAGIC` + `8u32.to_le_bytes()`（本次改动淘汰的旧版本）+ 8 字节占位
   - 断言 1：`GraphStore::load_bincode(&path)` 返回 err，错误信息包含 `"unsupported cache version"`
   - 断言 2：`GraphStore::file_is_current(&path) == false`（store.rs L1242 —— 这是 `Project::store_is_current()`（src/project/mod.rs L540）在 bincode 格式下调用的真实入口，即 analyze 增量快速路径判定"过期需重建"的依据）

**集成回归测试**（`tests/regress_issue_159_sequence_inferred.rs`，跟随 regress_issue_140/144 先例）：

8. 端到端：构建项目（仅 SELECT + sys_dummy，无 DDL）→ store 落盘 → `resolve`/detail 路径能看到 `seq_batch_payment` 节点与 `UsesSequence` 边；再跑一次 analyze（增量路径）不重复建边。

### 3.2 Green — 最小实现

**`src/graph/mod.rs`**：
- `Node::Sequence` 变体：加 `#[serde(default)] explicit: bool`、`location` → `#[serde(default)] Option<SourceLocation>`
- `node_type_tag`：`Sequence { explicit: false, .. } => "seq*"`（L681 拆成两臂）
- `Node::file()` L870：`&location.file` → 按 Table 模式（L859-866）`location.as_ref().map(...).unwrap_or(Path::new(""))`

**`src/graph/builder.rs`**：
- L651 `CREATE SEQUENCE` 构造：`explicit: true, location: Some(...)`
- `create_object_ref_edges`（L1785-1800 proc、L1856-1871 func）与 `collect_package_object_ref_edges`（L1964）三处：
  - lookup key 逻辑对齐表路径：`seq_ref.sequence_name` 含 `.`（schema 限定）→ 用全名 key 查，miss 再退短名；无前缀 → 短名查
  - miss 时：`graph.add_node(Node::Sequence { schema, name, explicit: false, location: None })` 并建 `UsesSequence` 边
  - 用函数内局部 `HashMap<String, NodeIndex>` 缓存本次调用已建的 inferred 节点（不修改 `sequence_index` 签名，避免 &mut 传染）
  - 抽一个共享 helper（如 `fn resolve_or_infer_sequence(...) -> NodeIndex`）供三处调用，避免复制三遍

**`src/main.rs`**：
- `is_inferred_node`（L2036）：加 `Node::Sequence { explicit: false, .. }` → detail 自动打印 `⚠ inferred node`（L2290 既有路径，不改）

**`src/export/json.rs`**（两处，精确形状）：
- `NodeKindJson::Sequence` 定义（L172-177）改为与 `NodeKindJson::Table` 完全一致的 Option 语义并补 `explicit`：
  ```rust
  Sequence {
      name: String,
      schema: Option<String>,
      explicit: bool,
      file: Option<String>,   // None = inferred 节点（对齐 Table L434-458 的 JSON 形状）
      line: Option<usize>,
  },
  ```
- `Node::Sequence` → `NodeJson` match 臂（L527-539）改为 Table 臂同款写法：
  `file: location.as_ref().map(|l| l.file.to_string_lossy().to_string())`、`line: location.as_ref().map(|l| l.line)`、`explicit: *explicit`
- JSON 消费方无内部引用（server 静态资源、mcp、tui 均不解析 `NodeKindJson::Sequence`），Option 化仅影响对外 API 输出，与 Table/View 的既有输出惯例一致

**`src/import/parser.rs`** L420：
- CGEF sequence 节点：`explicit: true`（外部导入即有定义）

**`src/graph/store.rs`**：
- `STORE_VERSION` 8 → 9

**构造点补字段**（编译器兜底，机械改动）：
- `src/graph/mod.rs` tests L1173、L1575：加 `explicit: true` + `location: Some(loc)`（测试代码，本任务可改）

### 3.3 Refactor

- 三处 miss 分支收敛到共享 helper 后，若 proc/func 两处外层循环结构仍重复，仅在当前改动路径内做小范围提取；不做超出路径的重构
- 重构后立刻重跑同一组测试

## 4. 验收映射

| Issue 验收项 | 对应测试 |
|---|---|
| 无 DDL 时 detail CALLEES 出现 `seq_batch_payment [uses_seq]` | 单测 1 + 集成 8 |
| 有 DDL 时仍一条边、explicit、不重复 | 单测 2 |
| `SELECT seq.nextval INTO v FROM sys_dummy` 回归（抽取 + 有/无 DDL） | 单测 1、2 |
| `nextval('seq')` / 赋值 / `INSERT VALUES` 无 DDL 建 inferred 边 | 单测 3、4 |
| 旧 store 可加载 | store 版本门禁重建（决策 2.2）+ 测试 7 |

## 5. 每任务 QA 场景（工具 + 步骤 + 预期结果）

### QA-A 新增单元测试（Red 阶段）

| 步骤 | 命令 | 预期 |
|---|---|---|
| A1 | `cargo test --features full procedure_using_nextval_without_ddl_creates_inferred_sequence_node 2>&1 \| tail -5` | **编译失败**（`Node::Sequence` 无 `explicit` 字段 / `location` 非 Option）—— 合法 Red |
| A2 | `cargo test --features full node_type_tag_inferred_sequence_is_seq_star` | 同上，编译失败 |
| A3 | `cargo test --features full load_bincode_rejects_pre_issue_159_version` | **断言失败**（当前 STORE_VERSION=8，version=8 的文件被接受）—— 合法 Red；先改 `STORE_VERSION=9` 后此测试即绿，作为 2.2 的验证 |
| A4 | `cargo test --features full --test regress_issue_159_sequence_inferred` | Red：编译失败或断言失败 |

### QA-B 最小实现（Green 阶段）

| 步骤 | 命令 | 预期 |
|---|---|---|
| B1 | `cargo build --features full` | 退出码 0；编译器逐个暴露所有 `Node::Sequence` 构造点 / exhaustive match 漏改处（mod.rs tests L1173/L1575、json.rs、import/parser.rs、builder.rs L651） |
| B2 | 重跑 QA-A 全部 4 条命令 | 全部 pass（0 failed） |
| B3 | `cargo test --features full procedure_using_nextval_creates_uses_sequence_edge` 与 `cargo test --features full procedure_using_dot_nextval_creates_uses_sequence_edge`（**两条独立命令**，`cargo test` 只接受一个 TESTNAME 过滤参数） | 既有 2 测试各自 pass（未改人类测试，DDL 存在路径行为不变） |

### QA-C store 版本与增量回归

| 步骤 | 命令 | 预期 |
|---|---|---|
| C1 | `cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_ store` | store 模块全部测试 pass，含既有 `load_bincode_rejects_previous_layout_version`（version=7 仍被拒） |
| C2 | 集成测试 8（`tests/regress_issue_159_sequence_inferred.rs` 内）：先以旧格式落盘（或手写 version=8 头文件），再调用 `GraphStore::file_is_current` → false；随后正常 `analyze` 全量重建 → `file_is_current` → true | 断言通过 = 「旧 store 可加载（触发重建、不 panic、不死循环）」 |

### QA-D CLI 手工验收（issue 实测场景）

在预授权临时目录 `/var/folders/xh/8xyzggmj4jg02gnjyxwwbnb00000gn/T/opencode/issue159` 建项目：

```bash
TMP=/var/folders/xh/8xyzggmj4jg02gnjyxwwbnb00000gn/T/opencode/issue159
mkdir -p $TMP/sql
printf 'CREATE PROCEDURE p_pay() AS $$\nBEGIN\n  SELECT seq_batch_payment.nextval INTO v_seq FROM sys_dummy;\nEND;\n$$ LANGUAGE plpgsql;\n' > $TMP/sql/p.sql
cargo run -q --features cli -- init $TMP/demo -d $TMP/sql
cargo run -q --features cli -- detail p_pay -p $TMP/demo
cargo run -q --features cli -- export --format json -p $TMP/demo
```

（所有子命令显式 `-p $TMP/demo`：各子命令的 `--project` 默认是当前目录，`cargo run` 在仓库根执行时会找不到 `$TMP/demo` 的 codeweb.toml。）

预期输出：
- `detail` 的 CALLEES 区出现 `seq:seq_batch_payment [seq*] [uses_seq]`（无 DDL 场景）
- 追加 `CREATE SEQUENCE seq_batch_payment;` 到 `$TMP/sql/p.sql` 后重新 `analyze -p $TMP/demo`，`detail` 仍只显示一条 uses_seq 边，tag 变为 `seq`（非 `seq*`），无重复行
- `export --format json` 输出中该 sequence 节点含 `"explicit": false`（无 DDL）/ `true`（有 DDL）

（QA-D 为人工抽查；CI 依赖 QA-A~C 的自动化断言。）

## 6. 门禁（与 CI 一致）

```bash
cargo fmt --all -- --check
cargo clippy --features full -- -D warnings
cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_
```

另跑 `cargo build --features full`（cross-feature 回归；Node 变体变化可能影响 server/mcp 匹配臂）。

## 7. 风险与边界

- `Node::file()` L870 若漏改会在 detail/文件列表对 inferred seq 节点时 panic —— 单测 6 覆盖 `file()` 行为
- export/json.rs 的 None-location 渲染已在 3.2 固化为与 `NodeKindJson::Table` 完全一致的 Option 形状（`file: Option<String>`、`line: Option<usize>` + `explicit`），无歧义空间
- `--features full` 下 jsp/server/mcp 对 `Node::Sequence` 的 exhaustive match 由编译器强制检查
- 既有环境性失败（`test_path_mapping_applied`、`test_serve_*`）按 AGENTS.md 跳过，不算本次回归
