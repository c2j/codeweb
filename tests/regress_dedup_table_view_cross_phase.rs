//! Regression test: dedup_table_view_nodes cross-phase merge panic.
//!
//! A qualified Table merged into a View in Phase 1 gets removed, then
//! Phase 2 tries to merge a bare Table into the now-removed qualified
//! Table — triggering `add_edge` panic on dead NodeIndex.
//!
//! Key to reproduction: View query table references (unlike procedure-body
//! references) do NOT insert bare-name entries into table_index. This allows
//! the bare Table to be created in file 2 without being suppressed by the
//! qualified Table from file 1.

use std::fs;
use std::path::PathBuf;
use tempfile::TempDir;

fn codeweb_bin() -> PathBuf {
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) {
        "codeweb.exe"
    } else {
        "codeweb"
    };
    for entry in std::fs::read_dir(&base).unwrap().flatten() {
        let p = entry.path().join("debug").join(bin_name);
        if p.exists() {
            return p;
        }
        let p = entry.path().join("release").join(bin_name);
        if p.exists() {
            return p;
        }
    }
    let debug = base.join("debug").join(bin_name);
    if debug.exists() {
        return debug;
    }
    let release = base.join("release").join(bin_name);
    release
}

#[test]
fn regress_dedup_cross_phase_no_panic() {
    let dir = TempDir::new().unwrap();

    fs::write(
        dir.path().join("chunk1_qualified.sql"),
        include_str!("regress/dedup_table_view_cross_phase/chunk1_qualified_table.sql"),
    )
    .unwrap();

    fs::write(
        dir.path().join("chunk2_view_and_bare.sql"),
        include_str!("regress/dedup_table_view_cross_phase/chunk2_view_and_bare.sql"),
    )
    .unwrap();

    let output = std::process::Command::new(codeweb_bin())
        .args([dir.path().to_str().unwrap(), "--format", "json"])
        .output()
        .expect("failed to run codeweb");

    assert!(
        output.status.success(),
        "codeweb crashed or failed:\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("failed to parse JSON output");

    let view_nodes: Vec<_> = json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| {
            n["type"] == "view"
                && n["schema"] == "bigfund"
                && n["name"] == "orders"
        })
        .collect();
    assert_eq!(
        view_nodes.len(),
        1,
        "View 'bigfund.orders' should exist in output"
    );
}
