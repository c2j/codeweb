# 大目录 OOM 修复方案

> **状态：待审核** | 分支：`feat/performance`

## 问题现状

处理大目录时进程被 OOM killer 杀掉。`feat/performance` 分支当前与 `main` 同提交（e9f53eb），尚无性能/内存优化落地。

已有的优化措施（来自 2026-04-23 计划）：
- ✅ 单次 WalkDir 扫描
- ✅ SQL 分块解析（`SQL_CHUNK_SIZE=500`）
- ✅ Java 单次读盘（SQL + method 合并）
- ✅ rayon 并行解析

但这些优化**主要面向 CPU/速度**，未充分控制**内存峰值**。

## 根因分析

### R1：rayon 并行解析峰值无界（高危）

每块 500 个文件 × 全核并行，同时持有：源码 + tokens + AST（`parse_with_text` 保留全文）。单文件膨胀系数 5–20×，全核并行下峰值 ≈ `cores × 500 × 平均文件大小 × 膨胀系数`。

```
load_all_files → parse_sql_files(chunk) → rayon par_iter
每个线程：fs::read → decode → Tokenizer → Parser::parse_with_text
全核同时进行，无背压
```

### R2：Java / XML / JSP 不分块，全量常驻

```rust
// Java: 全部路径一次性解析
let java_combined = load_java_files_combined_with_config(&all_java_paths, &config);

// XML: 同一文件解析两次，两份结果同时存活
let ibatis_files = load_ibatis_files_from_paths(&all_xml_paths);           // flat
let structured_ibatis_files = load_ibatis_structured_files_from_paths(...); // structured
```

Java 所有 combined 结果 + 两份 XML 解析 + JSP 全量，在 SQL 分块释放后仍常驻内存。

### R3：图节点永久持有完整 SQL 文本（稳态内存）

每个节点内嵌全文：

| 节点类型 | 字段 | 说明 |
|---------|------|------|
| Procedure / Function | `body_sql[].sql_text` | 存储过程体中每条 SQL |
| Table / View / MaterializedView | `ddl_source` | 完整 CREATE DDL |
| MappedStatement | `sql` | 展开后的 mapper SQL |
| JavaSql | `sql` | Java 中提取的 SQL |
| JspSql | `sql` | JSP 中提取的 SQL |

大仓库最终图可达数 GB；`GraphStore` 还有多套字符串索引（name / fingerprint / lock clause…）再复制。

### R4："增量分析"实际是全量重建

`compute_changes` 只用于判断是否 up-to-date。一旦有变更：
- 先加载完整旧 store（只为取 manifest）
- 解析**全部**文件、新建整图
- 峰值 ≈ 旧图 + 新图 + 解析缓冲

### R5：动态 SQL 笛卡尔积（已防护）

`MAX_VALUE_SET=64` 已防 2^n 爆炸。极端 PL 仍会抬高单文件峰值，但非大目录 OOM 主因。

### R6：图存档加载时 `deserialize` 峰值

`GraphStore::load_bincode` 反序列化整图到内存，大仓库加载本身就可能近 OOM 线。

## 修复路线

### Phase A：压解析峰值（小改动，高 ROI）

#### A1：限制 rayon 并行度

- 新增 `[analysis] threads = 4` 配置项
- 默认不设或设 4，允许用户调
- 环境变量 `CODEWEB_THREADS` 覆盖
- 在 `parse_sql_files` / `load_java_files_combined_with_config` / `load_ibatis_files_from_paths` 等入口处应用

#### A2：缩小 SQL 分块 + 可配置

- `SQL_CHUNK_SIZE` 500 → 100（默认）
- 新增 `[analysis] sql_chunk_size = 100` 配置项
- 环境变量 `CODEWEB_SQL_CHUNK_SIZE` 覆盖

#### A3：Java / XML / JSP 分块解析

- Java：按 50 个文件一批，parse → build → drop
- XML：按 50 个文件一批，合并 flat + structured 为单次解析
- JSP：按 50 个文件一批

