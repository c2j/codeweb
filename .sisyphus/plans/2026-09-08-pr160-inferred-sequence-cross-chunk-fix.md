# PR #160 Review 修复：inferred sequence 跨 chunk 重复与升级缺失

PR: https://github.com/c2j/codeweb/pull/160
Review: c2j 的 [bug] 评论（inferred_sequence_index 不跨 chunk、CREATE SEQUENCE 不升级、无 dedup 兜底、测试盲区）
分支：feat/issue-159（追加 commit，不改写已推送历史之外的本次修复 commit）

## 1. 根因（已逐条代码验证）

1. `create_object_ref_edges`（builder.rs L1740）内 `inferred_sequence_index` 是函数局部 HashMap，每 chunk 重建；而 `Project::analyze` 按 ≤100 文件/chunk 循环 `build_sql_chunk(&mut ctx, ...)`（project/mod.rs L208-233，ctx 跨 chunk 共享）
2. `CREATE SEQUENCE` 处理（builder.rs L646-663）只查 `sequence_index.contains_key(&full_key)`；inferred 节点从不进入 `sequence_index` → 后续 chunk 的 DDL 走 `add_node` 造出兄弟 explicit 节点，不原位升级
3. 兜底缺失：`finalize_graph`（L311）无 sequence 去重；`pick_richer_node`（store.rs L1775）无 Sequence 臂
4. 触发条件：同一序列「引用 chunk 在前、DDL chunk 在后」（文件字母序使存过先于 DDL 是常态）；单 chunk 内 DDL pass 先于 inference pass 所以安全——正是测试盲区成因

## 2. 修复设计（4 处代码改动）

### 2.1 `GraphBuildContext` 增加兄弟索引（builder.rs L127-160）

```rust
pub inferred_sequence_index: HashMap<String, petgraph::graph::NodeIndex>,
```

- `new()` 初始化（唯一构造点；`build_graph_internal` 与 project/mod.rs 均走 `GraphBuildContext::new()`，无其他改动）
- 选择兄弟 map 而非直接写入 `sequence_index`：保持「DDL 纯索引」语义，消费方无需 `explicit` 判别；升级时从兄弟 map 移除并写入 `sequence_index`

### 2.2 `create_object_ref_edges` 使用 ctx 级缓存（L1733-1740）

- 签名追加 `inferred_sequence_index: &mut HashMap<String, petgraph::graph::NodeIndex>`（调用点 L300-306 传 `&mut ctx.inferred_sequence_index`；`collect_package_object_ref_edges` 的 `&mut` 透传保持不变）
- 删除 L1740 的局部 `HashMap::new()`

### 2.3 `CREATE SEQUENCE` 原位升级（L646-663）

镜像 `resolve_or_infer_sequence` 的**精确 key 纪律**（杜绝短名模糊匹配重新引入跨 schema 误绑定）：

```text
若 sequence_index 含 full_key：跳过（现状不变）
否则：
  promoted = schema.is_some()
      ? inferred_sequence_index.remove(full_key)   // 限定 DDL 只升级限定推测节点
      : inferred_sequence_index.remove(short_key)  // 无前缀 DDL 只升级无前缀推测节点
  命中 → 原位改写该节点：explicit = true, location = Some(DDL 位置)
        并 sequence_index.entry(short_key).or_insert(idx) + insert(full_key, idx)
        （petgraph 权重原位改写不改 NodeIndex，既有 UsesSequence 边自动指向升级后节点）
  未命中 → 现状 add_node 路径不变
```

- `create_sql_nodes` 签名追加 `inferred_sequence_index: &mut HashMap`（调用点同步）
- 升级分支加必要注释说明精确 key 纪律（非显而易见的不变量，防止未来重构回退）

### 2.4 `pick_richer_node` 加 Sequence 臂（store.rs L1775，dedup/merge 兜底）

```rust
(Node::Sequence { location: Some(_), .. }, Node::Sequence { location: None, .. }) => idx_a,
(Node::Sequence { location: None, .. }, Node::Sequence { location: Some(_), .. }) => idx_b,
```

