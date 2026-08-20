//! Table-level lineage traversal (#115).
//!
//! `codeweb lineage <table> --direction upstream|downstream` answers "who writes this
//! table" / "where does this table flow to" in one step.

use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn codeweb_bin() -> std::path::PathBuf {
    let base = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target");
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

fn run_codeweb_in(cwd: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(codeweb_bin())
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("failed to run codeweb")
}

/// Lay out a project directory with `src/t.sql`, then `init` it (which also runs the
/// first full analysis). Returns the project root.
fn project_with_sql(dir: &TempDir, sql: &str) -> std::path::PathBuf {
    let root = dir.path().to_path_buf();
    let src = root.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(src.join("t.sql"), sql).unwrap();

    let out = run_codeweb_in(
        &root,
        &["init", "lineage-test", "--dir", src.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    root
}

fn lineage(root: &Path, target: &str, direction: &str, format: &str) -> String {
    lineage_extra(root, target, direction, format, &[])
}

fn lineage_extra(
    root: &Path,
    target: &str,
    direction: &str,
    format: &str,
    extra: &[&str],
) -> String {
    let mut args: Vec<&str> = vec![
        "lineage",
        target,
        "--direction",
        direction,
        "--format",
        format,
        "-p",
        root.to_str().unwrap(),
    ];
    args.extend_from_slice(extra);
    let out = run_codeweb_in(root, &args);
    assert!(
        out.status.success(),
        "lineage failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Three-stage pipeline: source_tbl -> mid_tbl -> final_tbl.
const PIPELINE_SQL: &str = r#"
CREATE TABLE source_tbl(id NUMBER, amount NUMBER);
CREATE TABLE mid_tbl(id NUMBER, total NUMBER);
CREATE TABLE final_tbl(id NUMBER, result NUMBER);

CREATE PROCEDURE prc_step1 AS BEGIN
  INSERT INTO mid_tbl(id, total)
  SELECT id, SUM(amount) FROM source_tbl GROUP BY id;
END;

CREATE PROCEDURE prc_step2 AS BEGIN
  INSERT INTO final_tbl(id, result)
  SELECT id, total FROM mid_tbl;
END;
"#;

#[test]
fn upstream_walks_back_through_writing_routines() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    let out = lineage(&root, "final_tbl", "upstream", "tree");

    // final_tbl is written by prc_step2, which reads mid_tbl; mid_tbl is written by
    // prc_step1, which reads source_tbl.
    assert!(out.contains("final_tbl"), "missing root node:\n{out}");
    assert!(
        out.contains("prc_step2"),
        "missing writer of final_tbl:\n{out}"
    );
    assert!(
        out.contains("mid_tbl"),
        "missing 1-hop upstream table:\n{out}"
    );
    assert!(
        out.contains("prc_step1"),
        "missing writer of mid_tbl:\n{out}"
    );
    assert!(
        out.contains("source_tbl"),
        "missing 2-hop upstream table:\n{out}"
    );
}

#[test]
fn downstream_walks_forward_through_reading_routines() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    let out = lineage(&root, "source_tbl", "downstream", "tree");

    assert!(out.contains("source_tbl"), "missing root node:\n{out}");
    assert!(
        out.contains("prc_step1"),
        "missing reader of source_tbl:\n{out}"
    );
    assert!(
        out.contains("mid_tbl"),
        "missing 1-hop downstream table:\n{out}"
    );
    assert!(
        out.contains("final_tbl"),
        "missing 2-hop downstream table:\n{out}"
    );
}

#[test]
fn upstream_and_downstream_label_the_direction_differently() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    let up = lineage(&root, "final_tbl", "upstream", "tree");
    assert!(
        up.contains("upstream entities"),
        "upstream should label the traversal direction:\n{up}"
    );
    // Upstream children feed the parent: `← proc`; downstream children are fed by it: `→ proc`.
    assert!(
        up.contains("← proc:"),
        "upstream edges should point at the connecting routine:\n{up}"
    );
    let down = lineage(&root, "source_tbl", "downstream", "tree");
    assert!(
        down.contains("downstream entities"),
        "downstream should label the traversal direction:\n{down}"
    );
    assert!(
        down.contains("→ proc:"),
        "downstream edges should point at the connecting routine:\n{down}"
    );
}

/// Every parent→child edge must be attributed to the routine performing the hop — the
/// core "direction & convergence" information of a lineage tree (#115 review feedback).
#[test]
fn every_child_edge_is_attributed_to_its_connecting_routine() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    // final_tbl ← prc_step2 (writes final_tbl, reads mid_tbl) ← prc_step1 (writes mid_tbl,
    // reads source_tbl). Each child line carries the routine that feeds the parent.
    let up = lineage(&root, "final_tbl", "upstream", "tree");
    assert!(
        up.contains("table:mid_tbl  ← proc:prc_step2"),
        "1-hop child must be attributed to its connecting routine:\n{up}"
    );
    assert!(
        up.contains("table:source_tbl  ← proc:prc_step1"),
        "2-hop child must be attributed to its connecting routine:\n{up}"
    );

    // Downstream, the arrow points at the child and carries the write kind.
    let down = lineage(&root, "source_tbl", "downstream", "tree");
    assert!(
        down.contains("table:mid_tbl  → proc:prc_step1"),
        "downstream child must be attributed:\n{down}"
    );
    assert!(
        down.contains("table:final_tbl  → proc:prc_step2"),
        "2-hop downstream child must be attributed:\n{down}"
    );
}

/// The writer label shows how each routine writes the table (detail-aligned `[W:...]`
/// mode bracket), so a table fed by INSERT from one routine and UPDATE from another is
/// distinguishable.
#[test]
fn writer_label_includes_write_kinds() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    let out = lineage(&root, "final_tbl", "upstream", "tree");
    assert!(
        out.contains("[W:insert_select]"),
        "writer label should carry the detail-aligned write-kind bracket:\n{out}"
    );
}

