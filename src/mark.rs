use std::collections::{HashMap, HashSet};
use std::path::Path;

use petgraph::graph::NodeIndex;
use petgraph::Direction;

use crate::error::Result;
use crate::graph::key::NodeKey;
use crate::graph::store::GraphStore;
use crate::graph::Node;

/// Build the incoming transitive closure of `target` and record ALL parent relationships
/// for path reconstruction. Returns (visited_set, parent_map).
/// parent[node] = all nodes through which `node` was discovered (one step closer to target).
pub fn build_callers_set(
    store: &GraphStore,
    target: NodeIndex,
) -> (HashSet<NodeIndex>, HashMap<NodeIndex, Vec<NodeIndex>>) {
    let graph = store.graph();
    let mut visited = HashSet::new();
    let mut parent: HashMap<NodeIndex, Vec<NodeIndex>> = HashMap::new();
    let mut frontier = vec![target];
    visited.insert(target);

    while let Some(node) = frontier.pop() {
        for neighbor in graph.neighbors_directed(node, Direction::Incoming) {
            parent.entry(neighbor).or_default().push(node);
            if visited.insert(neighbor) {
                frontier.push(neighbor);
            }
        }
    }
    (visited, parent)
}

/// Reconstruct all caller paths from `caller_idx` back to `target_idx` using parent map.
/// Returns paths joined by " | ", e.g. "proc:A → table:T | proc:A → proc:B → table:T".
fn build_all_caller_paths(
    caller_idx: NodeIndex,
    target_idx: NodeIndex,
    parent: &HashMap<NodeIndex, Vec<NodeIndex>>,
    store: &GraphStore,
) -> String {
    let graph = store.graph();
    let mut all_paths: Vec<String> = Vec::new();
    let mut current_path: Vec<String> = vec![NodeKey::from_node(&graph[caller_idx]).to_string()];
    let mut visited_in_path: HashSet<NodeIndex> = HashSet::new();
    visited_in_path.insert(caller_idx);

    backtrack_paths(
        caller_idx,
        target_idx,
        parent,
        graph,
        &mut current_path,
        &mut visited_in_path,
        &mut all_paths,
    );

    if all_paths.is_empty() {
        // No parent chain found — just the caller itself
        return current_path[0].clone();
    }
    all_paths.join(" | ")
}

