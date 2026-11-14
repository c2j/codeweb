//! codeweb 库入口：图分析内核与 embed 稳定接口。CLI/TUI/serve/MCP 在 bin crate
//! （`src/main.rs`，`cli` feature 门控）。
//!
//! feature 纪律（见 Cargo.toml features 注释）：
//! - 任何可用形态的核心（ogsql-parser / petgraph / 扫描类依赖）无条件编译；
//! - `jsp` 门控 JSP 脚本块与 iBatis mapper 链；`java` 门控 tree-sitter Java 图；
//! - `analyze` 是嵌入形态标记（SpinDuck 引入方式），本身不裁剪核心。

pub mod embed;
pub mod error;
pub mod export;
pub mod graph;
pub mod import;
pub mod mark;
pub mod parse_log;
pub mod parser;
pub mod project;
pub mod sql_match;