/// A table reached through two different routines lists both on its edge, so shared
/// upstream sources are not silently flattened into one path.
#[test]
fn shared_child_lists_every_connecting_routine() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE src_a(id NUMBER);
CREATE TABLE src_b(id NUMBER);
CREATE TABLE out_tbl(id NUMBER);

CREATE PROCEDURE prc_first AS BEGIN
  INSERT INTO out_tbl(id) SELECT id FROM src_a;
END;

CREATE PROCEDURE prc_second AS BEGIN
  INSERT INTO out_tbl(id) SELECT id FROM src_b;
END;
"#,
    );

    let up = lineage(&root, "out_tbl", "upstream", "tree");
    assert!(
        up.contains("table:src_a  ← proc:prc_first"),
        "src_a must be attributed to prc_first:\n{up}"
    );
    assert!(
        up.contains("table:src_b  ← proc:prc_second"),
        "src_b must be attributed to prc_second:\n{up}"
    );
    // No child should be mislabeled with the other routine.
    assert!(
        !up.contains("table:src_a  ← proc:prc_second"),
        "src_a is read by prc_first only:\n{up}"
    );
}

#[test]
fn json_output_is_a_nested_node_via_children_tree() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    let out = lineage(&root, "final_tbl", "upstream", "json");
    let json: serde_json::Value =
        serde_json::from_str(&out).expect("lineage --format json must emit valid JSON");

    assert!(json["node"].as_str().unwrap().contains("final_tbl"));
    assert!(json["via"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str().unwrap_or("").contains("prc_step2")));

    let child = &json["children"][0];
    assert!(child["node"].as_str().unwrap().contains("mid_tbl"));
    // Edge attribution: the child must say which routine connects final_tbl → mid_tbl.
    assert!(child["connected_by"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str().unwrap_or("").contains("prc_step2")));
    assert!(child["children"][0]["node"]
        .as_str()
        .unwrap()
        .contains("source_tbl"));
    assert!(child["children"][0]["connected_by"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v.as_str().unwrap_or("").contains("prc_step1")));
}

/// A view's base tables are upstream of it; the view is downstream of its base tables.
/// Getting this backwards was the first bug found against the real exam codebase.
#[test]
fn view_dependencies_point_from_base_table_to_view() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE base_tbl(id NUMBER, raw NUMBER);
CREATE VIEW v_derived AS SELECT id, raw * 2 AS doubled FROM base_tbl;
"#,
    );

    let up = lineage(&root, "v_derived", "upstream", "tree");
    assert!(
        up.contains("base_tbl"),
        "a view's base table is upstream of it:\n{up}"
    );

    let down = lineage(&root, "base_tbl", "downstream", "tree");
    assert!(
        down.contains("v_derived"),
        "a view that selects from a table is downstream of it:\n{down}"
    );
}

