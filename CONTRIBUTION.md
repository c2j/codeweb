# 贡献指南

感谢你对 codeweb 的关注！本文档帮助你了解如何参与项目开发。

## 目录

- [行为准则](#行为准则)
- [开发环境搭建](#开发环境搭建)
- [项目结构](#项目结构)
- [代码规范](#代码规范)
- [开发流程](#开发流程)
- [测试](#测试)
- [提交规范](#提交规范)
- [Pull Request 流程](#pull-request-流程)
- [文档贡献](#文档贡献)

---

## 行为准则

- 尊重所有贡献者，保持专业和建设性的沟通
- 代码审查时对代码不对人
- 接受建设性反馈，乐于改进

---

## 开发环境搭建

### 前置条件

- **Rust**：最新稳定版（通过 [rustup](https://rustup.rs/) 安装）
- **C 编译器**：用于编译 tree-sitter 原生依赖（macOS 自带 clang，Linux 需 `build-essential`）
- **Git**：用于版本控制

### 克隆与构建

```bash
git clone https://github.com/c2j/cobweb.git
cd cobweb

# 构建默认功能（CLI + TUI）
cargo build

# 构建全部功能（包含 HTTP 服务器 + 浏览器 UI）
cargo build --features full

# 运行测试
cargo test

# 运行全部测试（包含 serve 集成测试）
cargo test --features serve
```

### IDE 配置

推荐使用 VS Code + [rust-analyzer](https://marketplace.visualstudio.com/items?itemName=rust-lang.rust-analyzer) 插件。项目根目录下的 `.vscode/settings.json` 已包含推荐配置。

### 常用命令

```bash
cargo build                      # 构建
cargo build --features serve     # 构建 + HTTP 服务器
cargo build --features mcp       # 构建 + MCP 服务器
cargo build --features full      # 构建全部功能
cargo test                       # 运行单元测试
cargo test --features serve      # 运行全部测试（含 serve 集成测试）
cargo test --features mcp        # 运行全部测试（含 MCP 集成测试）
cargo test --test <name>         # 运行单个集成测试
cargo clippy -- -D warnings      # 代码检查（CI 门禁级别）
cargo clippy --features serve -- -D warnings
cargo clippy --features mcp -- -D warnings
cargo fmt -- --check             # 格式检查
cargo fmt                        # 自动格式化
```

---

## 项目结构

```
cobweb/
├── src/
│   ├── main.rs              # CLI 入口（clap 命令行解析，命令分发）
│   ├── error.rs             # 错误类型定义（thiserror）
│   ├── parse_log.rs         # 解析警告/错误日志记录
│   ├── parser/              # 源码解析层
│   │   ├── extractor.rs     # SQL 调用关系提取（CALL/EXECUTE 语句）
│   │   ├── loader.rs        # 文件加载与分发
│   │   ├── scanner.rs       # 源码目录扫描
│   │   ├── ibatis_loader.rs # MyBatis XML Mapper 加载
│   │   ├── java_loader.rs   # Java 源码加载
│   │   ├── java_method.rs   # Java 方法调用提取（tree-sitter）
│   │   └── fingerprint.rs   # 文件内容指纹（增量分析基础）
│   ├── graph/               # 图模型层（核心）
│   │   ├── store.rs         # GraphStore：序列化、合并、索引、搜索
│   │   ├── builder.rs       # 从解析结果构建有向图
│   │   ├── traverse.rs      # 图遍历（trace 链、格式化输出）
│   │   ├── resolver.rs      # 节点解析与匹配
│   │   ├── key.rs           # NodeKey 类型定义
│   │   ├── search/          # 模糊搜索（fuzzy.rs）与模块定义
│   │   └── query/           # 声明式查询引擎
│   │       ├── spec.rs      # QuerySpec 定义与执行
│   │       ├── traversal.rs # 图遍历引擎
│   │       └── filter.rs    # 节点/边过滤器
│   ├── export/              # 导出层
│   │   ├── dot.rs           # Graphviz DOT 格式
│   │   ├── json.rs          # JSON 序列化
│   │   └── mermaid.rs       # Mermaid 流程图
│   ├── import/              # CGEF 导入层
│   │   ├── format.rs        # CGEF 文档数据模型
│   │   ├── parser.rs        # CGEF → 内部图 解析
│   │   ├── validator.rs     # CGEF 格式校验
│   │   ├── schema.rs        # 自定义节点/边类型注册
│   │   └── path_mapper.rs   # 路径前缀映射
│   ├── server/              # HTTP 服务器（feature: serve）
│   │   ├── mod.rs           # 服务启动（tokio + axum）
│   │   ├── handlers.rs      # API 路由处理器
│   │   ├── state.rs         # 应用状态（Project + Store 共享）
│   │   ├── assets.rs        # 嵌入式浏览器 UI（rust-embed）
│   │   └── access_log.rs    # HTTP 访问日志
│   ├── tui/                 # 终端 UI（feature: tui）
│   │   ├── mod.rs           # TUI 入口
│   │   ├── app.rs           # 应用逻辑
│   │   └── theme.rs         # 颜色主题
│   └── project/             # 项目管理
│       ├── mod.rs           # 项目生命周期（init, analyze, diff）
│       └── config.rs        # codeweb.toml 配置解析
├── docs/                    # 文档
│   ├── user-guide.md        # 用户手册
│   ├── serve-api-guide.md   # HTTP API 参考
│   ├── cgef-user-guide.md   # CGEF 格式指南
│   ├── op2cgef-guide.md     # OP 血缘转 CGEF 指南
│   ├── DeveloperGuide.md    # 开发指南
│   └── plans/               # 实现计划
├── tests/                   # 集成测试
├── locales/                 # 国际化翻译文件
├── Cargo.toml               # 项目配置
├── AGENTS.md                # AI 开发代理指南
└── CONTRIBUTION.md          # 本文件
```

### 模块依赖关系

```
main.rs (CLI)
  ├── project/ ───── 项目管理
  │     └── parser/ ─ 源码解析
  ├── graph/ ─────── 图模型（核心）
  │     ├── store.rs ── 持久化、索引
  │     ├── builder.rs ─ 图构建
  │     └── query/ ──── 声明式查询
  ├── export/ ────── 导出（DOT/JSON/Mermaid）
  ├── import/ ────── CGEF 导入
  ├── server/ ────── HTTP 服务（feature: serve）
  └── tui/ ───────── 终端 UI（feature: tui）
```

---

## 代码规范

### Rust 风格

- 遵循 `cargo fmt` 默认格式
- 通过 `cargo clippy -- -D warnings` 无警告
- 使用 `thiserror` 定义错误类型，不在库代码中使用 `anyhow`
- 避免 `unwrap()`，优先使用 `?` 或 `.expect("说明")`
- 禁止使用 `as any`、`@ts-ignore` 等类型安全性规避手段
- `match` 表达式须穷尽所有分支

### 命名约定

- 文件名：`snake_case`（如 `graph_store.rs`）
- 类型/结构体：`PascalCase`（如 `GraphStore`）
- 函数/方法：`snake_case`（如 `build_graph`）
- 常量：`SCREAMING_SNAKE_CASE`（如 `MAX_DEPTH`）
- Feature flags：`kebab-case`（如 `search-sql-v2`）

### 模块组织

- 每个模块一个文件，复杂模块用目录 + `mod.rs`
- 公共 API 通过 `pub use` 重导出
- 内部实现标注 `pub(crate)` 而非 `pub`
- Feature-gated 代码使用 `#[cfg(feature = "...")]`

### 注释规范

- 公共类型和函数使用 `///` 文档注释
- 复杂逻辑使用 `//` 行注释说明意图
- 中文项目使用中文注释，英文变量名

---

## 开发流程

### 1. 选择 Issue

从 [Issues](https://github.com/c2j/cobweb/issues) 中选择一个任务，或创建新的 Issue 描述你要做的工作。

### 2. 创建分支

```bash
git checkout -b feature/your-feature-name
# 或
git checkout -b fix/your-bug-fix
```

分支命名：`feature/<描述>` 或 `fix/<描述>`，使用 `kebab-case`。

### 3. 开发

- 遵循 [测试驱动开发](https://en.wikipedia.org/wiki/Test-driven_development) 流程
- 小步提交，每个提交保持原子性
- 添加新节点/边类型时同步更新：
  - `src/graph/key.rs` 中的 `NodeKey`
  - `src/main.rs` 中的 `node_type_tag()` 函数
  - 相关统计、导出逻辑
  - 文档中的节点类型表

### 4. 提交前检查

```bash
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test
```

### 5. 推送与 PR

```bash
git push origin feature/your-feature-name
```

然后在 GitHub 上创建 Pull Request。

---

## 测试

### 测试层级

| 层级 | 位置 | 说明 |
|------|------|------|
| 单元测试 | `src/**/*.rs`（`#[cfg(test)]` 模块） | 测试单个函数/方法 |
| 集成测试 | `tests/*.rs` | 测试完整功能流程 |

### 运行测试

```bash
# 运行所有测试
cargo test

# 运行特定测试
cargo test test_name

# 运行集成测试（需要 serve feature）
cargo test --features serve

# 运行单个集成测试文件
cargo test --test integration_test_name
```

### 编写测试

- 新功能必须包含测试
- Bug 修复应先编写复现测试
- 使用 `tempfile` 创建临时目录/文件进行文件系统测试
- 测试函数命名：`test_<功能描述>`

---

## 提交规范

### 提交信息格式

```
<类型>: <简短描述>

<详细说明（可选）>
```

类型：

| 类型 | 说明 |
|------|------|
| `feat` | 新功能 |
| `fix` | Bug 修复 |
| `docs` | 文档更新 |
| `refactor` | 代码重构（不改变功能） |
| `perf` | 性能优化 |
| `test` | 测试相关 |
| `chore` | 构建/工具/依赖更新 |

### 示例

```
feat: 添加 search-sql-v2 feature，支持基于指纹的 SQL 搜索

- 新增 sql_fingerprint_index 索引
- 使用 blake3 对 SQL 归一化后生成指纹
- trace-sql 命令自动启用快速查找路径
```

---

## Pull Request 流程

1. **确保 CI 通过**：`cargo fmt --check`、`cargo clippy -- -D warnings`、`cargo test` 全部通过
2. **更新文档**：如涉及用户可见变更，更新相关文档（README、UserGuide 等）
3. **描述清晰**：PR 描述中说明改了什么、为什么改、如何验证
4. **关联 Issue**：在 PR 描述中使用 `Closes #123` 关联相关 Issue
5. **等待审查**：至少一位维护者审查通过后合并

---

## 文档贡献

文档文件位置：

| 文档 | 文件 | 面向读者 |
|------|------|---------|
| 用户手册 | `docs/user-guide.md` | 最终用户 |
| API 参考 | `docs/serve-api-guide.md` | API 使用者 |
| CGEF 指南 | `docs/cgef-user-guide.md` | CGEF 格式使用者 |
| OP 转 CGEF 指南 | `docs/op2cgef-guide.md` | 企业血缘数据转换者 |
| 开发指南 | `docs/DeveloperGuide.md` | MCP/API/扩展开发者 |
| 贡献指南 | `CONTRIBUTION.md`（本文件） | 合作开发者 |
| README | `README.md` | 所有访客 |

文档使用 Markdown 格式，中文撰写。

---

## 技术栈参考

| 组件 | 技术 |
|------|------|
| SQL 解析 | [ogsql-parser](https://github.com/c2j/ogsql-parser)（手写递归下降） |
| 图引擎 | [petgraph](https://crates.io/crates/petgraph) |
| Java 解析 | [tree-sitter-java](https://crates.io/crates/tree-sitter-java) |
| CLI | [clap](https://crates.io/crates/clap) |
| TUI | [ratatui](https://crates.io/crates/ratatui) + [crossterm](https://crates.io/crates/crossterm) |
| HTTP 服务器 | [axum](https://crates.io/crates/axum) + [tokio](https://crates.io/crates/tokio) |
| 错误处理 | [thiserror](https://crates.io/crates/thiserror) |
| 序列化 | [serde](https://crates.io/crates/serde) + [serde_json](https://crates.io/crates/serde_json) |
| 国际化 | [rust-i18n](https://crates.io/crates/rust-i18n) |
| 文件哈希 | [blake3](https://crates.io/crates/blake3) |
| 并发 | [rayon](https://crates.io/crates/rayon) |
