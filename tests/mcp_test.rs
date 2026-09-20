#[cfg(feature = "mcp")]
mod tests {
    use std::io::{BufRead, BufReader, Write};
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::time::Duration;
    use tempfile::TempDir;

    // ── Helper: find the built codeweb binary ──

    fn codeweb_bin() -> PathBuf {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
        let bin_name = if cfg!(windows) {
            "codeweb.exe"
        } else {
            "codeweb"
        };
        let entries = std::fs::read_dir(&base).unwrap_or_else(|_| panic!("no target dir"));
        for entry in entries.flatten() {
            let p = entry.path().join("debug").join(bin_name);
            if p.exists() {
                return p;
            }
        }
        base.join("debug").join(bin_name)
    }

    /// Create a minimal temporary project by running `codeweb init` in a temp dir.
    fn create_test_project() -> (TempDir, PathBuf) {
        let tmpdir = TempDir::new().expect("failed to create temp dir");
        let project_path = tmpdir.path().to_path_buf();

        let output = Command::new(codeweb_bin())
            .args(["init", "test", "--dir", "."])
            .current_dir(&project_path)
            .output()
            .expect("failed to run codeweb init");

        assert!(
            output.status.success(),
            "codeweb init failed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );

        (tmpdir, project_path)
    }

    /// Create a temp project pre-populated with the `serve_demo` SQL fixture (shared with
    /// `tests/serve_api.rs`) and analyzed, so MCP tools that need real graph data
    /// (`codeweb_column_analysis`, `codeweb_lineage`) have a procedure/table to query.
    fn create_analyzed_project() -> (TempDir, PathBuf) {
        let tmpdir = TempDir::new().expect("failed to create temp dir");
        let project_path = tmpdir.path().to_path_buf();

        let toml = "[project]\n\
                    name = \"mcp-test\"\n\
                    \n\
                    [analysis]\n\
                    paths = [\"sql/\"]\n\
                    \n\
                    [store]\n\
                    path = \".codeweb/store.bincode\"\n\
                    format = \"bincode\"\n";
        std::fs::write(project_path.join("codeweb.toml"), toml).expect("write codeweb.toml");

        let fixture_sql =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/serve_demo/sample.sql");
        let sql_dir = project_path.join("sql");
        std::fs::create_dir_all(&sql_dir).expect("create sql dir");
        std::fs::copy(&fixture_sql, sql_dir.join("sample.sql")).expect("copy fixture sql");

        let output = Command::new(codeweb_bin())
            .arg("analyze")
            .current_dir(&project_path)
            .output()
            .expect("failed to run codeweb analyze");
        assert!(
            output.status.success(),
            "codeweb analyze failed: stderr={}",
            String::from_utf8_lossy(&output.stderr)
        );

        (tmpdir, project_path)
    }

    // ── McpChild: manages a codeweb mcp subprocess ──

    struct McpChild {
        child: Child,
        reader: BufReader<std::process::ChildStdout>,
    }

    impl McpChild {
        fn start(project: &PathBuf) -> Self {
            let mut child = Command::new(codeweb_bin())
                .args(["mcp", "--project"])
                .arg(project)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap_or_else(|e| panic!("failed to spawn codeweb mcp: {e}"));

            let stdout = child.stdout.take().expect("no stdout");
            let reader = BufReader::new(stdout);

            // Give the server time to start the tokio runtime and load the project
            std::thread::sleep(Duration::from_millis(1000));

            Self { child, reader }
        }

        fn send(&mut self, json: &str) {
            let stdin = self.child.stdin.as_mut().expect("no stdin");
            writeln!(stdin, "{}", json).expect("failed to write to stdin");
            stdin.flush().expect("failed to flush stdin");
        }

        fn read_line(&mut self) -> Option<String> {
            let mut line = String::new();
            match self.reader.read_line(&mut line) {
                Ok(0) => None,
                Ok(_) => {
                    let trimmed = line.trim().to_string();
                    if trimmed.is_empty() {
                        None
                    } else {
                        Some(trimmed)
                    }
                }
                Err(e) => panic!("read error: {e}"),
            }
        }

        fn recv_response(&mut self, expected_id: i64) -> serde_json::Value {
            loop {
                let line = self.read_line().unwrap_or_else(|| {
                    panic!("stdout closed before receiving response for id {expected_id}")
                });

                let json: serde_json::Value = serde_json::from_str(&line)
                    .unwrap_or_else(|e| panic!("invalid JSON '{line}': {e}"));

                if let Some(id) = json.get("id") {
                    if id == expected_id {
                        return json;
                    }
                }
            }
        }
    }

    impl Drop for McpChild {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn handshake(mcp: &mut McpChild) {
        mcp.send(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}}}"#,
        );
        let resp = mcp.recv_response(1);