fn backtrack_paths(
    current: NodeIndex,
    target: NodeIndex,
    parent: &HashMap<NodeIndex, Vec<NodeIndex>>,
    graph: &crate::graph::CodeGraph,
    current_path: &mut Vec<String>,
    visited_in_path: &mut HashSet<NodeIndex>,
    all_paths: &mut Vec<String>,
) {
    if current == target {
        // Reached target — record a reversed copy of the path
        let path_str = current_path.join(" → ");
        all_paths.push(path_str);
        return;
    }

    if let Some(parents) = parent.get(&current) {
        for &p in parents {
            if visited_in_path.contains(&p) {
                continue; // cycle guard
            }
            let node_key = NodeKey::from_node(&graph[p]).to_string();
            current_path.push(node_key);
            visited_in_path.insert(p);
            backtrack_paths(p, target, parent, graph, current_path, visited_in_path, all_paths);
            visited_in_path.remove(&p);
            current_path.pop();
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MatchType {
    Direct,
    Indirect,
    None,
}

#[derive(Debug, Clone)]
pub struct MatchResult {
    pub match_type: MatchType,
    pub matched_by: Option<String>,
}

impl MatchResult {
    pub fn csv_values(&self, target_name: &str) -> (String, String) {
        let match_str = match self.match_type {
            MatchType::Direct => format!("direct to {}", target_name),
            MatchType::Indirect => format!("indirect to {}", target_name),
            MatchType::None => String::new(),
        };
        let by_str = self.matched_by.clone().unwrap_or_default();
        (match_str, by_str)
    }
}

/// Classify a single CSV row's `sql_text` against a target node and its callers set.
///
/// Priority: direct > indirect (name) > indirect (fingerprint).
/// Returns the first match found.
pub fn classify_row(
    sql_text: &str,
    target_idx: NodeIndex,
    target_node: &Node,
    callers: &HashSet<NodeIndex>,
    parent: &HashMap<NodeIndex, Vec<NodeIndex>>,
    store: &GraphStore,
) -> MatchResult {
    // --- Direct match ---
    match target_node {
        Node::Table { name, .. } | Node::View { name, .. } => {
            if contains_table_name(sql_text, name) {
                let path =
                    build_all_caller_paths(target_idx, target_idx, parent, store);
                return MatchResult {
                    match_type: MatchType::Direct,
                    matched_by: Some(path),
                };
            }
        }
        Node::Procedure { id, .. } | Node::Function { id, .. } => {
            if contains_routine_call(sql_text, &id.name) {
                let path =
                    build_all_caller_paths(target_idx, target_idx, parent, store);
                return MatchResult {
                    match_type: MatchType::Direct,
                    matched_by: Some(path),
                };
            }
        }
        _ => {}
    }

    // --- Indirect match: iterate callers set ---
    let graph = store.graph();
    for &caller_idx in callers {
        if caller_idx == target_idx {
            continue;
        }
        let caller_node = &graph[caller_idx];

        // 1) Name match
        if let Some(routine_name) = try_extract_routine_name(caller_node) {
            if contains_routine_call(sql_text, routine_name) {
                let path =
                    build_all_caller_paths(caller_idx, target_idx, parent, store);
                return MatchResult {
                    match_type: MatchType::Indirect,
                    matched_by: Some(path),
                };
            }
        }

        // 2) Fingerprint match
        if let Some(body_sql) = try_extract_body_sql(caller_node) {
            if sql_text_fingerprint_matches(sql_text, body_sql) {
                let path =
                    build_all_caller_paths(caller_idx, target_idx, parent, store);
                return MatchResult {
                    match_type: MatchType::Indirect,
                    matched_by: Some(path),
                };
            }
        }
    }

    MatchResult {
        match_type: MatchType::None,
        matched_by: None,
    }
}

/// Case-insensitive token-level match: ensures `orders` matches `SELECT * FROM orders`
/// but NOT `SELECT * FROM customer_orders`.
fn contains_table_name(sql_text: &str, table_name: &str) -> bool {
    let lower_sql = sql_text.to_lowercase();
    let lower_name = table_name.to_lowercase();
    for token in sql_tokens(&lower_sql) {
        if token == lower_name {
            return true;
        }
    }
    false
}

/// Detect a routine call in SQL text (CALL / EXECUTE / EXEC / SELECT func(...)).
fn contains_routine_call(sql_text: &str, routine_name: &str) -> bool {
    let lower_sql = sql_text.to_lowercase();
    let lower_name = routine_name.to_lowercase();

    // Helper: check if the routine name appears as a bare name or
    // qualified name (token ending with ".name") after a CALL/EXEC/etc.
    let name_matches = |prefix: &str| -> bool {
        // exact: "CALL name" or "CALL name("
        let bare = format!("{}{}", prefix, lower_name);
        if lower_sql.contains(&bare) {
            let after = &lower_sql[lower_sql.find(&bare).unwrap() + bare.len()..];
            if after.is_empty()
                || after.starts_with('(')
                || after.starts_with(' ')
                || after.starts_with(';')
            {
                return true;
            }
        }
        // qualified: "CALL pkg.name" — check if any token ends with ".name"
        let dot_name = format!(".{}", lower_name);
        for token in sql_tokens(&lower_sql) {
            if token.ends_with(&dot_name) {
                // Verify it's preceded by the call prefix
                if let Some(pos) = lower_sql.find(token) {
                    let before = &lower_sql[..pos].trim_end();
                    if before.ends_with(prefix.trim_end()) {
                        return true;
                    }
                }
            }
        }
        false
    };

    if name_matches("call ") {
        return true;
    }
    if name_matches("execute ") || name_matches("exec ") {
        return true;
    }
    if name_matches("perform ") {
        return true;
    }
    // SELECT func_name(...) — only match if func_name is followed by '('
    let with_paren = format!("{}(", lower_name);
    if lower_sql.contains(&with_paren) {
        // Check it's after SELECT/select
        if lower_sql.contains(&format!("select {}", lower_name))
            || lower_sql.contains(&format!("select {}", with_paren))
        {
            return true;
        }
        // Also check qualified: "select pkg.func_name("
        let dot_paren = format!(".{}(", lower_name);
        if lower_sql.contains(&dot_paren) {
            return true;
        }
    }

    false
}

/// Split SQL text into tokens by non-identifier characters.
fn sql_tokens(sql: &str) -> Vec<&str> {
    sql.split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| !t.is_empty())
        .collect()
}

/// Try to extract the routine name from a Node (Procedure or Function).
fn try_extract_routine_name(node: &Node) -> Option<&str> {
    match node {
        Node::Procedure { id, .. } | Node::Function { id, .. } => Some(&id.name),
        _ => None,
    }
}

/// Try to extract body_sql from a Node for fingerprint matching.
fn try_extract_body_sql(node: &Node) -> Option<&str> {
    match node {
        Node::Procedure { body_sql, .. } | Node::Function { body_sql, .. } => {
            body_sql.first().map(|bs| bs.sql_text.as_str())
        }
        _ => None,
    }
}

/// Check if `sql_text` matches `body_sql` after normalization.
/// Uses the same normalize_for_matching logic as the store's search_by_sql.
fn sql_text_fingerprint_matches(sql_text: &str, body_sql: &str) -> bool {
    let norm_sql = crate::graph::store::normalize_for_matching(&sql_text.to_lowercase());
    let norm_body = crate::graph::store::normalize_for_matching(&body_sql.to_lowercase());

    // Body SQL is typically short (single DML statement). Check if the
    // normalized body appears as a substring of the normalized CSV SQL.
    if norm_body.len() >= 10 && norm_sql.contains(&norm_body) {
        return true;
    }
    // Also check if the normalized SQL appears within the body (less likely but cheap)
    if norm_sql.len() >= 10 && norm_body.contains(&norm_sql) {
        return true;
    }

    false
}

/// Normalize a CSV header name to a canonical form for matching:
/// lowercase, underscores → spaces, collapse whitespace.
/// "Unique SQL Id" → "unique sql id"
/// "unique_sql_id"   → "unique sql id"
fn normalize_header(h: &str) -> String {
    h.trim()
        .to_lowercase()
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Pre-resolved target with its callers set and parent map.
struct TargetContext {
    name: String,          // the --node argument as typed by user
    node_idx: NodeIndex,
    node: Node,
    callers: HashSet<NodeIndex>,
    parent: HashMap<NodeIndex, Vec<NodeIndex>>,
}

/// Entry point: read CSV, classify each row against all targets, write annotated CSV.
pub fn process_mark(
    store: &GraphStore,
    node_names: &[String],
    csv_path: &Path,
    output_path: Option<&Path>,
) -> Result<()> {
    // 1) Resolve all targets. Unresolved targets still get columns (with empty results).
    let targets: Vec<TargetContext> = node_names
        .iter()
        .map(|name| {
            let matches = store.search_nodes(name);
            if matches.is_empty() {
                eprintln!("No nodes matching '{}'", name);
                eprintln!(
                    "Try `codeweb nodes -s {}` to find available nodes.",
                    name
                );
                TargetContext {
                    name: name.clone(),
                    node_idx: NodeIndex::end(), // dummy, never used
                    node: Node::Unresolved {
                        raw_expr: Box::new(String::new()),
                        context: Box::new(String::new()),
                    },
                    callers: HashSet::new(),
                    parent: HashMap::new(),
                }
            } else {
                if matches.len() > 1 {
                    eprintln!("Multiple nodes match '{}':", name);
                    for (i, (_, n)) in matches.iter().enumerate() {
                        eprintln!("  {}: {}", i + 1, n);
                    }
                    eprintln!("Using first match: {}", matches[0].1);
                }
                let (idx, _) = &matches[0];
                let (callers, parent) = build_callers_set(store, *idx);
                TargetContext {
                    name: name.clone(),
                    node_idx: *idx,
                    node: store.graph()[*idx].clone(),
                    callers,
                    parent,
                }
            }
        })
        .collect();

    // 2) Read CSV
    let csv_content =
        std::fs::read_to_string(csv_path).map_err(|e| crate::error::CodeWebError::FileRead {
            path: csv_path.to_path_buf(),
            source: e,
        })?;

    let mut reader = csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_reader(csv_content.as_bytes());

    let headers: Vec<String> = reader
        .headers()
        .map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("CSV header error: {}", e),
        })?
        .iter()
        .map(|s| s.to_string())
        .collect();

    // Validate required columns
    let has_sql_id = headers
        .iter()
        .any(|h| normalize_header(h) == "unique sql id");
    let has_sql_text = headers
        .iter()
        .any(|h| normalize_header(h) == "sql text");
    if !has_sql_id || !has_sql_text {
        return Err(crate::error::CodeWebError::ExportError {
            message: format!(
                "CSV missing required columns. Expected: Unique SQL Id (or unique_sql_id), SQL Text (or sql_text). Found: {}",
                headers.join(", ")
            ),
        });
    }

    // 3) Classify each row against all targets
    // Each row produces ONE output row. Per-target results go into target-specific columns.
    let mut output_rows: Vec<(Vec<String>, Vec<Option<MatchResult>>)> = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("CSV parse error: {}", e),
        })?;
        let fields: Vec<String> = record.iter().map(|s| s.to_string()).collect();

        let sql_text = fields
            .iter()
            .zip(headers.iter())
            .find(|(_, h)| normalize_header(h) == "sql text")
            .map(|(v, _)| v.as_str())
            .unwrap_or("");

        let results: Vec<Option<MatchResult>> = targets
            .iter()
            .map(|t| {
                let mr = classify_row(
                    sql_text, t.node_idx, &t.node, &t.callers, &t.parent, store,
                );
                if mr.match_type == MatchType::None {
                    None
                } else {
                    Some(mr)
                }
            })
            .collect();

        output_rows.push((fields, results));
    }

    // 4) Write output
    let writer: Box<dyn std::io::Write> = if let Some(out_path) = output_path {
        let file =
            std::fs::File::create(out_path).map_err(|e| crate::error::CodeWebError::FileRead {
                path: out_path.to_path_buf(),
                source: e,
            })?;
        Box::new(file)
    } else {
        Box::new(std::io::stdout())
    };
    let mut wtr = csv::WriterBuilder::new().from_writer(writer);

    // Header: original + per-target columns
    let mut out_headers = headers.clone();
    for t in &targets {
        out_headers.push(format!("codeweb_match_{}", t.name));
        out_headers.push(format!("codeweb_matched_by_{}", t.name));
    }
    wtr.write_record(&out_headers)
        .map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("CSV write error: {}", e),
        })?;

    // Rows
    for (fields, results) in &output_rows {
        let mut row = fields.clone();
        for (i, result) in results.iter().enumerate() {
            let t = &targets[i];
            let (match_str, by_str) = match result {
                Some(mr) => mr.csv_values(&t.name),
                None => (String::new(), String::new()),
            };
            row.push(match_str);
            row.push(by_str);
        }
        wtr.write_record(&row)
            .map_err(|e| crate::error::CodeWebError::ExportError {
                message: format!("CSV write error: {}", e),
            })?;
    }
    wtr.flush()
        .map_err(|e| crate::error::CodeWebError::ExportError {
            message: format!("CSV flush error: {}", e),
        })?;

    if let Some(out_path) = output_path {
        eprintln!("Output written to {}", out_path.display());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contains_table_name_exact_match() {
        assert!(contains_table_name(
            "SELECT * FROM orders WHERE id = 1",
            "orders"
        ));
    }

    #[test]
    fn test_contains_table_name_case_insensitive() {
        assert!(contains_table_name("SELECT * FROM ORDERS", "orders"));
        assert!(contains_table_name("select * from orders", "ORDERS"));
    }

    #[test]
    fn test_contains_table_name_no_substring_false_positive() {
        // "orders" should NOT match inside "customer_orders"
        assert!(!contains_table_name(
            "SELECT * FROM customer_orders",
            "orders"
        ));
    }

    #[test]
    fn test_contains_table_name_in_insert() {
        assert!(contains_table_name(
            "INSERT INTO orders (id) VALUES (1)",
            "orders"
        ));
    }

    #[test]
    fn test_contains_routine_call_call() {
        assert!(contains_routine_call(
            "CALL update_orders()",
            "update_orders"
        ));
    }

    #[test]
    fn test_contains_routine_call_call_no_parens() {
        assert!(contains_routine_call("CALL update_orders", "update_orders"));
    }

    #[test]
    fn test_contains_routine_call_execute() {
        assert!(contains_routine_call(
            "EXECUTE update_orders(1)",
            "update_orders"
        ));
    }

    #[test]
    fn test_contains_routine_call_exec() {
        assert!(contains_routine_call("EXEC update_orders", "update_orders"));
    }

    #[test]
    fn test_contains_routine_call_select_func() {
        assert!(contains_routine_call("SELECT calc_tax(100)", "calc_tax"));
    }

    #[test]
    fn test_contains_routine_call_no_false_positive_table() {
        // "SELECT orders" without paren should NOT match as routine call
        assert!(!contains_routine_call("SELECT * FROM orders", "orders"));
    }

    #[test]
    fn test_contains_routine_call_case_insensitive() {
        assert!(contains_routine_call(
            "call UPDATE_ORDERS()",
            "update_orders"
        ));
    }

    #[test]
    fn test_contains_routine_call_perform() {
        assert!(contains_routine_call(
            "PERFORM update_orders()",
            "update_orders"
        ));
    }
}