/// The traversal must not label a node as its own step (a view reaching its own base
/// tables would otherwise print `v_x  [v_x [R]]` as its own writer).
#[test]
fn a_node_is_never_listed_as_its_own_step() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE base_tbl(id NUMBER);
CREATE VIEW v_derived AS SELECT id FROM base_tbl;
"#,
    );

    let out = lineage(&root, "v_derived", "upstream", "tree");
    for line in out.lines() {
        // Strip any leading role tag (`[flow] ` / `[ref]  ` / `[ref?] `) so the node key
        // is the first real token, then check the node's own step bracket does not
        // contain that key.
        let trimmed = line.trim_start();
        let rest = trimmed
            .strip_prefix("[flow]")
            .or_else(|| trimmed.strip_prefix("[ref]"))
            .or_else(|| trimmed.strip_prefix("[ref?]"))
            .unwrap_or(trimmed);
        let node = rest.split_whitespace().next().unwrap_or("");
        if node.is_empty() {
            continue;
        }
        if let Some((_, label)) = rest.split_once("  [") {
            assert!(!label.contains(node), "node listed as its own step: {line}");
        }
    }
}

/// A wide target lets the L0 column-overlap heuristic separate the data-bearing source
/// (flow) from the parameter lookup (reference), with per-edge coverage (issue #146).
const WIDE_SQL: &str = r#"
CREATE TABLE src_trade(id NUMBER, c1 NUMBER, c2 NUMBER, c3 NUMBER, c4 NUMBER, c5 NUMBER, c6 NUMBER, c7 NUMBER, c8 NUMBER, c9 NUMBER);
CREATE TABLE par_cfg(fund_code VARCHAR2(8), rate NUMBER, mode NUMBER);
CREATE TABLE out_tbl(id NUMBER, c1 NUMBER, c2 NUMBER, c3 NUMBER, c4 NUMBER, c5 NUMBER, c6 NUMBER, c7 NUMBER, c8 NUMBER, c9 NUMBER, rate NUMBER, mode NUMBER);
CREATE PROCEDURE prc_main AS BEGIN
  INSERT INTO out_tbl(id, c1, c2, c3, c4, c5, c6, c7, c8, c9, rate, mode)
  SELECT t.id, t.c1, t.c2, t.c3, t.c4, t.c5, t.c6, t.c7, t.c8, t.c9, c.rate, c.mode
    FROM src_trade t, par_cfg c
   WHERE c.fund_code = t.id;
END;
"#;

#[test]
fn flow_and_reference_sources_are_marked_with_coverage() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, WIDE_SQL);

    let out = lineage(&root, "out_tbl", "upstream", "tree");
    // src_trade feeds 10 of out_tbl's 12 columns → flow; par_cfg feeds only 2 → reference.
    assert!(
        out.contains("[flow]  table:src_trade  ← proc:prc_main [R]  (flow 10/12)"),
        "data-bearing source must be marked flow with coverage:\n{out}"
    );
    assert!(
        out.contains("[ref]   table:par_cfg  ← proc:prc_main [R]  (overlap 2/12)  [external]"),
        "parameter lookup must be marked reference:\n{out}"
    );
    assert!(
        out.contains("flow 1 · reference 1"),
        "root summary must count flow/reference:\n{out}"
    );
}

#[test]
fn flow_only_hides_reference_sources() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, WIDE_SQL);

    let out = lineage_extra(&root, "out_tbl", "upstream", "tree", &["--flow-only"]);
    assert!(
        out.contains("table:src_trade"),
        "flow source must remain with --flow-only:\n{out}"
    );
    assert!(
        !out.contains("table:par_cfg"),
        "reference source must be hidden with --flow-only:\n{out}"
    );
    assert!(
        out.contains("flow 1 · reference 0"),
        "summary must reflect the filter:\n{out}"
    );
}

