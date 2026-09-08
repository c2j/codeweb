//! Regression for #158: `%TYPE`/`%ROWTYPE` schema anchors must produce
//! `AnchorsOn` edges alongside (not instead of) normal `TableAccess` edges,
//! and must never fire for `cursor%ROWTYPE` (issue #147/#142 guard).
//!
//! Simplified, parseable equivalent of the real-world issue #158 sample: a
//! function whose `RETURN` clause, a local `RESULT` variable, and another
//! local variable all anchor to the same DML-read table
//! (`par_sys_purchase`), plus a second variable anchored to a table that is
//! never referenced in DML (`dat_trd_repurchase`) — this table must get an
//! inferred `table*` node with only an `AnchorsOn` edge, no `TableAccess`.

use std::fs;
use std::path::Path;
use tempfile::TempDir;

const ANCHOR_EDGES: &str =
    include_str!("regress/issue_158_type_anchor_edges/cases/anchor_edges.sql");

fn run_codeweb(args: &[&str]) -> std::process::Output {
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
            return std::process::Command::new(p)
                .args(args)
                .output()
                .expect("failed to run codeweb");
        }
    }
    let bin = base.join("debug").join(bin_name);
    std::process::Command::new(bin)
        .args(args)
        .output()
        .expect("failed to run codeweb")
}

fn analyze_json(sql: &str) -> serde_json::Value {
    let dir = TempDir::new().unwrap();
    fs::write(dir.path().join("test.sql"), sql).unwrap();
    let output = run_codeweb(&[dir.path().to_str().unwrap(), "--format", "json"]);
    assert!(
        output.status.success(),
        "codeweb analyze failed\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).expect("failed to parse JSON output")
}

fn node_id_by_name(json: &serde_json::Value, name: &str) -> Option<usize> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"].as_str().map(|s| s.eq_ignore_ascii_case(name)) == Some(true))
        .and_then(|n| n["id"].as_u64())
        .map(|id| id as usize)
}

fn edges_between(json: &serde_json::Value, source: &str, target: &str) -> Vec<serde_json::Value> {
    let (Some(src_id), Some(dst_id)) =
        (node_id_by_name(json, source), node_id_by_name(json, target))
    else {
        return vec![];
    };
    json["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| {
            e["source"].as_u64() == Some(src_id as u64)
                && e["target"].as_u64() == Some(dst_id as u64)
        })
        .cloned()
        .collect()
}

fn node_by_name(json: &serde_json::Value, name: &str) -> serde_json::Value {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["name"].as_str().map(|s| s.eq_ignore_ascii_case(name)) == Some(true))
        .cloned()
        .unwrap_or_else(|| panic!("node '{name}' not found in graph"))
}

/// Both `TableAccess` (from the `SELECT ... INTO` DML read) and `AnchorsOn`
/// (from the `%TYPE` signature/variable anchors) must exist between the
/// function and `par_sys_purchase` — coexisting, not merged into one edge.
#[test]
fn issue_158_dml_and_anchor_edges_coexist_on_read_table() {
    let json = analyze_json(ANCHOR_EDGES);

    let edges = edges_between(&json, "fnc_get_purchase_js_days", "par_sys_purchase");
    let table_access_count = edges
        .iter()
        .filter(|e| e["type"].as_str() == Some("table_access"))
        .count();
    let anchor_count = edges
        .iter()
        .filter(|e| e["type"].as_str() == Some("anchors_on"))
        .count();

    assert!(
        table_access_count >= 1,
        "expected at least 1 TableAccess edge f -> par_sys_purchase, got edges: {edges:?}"
    );
    assert!(
        anchor_count >= 1,
        "expected at least 1 AnchorsOn edge f -> par_sys_purchase, got edges: {edges:?}"
    );

    let has_read = edges.iter().any(|e| {
        e["type"].as_str() == Some("table_access")
            && e["modes"]
                .as_array()
                .map(|m| m.iter().any(|v| v.as_str() == Some("read")))
                .unwrap_or(false)
    });
    assert!(
        has_read,
        "TableAccess edge to par_sys_purchase must carry Read mode, got edges: {edges:?}"
    );
}

