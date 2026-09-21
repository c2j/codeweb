//! Issue #175: the nondeterminism was reported as entry-point independent
//! ("CLI `codeweb analyze` 与 MCP `codeweb_analyze` 都会出现"). The fix lives in
//! the shared `Project::analyze`, so this pins the strongest form of that claim:
//! the graph produced through the MCP entry point must be byte-identical to the
//! one produced through the CLI entry point for the same source.
//!
//! Only compiled with `--features mcp` (the repo's `full` gate).

#![cfg(feature = "mcp")]

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;

fn codeweb_bin() -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) {
        "codeweb.exe"
    } else {
        "codeweb"
    };
    if let Ok(entries) = std::fs::read_dir(&base) {
        for entry in entries.flatten() {
            let p = entry.path().join("debug").join(bin_name);
            if p.exists() {
                return p;
            }
        }
    }
    base.join("debug").join(bin_name)
}

fn write_project(root: &Path, name: &str, source: &Path) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        root.join("codeweb.toml"),
        format!(
            "[project]\nname = \"{name}\"\n\n[analysis]\npaths = [\"{}\"]\n\n\
             [store]\npath = \".codeweb/store.bincode\"\nformat = \"bincode\"\n",
            source.display()
        ),
    )
    .unwrap();
}

fn export_ndjson(root: &Path) -> String {
    let out = Command::new(codeweb_bin())
        .current_dir(root)
        .args(["export", "--format", "ndjson", "-p", "."])
        .output()
        .expect("failed to run codeweb export");
    assert!(
        out.status.success(),
        "export failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

struct McpChild {
    child: Child,
    reader: BufReader<std::process::ChildStdout>,
}

impl McpChild {
    fn start(project: &Path) -> Self {
        let mut child = Command::new(codeweb_bin())
            .args(["mcp", "--project"])
            .arg(project)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("failed to spawn codeweb mcp");
        let stdout = child.stdout.take().expect("no stdout");
        // Let the tokio runtime start and load the project.
        std::thread::sleep(Duration::from_millis(1000));
        Self {
            child,
            reader: BufReader::new(stdout),
        }
    }

    fn send(&mut self, json: &str) {
        let stdin = self.child.stdin.as_mut().expect("no stdin");
        writeln!(stdin, "{json}").expect("write to stdin");
        stdin.flush().expect("flush stdin");
    }

    fn recv(&mut self, id: i64) -> serde_json::Value {
        loop {
            let mut line = String::new();
            let read = self.reader.read_line(&mut line).expect("read stdout");
            assert_ne!(read, 0, "stdout closed before response id {id}");
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let json: serde_json::Value = serde_json::from_str(trimmed)
                .unwrap_or_else(|e| panic!("invalid JSON '{trimmed}': {e}"));
            if json.get("id").and_then(|v| v.as_i64()) == Some(id) {
                return json;
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

#[test]
fn mcp_analyze_produces_the_same_graph_as_the_cli() {
    let tmp = TempDir::new().unwrap();
    // A source proven sensitive to the ordering leaks: dynamic SQL whose call
    // targets are collected from several branches.
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/regress/execute_immediate_expr/cases");

    let cli_project = tmp.path().join("via_cli");
    let mcp_project = tmp.path().join("via_mcp");
    write_project(&cli_project, "entry", &source);
    write_project(&mcp_project, "entry", &source);

    let cli_analyze = Command::new(codeweb_bin())
        .current_dir(&cli_project)
        .args(["analyze", "-p", "."])
        .output()
        .expect("failed to run codeweb analyze");
    assert!(
        cli_analyze.status.success(),
        "cli analyze failed: {}",
        String::from_utf8_lossy(&cli_analyze.stderr)
    );

    let mut mcp = McpChild::start(&mcp_project);
    mcp.send(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}}}"#,
    );
    let _ = mcp.recv(1);
    mcp.send(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    mcp.send(
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"codeweb_analyze","arguments":{}}}"#,
    );
    let response = mcp.recv(2);
    assert!(
        response.get("error").is_none(),
        "codeweb_analyze errored: {response}"
    );
    drop(mcp);

    let cli_graph = export_ndjson(&cli_project);
    let mcp_graph = export_ndjson(&mcp_project);

    assert!(
        !cli_graph.is_empty() && cli_graph.lines().count() > 10,
        "the fixture should produce a non-trivial graph, got {} lines",
        cli_graph.lines().count()
    );
    assert_eq!(
        cli_graph, mcp_graph,
        "the MCP entry point must produce the same graph as the CLI entry point"
    );
}
