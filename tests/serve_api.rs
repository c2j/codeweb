#[cfg(feature = "serve")]
mod tests {
    use std::process::{Child, Command};
    use std::sync::OnceLock;
    use std::time::Duration;

    use tempfile::TempDir;

    static PROJECT: OnceLock<ProjectFixture> = OnceLock::new();

    struct ProjectFixture {
        _tmp: TempDir,
        root: std::path::PathBuf,
    }

    fn codeweb_bin() -> std::path::PathBuf {
        let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
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

    fn project_root() -> &'static std::path::Path {
        let fixture = PROJECT
            .get_or_init(|| build_fixture().expect("failed to build serve_api fixture project"));
        &fixture.root
    }

    fn build_fixture() -> std::io::Result<ProjectFixture> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path().to_path_buf();

        let toml = "[project]\n\
                    name = \"serve-api-test\"\n\
                    \n\
                    [analysis]\n\
                    paths = [\"sql/\"]\n\
                    \n\
                    [store]\n\
                    path = \".codeweb/store.bincode\"\n\
                    format = \"bincode\"\n";
        std::fs::write(root.join("codeweb.toml"), toml)?;

        let fixture_sql = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/serve_demo/sample.sql");
        let sql_dir = root.join("sql");
        std::fs::create_dir_all(&sql_dir)?;
        std::fs::copy(&fixture_sql, sql_dir.join("sample.sql"))?;

        let output = Command::new(codeweb_bin())
            .arg("analyze")
            .current_dir(&root)
            .output()?;
        assert!(
            output.status.success(),
            "codeweb analyze failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            root.join(".codeweb/store.bincode").exists(),
            "store.bincode not produced by analyze"
        );

        Ok(ProjectFixture { _tmp: tmp, root })
    }

    fn start_server(port: u16) -> Child {
        let child = Command::new(codeweb_bin())
            .args([
                "serve",
                "--project",
                project_root().to_str().expect("utf-8 path"),
                "--addr",
                &format!("127.0.0.1:{}", port),
            ])
            .spawn()
            .expect("failed to start codeweb serve");

        std::thread::sleep(Duration::from_secs(2));
        child
    }

    fn stop_server(child: &mut Child) {
        let _ = child.kill();
        let _ = child.wait();
    }

    fn get(port: u16, path: &str) -> (u16, String) {
        let url = format!("http://127.0.0.1:{}{}", port, path);
        let resp = minreq::get(&url).send();
        match resp {
            Ok(r) => (
                r.status_code as u16,
                r.as_str().unwrap_or_default().to_string(),
            ),
            Err(e) => panic!("request failed: {}", e),
        }
    }

    #[test]
    fn test_serve_stats_endpoint() {
        let port = 19876;
        let mut child = start_server(port);

        let (status, body) = get(port, "/api/v1/stats");
        stop_server(&mut child);

        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(json["edges"].is_number());
    }

    #[test]
    fn test_serve_nodes_endpoint() {
        let port = 19877;
        let mut child = start_server(port);

        let (status, body) = get(port, "/api/v1/nodes");
        stop_server(&mut child);

        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(json["nodes"].is_array());
        assert!(json["total"].is_number());
        assert_eq!(json["limit"], 100);
        assert_eq!(json["offset"], 0);
    }

    #[test]
    fn test_serve_graph_endpoint() {
        let port = 19878;
        let mut child = start_server(port);

        let (status, body) = get(port, "/api/v1/graph");
        stop_server(&mut child);

        assert_eq!(status, 200);
        assert!(body.contains("nodes"));
    }

    #[test]
    fn test_serve_export_dot() {
        let port = 19879;
        let mut child = start_server(port);

        let (status, _body) = get(port, "/api/v1/export?format=dot");
        stop_server(&mut child);

        assert_eq!(status, 200);
    }

    #[test]
    fn test_serve_index_html() {
        let port = 19880;
        let mut child = start_server(port);

        let (status, body) = get(port, "/");
        stop_server(&mut child);

        assert_eq!(status, 200);
        assert!(body.contains("codeweb"));
        assert!(body.contains("app.js"));
    }

    #[test]
    fn test_serve_search_sql_endpoint() {
        let port = 19881;
        let mut child = start_server(port);

        let (status, body) = get(port, "/api/v1/nodes/search-sql?q=select");
        stop_server(&mut child);

        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(json["nodes"].is_array());
        assert!(json["total"].is_number());
    }

    #[test]
    fn test_serve_search_sql_empty_for_unrelated() {
        let port = 19882;
        let mut child = start_server(port);

        let (status, body) = get(
            port,
            "/api/v1/nodes/search-sql?q=DROP+TABLE+nonexistent_xyz",
        );
        stop_server(&mut child);

        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(json["total"], 0);
    }

    #[test]
    fn test_serve_trace_empty_match_returns_200() {
        let port = 19883;
        let mut child = start_server(port);

        let (status, body) = get(
            port,
            "/api/v1/trace?from=nonexistent_symbol_xyz_123",
        );
        stop_server(&mut child);

        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(json["target"].is_null());
        assert_eq!(json["callers"], serde_json::json!([]));
        assert_eq!(json["callees"], serde_json::json!([]));
        assert_eq!(json["caller_count"], 0);
        assert_eq!(json["callee_count"], 0);
    }
}