/// `dat_trd_repurchase` is referenced only through a `%TYPE` variable
/// declaration, never in DML — it must become an inferred (`explicit:
/// false`) table node reachable only via `AnchorsOn`, with zero
/// `TableAccess` edges.
#[test]
fn issue_158_type_only_reference_produces_inferred_table_with_anchor_only() {
    let json = analyze_json(ANCHOR_EDGES);

    let table_node = node_by_name(&json, "dat_trd_repurchase");
    assert_eq!(table_node["type"].as_str(), Some("table"));
    // `explicit` is only serialized when `true` (skip_serializing_if =
    // is_false in json.rs) — a missing field means explicit=false, i.e.
    // this table has no DDL and was inferred from the %TYPE anchor.
    assert!(
        !table_node["explicit"].as_bool().unwrap_or(false),
        "dat_trd_repurchase has no DDL — must be inferred (explicit=false), got {table_node:?}"
    );

    let edges = edges_between(&json, "fnc_get_purchase_js_days", "dat_trd_repurchase");
    assert!(
        !edges.is_empty(),
        "expected at least 1 AnchorsOn edge f -> dat_trd_repurchase"
    );
    assert!(
        edges
            .iter()
            .all(|e| e["type"].as_str() == Some("anchors_on")),
        "dat_trd_repurchase must only be reached via AnchorsOn edges (no DML), got: {edges:?}"
    );
}

/// No `AnchorsOn` edge anywhere in the graph may originate from a
/// `cursor%ROWTYPE` declaration — this fixture has no cursors at all, so
/// this is a straightforward absence check guarding against a regression
/// that would anchor every `%ROWTYPE` unconditionally.
#[test]
fn issue_158_no_cursor_rowtype_anchor_edges_present() {
    let json = analyze_json(ANCHOR_EDGES);

    let anchor_edges: Vec<_> = json["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"].as_str() == Some("anchors_on"))
        .collect();
    assert!(
        !anchor_edges.is_empty(),
        "sanity check: fixture should produce at least one AnchorsOn edge"
    );

    for edge in &anchor_edges {
        assert_ne!(
            edge["kind"].as_str(),
            Some("percent_row_type"),
            "this fixture declares no cursors — no percent_row_type anchor should exist, got {edge:?}"
        );
    }
}

// ── Remaining #158 acceptance items: lineage / conflicts / impact / detail ──
//
// These four tests need a real `codeweb.toml` project (via `init`) because
// `lineage`, `conflicts`, `impact`, and `detail` all load a `GraphStore` via
// `project::Project::find`, unlike the legacy no-subcommand `analyze_json`
// path above which only exports the freshly built graph. This file only has
// access to the compiled binary's CLI surface (no `[lib]` target exists in
// this crate — see Cargo.toml — so integration tests cannot call
// `GraphBuilder`, `find_conflicts`, or `edge_label_for` directly).

/// Run `codeweb` with `dir` as the working directory (needed for `init`,
/// which always operates on `std::env::current_dir()`).
fn run_in_dir(dir: &Path, args: &[&str]) -> std::process::Output {
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
    let bin_name = if cfg!(windows) {
        "codeweb.exe"
    } else {
        "codeweb"
    };
    let bin = std::fs::read_dir(&base)
        .unwrap_or_else(|_| panic!("no target dir"))
        .flatten()
        .map(|entry| entry.path().join("debug").join(bin_name))
        .find(|p| p.exists())
        .unwrap_or_else(|| base.join("debug").join(bin_name));
    std::process::Command::new(bin)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("failed to run codeweb")
}

/// Write `sql` as the sole source file of a fresh project directory and
/// `init` it (which also runs the first full analysis). Returns the project
/// root (== `dir.path()`).
fn init_project(dir: &TempDir, name: &str, sql: &str) -> std::path::PathBuf {
    let root = dir.path().to_path_buf();
    fs::write(root.join("t.sql"), sql).unwrap();
    let out = run_in_dir(&root, &["init", name, "-d", "."]);
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    root
}

/// A table (`par_sys_purchase`) reached only via a `%TYPE` anchor by
/// `proc_anchor_only` (no DML at all on that table — the write it does
/// perform, to `anchor_target`, is in a statement with zero relation to
/// `par_sys_purchase`), alongside `proc_with_dml`, which genuinely reads
/// `par_sys_purchase` and writes `dml_target` in the same statement.
const LINEAGE_ANCHOR_SQL: &str = r#"
CREATE TABLE par_sys_purchase(id NUMBER, purchase_days NUMBER);
CREATE TABLE anchor_target(id NUMBER);
CREATE TABLE dml_target(id NUMBER, purchase_days NUMBER);