镜像既有 Table 臂的 location 风格。既有 View 臂缺失属 pre-existing，不扩大范围。

### 2.5 明确不做

- 不新增 finalize 序列去重 pass（构建期已防重 + merge 期 pick_richer_node 兜底即可，避免过度工程）
- 不 bump `STORE_VERSION`（无序列化形状变更；旧 store 合法。含历史双节点的新构建产物由 `codeweb dedup` 清理——PR 回评说明）
- 残余歧义（chunk1 无前缀推测 `seq_id` + chunk2 限定 `CREATE SEQUENCE finance.seq_id` → key 不同不升级、双节点保留）记录于 PR 回评，不引入 finalize 重建 pass

## 3. TDD 步骤

### 3.1 Red — 新测试（全部先写，确认失败）

**builder.rs tests（两 chunk 风格，仿 L5464：共享 `GraphBuildContext` + 多次 `build_sql_chunk` + `finalize_graph`）**：

1. `two_chunk_reference_then_ddl_promotes_inferred_sequence`
   - chunk1：存过引用 `my_seq.NEXTVAL`（无 DDL）；chunk2：`CREATE SEQUENCE my_seq;`
   - 断言：恰好 1 个 Sequence 节点；`explicit == true`；`location.is_some()`；UsesSequence 边指向该节点（升级不改 NodeIndex，边必须存活）
2. `two_chunk_duplicate_references_share_single_inferred_sequence`
   - chunk1：存过 A 引用 `my_seq`；chunk2：存过 B 引用 `my_seq`（均无 DDL）
   - 断言：1 个 Sequence 节点（explicit: false），2 条边指向同一 NodeIndex
3. `ddl_does_not_promote_other_schema_inferred_sequence`
   - chunk1：引用 `hr.seq_id.NEXTVAL`；chunk2：`CREATE SEQUENCE finance.seq_id`
   - 断言：2 个不同节点（hr.seq_id explicit:false；finance.seq_id explicit:true）；边仍指向 hr.seq_id

**store.rs tests**：

4. `pick_richer_node_prefers_located_sequence`（同模块直测私有 fn，仿既有 pick_richer_node 测试风格）
   - `Node::Sequence { location: Some, .. }` vs `{ location: None, .. }` → 返回 Some 侧 idx

### 3.2 Green — 按 2.1→2.2→2.3→2.4 顺序实施

### 3.3 Refactor

无（改动本身即收敛）；重构后重跑同组测试。

## 4. QA 场景

| 步骤 | 命令 | 预期 |
|---|---|---|
| R1 | `cargo test --features full two_chunk_` | 3 个新两 chunk 测试 Red（当前断言失败：节点数 2） |
| R2 | `cargo test --features full pick_richer_node_prefers_located_sequence` | Red（无 Sequence 臂，返回 idx_a） |
| G1 | `cargo build --features full` | exit 0 |
| G2 | 重跑 R1、R2 | 全部 pass |
| G3 | `cargo test --features full procedure_using` | 既有测试 pass |
| G4 | `cargo test --features full --test regress_issue_159_sequence_inferred` | 既有集成回归 pass |
| G5 | 两 chunk 手工验证（可选）：临时项目 >100 文件或直接调 `sql_chunk_size` 配置构造跨 chunk 场景，analyze 后 `nodes -t seq` 仅 1 节点 | 与单测一致 |

## 5. 门禁

```bash
cargo fmt --all -- --check
cargo clippy --features full -- -D warnings
cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_
```

## 6. 风险与边界

- 借用检查：`create_object_ref_edges` 调用点同时取 `&ctx.sequence_index` 与 `&mut ctx.inferred_sequence_index`——不相交字段借用，合法
- 升级路径的 `graph[idx]` 原位改写：`CodeGraph = petgraph::Graph<Node, Edge>` 支持 `IndexMut<NodeIndex>`；不改 NodeIndex，既有边零迁移
- 测试 1 的边存活断言是升级正确性的关键证据（若实现误删节点重建会在此失败）
- 既有全部测试（含 issue #159 的 10 个）必须保持通过——特别是 `ddl_does_not_promote_other_schema_inferred_sequence` 守护跨 schema 纪律
