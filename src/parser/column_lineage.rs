//! Column-level lineage extraction.
//!
//! Built on top of [`ColumnAccessExtractor`], this extractor additionally
//! captures *how* output columns are produced: direct mappings (flows),
//! derived expressions, and aggregations.

use crate::graph::SourceLocation;
use crate::parser::ColumnAccessExtractor;
use ogsql_parser::ast::{Expr, GroupByItem, Ident, SelectStatement, SelectTarget};
use std::collections::BTreeMap;

/// Column-level lineage edge (intermediate representation during extraction).
#[derive(Debug, Clone)]
pub enum ColumnEdge {
    /// Direct mapping: `SELECT a AS b` → `a` → `b`
    Flow {
        target_col: String,
        source_table: Option<String>,
        source_col: String,
        location: Option<SourceLocation>,
    },
    /// Expression-derived: `SELECT a + 1 AS b`
    Derived {
        target_col: String,
        source_cols: Vec<(Option<String>, String)>,
        expression: String,
        location: Option<SourceLocation>,
    },
    /// Aggregated: `SELECT SUM(a) AS total`
    Aggregated {
        target_col: String,
        source_cols: Vec<(Option<String>, String)>,
        function: String,
        distinct: bool,
        group_by_cols: Vec<String>,
        location: Option<SourceLocation>,
    },
}

/// Column-level lineage extractor.
///
/// Wraps [`ColumnAccessExtractor`] to additionally track expression
/// transformations and aggregations in `SELECT` target lists.
///
/// Usage:
/// - run `walk_statement` over `base_mut()` to collect the base column refs
/// - call [`ColumnLineageExtractor::set_output`] with the owning view/CTE name
/// - call [`ColumnLineageExtractor::analyze_select_targets`] to emit lineage edges
/// - call [`ColumnLineageExtractor::finish`] to get the edges
pub struct ColumnLineageExtractor {
    base: ColumnAccessExtractor,
    column_edges: Vec<ColumnEdge>,
    group_by_columns: Vec<String>,
    current_output: Option<OutputContext>,
}

struct OutputContext {
    owner_table: String,
}

impl ColumnLineageExtractor {
    pub fn new() -> Self {
        Self {
            base: ColumnAccessExtractor::new(),
            column_edges: Vec::new(),
            group_by_columns: Vec::new(),
            current_output: None,
        }
    }

    /// Access the wrapped [`ColumnAccessExtractor`] for the underlying
    /// column-reference / alias analysis.
    pub fn base(&self) -> &ColumnAccessExtractor {
        &self.base
    }

    /// Mutable access to the wrapped [`ColumnAccessExtractor`] (e.g. to run
    /// `walk_statement` over it).
    pub fn base_mut(&mut self) -> &mut ColumnAccessExtractor {
        &mut self.base
    }

    /// Set the table that owns the produced columns (e.g. the view / CTE name).
    pub fn set_output(&mut self, owner_table: &str) {
        self.current_output = Some(OutputContext {
            owner_table: owner_table.to_string(),
        });
    }

    /// Set the GROUP BY columns of the current query (used by aggregation edges).
    pub fn set_group_by_columns(&mut self, cols: &[String]) {
        self.group_by_columns = cols.to_vec();
    }

    /// Inject an already-populated alias map from a prior `ColumnAccessExtractor` run.
    pub fn set_alias_map(&mut self, aliases: BTreeMap<String, crate::parser::TableAlias>) {
        self.base.set_alias_map(aliases);
    }

    /// Extract table aliases from a SELECT's FROM clause.
    /// Returns a fresh alias_map scoped to this SELECT only,
    /// avoiding alias shadowing across sub-selects.
    pub fn extract_aliases_from_from(
        from: &[ogsql_parser::ast::TableRef],
    ) -> BTreeMap<String, crate::parser::TableAlias> {
        let mut aliases = BTreeMap::new();
        for tbl in from {
            extract_aliases_recursive(tbl, &mut aliases);
        }
        aliases
    }

    /// Convenience entry point for a whole `SELECT` statement: records the
    /// GROUP BY columns, then analyzes the target list.
    pub fn analyze_select_statement(&mut self, select: &SelectStatement) {
        self.group_by_columns = group_by_column_names(&select.group_by);
        self.analyze_select_targets(&select.targets);
    }

