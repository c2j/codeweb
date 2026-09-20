# Issue #171: MCP 模式生命周期工具（init / analyze / diff）

## 问题

`codeweb mcp` 只暴露 8 个只读查询工具。冷启动路径是断的：

1. `src/mcp/server.rs` 第一行 `Project::find(project_path)?` —— 目录里没有 `codeweb.toml` 时进程直接退出，客户端拿不到任何 JSON-RPC 响应。
2. `McpState` 只持有 `Arc<GraphStore>`，图中的 store 在启动时加载一次，之后不可变。
3. store 缺失/损坏/过期时，所有工具返回 `{"status":"empty"}` 并让用户手动去跑 `codeweb analyze` 然后**重启 MCP 进程**。
4. 即使外部跑了 `analyze`，内存图也不会刷新。

结果：LLM 客户端无法自行初始化项目、无法构建图谱、无法查看变更，必须人工介入。

## 目标（范围经确认）

只加三个生命周期工具，**不自动 analyze**：

| Tool | 语义 |
|---|---|
| `codeweb_init` | 在服务指向的目录创建 `codeweb.toml` + `.codeweb/`，不自动 analyze |
| `codeweb_analyze` | 全量/增量构建图谱，成功后热替换内存 store，无需重启 |
| `codeweb_diff` | 列出相对上次分析的变更文件 |

约束：

- 允许读用户指定的任意目录（config `analysis.paths`）。
- **写必须在许可目录内**（服务启动时确定的项目根）。不引入 `--allow-write` 开关。
- 可以修改 `Project::init` 的形状（加 `root` 参数）。

## 设计决策

### 1. 许可目录 = 项目根

- 已初始化：`Project::find(project_path)` 找到的 `codeweb.toml` 所在目录。
- 未初始化：`--project` 参数指向的目录（canonicalize 后）。

所有写操作（`codeweb.toml`、`.codeweb/`、store、manifest、`parse.log`）都必须落在该目录树内。
`confine_to_root(root, candidate)` 做词法归一化（处理 `.` / `..`）后校验 `starts_with(root)`，防止
`store.path = "../../escape.bincode"` 这类配置把写操作带出许可目录。读路径不校验。

### 2. 状态改为可变，查询走快照

```rust
pub struct McpState { inner: Arc<Inner> }

struct Inner {
    permitted_root: PathBuf,               // 唯一可写目录树
    project: Mutex<ProjectSlot>,           // Option<Project>，生命周期工具使用
    graph: RwLock<GraphSnapshot>,          // 查询工具使用
    // Project 与 store 分开加锁，避免 analyze 期间阻塞查询
}

struct GraphSnapshot {
    store: Arc<GraphStore>,
    project_name: String,
    empty_reason: Option<String>,
}
```

- 查询工具：`RwLock::read()` 取出 `Arc<GraphStore>` 快照后立即释放锁。
- `codeweb_analyze`：`tokio::task::spawn_blocking` 内持有 project 锁；analyze 是 CPU 密集同步函数，
  不能阻塞 async runtime。完成后写锁更新 `GraphSnapshot`。
- 因为 `Project::analyze()` 结尾会 `self.store = Some(new_store)`，热替换无需重读磁盘。
- 并发：`spawn_blocking` + `std::sync::Mutex` 串行化，避免两个 analyze 互相覆盖。

### 3. 未初始化不再退出

`Project::find` 失败时进入 uninitialized 状态，工具列表照常注册：

- 查询工具返回 `{"status":"uninitialized","hint":"call codeweb_init"}`。
- `codeweb_analyze` / `codeweb_diff` 同样引导到 `codeweb_init`，而不是让进程死掉。

### 4. stdout 通道隔离

MCP 的 stdout 是 JSON-RPC 通道。analyze 进度条（indicatif）与报告输出必须只走 stderr，
且**不得**调用 `print_analyze_report`（它走 stderr，但耦合 CLI 格式化），只序列化 `AnalyzeReport` 结构体。

## TDD 循环

| # | 行为 | 测试层级 |
|---|---|---|
| 1 | `Project::init_at(root, dirs, name)` 在指定目录建配置，相对路径相对 root | 单元（`src/project/mod.rs` tests） |
| 2 | `confine_to_root` 拒绝 `..` 逃逸、接受目录内路径 | 单元（`src/mcp/tools.rs` tests） |
| 3 | 未初始化目录启动 MCP 不退进程，stats 返回 uninitialized | 集成（`tests/mcp_test.rs`） |
| 4 | `codeweb_init` 创建 `codeweb.toml`，且不自动 analyze | 集成 |
| 5 | `codeweb_analyze` 构建图谱并热替换，随后 stats 返回 ready | 集成 |
| 6 | `codeweb_diff` 返回变更文件分类 | 集成 |
| 7 | `store.path` 逃逸许可目录时 analyze 返回错误且不写盘 | 集成 |
| 8 | 已分析且无变更的项目调用 analyze 报 `is_up_to_date:true` 且给出真实 nodes/edges（而非 up-to-date 短路返回的 0） | 集成 |

循环 7 的 Red 通过临时禁用 `confine_to_root` 验证：无守卫时 analyze 返回 `ready` 并在服务目录外写出文件。
循环 8 的 Red 通过临时改用 `report.nodes/edges` 验证：此时报告为 `0 nodes`，测试失败。

测试权限：`test_mcp_tools_list` 的期望工具数由 8 变 11 是本次 feature 的必然结果，
更新时保持「精确集合」断言而非放宽为子集断言，并在提交信息中说明。

## 影响文件

- `src/project/mod.rs` — 新增 `init_at`，`init` 委托
- `src/mcp/tools.rs` — 状态重构、3 个新工具、写守卫
- `src/mcp/server.rs` — 容错启动
- `tests/mcp_test.rs` — 新增集成测试
- `README.md` / `docs/DeveloperGuide.md` — 工具表与说明

## 明确不做

- `export` / `merge` 等其它写操作。
- `init` 后自动 analyze。
- 外部 analyze 后的自动重载（仍需调用 `codeweb_analyze` 或重启）。
- store 原子写（临时文件 + rename）。

## 门禁

```sh
cargo build --features full
cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_
cargo clippy --features full -- -D warnings
cargo fmt --all -- --check
```

## 交付状态

- 分支：`feat/issue-171-mcp-lifecycle-tools`
- PR：https://github.com/c2j/codeweb/pull/172
- 门禁结果：`cargo build --features full` 通过；`cargo test --features full -- --skip test_path_mapping_applied --skip test_serve_` 全绿（`mcp_test` 13 passed）；`cargo clippy --features full -- -D warnings` 干净；`cargo fmt --all -- --check` 干净；GitHub CI（Lint / Test ubuntu full）通过。
- 已知遗留：默认（非 mcp）构建下 `node_sub_type_tag`、`TreeNode::has_more/more_count` 报 dead_code，为既有 mcp-gated 代码，与本次改动无关。
- `tests/mcp_test.rs::test_mcp_tools_list` 期望工具集 8 → 11 为 feature 必然结果，保持精确集合断言。
