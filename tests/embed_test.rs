//! embedded-codeweb Phase 2 / PR-C 的契约测试（docs/plans/2026-11-14-embedded-codeweb.md
//! T2.7.4 / T2.7.5）。全部走进程内 lib API——这正是嵌入形态的消费方式，不再经 CLI 子进程。
//!
//! fixture 口径：与 tests/mcp_test.rs 相同的最小项目（codeweb.toml + serve_demo SQL），
//! 但用 `Project::init_at` 的等价 TOML 直接落盘、进程内 `EmbeddedProject` 驱动。

#[cfg(feature = "analyze")]
mod tests {
    use std::path::PathBuf;
    use tempfile::TempDir;

    const PROJECT_TOML: &str = "[project]\n\
         name = \"embed-test\"\n\
         \n\
         [analysis]\n\
         paths = [\"sql/\"]\n\
         \n\
         [store]\n\
         path = \".codeweb/store.bincode\"\n\
         format = \"bincode\"\n";

    /// 最小项目：codeweb.toml + sql/sample.sql（serve_demo fixture）。
    /// 返回 (临时目录, 项目根)。
    fn create_project() -> (TempDir, PathBuf) {
        let tmpdir = TempDir::new().expect("failed to create temp dir");
        let root = tmpdir.path().to_path_buf();
        std::fs::write(root.join("codeweb.toml"), PROJECT_TOML).expect("write codeweb.toml");
        let sql_dir = root.join("sql");
        std::fs::create_dir_all(&sql_dir).expect("create sql dir");
        let fixture_sql =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/serve_demo/sample.sql");
        std::fs::copy(&fixture_sql, sql_dir.join("sample.sql")).expect("copy fixture sql");
        (tmpdir, root)
    }

    // ── T2.7.4：EmbeddedProject + DiffStatus ─────────────────────────────

    #[test]
    fn open_exact_reports_store_exists_false_for_a_fresh_project() {
        let (_tmp, root) = create_project();
        let proj = codeweb::embed::EmbeddedProject::open_exact(&root).expect("open_exact");
        let diff = proj.diff().expect("diff");
        assert!(
            !diff.store_exists,
            "fresh project has no store yet: {diff:?}"
        );
        assert!(
            !diff.is_up_to_date(),
            "fresh project cannot be up to date: {diff:?}"
        );
    }

    #[test]
    fn analyze_makes_diff_report_up_to_date() {
        let (_tmp, root) = create_project();
        let mut proj = codeweb::embed::EmbeddedProject::open_exact(&root).expect("open_exact");
        proj.analyze().expect("analyze");
        let diff = proj.diff().expect("diff");
        assert!(diff.store_exists, "store written by analyze: {diff:?}");
        assert!(
            diff.is_up_to_date(),
            "just-analyzed project must be up to date: {diff:?}"
        );
        assert!(
            diff.added.is_empty() && diff.modified.is_empty() && diff.deleted.is_empty(),
            "no changes right after analyze: {diff:?}"
        );
    }

    #[test]
    fn modifying_a_source_file_falls_out_of_date_with_modified_listed() {
        let (_tmp, root) = create_project();
        let mut proj = codeweb::embed::EmbeddedProject::open_exact(&root).expect("open_exact");
        proj.analyze().expect("analyze");

        let sql_file = root.join("sql/sample.sql");
        let mut sql = std::fs::read_to_string(&sql_file).unwrap();
        sql.push_str("\n-- touch\n");
        std::fs::write(&sql_file, sql).unwrap();

        let diff = proj.diff().expect("diff");
        assert!(
            !diff.is_up_to_date(),
            "modified source must fall out of date: {diff:?}"
        );
        assert!(
            diff.modified.iter().any(|p| p.ends_with("sql/sample.sql")),
            "modified file must be listed: {diff:?}"
        );
        assert!(
            diff.added.is_empty() && diff.deleted.is_empty(),
            "only modification expected: {diff:?}"
        );
    }

    #[test]
    fn deleting_the_store_reports_store_exists_false_again() {
        let (_tmp, root) = create_project();
        let mut proj = codeweb::embed::EmbeddedProject::open_exact(&root).expect("open_exact");
        proj.analyze().expect("analyze");
        std::fs::remove_file(root.join(".codeweb/store.bincode")).unwrap();

        let diff = proj.diff().expect("diff");
        assert!(
            !diff.store_exists,
            "deleted store must be reported: {diff:?}"
        );
        assert!(!diff.is_up_to_date());
    }

    #[test]
    fn stale_store_version_is_reported_from_the_store_header() {
        let (_tmp, root) = create_project();
        let mut proj = codeweb::embed::EmbeddedProject::open_exact(&root).expect("open_exact");
        proj.analyze().expect("analyze");

        // 把刚写好的 store 的版本头（字节 9..=12，紧随 9 字节 MAGIC）改成旧版本号 7，
        // 精确模拟「store 存在但版本过期」。
        let store_path = root.join(".codeweb/store.bincode");
        let mut bytes = std::fs::read(&store_path).expect("store file exists");
        assert!(
            bytes.len() >= 13,
            "bincode store must have the 13-byte header"
        );
        bytes[9..13].copy_from_slice(&7u32.to_le_bytes());
        std::fs::write(&store_path, &bytes).unwrap();

        let diff = proj.diff().expect("diff");
        assert_eq!(
            diff.stale_store_version,
            Some(7),
            "stale store version must be surfaced: {diff:?}"
        );
        assert!(!diff.is_up_to_date());
    }

