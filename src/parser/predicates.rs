//! PL/SQL branch predicates extracted from `IF` and `CASE WHEN` conditions (#167).

use super::extractor::{
    as_column_ref, column_transform_of, format_expr_short, literal_to_filter_value,
    resolve_record_field_from_context, split_alias_column,
};
use super::{FilterOperator, FilterTransform, FilterValue, ProcedureVarContext};
use ogsql_parser::ast::plpgsql::{PlBlock, PlStatement};
use ogsql_parser::ast::{Expr, SelectStatement, SelectTarget, Statement, TableRef};
use ogsql_parser::{Visitor, VisitorResult};
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PlPredicate {
    pub id: String,
    pub line: usize,
    pub origin: String,
    pub kind: PredicateKind,
    pub confidence: Confidence,
    pub table_predicate: Option<TablePredicate>,
    pub needs_review: Option<String>,
    pub param_table_hint: Option<ParamTableHint>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum PredicateKind {
    If,
    CaseWhen,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Confidence {
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TablePredicate {
    pub table: String,
    pub clauses: Vec<PredicateClause>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct PredicateClause {
    pub column: String,
    pub op: FilterOperator,
    pub value: FilterValue,
    /// #167/#169: describes a whitelisted pure column transform (e.g. `substr`)
    /// applied to the column before comparison, so consumers don't misread
    /// `substr(col,1,2) = 'x'` as an exact-value equality on `col`.
    #[serde(default)]
    pub transform: Option<FilterTransform>,
}

/// `PredicateClause` is bincode-persisted inside `GraphStore.procedure_predicates`.
/// `#[serde(skip_serializing_if = ...)]` would change bincode's fixed field count
/// depending on data, corrupting the binary layout. Branching manually
/// on `Serializer::is_human_readable()` keeps bincode's field count fixed while
/// still omitting `transform` from JSON when absent — same pattern as
/// `HardFilter` (#169).
impl serde::Serialize for PredicateClause {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let omit_transform = serializer.is_human_readable() && self.transform.is_none();
        let field_count = if omit_transform { 3 } else { 4 };
        let mut state = serializer.serialize_struct("PredicateClause", field_count)?;
        state.serialize_field("column", &self.column)?;
        state.serialize_field("op", &self.op)?;
        state.serialize_field("value", &self.value)?;
        if !omit_transform {
            state.serialize_field("transform", &self.transform)?;
        }
        state.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ParamTableHint {
    pub table: String,
    pub filters: Vec<PredicateClause>,
    pub set: Vec<(String, FilterValue)>,
}

pub struct PredicateExtractor<'a> {
    ctx: &'a ProcedureVarContext,
    predicates: Vec<PlPredicate>,
    var_sources: HashMap<String, VarSource>,
}

#[derive(Debug, Clone)]
struct VarSource {
    table: String,
    column: String,
    filters: Vec<PredicateClause>,
    role: VarSourceRole,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VarSourceRole {
    Main,
    Parameter,
}

#[derive(Debug)]
enum ConditionResolution {
    Direct(Vec<(String, PredicateClause)>),
    Derived(Vec<(VarSource, PredicateClause)>),
}

impl<'a> PredicateExtractor<'a> {
    pub fn new_with_context(ctx: &'a ProcedureVarContext) -> Self {
        Self {
            ctx,
            predicates: Vec::new(),
            var_sources: HashMap::new(),
        }
    }

    pub fn finish(self) -> Vec<PlPredicate> {
        self.predicates
    }

    fn push_condition(&mut self, condition: &Expr, kind: PredicateKind, line: usize) {
        let resolved = condition_clauses(condition, self.ctx, &self.var_sources);
        let id = format!("B{:03}", self.predicates.len() + 1);
        let origin = format!(
            "{} {}",
            match kind {
                PredicateKind::If => "IF",
                PredicateKind::CaseWhen => "WHEN",
            },
            format_condition(condition)
        );
        let (confidence, table_predicate, needs_review, param_table_hint) = match resolved {
            Some(ConditionResolution::Direct(clauses)) => {
                let table_predicate = one_table_predicate(clauses);
                if table_predicate.is_some() {
                    (Confidence::High, table_predicate, None, None)
                } else {
                    (
                        Confidence::Low,
                        None,
                        Some("condition spans multiple or unresolved tables".to_string()),
                        None,
                    )
                }
            }
            Some(ConditionResolution::Derived(clauses)) => {
                let first = clauses.first().map(|(source, _)| source.clone());
                let same_source = first.as_ref().is_some_and(|source| {
                    clauses.iter().all(|(candidate, _)| {
                        candidate.table.eq_ignore_ascii_case(&source.table)
                            && candidate.role == source.role
                    })
                });
                match (first, same_source) {
                    (Some(source), true) if source.role == VarSourceRole::Main => (
                        Confidence::Medium,
                        Some(TablePredicate {
                            table: source.table,
                            clauses: clauses.into_iter().map(|(_, clause)| clause).collect(),
                        }),
                        Some("predicate derived through a SELECT INTO variable".to_string()),
                        None,
                    ),
                    (Some(source), true) => (
                        Confidence::Low,
                        None,
                        Some("condition derives from a parameter/dimension table".to_string()),
                        Some(ParamTableHint {
                            table: source.table,
                            filters: source.filters,
                            set: clauses
                                .into_iter()
                                .map(|(_, clause)| (clause.column, clause.value))
                                .collect(),
                        }),
                    ),
                    _ => (
                        Confidence::Low,
                        None,
                        Some("condition has mixed SELECT INTO sources".to_string()),
                        None,
                    ),
                }
            }
            None => (
                Confidence::Low,
                None,
                Some("condition could not be resolved to one table with certainty".to_string()),
                None,
            ),
        };
        self.predicates.push(PlPredicate {
            id,
            line,
            origin,
            kind,
            confidence,
            table_predicate,
            needs_review,
            param_table_hint,
        });
    }

    fn record_select_into(&mut self, select: &SelectStatement) {
        let Some(into_targets) = &select.into_targets else {
            return;
        };
        let aliases = table_aliases(&select.from);
        let sole_table = sole_table(&aliases);
        let filters = select
            .where_clause
            .as_ref()
            .and_then(|expr| {
                direct_clauses(expr, self.ctx, Some((&aliases, sole_table.as_deref())))
            })
            .and_then(one_table_predicate)
            .map(|predicate| predicate.clauses)
            .unwrap_or_default();

        // Finding 3 (#167): `zip` silently drops extras on length mismatch. Surface
        // it via parse.log before falling back to the (still-truncating) zip so a
        // malformed/unsupported SELECT INTO doesn't fail silently.
        if select.targets.len() != into_targets.len() {
            crate::parse_log::warn(
                "predicates",
                &format!(
                    "SELECT INTO target/variable count mismatch ({} SELECT targets vs {} INTO \
                     variables) — extra entries are dropped in predicate extraction; statement: {}",
                    select.targets.len(),
                    into_targets.len(),
                    select
                        .raw_body
                        .as_deref()
                        .unwrap_or("<SELECT INTO statement, no raw body captured>")
                ),
            );
        }

        for (target, into) in select.targets.iter().zip(into_targets) {
            let (SelectTarget::Expr(value, _), SelectTarget::Expr(variable, _)) = (target, into)
            else {
                continue;
            };
            let Some(names) = as_column_ref(value) else {
                continue;
            };
            let Some(var_name) = expr_name(variable) else {
                continue;
            };
            let Some((table, column)) =
                resolve_select_column(&names, &aliases, sole_table.as_deref())
            else {
                continue;
            };
            self.var_sources.insert(
                var_name.to_lowercase(),
                VarSource {
                    role: classify_var_source(&table, self.ctx),
                    table,
                    column,
                    filters: filters.clone(),
                },
            );
        }
    }
}

impl Visitor for PredicateExtractor<'_> {
    fn visit_pl_statement(&mut self, stmt: &PlStatement) -> VisitorResult {
        match stmt {
            PlStatement::If(spanned) => self.push_condition(
                &spanned.condition,
                PredicateKind::If,
                spanned.span.as_ref().map_or(0, |span| span.start.line),
            ),
            PlStatement::Case(spanned) => {
                let line = spanned.span.as_ref().map_or(0, |span| span.start.line);
                for when in &spanned.whens {
                    self.push_condition(&when.condition, PredicateKind::CaseWhen, line);
                }
            }
            PlStatement::SqlStatement { statement, .. } => {
                if let Statement::Select(select) = statement.as_ref() {
                    self.record_select_into(&select.node);
                }
            }
            _ => {}
        }
        VisitorResult::Continue
    }
}

pub fn extract_predicates(block: &PlBlock, ctx: &ProcedureVarContext) -> Vec<PlPredicate> {
    let mut extractor = PredicateExtractor::new_with_context(ctx);
    ogsql_parser::walk_pl_block(&mut extractor, block);
    extractor.finish()
}

fn condition_clauses(
    expr: &Expr,
    ctx: &ProcedureVarContext,
    var_sources: &HashMap<String, VarSource>,
) -> Option<ConditionResolution> {
    match expr {
        Expr::BinaryOp { left, op, right } if op.eq_ignore_ascii_case("AND") => {
            match (
                condition_clauses(left, ctx, var_sources)?,
                condition_clauses(right, ctx, var_sources)?,
            ) {
                (ConditionResolution::Direct(mut left), ConditionResolution::Direct(right)) => {
                    left.extend(right);
                    Some(ConditionResolution::Direct(left))
                }
                (ConditionResolution::Derived(mut left), ConditionResolution::Derived(right)) => {
                    left.extend(right);
                    Some(ConditionResolution::Derived(left))
                }
                _ => None,
            }
        }
        Expr::BinaryOp { left, op, right } => {
            let operator = comparison_operator(op)?;
            if let Some(value) = literal_to_filter_value(right) {
                return condition_operand(left, ctx, var_sources, operator, value);
            }
            if let Some(value) = literal_to_filter_value(left) {
                return condition_operand(
                    right,
                    ctx,
                    var_sources,
                    reverse_operator(operator),
                    value,
                );
            }
            None
        }
        Expr::Between {
            expr,
            low,
            high,
            negated: false,
        } => {
            let names = as_column_ref(expr)?;
            let low = literal_to_filter_value(low)?;
            let high = literal_to_filter_value(high)?;
            resolved_clause(
                ctx,
                &names,
                FilterOperator::Between,
                FilterValue::List(vec![low, high]),
                None,
            )
            .map(|v| ConditionResolution::Direct(vec![v]))
        }
        Expr::InList {
            expr,
            list,
            negated: false,
        } => {
            let names = as_column_ref(expr)?;
            let values = list
                .iter()
                .map(literal_to_filter_value)
                .collect::<Option<Vec<_>>>()?;
            resolved_clause(
                ctx,
                &names,
                FilterOperator::In,
                FilterValue::List(values),
                None,
            )
            .map(|v| ConditionResolution::Direct(vec![v]))
        }
        _ => None,
    }
}

fn condition_operand(
    expr: &Expr,
    ctx: &ProcedureVarContext,
    var_sources: &HashMap<String, VarSource>,
    op: FilterOperator,
    value: FilterValue,
) -> Option<ConditionResolution> {
    let column_ref = as_column_ref(expr);
    let transform_hit = column_transform_of(expr);
    let names = column_ref.or_else(|| transform_hit.as_ref().map(|(names, _)| names.clone()));
    // #167/#169: the transform (if any) describes the VALUE shape (e.g. a
    // prefix match via `substr`), not the binding — confidence is unaffected.
    let transform = transform_hit.map(|(_, t)| t);
    if let Some(names) = &names {
        if let Some(resolved) = resolved_clause(ctx, names, op, value.clone(), transform.clone()) {
            return Some(ConditionResolution::Direct(vec![resolved]));
        }
    }
    let name = expr_name(expr)?.to_lowercase();
    if let Some(source) = var_sources.get(&name) {
        return Some(ConditionResolution::Derived(vec![(
            source.clone(),
            PredicateClause {
                column: source.column.clone(),
                op,
                value,
                transform: None,
            },
        )]));
    }
    let tables: HashSet<String> = ctx
        .cursor_sources
        .values()
        .flatten()
        .filter_map(|source| source.source_table.clone())
        .collect();
    let names = names?;
    if names.len() == 1 && tables.len() == 1 {
        return tables.into_iter().next().map(|table| {
            ConditionResolution::Direct(vec![(
                table,
                PredicateClause {
                    column: names[0].to_string(),
                    op,
                    value,
                    transform,
                },
            )])
        });
    }
    None
}

fn one_table_predicate(clauses: Vec<(String, PredicateClause)>) -> Option<TablePredicate> {
    let table = clauses.first()?.0.clone();
    clauses
        .iter()
        .all(|(candidate, _)| candidate.eq_ignore_ascii_case(&table))
        .then(|| TablePredicate {
            table,
            clauses: clauses.into_iter().map(|(_, clause)| clause).collect(),
        })
}

fn table_aliases(from: &[TableRef]) -> BTreeMap<String, String> {
    fn collect(table_ref: &TableRef, aliases: &mut BTreeMap<String, String>) {
        match table_ref {
            TableRef::Table { name, alias, .. } => {
                if let Some(table) = name.last() {
                    let table = table.to_string();
                    aliases.insert(
                        alias
                            .as_ref()
                            .map_or_else(|| table.to_lowercase(), |a| a.to_lowercase()),
                        table,
                    );
                }
            }
            TableRef::Join { left, right, .. } => {
                collect(left, aliases);
                collect(right, aliases);
            }
            _ => {}
        }
    }
    let mut aliases = BTreeMap::new();
    for table_ref in from {
        collect(table_ref, &mut aliases);
    }
    aliases
}

/// A bare SELECT column is attributed only when every alias points at the same physical
/// table. Multi-table scopes stay unresolved rather than guessing an owner.
fn sole_table(aliases: &BTreeMap<String, String>) -> Option<String> {
    let mut tables = aliases.values();
    let first = tables.next()?.clone();
    tables
        .all(|table| table.eq_ignore_ascii_case(&first))
        .then_some(first)
}

fn resolve_select_column(
    names: &[ogsql_parser::Ident],
    aliases: &BTreeMap<String, String>,
    sole_table: Option<&str>,
) -> Option<(String, String)> {
    let (prefix, column) = split_alias_column(names);
    let table = match prefix {
        Some(prefix) => aliases.get(&prefix.to_lowercase()).cloned(),
        None => sole_table.map(str::to_string),
    }?;
    Some((table, column))
}

fn expr_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::ColumnRef(names) | Expr::PlVariable(names) => Some(names.join(".")),
        _ => None,
    }
}

/// The AST has no schema metadata that labels lookup tables. Cursor-backed source tables
/// are treated as main/wide tables; otherwise conservative enterprise naming conventions
/// (`dim_*`, `par_*`, `swh_*`, or names containing `param`) identify parameter tables.
/// Unknown names remain main-table candidates but only at Medium confidence.
fn classify_var_source(table: &str, ctx: &ProcedureVarContext) -> VarSourceRole {
    let main_tables: HashSet<String> = ctx
        .cursor_sources
        .values()
        .flatten()
        .filter_map(|source| source.source_table.as_ref())
        .map(|table| table.to_lowercase())
        .collect();
    if main_tables.contains(&table.to_lowercase()) {
        return VarSourceRole::Main;
    }
    let lower = table.to_lowercase();
    if lower.starts_with("dim_")
        || lower.starts_with("par_")
        || lower.starts_with("swh_")
        || lower.contains("param")
    {
        VarSourceRole::Parameter
    } else {
        VarSourceRole::Main
    }
}

fn direct_clauses(
    expr: &Expr,
    ctx: &ProcedureVarContext,
    select_scope: Option<(&BTreeMap<String, String>, Option<&str>)>,
) -> Option<Vec<(String, PredicateClause)>> {
    match expr {
        Expr::BinaryOp { left, op, right } if op.eq_ignore_ascii_case("AND") => {
            let mut clauses = direct_clauses(left, ctx, select_scope)?;
            clauses.extend(direct_clauses(right, ctx, select_scope)?);
            Some(clauses)
        }
        Expr::BinaryOp { left, op, right } => {
            let operator = comparison_operator(op)?;
            let (names, value, operator) = if let (Some(names), Some(value)) =
                (as_column_ref(left), literal_to_filter_value(right))
            {
                (names, value, operator)
            } else {
                (
                    as_column_ref(right)?,
                    literal_to_filter_value(left)?,
                    reverse_operator(operator),
                )
            };
            let resolved = select_scope
                .and_then(|(aliases, sole)| resolve_select_column(&names, aliases, sole))
                .or_else(|| resolve_record_field_from_context(ctx, &names))?;
            Some(vec![(
                resolved.0,
                PredicateClause {
                    column: resolved.1,
                    op: operator,
                    value,
                    transform: None,
                },
            )])
        }
        _ => None,
    }
}

fn resolved_clause(
    ctx: &ProcedureVarContext,
    names: &[ogsql_parser::Ident],
    op: FilterOperator,
    value: FilterValue,
    transform: Option<FilterTransform>,
) -> Option<(String, PredicateClause)> {
    let (table, column) = resolve_record_field_from_context(ctx, names)?;
    Some((
        table,
        PredicateClause {
            column,
            op,
            value,
            transform,
        },
    ))
}

fn comparison_operator(op: &str) -> Option<FilterOperator> {
    match op.trim() {
        "=" => Some(FilterOperator::Eq),
        "<>" | "!=" => Some(FilterOperator::Neq),
        ">" => Some(FilterOperator::Gt),
        ">=" => Some(FilterOperator::Gte),
        "<" => Some(FilterOperator::Lt),
        "<=" => Some(FilterOperator::Lte),
        _ => None,
    }
}

fn reverse_operator(op: FilterOperator) -> FilterOperator {
    match op {
        FilterOperator::Gt => FilterOperator::Lt,
        FilterOperator::Gte => FilterOperator::Lte,
        FilterOperator::Lt => FilterOperator::Gt,
        FilterOperator::Lte => FilterOperator::Gte,
        other => other,
    }
}

fn format_condition(expr: &Expr) -> String {
    match expr {
        Expr::BinaryOp { left, op, right } => format!(
            "{} {} {}",
            format_condition(left),
            op,
            format_condition(right)
        ),
        Expr::Between {
            expr,
            low,
            high,
            negated,
        } => format!(
            "{} {}BETWEEN {} AND {}",
            format_expr_short(expr),
            if *negated { "NOT " } else { "" },
            format_expr_short(low),
            format_expr_short(high)
        ),
        _ => format_expr_short(expr),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::{ColumnAccessExtractor, ProcedureVarContext};
    use ogsql_parser::ast::Statement;

    fn procedure_block(sql: &str) -> ogsql_parser::ast::plpgsql::PlBlock {
        let tokens = ogsql_parser::Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let infos = parser.parse_with_text();
        match &infos[0].statement {
            Statement::CreateProcedure(proc) => proc.block.clone().expect("procedure block"),
            other => panic!("expected procedure, got {other:?}"),
        }
    }

    fn context_from_block(block: &ogsql_parser::ast::plpgsql::PlBlock) -> ProcedureVarContext {
        let mut extractor = ColumnAccessExtractor::new();
        ogsql_parser::walk_pl_block(&mut extractor, block);
        extractor.procedure_context()
    }

    #[test]
    fn star_market_if_resolves_high_confidence() {
        let sql = r#"
CREATE PROCEDURE star_market_jsf AS
  CURSOR c_get_data IS SELECT stock_kind, zqdm FROM mid_yjqs_detail;
  r_get_data c_get_data%ROWTYPE;
BEGIN
  IF r_get_data.stock_kind = '0100' AND
     r_get_data.zqdm BETWEEN '609100' AND '609999' THEN
    NULL;
  END IF;
END;
"#;
        let block = procedure_block(sql);
        let ctx = context_from_block(&block);
        assert_eq!(ctx.cursor_sources["c_get_data"].len(), 2);

        let predicates = extract_predicates(&block, &ctx);

        assert_eq!(predicates.len(), 1);
        let predicate = &predicates[0];
        assert_eq!(predicate.id, "B001");
        assert!(predicate.line > 0);
        assert_eq!(predicate.kind, PredicateKind::If);
        assert_eq!(predicate.confidence, Confidence::High);
        assert!(predicate
            .origin
            .contains("IF r_get_data.stock_kind = '0100'"));
        let table = predicate.table_predicate.as_ref().expect("table predicate");
        assert_eq!(table.table, "mid_yjqs_detail");
        assert_eq!(
            table.clauses,
            vec![
                PredicateClause {
                    column: "stock_kind".to_string(),
                    op: FilterOperator::Eq,
                    value: FilterValue::String("0100".to_string()),
                    transform: None,
                },
                PredicateClause {
                    column: "zqdm".to_string(),
                    op: FilterOperator::Between,
                    value: FilterValue::List(vec![
                        FilterValue::String("609100".to_string()),
                        FilterValue::String("609999".to_string()),
                    ]),
                    transform: None,
                },
            ]
        );
        assert_eq!(predicate.needs_review, None);
        assert_eq!(predicate.param_table_hint, None);
    }

    #[test]
    fn case_when_yields_predicates() {
        let sql = r#"
CREATE PROCEDURE case_predicates AS
  CURSOR c_data IS SELECT x FROM main_data;
  r c_data%ROWTYPE;
BEGIN
  CASE
    WHEN r.x = '1' THEN NULL;
    WHEN r.x = '2' THEN NULL;
  END CASE;
END;
"#;
        let block = procedure_block(sql);
        let ctx = context_from_block(&block);

        let predicates = extract_predicates(&block, &ctx);

        assert_eq!(predicates.len(), 2);
        assert_eq!(predicates[0].id, "B001");
        assert_eq!(predicates[1].id, "B002");
        assert!(predicates
            .iter()
            .all(|p| p.kind == PredicateKind::CaseWhen && p.confidence == Confidence::High));
        assert_eq!(
            predicates[0].table_predicate.as_ref().unwrap().clauses[0].value,
            FilterValue::String("1".to_string())
        );
        assert_eq!(
            predicates[1].table_predicate.as_ref().unwrap().clauses[0].value,
            FilterValue::String("2".to_string())
        );
    }

    #[test]
    fn cursor_where_hard_filters_not_in_predicates() {
        let sql = r#"
CREATE PROCEDURE cursor_filter_isolation AS
  CURSOR c_data IS SELECT stock_kind FROM main_data WHERE scdm = '001';
  r c_data%ROWTYPE;
BEGIN
  IF r.stock_kind = '0100' THEN NULL; END IF;
END;
"#;
        let block = procedure_block(sql);
        let ctx = context_from_block(&block);

        let predicates = extract_predicates(&block, &ctx);

        assert_eq!(predicates.len(), 1);
        let clauses = &predicates[0].table_predicate.as_ref().unwrap().clauses;
        assert_eq!(clauses.len(), 1);
        assert_eq!(clauses[0].column, "stock_kind");
        assert!(!predicates[0].origin.contains("scdm"));
    }

    #[test]
    fn unresolvable_condition_keeps_origin_low_confidence() {
        let block = procedure_block(
            r#"
CREATE PROCEDURE unresolved_predicate AS
BEGIN
  IF unknown_value = '1' THEN NULL; END IF;
END;
"#,
        );

        let predicates = extract_predicates(&block, &ProcedureVarContext::default());

        assert_eq!(predicates.len(), 1);
        assert_eq!(predicates[0].confidence, Confidence::Low);
        assert!(predicates[0].origin.contains("unknown_value = '1'"));
        assert!(predicates[0].table_predicate.is_none());
        assert!(predicates[0].needs_review.is_some());
    }

    #[test]
    fn function_condition_degrades_confidence() {
        let block = procedure_block(
            r#"
CREATE PROCEDURE function_predicate AS
BEGIN
  IF fnc_x(col_a) = 1 THEN NULL; END IF;
END;
"#,
        );

        let predicates = extract_predicates(&block, &ProcedureVarContext::default());

        assert_eq!(predicates.len(), 1);
        assert_eq!(predicates[0].confidence, Confidence::Low);
        assert!(predicates[0].origin.contains("fnc_x(col_a) = 1"));
        assert!(predicates[0].table_predicate.is_none());
    }

    #[test]
    fn select_into_main_table_var_yields_medium() {
        let block = procedure_block(
            r#"
CREATE PROCEDURE main_var_predicate AS
  v_qty NUMBER;
BEGIN
  SELECT qty INTO v_qty FROM big_main WHERE big_main.x = 1;
  IF v_qty > 100 THEN NULL; END IF;
END;
"#,
        );
        let ctx = context_from_block(&block);
        let PlStatement::SqlStatement { statement, .. } = &block.body[0] else {
            panic!("expected SELECT statement, got {:?}", block.body[0]);
        };
        let Statement::Select(select) = statement.as_ref() else {
            panic!("expected parsed SELECT, got {statement:?}");
        };
        assert!(select.into_targets.is_some(), "SELECT INTO AST: {select:?}");

        let predicates = extract_predicates(&block, &ctx);

        assert_eq!(predicates.len(), 1);
        assert_eq!(predicates[0].confidence, Confidence::Medium);
        assert!(predicates[0].needs_review.is_some());
        assert!(predicates[0].param_table_hint.is_none());
        let table = predicates[0].table_predicate.as_ref().unwrap();
        assert_eq!(table.table, "big_main");
        assert_eq!(table.clauses[0].column, "qty");
        assert_eq!(table.clauses[0].op, FilterOperator::Gt);
        assert_eq!(table.clauses[0].value, FilterValue::Integer(100));
    }

    /// Regression for review Finding 2 (#167/#169): a column-transform condition
    /// like `substr(r.stock_kind, 1, 2) = '05'` must carry the `FilterTransform`
    /// on the clause rather than silently emitting a plain `Eq` clause that would
    /// misrepresent a prefix match as an exact-value match.
    #[test]
    fn transformed_condition_clause_carries_transform() {
        let sql = r#"
CREATE PROCEDURE substr_predicate AS
  CURSOR c_get_data IS SELECT stock_kind FROM mid_yjqs_detail;
  r c_get_data%ROWTYPE;
BEGIN
  IF substr(r.stock_kind, 1, 2) = '05' THEN NULL; END IF;
END;
"#;
        let block = procedure_block(sql);
        let ctx = context_from_block(&block);

        let predicates = extract_predicates(&block, &ctx);

        assert_eq!(predicates.len(), 1);
        let predicate = &predicates[0];
        assert_eq!(predicate.confidence, Confidence::High);
        let table = predicate.table_predicate.as_ref().expect("table predicate");
        assert_eq!(table.table, "mid_yjqs_detail");
        assert_eq!(table.clauses.len(), 1);
        let clause = &table.clauses[0];
        assert_eq!(clause.column, "stock_kind");
        assert_eq!(clause.op, FilterOperator::Eq);
        assert_eq!(clause.value, FilterValue::String("05".to_string()));
        assert_eq!(
            clause.transform,
            Some(FilterTransform {
                fn_name: "substr".to_string(),
                args: vec![FilterValue::Integer(1), FilterValue::Integer(2)],
            })
        );
    }

    #[test]
    fn select_into_var_condition_yields_param_table_hint() {
        let block = procedure_block(
            r#"
CREATE PROCEDURE param_var_predicate AS
  v_kind VARCHAR(10);
BEGIN
  SELECT kind_id INTO v_kind FROM swh_all_kind
   WHERE operation_kind = 'COMMISSION_SWITCH';
  IF v_kind = '1' THEN NULL; END IF;
END;
"#,
        );
        let ctx = context_from_block(&block);

        let predicates = extract_predicates(&block, &ctx);

        assert_eq!(predicates.len(), 1);
        let predicate = &predicates[0];
        assert_eq!(predicate.confidence, Confidence::Low);
        assert!(predicate.table_predicate.is_none());
        assert!(predicate.needs_review.is_some());
        let hint = predicate.param_table_hint.as_ref().expect("parameter hint");
        assert_eq!(hint.table, "swh_all_kind");
        assert_eq!(
            hint.filters,
            vec![PredicateClause {
                column: "operation_kind".to_string(),
                op: FilterOperator::Eq,
                value: FilterValue::String("COMMISSION_SWITCH".to_string()),
                transform: None,
            }]
        );
        assert_eq!(
            hint.set,
            vec![("kind_id".to_string(), FilterValue::String("1".to_string()))]
        );
    }
}