    pub fn finish(self) -> Vec<ColumnEdge> {
        self.column_edges
    }

    /// Analyze the SELECT target list.
    ///
    /// ⚠️ ogsql-parser's `SelectTarget` is:
    ///   `Expr(Expr, Option<Ident>)` | `Star(Option<Ident>)`
    ///   - `Expr(e, Some(alias))` = expression with alias
    ///   - `Expr(e, None)` = expression without alias (name derived heuristically)
    ///   - `Star(None)` = `SELECT *`
    ///   - `Star(Some(q))` = `SELECT q.*`
    pub fn analyze_select_targets(&mut self, targets: &[SelectTarget]) {
        for target in targets {
            match target {
                SelectTarget::Expr(expr, Some(alias)) => {
                    let target_name = alias.value.clone();
                    self.classify_and_add_edge(expr, &target_name);
                }
                SelectTarget::Expr(expr, None) => {
                    if let Some(name) = self.derive_column_name(expr) {
                        self.classify_and_add_edge(expr, &name);
                    }
                }
                // MVP: no source schema available for `*` expansion; skip.
                SelectTarget::Star(_) => {}
            }
        }
    }

    fn classify_and_add_edge(&mut self, expr: &Expr, target_col: &str) {
        let owner = self
            .current_output
            .as_ref()
            .map(|c| c.owner_table.clone())
            .unwrap_or_default();

        match expr {
            // Aggregate function: SUM(col) / COUNT(*) / AVG(col) ...
            Expr::FunctionCall {
                name,
                args,
                distinct,
                ..
            } if is_aggregate_function(name) => {
                let mut source_cols = extract_arg_columns(args);
                self.resolve_all_aliases(&mut source_cols);
                let func_name = name
                    .last()
                    .map(|n| n.value.to_uppercase())
                    .unwrap_or_default();
                self.column_edges.push(ColumnEdge::Aggregated {
                    target_col: format!("{}.{}", owner, target_col),
                    source_cols,
                    function: func_name,
                    distinct: *distinct,
                    group_by_cols: self.group_by_columns.clone(),
                    location: None,
                });
            }
            // Simple column reference: SELECT col or SELECT t.col
            expr if is_simple_column_ref(expr) => {
                let (mut table, col) = extract_column_ref(expr);
                self.resolve_alias(&mut table);
                self.column_edges.push(ColumnEdge::Flow {
                    target_col: format!("{}.{}", owner, target_col),
                    source_table: table,
                    source_col: col,
                    location: None,
                });
            }
            // Other expressions: a + b, DECODE(...), CASE WHEN ...
            _ => {
                let mut source_cols = extract_all_columns(expr);
                self.resolve_all_aliases(&mut source_cols);
                let expr_text = expr_to_source_text(expr);
                self.column_edges.push(ColumnEdge::Derived {
                    target_col: format!("{}.{}", owner, target_col),
                    source_cols,
                    expression: expr_text,
                    location: None,
                });
            }
        }
    }

    /// Derive a column name from an expression without an alias.
    fn derive_column_name(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::ColumnRef(parts) | Expr::ColumnRefOuterJoin(parts) => {
                parts.last().map(|p| p.value.clone())
            }
            _ => None,
        }
    }

    /// Resolve a single table alias to its physical table name.
    fn resolve_alias(&self, table: &mut Option<String>) {
        if let Some(ref alias) = table {
            if let Some(resolved) = self.base.resolve_alias(alias) {
                *table = Some(resolved.table.clone());
            }
        }
    }

    /// Resolve all table aliases in a list of `(table, column)` pairs.
    fn resolve_all_aliases(&self, cols: &mut Vec<(Option<String>, String)>) {
        for (table, _col) in cols.iter_mut() {
            self.resolve_alias(table);
        }
    }
}

/// Whether the function name is a known aggregate function.
fn is_aggregate_function(name: &[Ident]) -> bool {
    name.last()
        .map(|n| {
            matches!(
                n.value.to_uppercase().as_str(),
                "SUM" | "COUNT" | "AVG" | "MAX" | "MIN"
            )
        })
        .unwrap_or(false)
}

/// Whether the expression is a simple column reference.
fn is_simple_column_ref(expr: &Expr) -> bool {
    matches!(expr, Expr::ColumnRef(_) | Expr::ColumnRefOuterJoin(_))
}

