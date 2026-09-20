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
            stats["hint"].as_str().is_some_and(|h| h.contains("codeweb_init")),
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