/// A source with no DDL definition has no column evidence — it classifies `unknown`
/// (`[ref?]`), not silently as a reference (issue #146 revision 2).
#[test]
fn unknown_role_when_source_has_no_ddl() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE out_tbl(id NUMBER, c1 NUMBER);
CREATE PROCEDURE prc_main AS BEGIN
  INSERT INTO out_tbl(id, c1) SELECT id, c1 FROM ext_source;
END;
"#,
    );

    let out = lineage(&root, "out_tbl", "upstream", "tree");
    assert!(
        out.contains("[ref?]  table:ext_source"),
        "source without DDL columns must be marked unknown:\n{out}"
    );
    assert!(
        out.contains("unknown 1"),
        "summary must count unknown sources:\n{out}"
    );
}

#[test]
fn entity_view_omits_processes() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, WIDE_SQL);

    let out = lineage_extra(&root, "out_tbl", "upstream", "tree", &["--view", "entity"]);
    assert!(
        out.contains("[flow]  table:src_trade  (flow 10/12)"),
        "entity view keeps role + coverage but drops the process:\n{out}"
    );
    assert!(
        !out.contains("proc:"),
        "entity view must not show processes:\n{out}"
    );
}

#[test]
fn relation_view_lines_are_self_contained() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, WIDE_SQL);

    let out = lineage_extra(
        &root,
        "out_tbl",
        "upstream",
        "tree",
        &["--view", "relation"],
    );
    assert!(
        out.contains("table:src_trade ──[proc:prc_main [R]]──▶ table:out_tbl  [flow 10/12]"),
        "relation view must print self-contained source──process──▶target lines:\n{out}"
    );
    assert!(
        out.contains(
            "table:par_cfg ──[proc:prc_main [R]]──▶ table:out_tbl  [ref overlap 2/12]  [external]"
        ),
        "reference relationship must be marked:\n{out}"
    );
}

#[test]
fn grouped_view_groups_by_process() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, WIDE_SQL);

    let out = lineage_extra(&root, "out_tbl", "upstream", "tree", &["--view", "grouped"]);
    assert!(
        out.contains("── proc:prc_main [W:insert_select]   feeds 2"),
        "grouped view must group by the connecting process:\n{out}"
    );
    assert!(
        out.contains("transform: aggregate 0 · derived 0 · direct 12"),
        "grouped view must summarize the transformation by mapping kind:\n{out}"
    );
    assert!(
        out.contains("flow  src_trade (10/12)"),
        "grouped view must list flow sources with coverage:\n{out}"
    );
}

#[test]
fn json_has_numeric_role_and_coverage_fields() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, WIDE_SQL);

    let out = lineage(&root, "out_tbl", "upstream", "json");
    let json: serde_json::Value =
        serde_json::from_str(&out).expect("lineage --format json must emit valid JSON");

    let children = json["children"].as_array().unwrap();
    assert_eq!(children.len(), 2, "json children: {json}");
    let by_role: std::collections::HashMap<&str, &serde_json::Value> = children
        .iter()
        .map(|c| (c["role"].as_str().unwrap(), c))
        .collect();
    assert_eq!(by_role["flow"]["flow_overlap"], 10);
    assert_eq!(by_role["flow"]["flow_total"], 12);
    assert_eq!(by_role["flow"]["coverage_basis"], "ddl_columns");
    assert_eq!(by_role["ref"]["flow_overlap"], 2);
    assert_eq!(by_role["ref"]["coverage_basis"], "ddl_columns");
    // Neither source is written in-code in WIDE_SQL → both are terminal inputs.
    assert_eq!(by_role["flow"]["terminal_input"], true);
    assert_eq!(by_role["ref"]["terminal_input"], true);
}

/// A source with no in-code writers is a terminal input: it appears as `[external]` in
/// every view, reconciling its own empty upstream query with its role as a source.
#[test]
fn terminal_input_sources_are_marked_external() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, WIDE_SQL);

    let tree = lineage(&root, "out_tbl", "upstream", "tree");
    assert!(
        tree.contains("(flow 10/12)  [external]"),
        "flow terminal input (src_trade) must be marked external:\n{tree}"
    );
    assert!(
        tree.contains("(overlap 2/12)  [external]"),
        "reference terminal input (par_cfg) must be marked external:\n{tree}"
    );

    let entity = lineage_extra(&root, "out_tbl", "upstream", "tree", &["--view", "entity"]);
    assert!(
        entity.contains("(flow 10/12)  [external]"),
        "entity view must mark terminal inputs:\n{entity}"
    );
}

