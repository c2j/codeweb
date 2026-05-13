#[cfg(feature = "serve")]
mod tests {
    use std::process::{Child, Command};
    use std::time::Duration;

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

    fn start_server(port: u16) -> Child {
        let child = Command::new(codeweb_bin())
            .args([
                "serve",
                "--project",
                env!("CARGO_MANIFEST_DIR"),
                "--addr",
                &format!("127.0.0.1:{}", port),
            ])
            .spawn()
            .expect("failed to start codeweb serve");

        std::thread::sleep(Duration::from_secs(2));
        child
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
        child.kill().ok();

        assert_eq!(status, 200);
        let json: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert!(json["edges"].is_number());
    }

    #[test]
    fn test_serve_nodes_endpoint() {
        let port = 19877;
        let mut child = start_server(port);

        let (status, body) = get(port, "/api/v1/nodes");
        child.kill().ok();

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
        child.kill().ok();

        assert_eq!(status, 200);
        assert!(body.contains("nodes"));
    }

    #[test]
    fn test_serve_export_dot() {
        let port = 19879;
        let mut child = start_server(port);

        let (status, _body) = get(port, "/api/v1/export?format=dot");
        child.kill().ok();

        assert_eq!(status, 200);
    }

    #[test]
    fn test_serve_index_html() {
        let port = 19880;
        let mut child = start_server(port);

        let (status, body) = get(port, "/");
        child.kill().ok();

        assert_eq!(status, 200);
        assert!(body.contains("codeweb"));
        assert!(body.contains("app.js"));
    }
}
