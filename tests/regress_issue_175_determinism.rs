//! Issue #175: `analyze` must be deterministic. The same source set must
//! produce the same graph, no matter what order the configured
//! `analysis.paths` are listed in.
//!
//! `scan_directory` yields files in `WalkDir` order, and the chunk order decides
//! node creation order, hence graph indices and the exported NDJSON. Java/XML/JSP
//! lists used to keep that scan order verbatim (only SQL was sorted), so listing
//! the same two source roots in the opposite order produced a different node
//! numbering for the exact same inputs.

use std::path::{Path, PathBuf};
use std::process::Output;
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

fn run(dir: &Path, args: &[&str]) -> Output {
    std::process::Command::new(codeweb_bin())
        .current_dir(dir)
        .args(args)
        .output()
        .expect("failed to run codeweb")
}

fn write_project(root: &Path, name: &str, paths: &[&Path]) {
    std::fs::create_dir_all(root).unwrap();
    let mut toml = format!("[project]\nname = \"{name}\"\n\n[analysis]\npaths = [");
    for (i, p) in paths.iter().enumerate() {
        if i > 0 {
            toml.push_str(", ");
        }
        toml.push_str(&format!("{:?}", p.to_string_lossy()));
    }
    toml.push_str("]\n\n[store]\npath = \".codeweb/store.bincode\"\nformat = \"bincode\"\n");
    std::fs::write(root.join("codeweb.toml"), toml).unwrap();
}

fn analyze_and_export(root: &Path) -> String {
    let analyze = run(root, &["analyze", "-p", "."]);
    assert!(
        analyze.status.success(),
        "analyze failed: {}",
        String::from_utf8_lossy(&analyze.stderr)
    );
    let export = run(root, &["export", "--format", "ndjson", "-p", "."]);
    assert!(
        export.status.success(),
        "export failed: {}",
        String::from_utf8_lossy(&export.stderr)
    );
    String::from_utf8_lossy(&export.stdout).to_string()
}

#[test]
fn analyze_output_is_independent_of_configured_path_order() {
    let tmp = TempDir::new().unwrap();
    let src_a = tmp.path().join("src_a");
    let src_b = tmp.path().join("src_b");
    std::fs::create_dir_all(&src_a).unwrap();
    std::fs::create_dir_all(&src_b).unwrap();
    std::fs::write(src_a.join("Alpha.java"), "class Alpha { void one() {} }").unwrap();
    std::fs::write(src_b.join("Beta.java"), "class Beta { void two() {} }").unwrap();

    let proj_a_then_b = tmp.path().join("proj_a_then_b");
    let proj_b_then_a = tmp.path().join("proj_b_then_a");
    write_project(&proj_a_then_b, "det", &[&src_a, &src_b]);
    write_project(&proj_b_then_a, "det", &[&src_b, &src_a]);

    let out_a_then_b = analyze_and_export(&proj_a_then_b);
    let out_b_then_a = analyze_and_export(&proj_b_then_a);

    assert!(!out_a_then_b.is_empty(), "NDJSON export must not be empty");
    assert_eq!(
        out_a_then_b, out_b_then_a,
        "analyze output must not depend on the order of `analysis.paths`"
    );
}
