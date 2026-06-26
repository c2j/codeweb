use std::fs;
use tempfile::TempDir;

const EXECUTE_IMMEDIATE_PAREN_PLVAR: &str =
    include_str!("regress/execute_immediate_paren_plvar/cases/execute_immediate_paren_plvar.sql");
const EXECUTE_IMMEDIATE_BARE_PLVAR: &str =
    include_str!("regress/execute_immediate_paren_plvar/cases/execute_immediate_bare_plvar.sql");

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
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout).expect("failed to parse JSON output")
}

/// `raw_expr` of all Unresolved nodes — each spurious dynamic-SQL noise string
/// that survives `noise_rule` surfaces here.
fn unresolved_raw_exprs(json: &serde_json::Value) -> Vec<String> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some("unresolved"))
        .filter_map(|n| n["raw_expr"].as_str().map(String::from))
        .collect()
}

/// Any Unresolved node whose `raw_expr` leaked an ogsql-parser AST Debug string
/// (e.g. `Parenthesized(...)`, `PlVariable(...)`, `BinaryOp ...`).
fn ast_debug_unresolved(json: &serde_json::Value) -> Vec<String> {
    unresolved_raw_exprs(json)
        .into_iter()
        .filter(|expr| {
            expr.contains("Parenthesized(")
                || expr.contains("PlVariable(")
                || expr.contains("BinaryOp ")
                || expr.contains("FunctionCall ")
        })
        .collect()
}

#[test]
fn regress_execute_immediate_bare_plvar_no_noise() {
    let json = analyze_json(EXECUTE_IMMEDIATE_BARE_PLVAR);
    let leaked = ast_debug_unresolved(&json);
    assert!(
        leaked.is_empty(),
        "EXECUTE IMMEDIATE v_sql3 (bare, no parens) must NOT leak AST debug strings as \
         Unresolved nodes; found: {leaked:?}"
    );
}

#[test]
fn regress_execute_immediate_paren_plvar_leaks_parenthesized_noise() {
    let json = analyze_json(EXECUTE_IMMEDIATE_PAREN_PLVAR);
    let leaked = ast_debug_unresolved(&json);
    assert!(
        leaked.is_empty(),
        "EXECUTE IMMEDIATE (v_sql3) — parenthesized PL variable — must NOT leak the AST \
         Debug string `Parenthesized(PlVariable([...]))` as an Unresolved node.\n\n\
         Root cause: extractor.rs:340 formats the non-literal EXECUTE IMMEDIATE target via \
         `format!(\"{{:?}}\", string_expr)`, and noise_rule (builder.rs:2795) only matches \
         `starts_with(\"PlVariable(\")`, missing the `Parenthesized(` wrapper.\n\n\
         Found leaked AST debug node(s): {leaked:?}"
    );
}