对应修改 `project/mod.rs` 的 analyze 流程，将 Java/XML/JSP 改为与 SQL 相同的 chunk → build → drop 模式。

#### A4：XML 合并为单次解析

新增 `ibatis_loader` 方法：一次读盘，同时返回 `(ParsedMapper, StructuredMapper)`。

```rust
pub struct IbatisCombinedFile {
    pub path: PathBuf,
    pub flat: ParsedMapper,
    pub structured: StructuredMapper,
    pub content_hash: String,
}

pub fn load_ibatis_files_combined(paths: &[PathBuf]) -> Vec<IbatisCombinedFile>;
```

#### A5：全量重建时不加载完整旧 store

`try_load_store()` 改为只反序列化 manifest。新增 `GraphStore::load_manifest_only()` 或改变加载策略。

### Phase B：压稳态图内存（中改动，中 ROI）

#### B1：SQL 文本懒加载（默认不内嵌）

- `body_sql` / `ddl_source` / mapper `sql` / JavaSql `sql` / JspSql `sql` 改为 `file + offset + length`（或按 feature flag `keep-sql-text` 可选保留）
- `detail` / `trace-sql` 等需要展示时再从源文件读取
- 搜索索引 `sql_fingerprint_index` 保留（指纹只有 32B）

新增 `[analysis] keep_sql_text = false` 配置项，默认 false。

#### B2：路径/标识符 string interning

- `PathBuf` → `Arc<Path>` 或 `InternedString`
- schema / package / name 高频重复，大量 duplicate 字符串

评估 `string_cache` 或使用 `Arc<str>` + 手工 intern table。

#### B3：GraphStore 索引去重

`name_index` / `sql_fingerprint_index` / `lock_clause_index` 中 display_key 用 `Arc<str>` 共享而非克隆。

### Phase C：真增量（大改动，高 ROI）

#### C1：只重解析变更文件

- `added` / `modified`：parse → build → insert/update graph nodes
- `deleted`：从现有 graph 移除相关节点和边
- 避免全量重建

需要 `GraphStore` 支持部分更新：
- `remove_file_nodes(path)`
- `add_file_nodes(path, nodes, edges)`
- `update_manifest_entry(path, record)`

### Phase D：图加载优化

#### D1：压缩图存储

- bincode 改为带压缩（zstd/lz4）
- 减少 IO，不减少内存

#### D2：mmap + 按需加载索引

- 大图分两部分存储：索引（mmap 常驻）+ 节点详情（按需加载）

## 实施顺序

| 阶段 | 任务 | 改动量 | 预期效果 | 风险 |
|------|------|--------|---------|------|
| A1 | 限制 rayon 并行度 | 30 行 | 峰值降 50–75% | 低 |
| A2 | SQL 分块缩小 | 5 行 | 峰值再降 | 低 |
| A3 | Java/XML/JSP 分块 | 100 行 | 消除 Java/XML/JSP 峰值 | 中 |
| A4 | XML 合并解析 | 40 行 | 减半 XML 峰值 | 低 |
| A5 | 轻量 manifest 加载 | 30 行 | 消除旧图峰值 | 低 |
| B1 | SQL 文本懒加载 | 300 行 | 稳态内存降 40–70% | 中 |
| B2 | string interning | 200 行 | 稳态内存再降 10–20% | 中 |
| C1 | 真增量 | 500 行 | 增量场景接近零解析 | 高 |

**建议首轮：A1–A5 全部 + B1。** 可解决 OOM 问题且改动可控。

## 需要用户确认

1. **典型场景规模**：文件数？总大小？机器内存上限？
2. **优先目标**：不被 OOM 杀掉为主，还是也要明显加速？
3. **SQL 文本展示需求**：`detail` / `trace-sql` 是否需要 sql 文本常驻？接受懒加载（`file + offset` 实时读取）吗？
4. **实施节奏**：A1–A5 先落地测试，B1 跟进？还是一次性全部？

## 验证矩阵

```sh
cargo build --features full
cargo test --features full
cargo clippy --features full -- -D warnings
cargo fmt -- --check
```
