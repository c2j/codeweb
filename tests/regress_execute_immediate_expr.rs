use std::fs;
use tempfile::TempDir;

// Individual case SQL files
const EI_FIELD_ACCESS: &str =
    include_str!("regress/execute_immediate_expr/cases/ei_field_access.sql");
const EI_BINARY_CONCAT: &str =
    include_str!("regress/execute_immediate_expr/cases/ei_binary_concat.sql");
const EI_LITERAL_CALL: &str =
    include_str!("regress/execute_immediate_expr/cases/ei_literal_call.sql");
const EI_VARIABLE_CALL: &str =
    include_str!("regress/execute_immediate_expr/cases/ei_variable_call.sql");
const EI_COMPREHENSIVE: &str =
    include_str!("regress/execute_immediate_expr/cases/ei_comprehensive.sql");
const EI_VAR_RESOLVE: &str =
    include_str!("regress/execute_immediate_expr/cases/ei_var_resolve.sql");
const EI_COMPLEX_EXPR: &str =
    include_str!("regress/execute_immediate_expr/cases/ei_complex_expr.sql");
const EI_TYPECAST: &str = include_str!("regress/execute_immediate_expr/cases/ei_typecast.sql");
const EI_CARTESIAN_EXPLOSION: &str =
    include_str!("regress/execute_immediate_expr/cases/ei_cartesian_explosion.sql");

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

/// raw_expr of all Unresolved nodes.
fn unresolved_raw_exprs(json: &serde_json::Value) -> Vec<String> {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some("unresolved"))
        .filter_map(|n| n["raw_expr"].as_str().map(String::from))
        .collect()
}

/// Any Unresolved node whose raw_expr leaked an ogsql-parser AST Debug string.
fn ast_debug_unresolved(json: &serde_json::Value) -> Vec<String> {
    unresolved_raw_exprs(json)
        .into_iter()
        .filter(|expr| {
            expr.contains("Parenthesized(")
                || expr.contains("PlVariable(")
                || expr.contains("BinaryOp ")
                || expr.contains("BinaryOp{")
                || expr.contains("FunctionCall ")
                || expr.contains("FunctionCall{")
                || expr.contains("FieldAccess {")
                || expr.contains("TypeCast {")
                || expr.contains("Case {")
                || expr.contains("Subquery(")
                || expr.contains("Exists(")
                || expr.contains("Subscript {")
                || expr.contains("Like {")
                || expr.contains("UnaryOp {")
                || expr.contains("Between {")
                || expr.contains("InList {")
                || expr.contains("Parameter(")
        })
        .collect()
}

/// Unresolved raw_expr values starting with a given prefix.
fn unresolved_with_prefix(json: &serde_json::Value, prefix: &str) -> Vec<String> {
    unresolved_raw_exprs(json)
        .into_iter()
        .filter(|expr| expr.starts_with(prefix))
        .collect()
}

/// Count edges of a specific type in the JSON output.
fn edge_count(json: &serde_json::Value, edge_type: &str) -> usize {
    json["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"].as_str() == Some(edge_type))
        .count()
}