/// Extract `(table, column)` from a column reference expression.
fn extract_column_ref(expr: &Expr) -> (Option<String>, String) {
    match expr {
        Expr::ColumnRef(parts) | Expr::ColumnRefOuterJoin(parts) => {
            if parts.len() >= 2 {
                // Has a table prefix: t.col → (Some("t"), "col")
                (
                    Some(parts[parts.len() - 2].value.clone()),
                    parts[parts.len() - 1].value.clone(),
                )
            } else if parts.len() == 1 {
                // No table prefix: col → (None, "col")
                (None, parts[0].value.clone())
            } else {
                (None, String::new())
            }
        }
        _ => (None, String::new()),
    }
}

/// Extract column references from aggregate function arguments.
fn extract_arg_columns(args: &[Expr]) -> Vec<(Option<String>, String)> {
    args.iter().flat_map(extract_all_columns).collect()
}

/// Recursively collect all column references in an expression tree.
///
/// Subqueries (`EXISTS`, `Subquery`, `ScalarSublink`) are intentionally not
/// traversed to avoid scope pollution (matches the base extractor's behavior).
fn extract_all_columns(expr: &Expr) -> Vec<(Option<String>, String)> {
    match expr {
        Expr::ColumnRef(_) | Expr::ColumnRefOuterJoin(_) => {
            let (table, col) = extract_column_ref(expr);
            vec![(table, col)]
        }
        Expr::BinaryOp { left, right, .. } => {
            let mut cols = extract_all_columns(left);
            cols.extend(extract_all_columns(right));
            cols
        }
        Expr::UnaryOp { expr: inner, .. } => extract_all_columns(inner),
        Expr::FunctionCall { args, .. } => args.iter().flat_map(extract_all_columns).collect(),
        Expr::Parenthesized(inner) => extract_all_columns(inner),
        Expr::Case {
            operand,
            whens,
            else_expr,
        } => {
            let mut cols = Vec::new();
            if let Some(op) = operand {
                cols.extend(extract_all_columns(op));
            }
            for wc in whens {
                cols.extend(extract_all_columns(&wc.condition));
                cols.extend(extract_all_columns(&wc.result));
            }
            if let Some(e) = else_expr {
                cols.extend(extract_all_columns(e));
            }
            cols
        }
        Expr::Like { expr, pattern, .. } => {
            let mut cols = extract_all_columns(expr);
            cols.extend(extract_all_columns(pattern));
            cols
        }
        Expr::Between {
            expr, low, high, ..
        } => {
            let mut cols = extract_all_columns(expr);
            cols.extend(extract_all_columns(low));
            cols.extend(extract_all_columns(high));
            cols
        }
        Expr::InList { expr, list, .. } => {
            let mut cols = extract_all_columns(expr);
            cols.extend(list.iter().flat_map(extract_all_columns));
            cols
        }
        Expr::InSubquery { expr, .. } => extract_all_columns(expr),
        Expr::IsNull { expr, .. } => extract_all_columns(expr),
        Expr::IsBoolean { expr, .. } => extract_all_columns(expr),
        Expr::TypeCast { expr, .. } => extract_all_columns(expr),
        Expr::Treat { expr, .. } => extract_all_columns(expr),
        Expr::FieldAccess { object, .. } => extract_all_columns(object),
        Expr::Subscript { object, .. } => extract_all_columns(object),
        Expr::Prior(inner) => extract_all_columns(inner),
        Expr::RowConstructor(items) => items.iter().flat_map(extract_all_columns).collect(),
        // Subqueries deliberately skipped (scope isolation).
        Expr::Exists(_) | Expr::Subquery(_) | Expr::ScalarSublink { .. } => Vec::new(),
        _ => Vec::new(),
    }
}

