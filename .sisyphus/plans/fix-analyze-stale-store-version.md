# Fix: analyze 不校验 store 版本 → 升级二进制后 stale 缓存死循环

## 问题（来自真实用户场景）

用户升级 codeweb 二进制（STORE_VERSION 7→8, PR #148）后，在旧项目目录执行：

```
codeweb analyze   →  "Up to date. 136 files, 0 nodes, 0 edges."   （不重建）
codeweb stats     →  error: unsupported cache version 7, expected 8 — run `codeweb analyze` to regenerate
```

死循环：错误信息让用户跑 analyze，但 analyze 因指纹未变而拒绝重建。

## 根因

`src/project/mod.rs:143-172` — `analyze()` 的 up-to-date 判定只比较**文件指纹 vs manifest 边车**（`load_manifest_only` → `compute_changes`），从不触碰 store 文件本身。而 STORE_VERSION 升级作废的是 store（`src/graph/store.rs:22` `STORE_VERSION: u32 = 8`），不是 manifest 边车（边无版本头，`FileRecord` 布局 v7↔v8 未变，旧边车反序列化成功）→ `changes.is_empty()` 为真 → 提前返回，v7 store 原样保留。

次生问题：up-to-date 提前返回时 `self.store` 为 `None`，报告里 `nodes/edges` 来自 `unwrap_or(0)`（mod.rs:158-167），`print_analyze_report`（main.rs:3758-3764）于是永远打印 `0 nodes, 0 edges` —— 与 store 实际内容无关，误导用户。

## 修复设计（两个 TDD 循环）

### 循环 1（核心）：analyze 在 up-to-date 判定前校验 store 版本，不匹配 → 强制 full rebuild

**行为断言（Red 测试）**：`analyze_rebuilds_when_store_version_stale`（`src/project/mod.rs` `mod tests`，行 579 处已有测试模块，可在模块内直接构造 `Project { root, config, store: None }`，config 用 `ProjectConfig::load(toml_str)` 从字符串解析）

测试步骤：
1. `tempfile::TempDir` 建项目目录，写入一个 `.sql` 文件（内容任意，`parse_file` 对 tokenizer 失败才返回 Err，普通文本也会记录哈希 → 断言不依赖 SQL 解析成功）
2. `proj.analyze()` → 断言 `report.is_full_build == true`（建立基线，manifest 边车写入）
3. 用 v7 布局字节覆盖 `store.bincode`：`STORE_MAGIC + 7u32.to_le_bytes() + [0u8; 8]`（模板照抄既有测试 `load_bincode_rejects_previous_layout_version`, store.rs:2324-2343）；manifest 边车不动
4. 再跑 `proj.analyze()` → 断言 `report.is_up_to_date == false` 且 `report.is_full_build == true`
5. 断言磁盘上 `store.bincode` 头部版本 == 当前 `STORE_VERSION`（自愈验证）

**最小实现（Green）**：

1. `src/graph/store.rs` 新增轻量头探测（不做全量反序列化）：
```rust
/// Peek at the on-disk store's format version WITHOUT deserializing the payload.
/// Returns `None` for missing files and legacy headerless (pre-#110) stores;
/// callers treat `None` as stale.
pub fn peek_version(path: &Path) -> Option<u32> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = [0u8; 13];
    file.read_exact(&mut header).ok()?;
    if header[..9] != STORE_MAGIC {
        return None;
    }
    Some(u32::from_le_bytes([header[9], header[10], header[11], header[12]]))
}
```

2. `src/project/mod.rs` `Project` 新增私有方法：
```rust
/// True when the on-disk store exists and its format version matches
/// STORE_VERSION. Mismatch / missing / legacy headerless → stale.
fn store_is_current(&self) -> bool {
    let store_path = self.store_path();
    if !store_path.exists() { return false; }
    match self.config.store.format {
        config::StoreFormat::Bincode => {
            GraphStore::peek_version(&store_path).is_some_and(|v| v == STORE_VERSION)
        }
        config::StoreFormat::Json => {
            // JSON 无 13 字节头；全量加载后查 version 字段（JSON 格式为非默认 opt-in，
            // 且 load_json 自带版本门禁，mismatch 即 Err → false）
            GraphStore::load_json(&store_path).map(|s| s.version == STORE_VERSION).unwrap_or(false)
        }
    }
}
```

3. `analyze()` 修改一行判定（mod.rs:148）：
```rust
let is_up_to_date = changes.is_empty() && self.store_is_current();
```
   STORE_VERSION 不变（保持 8）；修复后 analyze 自愈：mismatch → 走既有 full build 路径 → `save_bincode` 覆写为 v8 + 重写边车。

**store.rs 单测（随循环 1 一起，锚定 peek 行为）**：
- `peek_version_returns_header_version`：写 `STORE_MAGIC + STORE_VERSION.to_le_bytes()` → `Some(STORE_VERSION)`
- `peek_version_none_for_legacy_headerless`：纯 bincode 字节（模板 store.rs:2346-2361）→ `None`
- `peek_version_none_for_missing_file`：不存在路径 → `None`

### 循环 2（次生）：up-to-date 输出不再显示假 0/0 计数

**行为断言（Red 测试）**：把 main.rs:3760-3764 的行文案构造提为纯函数并测试：
```rust
fn format_up_to_date_line(report: &project::AnalyzeReport) -> String {
    // store 未加载时（analyze 快路径）报告中的 nodes/edges 恒为 0，
    // 打印出来是误导 —— 只报文件数。
    format!("Up to date. {} files.", report.files_scanned)
}
```
测试：`up_to_date_line_reports_files_without_zero_counts` —— 构造 `AnalyzeReport { nodes: 0, edges: 0, files_scanned: 136, .. }`，断言输出为 `"Up to date. 136 files."` 且不含 `"0 nodes"`。
（注：核心 bug 修复后 up-to-date 快路径只在 store 版本匹配时可达，加载 store 换真计数会牺牲快路径性能；只报文件数是零成本且诚实的折中。`print_analyze_report` 改调该函数。）

## 明确不做（Out of scope）

- 不改 `STORE_VERSION`（保持 8，无需再 bump：修复不改变 v8 布局）
- 不动 `diff` 命令（只展示文件差异，不读 store）
- 不做 store 迁移/升级（full rebuild 即自愈，符合既有"拒绝+重建"设计，store.rs:1172-1179 错误信息语义不变）
- 不修 explore 报告提到的其他静默失败（loader 丢文件等，与本 bug 无关）

## 验证门禁（AGENTS.md 规定）

```bash
cargo test --features <default> <新测试名>          # 循环内单测
cargo fmt --all -- --check
cargo clippy --features full -- -D warnings
cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_
```

注：`test_serve_*` / `test_path_mapping_applied` 为 CI 既有环境跳过项，失败与本次无关。

## 风险与边界

- store 存在但边车缺失：`load_manifest_only` 返回空 → `is_full_build=true` → 本来就 full rebuild，行为不变
- v7 边车可被 v8 二进制正常反序列化（FileRecord 未变）→ 指纹判定仍为"无变化"，由 store 版本校验兜底触发重建 —— 两道检查互补
- 性能：bincode 快路径只读 13 字节；Json 格式全量加载但属非默认 opt-in
- `is_some_and`：Rust 1.70+ 稳定 API，仓库无 rust-toolchain.toml、CI 用 stable，可用