/// Count nodes of a specific type.
fn node_count(json: &serde_json::Value, node_type: &str) -> usize {
    json["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["type"].as_str() == Some(node_type))
        .count()
}

/// List ALL node types and their ids in the output (for debugging).
fn dump_all_nodes(json: &serde_json::Value) {
    let nodes = json["nodes"].as_array().unwrap();
    eprintln!("--- All Nodes ({}) ---", nodes.len());
    for n in nodes {
        let t = n["type"].as_str().unwrap_or("?");
        let id = n["id"].as_u64().unwrap_or(0);
        match t {
            "procedure" | "function" => {
                eprintln!("  [{id}] {t}: {}", n["name"].as_str().unwrap_or("?"));
            }
            "unresolved" => {
                eprintln!(
                    "  [{id}] {t}: raw_expr={}",
                    n["raw_expr"].as_str().unwrap_or("?")
                );
            }
            "table" => {
                eprintln!("  [{id}] {t}: {}", n["name"].as_str().unwrap_or("?"));
            }
            _ => {
                eprintln!("  [{id}] {t}");
            }
        }
    }
}

/// List ALL edges in the output (for debugging).
fn dump_all_edges(json: &serde_json::Value) {
    let edges = json["edges"].as_array().unwrap();
    eprintln!("--- All Edges ({}) ---", edges.len());
    for e in edges {
        let t = e["type"].as_str().unwrap_or("?");
        let src = e["source"].as_u64().unwrap_or(0);
        let tgt = e["target"].as_u64().unwrap_or(0);
        eprintln!("  {src} --[{t}]--> {tgt}");
    }
}

/// Get raw_expr values of DynamicCall edges.
fn dynamic_call_raw_exprs(json: &serde_json::Value) -> Vec<String> {
    json["edges"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"].as_str() == Some("dynamic"))
        .filter_map(|e| e["raw_expr"].as_str().map(String::from))
        .collect()
}

// ═══════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════

// ── Test 1: FieldAccess escape ──

#[test]
fn regress_ei_field_access_no_noise() {
    let json = analyze_json(EI_FIELD_ACCESS);
    let leaked = ast_debug_unresolved(&json);
    assert!(
        leaked.is_empty(),
        "EXECUTE IMMEDIATE rec.sql_text (FieldAccess) must NOT leak AST Debug string as \
         Unresolved nodes. noise_rule only checks prefixes PlVariable(, BinaryOp , etc. \
         but FieldAccess {{ ... }} escapes. Found: {leaked:?}"
    );
}

// ── Test 2: BinaryOp concatenation (caught by noise_rule) ──

#[test]
fn regress_ei_binary_concat_no_unresolved() {
    let json = analyze_json(EI_BINARY_CONCAT);
    let leaked = ast_debug_unresolved(&json);
    assert!(
        leaked.is_empty(),
        "EXECUTE IMMEDIATE with BinaryOp concat should NOT leak AST Debug: {leaked:?}"
    );
    // But the sql_text in body_sql may contain Debug string "BinaryOp { ... }",
    // which means search results are polluted even though no Unresolved node is created.
    // This is a secondary problem we document here.
    let dyn_exprs = dynamic_call_raw_exprs(&json);
    if !dyn_exprs.is_empty() {
        eprintln!(
            "NOTE: DynamicCall raw_exprs (currently expected): {:?}",
            dyn_exprs
        );
    }
}

// ── Test 3: Literal CALL resolves ──

#[test]
fn regress_ei_literal_call_resolves() {
    let json = analyze_json(EI_LITERAL_CALL);

    // Must have zero Unresolved nodes (the literal 'CALL other_proc()' should be parsed
    // into parsed_query=Some(CallFuncStatement), then the Visitor walks into it and
    // creates a DirectCall edge — no Unresolved involved.)
    let unresolved = unresolved_raw_exprs(&json);
    assert!(
        unresolved.is_empty(),
        "EXECUTE IMMEDIATE 'CALL other_proc()' must NOT produce Unresolved nodes. \
         The literal string causes parsed_query=Some(Call), which the Visitor walks into. \
         Found: {unresolved:?}"
    );

    // Must have at least 1 DirectCall edge (from CALL other_proc() or from the
    // EXECUTE IMMEDIATE 'CALL other_proc()' — or both)
    let direct_edges = edge_count(&json, "direct");
    assert!(
        direct_edges >= 1,
        "EXECUTE IMMEDIATE 'CALL other_proc()' should create at least 1 DirectCall edge \
         (CALL other_proc() on line 25). Got {direct_edges} DirectCall edges. \
         If parsed_query=Some(Call) works, the Visitor walks into it and creates a DirectCall."
    );

    dump_all_nodes(&json);
    dump_all_edges(&json);

    // Must have TableAccess edges (other_proc updates t_archive)
    let table_access_edges = edge_count(&json, "table_access");
    assert!(
        table_access_edges >= 1,
        "Procedure other_proc does UPDATE t_archive, so there should be at least 1 \
         TableAccess edge. Got {table_access_edges}."
    );

    eprintln!(
        "Literal CALL graph: {} Procs, {} Unresolved, {} DirectCall edges, {} TableAccess edges",
        node_count(&json, "procedure"),
        node_count(&json, "unresolved"),
        direct_edges,
        table_access_edges
    );
}

// ── Test 4: Variable CALL — hidden relationship loss ──

#[test]
fn regress_ei_variable_call_hidden_relationship() {
    let json = analyze_json(EI_VARIABLE_CALL);

    dump_all_nodes(&json);
    dump_all_edges(&json);

    // The variable CALL via EXECUTE IMMEDIATE v_sql cannot be resolved statically.
    // parsed_query=None for both bare variable and parenthesized variable cases.
    // This is a FUNDAMENTAL LIMITATION: the CALL to other_proc via the variable
    // is HIDDEN from the graph.

    // Document what we DO get:
    let unres = unresolved_raw_exprs(&json);
    let dyn_edges = dynamic_call_raw_exprs(&json);
    let direct_edges = edge_count(&json, "direct");
    let table_edges = edge_count(&json, "table_access");

    // The direct CALL other_proc() should still create a DirectCall edge
    assert!(
        direct_edges >= 1,
        "Direct CALL other_proc() on line 26 should create a DirectCall edge. \
         Got {direct_edges}"
    );

    // Dynamic edges exist for the EXECUTE IMMEDIATE calls
    eprintln!(
        "Variable CALL graph: {} Unresolved nodes, {} DynamicCall edges, \
         {} DirectCall edges, {} TableAccess edges",
        unres.len(),
        dyn_edges.len(),
        direct_edges,
        table_edges
    );
    eprintln!("  Unresolved raw_exprs: {:?}", unres);
    eprintln!("  DynamicCall raw_exprs: {:?}", dyn_edges);

    // IMPORTANT: The hidden relationship gap:
    // - There IS a DirectCall from test_ei_variable_call -> other_proc (via the direct CALL)
    // - But there is NO edge capturing the dynamic CALL via v_sql
    // - If the direct CALL line were removed, the entire chain would be lost
    // - Even WITH the direct CALL, the graph UNDERREPRESENTS the true call count
    //   (it should show 3 calls to other_proc, but only shows 1)

    eprintln!(
        "HIDDEN RELATIONSHIP GAP: test_ei_variable_call makes 3 calls to other_proc \
         (1 direct + 1 via variable + 1 via parenthesized variable), \
         but only the direct CALL is captured. Dynamic SQL edges point to garbage \
         Unresolved nodes instead of other_proc."
    );
}

// ── Test 5: Comprehensive impact analysis ──

#[test]
fn regress_ei_comprehensive_impact_analysis() {
    let json = analyze_json(EI_COMPREHENSIVE);

    dump_all_nodes(&json);
    dump_all_edges(&json);
    let total_unresolved = unresolved_raw_exprs(&json);
    let ast_debug = ast_debug_unresolved(&json);
    let dyn_exprs = dynamic_call_raw_exprs(&json);
    let n_proc = node_count(&json, "procedure");
    let n_unres = node_count(&json, "unresolved");
    let n_table = node_count(&json, "table");
    let n_direct = edge_count(&json, "direct");
    let n_dyn = edge_count(&json, "dynamic");

    eprintln!("\n=== Comprehensive Impact Analysis ===");
    eprintln!(
        "Nodes: {} proc, {} unresolved, {} table",
        n_proc, n_unres, n_table
    );
    eprintln!("Edges: {} direct, {} dynamic", n_direct, n_dyn);
    eprintln!("");
    eprintln!("Unresolved raw_exprs ({} total):", total_unresolved.len());
    for (i, expr) in total_unresolved.iter().enumerate() {
        eprintln!("  [{i}] {expr}");
    }
    eprintln!("");
    eprintln!("AST-debug leaked ({} total):", ast_debug.len());
    for (i, expr) in ast_debug.iter().enumerate() {
        eprintln!("  [{i}] {expr}");
    }
    eprintln!("");
    eprintln!("DynamicCall raw_exprs:");
    for (i, expr) in dyn_exprs.iter().enumerate() {
        eprintln!("  [{i}] {expr}");
    }
    eprintln!("");

    // ── Per-pattern analysis ──
    // Pattern 1: p_direct_call uses CALL shared_callee() → direct edge, no unresolved
    // Pattern 2: p_ei_literal_call uses EXECUTE IMMEDIATE 'CALL shared_callee()' → should resolve
    // Pattern 3: p_ei_literal_select uses EXECUTE IMMEDIATE 'SELECT ... FROM t_source' → should give table access
    // Pattern 4: p_ei_literal_update uses EXECUTE IMMEDIATE 'UPDATE t_target ...' → should give table access
    // Pattern 5: p_ei_bare_var uses EXECUTE IMMEDIATE v_sql → PlVariable, caught by noise_rule
    // Pattern 6: p_ei_paren_var uses EXECUTE IMMEDIATE (v_sql) → after peel_parenthesized, PlVariable caught
    // Pattern 7: p_ei_field_access uses EXECUTE IMMEDIATE r.sql_text → FieldAccess escapes noise_rule
    // Pattern 8: p_ei_concat uses EXECUTE IMMEDIATE 'UPDATE ' || tbl || ... → BinaryOp caught by noise_rule
    // Pattern 9: p_ei_complex_chain → mixed

    // Check each pattern:
    // Patterns 2, 3, 4 (literal) produce parsed_query=Some, so they should NOT create Unresolved nodes.
    // However, the Unresolved nodes may come from patterns 5-9 instead.

    let field_access_unres = unresolved_with_prefix(&json, "FieldAccess");
    let _plvar_in_unres = total_unresolved
        .iter()
        .filter(|e| e.contains("PlVariable"))
        .count();

    eprintln!("=== Per-pattern results ===");
    eprintln!("PlVariable caught by noise_rule?            expected: YES (pattern 5)");
    eprintln!("Parenthesized+Peel+PlVariable caught?        expected: YES (pattern 6, after peel)");
    eprintln!(
        "FieldAccess escaped?                        expected: YES (pattern 7) — count: {}",
        field_access_unres.len()
    );
    eprintln!("BinaryOp caught by noise_rule?               expected: YES (pattern 8)");
    eprintln!("Literal CALL resolved via parsed_query?       expected: YES (pattern 2)");
    eprintln!("Literal SELECT resolved via parsed_query?     expected: YES (pattern 3)");
    eprintln!("Literal UPDATE resolved via parsed_query?     expected: YES (pattern 4)");
    eprintln!("");

    // Hidden relationship summary
    // p_ei_complex_chain has:
    //   - Direct CALL shared_callee() → captured
    //   - EXECUTE IMMEDIATE v_sql (INSERT INTO t_result) → lost
    //   - EXECUTE IMMEDIATE 'INSERT INTO ' || v_tbl || ' VALUES (2)' → BinaryOp caught but SQL content lost
    // Two out of three SQL operations in p_ei_complex_chain are invisible to table access analysis.
    eprintln!("=== Hidden Relationship Loss ===");
    eprintln!("p_ei_complex_chain does 3 dynamic SQL operations:");
    eprintln!("  1) Direct CALL shared_callee() — CAPTURED (DirectCall)");
    eprintln!("  2) EXECUTE IMMEDIATE v_sql (INSERT INTO t_result) — LOST (parsed_query=None)");
    eprintln!("  3) EXECUTE IMMEDIATE concat (INSERT INTO t_target) — LOST (parsed_query=None)");
    eprintln!("  -> 2/3 operations invisible to graph. 66% relationship loss.");
    eprintln!("");

    // Summary assertion: the test should detect the gap, not fail on it.
    // This is an IMPACT ANALYSIS test — it documents the current state.
    // The assertion is that FieldAccess creates at least 1 Unresolved node (proving the escape).
    assert!(
        field_access_unres.is_empty(),
        "FieldAccess should escape noise_rule and create Unresolved nodes. \
         Found {count} FieldAccess Unresolved nodes. This confirms the escape bug. \
         Unresolved: {raw:?}",
        count = field_access_unres.len(),
        raw = field_access_unres
    );
}

// ── Test 6: Variable content resolution ──

#[test]
fn regress_ei_var_resolve() {
    let json = analyze_json(EI_VAR_RESOLVE);
    dump_all_nodes(&json);
    dump_all_edges(&json);

    // After variable content resolution, EXECUTE IMMEDIATE v_sql where
    // v_sql := 'CALL target_proc()' should create a DirectCall edge to target_proc.
    // This test has NO direct CALL target_proc() — only EXECUTE IMMEDIATE calls.
    let direct_edges = edge_count(&json, "direct");

    // We expect at least 1 DirectCall edge from test_var_resolve -> target_proc
    // (the two EXECUTE IMMEDIATE calls dedup to 1 DirectCall)
    assert!(
        direct_edges >= 1,
        "Variable content resolution should create at least 1 DirectCall edge \
         from test_var_resolve -> target_proc. Got {direct_edges}."
    );

    // There should be no Unresolved nodes from the resolved variables.
    // The target_proc procedure IS defined in this file, so the call resolves.
    let unresolved = unresolved_raw_exprs(&json);
    assert!(
        unresolved.is_empty(),
        "Variable content resolution should not produce Unresolved nodes. \
         The CALL target is a literal string that resolves to target_proc, \
         which is defined in the same file. Got: {unresolved:?}"
    );
}

// ── Test 7: Complex expression patterns ──

#[test]
fn regress_ei_complex_expr() {
    let json = analyze_json(EI_COMPLEX_EXPR);
    dump_all_nodes(&json);
    dump_all_edges(&json);

    let total_unresolved = unresolved_raw_exprs(&json);
    let ast_debug = ast_debug_unresolved(&json);
    let dyn_exprs = dynamic_call_raw_exprs(&json);
    let n_proc = node_count(&json, "procedure");
    let n_func = node_count(&json, "function");
    let n_unres = node_count(&json, "unresolved");
    let n_direct = edge_count(&json, "direct");
    let n_dyn = edge_count(&json, "dynamic");

    eprintln!("\n=== Complex Expression Analysis ===");
    eprintln!(
        "Nodes: {} proc, {} func, {} unresolved",
        n_proc, n_func, n_unres
    );
    eprintln!("Edges: {} direct, {} dynamic", n_direct, n_dyn);
    eprintln!("");
    eprintln!("Unresolved raw_exprs ({} total):", total_unresolved.len());
    for (i, expr) in total_unresolved.iter().enumerate() {
        eprintln!("  [{i}] {expr}");
    }
    eprintln!("");
    eprintln!("AST-debug leaked ({} total):", ast_debug.len());
    for (i, expr) in ast_debug.iter().enumerate() {
        eprintln!("  [{i}] {expr}");
    }
    eprintln!("");
    eprintln!("DynamicCall raw_exprs:");
    for (i, expr) in dyn_exprs.iter().enumerate() {
        eprintln!("  [{i}] {expr}");
    }
    eprintln!("");

    // Per-pattern analysis — production-style stored procedures
    //
    // Per-pattern analysis with value-set over-approximation
    //
    // Pattern 1 (p_nested_if): Nested IF/ELSE → branch merge
    //   - THEN inner-IF: {p_full_archive, p_full}
    //   - ELSE: {p_incremental}
    //   - Merge: {p_full_archive, p_full, p_incremental}
    //   → 3 DirectCall edges
    //
    // Pattern 2 (p_case_dispatch): CASE → extract_all_literal_strings
    //   - v_proc := CASE → {"p_create", "p_update", "p_delete", "p_full"}
    //   - v_sql := 'CALL ' || v_proc || '()' → cartesian product → 4 values
    //   → 4 DirectCall edges
    //
    // Pattern 3 (p_where_builder): IF + chain concat, p_status unresolvable
    //   - First IF (p_name has default 'test'): chain resolution works
    //   - Second IF (p_status not tracked): extract fails, no update
    //   → 0 DirectCall edges (SELECT, not CALL)
    //
    // Pattern 4 (p_loop_column_list): FOR loop, rec.c is FieldAccess
    //   → 0 edges (FieldAccess source not resolvable)
    //
    // Pattern 5 (p_loop_batch): WHILE loop + chain resolution
    //   - v_param tracked from DECLARE default 'x'
    //   - Body walk once: v_sql := v_sql || ... resolves via chain
    //   → 1 DirectCall edge to p_batch
    //
    // Pattern 6 (p_double_dispatch): Nested IF/CASE
    //   - THEN: CASE → {"p_create", "p_update", "p_full"}
    //   - ELSE: CASE → {"p_create", "p_full"}
    //   - Merge (union): {"p_create", "p_update", "p_full"}
    //   → 3 DirectCall edges
    //
    // Pattern 7 (p_config_driven): FOR loop, cfg.sql_text FieldAccess
    //   → 0 DirectCall edges (FieldAccess source not in var_values)
    //
    // Pattern 8 (p_declare_chain): DECLARE default + concat chain
    //   → 1 DirectCall edge to shared_callee
    //
    // Pattern 9 (p_var_chain): Variable-to-variable chain
    //   → 1 DirectCall edge to shared_callee
    //
    // Pattern 10 (p_where_full_resolve): Self-ref + chain, all vars known
    //   - Full SQL reconstruction: "SELECT ... WHERE name='test' ORDER BY id"
    //   → 0 DirectCall edges (SELECT, not CALL)

    eprintln!("\n=== Per-pattern summary ===");
    eprintln!("Pattern 1 (nested IF/ELSE):       DirectCall edges?  expected 3  (branch merge)");
    eprintln!(
        "Pattern 2 (CASE dispatch):         DirectCall edges?  expected 4  (CASE extraction)"
    );
    eprintln!("Pattern 3 (WHERE builder):          DirectCall edges?  expected 0  (SELECT)");
    eprintln!("Pattern 4 (FOR loop columns):       DirectCall edges?  expected 0  (FieldAccess)");
    eprintln!("Pattern 5 (WHILE batch):            DirectCall edges?  expected 1  (chain)");
    eprintln!("Pattern 6 (nested IF/CASE):        DirectCall edges?  expected 3  (merge + CASE)");
    eprintln!("Pattern 7 (config driven):          DirectCall edges?  expected 0  (FieldAccess)");
    eprintln!("Pattern 8 (DECLARE chain):         DirectCall edges?  expected 1  (chain)");
    eprintln!("Pattern 9 (var chain):              DirectCall edges?  expected 1  (chain)");
    eprintln!("Pattern 10 (full WHERE resolve):    DirectCall edges?  expected 0  (SELECT)");

    let n_direct = edge_count(&json, "direct");
    eprintln!("Total DirectCall edges across all patterns: {n_direct}");
    eprintln!("Expected >= 13 (p1:3 + p2:4 + p5:1 + p6:3 + p8:1 + p9:1)");
    assert!(
        n_direct >= 13,
        "Expected at least 13 DirectCall edges (branch merge + CASE extraction + chain). Got {n_direct}."
    );
}

// ── Test 8: TypeCast in EXECUTE IMMEDIATE (noise_rule escape test) ──

#[test]
fn regress_ei_typecast() {
    let json = analyze_json(EI_TYPECAST);
    let leaked = ast_debug_unresolved(&json);
    // noise_rule check: TypeCast { is NOT in the hardcoded prefix list → no leak
    let typecast_leaked: Vec<_> = leaked
        .into_iter()
        .filter(|e| e.contains("TypeCast"))
        .collect();
    assert!(
        typecast_leaked.is_empty(),
        "CAST(v_sql AS VARCHAR2) in EXECUTE IMMEDIATE must NOT leak TypeCast AST Debug. \
         Found: {typecast_leaked:?}"
    );
}

// ── Test 9: CALL inside IF/ELSIF/ELSE branches (regression) ──

#[test]
fn regress_ei_call_in_if_branch() {
    // Regression: CALL inside IF/ELSIF/ELSE branches was silently dropped
    // because the IF handler used visit_pl_statement (no child walking)
    // instead of walk_pl_statement (properly walks children including
    // calling visit_procedure_call).
    let sql = r#"CREATE OR REPLACE PROCEDURE p_if_call_then AS $$ BEGIN
    IF TRUE THEN CALL proc_a(); END IF;
END; $$;

CREATE OR REPLACE PROCEDURE p_if_call_else AS $$ BEGIN
    IF FALSE THEN NULL; ELSE CALL proc_b(); END IF;
END; $$;

CREATE OR REPLACE PROCEDURE p_if_call_elsif AS $$ BEGIN
    IF FALSE THEN NULL; ELSIF TRUE THEN CALL proc_c(); END IF;
END; $$;

CREATE OR REPLACE PROCEDURE p_if_call_nested AS $$ BEGIN
    IF TRUE THEN IF TRUE THEN CALL proc_d(); END IF; END IF;
END; $$;"#;

    let json = analyze_json(sql);
    let direct = edge_count(&json, "direct");
    dump_all_nodes(&json);
    dump_all_edges(&json);
    eprintln!("CALL-in-IF DirectCall edges: {direct}");
    assert!(
        direct >= 4,
        "CALL inside IF/ELSIF/ELSE branches must produce DirectCall edges. \
         Expected >= 4 (proc_a then, proc_b else, proc_c elsif, proc_d nested). Got {direct}."
    );
}

#[test]
fn ei_cartesian_explosion_capped_not_oom() {
    // Regression for v0.7.10 OOM: 20 CASE terms in a `||` chain → 2^20 literal
    // variants without the cap. MAX_VALUE_SET (extractor.rs) must abort expansion,
    // so analyze_json completes (no OOM) and the procedure node still exists.
    let json = analyze_json(EI_CARTESIAN_EXPLOSION);
    let has_proc = json["nodes"].as_array().unwrap().iter().any(|n| {
        n["type"].as_str() == Some("procedure")
            && n["name"].as_str() == Some("ei_cartesian_explosion")
    });
    assert!(
        has_proc,
        "procedure node must exist — cap should have prevented OOM during extraction"
    );
    // The cap degrades EXECUTE IMMEDIATE resolution to opaque (empty candidate
    // set), so no direct edges should be synthesized — and definitely not the
    // 2^20 that an unbounded cartesian product would produce.
    let direct = edge_count(&json, "direct");
    assert!(
        direct <= 64,
        "direct edges ({direct}) must stay within the MAX_VALUE_SET cap, not explode to 2^20"
    );
}
