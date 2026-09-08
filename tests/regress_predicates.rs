//! #167: standalone PL IF/CASE predicate JSON surface.

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
            let candidate = entry.path().join("debug").join(bin_name);
            if candidate.exists() {
                return candidate;
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

#[test]
fn predicates_json_reports_main_and_parameter_conditions_after_store_round_trip() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("step3.sql"),
        r#"
CREATE TABLE mid_yjqs_detail(stock_kind VARCHAR(10), zqdm VARCHAR(20));
CREATE TABLE swh_all_kind(kind_id VARCHAR(10), operation_kind VARCHAR(40));
CREATE PROCEDURE star_market_jsf AS
  CURSOR c_get_data IS SELECT stock_kind, zqdm FROM mid_yjqs_detail;
  r_get_data c_get_data%ROWTYPE;
  v_kind VARCHAR(10);
BEGIN
  SELECT kind_id INTO v_kind FROM swh_all_kind
   WHERE operation_kind = 'COMMISSION_SWITCH';
  IF r_get_data.stock_kind = '0100' AND
     r_get_data.zqdm BETWEEN '609100' AND '609999' THEN NULL; END IF;
  IF v_kind = '1' THEN NULL; END IF;
END;
"#,
    )
    .unwrap();
    let init = run_codeweb_in(
        dir.path(),
        &["init", "predicate-project", "--dir", src.to_str().unwrap()],
    );
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let output = run_codeweb_in(
        dir.path(),
        &[
            "predicates",
            "--procedure",
            "star_market_jsf",
            "--format",
            "json",
            "-p",
            dir.path().to_str().unwrap(),
        ],
    );
    assert!(
        output.status.success(),
        "predicates failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["procedure"], "star_market_jsf");
    let predicates = json["predicates"].as_array().unwrap();
    assert_eq!(predicates.len(), 2);
    let star = predicates
        .iter()
        .find(|predicate| predicate["confidence"] == "High")
        .expect("high-confidence star-market predicate");
    assert_eq!(star["table_predicate"]["table"], "mid_yjqs_detail");
    assert_eq!(
        star["table_predicate"]["clauses"].as_array().unwrap().len(),
        2
    );
    let parameter = predicates
        .iter()
        .find(|predicate| predicate["param_table_hint"].is_object())
        .expect("parameter-table hint");
    assert_eq!(parameter["confidence"], "Low");
    assert_eq!(parameter["param_table_hint"]["table"], "swh_all_kind");
    assert!(parameter["table_predicate"].is_null());
}

/// Review Finding 1 (#167): the procedure is declared UPPERCASE in source but
/// predicates must still resolve via a lowercase query, end-to-end, proving
/// the storage key and the `NodeKey::from_node` lookup key agree.
#[test]
fn predicates_resolve_case_insensitively_for_uppercase_procedure() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("step3_upper.sql"),
        r#"
CREATE TABLE mid_yjqs_detail(stock_kind VARCHAR(10));
CREATE PROCEDURE PRC_STAR_MARKET AS
  CURSOR c_get_data IS SELECT stock_kind FROM mid_yjqs_detail;
  r_get_data c_get_data%ROWTYPE;
BEGIN
  IF r_get_data.stock_kind = '0100' THEN NULL; END IF;
END;
"#,
    )
    .unwrap();
    let init = run_codeweb_in(
        dir.path(),
        &[
            "init",
            "predicate-upper-project",
            "--dir",
            src.to_str().unwrap(),
        ],
    );
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let output = run_codeweb_in(
        dir.path(),
        &[
            "predicates",
            "--procedure",
            "prc_star_market",
            "--format",
            "json",
            "-p",
            dir.path().to_str().unwrap(),
        ],
    );
    assert!(
        output.status.success(),
        "predicates failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["procedure"], "prc_star_market");
    let predicates = json["predicates"].as_array().unwrap();
    assert_eq!(predicates.len(), 1);
    assert_eq!(predicates[0]["confidence"], "High");
    assert_eq!(predicates[0]["table_predicate"]["table"], "mid_yjqs_detail");
}

#[test]
fn predicates_json_includes_elsif_and_simple_case_comparisons() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("branches.sql"),
        r#"
CREATE TABLE main_data(x VARCHAR(10));
CREATE PROCEDURE branch_predicates AS
  CURSOR c_data IS SELECT x FROM main_data;
  r c_data%ROWTYPE;
BEGIN
  IF r.x = '1' THEN NULL;
  ELSIF r.x = '2' THEN NULL;
  ELSIF r.x = '3' THEN NULL;
  END IF;
  CASE r.x
    WHEN '4' THEN NULL;
    WHEN '5' THEN NULL;
  END CASE;