/// Reconstruct the raw SQL text of an expression (MVP: uses the AST `Debug` format).
fn expr_to_source_text(expr: &Expr) -> String {
    match expr {
        Expr::ColumnRef(parts) => parts
            .iter()
            .map(|i| i.value.as_str())
            .collect::<Vec<_>>()
            .join("."),
        Expr::BinaryOp { left, op, right } => {
            format!(
                "{} {} {}",
                expr_to_source_text(left),
                op,
                expr_to_source_text(right)
            )
        }
        Expr::UnaryOp { op, expr: inner } => {
            format!("({}{})", op, expr_to_source_text(inner))
        }
        Expr::FunctionCall { name, args, .. } => {
            let func_name = name
                .iter()
                .map(|i| i.value.as_str())
                .collect::<Vec<_>>()
                .join(".");
            let args_str: Vec<String> = args.iter().map(expr_to_source_text).collect();
            format!("{}({})", func_name, args_str.join(", "))
        }
        Expr::Parenthesized(inner) => {
            format!("({})", expr_to_source_text(inner))
        }
        Expr::Literal(lit) => format!("{:?}", lit),
        Expr::Case {
            operand,
            whens,
            else_expr,
        } => {
            let mut s = String::from("CASE");
            if let Some(ref op) = operand {
                s.push_str(&format!(" {}", expr_to_source_text(op)));
            }
            for w in whens {
                s.push_str(&format!(
                    " WHEN {} THEN {}",
                    expr_to_source_text(&w.condition),
                    expr_to_source_text(&w.result)
                ));
            }
            if let Some(ref e) = else_expr {
                s.push_str(&format!(" ELSE {}", expr_to_source_text(e)));
            }
            s.push_str(" END");
            s
        }
        _ => {
            let dbg = format!("{:?}", expr);
            if dbg.len() > 40 {
                format!("{}...", &dbg[..37])
            } else {
                dbg
            }
        }
    }
}

/// Recursively extract table aliases from a TableRef tree (tables, joins, subqueries).
fn extract_aliases_recursive(
    tbl: &ogsql_parser::ast::TableRef,
    aliases: &mut BTreeMap<String, crate::parser::TableAlias>,
) {
    match tbl {
        ogsql_parser::ast::TableRef::Table { name, alias, .. } => {
            let tbl_name = name.last().map(|i| i.value.clone()).unwrap_or_default();
            let schema = if name.len() >= 2 {
                Some(name[0].value.clone())
            } else {
                None
            };
            let key = alias
                .as_ref()
                .map(|a| a.value.to_lowercase())
                .unwrap_or_else(|| tbl_name.to_lowercase());
            aliases.insert(
                key,
                crate::parser::TableAlias {
                    schema,
                    table: tbl_name,
                },
            );
        }
        ogsql_parser::ast::TableRef::Join { left, right, .. } => {
            extract_aliases_recursive(left, aliases);
            extract_aliases_recursive(right, aliases);
        }
        ogsql_parser::ast::TableRef::Subquery { alias, .. } => {
            if let Some(ref a) = alias {
                aliases.insert(
                    a.value.to_lowercase(),
                    crate::parser::TableAlias {
                        schema: None,
                        table: format!("<subquery:{}>", a.value),
                    },
                );
            }
        }
        _ => {}
    }
}