CREATE OR REPLACE PROCEDURE proc_anchor_only IS
    v_days par_sys_purchase.purchase_days%TYPE;
BEGIN
    INSERT INTO anchor_target(id) VALUES (1);
END;
/

CREATE OR REPLACE PROCEDURE proc_with_dml AS
BEGIN
    INSERT INTO dml_target(id, purchase_days)
    SELECT id, purchase_days FROM par_sys_purchase;
END;
/
"#;

/// `AnchorsOn` edges must not produce table-level lineage hops: `lineage`
/// only pattern-matches `Edge::TableAccess` (see
/// `src/graph/lineage.rs::build_table_lineage`), so a routine connected to a
/// table solely through a `%TYPE` anchor must never be treated as a reader
/// or writer of that table. If a future change broadened the match to also
/// treat `AnchorsOn` as an implicit read, `proc_anchor_only` would wrongly
/// qualify as a downstream reader of `par_sys_purchase` and leak its
/// unrelated write to `anchor_target` into the lineage tree — this test
/// would then fail on the last two assertions.
#[test]
fn issue_158_lineage_ignores_anchor_edges() {
    let dir = TempDir::new().unwrap();
    let root = init_project(&dir, "lineage-anchor-test", LINEAGE_ANCHOR_SQL);

    let out = run_in_dir(
        &root,
        &[
            "lineage",
            "par_sys_purchase",
            "--direction",
            "downstream",
            "--format",
            "tree",
        ],
    );
    assert!(
        out.status.success(),
        "lineage failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_lowercase();

    assert!(
        stdout.contains("dml_target"),
        "genuine DML-connected downstream table missing:\n{stdout}"
    );
    assert!(
        stdout.contains("proc_with_dml"),
        "connecting DML routine missing:\n{stdout}"
    );
    assert!(
        !stdout.contains("proc_anchor_only"),
        "AnchorsOn-only routine must never be treated as a lineage hop:\n{stdout}"
    );
    assert!(
        !stdout.contains("anchor_target"),
        "table only reachable through the anchor-only routine's unrelated \
         write must not leak into par_sys_purchase's lineage:\n{stdout}"
    );
}

/// Same fixture idea for lock-conflict detection: `proc_anchor_only` only
/// anchors to `par_sys_purchase` via `%TYPE` and performs no DML on it at
/// all, while `proc_trunc`/`proc_select` genuinely conflict (TRUNCATE vs.
/// SELECT — same HIGH-severity pattern already locked by
/// `regress_issue_144_ddl_locks.rs`).
const CONFLICT_ANCHOR_SQL: &str = r#"
CREATE TABLE par_sys_purchase(id NUMBER, purchase_days NUMBER);

CREATE OR REPLACE PROCEDURE proc_anchor_only IS
    v_days par_sys_purchase.purchase_days%TYPE;
BEGIN
    NULL;
END;
/

CREATE OR REPLACE PROCEDURE proc_trunc IS
BEGIN
    TRUNCATE TABLE par_sys_purchase;
END;
/

CREATE OR REPLACE PROCEDURE proc_select IS
    v INT;
BEGIN
    SELECT COUNT(*) INTO v FROM par_sys_purchase;
END;
/
"#;

/// `find_conflicts` (src/graph/conflict.rs) only collects locks from
/// `Edge::TableAccess { flow_kind: DmlAccess, .. }` edges — `AnchorsOn`
/// edges carry no `AccessMode` and are a different enum variant entirely, so
/// they can never contribute a `ProcTableLock`. `proc_anchor_only` must
/// therefore never appear in any conflict entry, while the genuinely
/// conflicting DML pair still must be reported (proving the check isn't
/// vacuously true because conflict detection produced nothing at all).
#[test]
fn issue_158_conflicts_ignore_anchor_edges() {
    let dir = TempDir::new().unwrap();
    let root = init_project(&dir, "conflict-anchor-test", CONFLICT_ANCHOR_SQL);

    let out = run_in_dir(&root, &["conflicts", "--format", "json"]);
    assert!(
        out.status.success(),
        "conflicts failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();
    let conflicts = json["conflicts"].as_array().unwrap();

    assert!(
        conflicts.iter().all(|c| {
            let a = c["proc_a"].as_str().unwrap_or("").to_lowercase();
            let b = c["proc_b"].as_str().unwrap_or("").to_lowercase();
            !a.contains("proc_anchor_only") && !b.contains("proc_anchor_only")
        }),
        "an AnchorsOn-only routine must never appear in a lock conflict: {conflicts:?}"
    );

    let has_dml_conflict = conflicts.iter().any(|c| {
        c["severity"].as_str() == Some("high")
            && c["table"]
                .as_str()
                .unwrap_or("")
                .to_lowercase()
                .contains("par_sys_purchase")
            && {
                let a = c["proc_a"].as_str().unwrap_or("").to_lowercase();
                let b = c["proc_b"].as_str().unwrap_or("").to_lowercase();
                (a.contains("proc_trunc") && b.contains("proc_select"))
                    || (a.contains("proc_select") && b.contains("proc_trunc"))
            }
    });
    assert!(
        has_dml_conflict,
        "expected HIGH proc_trunc vs proc_select conflict on par_sys_purchase \
         (sanity check that conflict detection is actually exercised): {conflicts:?}"
    );
}

/// `impact`'s core value for #158: a table reached only via an `AnchorsOn`
/// edge (`dat_trd_repurchase`, from the same fixture Task 7 uses for the
/// coexistence/inferred-table tests above) must still resolve upstream
/// impact back to the anchoring function — `impact`'s default `EdgeFilter`
/// has no category restriction (`EdgeFilter::new()` → `categories: None`,
/// see `src/graph/query/filter.rs`), so it traverses every edge kind
/// including `AnchorsOn`. If a future change scoped the default filter to
/// exclude `AnchorsOn`, this table would show empty upstream impact even
/// though the function is the entire reason the table node exists.
#[test]
fn issue_158_impact_reaches_via_anchor_edge() {
    let dir = TempDir::new().unwrap();
    let root = init_project(&dir, "impact-anchor-test", ANCHOR_EDGES);

    let out = run_in_dir(
        &root,
        &["impact", "--node", "dat_trd_repurchase", "--format", "json"],
    );
    assert!(
        out.status.success(),
        "impact failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let json: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&out.stdout)).unwrap();

    let upstream = json["upstream"].as_array().unwrap();
    assert!(
        upstream.iter().any(|e| e["symbol"]
            .as_str()
            .unwrap_or("")
            .to_lowercase()
            .contains("fnc_get_purchase_js_days")),
        "impact from dat_trd_repurchase (reachable only through an AnchorsOn \
         edge) must surface the anchoring function as upstream: {upstream:?}"
    );
}

/// `detail`'s CALLEES section renders each edge label via
/// `traverse::edge_label_for`, which is `pub(crate)` — integration tests
/// cannot call it directly (this crate has no `[lib]` target at all, so even
/// `pub` items are unreachable from `tests/`; see the module comment above).
/// The equivalent, fully public-API verification is running the actual
/// `codeweb detail` CLI command and asserting on its rendered text: the
/// coexisting-edges table (`par_sys_purchase`) must show the aggregated
/// `[R,T]` bracket (both TableAccess-Read and AnchorsOn present), while the
/// anchor-only table (`dat_trd_repurchase`) must show `[T]` alone. This is
/// the same invariant `edge_label_for`'s unit tests in
/// `src/graph/traverse.rs` already lock (e.g.
/// `should_aggregate_table_access_and_anchors_on` asserting `Some("[R,T]")`),
/// verified here through the same public path an actual user runs.
#[test]
fn issue_158_detail_labels_show_both_r_and_t() {
    let dir = TempDir::new().unwrap();
    let root = init_project(&dir, "detail-anchor-test", ANCHOR_EDGES);

    let out = run_in_dir(&root, &["detail", "fnc_get_purchase_js_days"]);
    assert!(
        out.status.success(),
        "detail failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();

    let callees_section = stdout
        .split("── CALLEES ──")
        .nth(1)
        .unwrap_or_else(|| panic!("no CALLEES section in detail output:\n{stdout}"));

    let dml_and_anchor_line = callees_section
        .lines()
        .find(|l| l.to_lowercase().contains("par_sys_purchase"))
        .unwrap_or_else(|| panic!("par_sys_purchase missing from CALLEES:\n{stdout}"));
    assert!(
        dml_and_anchor_line.contains("[R,T]"),
        "par_sys_purchase CALLEES line must show the aggregated [R,T] label \
         (TableAccess-Read + AnchorsOn coexisting): {dml_and_anchor_line}"
    );

    let anchor_only_line = callees_section
        .lines()
        .find(|l| l.to_lowercase().contains("dat_trd_repurchase"))
        .unwrap_or_else(|| panic!("dat_trd_repurchase missing from CALLEES:\n{stdout}"));
    assert!(
        anchor_only_line.contains("[T]") && !anchor_only_line.contains("[R"),
        "dat_trd_repurchase CALLEES line must show [T] alone (no DML read \
         ever happens on it): {anchor_only_line}"
    );
}

// ── PR #164 review fixes: parameter-name guard + package-level nested TYPE ──

/// PR #164 review Issue 1/2: a routine parameter name is not a
/// `PlDeclaration` inside the block, so without explicit injection the
/// `%TYPE` anchor `v p_emp.empno%TYPE` would resolve `p_emp` into a fake
/// inferred table. End-to-end through the JSON export: no `p_emp` node, no
/// `anchors_on` edge anywhere in the graph.
#[test]
fn issue_158_param_name_anchor_suppressed_end_to_end() {
    let sql = r#"
        CREATE OR REPLACE PROCEDURE proc_param_guard_e2e(p_emp VARCHAR2)
        IS
            v p_emp.empno%TYPE;
        BEGIN
            NULL;
        END;
    "#;
    let json = analyze_json(sql);

    assert!(
        node_id_by_name(&json, "p_emp").is_none(),
        "parameter name p_emp must never surface as a graph node: {json}"
    );

    let anchor_edges: Vec<_> = json["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"].as_str() == Some("anchors_on"))
        .collect();
    assert!(
        anchor_edges.is_empty(),
        "parameter name guard must suppress this anchor entirely, got {anchor_edges:?}"
    );
}

/// PR #164 review Issue 3: a package-level nested `TYPE ... IS TABLE OF
/// tbl.col%TYPE` must produce an `anchors_on` edge from the **package**
/// node to the anchored table, verified through the JSON export (the same
/// public surface an actual user inspects).
#[test]
fn issue_158_package_nested_type_anchor_end_to_end() {
    let sql = r#"
        CREATE OR REPLACE PACKAGE BODY pkg_nested_type_e2e AS
            TYPE t_list_e2e IS TABLE OF some_table_e2e.some_col_e2e%TYPE;
        END pkg_nested_type_e2e;
    "#;
    let json = analyze_json(sql);

    let edges = edges_between(&json, "pkg_nested_type_e2e", "some_table_e2e");
    assert!(
        !edges.is_empty(),
        "expected an anchors_on edge pkg_nested_type_e2e -> some_table_e2e, got json: {json}"
    );
    assert!(
        edges
            .iter()
            .all(|e| e["type"].as_str() == Some("anchors_on")),
        "package -> table edge must be anchors_on (site=nested_type), got {edges:?}"
    );
    assert!(
        edges
            .iter()
            .any(|e| e["site"].as_str() == Some("nested_type")),
        "expected an anchors_on edge with site=nested_type, got {edges:?}"
    );

    assert!(
        node_id_by_name(&json, "t_list_e2e").is_none(),
        "package-level TYPE name t_list_e2e must never surface as a graph node"
    );
}

// ── PR #164 review round 2 (#158): SPEC inheritance + signature guard +
//    self-naming idiom, combined end-to-end ──

/// Combines all three PR #164 review round 2 fixes in one SPEC+BODY
/// fixture, verified through the JSON export:
/// - SPEC declares `CURSOR c` / `TYPE rec_t` / `v_emp employees_e2e_r2%ROWTYPE`.
/// - BODY's `p_body(p_rec c%ROWTYPE)` anchors its parameter to the
///   SPEC-inherited cursor `c` (Task 1 SPEC inheritance + Task 2 signature
///   guard) and its locals to the SPEC-inherited `rec_t`/`v_emp` (Task 1 +
///   existing body-walk guard) — all three must produce no fake table.
/// - BODY's `v_ok real_table_e2e_r2.real_col_e2e_r2%TYPE` is the control:
///   a genuine, unrelated table anchor that must still come through.
/// - BODY's `p(employees_e2e_r2 employees_e2e_r2%ROWTYPE)` is the Oracle
///   self-naming idiom (Task 3 signature side, via Task 2's self-exclusion)
///   and must anchor to the real `employees_e2e_r2` table (site=param).
/// - The SPEC's own `v_emp employees_e2e_r2%ROWTYPE` is a genuine
///   package-level anchor and must also survive (site=variable).
#[test]
fn issue_158_spec_body_inherited_guards_end_to_end() {
    let sql = r#"
        CREATE OR REPLACE PACKAGE pkg_e2e_review2 AS
            CURSOR c IS SELECT id FROM t_cursor_src_e2e_r2;
            TYPE rec_t IS RECORD (f INTEGER);
            v_emp employees_e2e_r2%ROWTYPE;
        END pkg_e2e_review2;

        CREATE OR REPLACE PACKAGE BODY pkg_e2e_review2 AS
            PROCEDURE p_body(p_rec c%ROWTYPE) IS
                v1 rec_t.f%TYPE;
                v2 v_emp.empno%TYPE;
                v_ok real_table_e2e_r2.real_col_e2e_r2%TYPE;
            BEGIN
                NULL;
            END;

            PROCEDURE p(employees_e2e_r2 employees_e2e_r2%ROWTYPE) IS
            BEGIN
                NULL;
            END;
        END pkg_e2e_review2;
    "#;
    let json = analyze_json(sql);

    // Fake tables that must never appear: the cursor name, the
    // package-level TYPE name, and the SPEC-inherited variable name.
    for fake in ["c", "rec_t", "v_emp"] {
        assert!(
            node_id_by_name(&json, fake).is_none(),
            "'{fake}' must never surface as a graph node, json: {json}"
        );
    }

    // v_ok's control anchor: p_body -> real_table_e2e_r2.
    let control_edges = edges_between(&json, "p_body", "real_table_e2e_r2");
    assert!(
        !control_edges.is_empty(),
        "expected p_body -> real_table_e2e_r2 anchors_on edge (control case), json: {json}"
    );
    assert!(
        control_edges
            .iter()
            .all(|e| e["type"].as_str() == Some("anchors_on")),
        "control edge must be anchors_on, got {control_edges:?}"
    );

    // Self-naming idiom on a signature parameter: p -> employees_e2e_r2,
    // site=param.
    let self_named_edges = edges_between(&json, "p", "employees_e2e_r2");
    assert!(
        !self_named_edges.is_empty(),
        "expected p -> employees_e2e_r2 anchors_on edge (self-named param), json: {json}"
    );
    assert!(
        self_named_edges
            .iter()
            .any(|e| e["site"].as_str() == Some("param")),
        "expected an anchors_on edge with site=param, got {self_named_edges:?}"
    );

    // SPEC's own package-level anchor: pkg_e2e_review2 -> employees_e2e_r2,
    // site=variable.
    let spec_edges = edges_between(&json, "pkg_e2e_review2", "employees_e2e_r2");
    assert!(
        !spec_edges.is_empty(),
        "expected pkg_e2e_review2 -> employees_e2e_r2 anchors_on edge (SPEC variable), \
         json: {json}"
    );
    assert!(
        spec_edges
            .iter()
            .any(|e| e["site"].as_str() == Some("variable")),
        "expected an anchors_on edge with site=variable, got {spec_edges:?}"
    );

    // No anchors_on edge anywhere may target 'c' / 'rec_t' / 'v_emp' — the
    // fake node names themselves are already checked above, but this also
    // rules out an anchor pointing at them via schema-qualification or any
    // other resolution path.
    let anchor_edges: Vec<_> = json["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"].as_str() == Some("anchors_on"))
        .collect();
    for edge in &anchor_edges {
        let target_id = edge["target"].as_u64();
        for fake in ["c", "rec_t", "v_emp"] {
            assert_ne!(
                target_id,
                node_id_by_name(&json, fake).map(|id| id as u64),
                "no anchors_on edge may target the fake node '{fake}': {edge:?}"
            );
        }
    }
}