    #[test]
    fn open_exact_rejects_a_directory_without_codeweb_toml() {
        let tmpdir = TempDir::new().unwrap();
        let err = codeweb::embed::EmbeddedProject::open_exact(tmpdir.path()).unwrap_err();
        assert!(
            matches!(err, codeweb::error::CodeWebError::ProjectNotFound { .. }),
            "expected ProjectNotFound, got: {err:?}"
        );
    }

    #[test]
    fn find_walks_up_to_the_nearest_codeweb_toml() {
        let (_tmp, root) = create_project();
        let nested = root.join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        let proj = codeweb::embed::EmbeddedProject::find(&nested).expect("find");
        assert_eq!(
            proj.root(),
            root.as_path(),
            "must settle on the project root"
        );
        assert_eq!(proj.name(), "embed-test");
    }

    #[test]
    fn version_exposes_the_crate_semver() {
        let v = codeweb::embed::version();
        assert_eq!(v.split('.').count(), 3, "semver 三段: {v}");
    }

    // ── T2.7.5：export_thin_graph + 等价断言（最小形态：锚定可观察面） ────

    #[test]
    fn export_thin_graph_keeps_only_sql_plsql_relevant_nodes() {
        let (_tmp, root) = create_project();
        let mut proj = codeweb::embed::EmbeddedProject::open_exact(&root).expect("open_exact");
        proj.analyze().expect("analyze");

        let thin = proj.export_thin_graph().expect("export_thin_graph");
        let kinds: Vec<&str> = thin.nodes.iter().map(|n| n.kind.tag()).collect();
        for banned in [
            "java_class",
            "java_method",
            "java_sql",
            "mapped_statement",
            "jsp",
        ] {
            assert!(
                !kinds.contains(&banned),
                "thin graph must drop {banned} nodes, got: {kinds:?}"
            );
        }
        assert!(
            kinds.iter().any(|k| *k == "procedure"),
            "serve_demo has procedures: {kinds:?}"
        );
        assert!(
            kinds.iter().any(|k| *k == "table"),
            "serve_demo references tables: {kinds:?}"
        );
    }

    #[test]
    fn export_thin_graph_matches_the_json_projection_one_to_one() {
        let (_tmp, root) = create_project();
        let mut proj = codeweb::embed::EmbeddedProject::open_exact(&root).expect("open_exact");
        proj.analyze().expect("analyze");

        // 参照面：现有 JSON 导出（CLI `export --format json` 同一条 to_json 路径）。
        let store = proj
            .store()
            .cloned()
            .expect("analyze must have left the store loaded");
        let json_text = codeweb::export::json::to_json(store.graph()).expect("to_json");
        let json: serde_json::Value = serde_json::from_str(&json_text).unwrap();

        let thin = proj.export_thin_graph().expect("export_thin_graph");

        // JSON 侧的"保留类节点"集合：type ∈ procedure/function/package/table/
        // view/materialized_view/sequence/unresolved/builtin_function。
        const KEPT: [&str; 9] = [
            "procedure",
            "function",
            "package",
            "table",
            "view",
            "materialized_view",
            "sequence",
            "unresolved",
            "builtin_function",
        ];
        let kept_ids: std::collections::HashSet<usize> = json["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| KEPT.contains(&n["type"].as_str().unwrap_or("")))
            .map(|n| n["id"].as_u64().unwrap() as usize)
            .collect();

        // 1) 节点集合 (id, tag) 1:1。
        let mut json_kept: Vec<(usize, &str)> = json["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| kept_ids.contains(&(n["id"].as_u64().unwrap() as usize)))
            .map(|n| {
                (
                    n["id"].as_u64().unwrap() as usize,
                    n["type"].as_str().unwrap(),
                )
            })
            .collect();
        json_kept.sort();
        let mut thin_nodes: Vec<(usize, &str)> =
            thin.nodes.iter().map(|n| (n.id, n.kind.tag())).collect();
        thin_nodes.sort();
        assert_eq!(
            json_kept, thin_nodes,
            "kept node (id, tag) sets must match 1:1"
        );

        // 2) 边集合 (source, target, type) 1:1——仅 7 类保留边，且两端都在保留集。
        const KEPT_EDGES: [&str; 7] = [
            "direct",
            "dynamic",
            "table_access",
            "depends_on",
            "uses_sequence",
            "anchors_on",
            "contains_routine",
        ];
        let mut json_edges: Vec<(usize, usize, &str)> = json["edges"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| {
                let s = e["source"].as_u64().unwrap() as usize;
                let t = e["target"].as_u64().unwrap() as usize;
                kept_ids.contains(&s)
                    && kept_ids.contains(&t)
                    && KEPT_EDGES.contains(&e["type"].as_str().unwrap_or(""))
            })
            .map(|e| {
                (
                    e["source"].as_u64().unwrap() as usize,
                    e["target"].as_u64().unwrap() as usize,
                    e["type"].as_str().unwrap(),
                )
            })
            .collect();
        json_edges.sort();
        let mut thin_edges: Vec<(usize, usize, &str)> = thin
            .edges
            .iter()
            .map(|e| (e.source, e.target, e.kind.tag()))
            .collect();
        thin_edges.sort();
        assert_eq!(
            json_edges, thin_edges,
            "kept edge (source, target, type) sets must match 1:1"
        );
    }
}
