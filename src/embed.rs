//! 嵌入稳定接口（docs/embeded-codeweb.md §3.2）。
//!
//! 本模块是 codeweb 对 SpinDuck（以及任何宿主进程）暴露的唯一契约面：
//! 不直接暴露 `petgraph::DiGraph`、`GraphStore` 或带 `body_sql`/`columns` 的
//! 内部节点。实现按 TDD 逐个补齐：version / EmbeddedProject / DiffStatus /
//! ThinGraph。

/// codeweb crate 版本，替代宿主侧解析 CLI `--version` 输出。
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_expose_a_semver_version_string() {
        let v = version();
        let parts: Vec<&str> = v.split('.').collect();
        assert_eq!(parts.len(), 3, "semver 三段: {v}");
        assert!(
            parts
                .iter()
                .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit())),
            "{v}"
        );
    }
}