        assert_eq!(
            resp["result"]["serverInfo"]["name"], "codeweb",
            "serverInfo.name should be 'codeweb'"
        );
        assert!(
            resp["result"]["capabilities"]
                .get("tools")
                .is_some_and(|v| v.is_object()),
            "capabilities.tools should exist and be an object"
        );

        mcp.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
        std::thread::sleep(Duration::from_millis(200));
    }

    // ── Tests ──

    #[test]
    fn test_mcp_initialize() {
        let (_tmpdir, project) = create_test_project();
        let mut mcp = McpChild::start(&project);

        mcp.send(
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}}}"#,
        );
        let resp = mcp.recv_response(1);

        assert_eq!(
            resp["result"]["serverInfo"]["name"], "codeweb",
            "serverInfo.name should be 'codeweb'"
        );
        assert!(
            resp["result"]["capabilities"]
                .get("tools")
                .is_some_and(|v| v.is_object()),
            "capabilities.tools should exist and be an object"
        );
    }

    #[test]
    fn test_mcp_tools_list() {
        let (_tmpdir, project) = create_test_project();
        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        mcp.send(r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#);
        let resp = mcp.recv_response(2);

        let tools = resp["result"]["tools"]
            .as_array()
            .expect("result.tools should be an array");
        let tool_names: Vec<&str> = tools
            .iter()
            .map(|t| t["name"].as_str().expect("tool name should be a string"))
            .collect();

        let expected = [
            "codeweb_init",
            "codeweb_analyze",
            "codeweb_diff",
            "codeweb_stats",
            "codeweb_nodes",
            "codeweb_node_detail",
            "codeweb_trace",
            "codeweb_search_sql",
            "codeweb_query",
            "codeweb_column_analysis",
            "codeweb_lineage",
        ];

        for name in &expected {
            assert!(
                tool_names.contains(name),
                "tools list should contain '{name}'"
            );
        }

        assert_eq!(
            tool_names.len(),
            expected.len(),
            "should have exactly {} tools",
            expected.len()
        );
    }

    #[test]
    fn test_mcp_uninitialized_project_stays_alive() {
        // A directory with no codeweb.toml anywhere above it: the server used to
        // exit before answering `initialize`. It must now stay up and report the
        // project as uninitialized instead of dying.
        let tmpdir = TempDir::new().expect("failed to create temp dir");
        let project = tmpdir.path().to_path_buf();
        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        mcp.send(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"codeweb_stats","arguments":{}}}"#,
        );
        let resp = mcp.recv_response(2);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("stats text");
        let stats: serde_json::Value = serde_json::from_str(text).expect("stats JSON");

        assert_eq!(
            stats["status"], "uninitialized",
            "an uninitialized project must report status=uninitialized, got: {stats}"
        );
        assert!(
            stats["hint"]
                .as_str()
                .is_some_and(|h| h.contains("codeweb_init")),
            "hint should point at codeweb_init, got: {stats}"
        );
    }

    #[test]
    fn test_mcp_init_creates_project_without_auto_analyze() {
        let tmpdir = TempDir::new().expect("failed to create temp dir");
        let project = tmpdir.path().to_path_buf();
        std::fs::create_dir_all(project.join("sql")).expect("create sql dir");
        std::fs::write(project.join("sql").join("a.sql"), "SELECT 1;").expect("write sql");

        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        mcp.send(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"codeweb_init","arguments":{"name":"demo","paths":["sql"]}}}"#,
        );
        let resp = mcp.recv_response(2);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("init text");
        let init: serde_json::Value = serde_json::from_str(text).expect("init JSON");

        assert_eq!(init["status"], "initialized", "got: {init}");
        assert_eq!(init["project"], "demo", "got: {init}");
        assert!(
            project.join("codeweb.toml").exists(),
            "codeweb_init must write codeweb.toml under the served directory"
        );

        // `codeweb_init` must not analyze: the graph stays empty until the caller
        // explicitly asks for `codeweb_analyze`.
        mcp.send(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"codeweb_stats","arguments":{}}}"#,
        );
        let resp = mcp.recv_response(3);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("stats text");
        let stats: serde_json::Value = serde_json::from_str(text).expect("stats JSON");

        assert_eq!(
            stats["status"], "empty",
            "after init but before analyze the graph must be empty (not uninitialized), got: {stats}"
        );
    }

    #[test]
    fn test_mcp_init_is_idempotent_error() {
        let (_tmpdir, project) = create_test_project();
        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        mcp.send(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"codeweb_init","arguments":{"name":"again"}}}"#,
        );
        let resp = mcp.recv_response(2);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("init text");
        let init: serde_json::Value = serde_json::from_str(text).expect("init JSON");

        assert_eq!(
            init["status"], "already_initialized",
            "re-initializing an existing project must be reported, got: {init}"
        );
    }

    /// Shared with `create_analyzed_project`: a small SQL fixture with procedures/tables.
    fn copy_serve_demo_fixture(project: &std::path::Path) {
        let fixture_sql =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/serve_demo/sample.sql");
        let sql_dir = project.join("sql");
        std::fs::create_dir_all(&sql_dir).expect("create sql dir");
        std::fs::copy(&fixture_sql, sql_dir.join("sample.sql")).expect("copy fixture sql");
    }

    #[test]
    fn test_mcp_analyze_builds_graph_and_hot_swaps() {
        let tmpdir = TempDir::new().expect("failed to create temp dir");
        let project = tmpdir.path().to_path_buf();
        copy_serve_demo_fixture(&project);

        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        // Before init, analyze must guide the caller to codeweb_init.
        mcp.send(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"codeweb_analyze","arguments":{}}}"#,
        );
        let resp = mcp.recv_response(2);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("analyze text");
        let before: serde_json::Value = serde_json::from_str(text).expect("analyze JSON");
        assert_eq!(
            before["status"], "uninitialized",
            "analyze on an uninitialized project must guide to codeweb_init, got: {before}"
        );

        mcp.send(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"codeweb_init","arguments":{"name":"mcp-analyze","paths":["sql"]}}}"#,
        );
        let _ = mcp.recv_response(3);

        mcp.send(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"codeweb_analyze","arguments":{}}}"#,
        );
        let resp = mcp.recv_response(4);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("analyze text");
        let analyzed: serde_json::Value = serde_json::from_str(text).expect("analyze JSON");

        assert_eq!(analyzed["status"], "ready", "got: {analyzed}");
        assert!(
            analyzed["nodes"].as_u64().unwrap_or(0) > 0,
            "analyze must build a non-empty graph, got: {analyzed}"
        );
        assert!(
            project.join(".codeweb").join("store.bincode").exists(),
            "analyze must persist the store under the served directory"
        );

        // Hot swap: the very next query sees the new graph, no restart.
        mcp.send(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"codeweb_stats","arguments":{}}}"#,
        );
        let resp = mcp.recv_response(5);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("stats text");
        let stats: serde_json::Value = serde_json::from_str(text).expect("stats JSON");

        assert_eq!(stats["status"], "ready", "got: {stats}");
        assert_eq!(
            stats["edges"].as_u64(),
            analyzed["edges"].as_u64(),
            "stats must reflect the freshly built graph without a server restart"
        );
    }

    #[test]
    fn test_mcp_analyze_rejects_store_path_escaping_root() {
        // `store.path` is user-controlled config: a tampered value must not be
        // able to redirect the store write outside the served directory.
        let tmpdir = TempDir::new().expect("failed to create temp dir");
        let project = tmpdir.path().join("proj");
        copy_serve_demo_fixture(&project);

        let escaped_store = tmpdir.path().join("outside.bincode");
        let toml = "[project]\n\
                    name = \"escape\"\n\
                    \n\
                    [analysis]\n\
                    paths = [\"sql\"]\n\
                    \n\
                    [store]\n\
                    path = \"../outside.bincode\"\n\
                    format = \"bincode\"\n";
        std::fs::write(project.join("codeweb.toml"), toml).expect("write codeweb.toml");

        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        mcp.send(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"codeweb_analyze","arguments":{}}}"#,
        );
        let resp = mcp.recv_response(2);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("analyze text");
        let result: serde_json::Value = serde_json::from_str(text).expect("analyze JSON");

        assert_eq!(
            result["status"], "error",
            "an escaping store.path must be refused, got: {result}"
        );
        assert!(
            !escaped_store.exists(),
            "analysis must not write outside the served directory"
        );
    }

    #[test]
    fn test_mcp_analyze_refreshes_already_analyzed_project() {
        // The CLI analyzed this project before the server started, so
        // `Project::analyze` takes its up-to-date short-circuit and leaves the
        // store unloaded. The tool must still report the real graph (not the
        // short-circuit's zero counts) and keep it queryable.
        let (_tmpdir, project) = create_analyzed_project();
        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        mcp.send(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"codeweb_analyze","arguments":{}}}"#,
        );
        let resp = mcp.recv_response(2);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("analyze text");
        let analyzed: serde_json::Value = serde_json::from_str(text).expect("analyze JSON");

        assert_eq!(analyzed["status"], "ready", "got: {analyzed}");
        assert_eq!(
            analyzed["is_up_to_date"], true,
            "an unchanged, already-analyzed project must report is_up_to_date, got: {analyzed}"
        );
        assert!(
            analyzed["nodes"].as_u64().unwrap_or(0) > 0
                && analyzed["edges"].as_u64().unwrap_or(0) > 0,
            "the up-to-date path must still report the real graph, got: {analyzed}"
        );

        // The graph stays queryable after the refresh.
        mcp.send(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"codeweb_stats","arguments":{}}}"#,
        );
        let resp = mcp.recv_response(3);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("stats text");
        let stats: serde_json::Value = serde_json::from_str(text).expect("stats JSON");

        assert_eq!(stats["status"], "ready", "got: {stats}");
        assert_eq!(
            stats["edges"].as_u64(),
            analyzed["edges"].as_u64(),
            "stats must agree with the refresh report"
        );
    }

    #[test]
    fn test_mcp_diff_reports_changes_since_last_analyze() {
        let (_tmpdir, project) = create_analyzed_project();
        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        // Right after analysis there is nothing to report.
        mcp.send(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"codeweb_diff","arguments":{}}}"#,
        );
        let resp = mcp.recv_response(2);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("diff text");
        let clean: serde_json::Value = serde_json::from_str(text).expect("diff JSON");
        assert_eq!(clean["status"], "up_to_date", "got: {clean}");
        assert_eq!(
            clean["added"].as_array().map(Vec::len),
            Some(0),
            "got: {clean}"
        );

        // A new source file must show up as added.
        std::fs::write(project.join("sql").join("brand_new.sql"), "SELECT 42;")
            .expect("write new sql file");

        mcp.send(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"codeweb_diff","arguments":{}}}"#,
        );
        let resp = mcp.recv_response(3);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("diff text");
        let changed: serde_json::Value = serde_json::from_str(text).expect("diff JSON");

        assert_eq!(changed["status"], "changed", "got: {changed}");
        let added = changed["added"].as_array().expect("added array");
        assert!(
            added
                .iter()
                .any(|p| p.as_str().is_some_and(|s| s.contains("brand_new.sql"))),
            "added must contain brand_new.sql, got: {changed}"
        );
    }

    #[test]
    fn test_mcp_analyze_heals_corrupt_store() {
        // A store left behind by a crashed/older binary must not brick the server:
        // queries report the problem, and codeweb_analyze rebuilds from scratch.
        let (_tmpdir, project) = create_analyzed_project();
        let store_path = project.join(".codeweb").join("store.bincode");
        assert!(store_path.exists(), "fixture should have produced a store");
        std::fs::write(&store_path, b"not a valid codeweb store").expect("corrupt the store");

        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        mcp.send(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"codeweb_stats","arguments":{}}}"#,
        );
        let resp = mcp.recv_response(2);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("stats text");
        let stats: serde_json::Value = serde_json::from_str(text).expect("stats JSON");

        assert_eq!(
            stats["status"], "empty",
            "a corrupt store is an initialized-but-empty graph, not uninitialized, got: {stats}"
        );
        assert!(
            stats["message"]
                .as_str()
                .is_some_and(|m| m.contains("could not be loaded")),
            "the corrupt store must be reported explicitly, got: {stats}"
        );

        mcp.send(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"codeweb_analyze","arguments":{}}}"#,
        );
        let resp = mcp.recv_response(3);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("analyze text");
        let analyzed: serde_json::Value = serde_json::from_str(text).expect("analyze JSON");

        assert_eq!(analyzed["status"], "ready", "got: {analyzed}");
        assert!(
            analyzed["nodes"].as_u64().unwrap_or(0) > 0,
            "analyze must rebuild a corrupted store, got: {analyzed}"
        );

        mcp.send(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"codeweb_stats","arguments":{}}}"#,
        );
        let resp = mcp.recv_response(4);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("stats text");
        let healed: serde_json::Value = serde_json::from_str(text).expect("stats JSON");
        assert_eq!(healed["status"], "ready", "got: {healed}");
    }

    #[test]
    fn test_mcp_handles_pipelined_requests() {
        // Requests are written back-to-back without waiting. Two analyzes in a
        // row plus a read must all answer (no deadlock between the project mutex
        // and the graph lock) and agree on the final state.
        let (_tmpdir, project) = create_analyzed_project();
        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        mcp.send(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"codeweb_analyze","arguments":{}}}"#,
        );
        mcp.send(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"codeweb_analyze","arguments":{}}}"#,
        );
        mcp.send(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"codeweb_stats","arguments":{}}}"#,
        );

        let mut seen = std::collections::BTreeMap::new();
        // Notifications may be interleaved; keep reading until all three ids land.
        for _ in 0..12 {
            if seen.len() == 3 {
                break;
            }
            let line = mcp
                .read_line()
                .expect("stdout closed while handling pipelined requests");
            let json: serde_json::Value = serde_json::from_str(&line).expect("valid JSON");
            if let Some(id) = json["id"].as_i64() {
                seen.insert(id, json);
            }
        }

        for id in [2, 3, 4] {
            let resp = seen
                .get(&id)
                .unwrap_or_else(|| panic!("no response for id {id}: {seen:?}"));
            let text = resp["result"]["content"][0]["text"]
                .as_str()
                .expect("tool response text");
            let body: serde_json::Value = serde_json::from_str(text).expect("tool response JSON");
            assert_eq!(body["status"], "ready", "id {id} returned: {body}");
            assert!(
                body["edges"].as_u64().unwrap_or(0) > 0,
                "id {id} returned an empty graph: {body}"
            );
        }
    }

    #[test]
    fn test_mcp_init_creates_missing_project_directory() {
        // `--project` may point at a directory that does not exist yet.
        let tmpdir = TempDir::new().expect("failed to create temp dir");
        let project = tmpdir.path().join("new").join("nested");
        assert!(!project.exists());

        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        mcp.send(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"codeweb_init","arguments":{"name":"fresh","paths":["."]}}}"#,
        );
        let resp = mcp.recv_response(2);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("init text");
        let init: serde_json::Value = serde_json::from_str(text).expect("init JSON");

        assert_eq!(init["status"], "initialized", "got: {init}");
        assert!(
            project.join("codeweb.toml").exists(),
            "init must create the served directory and its config"
        );
    }

    #[test]
    fn test_mcp_analyze_rejects_symlinked_store_dir() {
        // Lexical path checks cannot see symlinks: `.codeweb` looks like it is
        // inside the project, but it points outside. The write guard must still
        // refuse, and nothing may be written into the symlink target.
        let tmpdir = TempDir::new().expect("failed to create temp dir");
        let project = tmpdir.path().join("proj");
        let outside = tmpdir.path().join("outside");
        std::fs::create_dir_all(&project).expect("create project dir");
        std::fs::create_dir_all(&outside).expect("create outside dir");
        copy_serve_demo_fixture(&project);

        let toml = "[project]\n\
                    name = \"symlink\"\n\
                    \n\
                    [analysis]\n\
                    paths = [\"sql\"]\n\
                    \n\
                    [store]\n\
                    path = \".codeweb/store.bincode\"\n\
                    format = \"bincode\"\n";
        std::fs::write(project.join("codeweb.toml"), toml).expect("write codeweb.toml");
        std::os::unix::fs::symlink(&outside, project.join(".codeweb")).expect("symlink .codeweb");

        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        mcp.send(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"codeweb_analyze","arguments":{}}}"#,
        );
        let resp = mcp.recv_response(2);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("analyze text");
        let result: serde_json::Value = serde_json::from_str(text).expect("analyze JSON");

        assert_eq!(
            result["status"], "error",
            "a symlinked store directory must be refused, got: {result}"
        );
        assert!(
            result["error"]
                .as_str()
                .is_some_and(|e| e.contains("outside")),
            "the error should say the path resolves outside the root, got: {result}"
        );
        let leaked: Vec<String> = std::fs::read_dir(&outside)
            .expect("read outside dir")
            .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().to_string()))
            .collect();
        assert!(
            leaked.is_empty(),
            "nothing may be written through the symlink, found: {leaked:?}"
        );
    }

    #[test]
    fn test_mcp_call_stats() {
        let (_tmpdir, project) = create_test_project();
        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        mcp.send(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"codeweb_stats","arguments":{}}}"#,
        );
        let resp = mcp.recv_response(3);

        let content = resp["result"]["content"]
            .as_array()
            .expect("result.content should be an array");
        assert!(!content.is_empty(), "content should not be empty");

        let text = content[0]["text"]
            .as_str()
            .expect("content[0].text should be a string");

        let stats: serde_json::Value = serde_json::from_str(text)
            .unwrap_or_else(|e| panic!("stats text should be valid JSON: {e}"));

        // New project without `codeweb analyze` → graph is empty.
        // The MCP server stays alive and returns status="empty" with guidance.
        assert_eq!(
            stats["status"], "empty",
            "unanalyzed project should return status=empty, got: {stats}"
        );
        assert!(
            stats.get("hint").is_some(),
            "empty stats should include a hint, got: {stats}"
        );
    }

    #[test]
    fn test_mcp_call_column_analysis() {
        let (_tmpdir, project) = create_analyzed_project();
        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        mcp.send(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"codeweb_column_analysis","arguments":{"procedure":"p_demo_query"}}}"#,
        );
        let resp = mcp.recv_response(4);

        let content = resp["result"]["content"]
            .as_array()
            .expect("result.content should be an array");
        assert!(!content.is_empty(), "content should not be empty");

        let text = content[0]["text"]
            .as_str()
            .expect("content[0].text should be a string");
        let analysis: serde_json::Value = serde_json::from_str(text)
            .unwrap_or_else(|e| panic!("column_analysis text should be valid JSON: {e}"));

        assert_eq!(
            analysis["schema_version"], 1,
            "expected schema_version 1, got: {analysis}"
        );
        assert_eq!(
            analysis["procedure"], "p_demo_query",
            "expected procedure field to echo the resolved name, got: {analysis}"
        );
        assert!(
            analysis.get("hard_filters").is_some_and(|v| v.is_array()),
            "expected hard_filters array field, got: {analysis}"
        );
    }

    #[test]
    fn test_mcp_column_analysis_reports_ambiguous_match() {
        let (_tmpdir, project) = create_analyzed_project();
        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        mcp.send(
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"codeweb_column_analysis","arguments":{"procedure":"p_demo"}}}"#,
        );
        let resp = mcp.recv_response(6);
        let text = resp["result"]["content"][0]["text"]
            .as_str()
            .expect("tool response text");
        let error: serde_json::Value = serde_json::from_str(text).expect("error JSON");

        assert_eq!(error["error"], "Ambiguous match: 2 candidates for 'p_demo'");
    }

    #[test]
    fn test_mcp_call_lineage() {
        let (_tmpdir, project) = create_analyzed_project();
        let mut mcp = McpChild::start(&project);
        handshake(&mut mcp);

        mcp.send(
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"codeweb_lineage","arguments":{"target":"t_users"}}}"#,
        );
        let resp = mcp.recv_response(5);

        let content = resp["result"]["content"]
            .as_array()
            .expect("result.content should be an array");
        assert!(!content.is_empty(), "content should not be empty");

        let text = content[0]["text"]
            .as_str()
            .expect("content[0].text should be a string");
        let lineage: serde_json::Value = serde_json::from_str(text)
            .unwrap_or_else(|e| panic!("lineage text should be valid JSON: {e}"));

        assert!(
            lineage.get("upstream").is_some() && lineage.get("downstream").is_some(),
            "table-level lineage with default direction=both should have upstream+downstream keys, got: {lineage}"
        );
    }
}