END;
"#,
    )
    .unwrap();
    let init = run_codeweb_in(
        dir.path(),
        &["init", "predicate-branches", "--dir", src.to_str().unwrap()],
    );
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let output = run_codeweb_in(
        dir.path(),
        &[
            "predicates",
            "--procedure",
            "branch_predicates",
            "--format",
            "json",
            "-p",
            dir.path().to_str().unwrap(),
        ],
    );
    assert!(
        output.status.success(),
        "predicates failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let predicates = json["predicates"].as_array().unwrap();
    assert_eq!(predicates.len(), 5);
    assert_eq!(predicates[1]["kind"], "If");
    assert_eq!(predicates[2]["kind"], "If");
    assert_eq!(predicates[3]["kind"], "CaseWhen");
    assert_eq!(predicates[3]["confidence"], "High");
    assert_eq!(predicates[3]["table_predicate"]["table"], "main_data");
    assert_eq!(
        predicates[3]["table_predicate"]["clauses"][0]["column"],
        "x"
    );
    assert_eq!(
        predicates[3]["table_predicate"]["clauses"][0]["value"]["String"],
        "4"
    );
}

#[test]
fn predicates_identity_matches_columns_for_packaged_procedure() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("package.sql"),
        r#"
CREATE TABLE main_data(x VARCHAR(10));
CREATE OR REPLACE PACKAGE BODY pkg_predicates AS
  PROCEDURE check_value IS
    CURSOR c_data IS SELECT x FROM main_data;
    r c_data%ROWTYPE;
  BEGIN
    IF r.x = '1' THEN NULL; END IF;
  END;
END pkg_predicates;
"#,
    )
    .unwrap();
    let init = run_codeweb_in(
        dir.path(),
        &["init", "predicate-package", "--dir", src.to_str().unwrap()],
    );
    assert!(
        init.status.success(),
        "init failed: {}",
        String::from_utf8_lossy(&init.stderr)
    );

    let project = dir.path().to_str().unwrap();
    let columns = run_codeweb_in(
        dir.path(),
        &[
            "columns",
            "--procedure",
            "check_value",
            "--format",
            "json",
            "-p",
            project,
        ],
    );
    let predicates = run_codeweb_in(
        dir.path(),
        &[
            "predicates",
            "--procedure",
            "check_value",
            "--format",
            "json",
            "-p",
            project,
        ],
    );
    assert!(
        columns.status.success(),
        "columns failed: {}",
        String::from_utf8_lossy(&columns.stderr)
    );
    assert!(
        predicates.status.success(),
        "predicates failed: {}",
        String::from_utf8_lossy(&predicates.stderr)
    );
    let columns_json: serde_json::Value = serde_json::from_slice(&columns.stdout).unwrap();
    let predicates_json: serde_json::Value = serde_json::from_slice(&predicates.stdout).unwrap();
    assert_eq!(predicates_json["procedure"], columns_json["procedure"]);
    assert_eq!(predicates_json["package"], columns_json["package"]);
    assert_eq!(predicates_json["procedure"], "check_value");
    assert_eq!(predicates_json["package"], "pkg_predicates");
}

#[test]
fn predicates_ambiguous_substring_fails_explicitly() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("ambiguous.sql"),
        r#"
CREATE PROCEDURE prc_order AS BEGIN NULL; END;
CREATE PROCEDURE prc_order_header AS BEGIN NULL; END;
"#,
    )
    .unwrap();
    let init = run_codeweb_in(
        dir.path(),
        &[
            "init",
            "predicate-ambiguous",
            "--dir",
            src.to_str().unwrap(),
        ],
    );
    assert!(init.status.success());

    let output = run_codeweb_in(
        dir.path(),
        &[
            "predicates",
            "--procedure",
            "prc_order",
            "--format",
            "json",
            "-p",
            dir.path().to_str().unwrap(),
        ],
    );

    assert!(!output.status.success(), "ambiguous query should fail");
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .to_lowercase()
            .contains("ambiguous"),
        "stderr should explain ambiguity: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn predicates_empty_branches_return_empty_array() {
    let dir = TempDir::new().unwrap();
    let src = dir.path().join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("empty.sql"),
        "CREATE PROCEDURE no_branches AS BEGIN NULL; END;",
    )
    .unwrap();
    let init = run_codeweb_in(
        dir.path(),
        &["init", "predicate-empty", "--dir", src.to_str().unwrap()],
    );
    assert!(init.status.success());

    let output = run_codeweb_in(
        dir.path(),
        &[
            "predicates",
            "--procedure",
            "no_branches",
            "--format",
            "json",
            "-p",
            dir.path().to_str().unwrap(),
        ],
    );

    assert!(
        output.status.success(),
        "resolved empty predicates failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["procedure"], "no_branches");
    assert_eq!(json["predicates"], serde_json::json!([]));
}