/// A view's base tables are not terminal inputs (views are derived, not external), and a
/// table written in-code is not marked external either.
#[test]
fn derived_entities_are_not_marked_external() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    // mid_tbl is written by prc_step1 → NOT a terminal input.
    let out = lineage(&root, "final_tbl", "upstream", "tree");
    let mid_line = out
        .lines()
        .find(|l| l.contains("table:mid_tbl"))
        .expect("mid_tbl must appear");
    assert!(
        !mid_line.contains("[external]"),
        "in-code-written tables must not be marked external: {mid_line}"
    );
    // source_tbl is never written in PIPELINE_SQL → legitimately a terminal input.
    let src_line = out
        .lines()
        .find(|l| l.contains("table:source_tbl"))
        .expect("source_tbl must appear");
    assert!(
        src_line.contains("[external]"),
        "never-written source must be marked external: {src_line}"
    );

    // A view node itself is never a terminal input (views are derived); its base table,
    // being a table with no in-code writers, legitimately is.
    let view_dir = TempDir::new().unwrap();
    let view_root = project_with_sql(
        &view_dir,
        r#"
CREATE TABLE base_tbl(id NUMBER, raw NUMBER);
CREATE VIEW v_derived AS SELECT id, raw * 2 AS doubled FROM base_tbl;
"#,
    );
    let view_up = lineage(&view_root, "v_derived", "upstream", "tree");
    let view_line = view_up
        .lines()
        .next()
        .expect("view upstream must have a header line");
    assert!(
        !view_line.contains("[external]"),
        "view nodes must never be marked external: {view_line}"
    );
    assert!(
        view_up.contains("(flow 2/2)  [external]"),
        "base table without in-code writers must be marked external:\n{view_up}"
    );
}

/// A target narrower than the absolute floor still classifies a fully-covering source as
/// flow: the floor is capped by the target's column count (issue #146 revision 1).
#[test]
fn narrow_target_can_still_classify_flow() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    // final_tbl has 2 columns; source_tbl supplies both via prc_step1/prc_step2.
    let out = lineage(&root, "final_tbl", "upstream", "tree");
    assert!(
        out.contains("[flow]  table:mid_tbl  ← proc:prc_step2 [R]") && out.contains("(flow 2/2)"),
        "narrow target must still classify a fully-covering source as flow:\n{out}"
    );
    assert!(
        out.contains("flow 1"),
        "narrow-target summary must count the flow source:\n{out}"
    );
}

// ── L1 statement-scoped hops (issue #147) ─────────────────────────────────────

/// A routine that reads table A in one statement and writes T in another must NOT
/// present A as upstream of T: only tables read in the SAME statement as the write are
/// connected. Before #147, every read of the routine was connected to every write.
#[test]
fn hops_are_restricted_to_same_statement_reads() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE a_t(id NUMBER, c1 NUMBER);
CREATE TABLE b_t(id NUMBER, c1 NUMBER);
CREATE TABLE t(id NUMBER, c1 NUMBER);
CREATE TABLE other_t(id NUMBER, c1 NUMBER);
CREATE PROCEDURE prc AS BEGIN
  INSERT INTO t(id, c1) SELECT id, c1 FROM a_t;       -- writes t, reads a_t
  INSERT INTO other_t(id, c1) SELECT id, c1 FROM b_t; -- unrelated statement
END;
"#,
    );

    let out = lineage(&root, "t", "upstream", "tree");
    assert!(
        out.contains("table:a_t"),
        "same-statement source must be present:\n{out}"
    );
    assert!(
        !out.contains("table:b_t"),
        "cross-statement read must NOT be connected:\n{out}"
    );
}

/// Downstream is symmetric: a child is downstream of the parent only when the child is
/// written in the same statement that reads the parent.
#[test]
fn downstream_hops_are_statement_scoped() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE parent_t(id NUMBER, c1 NUMBER);
CREATE TABLE child_t(id NUMBER, c1 NUMBER);
CREATE TABLE unrelated_t(id NUMBER, c1 NUMBER);
CREATE TABLE src_t(id NUMBER, c1 NUMBER);
CREATE PROCEDURE prc AS BEGIN
  INSERT INTO child_t(id, c1) SELECT id, c1 FROM parent_t; -- parent feeds child
  INSERT INTO unrelated_t(id, c1) SELECT id, c1 FROM src_t; -- unrelated
