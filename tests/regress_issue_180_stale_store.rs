//! Issue #180: a store written by an older codeweb must fail with a message the
//! user can act on, and `analyze -p` must rebuild it in place.
//!
//! Real report: `codeweb detail prc_deal_bond_repurchase_inst` printed only
//! "unsupported cache version 8, expected 13 — run `codeweb analyze` to
//! regenerate" against `exam/清算拆分优化考题/基线代码/.codeweb/store.bincode`,
//! naming neither the file nor the directory to rebuild.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::TempDir;

const STORE_MAGIC: &[u8] = b"CWEBSTORE";

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

fn run_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(codeweb_bin())
        .current_dir(dir)
        .args(args)
        .output()
        .expect("failed to run codeweb")
}

fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// A project directory holding a store written by an older layout (version 8,
/// the one from the issue) plus its manifest sidecar, so nothing but the store
/// itself is stale.
fn project_with_stale_store(root: &Path) {
    std::fs::create_dir_all(root.join(".codeweb")).unwrap();
    std::fs::write(
        root.join("codeweb.toml"),
        "[project]\nname = \"baseline\"\n\n[analysis]\npaths = [\".\"]\n\n\
         [store]\npath = \".codeweb/store.bincode\"\nformat = \"bincode\"\n",
    )
    .unwrap();
    let mut bytes: Vec<u8> = Vec::new();
    bytes.extend_from_slice(STORE_MAGIC);
    bytes.extend_from_slice(&8u32.to_le_bytes());
    bytes.extend_from_slice(&[0u8; 32]);
    std::fs::write(root.join(".codeweb/store.bincode"), &bytes).unwrap();
}

#[test]
fn stale_store_error_names_versions_path_and_repair_command() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("baseline");
    project_with_stale_store(&root);

    let out = run_in(
        &root,
        &["detail", "prc_deal_bond_repurchase_inst", "-p", "."],
    );
    assert!(
        !out.status.success(),
        "a stale store must not be read silently"
    );

    let text = combined(&out);
    assert!(
        text.contains("version 8"),
        "must name the version found in the file: {text}"
    );
    // The exact expected version is pinned dynamically by the store unit test
    // (`stale_store_error_names_versions_path_and_repair_command`); here it only
    // has to be present and newer than the stale file, so a version bump does
    // not silently make this end-to-end check vacuous.
    let expected = text
        .split("expected ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .map(|token| token.trim_end_matches(|c: char| !c.is_ascii_digit()))
        .and_then(|token| token.parse::<u32>().ok());
    assert!(
        matches!(expected, Some(v) if v > 8),
        "must name the version this binary expects: {text}"
    );
    assert!(
        text.contains("codeweb analyze -p"),
        "must give a runnable repair command: {text}"
    );
    let root_str = root.display().to_string();
    assert!(
        text.contains(&root_str),
        "the repair command must name the project root ({root_str}): {text}"
    );
    assert!(
        text.contains("not migrated"),
        "must say the old store is not migrated: {text}"
    );
}

#[test]
fn analyze_rebuilds_stale_store_and_says_so() {
    let tmp = TempDir::new().unwrap();
    let root = tmp.path().join("baseline");
    project_with_stale_store(&root);

    let out = run_in(&root, &["analyze", "-p", "."]);
    assert!(
        out.status.success(),
        "analyze must rebuild a stale store instead of failing: {}",
        combined(&out)
    );
    let text = combined(&out);
    assert!(
        text.contains("rebuilt store v8"),
        "analyze must report that it replaced the stale store: {text}"
    );

    // The point of rebuilding: the store is usable again.
    let stats = run_in(&root, &["stats", "-p", "."]);
    assert!(
        stats.status.success(),
        "the rebuilt store must load: {}",
        combined(&stats)
    );
}