/// Collect GROUP BY column names (only simple `GROUP BY col` items; complex
/// grouping sets / rollup / cube are skipped in the MVP).
fn group_by_column_names(items: &[GroupByItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| match item {
            GroupByItem::Expr(Expr::ColumnRef(parts) | Expr::ColumnRefOuterJoin(parts)) => {
                parts.last().map(|p| p.value.clone())
            }
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogsql_parser::ast::Statement;
    use ogsql_parser::{Parser, Tokenizer};

    /// Parse `sql`, run the lineage extractor over each SELECT statement, and
    /// collect every emitted edge.
    fn edges_for(sql: &str, owner: &str) -> Vec<ColumnEdge> {
        let tokens = Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = Parser::with_source(tokens, sql.to_string());
        let stmts = parser.parse_with_text();
        let mut all = Vec::new();
        for info in &stmts {
            if let Statement::Select(select) = &info.statement {
                let mut ext = ColumnLineageExtractor::new();
                ext.set_output(owner);
                ext.analyze_select_statement(&select.node);
                all.extend(ext.finish());
            }
        }
        all
    }

    #[test]
    fn simple_column_flow() {
        // `SELECT a FROM t` — unqualified column, no alias.
        let edges = edges_for("SELECT a FROM t", "t");
        assert_eq!(edges.len(), 1, "expected 1 edge, got: {:?}", edges);
        match &edges[0] {
            ColumnEdge::Flow {
                target_col,
                source_table,
                source_col,
                location,
            } => {
                assert_eq!(target_col, "t.a");
                assert_eq!(source_table.as_deref(), None);
                assert_eq!(source_col, "a");
                assert!(location.is_none());
            }
            other => panic!("expected Flow, got {:?}", other),
        }
    }

    #[test]
    fn qualified_column_flow_keeps_source_table() {
        // `SELECT t.a AS b FROM t` — qualified column with alias.
        let edges = edges_for("SELECT t.a AS b FROM t", "t");
        assert_eq!(edges.len(), 1, "expected 1 edge, got: {:?}", edges);
        match &edges[0] {
            ColumnEdge::Flow {
                target_col,
                source_table,
                source_col,
                ..
            } => {
                assert_eq!(target_col, "t.b");
                assert_eq!(source_table.as_deref(), Some("t"));
                assert_eq!(source_col, "a");
            }
            other => panic!("expected Flow, got {:?}", other),
        }
    }

    #[test]
    fn aggregation_detected() {
        let sql = "SELECT SUM(a) AS total FROM t GROUP BY x";
        let tokens = Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = Parser::with_source(tokens, sql.to_string());
        let stmts = parser.parse_with_text();
        let mut ext = ColumnLineageExtractor::new();
        ext.set_output("t");
        let mut analyzed = false;
        for info in &stmts {
            if let Statement::Select(select) = &info.statement {
                ext.analyze_select_statement(&select.node);
                analyzed = true;
            }
        }
        assert!(analyzed, "expected a SELECT statement to be parsed");
        let edges = ext.finish();
        assert_eq!(edges.len(), 1, "expected 1 edge, got: {:?}", edges);
        match &edges[0] {
            ColumnEdge::Aggregated {
                target_col,
                source_cols,
                function,
                distinct,
                group_by_cols,
                location,
            } => {
                assert_eq!(target_col, "t.total");
                assert_eq!(*source_cols, vec![(None, "a".to_string())]);
                assert_eq!(function, "SUM");
                assert!(!distinct);
                assert_eq!(*group_by_cols, vec!["x".to_string()]);
                assert!(location.is_none());
            }
            other => panic!("expected Aggregated, got {:?}", other),
        }
    }

    #[test]
    fn distinct_aggregation_flagged() {
        // `SELECT COUNT(DISTINCT b) AS cnt` — distinct must be captured.
        let sql = "SELECT COUNT(DISTINCT b) AS cnt FROM t";
        let edges = edges_for(sql, "t");
        assert_eq!(edges.len(), 1, "expected 1 edge, got: {:?}", edges);
        match &edges[0] {
            ColumnEdge::Aggregated {
                target_col,
                source_cols,
                function,
                distinct,
                ..
            } => {
                assert_eq!(target_col, "t.cnt");
                assert_eq!(*source_cols, vec![(None, "b".to_string())]);
                assert_eq!(function, "COUNT");
                assert!(*distinct);
            }
            other => panic!("expected Aggregated, got {:?}", other),
        }
    }

    #[test]
    fn derived_expression_detected() {
        // `SELECT a + 1 AS b` — derived expression with one source column.
        let edges = edges_for("SELECT a + 1 AS b FROM t", "t");
        assert_eq!(edges.len(), 1, "expected 1 edge, got: {:?}", edges);
        match &edges[0] {
            ColumnEdge::Derived {
                target_col,
                source_cols,
                expression,
                location,
            } => {
                assert_eq!(target_col, "t.b");
                assert_eq!(*source_cols, vec![(None, "a".to_string())]);
                assert!(!expression.is_empty(), "expression text must be captured");
                assert!(location.is_none());
            }
            other => panic!("expected Derived, got {:?}", other),
        }
    }

    #[test]
    fn derived_expression_multiple_source_columns() {
        // `SELECT t.a + t.b AS c` — both qualified source columns captured.
        let edges = edges_for("SELECT t.a + t.b AS c FROM t", "t");
        assert_eq!(edges.len(), 1, "expected 1 edge, got: {:?}", edges);
        match &edges[0] {
            ColumnEdge::Derived {
                target_col,
                source_cols,
                ..
            } => {
                assert_eq!(target_col, "t.c");
                assert_eq!(
                    *source_cols,
                    vec![
                        (Some("t".to_string()), "a".to_string()),
                        (Some("t".to_string()), "b".to_string()),
                    ]
                );
            }
            other => panic!("expected Derived, got {:?}", other),
        }
    }
}