END;
"#,
    );

    let out = lineage(&root, "parent_t", "downstream", "tree");
    assert!(
        out.contains("table:child_t"),
        "same-statement child must be present:\n{out}"
    );
    assert!(
        !out.contains("table:unrelated_t"),
        "cross-statement write must NOT be downstream:\n{out}"
    );
}

/// The cursor → FETCH → INSERT VALUES chain survives statement splitting via the
/// procedure-level cursor context pre-pass: per-statement column analysis still
/// resolves FETCH variables back to the cursor's source columns.
#[test]
fn cursor_fetch_resolution_survives_statement_scoping() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(
        &dir,
        r#"
CREATE TABLE src_cursor(id NUMBER, amount NUMBER);
CREATE TABLE dst_cursor(id NUMBER, total NUMBER);
CREATE PROCEDURE prc_cursor AS
  CURSOR c IS SELECT id, amount FROM src_cursor;
  v_id NUMBER;
  v_amount NUMBER;
BEGIN
  OPEN c;
  LOOP
    FETCH c INTO v_id, v_amount;
    EXIT WHEN c%NOTFOUND;
    INSERT INTO dst_cursor(id, total) VALUES (v_id, v_amount);
  END LOOP;
  CLOSE c;
END;
"#,
    );

    // Table-level upstream of dst_cursor must include the cursor's source table.
    let tbl = lineage(&root, "dst_cursor", "upstream", "tree");
    assert!(
        tbl.contains("table:src_cursor"),
        "cursor source table must be upstream:\n{tbl}"
    );

    // Downstream is symmetric: the %ROWTYPE record chain must also surface the child.
    let down = lineage(&root, "src_cursor", "downstream", "tree");
    assert!(
        down.contains("table:dst_cursor"),
        "cursor-mediated child must be downstream:\n{down}"
    );
}

fn lineage_default(root: &Path, target: &str, format: &str) -> String {
    let out = run_codeweb_in(
        root,
        &[
            "lineage",
            target,
            "--format",
            format,
            "-p",
            root.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "lineage failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// The default direction is `both`: upstream and downstream are shown as labelled
/// sections in one run.
#[test]
fn default_direction_shows_upstream_and_downstream() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    // mid_tbl: upstream = source_tbl (prc_step1), downstream = final_tbl (prc_step2).
    let out = lineage_default(&root, "mid_tbl", "tree");
    assert!(
        out.contains("── upstream ──"),
        "both mode must label the upstream section:\n{out}"
    );
    assert!(
        out.contains("── downstream ──"),
        "both mode must label the downstream section:\n{out}"
    );
    assert!(
        out.contains("table:source_tbl"),
        "upstream section must list the source:\n{out}"
    );
    assert!(
        out.contains("table:final_tbl"),
        "downstream section must list the consumer:\n{out}"
    );
}

#[test]
fn default_direction_json_has_upstream_and_downstream_objects() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    let out = lineage_default(&root, "mid_tbl", "json");
    let json: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert!(json["upstream"].is_object(), "json both: {json}");
    assert!(json["downstream"].is_object(), "json both: {json}");
    assert!(
        json["downstream"]["children"]
            .as_array()
            .map_or(0, |c| c.len())
            >= 1,
        "downstream must list final_tbl:\n{json}"
    );
}

/// Explicit single directions still work and are unchanged.
#[test]
fn explicit_direction_is_unaffected_by_both_default() {
    let dir = TempDir::new().unwrap();
    let root = project_with_sql(&dir, PIPELINE_SQL);

    let up = lineage(&root, "mid_tbl", "upstream", "tree");
    assert!(
        !up.contains("── upstream ──"),
        "single direction has no section label:\n{up}"
    );
    assert!(up.contains("table:source_tbl"), "upstream:\n{up}");
    assert!(
        !up.contains("table:final_tbl"),
        "no downstream in upstream-only:\n{up}"
    );
}
