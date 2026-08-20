use crate::graph::{AccessMode, RoutineId, RoutineKind, SourceLocation, WriteKind};
use ogsql_parser::ast::plpgsql::{PlCursorDecl, PlExecuteStmt, PlProcedureCall, PlStatement};
use ogsql_parser::ast::plpgsql::{PlDeclaration, PlTypeDecl};
use ogsql_parser::ast::{
    CallFuncStatement, DataType, Expr, InsertStatement, JoinType as AstJoinType, Literal,
    ObjectName, RoutineParam, SelectStatement, SelectTarget, SequenceFunc, Statement,
    TableRef as AstTableRef, UpdateStatement, WhenClause, WithClause,
};
use ogsql_parser::{Visitor, VisitorResult};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct ProcedureBodySql {
    pub sql_text: String,
    pub kind: String,
    pub line: Option<usize>,
    /// The parsed statement AST from the ORIGINAL procedure-body parse, when the source
    /// was a typed SQL statement. Walking this (instead of re-parsing `sql_text`) keeps
    /// procedure context such as declared-variable classification (issue #147).
    pub statement: Option<Statement>,
}

pub struct ProcedureSqlExtractor {
    results: Vec<ProcedureBodySql>,
}

impl ProcedureSqlExtractor {
    pub fn new() -> Self {
        Self {
            results: Vec::new(),
        }
    }
    pub fn finish(self) -> Vec<ProcedureBodySql> {
        self.results
    }
}

impl Visitor for ProcedureSqlExtractor {
    fn visit_pl_declaration(&mut self, decl: &PlDeclaration) -> VisitorResult {
        if let PlDeclaration::Cursor(PlCursorDecl {
            query,
            parsed_query,
            ..
        }) = decl
        {
            let kind = parsed_query
                .as_ref()
                .and_then(|q| {
                    let name = statement_kind_name(q);
                    if name == "SQL" {
                        None
                    } else {
                        Some(name)
                    }
                })
                .unwrap_or_else(|| "SQL".to_string());
            self.results.push(ProcedureBodySql {
                sql_text: query.clone(),
                kind,
                line: None,
                statement: parsed_query.as_ref().map(|q| (**q).clone()),
            });
        }
        VisitorResult::Continue
    }

    fn visit_pl_statement(&mut self, stmt: &PlStatement) -> VisitorResult {
        match stmt {
            PlStatement::SqlStatement {
                sql_text,
                statement,
                span,
                ..
            } => {
                let kind = statement_kind_name(statement);
                let line = span.as_ref().map(|s| s.start.line);
                self.results.push(ProcedureBodySql {
                    sql_text: sql_text.clone(),
                    kind,
                    line,
                    statement: Some((**statement).clone()),
                });
            }
            PlStatement::Sql(sql_text) => {
                self.results.push(ProcedureBodySql {
                    sql_text: sql_text.clone(),
                    kind: "SQL".to_string(),
                    line: None,
                    statement: None,
                });
            }
            PlStatement::Perform {
                query,
                parsed_query,
                ..
            } => {
                let kind = parsed_query
                    .as_ref()
                    .and_then(|q| {
                        let name = statement_kind_name(q);
                        if name == "SQL" {
                            None
                        } else {
                            Some(name)
                        }
                    })
                    .unwrap_or_else(|| "SQL".to_string());
                self.results.push(ProcedureBodySql {
                    sql_text: query.clone(),
                    kind,
                    line: None,
                    statement: parsed_query.as_ref().map(|q| (**q).clone()),
                });
            }
            PlStatement::ReturnQuery(rq) => {
                self.results.push(ProcedureBodySql {
                    sql_text: rq.node.query.clone(),
                    kind: "SQL".to_string(),
                    line: rq.span.as_ref().map(|sp| sp.start.line),
                    statement: None,
                });
            }
            PlStatement::Execute(exec) => {
                let kind = exec
                    .node
                    .parsed_query
                    .as_ref()
                    .map(|parsed| statement_kind_name(parsed))
                    .unwrap_or_else(|| "SQL".to_string());
                let sql_text = match &exec.node.string_expr {
                    Expr::Literal(Literal::String(s)) => s.clone(),
                    _ => format!("{:?}", exec.node.string_expr),
                };
                self.results.push(ProcedureBodySql {
                    sql_text,
                    kind,
                    line: None,
                    statement: exec.node.parsed_query.as_ref().map(|q| (**q).clone()),
                });
            }
            _ => {}
        }
        VisitorResult::Continue
    }
}

fn statement_kind_name(stmt: &Statement) -> String {
    match stmt {
        Statement::Select(_) => "SELECT",
        Statement::Insert(_) => "INSERT",
        Statement::Update(_) => "UPDATE",
        Statement::Delete(_) => "DELETE",
        Statement::Merge(_) => "MERGE",
        _ => "SQL",
    }
    .to_string()
}

pub fn extract_body_sql(block: &ogsql_parser::ast::plpgsql::PlBlock) -> Vec<ProcedureBodySql> {
    let mut extractor = ProcedureSqlExtractor::new();
    ogsql_parser::walk_pl_block(&mut extractor, block);
    extractor.finish()
}

#[derive(Debug, Clone)]
pub struct CallEdge {
    pub caller: Option<RoutineId>,
    pub callee_name: String,
    pub is_dynamic: bool,
    pub location: SourceLocation,
    pub builtin_meta: Option<ogsql_parser::ast::BuiltinFuncMeta>,
}

/// Maximum number of literal-string variants to track per PL variable.
///
/// `extract_all_literal_strings` builds a cartesian product across `||`
/// operands and IF/CASE branches. Without a cap, a long concatenation chain
/// whose operands each carry multiple values (e.g. 30 `CASE WHEN ... END`
/// terms) explodes to 2^30 strings → CPU spin + OOM (regression introduced in
/// v0.7.10, fixed here). When the product would exceed this cap we abandon
/// static expansion for that expression; the EXECUTE IMMEDIATE call then
/// falls back to an opaque dynamic edge — a sound, lossy fallback that
/// preserves call-graph correctness.
pub const MAX_VALUE_SET: usize = 64;

/// All literal strings `expr` can evaluate to (`||` concatenation, variable lookup via
/// `var_values`, CASE branches), capped to avoid exponential blowup on long chains.
/// Shared by call-edge and column-analysis dynamic-SQL tracking.
fn literal_strings(expr: &Expr, var_values: &HashMap<String, HashSet<String>>) -> Vec<String> {
    match expr {
        Expr::Literal(Literal::String(s)) => vec![s.clone()],
        Expr::BinaryOp {
            left, op, right, ..
        } if op.trim() == "||" => {
            let left_vals = literal_strings(left, var_values);
            let right_vals = literal_strings(right, var_values);
            if left_vals.is_empty() || right_vals.is_empty() {
                return vec![];
            }
            let product = left_vals.len().saturating_mul(right_vals.len());
            if product > MAX_VALUE_SET {
                return vec![];
            }
            let mut result = Vec::with_capacity(product);
            for l in &left_vals {
                for r in &right_vals {
                    result.push(format!("{}{}", l, r));
                }
            }
            result
        }
        Expr::PlVariable(names) => {
            let var_name = names.join(".").to_lowercase();
            var_values
                .get(&var_name)
                .map(|set| set.iter().cloned().collect())
                .unwrap_or_default()
        }
        Expr::Case {
            whens, else_expr, ..
        } => {
            let mut result = Vec::new();
            for wc in whens {
                result.extend(literal_strings(&wc.result, var_values));
            }
            if let Some(else_expr) = else_expr {
                result.extend(literal_strings(else_expr, var_values));
            }
            result
        }
        _ => vec![],
    }
}

/// Collect every string literal leaf in `expr` regardless of `||` concatenation success —
/// used to scan partially-dynamic SQL for its static table references.
fn literal_leaves(expr: &Expr, out: &mut Vec<String>) {
    match expr {
        Expr::Literal(Literal::String(s)) => out.push(s.clone()),
        Expr::BinaryOp { left, right, .. } => {
            literal_leaves(left, out);
            literal_leaves(right, out);
        }
        Expr::UnaryOp { expr, .. } => literal_leaves(expr, out),
        Expr::Case {
            whens, else_expr, ..
        } => {
            for wc in whens {
                literal_leaves(&wc.result, out);
            }
            if let Some(else_expr) = else_expr {
                literal_leaves(else_expr, out);
            }
        }
        _ => {}
    }
}

/// Tables named in a (possibly partial) dynamic-SQL text: whitespace-split and take the
/// word after each FROM. Tolerant of fragments ending in an unterminated quote (a
/// tokenizer would reject them). Static fragments like `... from mid_yjqs_detail ...`
/// are resolvable even when the full dynamic statement is built from runtime values.
fn tables_in_sql_fragment(sql: &str) -> HashSet<String> {
    let mut tables = HashSet::new();
    let words: Vec<&str> = sql.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        if word.eq_ignore_ascii_case("from") {
            let Some(raw) = words.get(i + 1) else {
                continue;
            };
            let cleaned: String = raw
                .trim_matches(|c: char| c.is_ascii_punctuation() && c != '_')
                .chars()
                .take_while(|c| !c.is_ascii_punctuation() || *c == '_' || *c == '.')
                .collect();
            let name = cleaned.rsplit('.').next().unwrap_or("").to_lowercase();
            if !name.is_empty() && !raw.contains('(') {
                tables.insert(name);
            }
        }
    }
    tables
}

pub struct CallExtractor {
    pub current_procedure: Option<RoutineId>,
    pub edges: Vec<CallEdge>,
    file: Arc<std::path::PathBuf>,
    /// Local identifiers declared in the current procedure's DECLARE block
    /// (variables, cursors, records) plus its parameter list. Used to filter
    /// false-positive call edges from PL/SQL collection indexing `v(i)`.
    local_vars: HashSet<String>,
    /// Globally known TYPE names (lowercased). A `type_name()` constructor
    /// call or `type_name.method(...)` member call is not a procedure/function
    /// call edge and must be filtered.
    known_types: HashSet<String>,
    /// Variable → set-of-literal-values mapping for tracking assignments.
    /// `v_sql := 'CALL proc()'` stores {"CALL proc()"}.
    /// At IF/ELSE merge points the set accumulates values from all branches
    /// (sound over-approximation). Used to resolve EXECUTE IMMEDIATE v_sql
    /// into DirectCall edges for every tracked value containing a CALL.
    /// Cleared on begin_routine_scope.
    var_values: HashMap<String, HashSet<String>>,
    /// Guards against log spam: once a routine hits the [`MAX_VALUE_SET`] cap,
    /// further overflows in the same routine are silent.
    value_overflow_warned: bool,
}

impl CallExtractor {
    pub fn new(file: Arc<std::path::PathBuf>, known_types: HashSet<String>) -> Self {
        Self {
            current_procedure: None,
            edges: Vec::new(),
            file,
            local_vars: HashSet::new(),
            known_types,
            var_values: HashMap::new(),
            value_overflow_warned: false,
        }
    }

    fn make_location(&self, line: usize) -> SourceLocation {
        SourceLocation {
            file: self.file.clone(),
            line,
        }
    }

    /// Establish a fresh local-variable scope for a routine body.
    /// Clears any previous locals and populates from the parameter list.
    /// Must be called before walking every routine body — standalone
    /// (visit_statement), package item (collect_package_call_edges), or
    /// nested (visit_pl_declaration) — so identifiers don't leak across
    /// sibling or enclosing scopes.
    pub fn begin_routine_scope(&mut self, params: &[RoutineParam]) {
        self.local_vars.clear();
        self.var_values.clear();
        self.value_overflow_warned = false;
        for param in params {
            self.local_vars.insert(param.name.to_lowercase());
        }
    }

    /// Extend the current routine's local scope with package-level identifiers.
    /// Must be called AFTER `begin_routine_scope` so the names survive the clear.
    pub fn extend_local_scope(&mut self, names: impl IntoIterator<Item = String>) {
        for name in names {
            self.local_vars.insert(name);
        }
    }

    /// Register a TYPE name so that constructor calls `my_type(...)` are not
    /// mistaken for procedure/function calls.
    pub fn register_type_name(&mut self, name: &str) {
        if !name.is_empty() {
            self.known_types.insert(name.to_lowercase());
        }
    }

    pub fn push_call(&mut self, callee: &str, is_dynamic: bool, line: usize) {
        self.edges.push(CallEdge {
            caller: self.current_procedure.clone(),
            callee_name: callee.to_string(),
            is_dynamic,
            location: self.make_location(line),
            builtin_meta: None,
        });
    }

    pub fn push_builtin_call(
        &mut self,
        callee: &str,
        meta: ogsql_parser::ast::BuiltinFuncMeta,
        line: usize,
    ) {
        self.edges.push(CallEdge {
            caller: self.current_procedure.clone(),
            callee_name: callee.to_string(),
            is_dynamic: false,
            location: self.make_location(line),
            builtin_meta: Some(meta),
        });
    }

    /// Record (once per routine) that value-set expansion was capped, surfacing
    /// the file + cause in `parse.log` so future pathological inputs are
    /// diagnosed instantly instead of presenting as a hang/OOM.
    fn warn_value_overflow(&mut self) {
        if self.value_overflow_warned {
            return;
        }
        self.value_overflow_warned = true;
        crate::parse_log::warn(
            &self.file.display().to_string(),
            &format!(
                "value-set expansion capped at {} (EXECUTE IMMEDIATE dynamic-SQL resolution \
                 degraded to opaque) — likely a long `||` concatenation chain or many \
                 CASE/IF branches producing a combinatorial value explosion",
                MAX_VALUE_SET
            ),
        );
    }

    fn extract_call_from_sql_text(&mut self, sql_text: &str) {
        let normalized = sql_text.to_lowercase().replace(' ', "");
        if let Some(rest) = normalized.strip_prefix("call") {
            let call_target = rest.split('(').next().unwrap_or("");
            if !call_target.is_empty() {
                self.push_call(call_target, false, 0);
            }
        }
    }

    /// Extract ALL possible concrete literal strings from an expression.
    ///
    /// Handles:
    /// - Direct string literals
    /// - `||` concatenation (cartesian product when both sides have multiple values)
    /// - Variable chain resolution (looks up [`var_values`])
    /// - `CASE` expressions (collects result values from all WHEN/ELSE branches)
    ///
    /// Returns an empty Vec when no literal value can be statically determined.
    /// A Vec with multiple entries means the value is non-deterministic
    /// (e.g. different IF/ELSE branches, CASE with different WHEN values).
    fn extract_all_literal_strings(&mut self, expr: &Expr) -> Vec<String> {
        literal_strings(expr, &self.var_values)
    }
}

/// Extract the declared name from any `PlTypeDecl` variant.
pub fn pl_type_decl_name(t: &PlTypeDecl) -> &str {
    match t {
        PlTypeDecl::Record { name, .. }
        | PlTypeDecl::TableOf { name, .. }
        | PlTypeDecl::VarrayOf { name, .. }
        | PlTypeDecl::RefCursor { name } => name,
    }
}

impl Visitor for CallExtractor {
    fn visit_statement(&mut self, stmt: &Statement) -> VisitorResult {
        match stmt {
            Statement::CreateProcedure(p) => {
                self.begin_routine_scope(&p.parameters);
                let id = RoutineId::from_object_name(&p.name, RoutineKind::Procedure);
                self.current_procedure = Some(id);
            }
            Statement::CreateFunction(f) => {
                self.begin_routine_scope(&f.parameters);
                let id = RoutineId::from_object_name(&f.name, RoutineKind::Function);
                self.current_procedure = Some(id);
            }
            _ => {}
        }
        VisitorResult::Continue
    }

    fn visit_pl_declaration(&mut self, decl: &PlDeclaration) -> VisitorResult {
        match decl {
            PlDeclaration::Variable(v) => {
                self.local_vars.insert(v.name.to_lowercase());
                if let Some(ref default) = v.default {
                    let values = self.extract_all_literal_strings(default);
                    if !values.is_empty() {
                        self.var_values
                            .insert(v.name.to_lowercase(), values.into_iter().collect());
                    }
                }
                VisitorResult::Continue
            }
            PlDeclaration::Cursor(c) => {
                self.local_vars.insert(c.name.to_lowercase());
                VisitorResult::Continue
            }
            PlDeclaration::Record(r) => {
                self.local_vars.insert(r.name.to_lowercase());
                VisitorResult::Continue
            }
            // Nested routines need their own fresh scope. The default walker
            // recurses into walk_pl_block AFTER visit_pl_declaration returns,
            // with no cleanup — causing inner locals to leak outward. We
            // prevent this by returning SkipChildren and manually walking
            // the nested block with a save/restore barrier.
            PlDeclaration::NestedProcedure(p) => {
                let saved_vars = std::mem::take(&mut self.local_vars);
                let saved_values = std::mem::take(&mut self.var_values);
                self.begin_routine_scope(&p.parameters);
                if let Some(ref block) = p.block {
                    ogsql_parser::walk_pl_block(self, block);
                }
                self.local_vars = saved_vars;
                self.var_values = saved_values;
                VisitorResult::SkipChildren
            }
            PlDeclaration::NestedFunction(f) => {
                let saved_vars = std::mem::take(&mut self.local_vars);
                let saved_values = std::mem::take(&mut self.var_values);
                self.begin_routine_scope(&f.parameters);
                if let Some(ref block) = f.block {
                    ogsql_parser::walk_pl_block(self, block);
                }
                self.local_vars = saved_vars;
                self.var_values = saved_values;
                VisitorResult::SkipChildren
            }
            PlDeclaration::Type(t) => {
                self.register_type_name(pl_type_decl_name(t));
                VisitorResult::Continue
            }
            PlDeclaration::Pragma { .. } => VisitorResult::Continue,
        }
    }

    fn visit_call(&mut self, call: &CallFuncStatement) -> VisitorResult {
        let name: String = call.func_name.join(".");
        if let Some(meta) = &call.builtin {
            self.push_builtin_call(&name, meta.clone(), 0);
        } else {
            self.push_call(&name, false, 0);
        }
        VisitorResult::Continue
    }

    fn visit_procedure_call(&mut self, call: &PlProcedureCall) -> VisitorResult {
        if let Some(first) = call.name.first() {
            let first_lower = first.to_lowercase();
            if self.local_vars.contains(&first_lower) || self.known_types.contains(&first_lower) {
                return VisitorResult::Continue;
            }
        }
        let name: String = call.name.join(".");
        if let Some(meta) = &call.builtin {
            self.push_builtin_call(&name, meta.clone(), 0);
        } else {
            self.push_call(&name, false, 0);
        }
        VisitorResult::Continue
    }

    fn visit_pl_statement(&mut self, stmt: &PlStatement) -> VisitorResult {
        match stmt {
            // ── Assignment: track variable values ──
            // Target is a PL variable: v := expr
            PlStatement::Assignment {
                target: Expr::PlVariable(names),
                expression,
            } => {
                let var_name = names.join(".").to_lowercase();
                let values = self.extract_all_literal_strings(expression);
                if !values.is_empty() {
                    // Sequential assignment REPLACES the set (last wins)
                    self.var_values
                        .insert(var_name, values.into_iter().collect());
                }
            }
            // Target is a record field: r.field := expr
            PlStatement::Assignment {
                target: Expr::FieldAccess { object, field },
                expression,
            } => {
                if let Expr::PlVariable(record_name) = object.as_ref() {
                    let compound = format!("{}.{}", record_name.join("."), field).to_lowercase();
                    let values = self.extract_all_literal_strings(expression);
                    if !values.is_empty() {
                        self.var_values
                            .insert(compound, values.into_iter().collect());
                    }
                }
            }

            // ── IF/ELSE: branch-aware value set merging ──
            // NOTE: Must use walk_pl_statement (not visit_pl_statement) so that
            // child-walking happens: visit_procedure_call for ProcedureCall,
            // walk_expr for expression children, nested IF branch merging, etc.
            // The walk function calls visit_pl_statement first, then walks
            // children on Continue. Our SkipChildren from nested IF handlers
            // is still respected (walk function short-circuits on SkipChildren).
            PlStatement::If(spanned) => {
                let node = &spanned.node;
                // Snapshot state before IF
                let pre_snapshot = self.var_values.clone();
                let mut branch_states: Vec<HashMap<String, HashSet<String>>> = Vec::new();

                // Walk THEN branch
                self.var_values = pre_snapshot.clone();
                for s in &node.then_stmts {
                    ogsql_parser::walk_pl_statement(self, s);
                }
                branch_states.push(std::mem::take(&mut self.var_values));

                // Walk ELSIF branches
                for elsif in &node.elsifs {
                    self.var_values = pre_snapshot.clone();
                    for s in &elsif.stmts {
                        ogsql_parser::walk_pl_statement(self, s);
                    }
                    branch_states.push(std::mem::take(&mut self.var_values));
                }

                // Walk ELSE branch (even if absent, restores baseline)
                self.var_values = pre_snapshot.clone();
                for s in &node.else_stmts {
                    ogsql_parser::walk_pl_statement(self, s);
                }
                branch_states.push(std::mem::take(&mut self.var_values));

                // Merge: for each variable, UNION values across all branches.
                // Since each branch starts from the same pre-snapshot,
                // variables assigned in AT LEAST one branch accumulate all
                // branch-specific values — a sound over-approximation.
                // Variables not assigned in any branch keep the pre-snapshot
                // value (single, from every branch starting point).
                let mut merged: HashMap<String, HashSet<String>> = HashMap::new();
                for state in &branch_states {
                    for (k, vals) in state {
                        let entry = merged.entry(k.clone()).or_default();
                        for v in vals {
                            if entry.len() >= MAX_VALUE_SET {
                                break;
                            }
                            entry.insert(v.clone());
                        }
                    }
                }
                if merged.values().any(|s| s.len() >= MAX_VALUE_SET) {
                    self.warn_value_overflow();
                }
                self.var_values = merged;
                return VisitorResult::SkipChildren;
            }

            // ── EXECUTE IMMEDIATE: resolve variable content ──
            PlStatement::Execute(ogsql_parser::ast::Spanned {
                node:
                    PlExecuteStmt {
                        parsed_query: None,
                        string_expr,
                        ..
                    },
                ..
            }) => {
                let mut peeled = peel_parenthesized(string_expr);
                // Unwrap TypeCast to get the inner expression
                // (e.g., CAST(v_sql AS VARCHAR2) → v_sql)
                if let Expr::TypeCast { expr, .. } = peeled {
                    peeled = expr.as_ref();
                }
                let mut resolved = false;

                // Resolve via PL variable reference
                if let Expr::PlVariable(names) = peeled {
                    let var_name = names.join(".").to_lowercase();
                    // Clone to avoid borrow conflict with extract_call_from_sql_text
                    let candidates: Vec<String> = self
                        .var_values
                        .get(&var_name)
                        .map(|s| s.iter().cloned().collect())
                        .unwrap_or_default();
                    for sql_text in &candidates {
                        self.extract_call_from_sql_text(sql_text);
                    }
                    if !candidates.is_empty() {
                        resolved = true;
                    }
                }

                // Resolve via record.field reference: r.sql_text
                if let Expr::FieldAccess { object, field } = peeled {
                    if let Expr::PlVariable(record_name) = object.as_ref() {
                        let compound =
                            format!("{}.{}", record_name.join("."), field).to_lowercase();
                        let candidates: Vec<String> = self
                            .var_values
                            .get(&compound)
                            .map(|s| s.iter().cloned().collect())
                            .unwrap_or_default();
                        for sql_text in &candidates {
                            self.extract_call_from_sql_text(sql_text);
                        }
                        if !candidates.is_empty() {
                            resolved = true;
                        }
                    }
                }

                // Fallback: unresolvable → dynamic call (noise_rule filters)
                if !resolved {
                    let raw = format!("{:?}", peeled);
                    self.push_call(&raw, true, 0);
                }
            }

            // ── Direct SQL text ──
            PlStatement::Sql(sql_text) => {
                self.extract_call_from_sql_text(sql_text);
            }

            _ => {}
        }
        VisitorResult::Continue
    }

    fn visit_select(&mut self, select: &SelectStatement) -> VisitorResult {
        for hint in select.hints.iter() {
            if hint.name.is_empty() {
                continue;
            }
            self.push_builtin_call(
                &hint.name,
                ogsql_parser::ast::BuiltinFuncMeta {
                    category: "Hint".into(),
                    domain: "QueryPlan".into(),
                },
                0,
            );
        }
        for tr in &select.from {
            self.extract_func_from_table_ref(tr);
        }
        VisitorResult::Continue
    }

    fn visit_expr(&mut self, expr: &Expr) -> VisitorResult {
        // ── Operator detection (ANY / ALL / SOME / EXISTS / IN / NOT_IN) ──
        match expr {
            Expr::ScalarSublink { sublink_type, .. } => {
                let name = match sublink_type {
                    ogsql_parser::ast::ScalarSublinkType::Any => "ANY",
                    ogsql_parser::ast::ScalarSublinkType::Some => "SOME",
                    ogsql_parser::ast::ScalarSublinkType::All => "ALL",
                };
                self.push_builtin_call(
                    name,
                    ogsql_parser::ast::BuiltinFuncMeta {
                        category: "Operator".into(),
                        domain: "Comparison".into(),
                    },
                    0,
                );
                return VisitorResult::Continue;
            }
            Expr::Exists(_) => {
                self.push_builtin_call(
                    "EXISTS",
                    ogsql_parser::ast::BuiltinFuncMeta {
                        category: "Operator".into(),
                        domain: "Predicate".into(),
                    },
                    0,
                );
                return VisitorResult::Continue;
            }
            Expr::InSubquery { negated, .. } => {
                let name = if *negated { "NOT_IN" } else { "IN" };
                self.push_builtin_call(
                    name,
                    ogsql_parser::ast::BuiltinFuncMeta {
                        category: "Operator".into(),
                        domain: "Predicate".into(),
                    },
                    0,
                );
                return VisitorResult::Continue;
            }
            _ => {}
        }

        // ── Existing FunctionCall + SpecialFunction handling ──
        if let Expr::FunctionCall { name, builtin, .. } = expr {
            if name.is_empty() {
                return VisitorResult::Continue;
            }
            let first = name[0].to_lowercase();
            if self.local_vars.contains(&first) || self.known_types.contains(&first) {
                return VisitorResult::Continue;
            }
            match builtin {
                None => {
                    self.push_call(&name.join("."), false, 0);
                }
                Some(meta) => {
                    if meta.category != "Hint" {
                        self.push_builtin_call(&name.join("."), meta.clone(), 0);
                    }
                }
            }
        } else if let Expr::SpecialFunction {
            name,
            builtin: Some(meta),
            ..
        } = expr
        {
            self.push_builtin_call(name, meta.clone(), 0);
        }
        VisitorResult::Continue
    }
}

impl CallExtractor {
    fn extract_func_from_table_ref(&mut self, tr: &AstTableRef) {
        match tr {
            AstTableRef::FunctionCall { name, .. } => {
                let callee: String = name.join(".");
                self.push_call(&callee, false, 0);
            }
            AstTableRef::Join { left, right, .. } => {
                self.extract_func_from_table_ref(left);
                self.extract_func_from_table_ref(right);
            }
            AstTableRef::Subquery { query, .. } => {
                let stmt = Statement::Select(ogsql_parser::ast::Spanned {
                    node: query.as_ref().clone(),
                    span: None,
                });
                ogsql_parser::walk_statement(self, &stmt);
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone)]
pub struct TypeRef {
    pub type_name: String,
    pub context: String,
}

#[derive(Debug, Clone)]
pub struct SequenceRef {
    pub sequence_name: String,
    pub via: SequenceRefVia,
}

#[derive(Debug, Clone, Copy)]
pub enum SequenceRefVia {
    Nextval,
    Currval,
    Setval,
    DotNextval,
    DotCurrval,
}

pub struct TypeSequenceRefExtractor {
    pub known_types: HashSet<String>,
    pub type_refs: Vec<TypeRef>,
    pub sequence_refs: Vec<SequenceRef>,
    pub current_context: String,
}

impl TypeSequenceRefExtractor {
    pub fn new(known_types: HashSet<String>) -> Self {
        Self {
            known_types,
            type_refs: Vec::new(),
            sequence_refs: Vec::new(),
            current_context: String::new(),
        }
    }

    fn is_known_type(&self, name: &str) -> bool {
        self.known_types.contains(&name.to_lowercase()) || self.known_types.contains(name)
    }

    fn resolve_sequence_name(name: &str) -> String {
        name.trim_matches('\'').trim_matches('"').to_string()
    }

    fn extract_sequence_from_expr(&mut self, expr: &Expr, via: SequenceRefVia) {
        match expr {
            Expr::Literal(Literal::String(s)) => {
                self.sequence_refs.push(SequenceRef {
                    sequence_name: Self::resolve_sequence_name(s),
                    via,
                });
            }
            Expr::ColumnRef(name) | Expr::PlVariable(name) if !name.is_empty() => {
                let seq_name = name.join(".");
                self.sequence_refs.push(SequenceRef {
                    sequence_name: Self::resolve_sequence_name(&seq_name),
                    via,
                });
            }
            _ => {}
        }
    }
}

impl Visitor for TypeSequenceRefExtractor {
    fn visit_pl_declaration(
        &mut self,
        decl: &ogsql_parser::ast::plpgsql::PlDeclaration,
    ) -> VisitorResult {
        if let ogsql_parser::ast::plpgsql::PlDeclaration::Variable(var_decl) = decl {
            if let ogsql_parser::ast::plpgsql::PlDataType::TypeName(type_name) = &var_decl.data_type
            {
                if self.is_known_type(type_name) {
                    self.type_refs.push(TypeRef {
                        type_name: type_name.clone(),
                        context: self.current_context.clone(),
                    });
                }
            }
        }
        VisitorResult::Continue
    }

    fn visit_expr(&mut self, expr: &Expr) -> VisitorResult {
        match expr {
            Expr::FunctionCall { name, args, .. } if !name.is_empty() => {
                let func_name = name[name.len() - 1].to_lowercase();
                match func_name.as_str() {
                    "nextval" if !args.is_empty() => {
                        self.extract_sequence_from_expr(&args[0], SequenceRefVia::Nextval);
                    }
                    "currval" if !args.is_empty() => {
                        self.extract_sequence_from_expr(&args[0], SequenceRefVia::Currval);
                    }
                    "setval" if !args.is_empty() => {
                        self.extract_sequence_from_expr(&args[0], SequenceRefVia::Setval);
                    }
                    _ => {}
                }
            }
            Expr::FieldAccess { object, field } => {
                let field_upper = field.to_uppercase();
                let via = match field_upper.as_str() {
                    "NEXTVAL" => Some(SequenceRefVia::DotNextval),
                    "CURRVAL" => Some(SequenceRefVia::DotCurrval),
                    _ => None,
                };
                if let Some(via) = via {
                    if let Expr::ColumnRef(name) | Expr::PlVariable(name) = object.as_ref() {
                        if !name.is_empty() {
                            let seq_name = name.join(".");
                            self.sequence_refs.push(SequenceRef {
                                sequence_name: Self::resolve_sequence_name(&seq_name),
                                via,
                            });
                        }
                    }
                }
            }
            Expr::SequenceValue { sequence, function } => {
                if !sequence.is_empty() {
                    let via = match function {
                        SequenceFunc::Nextval => SequenceRefVia::DotNextval,
                        SequenceFunc::Currval => SequenceRefVia::DotCurrval,
                    };
                    let seq_name = sequence.join(".");
                    self.sequence_refs.push(SequenceRef {
                        sequence_name: Self::resolve_sequence_name(&seq_name),
                        via,
                    });
                }
            }
            Expr::ColumnRef(name) if name.len() >= 2 => {
                let last = name[name.len() - 1].to_uppercase();
                let via = match last.as_str() {
                    "NEXTVAL" => Some(SequenceRefVia::DotNextval),
                    "CURRVAL" => Some(SequenceRefVia::DotCurrval),
                    _ => None,
                };
                if let Some(via) = via {
                    let seq_name = name[..name.len() - 1].join(".");
                    self.sequence_refs.push(SequenceRef {
                        sequence_name: Self::resolve_sequence_name(&seq_name),
                        via,
                    });
                }
            }
            Expr::TypeCast {
                type_name: DataType::Custom(name, ..),
                ..
            } => {
                let type_name_str = name.join(".");
                if self.is_known_type(&type_name_str) {
                    self.type_refs.push(TypeRef {
                        type_name: type_name_str,
                        context: self.current_context.clone(),
                    });
                }
            }
            _ => {}
        }
        VisitorResult::Continue
    }
}

#[derive(Debug, Clone)]
pub struct TableAccessInfo {
    pub name: String,
    pub schema: Option<String>,
    pub modes: AccessMode,
    pub write_kinds: HashSet<WriteKind>,
}

pub struct TableAccessExtractor {
    pub accesses: Vec<TableAccessInfo>,
    /// Stack of CTE names per nesting level. Each `visit_select` /
    /// `visit_insert` / `visit_update` / `visit_delete` pushes its local
    /// WITH-clause CTE names; when children (CTE bodies, subqueries,
    /// source SELECTs) are walked, the full stack is consulted so that
    /// recursive CTE self-references and INSERT...WITH source references
    /// are correctly filtered.
    cte_scope: Vec<HashSet<String>>,
}

impl TableAccessExtractor {
    pub fn new() -> Self {
        Self {
            accesses: Vec::new(),
            cte_scope: Vec::new(),
        }
    }

    fn add_access(&mut self, name: &ObjectName, mode: AccessMode, write_kind: Option<WriteKind>) {
        if name.is_empty() {
            return;
        }
        let (schema, table_name) = if name.len() == 1 {
            (None, name[0].to_string())
        } else {
            (
                Some(name[..name.len() - 1].join(".")),
                name[name.len() - 1].to_string(),
            )
        };

        if let Some(existing) = self
            .accesses
            .iter_mut()
            .find(|t| t.schema == schema && t.name == table_name)
        {
            existing.modes |= mode;
            if let Some(wk) = write_kind {
                existing.write_kinds.insert(wk);
            }
            return;
        }

        let mut write_kinds = HashSet::new();
        if let Some(wk) = write_kind {
            write_kinds.insert(wk);
        }
        self.accesses.push(TableAccessInfo {
            name: table_name,
            schema,
            modes: mode,
            write_kinds,
        });
    }

    /// Check whether `name` matches any CTE name currently in scope.
    fn is_cte_reference(&self, name: &ObjectName) -> bool {
        if name.is_empty() {
            return false;
        }
        let table_name = name[name.len() - 1].to_lowercase();
        self.cte_scope
            .iter()
            .any(|scope| scope.contains(&table_name))
    }

    fn extract_reads_from_table_refs(&mut self, table_refs: &[AstTableRef]) {
        for tr in table_refs {
            match tr {
                AstTableRef::Table { name, .. } => {
                    if !self.is_cte_reference(name) {
                        self.add_access(name, AccessMode::Read, None);
                    }
                }
                AstTableRef::Join { left, right, .. } => {
                    self.extract_reads_from_table_refs(std::slice::from_ref(left));
                    self.extract_reads_from_table_refs(std::slice::from_ref(right));
                }
                AstTableRef::Pivot { source, .. } | AstTableRef::Unpivot { source, .. } => {
                    self.extract_reads_from_table_refs(std::slice::from_ref(source));
                }
                AstTableRef::Subquery { query, .. } => {
                    let stmt = Statement::Select(ogsql_parser::ast::Spanned {
                        node: query.as_ref().clone(),
                        span: None,
                    });
                    ogsql_parser::walk_statement(self, &stmt);
                }
                _ => {}
            }
        }
    }

    fn extract_writes_from_table_refs(&mut self, table_refs: &[AstTableRef], kind: WriteKind) {
        for tr in table_refs {
            match tr {
                AstTableRef::Table { name, .. } => {
                    if !self.is_cte_reference(name) {
                        self.add_access(name, AccessMode::Write, Some(kind));
                    }
                }
                AstTableRef::Join { left, right, .. } => {
                    self.extract_writes_from_table_refs(std::slice::from_ref(left), kind);
                    self.extract_writes_from_table_refs(std::slice::from_ref(right), kind);
                }
                AstTableRef::Pivot { source, .. } | AstTableRef::Unpivot { source, .. } => {
                    self.extract_writes_from_table_refs(std::slice::from_ref(source), kind);
                }
                AstTableRef::Subquery { query, .. } => {
                    let stmt = Statement::Select(ogsql_parser::ast::Spanned {
                        node: query.as_ref().clone(),
                        span: None,
                    });
                    ogsql_parser::walk_statement(self, &stmt);
                }
                _ => {}
            }
        }
    }

    fn extract_lock_read_from_object_name(&mut self, name: &ObjectName) {
        self.add_access(name, AccessMode::LockRead, None);
    }

    /// Push local CTE names onto the scope stack. Children walked while
    /// this scope is active will see these names and filter them.
    fn push_cte_scope(&mut self, names: HashSet<String>) {
        self.cte_scope.push(names);
    }

    fn pop_cte_scope(&mut self) {
        self.cte_scope.pop();
    }

    /// Walk the bodies of all CTEs in a WITH clause. Must be called
    /// while the CTE scope is active so that recursive CTE
    /// self-references inside the bodies are filtered.
    fn walk_cte_bodies(&mut self, with_clause: &WithClause) {
        for cte in &with_clause.ctes {
            let stmt = Statement::Select(ogsql_parser::ast::Spanned {
                node: cte.query.as_ref().clone(),
                span: None,
            });
            ogsql_parser::walk_statement(self, &stmt);
        }
    }
}

impl Visitor for TableAccessExtractor {
    fn visit_statement(&mut self, stmt: &Statement) -> VisitorResult {
        match stmt {
            Statement::Truncate(truncate) => {
                for table in &truncate.tables {
                    self.add_access(table, AccessMode::Truncate, Some(WriteKind::Truncate));
                }
            }
            Statement::Merge(merge) => {
                if let AstTableRef::Table { name, .. } = &merge.target {
                    for clause in &merge.when_clauses {
                        let kind = match &clause.action {
                            ogsql_parser::ast::MergeAction::Update(_) => WriteKind::MergeUpdate,
                            ogsql_parser::ast::MergeAction::Delete => WriteKind::MergeDelete,
                            ogsql_parser::ast::MergeAction::Insert { .. } => WriteKind::MergeInsert,
                        };
                        self.add_access(name, AccessMode::Write, Some(kind));
                    }
                }
                if let AstTableRef::Table { name, .. } = &merge.source {
                    self.add_access(name, AccessMode::Read, None);
                }
            }
            Statement::InsertAll(insert_all) => {
                for target in &insert_all.targets {
                    self.add_access(&target.table, AccessMode::Write, Some(WriteKind::Insert));
                }
                for cond in &insert_all.conditions {
                    for target in &cond.targets {
                        self.add_access(&target.table, AccessMode::Write, Some(WriteKind::Insert));
                    }
                }
                for target in &insert_all.else_targets {
                    self.add_access(&target.table, AccessMode::Write, Some(WriteKind::Insert));
                }
                let stmt = Statement::Select(ogsql_parser::ast::Spanned {
                    node: insert_all.source.as_ref().clone(),
                    span: None,
                });
                ogsql_parser::walk_statement(self, &stmt);
            }
            Statement::InsertFirst(insert_first) => {
                for cond in &insert_first.when_clauses {
                    for target in &cond.targets {
                        self.add_access(&target.table, AccessMode::Write, Some(WriteKind::Insert));
                    }
                }
                for target in &insert_first.else_targets {
                    self.add_access(&target.table, AccessMode::Write, Some(WriteKind::Insert));
                }
                let stmt = Statement::Select(ogsql_parser::ast::Spanned {
                    node: insert_first.source.as_ref().clone(),
                    span: None,
                });
                ogsql_parser::walk_statement(self, &stmt);
            }
            _ => {}
        }
        VisitorResult::Continue
    }

    fn visit_select(&mut self, select: &SelectStatement) -> VisitorResult {
        let local_cte_names: HashSet<String> = select
            .with
            .as_ref()
            .map(|w| w.ctes.iter().map(|c| c.name.to_lowercase()).collect())
            .unwrap_or_default();

        self.push_cte_scope(local_cte_names);

        // Every path below returns SkipChildren, so walk_select never reaches the
        // set_operation branches. Pick up the UNION/INTERSECT/EXCEPT right-hand sides
        // here — inside the CTE scope, which stays visible to all branches. The
        // recursive call follows the rest of the right-nested chain.
        if let Some(ref set_op) = select.set_operation {
            let right = match set_op {
                ogsql_parser::ast::SetOperation::Union { right, .. }
                | ogsql_parser::ast::SetOperation::Intersect { right, .. }
                | ogsql_parser::ast::SetOperation::Except { right, .. } => right.as_ref(),
            };
            self.visit_select(right);
        }

        if let Some(ref into) = select.into_table {
            self.add_access(
                &into.table_name,
                AccessMode::Write,
                Some(WriteKind::SelectInto),
            );
        }

        if let Some(ref lock) = select.lock_clause {
            match lock {
                ogsql_parser::ast::LockClause::Update { tables, .. } if tables.is_empty() => {
                    self.extract_reads_from_table_refs(&select.from);
                    for tr in &select.from {
                        if let AstTableRef::Table { name, .. } = tr {
                            if !self.is_cte_reference(name) {
                                self.extract_lock_read_from_object_name(name);
                            }
                        }
                    }
                    if let Some(ref w) = select.with {
                        self.walk_cte_bodies(w);
                    }
                    self.pop_cte_scope();
                    return VisitorResult::SkipChildren;
                }
                ogsql_parser::ast::LockClause::Update { tables, .. } => {
                    self.extract_reads_from_table_refs(&select.from);
                    for name in tables {
                        self.extract_lock_read_from_object_name(name);
                    }
                    if let Some(ref w) = select.with {
                        self.walk_cte_bodies(w);
                    }
                    self.pop_cte_scope();
                    return VisitorResult::SkipChildren;
                }
                ogsql_parser::ast::LockClause::Share { tables, .. } => {
                    self.extract_reads_from_table_refs(&select.from);
                    for name in tables {
                        self.extract_lock_read_from_object_name(name);
                    }
                    if let Some(ref w) = select.with {
                        self.walk_cte_bodies(w);
                    }
                    self.pop_cte_scope();
                    return VisitorResult::SkipChildren;
                }
                ogsql_parser::ast::LockClause::NoKeyUpdate { tables, .. } => {
                    self.extract_reads_from_table_refs(&select.from);
                    for name in tables {
                        self.extract_lock_read_from_object_name(name);
                    }
                    if let Some(ref w) = select.with {
                        self.walk_cte_bodies(w);
                    }
                    self.pop_cte_scope();
                    return VisitorResult::SkipChildren;
                }
                ogsql_parser::ast::LockClause::KeyShare { tables, .. } => {
                    self.extract_reads_from_table_refs(&select.from);
                    for name in tables {
                        self.extract_lock_read_from_object_name(name);
                    }
                    if let Some(ref w) = select.with {
                        self.walk_cte_bodies(w);
                    }
                    self.pop_cte_scope();
                    return VisitorResult::SkipChildren;
                }
            }
        }

        self.extract_reads_from_table_refs(&select.from);

        if let Some(ref w) = select.with {
            self.walk_cte_bodies(w);
        }

        self.pop_cte_scope();
        VisitorResult::SkipChildren
    }

    fn visit_insert(&mut self, insert: &ogsql_parser::ast::InsertStatement) -> VisitorResult {
        let cte_names: HashSet<String> = insert
            .with
            .as_ref()
            .map(|w| w.ctes.iter().map(|c| c.name.to_lowercase()).collect())
            .unwrap_or_default();

        self.push_cte_scope(cte_names);

        let write_kind = match &insert.source {
            ogsql_parser::ast::InsertSource::Select(_) => WriteKind::InsertSelect,
            _ => WriteKind::Insert,
        };
        self.add_access(&insert.table, AccessMode::Write, Some(write_kind));

        if let Some(ref w) = insert.with {
            self.walk_cte_bodies(w);
        }

        if let ogsql_parser::ast::InsertSource::Select(ref select_stmt) = insert.source {
            let stmt = Statement::Select(ogsql_parser::ast::Spanned {
                node: select_stmt.as_ref().clone(),
                span: None,
            });
            ogsql_parser::walk_statement(self, &stmt);
        }

        self.pop_cte_scope();
        VisitorResult::SkipChildren
    }

    fn visit_update(&mut self, update: &ogsql_parser::ast::UpdateStatement) -> VisitorResult {
        let cte_names: HashSet<String> = update
            .with
            .as_ref()
            .map(|w| w.ctes.iter().map(|c| c.name.to_lowercase()).collect())
            .unwrap_or_default();

        if !cte_names.is_empty() {
            self.push_cte_scope(cte_names);
            if let Some(ref w) = update.with {
                self.walk_cte_bodies(w);
            }
            self.extract_writes_from_table_refs(&update.tables, WriteKind::Update);
            self.extract_reads_from_table_refs(&update.from);
            self.pop_cte_scope();
        } else {
            self.extract_writes_from_table_refs(&update.tables, WriteKind::Update);
            self.extract_reads_from_table_refs(&update.from);
        }

        VisitorResult::Continue
    }

    fn visit_delete(&mut self, delete: &ogsql_parser::ast::DeleteStatement) -> VisitorResult {
        let cte_names: HashSet<String> = delete
            .with
            .as_ref()
            .map(|w| w.ctes.iter().map(|c| c.name.to_lowercase()).collect())
            .unwrap_or_default();

        if !cte_names.is_empty() {
            self.push_cte_scope(cte_names);
            if let Some(ref w) = delete.with {
                self.walk_cte_bodies(w);
            }
            self.extract_writes_from_table_refs(&delete.tables, WriteKind::Delete);
            self.extract_reads_from_table_refs(&delete.using);
            self.pop_cte_scope();
        } else {
            self.extract_writes_from_table_refs(&delete.tables, WriteKind::Delete);
            self.extract_reads_from_table_refs(&delete.using);
        }

        VisitorResult::Continue
    }
}

// ── Column-level analysis ───────────────────────────────────

/// Column-level analysis result from a single walk.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct ColumnAnalysis {
    /// Alias → (schema, table_name)
    pub alias_map: BTreeMap<String, TableAlias>,
    /// All column references (deduplicated).
    pub column_refs: Vec<ColumnRef>,
    /// Equi-join conditions (col = col).
    pub join_conditions: Vec<JoinCondition>,
    /// WHERE clause hard-coded filters (col = literal).
    pub hard_filters: Vec<HardFilter>,
    /// CASE/DECODE enum value mappings.
    pub enum_mappings: Vec<EnumMapping>,
    /// SELECT INTO variable assignments.
    pub select_into: Vec<SelectIntoMapping>,
    /// INSERT statement column lists.
    pub insert_columns: Vec<InsertColumnInfo>,
    /// UPDATE statement SET columns.
    pub update_columns: Vec<UpdateColumnInfo>,
    /// Per-column data flow: which sources feed each written column.
    #[serde(default)]
    pub column_mappings: Vec<ColumnMapping>,
    /// Names of the OTHER tables touched by the same statement as this edge (issue #147).
    /// Populated by the builder on every TableAccess edge of a statement; it is what lets
    /// lineage restrict hops to tables read in the same statement as a write, instead of
    /// connecting all of a routine's reads to all of its writes.
    ///
    /// `None` = not populated (store built before #147) — lineage falls back to
    /// connecting all of a routine's reads/writes. `Some(vec![])` = populated and the
    /// statement genuinely touches no other table (e.g. a bare `UPDATE t SET ...`) — a
    /// legitimate empty hop set, NOT a reason to fall back.
    #[serde(default)]
    pub read_tables: Option<Vec<String>>,
}

/// One written column and the sources its value is built from.
///
/// `INSERT INTO t (a) SELECT b FROM s` yields one mapping: target `t.a`, source `s.b`,
/// kind `Direct`. A bare `SELECT b AS a FROM s` (a view body, or a union branch) yields
/// the same mapping with `target_table: None` — the enclosing object names the table.
///
/// A union contributes one mapping per branch, all sharing a target. Merging them is left
/// to the consumer so that each branch keeps its own [`MappingKind`] and expression text.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ColumnMapping {
    /// Table written to. None when the statement itself does not name one.
    pub target_table: Option<String>,
    pub target_column: String,
    /// 0-based position in the producing SELECT list.
    ///
    /// Authoritative for a bare SELECT, where `target_column` is only that branch's own
    /// alias or column name: in `SELECT a FROM t1 UNION ALL SELECT b FROM t2` the output
    /// column is named by the first branch, so the second branch's `target_column` of
    /// `b` is a local name, not the result's. Aligning by position is what identifies
    /// them as the same output column.
    #[serde(default)]
    pub position: Option<usize>,
    pub sources: Vec<ColumnSource>,
    pub kind: MappingKind,
    /// Source expression text, when the value is not a plain column reference.
    pub expression: Option<String>,
}

/// Where one input of a [`ColumnMapping`] comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ColumnSource {
    Column {
        /// Resolved via `alias_map`. None when the reference is unqualified and the
        /// statement reads more than one table, so the owner is ambiguous.
        table: Option<String>,
        column: String,
    },
    Literal {
        value: String,
    },
    /// A PL/SQL variable or cursor field that could not be traced back to a column.
    Variable {
        name: String,
    },
    /// Assembled by dynamic SQL — the value is not statically knowable.
    Dynamic,
}

/// How a target column's value relates to its sources.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum MappingKind {
    /// Plain copy of a single column, with no transformation.
    Direct,
    /// Computed — arithmetic, `decode`/`nvl`/`CASE`, a function call, a cast.
    Derived,
    /// Produced by an aggregate function.
    Aggregated { function: String, distinct: bool },
}

/// Table alias definition.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TableAlias {
    pub schema: Option<String>,
    pub table: String,
}

/// One output column of a cursor's SELECT, with its resolved source.
#[derive(Debug, Clone)]
pub struct CursorColumn {
    pub output_name: String,
    pub source_table: Option<String>,
    pub source_col: String,
}

/// Column reference.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ColumnRef {
    /// Resolved table name (via alias_map). None if unresolvable or unprefixed.
    pub resolved_table: Option<String>,
    /// Original alias prefix (e.g. "t", "fi").
    pub alias_prefix: Option<String>,
    /// Column name.
    pub column: String,
    /// Contexts where this column appears.
    pub contexts: Vec<ColumnContext>,
}

/// Where a column reference appears.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum ColumnContext {
    SelectTarget,
    JoinCondition,
    WhereClause,
    OrderBy,
    GroupBy,
    Having,
    CorrelatedSubquery,
    InsertTarget,
    UpdateSet,
}

/// Equi-join condition.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct JoinCondition {
    pub left_table: String,
    pub left_column: String,
    pub right_table: String,
    pub right_column: String,
    pub join_type: JoinType,
    pub source: JoinConditionSource,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum JoinType {
    Inner,
    Left,
    Right,
    Full,
    Cross,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum JoinConditionSource {
    ImplicitWhere,
    ExplicitOn,
}

/// WHERE clause hard-coded filter.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HardFilter {
    pub table: Option<String>,
    pub column: String,
    pub operator: FilterOperator,
    pub value: FilterValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FilterOperator {
    Eq,
    Neq,
    Gt,
    Gte,
    Lt,
    Lte,
    Like,
    NotLike,
    In,
    Between,
    IsNull,
    IsNotNull,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FilterValue {
    String(String),
    Integer(i64),
    Float(String),
    Boolean(bool),
    Null,
    List(Vec<FilterValue>),
    Expression(String),
}

/// CASE/DECODE enum value mapping.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EnumMapping {
    pub column: String,
    pub table_alias: Option<String>,
    pub values: Vec<(FilterValue, String)>,
    pub has_else: bool,
}

/// SELECT INTO variable assignment.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SelectIntoMapping {
    pub column_expr: String,
    pub into_variable: String,
}

/// INSERT column info.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InsertColumnInfo {
    pub table: String,
    pub columns: Vec<String>,
}

/// UPDATE SET column info.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UpdateColumnInfo {
    pub table: String,
    pub set_columns: Vec<String>,
}

/// Procedure-level variable context collected by a pre-pass and seeded into per-statement
/// walks (issue #147): cursor sources, FETCH chains, `%ROWTYPE` records and `%TYPE`
/// anchors. Per-statement walks would otherwise lose these cross-statement bindings
/// (a cursor is declared in DECLARE, fetched in one statement, consumed in another).
#[derive(Debug, Clone, Default)]
pub struct ProcedureVarContext {
    /// Cursor name → the columns its SELECT produces (resolved to table.column).
    pub cursor_sources: HashMap<String, Vec<CursorColumn>>,
    /// Cursor name → the variables its FETCH reads into, in order.
    pub fetch_vars: HashMap<String, Vec<String>>,
    /// `%ROWTYPE` record variable → its cursor name.
    pub record_cursors: HashMap<String, String>,
    /// Variable → literal string values (for dynamic-SQL cursor resolution).
    pub var_values: HashMap<String, HashSet<String>>,
    /// Variable → the single distinct table found in its literal SQL fragments (partial
    /// dynamic-SQL resolution when the full statement cannot be statically built).
    pub dynamic_table_vars: HashMap<String, String>,
}

/// Column-level SQL dependency extractor.
///
/// Usage (same pattern as TableAccessExtractor):
///   let mut ext = ColumnAccessExtractor::new();
///   walk_statement(&mut ext, &statement);
///   let analysis = ext.finish();
pub struct ColumnAccessExtractor {
    alias_map: BTreeMap<String, TableAlias>,
    column_refs: Vec<ColumnRef>,
    join_conditions: Vec<JoinCondition>,
    hard_filters: Vec<HardFilter>,
    enum_mappings: Vec<EnumMapping>,
    select_into: Vec<SelectIntoMapping>,
    insert_columns: Vec<InsertColumnInfo>,
    update_columns: Vec<UpdateColumnInfo>,
    column_mappings: Vec<ColumnMapping>,
    /// The one table a bare column reference belongs to, for the select currently being
    /// mapped. Scoped per union branch: without that, branch 2 would see branch 1's table
    /// too and treat its unqualified columns as ambiguous.
    scope_sole_table: Option<String>,
    clause_stack: Vec<ColumnContext>,
    /// CTE names in scope, so a WITH reference is not reported as a real source table.
    cte_scope: Vec<HashSet<String>>,
    /// Cursor name → the columns its SELECT produces (resolved to table.column).
    cursor_sources: HashMap<String, Vec<CursorColumn>>,
    /// Cursor name → the variables its FETCH reads into, in order.
    fetch_vars: HashMap<String, Vec<String>>,
    /// `%ROWTYPE` record variable → its cursor name.
    record_cursors: HashMap<String, String>,
    /// Variable → literal string values (dynamic-SQL cursor resolution).
    var_values: HashMap<String, HashSet<String>>,
    /// Variable → single distinct table in its literal SQL fragments.
    dynamic_table_vars: HashMap<String, String>,
    /// The cursor whose SELECT is currently being walked.
    current_cursor: Option<String>,
}

impl ColumnAccessExtractor {
    pub fn new() -> Self {
        Self {
            alias_map: BTreeMap::new(),
            column_refs: Vec::new(),
            join_conditions: Vec::new(),
            hard_filters: Vec::new(),
            enum_mappings: Vec::new(),
            select_into: Vec::new(),
            insert_columns: Vec::new(),
            update_columns: Vec::new(),
            column_mappings: Vec::new(),
            scope_sole_table: None,
            clause_stack: Vec::new(),
            cte_scope: Vec::new(),
            cursor_sources: HashMap::new(),
            fetch_vars: HashMap::new(),
            record_cursors: HashMap::new(),
            var_values: HashMap::new(),
            dynamic_table_vars: HashMap::new(),
            current_cursor: None,
        }
    }

    /// Build an extractor pre-seeded with procedure variable context collected by a
    /// procedure-level pass (issue #147): a per-statement walk would otherwise lose the
    /// cross-statement cursor → FETCH → INSERT chain and `%ROWTYPE`/`%TYPE` anchors.
    pub fn new_with_context(ctx: &ProcedureVarContext) -> Self {
        let mut ext = Self::new();
        ext.cursor_sources = ctx.cursor_sources.clone();
        ext.fetch_vars = ctx.fetch_vars.clone();
        ext.record_cursors = ctx.record_cursors.clone();
        ext.var_values = ctx.var_values.clone();
        ext.dynamic_table_vars = ctx.dynamic_table_vars.clone();
        ext
    }

    /// The collected procedure variable context (cloned), for seeding per-statement
    /// walks from a procedure-level pass.
    pub fn procedure_context(&self) -> ProcedureVarContext {
        ProcedureVarContext {
            cursor_sources: self.cursor_sources.clone(),
            fetch_vars: self.fetch_vars.clone(),
            record_cursors: self.record_cursors.clone(),
            var_values: self.var_values.clone(),
            dynamic_table_vars: self.dynamic_table_vars.clone(),
        }
    }

    pub fn finish(mut self) -> ColumnAnalysis {
        self.resolve_cursor_flows();
        ColumnAnalysis {
            column_refs: dedup_column_refs(self.column_refs),
            alias_map: self.alias_map,
            join_conditions: self.join_conditions,
            hard_filters: self.hard_filters,
            enum_mappings: self.enum_mappings,
            select_into: self.select_into,
            insert_columns: self.insert_columns,
            update_columns: self.update_columns,
            column_mappings: self.column_mappings,
            read_tables: None,
        }
    }

    fn current_clause(&self) -> Option<ColumnContext> {
        self.clause_stack.last().copied()
    }

    fn resolve_alias(&self, prefix: &str) -> Option<&TableAlias> {
        self.alias_map.get(&prefix.to_lowercase())
    }

    fn is_cte_reference(&self, name: &ogsql_parser::ast::ObjectName) -> bool {
        if name.is_empty() {
            return false;
        }
        let table_name = name[name.len() - 1].to_lowercase();
        self.cte_scope
            .iter()
            .any(|scope| scope.contains(&table_name))
    }

    fn push_cte_scope(&mut self, names: HashSet<String>) {
        self.cte_scope.push(names);
    }

    fn pop_cte_scope(&mut self) {
        self.cte_scope.pop();
    }

    fn record_cursor(&mut self, name: &str, cols: Vec<CursorColumn>) {
        self.cursor_sources.insert(name.to_lowercase(), cols);
    }

    fn record_fetch(&mut self, cursor_name: &str, vars: Vec<String>) {
        self.fetch_vars.insert(
            cursor_name.to_lowercase(),
            vars.into_iter().map(|v| v.to_lowercase()).collect(),
        );
    }

    /// Resolve `INSERT ... VALUES(v1, v2)` variables back to the columns the cursor
    /// SELECT produced, by FETCH variable position.
    fn resolve_cursor_flows(&mut self) {
        let mut var_to_source: HashMap<String, ColumnSource> = HashMap::new();
        for (cursor, vars) in &self.fetch_vars {
            if let Some(cols) = self.cursor_sources.get(cursor) {
                for (i, var) in vars.iter().enumerate() {
                    // A single catch-all source (empty output name, table attributed from
                    // dynamic-SQL fragments) covers every FETCH position.
                    let col = cols.get(i).or_else(|| {
                        (cols.len() == 1 && cols[0].output_name.is_empty()).then_some(&cols[0])
                    });
                    if let Some(col) = col {
                        if !col.source_col.is_empty() {
                            var_to_source.insert(
                                var.clone(),
                                ColumnSource::Column {
                                    table: col.source_table.clone(),
                                    column: col.source_col.clone(),
                                },
                            );
                        } else if col.source_table.is_some() && col.output_name.is_empty() {
                            // Catch-all from partial dynamic-SQL resolution: the exact
                            // source column is unknown, attribute to the table under the
                            // variable's own name (table-level flow remains accurate).
                            var_to_source.insert(
                                var.clone(),
                                ColumnSource::Column {
                                    table: col.source_table.clone(),
                                    column: var.clone(),
                                },
                            );
                        }
                    }
                }
            }
        }
        if var_to_source.is_empty() {
            return;
        }
        for mapping in &mut self.column_mappings {
            for source in &mut mapping.sources {
                if let ColumnSource::Variable { name } = source {
                    if let Some(col) = var_to_source.get(&name.to_lowercase()) {
                        *source = col.clone();
                    }
                }
            }
        }
    }

    /// Collect the source columns (resolved to table.column) of a cursor SELECT.
    fn collect_cursor_select_sources(&self, select: &SelectStatement) -> Vec<CursorColumn> {
        // A direct single FROM table, or — when the driving table lives inside a derived
        // table (dynamic SQL `SELECT ... FROM (SELECT ... FROM t ...)`) — the single
        // distinct table anywhere in the FROM tree.
        let default_table = single_table_name_of(&select.from).or_else(|| {
            let mut tables: HashSet<String> = HashSet::new();
            collect_from_tables(&select.from, &mut tables);
            if tables.len() == 1 {
                tables.into_iter().next()
            } else {
                None
            }
        });
        let mut sources = Vec::new();
        for target in &select.targets {
            if let SelectTarget::Expr(expr, alias) = target {
                let output_name = alias
                    .as_ref()
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| derive_cursor_output_name(expr));
                let mut cols = collect_expr_columns(expr);
                for (table, _col) in cols.iter_mut() {
                    if let Some(ref alias) = table {
                        if let Some(resolved) = self.resolve_alias(alias) {
                            *table = Some(resolved.table.clone());
                        }
                    } else if let Some(ref dt) = default_table {
                        *table = Some(dt.clone());
                    }
                }
                if let Some((table, col)) = cols.into_iter().next() {
                    sources.push(CursorColumn {
                        output_name,
                        source_table: table,
                        source_col: col,
                    });
                }
            }
        }
        sources
    }

    /// The sole non-CTE table of a FROM list, or None when zero or several are in scope.
    fn scope_sole_table_of(&self, refs: &[AstTableRef]) -> Option<String> {
        let mut tables = refs.iter().filter_map(|tr| match tr {
            AstTableRef::Table { name, .. } if !self.is_cte_reference(name) => {
                Some(split_schema_table(name).1)
            }
            _ => None,
        });
        let first = tables.next()?;
        if tables.any(|t| !t.eq_ignore_ascii_case(&first)) {
            return None;
        }
        Some(first)
    }

    fn add_column_ref(
        &mut self,
        names: &[ogsql_parser::Ident],
        extra_context: Option<ColumnContext>,
    ) {
        let (alias_prefix, column) = if names.len() >= 2 {
            (
                Some(names[0].to_string()),
                names[names.len() - 1].to_string(),
            )
        } else {
            (None, names[0].to_string())
        };
        let resolved_table = alias_prefix
            .as_ref()
            .and_then(|a| self.resolve_alias(a))
            .map(|ta| ta.table.clone());
        let ctx = extra_context.unwrap_or(ColumnContext::SelectTarget);
        self.column_refs.push(ColumnRef {
            resolved_table,
            alias_prefix,
            column,
            contexts: vec![ctx],
        });
    }

    fn collect_aliases_from_table_refs(&mut self, refs: &[AstTableRef]) {
        for tr in refs {
            match tr {
                AstTableRef::Table { name, alias, .. } if !self.is_cte_reference(name) => {
                    let (schema, table) = split_schema_table(name);
                    if let Some(a) = alias {
                        self.alias_map
                            .insert(a.to_lowercase(), TableAlias { schema, table });
                    } else {
                        let table_lower = table.to_lowercase();
                        self.alias_map
                            .entry(table_lower)
                            .or_insert(TableAlias { schema, table });
                    }
                }
                AstTableRef::Join {
                    left,
                    right,
                    join_type,
                    condition,
                    ..
                } => {
                    self.collect_aliases_from_table_refs(std::slice::from_ref(left));
                    self.collect_aliases_from_table_refs(std::slice::from_ref(right));
                    if let Some(cond) = condition {
                        self.clause_stack.push(ColumnContext::JoinCondition);
                        self.process_expr_for_joins_and_filters(cond, join_type, true);
                        self.clause_stack.pop();
                    }
                }
                AstTableRef::Subquery { .. } => {}
                AstTableRef::FunctionCall { alias: Some(a), .. } => {
                    self.alias_map.insert(
                        a.to_lowercase(),
                        TableAlias {
                            schema: None,
                            table: String::new(),
                        },
                    );
                }
                AstTableRef::FunctionCall { .. } => {}
                _ => {}
            }
        }
    }

    /// Process an expression looking for equi-join conditions and hard filters.
    fn process_expr_for_joins_and_filters(
        &mut self,
        expr: &Expr,
        join_type: &AstJoinType,
        is_explicit_on: bool,
    ) {
        match expr {
            Expr::BinaryOp { left, op, right } => {
                let op_trimmed = op.trim();
                match op_trimmed {
                    "=" => {
                        if let (Some(l_names), Some(r_names)) =
                            (as_column_ref(left), as_column_ref(right))
                        {
                            // Both sides are column refs → equi-join
                            self.extract_join_condition(
                                &l_names,
                                &r_names,
                                join_type,
                                is_explicit_on,
                            );
                            self.extract_join_condition(
                                &r_names,
                                &l_names,
                                join_type,
                                is_explicit_on,
                            );
                            // Also add column refs in join context
                            self.add_column_ref(&l_names, Some(ColumnContext::JoinCondition));
                            self.add_column_ref(&r_names, Some(ColumnContext::JoinCondition));
                        } else if let Some(col_names) = as_column_ref(left) {
                            if let Some(val) = literal_to_filter_value(right) {
                                self.add_hard_filter(&col_names, FilterOperator::Eq, val);
                            }
                        } else if let Some(col_names) = as_column_ref(right) {
                            if let Some(val) = literal_to_filter_value(left) {
                                self.add_hard_filter(&col_names, FilterOperator::Eq, val);
                            }
                        }
                    }
                    "<>" | "!=" => {
                        if let Some(col_names) = as_column_ref(left) {
                            if let Some(val) = literal_to_filter_value(right) {
                                self.add_hard_filter(&col_names, FilterOperator::Neq, val);
                            }
                        } else if let Some(col_names) = as_column_ref(right) {
                            if let Some(val) = literal_to_filter_value(left) {
                                self.add_hard_filter(&col_names, FilterOperator::Neq, val);
                            }
                        }
                    }
                    ">" => {
                        if let Some(col_names) = as_column_ref(left) {
                            if let Some(val) = literal_to_filter_value(right) {
                                self.add_hard_filter(&col_names, FilterOperator::Gt, val);
                            }
                        }
                    }
                    ">=" => {
                        if let Some(col_names) = as_column_ref(left) {
                            if let Some(val) = literal_to_filter_value(right) {
                                self.add_hard_filter(&col_names, FilterOperator::Gte, val);
                            }
                        }
                    }
                    "<" => {
                        if let Some(col_names) = as_column_ref(left) {
                            if let Some(val) = literal_to_filter_value(right) {
                                self.add_hard_filter(&col_names, FilterOperator::Lt, val);
                            }
                        }
                    }
                    "<=" => {
                        if let Some(col_names) = as_column_ref(left) {
                            if let Some(val) = literal_to_filter_value(right) {
                                self.add_hard_filter(&col_names, FilterOperator::Lte, val);
                            }
                        }
                    }
                    "AND" => {
                        self.process_expr_for_joins_and_filters(left, join_type, is_explicit_on);
                        self.process_expr_for_joins_and_filters(right, join_type, is_explicit_on);
                    }
                    _ => {}
                }
            }
            Expr::Like {
                expr,
                pattern,
                negated,
                ..
            } => {
                if let Some(col_names) = as_column_ref(expr) {
                    if let Some(val) = literal_to_filter_value(pattern) {
                        let op = if *negated {
                            FilterOperator::NotLike
                        } else {
                            FilterOperator::Like
                        };
                        self.add_hard_filter(&col_names, op, val);
                    }
                }
            }
            Expr::Between {
                expr,
                low,
                high,
                negated: _,
            } => {
                if let Some(col_names) = as_column_ref(expr) {
                    if let (Some(lv), Some(hv)) =
                        (literal_to_filter_value(low), literal_to_filter_value(high))
                    {
                        self.add_hard_filter(
                            &col_names,
                            FilterOperator::Between,
                            FilterValue::List(vec![lv, hv]),
                        );
                    }
                }
            }
            Expr::InList {
                expr,
                list,
                negated: _,
            } => {
                if let Some(col_names) = as_column_ref(expr) {
                    let vals: Vec<FilterValue> =
                        list.iter().filter_map(literal_to_filter_value).collect();
                    if vals.len() == list.len() {
                        self.add_hard_filter(
                            &col_names,
                            FilterOperator::In,
                            FilterValue::List(vals),
                        );
                    }
                }
            }
            Expr::IsNull { expr, negated } => {
                if let Some(col_names) = as_column_ref(expr) {
                    let op = if *negated {
                        FilterOperator::IsNotNull
                    } else {
                        FilterOperator::IsNull
                    };
                    self.add_hard_filter(&col_names, op, FilterValue::Null);
                }
            }
            _ => {}
        }
    }

    fn extract_join_condition(
        &mut self,
        left_names: &[ogsql_parser::Ident],
        right_names: &[ogsql_parser::Ident],
        join_type: &AstJoinType,
        is_explicit_on: bool,
    ) {
        let (left_alias, left_col) = split_alias_column(left_names);
        let (right_alias, right_col) = split_alias_column(right_names);
        let left_table = left_alias
            .as_ref()
            .and_then(|a| self.resolve_alias(a))
            .map(|ta| ta.table.clone())
            .unwrap_or_default();
        let right_table = right_alias
            .as_ref()
            .and_then(|a| self.resolve_alias(a))
            .map(|ta| ta.table.clone())
            .unwrap_or_default();

        let jt = match join_type {
            AstJoinType::Inner => JoinType::Inner,
            AstJoinType::Left => JoinType::Left,
            AstJoinType::Right => JoinType::Right,
            AstJoinType::Full => JoinType::Full,
            AstJoinType::Cross => JoinType::Cross,
        };
        let source = if is_explicit_on {
            JoinConditionSource::ExplicitOn
        } else {
            JoinConditionSource::ImplicitWhere
        };

        // Only add if not already present (avoid duplicates from bidirectional extraction)
        let candidate = JoinCondition {
            left_table: left_table.clone(),
            left_column: left_col.clone(),
            right_table: right_table.clone(),
            right_column: right_col.clone(),
            join_type: jt,
            source,
        };
        // Check reverse doesn't already exist
        let already_exists = self.join_conditions.iter().any(|existing| {
            existing.left_table == right_table
                && existing.left_column == right_col
                && existing.right_table == left_table
                && existing.right_column == left_col
        });
        if !already_exists && !left_table.is_empty() && !right_table.is_empty() {
            self.join_conditions.push(candidate);
        }
    }

    fn add_hard_filter(
        &mut self,
        col_names: &[ogsql_parser::Ident],
        op: FilterOperator,
        val: FilterValue,
    ) {
        let (alias_prefix, column) = split_alias_column(col_names);
        let table = alias_prefix
            .as_ref()
            .and_then(|a| self.resolve_alias(a))
            .map(|ta| ta.table.clone());
        self.hard_filters.push(HardFilter {
            table,
            column,
            operator: op,
            value: val,
        });
    }

    fn extract_enum_from_case(
        &mut self,
        operand: &Expr,
        whens: &[WhenClause],
        else_expr: Option<&Expr>,
    ) {
        let (table_alias, column) = match operand {
            Expr::ColumnRef(names) => {
                let (a, c) = split_alias_column(names);
                (a, c)
            }
            _ => return,
        };
        let mut values = Vec::new();
        for wc in whens {
            if let Some(match_val) = literal_to_filter_value(&wc.condition) {
                let label = literal_to_string(&wc.result).unwrap_or_default();
                values.push((match_val, label));
            }
        }
        self.enum_mappings.push(EnumMapping {
            column,
            table_alias,
            values,
            has_else: else_expr.is_some(),
        });
    }

    fn extract_enum_from_decode(&mut self, args: &[Expr]) {
        if args.is_empty() {
            return;
        }
        let (table_alias, column) = match &args[0] {
            Expr::ColumnRef(names) => split_alias_column(names),
            _ => return,
        };
        let rest = &args[1..];
        let mut values = Vec::new();
        let mut has_else = false;
        let chunks = rest.chunks(2);
        for chunk in chunks {
            if chunk.len() == 2 {
                if let Some(match_val) = literal_to_filter_value(&chunk[0]) {
                    let label = literal_to_string(&chunk[1]).unwrap_or_default();
                    values.push((match_val, label));
                }
            } else if chunk.len() == 1 {
                // Default value (odd argument)
                has_else = true;
            }
        }
        self.enum_mappings.push(EnumMapping {
            column,
            table_alias,
            values,
            has_else,
        });
    }
}

impl Visitor for ColumnAccessExtractor {
    fn visit_statement(&mut self, stmt: &Statement) -> VisitorResult {
        if let Statement::Merge(merge) = stmt {
            self.visit_merge_statement(&merge.node);
        }
        VisitorResult::Continue
    }

    fn visit_pl_declaration(&mut self, decl: &PlDeclaration) -> VisitorResult {
        match decl {
            PlDeclaration::Cursor(PlCursorDecl { name, .. }) => {
                self.current_cursor = Some(name.clone());
            }
            PlDeclaration::Variable(v) => {
                use ogsql_parser::ast::plpgsql::PlDataType;
                // `rec cursor_name%ROWTYPE`: record fields resolve via the cursor's
                // SELECT sources (issue #147 L2). `%TYPE` anchors are deliberately NOT
                // resolved: typing a variable as `t.col%TYPE` says nothing about where
                // its value comes from, so resolving it would fabricate data edges.
                if let PlDataType::PercentRowType(cursor) = &v.data_type {
                    self.record_cursors
                        .insert(v.name.to_lowercase(), cursor.to_lowercase());
                }
            }
            _ => {}
        }
        VisitorResult::Continue
    }

    fn visit_pl_statement(&mut self, stmt: &PlStatement) -> VisitorResult {
        match stmt {
            // Track literal-string assignments so `OPEN c FOR v_sql` can resolve the
            // dynamic cursor's SELECT sources (issue #147).
            PlStatement::Assignment {
                target: Expr::PlVariable(names),
                expression,
            } => {
                let var_name = names.join(".").to_lowercase();
                let values = literal_strings(expression, &self.var_values);
                if !values.is_empty() {
                    self.var_values
                        .insert(var_name, values.into_iter().collect());
                } else {
                    // Partial resolution: the concatenation may contain runtime values,
                    // but its static fragments still name tables.
                    let mut leaves = Vec::new();
                    literal_leaves(expression, &mut leaves);
                    let mut tables: HashSet<String> = HashSet::new();
                    for frag in &leaves {
                        tables.extend(tables_in_sql_fragment(frag));
                    }
                    if tables.len() == 1 {
                        if let Some(t) = tables.into_iter().next() {
                            self.dynamic_table_vars.insert(var_name, t);
                        }
                    }
                }
            }
            PlStatement::Assignment {
                target: Expr::FieldAccess { object, field },
                expression,
            } => {
                if let Expr::PlVariable(record_name) = object.as_ref() {
                    let compound = format!("{}.{}", record_name.join("."), field).to_lowercase();
                    let values = literal_strings(expression, &self.var_values);
                    if !values.is_empty() {
                        self.var_values
                            .insert(compound, values.into_iter().collect());
                    }
                }
            }
            PlStatement::Fetch(fetch) => {
                let cursor_name = match &fetch.node.cursor {
                    Expr::ColumnRef(parts) | Expr::PlVariable(parts) => {
                        parts.last().map(|i| i.to_string()).unwrap_or_default()
                    }
                    _ => String::new(),
                };
                let vars: Vec<String> = fetch.node.into.iter().map(expr_var_name).collect();
                self.record_fetch(&cursor_name, vars);
            }
            // `OPEN c_fxj FOR v_sql_txt` / `FOR EXECUTE expr`: resolve the dynamic SQL to
            // the cursor's SELECT sources so FETCH-variable chains keep resolving.
            PlStatement::Open(spanned) => {
                use ogsql_parser::ast::plpgsql::PlOpenKind;
                let open = &spanned.node;
                let cursor_name = match &open.cursor {
                    Expr::ColumnRef(parts) | Expr::PlVariable(parts) => {
                        parts.last().map(|i| i.to_string()).unwrap_or_default()
                    }
                    _ => String::new(),
                };
                if cursor_name.is_empty()
                    || self
                        .cursor_sources
                        .contains_key(&cursor_name.to_lowercase())
                {
                    return VisitorResult::Continue;
                }
                let mut candidates: Vec<String> = Vec::new();
                let mut dynamic_table: Option<String> = None;
                match &open.kind {
                    PlOpenKind::ForQuery {
                        parsed_query,
                        query,
                        ..
                    } => {
                        if let Some(Statement::Select(select)) = parsed_query.as_deref() {
                            let sources = self.collect_cursor_select_sources(&select.node);
                            self.record_cursor(&cursor_name, sources);
                            return VisitorResult::Continue;
                        }
                        // The query text may be a variable name holding the SQL literal.
                        if let Some(vals) = self.var_values.get(&query.to_lowercase()) {
                            candidates.extend(vals.iter().cloned());
                        } else {
                            candidates.push(query.clone());
                        }
                        if let Some(t) = self.dynamic_table_vars.get(&query.to_lowercase()) {
                            dynamic_table = Some(t.clone());
                        }
                    }
                    PlOpenKind::ForExecute { query, .. } => {
                        candidates.extend(literal_strings(query, &self.var_values));
                    }
                    _ => {}
                }
                for sql in candidates {
                    let (stmts, _) = ogsql_parser::Parser::parse_sql(&sql);
                    for info in &stmts {
                        if let Statement::Select(select) = &info.statement {
                            let sources = self.collect_cursor_select_sources(&select.node);
                            self.record_cursor(&cursor_name, sources);
                            return VisitorResult::Continue;
                        }
                    }
                }
                // Last resort: the dynamic SQL is built from runtime values, but its
                // literal fragments name a single static table — attribute every cursor
                // output to it (empty output name marks the catch-all).
                if let Some(table) = dynamic_table {
                    self.record_cursor(
                        &cursor_name,
                        vec![CursorColumn {
                            output_name: String::new(),
                            source_table: Some(table),
                            source_col: String::new(),
                        }],
                    );
                }
            }
            // `FOR rec IN (SELECT ...)` — the loop variable is an implicit %ROWTYPE
            // record over the inline query's sources (issue #147 L2).
            PlStatement::For(spanned) => {
                use ogsql_parser::ast::plpgsql::PlForKind;
                if let PlForKind::Query {
                    parsed_query: Some(parsed),
                    ..
                } = &spanned.node.kind
                {
                    if let Statement::Select(select) = &**parsed {
                        let sources = self.collect_cursor_select_sources(&select.node);
                        let loop_var = spanned.node.variable.to_lowercase();
                        self.cursor_sources.insert(loop_var.clone(), sources);
                        self.record_cursors.insert(loop_var.clone(), loop_var);
                    }
                }
            }
            _ => {}
        }
        VisitorResult::Continue
    }

    fn visit_select(&mut self, select: &SelectStatement) -> VisitorResult {
        let local_cte_names: HashSet<String> = select
            .with
            .as_ref()
            .map(|w| w.ctes.iter().map(|c| c.name.to_lowercase()).collect())
            .unwrap_or_default();
        self.push_cte_scope(local_cte_names);

        // Collect aliases from FROM clause
        self.collect_aliases_from_table_refs(&select.from);

        if let Some(cursor_name) = self.current_cursor.clone() {
            let sources = self.collect_cursor_select_sources(select);
            self.record_cursor(&cursor_name, sources);
            self.current_cursor = None;
        }

        // Handle SELECT INTO (PL/pgSQL SELECT col INTO var FROM ...)
        if let Some(ref into_targets) = select.into_targets {
            for (i, target) in select.targets.iter().enumerate() {
                if i < into_targets.len() {
                    let col_expr = format_select_target(target);
                    let var_name = format_select_target(&into_targets[i]);
                    self.select_into.push(SelectIntoMapping {
                        column_expr: col_expr,
                        into_variable: var_name,
                    });
                }
            }
        }

        // Walk SELECT targets for column refs
        self.clause_stack.push(ColumnContext::SelectTarget);
        for target in &select.targets {
            walk_select_target_exprs(self, target);
        }
        self.clause_stack.pop();

        // Walk WHERE clause
        if let Some(ref where_clause) = select.where_clause {
            self.clause_stack.push(ColumnContext::WhereClause);
            self.process_expr_for_joins_and_filters(where_clause, &AstJoinType::Inner, false);
            self.walk_expr_for_column_refs(where_clause);
            self.clause_stack.pop();
        }

        // Walk GROUP BY
        for _item in &select.group_by {}

        // Walk HAVING
        if let Some(ref having) = select.having {
            self.clause_stack.push(ColumnContext::Having);
            self.walk_expr_for_column_refs(having);
            self.clause_stack.pop();
        }

        // Walk ORDER BY
        if !select.order_by.is_empty() {
            self.clause_stack.push(ColumnContext::OrderBy);
            for ob in &select.order_by {
                self.walk_expr_for_column_refs(&ob.expr);
            }
            self.clause_stack.pop();
        }

        self.pop_cte_scope();

        VisitorResult::Continue
    }

    fn visit_insert(&mut self, insert: &InsertStatement) -> VisitorResult {
        let local_cte_names: HashSet<String> = insert
            .with
            .as_ref()
            .map(|w| w.ctes.iter().map(|c| c.name.to_lowercase()).collect())
            .unwrap_or_default();
        self.push_cte_scope(local_cte_names);

        let table_name = insert.table.last().cloned().unwrap_or_default().to_string();
        self.insert_columns.push(InsertColumnInfo {
            table: table_name.clone(),
            columns: insert.columns.clone(),
        });

        if !insert.columns.is_empty() {
            match &insert.source {
                ogsql_parser::ast::InsertSource::Select(select) => {
                    // Each union branch fills the same target columns, so align the
                    // column list against every branch. Aliases must be collected per
                    // branch first: `visit_select` runs later, and until it does the
                    // FROM aliases are unknown, leaving every source table as None.
                    for branch in Self::set_operation_branches(select) {
                        self.collect_aliases_from_table_refs(&branch.from);
                        self.map_columns_positionally(
                            Some(&table_name),
                            &insert.columns,
                            &branch.targets,
                            &branch.from,
                        );
                    }
                }
                ogsql_parser::ast::InsertSource::Values(rows) => {
                    for row in rows {
                        for (position, (column, value)) in
                            insert.columns.iter().zip(row.iter()).enumerate()
                        {
                            self.push_column_mapping(
                                Some(table_name.clone()),
                                column.clone(),
                                Some(position),
                                value,
                            );
                        }
                    }
                }
                // DEFAULT VALUES has no sources; `SET` is handled as assignments; a
                // record variable needs the variable's own type to expand.
                ogsql_parser::ast::InsertSource::DefaultValues
                | ogsql_parser::ast::InsertSource::Set(_)
                | ogsql_parser::ast::InsertSource::RecordVariable(_) => {}
            }
        } else if let ogsql_parser::ast::InsertSource::Select(select) = &insert.source {
            // No column list: name the target columns from the SELECT output (the first
            // branch), then align every branch by position.
            let names = output_column_names(select);
            if !names.is_empty() {
                for branch in Self::set_operation_branches(select) {
                    self.collect_aliases_from_table_refs(&branch.from);
                    self.map_columns_positionally(
                        Some(&table_name),
                        &names,
                        &branch.targets,
                        &branch.from,
                    );
                }
            }
        }

        self.pop_cte_scope();

        VisitorResult::Continue
    }

    fn visit_update(&mut self, update: &UpdateStatement) -> VisitorResult {
        let local_cte_names: HashSet<String> = update
            .with
            .as_ref()
            .map(|w| w.ctes.iter().map(|c| c.name.to_lowercase()).collect())
            .unwrap_or_default();
        self.push_cte_scope(local_cte_names);

        // Collect aliases from the target table(s) and any FROM clause. `UPDATE ... FROM`
        // source tables must be in the alias map or their qualified columns would resolve
        // to the target table.
        for tr in update.tables.iter().chain(update.from.iter()) {
            if let AstTableRef::Table { name, alias, .. } = tr {
                if self.is_cte_reference(name) {
                    continue;
                }
                let (schema, table) = split_schema_table(name);
                match alias {
                    Some(a) => {
                        self.alias_map
                            .insert(a.to_lowercase(), TableAlias { schema, table });
                    }
                    None => {
                        let table_lower = table.to_lowercase();
                        self.alias_map
                            .entry(table_lower)
                            .or_insert(TableAlias { schema, table });
                    }
                }
            }
        }

        let table_name = update
            .tables
            .first()
            .and_then(|tr| {
                if let AstTableRef::Table { name, .. } = tr {
                    Some(name.last().cloned().unwrap_or_default().to_string())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        let set_columns: Vec<String> = update
            .assignments
            .iter()
            .filter_map(|a| a.columns.first()?.last().map(|i| i.to_string()))
            .collect();

        if !table_name.is_empty() {
            self.update_columns.push(UpdateColumnInfo {
                table: table_name.clone(),
                set_columns,
            });
        }

        // SET col = expr. A multi-column form, `SET (a, b) = (SELECT x, y ...)`, has one
        // assignment holding several targets but a single value expression; pairing them
        // by position would need the subquery's select list, so those targets share the
        // whole expression rather than being split.
        let new_scope = if update.from.is_empty() {
            self.scope_sole_table_of(&update.tables)
        } else {
            None
        };
        let previous_scope = std::mem::replace(&mut self.scope_sole_table, new_scope);
        for assignment in &update.assignments {
            for (position, target) in assignment.columns.iter().enumerate() {
                if let Some(column) = target.last() {
                    self.push_column_mapping(
                        Some(table_name.clone()),
                        column.to_string(),
                        Some(position),
                        &assignment.value,
                    );
                }
            }
        }
        self.scope_sole_table = previous_scope;

        // Walk WHERE clause for column refs and filters
        if let Some(ref where_clause) = update.where_clause {
            self.clause_stack.push(ColumnContext::WhereClause);
            self.process_expr_for_joins_and_filters(where_clause, &AstJoinType::Inner, false);
            self.walk_expr_for_column_refs(where_clause);
            self.clause_stack.pop();
        }

        self.pop_cte_scope();

        VisitorResult::Continue
    }

    fn visit_expr(&mut self, expr: &Expr) -> VisitorResult {
        match expr {
            Expr::ColumnRef(names) if !names.is_empty() => {
                let ctx = self.current_clause().unwrap_or(ColumnContext::SelectTarget);
                self.add_column_ref(names, Some(ctx));
            }
            Expr::Case {
                operand: Some(ref op),
                whens,
                else_expr,
            } => {
                self.extract_enum_from_case(op, whens, else_expr.as_ref().map(|e| e.as_ref()));
            }
            Expr::Case { .. } => {}
            Expr::FunctionCall { name, args, .. } => {
                if let Some(func_name) = name.last() {
                    if func_name.eq_ignore_ascii_case("decode") && args.len() >= 3 {
                        self.extract_enum_from_decode(args);
                    }
                }
            }
            _ => {}
        }
        VisitorResult::Continue
    }
}

// ── Column analysis helpers ────────────────────────────────

impl ColumnAccessExtractor {
    /// Resolve a column reference's owning table through `alias_map`.
    ///
    /// An unqualified column falls back to the sole table of the select being mapped: in
    /// `SELECT qsje FROM jsmx_temp` the owner is unambiguous even without a prefix. With
    /// two or more tables in scope it stays unattributed rather than guessed.
    fn column_source(&self, names: &[ogsql_parser::Ident]) -> ColumnSource {
        let (alias_prefix, column) = split_alias_column(names);

        // `%ROWTYPE` record field (issue #147 L2): `rec.id` where rec is a record
        // resolves to the cursor's source column by output name.
        if let Some(record) = &alias_prefix {
            if let Some(cursor) = self.record_cursors.get(&record.to_lowercase()) {
                if let Some(cols) = self.cursor_sources.get(cursor) {
                    if let Some(col) = cols
                        .iter()
                        .find(|c| c.output_name.eq_ignore_ascii_case(&column))
                    {
                        if !col.source_col.is_empty() {
                            return ColumnSource::Column {
                                table: col.source_table.clone(),
                                column: col.source_col.clone(),
                            };
                        }
                    }
                }
            }
        }

        let table = match alias_prefix.as_ref() {
            Some(a) => self.resolve_alias(a).map(|ta| ta.table.clone()),
            None => self.scope_sole_table.clone(),
        };
        ColumnSource::Column { table, column }
    }

    /// Describe how `expr` produces a value: which inputs feed it, and whether it is a
    /// plain copy, a computation, or an aggregate.
    fn classify_value_expr(&self, expr: &Expr) -> (Vec<ColumnSource>, MappingKind, Option<String>) {
        let expr = peel_parenthesized(expr);

        match expr {
            Expr::ColumnRef(names) if !names.is_empty() => {
                (vec![self.column_source(names)], MappingKind::Direct, None)
            }
            Expr::PlVariable(names) if !names.is_empty() => (
                vec![ColumnSource::Variable {
                    name: names.join("."),
                }],
                MappingKind::Direct,
                None,
            ),
            Expr::Literal(lit) => (
                vec![ColumnSource::Literal {
                    value: format_literal_short(lit),
                }],
                MappingKind::Direct,
                None,
            ),
            Expr::FunctionCall {
                name,
                args,
                distinct,
                ..
            } if is_aggregate_function(&name.last().cloned().unwrap_or_default()) => {
                let mut sources = Vec::new();
                for arg in args {
                    self.collect_value_sources(arg, &mut sources);
                }
                let function = name
                    .last()
                    .cloned()
                    .unwrap_or_default()
                    .to_string()
                    .to_uppercase();
                (
                    sources,
                    MappingKind::Aggregated {
                        function,
                        distinct: *distinct,
                    },
                    Some(format_expr_short(expr)),
                )
            }
            _ => {
                let mut sources = Vec::new();
                self.collect_value_sources(expr, &mut sources);
                (sources, MappingKind::Derived, Some(format_expr_short(expr)))
            }
        }
    }

    /// Collect the column and variable leaves that feed a computed expression.
    ///
    /// Literals are deliberately skipped here. Inside `decode(kind, '1', 'A', kind)` the
    /// quoted values are outputs of the branch, not upstream data — recording them would
    /// bury the one input that lineage cares about. A value that is *entirely* a literal
    /// is handled by [`Self::classify_value_expr`] instead, where "this column is a
    /// constant" is the useful answer.
    fn collect_value_sources(&self, expr: &Expr, out: &mut Vec<ColumnSource>) {
        match expr {
            Expr::ColumnRef(names) | Expr::ColumnRefOuterJoin(names) if !names.is_empty() => {
                push_unique_source(out, self.column_source(names));
            }
            Expr::PlVariable(names) if !names.is_empty() => {
                push_unique_source(
                    out,
                    ColumnSource::Variable {
                        name: names.join("."),
                    },
                );
            }
            Expr::BinaryOp { left, right, .. } => {
                self.collect_value_sources(left, out);
                self.collect_value_sources(right, out);
            }
            Expr::UnaryOp { expr, .. } => self.collect_value_sources(expr, out),
            Expr::Like { expr, pattern, .. } => {
                self.collect_value_sources(expr, out);
                self.collect_value_sources(pattern, out);
            }
            Expr::Between {
                expr, low, high, ..
            } => {
                self.collect_value_sources(expr, out);
                self.collect_value_sources(low, out);
                self.collect_value_sources(high, out);
            }
            Expr::InList { expr, list, .. } => {
                self.collect_value_sources(expr, out);
                for item in list {
                    self.collect_value_sources(item, out);
                }
            }
            Expr::IsNull { expr, .. } => self.collect_value_sources(expr, out),
            Expr::FunctionCall { args, .. } => {
                for arg in args {
                    self.collect_value_sources(arg, out);
                }
            }
            Expr::Case {
                operand,
                whens,
                else_expr,
            } => {
                if let Some(ref op) = operand {
                    self.collect_value_sources(op, out);
                }
                for wc in whens {
                    self.collect_value_sources(&wc.condition, out);
                    self.collect_value_sources(&wc.result, out);
                }
                if let Some(ref e) = else_expr {
                    self.collect_value_sources(e, out);
                }
            }
            Expr::InSubquery { expr, .. } => self.collect_value_sources(expr, out),
            Expr::TypeCast { expr, .. } => self.collect_value_sources(expr, out),
            Expr::Parenthesized(inner) => self.collect_value_sources(inner, out),
            Expr::FieldAccess { object, .. } => self.collect_value_sources(object, out),
            // Subqueries carry their own scope; walking them here would resolve their
            // columns against this statement's aliases.
            Expr::Exists(_) | Expr::Subquery(_) => {}
            _ => {}
        }
    }

    /// Extract the column mappings of a view body.
    ///
    /// Called explicitly for `CREATE VIEW` rather than from `visit_select`, so the
    /// `INSERT ... SELECT` path — which already emits table-qualified mappings — does not
    /// also produce a second, table-less set for the same select list.
    ///
    /// `declared_columns` is the view's explicit column list when it has one. Otherwise
    /// the first branch's aliases name the output columns and later union branches align
    /// to them by position, which is the only thing that identifies
    /// `SELECT entrust_type AS bond_kind` in one branch and a bare `bond_kind` in another
    /// as the same output column.
    pub fn extract_view_column_mappings(
        &mut self,
        view_name: &str,
        declared_columns: &[String],
        select: &SelectStatement,
    ) {
        let branches = Self::set_operation_branches(select);

        let names: Vec<String> = if !declared_columns.is_empty() {
            declared_columns.to_vec()
        } else {
            branches
                .first()
                .map(|b| output_column_names(b))
                .unwrap_or_default()
        };
        if names.is_empty() {
            return;
        }

        for branch in branches {
            self.collect_aliases_from_table_refs(&branch.from);
            self.map_columns_positionally(Some(view_name), &names, &branch.targets, &branch.from);
        }
    }

    fn push_column_mapping(
        &mut self,
        target_table: Option<String>,
        target_column: String,
        position: Option<usize>,
        value: &Expr,
    ) {
        let (sources, kind, expression) = self.classify_value_expr(value);
        self.column_mappings.push(ColumnMapping {
            target_table,
            target_column,
            position,
            sources,
            kind,
            expression,
        });
    }

    /// Extract column mappings from a MERGE statement's WHEN clauses.
    fn visit_merge_statement(&mut self, merge: &ogsql_parser::ast::MergeStatement) {
        let target_name = match &merge.target {
            AstTableRef::Table { name, .. } => name.last().cloned().unwrap_or_default().to_string(),
            _ => String::new(),
        };
        if target_name.is_empty() {
            return;
        }

        self.collect_aliases_from_table_refs(std::slice::from_ref(&merge.target));
        self.collect_aliases_from_table_refs(std::slice::from_ref(&merge.source));

        let new_scope = self.scope_sole_table_of(std::slice::from_ref(&merge.source));
        let previous_scope = std::mem::replace(&mut self.scope_sole_table, new_scope);

        for clause in &merge.when_clauses {
            match &clause.action {
                ogsql_parser::ast::MergeAction::Update(assignments) => {
                    for (position, assignment) in assignments.iter().enumerate() {
                        if let Some(column) = assignment.columns.first().and_then(|c| c.last()) {
                            self.push_column_mapping(
                                Some(target_name.clone()),
                                column.to_string(),
                                Some(position),
                                &assignment.value,
                            );
                        }
                    }
                }
                ogsql_parser::ast::MergeAction::Insert { columns, values } => {
                    for (position, (column, value)) in columns.iter().zip(values.iter()).enumerate()
                    {
                        if let Some(col) = column.last() {
                            self.push_column_mapping(
                                Some(target_name.clone()),
                                col.to_string(),
                                Some(position),
                                value,
                            );
                        }
                    }
                }
                ogsql_parser::ast::MergeAction::Delete => {}
            }
        }

        self.scope_sole_table = previous_scope;
    }

    /// Pair a written column list against the expressions that fill it, by position.
    ///
    /// A `SELECT *` target cannot be aligned without a schema, so the whole statement is
    /// skipped rather than emitting mappings shifted by one.
    fn map_columns_positionally(
        &mut self,
        target_table: Option<&str>,
        columns: &[String],
        values: &[SelectTarget],
        from: &[AstTableRef],
    ) {
        if values.iter().any(|t| matches!(t, SelectTarget::Star(_))) {
            return;
        }
        let new_scope = self.scope_sole_table_of(from);
        let previous_scope = std::mem::replace(&mut self.scope_sole_table, new_scope);
        for (position, (column, target)) in columns.iter().zip(values.iter()).enumerate() {
            if column.is_empty() {
                continue;
            }
            if let SelectTarget::Expr(expr, _) = target {
                self.push_column_mapping(
                    target_table.map(|s| s.to_string()),
                    column.clone(),
                    Some(position),
                    expr,
                );
            }
        }
        self.scope_sole_table = previous_scope;
    }

    /// Follow a set-operation chain, yielding each branch in source order (the head
    /// select included).
    fn set_operation_branches(select: &SelectStatement) -> Vec<&SelectStatement> {
        let mut branches = vec![select];
        let mut cur = select;
        while let Some(op) = cur.set_operation.as_ref() {
            let right = match op {
                ogsql_parser::ast::SetOperation::Union { right, .. }
                | ogsql_parser::ast::SetOperation::Intersect { right, .. }
                | ogsql_parser::ast::SetOperation::Except { right, .. } => right.as_ref(),
            };
            branches.push(right);
            cur = right;
        }
        branches
    }

    fn walk_expr_for_column_refs(&mut self, expr: &Expr) {
        match expr {
            Expr::ColumnRef(names) if !names.is_empty() => {
                let ctx = self.current_clause().unwrap_or(ColumnContext::SelectTarget);
                self.add_column_ref(names, Some(ctx));
            }
            Expr::BinaryOp { left, right, .. } => {
                self.walk_expr_for_column_refs(left);
                self.walk_expr_for_column_refs(right);
            }
            Expr::UnaryOp { expr, .. } => {
                self.walk_expr_for_column_refs(expr);
            }
            Expr::Like { expr, pattern, .. } => {
                self.walk_expr_for_column_refs(expr);
                self.walk_expr_for_column_refs(pattern);
            }
            Expr::Between {
                expr, low, high, ..
            } => {
                self.walk_expr_for_column_refs(expr);
                self.walk_expr_for_column_refs(low);
                self.walk_expr_for_column_refs(high);
            }
            Expr::InList { expr, list, .. } => {
                self.walk_expr_for_column_refs(expr);
                for item in list {
                    self.walk_expr_for_column_refs(item);
                }
            }
            Expr::IsNull { expr, .. } => {
                self.walk_expr_for_column_refs(expr);
            }
            Expr::FunctionCall { args, .. } => {
                for arg in args {
                    self.walk_expr_for_column_refs(arg);
                }
            }
            Expr::Case {
                operand,
                whens,
                else_expr,
            } => {
                if let Some(ref op) = operand {
                    self.walk_expr_for_column_refs(op);
                }
                for wc in whens {
                    self.walk_expr_for_column_refs(&wc.condition);
                    self.walk_expr_for_column_refs(&wc.result);
                }
                if let Some(ref e) = else_expr {
                    self.walk_expr_for_column_refs(e);
                }
            }
            Expr::Exists(subquery) | Expr::Subquery(subquery) => {
                // Don't recurse into subqueries to avoid alias pollution (P2 scope isolation)
                let _ = subquery;
            }
            Expr::InSubquery { expr, .. } => {
                self.walk_expr_for_column_refs(expr);
            }
            Expr::TypeCast { expr, .. } => {
                self.walk_expr_for_column_refs(expr);
            }
            Expr::Parenthesized(inner) => {
                self.walk_expr_for_column_refs(inner);
            }
            Expr::FieldAccess { object, .. } => {
                self.walk_expr_for_column_refs(object);
            }
            _ => {}
        }
    }
}

fn walk_select_target_exprs(extractor: &mut ColumnAccessExtractor, target: &SelectTarget) {
    match target {
        SelectTarget::Expr(expr, _) => {
            extractor.walk_expr_for_column_refs(expr);
        }
        SelectTarget::Star(_) => {}
    }
}

fn format_select_target(target: &SelectTarget) -> String {
    match target {
        SelectTarget::Expr(expr, alias) => {
            let base = format_expr_short(expr);
            match alias {
                Some(a) => format!("{} AS {}", base, a),
                None => base,
            }
        }
        SelectTarget::Star(alias) => match alias {
            Some(a) => format!("{}.*", a),
            None => "*".to_string(),
        },
    }
}

/// Strip nested `Parenthesized` wrappers, returning the innermost `Expr`.
///
/// `EXECUTE IMMEDIATE (v_sql)` parses as `Parenthesized(PlVariable(...))`;
/// without peeling, the Debug format prefix becomes `Parenthesized(` and
/// evades `noise_rule`'s `starts_with("PlVariable(")` check.
fn peel_parenthesized(mut expr: &Expr) -> &Expr {
    while let Expr::Parenthesized(inner) = expr {
        expr = inner;
    }
    expr
}

fn format_expr_short(expr: &Expr) -> String {
    match expr {
        Expr::ColumnRef(names) => names.join("."),
        Expr::ColumnRefOuterJoin(names) => format!("{}(+)", names.join(".")),
        Expr::PlVariable(names) => names.join("."),
        Expr::Literal(lit) => format_literal_short(lit),
        Expr::FunctionCall {
            name,
            args,
            distinct,
            ..
        } => {
            let fname = name.last().cloned().unwrap_or_default();
            let arg_strs: Vec<String> = args.iter().map(format_expr_short).collect();
            let prefix = if *distinct { "DISTINCT " } else { "" };
            format!("{}({}{})", fname, prefix, arg_strs.join(", "))
        }
        Expr::BinaryOp { left, op, right } => {
            format!(
                "{} {} {}",
                format_expr_short(left),
                op,
                format_expr_short(right)
            )
        }
        Expr::UnaryOp { op, expr } => format!("{}{}", op, format_expr_short(expr)),
        Expr::Parenthesized(inner) => format!("({})", format_expr_short(inner)),
        Expr::TypeCast { expr, .. } => format_expr_short(expr),
        Expr::Case {
            operand,
            whens,
            else_expr,
        } => {
            let mut out = String::from("CASE");
            if let Some(op) = operand {
                out.push(' ');
                out.push_str(&format_expr_short(op));
            }
            for wc in whens {
                out.push_str(&format!(
                    " WHEN {} THEN {}",
                    format_expr_short(&wc.condition),
                    format_expr_short(&wc.result)
                ));
            }
            if let Some(e) = else_expr {
                out.push_str(&format!(" ELSE {}", format_expr_short(e)));
            }
            out.push_str(" END");
            out
        }
        _ => format!("{:?}", expr),
    }
}

/// Names a select list produces, in order.
///
/// An explicit alias wins; a bare column reference names itself. Anything else — an
/// unaliased `decode(...)`, a literal — yields an empty placeholder so that later entries
/// keep their positions, and [`ColumnAccessExtractor::map_columns_positionally`] skips it.
fn output_column_names(select: &SelectStatement) -> Vec<String> {
    select
        .targets
        .iter()
        .map(|target| match target {
            SelectTarget::Expr(expr, Some(alias)) => {
                let _ = expr;
                alias.to_string()
            }
            SelectTarget::Expr(expr, None) => match peel_parenthesized(expr) {
                Expr::ColumnRef(names) | Expr::ColumnRefOuterJoin(names) => {
                    names.last().map(|n| n.to_string()).unwrap_or_default()
                }
                _ => String::new(),
            },
            SelectTarget::Star(_) => String::new(),
        })
        .collect()
}

/// Aggregate functions whose output summarises many rows of their input.
///
/// Kept to the set that appears in GaussDB/Oracle analytics code; an unlisted function
/// falls through to `Derived`, which is the safer default — it still records the same
/// source columns, only without labelling the aggregation.
fn is_aggregate_function(name: &str) -> bool {
    matches!(
        name.to_uppercase().as_str(),
        "SUM"
            | "COUNT"
            | "AVG"
            | "MIN"
            | "MAX"
            | "STDDEV"
            | "STDDEV_POP"
            | "STDDEV_SAMP"
            | "VARIANCE"
            | "VAR_POP"
            | "VAR_SAMP"
            | "MEDIAN"
            | "LISTAGG"
            | "STRING_AGG"
            | "ARRAY_AGG"
            | "GROUP_CONCAT"
            | "WM_CONCAT"
            | "BOOL_AND"
            | "BOOL_OR"
    )
}

fn push_unique_source(out: &mut Vec<ColumnSource>, source: ColumnSource) {
    if !out.contains(&source) {
        out.push(source);
    }
}

/// Collect distinct table names referenced anywhere in a FROM list, descending into
/// derived tables/subqueries (for dynamic-SQL cursor attribution).
fn collect_from_tables(from: &[AstTableRef], out: &mut HashSet<String>) {
    for tr in from {
        match tr {
            AstTableRef::Table { name, .. } => {
                if let Some(t) = name.last() {
                    out.insert(t.to_string().to_lowercase());
                }
            }
            AstTableRef::Subquery { query, .. } => collect_from_tables(&query.from, out),
            _ => {}
        }
    }
}

fn single_table_name_of(from: &[AstTableRef]) -> Option<String> {
    if from.len() == 1 {
        if let AstTableRef::Table { name, .. } = &from[0] {
            return name.last().map(|i| i.to_string());
        }
    }
    None
}

fn derive_cursor_output_name(expr: &Expr) -> String {
    match expr {
        Expr::ColumnRef(parts) | Expr::ColumnRefOuterJoin(parts) => {
            parts.last().map(|i| i.to_string()).unwrap_or_default()
        }
        Expr::FieldAccess { field, .. } => field.clone(),
        _ => String::new(),
    }
}

fn collect_expr_columns(expr: &Expr) -> Vec<(Option<String>, String)> {
    match expr {
        Expr::ColumnRef(parts) | Expr::ColumnRefOuterJoin(parts) => {
            let col = parts.last().map(|i| i.to_string()).unwrap_or_default();
            let table = if parts.len() >= 2 {
                Some(parts[parts.len() - 2].to_string())
            } else {
                None
            };
            vec![(table, col)]
        }
        Expr::FunctionCall { args, .. } => args.iter().flat_map(collect_expr_columns).collect(),
        Expr::BinaryOp { left, right, .. } => {
            let mut cols = collect_expr_columns(left);
            cols.extend(collect_expr_columns(right));
            cols
        }
        Expr::UnaryOp { expr: inner, .. } | Expr::Parenthesized(inner) => {
            collect_expr_columns(inner)
        }
        Expr::Case {
            whens, else_expr, ..
        } => {
            let mut cols: Vec<_> = whens
                .iter()
                .flat_map(|w| {
                    let mut c = collect_expr_columns(&w.condition);
                    c.extend(collect_expr_columns(&w.result));
                    c
                })
                .collect();
            if let Some(ref e) = else_expr {
                cols.extend(collect_expr_columns(e));
            }
            cols
        }
        _ => vec![],
    }
}

fn expr_var_name(expr: &Expr) -> String {
    match expr {
        Expr::ColumnRef(parts) | Expr::PlVariable(parts) => {
            parts.last().map(|i| i.to_string()).unwrap_or_default()
        }
        _ => String::new(),
    }
}

fn format_literal_short(lit: &Literal) -> String {
    match lit {
        Literal::String(s) => format!("'{}'", s),
        Literal::Integer(i) => i.to_string(),
        Literal::Float(f) => f.clone(),
        Literal::Boolean(b) => b.to_string(),
        Literal::Null => "NULL".to_string(),
        _ => format!("{:?}", lit),
    }
}

fn split_alias_column(names: &[ogsql_parser::Ident]) -> (Option<String>, String) {
    if names.len() >= 2 {
        (
            Some(names[0].to_string()),
            names[names.len() - 1].to_string(),
        )
    } else {
        (None, names[0].to_string())
    }
}

fn split_schema_table(name: &ObjectName) -> (Option<String>, String) {
    if name.len() == 1 {
        (None, name[0].to_string())
    } else {
        (
            Some(name[..name.len() - 1].join(".")),
            name[name.len() - 1].to_string(),
        )
    }
}

/// Check if an expression is a ColumnRef and return the names.
fn as_column_ref(expr: &Expr) -> Option<Vec<ogsql_parser::Ident>> {
    match expr {
        Expr::ColumnRef(names) => Some(names.clone()),
        _ => None,
    }
}

/// Convert a Literal expression to FilterValue. Returns None for non-literal (PL variables, etc).
fn literal_to_filter_value(expr: &Expr) -> Option<FilterValue> {
    match expr {
        Expr::Literal(lit) => Some(literal_to_fv(lit)),
        Expr::TypeCast { expr, .. } => literal_to_filter_value(expr),
        Expr::UnaryOp { op, expr } if op == "-" => literal_to_filter_value(expr).map(|v| match v {
            FilterValue::Integer(i) => FilterValue::Integer(-i),
            FilterValue::Float(f) => FilterValue::Float(format!("-{}", f)),
            other => other,
        }),
        _ => None,
    }
}

fn literal_to_fv(lit: &Literal) -> FilterValue {
    match lit {
        Literal::String(s) => FilterValue::String(s.clone()),
        Literal::Integer(i) => FilterValue::Integer(*i),
        Literal::Float(f) => FilterValue::Float(f.clone()),
        Literal::Boolean(b) => FilterValue::Boolean(*b),
        Literal::Null => FilterValue::Null,
        Literal::EscapeString(s) => FilterValue::String(s.clone()),
        _ => FilterValue::Expression(format!("{:?}", lit)),
    }
}

fn literal_to_string(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Literal(Literal::String(s)) => Some(s.clone()),
        Expr::Literal(Literal::Integer(i)) => Some(i.to_string()),
        Expr::Literal(Literal::Float(f)) => Some(f.clone()),
        Expr::Literal(Literal::Boolean(b)) => Some(b.to_string()),
        _ => None,
    }
}

fn dedup_column_refs(refs: Vec<ColumnRef>) -> Vec<ColumnRef> {
    let mut seen: BTreeMap<(Option<String>, String), Vec<ColumnContext>> = BTreeMap::new();
    let mut result: Vec<ColumnRef> = Vec::new();
    for cr in refs {
        let key = (cr.resolved_table.clone(), cr.column.clone());
        if let Some(existing_contexts) = seen.get_mut(&key) {
            for ctx in &cr.contexts {
                if !existing_contexts.contains(ctx) {
                    existing_contexts.push(*ctx);
                }
            }
        } else {
            seen.insert(key.clone(), cr.contexts.clone());
            result.push(cr);
        }
    }
    // Update contexts in result
    for cr in &mut result {
        let key = (cr.resolved_table.clone(), cr.column.clone());
        if let Some(contexts) = seen.get(&key) {
            cr.contexts = contexts.clone();
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use ogsql_parser::{walk_statement, Tokenizer};
    use std::path::PathBuf;

    fn extract_accesses(sql: &str) -> Vec<TableAccessInfo> {
        let tokens = Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let stmts = parser.parse_with_text();
        let mut all_accesses = Vec::new();
        for info in &stmts {
            let mut extractor = TableAccessExtractor::new();
            walk_statement(&mut extractor, &info.statement);
            all_accesses.extend(extractor.accesses);
        }
        all_accesses
    }

    fn find_access<'a>(accesses: &'a [TableAccessInfo], name: &str) -> Option<&'a TableAccessInfo> {
        accesses.iter().find(|a| a.name == name)
    }

    #[test]
    fn select_from_reads() {
        let sql = "SELECT * FROM t1 JOIN t2 ON t1.id = t2.id";
        let accesses = extract_accesses(sql);
        assert_eq!(
            accesses.len(),
            2,
            "expected 2 accesses, got: {:?}",
            accesses
        );
        let t1 = find_access(&accesses, "t1").expect("t1 not found");
        let t2 = find_access(&accesses, "t2").expect("t2 not found");
        assert!(t1.modes.contains(AccessMode::Read), "t1 should be Read");
        assert!(t2.modes.contains(AccessMode::Read), "t2 should be Read");
    }

    #[test]
    fn insert_values_writes() {
        let sql = "INSERT INTO t1 VALUES(1,2)";
        let accesses = extract_accesses(sql);
        assert_eq!(accesses.len(), 1, "expected 1 access, got: {:?}", accesses);
        let t1 = find_access(&accesses, "t1").expect("t1 not found");
        assert!(t1.modes.contains(AccessMode::Write), "t1 should be Write");
        assert!(
            t1.write_kinds.contains(&WriteKind::Insert),
            "t1 should be Insert"
        );
    }

    #[test]
    fn insert_select_read_write() {
        let sql = "INSERT INTO t_tgt SELECT * FROM t_src";
        let accesses = extract_accesses(sql);
        assert_eq!(
            accesses.len(),
            2,
            "expected 2 accesses, got: {:?}",
            accesses
        );
        let tgt = find_access(&accesses, "t_tgt").expect("t_tgt not found");
        let src = find_access(&accesses, "t_src").expect("t_src not found");
        assert!(tgt.modes.contains(AccessMode::Write), "tgt should be Write");
        assert!(
            tgt.write_kinds.contains(&WriteKind::InsertSelect),
            "tgt should be InsertSelect"
        );
        assert!(src.modes.contains(AccessMode::Read), "src should be Read");
    }

    #[test]
    fn update_writes() {
        let sql = "UPDATE t1 SET x=1";
        let accesses = extract_accesses(sql);
        assert_eq!(accesses.len(), 1, "expected 1 access, got: {:?}", accesses);
        let t1 = find_access(&accesses, "t1").expect("t1 not found");
        assert!(t1.modes.contains(AccessMode::Write), "t1 should be Write");
        assert!(
            t1.write_kinds.contains(&WriteKind::Update),
            "t1 should be Update"
        );
    }

    #[test]
    fn update_from_reads_writes() {
        let sql = "UPDATE t1 SET x=1 FROM t2 WHERE t1.id = t2.id";
        let accesses = extract_accesses(sql);
        assert_eq!(
            accesses.len(),
            2,
            "expected 2 accesses, got: {:?}",
            accesses
        );
        let t1 = find_access(&accesses, "t1").expect("t1 not found");
        let t2 = find_access(&accesses, "t2").expect("t2 not found");
        assert!(t1.modes.contains(AccessMode::Write), "t1 should be Write");
        assert!(
            t1.write_kinds.contains(&WriteKind::Update),
            "t1 should be Update"
        );
        assert!(t2.modes.contains(AccessMode::Read), "t2 should be Read");
    }

    #[test]
    fn delete_writes() {
        let sql = "DELETE FROM t1";
        let accesses = extract_accesses(sql);
        assert_eq!(accesses.len(), 1, "expected 1 access, got: {:?}", accesses);
        let t1 = find_access(&accesses, "t1").expect("t1 not found");
        assert!(t1.modes.contains(AccessMode::Write), "t1 should be Write");
        assert!(
            t1.write_kinds.contains(&WriteKind::Delete),
            "t1 should be Delete"
        );
    }

    #[test]
    fn delete_using_reads_writes() {
        let sql = "DELETE FROM t1 WHERE id IN (SELECT id FROM t2)";
        let accesses = extract_accesses(sql);
        assert_eq!(
            accesses.len(),
            2,
            "expected 2 accesses, got: {:?}",
            accesses
        );
        let t1 = find_access(&accesses, "t1").expect("t1 not found");
        let t2 = find_access(&accesses, "t2").expect("t2 not found");
        assert!(t1.modes.contains(AccessMode::Write), "t1 should be Write");
        assert!(
            t1.write_kinds.contains(&WriteKind::Delete),
            "t1 should be Delete"
        );
        assert!(t2.modes.contains(AccessMode::Read), "t2 should be Read");
    }

    #[test]
    fn merge_all_writes() {
        let sql = "MERGE INTO t_target USING t_source ON t_target.id = t_source.id WHEN MATCHED THEN UPDATE SET name = t_source.name WHEN NOT MATCHED THEN INSERT (id, name) VALUES (t_source.id, t_source.name) WHEN NOT MATCHED THEN DELETE";
        let accesses = extract_accesses(sql);
        assert_eq!(
            accesses.len(),
            2,
            "expected 2 accesses, got: {:?}",
            accesses
        );
        let target = find_access(&accesses, "t_target").expect("t_target not found");
        let source = find_access(&accesses, "t_source").expect("t_source not found");
        assert!(
            target.modes.contains(AccessMode::Write),
            "target should be Write"
        );
        assert!(
            target.write_kinds.contains(&WriteKind::MergeUpdate),
            "target should have MergeUpdate"
        );
        assert!(
            target.write_kinds.contains(&WriteKind::MergeInsert),
            "target should have MergeInsert"
        );
        assert!(
            target.write_kinds.contains(&WriteKind::MergeDelete),
            "target should have MergeDelete"
        );
        assert!(
            source.modes.contains(AccessMode::Read),
            "source should be Read"
        );
    }

    #[test]
    fn truncate_table() {
        let sql = "TRUNCATE TABLE t1";
        let accesses = extract_accesses(sql);
        assert_eq!(accesses.len(), 1, "expected 1 access, got: {:?}", accesses);
        let t1 = find_access(&accesses, "t1").expect("t1 not found");
        assert!(
            t1.modes.contains(AccessMode::Truncate),
            "t1 should be Truncate"
        );
        assert!(
            t1.write_kinds.contains(&WriteKind::Truncate),
            "t1 should have Truncate write_kind"
        );
    }

    #[test]
    fn select_for_update_locks() {
        let sql = "SELECT * FROM t1 FOR UPDATE";
        let accesses = extract_accesses(sql);
        assert_eq!(accesses.len(), 1, "expected 1 access, got: {:?}", accesses);
        let t1 = find_access(&accesses, "t1").expect("t1 not found");
        assert!(t1.modes.contains(AccessMode::Read), "t1 should be Read");
        assert!(
            t1.modes.contains(AccessMode::LockRead),
            "t1 should be LockRead"
        );
    }

    #[test]
    fn same_table_read_write_merge() {
        let sql = "CREATE PROCEDURE p() AS $$ BEGIN UPDATE t SET x=(SELECT y FROM t) WHERE id=1; END; $$;";
        let accesses = extract_accesses(sql);
        assert_eq!(accesses.len(), 1, "expected 1 access, got: {:?}", accesses);
        let t = find_access(&accesses, "t").expect("t not found");
        assert!(t.modes.contains(AccessMode::Read), "t should be Read");
        assert!(t.modes.contains(AccessMode::Write), "t should be Write");
        assert!(
            t.write_kinds.contains(&WriteKind::Update),
            "t should be Update"
        );
    }

    #[test]
    fn insert_all_multi_target() {
        let sql = "INSERT ALL INTO t1 (a) VALUES (x) INTO t2 (b) VALUES (y) SELECT x, y FROM src";
        let accesses = extract_accesses(sql);
        assert_eq!(
            accesses.len(),
            3,
            "expected 3 accesses, got: {:?}",
            accesses
        );
        let t1 = find_access(&accesses, "t1").expect("t1 not found");
        let t2 = find_access(&accesses, "t2").expect("t2 not found");
        let src = find_access(&accesses, "src").expect("src not found");
        assert!(t1.modes.contains(AccessMode::Write), "t1 should be Write");
        assert!(
            t1.write_kinds.contains(&WriteKind::Insert),
            "t1 should be Insert"
        );
        assert!(t2.modes.contains(AccessMode::Write), "t2 should be Write");
        assert!(
            t2.write_kinds.contains(&WriteKind::Insert),
            "t2 should be Insert"
        );
        assert!(src.modes.contains(AccessMode::Read), "src should be Read");
    }

    #[test]
    fn select_into_table() {
        let sql = "SELECT * INTO t_new FROM t_src";
        let accesses = extract_accesses(sql);
        assert_eq!(
            accesses.len(),
            2,
            "expected 2 accesses, got: {:?}",
            accesses
        );
        let t_new = find_access(&accesses, "t_new").expect("t_new not found");
        let t_src = find_access(&accesses, "t_src").expect("t_src not found");
        assert!(
            t_new.modes.contains(AccessMode::Write),
            "t_new should be Write"
        );
        assert!(
            t_new.write_kinds.contains(&WriteKind::SelectInto),
            "t_new should be SelectInto"
        );
        assert!(
            t_src.modes.contains(AccessMode::Read),
            "t_src should be Read"
        );
    }

    fn extract_edges(sql: &str) -> Vec<CallEdge> {
        let tokens = Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let stmts = parser.parse_with_text();
        let mut all_edges = Vec::new();
        for info in &stmts {
            let mut extractor =
                CallExtractor::new(Arc::new(PathBuf::from("test.sql")), HashSet::new());
            walk_statement(&mut extractor, &info.statement);
            all_edges.extend(extractor.edges);
        }
        all_edges
    }

    fn extract_type_seq_refs(
        sql: &str,
        known_types: HashSet<String>,
    ) -> (Vec<TypeRef>, Vec<SequenceRef>) {
        let tokens = Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let stmts = parser.parse_with_text();
        let mut extractor = TypeSequenceRefExtractor::new(known_types);
        for info in &stmts {
            walk_statement(&mut extractor, &info.statement);
        }
        (extractor.type_refs, extractor.sequence_refs)
    }

    #[test]
    fn standalone_procedure_call() {
        let sql = "CREATE PROCEDURE a() AS $$ BEGIN b(); END; $$;";
        let edges = extract_edges(sql);
        assert_eq!(edges.len(), 1);
        assert_eq!(
            edges[0].caller,
            Some(RoutineId {
                schema: None,
                package: None,
                name: "a".to_string(),
                kind: RoutineKind::Procedure,
            })
        );
        assert_eq!(edges[0].callee_name, "b");
        assert!(!edges[0].is_dynamic);
    }

    #[test]
    fn schema_qualified_call() {
        let sql = "CREATE PROCEDURE a() AS $$ BEGIN pkg.b(); END; $$;";
        let edges = extract_edges(sql);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].callee_name, "pkg.b");
    }

    #[test]
    fn dynamic_sql_is_marked() {
        let sql =
            "CREATE PROCEDURE a() AS $$ BEGIN EXECUTE IMMEDIATE 'CALL ' || v_proc || '()'; END; $$;";
        let edges = extract_edges(sql);
        assert!(
            edges.iter().any(|e| e.is_dynamic),
            "expected at least one dynamic edge, got: {:?}",
            edges
        );
    }

    #[test]
    fn multiple_calls_in_one_procedure() {
        let sql = "CREATE PROCEDURE a() AS $$ BEGIN b(); c(1); d(1,2); END; $$;";
        let edges = extract_edges(sql);
        assert_eq!(edges.len(), 3);
        let names: Vec<&str> = edges.iter().map(|e| e.callee_name.as_str()).collect();
        assert_eq!(names, vec!["b", "c", "d"]);
    }

    #[test]
    fn top_level_call_statement() {
        let sql = "CALL my_proc(1, 2);";
        let edges = extract_edges(sql);
        assert_eq!(edges.len(), 1);
        assert!(edges[0].caller.is_none());
        assert_eq!(edges[0].callee_name, "my_proc");
    }

    #[test]
    fn function_in_select_from() {
        let sql = "CREATE FUNCTION a() RETURNS void AS $$ BEGIN SELECT * FROM generate_series(1,10); END; $$ LANGUAGE plpgsql;";
        let edges = extract_edges(sql);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].callee_name, "generate_series");
        assert_eq!(
            edges[0].caller,
            Some(RoutineId {
                schema: None,
                package: None,
                name: "a".to_string(),
                kind: RoutineKind::Function,
            })
        );
    }

    #[test]
    fn package_body_procedure_calls_have_caller_context() {
        let sql = r#"
            CREATE OR REPLACE PACKAGE BODY pkg_api AS
                PROCEDURE do_work(p_id INT) IS
                BEGIN
                    helper.validate(p_id);
                    helper.process(p_id);
                END;
            END pkg_api;
        "#;
        let tokens = ogsql_parser::Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let stmts = parser.parse_with_text();

        let mut extractor = CallExtractor::new(Arc::new(PathBuf::from("test.sql")), HashSet::new());

        for info in &stmts {
            if let ogsql_parser::ast::Statement::CreatePackageBody(pkg) = &info.statement {
                for item in &pkg.items {
                    if let ogsql_parser::ast::PackageItem::Procedure(p) = item {
                        if let Some(ref block) = p.block {
                            extractor.current_procedure = Some(RoutineId {
                                schema: None,
                                package: None,
                                name: "pkg_api.do_work".to_string(),
                                kind: RoutineKind::Procedure,
                            });
                            ogsql_parser::walk_pl_block(&mut extractor, block);
                        }
                    }
                }
            }
        }

        assert_eq!(
            extractor.edges.len(),
            2,
            "Expected 2 call edges from do_work"
        );
        for edge in &extractor.edges {
            let caller = edge
                .caller
                .as_ref()
                .expect("caller should be set for package routine");
            assert_eq!(caller.name, "pkg_api.do_work");
        }
        assert_eq!(extractor.edges[0].callee_name, "helper.validate");
        assert_eq!(extractor.edges[1].callee_name, "helper.process");
    }

    #[test]
    fn type_ref_from_pl_variable_declaration() {
        let sql = r#"
            CREATE PROCEDURE test_proc() AS $$
            DECLARE
                v_foo my_custom_type;
            BEGIN
                NULL;
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let mut known = HashSet::new();
        known.insert("my_custom_type".to_string());
        let (type_refs, seq_refs) = extract_type_seq_refs(sql, known);
        assert_eq!(seq_refs.len(), 0, "expected no sequence refs");
        assert_eq!(
            type_refs.len(),
            1,
            "expected 1 type ref, got: {:?}",
            type_refs
        );
        assert_eq!(type_refs[0].type_name, "my_custom_type");
    }

    #[test]
    fn sequence_ref_via_nextval_function_call() {
        let sql = r#"
            CREATE PROCEDURE test_proc() AS $$
            BEGIN
                SELECT nextval('my_seq') INTO v_id;
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let (type_refs, seq_refs) = extract_type_seq_refs(sql, HashSet::new());
        assert_eq!(type_refs.len(), 0, "expected no type refs");
        assert_eq!(
            seq_refs.len(),
            1,
            "expected 1 sequence ref, got: {:?}",
            seq_refs
        );
        assert_eq!(seq_refs[0].sequence_name, "my_seq");
        assert!(matches!(seq_refs[0].via, SequenceRefVia::Nextval));
    }

    #[test]
    fn sequence_ref_via_dot_nextval_field_access() {
        let sql = r#"
            CREATE PROCEDURE test_proc() AS $$
            BEGIN
                v_id := my_seq.NEXTVAL;
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let (type_refs, seq_refs) = extract_type_seq_refs(sql, HashSet::new());
        assert_eq!(type_refs.len(), 0, "expected no type refs");
        assert_eq!(
            seq_refs.len(),
            1,
            "expected 1 sequence ref, got: {:?}",
            seq_refs
        );
        assert_eq!(seq_refs[0].sequence_name, "my_seq");
        assert!(matches!(seq_refs[0].via, SequenceRefVia::DotNextval));
    }

    #[test]
    fn sequence_ref_via_currval_function_call() {
        let sql = r#"
            CREATE PROCEDURE test_proc() AS $$
            BEGIN
                SELECT currval('my_seq') INTO v_id;
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let (type_refs, seq_refs) = extract_type_seq_refs(sql, HashSet::new());
        assert_eq!(type_refs.len(), 0, "expected no type refs");
        assert_eq!(
            seq_refs.len(),
            1,
            "expected 1 sequence ref, got: {:?}",
            seq_refs
        );
        assert_eq!(seq_refs[0].sequence_name, "my_seq");
        assert!(matches!(seq_refs[0].via, SequenceRefVia::Currval));
    }

    #[test]
    fn unknown_type_is_ignored() {
        let sql = r#"
            CREATE PROCEDURE test_proc() AS $$
            DECLARE
                v_foo unknown_type;
            BEGIN
                NULL;
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let mut known = HashSet::new();
        known.insert("my_custom_type".to_string());
        let (type_refs, _) = extract_type_seq_refs(sql, known);
        assert_eq!(type_refs.len(), 0, "expected 0 type refs for unknown type");
    }

    #[test]
    fn type_ref_from_typecast_custom_datatype() {
        let sql = r#"
            CREATE PROCEDURE test_proc() AS $$
            BEGIN
                v_foo := 'value'::my_custom_type;
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let mut known = HashSet::new();
        known.insert("my_custom_type".to_string());
        let (type_refs, _) = extract_type_seq_refs(sql, known);
        assert_eq!(
            type_refs.len(),
            1,
            "expected 1 type ref from typecast, got: {:?}",
            type_refs
        );
        assert_eq!(type_refs[0].type_name, "my_custom_type");
    }
}

#[cfg(test)]
mod column_tests {
    use super::*;
    use ogsql_parser::{walk_statement, Tokenizer};

    fn extract_column_analysis(sql: &str) -> Vec<ColumnAnalysis> {
        let tokens = Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let stmts = parser.parse_with_text();
        let mut results = Vec::new();
        for info in &stmts {
            let mut extractor = ColumnAccessExtractor::new();
            walk_statement(&mut extractor, &info.statement);
            results.push(extractor.finish());
        }
        results
    }

    fn find_column_ref<'a>(refs: &'a [ColumnRef], col: &str) -> Option<&'a ColumnRef> {
        refs.iter().find(|r| r.column == col)
    }

    fn find_hard_filter<'a>(filters: &'a [HardFilter], col: &str) -> Option<&'a HardFilter> {
        filters.iter().find(|f| f.column == col)
    }

    #[test]
    fn test_simple_select_no_where() {
        let sql = "SELECT id, name FROM users";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let a = &analyses[0];
        assert!(a.hard_filters.is_empty());
        assert!(a.join_conditions.is_empty());

        let id = find_column_ref(&a.column_refs, "id").expect("id column");
        assert_eq!(id.alias_prefix, None);
        let name = find_column_ref(&a.column_refs, "name").expect("name column");
        assert_eq!(name.alias_prefix, None);
    }

    #[test]
    fn test_join_with_alias_and_hard_filter() {
        let sql = "SELECT a.id, a.name, b.amount FROM table_a a LEFT JOIN table_b b ON a.id = b.a_id WHERE a.status = 'active'";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let a = &analyses[0];

        assert_eq!(a.alias_map.len(), 2);
        assert_eq!(a.alias_map.get("a").unwrap().table, "table_a");
        assert_eq!(a.alias_map.get("b").unwrap().table, "table_b");

        assert_eq!(a.join_conditions.len(), 1);
        let jc = &a.join_conditions[0];
        assert_eq!(jc.left_table, "table_a");
        assert_eq!(jc.left_column, "id");
        assert_eq!(jc.right_table, "table_b");
        assert_eq!(jc.right_column, "a_id");
        assert_eq!(jc.join_type, JoinType::Left);
        assert_eq!(jc.source, JoinConditionSource::ExplicitOn);

        let hf = find_hard_filter(&a.hard_filters, "status").expect("status filter");
        assert_eq!(hf.table, Some("table_a".to_string()));
        assert_eq!(hf.operator, FilterOperator::Eq);
        assert_eq!(&hf.value, &FilterValue::String("active".to_string()));
    }

    #[test]
    fn test_insert_columns() {
        let sql = "INSERT INTO t_log(product_id, delta, reason) VALUES (1, -5, 'RESERVE')";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let a = &analyses[0];
        assert_eq!(a.insert_columns.len(), 1);
        assert_eq!(a.insert_columns[0].table, "t_log");
        assert_eq!(
            a.insert_columns[0].columns,
            vec!["product_id", "delta", "reason"]
        );
    }

    #[test]
    fn test_update_set_columns() {
        let sql = "UPDATE t_products SET stock_qty = stock_qty - 5 WHERE id = 1";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let a = &analyses[0];
        assert_eq!(a.update_columns.len(), 1);
        assert_eq!(a.update_columns[0].table, "t_products");
        assert_eq!(a.update_columns[0].set_columns, vec!["stock_qty"]);
    }

    #[test]
    fn test_implicit_join_from_where() {
        let sql = "SELECT t.client_acnt_id, t.accno FROM v_par_client_acnt_info_noflag t, v_acnt_check_base_rule e WHERE e.client_acnt_id = t.client_acnt_id AND t.if_inter_bank = '2'";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let a = &analyses[0];

        assert_eq!(a.alias_map.len(), 2);
        assert_eq!(
            a.alias_map.get("t").unwrap().table,
            "v_par_client_acnt_info_noflag"
        );
        assert_eq!(
            a.alias_map.get("e").unwrap().table,
            "v_acnt_check_base_rule"
        );

        assert_eq!(a.join_conditions.len(), 1);
        let jc = &a.join_conditions[0];
        assert_eq!(jc.left_table, "v_acnt_check_base_rule");
        assert_eq!(jc.left_column, "client_acnt_id");
        assert_eq!(jc.right_table, "v_par_client_acnt_info_noflag");
        assert_eq!(jc.right_column, "client_acnt_id");
        assert_eq!(jc.join_type, JoinType::Inner);
        assert_eq!(jc.source, JoinConditionSource::ImplicitWhere);

        let hf = find_hard_filter(&a.hard_filters, "if_inter_bank").expect("filter");
        assert_eq!(hf.value, FilterValue::String("2".to_string()));
    }

    #[test]
    fn test_pl_variable_not_hard_filter() {
        let sql = "SELECT stock_qty INTO v_available FROM t_products WHERE id = p_product_id";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let a = &analyses[0];
        assert!(
            a.hard_filters.is_empty(),
            "col = PL_variable should NOT be hard_filter, got: {:?}",
            a.hard_filters
        );
    }

    #[test]
    fn test_same_table_different_aliases() {
        let sql = "SELECT um1.message_value, um2.message_value FROM usermessage um1, usermessage um2 WHERE um1.user_id = um2.user_id AND um1.message_id = '001'";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let a = &analyses[0];

        assert_eq!(a.alias_map.len(), 2);
        assert_eq!(a.alias_map.get("um1").unwrap().table, "usermessage");
        assert_eq!(a.alias_map.get("um2").unwrap().table, "usermessage");

        let hf = find_hard_filter(&a.hard_filters, "message_id").expect("filter");
        assert_eq!(hf.value, FilterValue::String("001".to_string()));
    }

    #[test]
    fn test_case_enum_extraction() {
        let sql = "SELECT CASE sys_flag WHEN '1' THEN '系统内' WHEN '2' THEN '系统外' ELSE '' END AS flag FROM temp";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let a = &analyses[0];

        assert_eq!(a.enum_mappings.len(), 1);
        let em = &a.enum_mappings[0];
        assert_eq!(em.column, "sys_flag");
        assert!(em.has_else);
        assert_eq!(em.values.len(), 2);
        assert_eq!(em.values[0].0, FilterValue::String("1".to_string()));
        assert_eq!(em.values[0].1, "系统内");
        assert_eq!(em.values[1].0, FilterValue::String("2".to_string()));
        assert_eq!(em.values[1].1, "系统外");
    }

    #[test]
    fn test_decode_enum_extraction() {
        let sql = "SELECT decode(cnt_flag, '1', '正常', '2', '冻结') AS status FROM temp";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let a = &analyses[0];

        assert_eq!(a.enum_mappings.len(), 1);
        let em = &a.enum_mappings[0];
        assert_eq!(em.column, "cnt_flag");
        assert!(!em.has_else);
        assert_eq!(em.values.len(), 2);
        assert_eq!(em.values[0].0, FilterValue::String("1".to_string()));
        assert_eq!(em.values[0].1, "正常");
        assert_eq!(em.values[1].0, FilterValue::String("2".to_string()));
        assert_eq!(em.values[1].1, "冻结");
    }

    #[test]
    fn test_is_null_hard_filter() {
        let sql = "SELECT id FROM orders WHERE deleted_at IS NULL";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let hf = find_hard_filter(&analyses[0].hard_filters, "deleted_at").expect("filter");
        assert_eq!(hf.operator, FilterOperator::IsNull);
        assert_eq!(hf.value, FilterValue::Null);
    }

    #[test]
    fn test_in_list_hard_filter() {
        let sql = "SELECT id FROM orders WHERE status IN ('active', 'pending', 'shipped')";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let hf = find_hard_filter(&analyses[0].hard_filters, "status").expect("filter");
        assert_eq!(hf.operator, FilterOperator::In);
        if let FilterValue::List(vals) = &hf.value {
            assert_eq!(vals.len(), 3);
        } else {
            panic!("expected List, got {:?}", hf.value);
        }
    }

    #[test]
    fn test_between_hard_filter() {
        let sql = "SELECT id FROM orders WHERE amount BETWEEN 100 AND 500";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let hf = find_hard_filter(&analyses[0].hard_filters, "amount").expect("filter");
        assert_eq!(hf.operator, FilterOperator::Between);
    }

    #[test]
    fn test_insert_select_columns() {
        let sql = "INSERT INTO t_reconciliation(date, total_amount, total_count) SELECT p_date, SUM(amount), COUNT(*) FROM t_payments WHERE status = 'PAID'";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let a = &analyses[0];
        assert_eq!(a.insert_columns.len(), 1);
        assert_eq!(a.insert_columns[0].table, "t_reconciliation");
        assert_eq!(
            a.insert_columns[0].columns,
            vec!["date", "total_amount", "total_count"]
        );

        let hf = find_hard_filter(&a.hard_filters, "status").expect("filter");
        assert_eq!(hf.value, FilterValue::String("PAID".to_string()));
    }

    #[test]
    fn test_comparison_operators() {
        let sql = "SELECT id FROM t WHERE level > 3 AND score >= 80 AND age < 65 AND weight <= 100";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let a = &analyses[0];
        assert_eq!(a.hard_filters.len(), 4);

        let level = find_hard_filter(&a.hard_filters, "level").unwrap();
        assert_eq!(level.operator, FilterOperator::Gt);
        assert_eq!(level.value, FilterValue::Integer(3));

        let score = find_hard_filter(&a.hard_filters, "score").unwrap();
        assert_eq!(score.operator, FilterOperator::Gte);
        assert_eq!(score.value, FilterValue::Integer(80));

        let age = find_hard_filter(&a.hard_filters, "age").unwrap();
        assert_eq!(age.operator, FilterOperator::Lt);

        let weight = find_hard_filter(&a.hard_filters, "weight").unwrap();
        assert_eq!(weight.operator, FilterOperator::Lte);
    }

    #[test]
    fn test_no_filter_on_col_eq_col() {
        let sql = "SELECT a.id FROM t1 a, t2 b WHERE a.id = b.id";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let a = &analyses[0];
        assert!(
            a.hard_filters.is_empty(),
            "col = col should be join, not filter"
        );
        assert_eq!(a.join_conditions.len(), 1);
    }

    #[test]
    fn test_dedup_column_refs() {
        let sql = "SELECT a.id, a.name FROM t a WHERE a.id > 0 ORDER BY a.id";
        let analyses = extract_column_analysis(sql);
        assert_eq!(analyses.len(), 1);
        let a = &analyses[0];
        let id_refs: Vec<&ColumnRef> = a.column_refs.iter().filter(|r| r.column == "id").collect();
        assert_eq!(
            id_refs.len(),
            1,
            "id should be deduplicated, got: {:?}",
            id_refs
        );
        assert!(
            id_refs[0].contexts.contains(&ColumnContext::SelectTarget),
            "should have SelectTarget context"
        );
        assert!(
            id_refs[0].contexts.contains(&ColumnContext::WhereClause),
            "should have WhereClause context"
        );
        assert!(
            id_refs[0].contexts.contains(&ColumnContext::OrderBy),
            "should have OrderBy context"
        );
    }

    // ── Column mappings (#136) ────────────────────────────────────────────────

    fn column_mappings_of(sql: &str) -> Vec<ColumnMapping> {
        extract_column_analysis(sql)
            .into_iter()
            .flat_map(|a| a.column_mappings)
            .collect()
    }

    /// Column mappings of a view body, via the explicit `CREATE VIEW` entry point.
    fn view_column_mappings(view: &str, declared: &[&str], select_sql: &str) -> Vec<ColumnMapping> {
        let tokens = Tokenizer::new(select_sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, select_sql.to_string());
        let stmts = parser.parse_with_text();
        let declared: Vec<String> = declared.iter().map(|s| s.to_string()).collect();

        let mut extractor = ColumnAccessExtractor::new();
        for info in &stmts {
            if let Statement::Select(select) = &info.statement {
                extractor.extract_view_column_mappings(view, &declared, select);
            }
        }
        extractor.finish().column_mappings
    }

    fn find_mapping<'a>(maps: &'a [ColumnMapping], column: &str) -> &'a ColumnMapping {
        maps.iter()
            .find(|m| m.target_column.eq_ignore_ascii_case(column))
            .unwrap_or_else(|| panic!("no mapping for {column} in {maps:#?}"))
    }

    fn sources_for<'a>(maps: &'a [ColumnMapping], column: &str) -> Vec<&'a ColumnSource> {
        maps.iter()
            .filter(|m| m.target_column.eq_ignore_ascii_case(column))
            .flat_map(|m| m.sources.iter())
            .collect()
    }

    fn col(table: Option<&str>, column: &str) -> ColumnSource {
        ColumnSource::Column {
            table: table.map(|s| s.to_string()),
            column: column.to_string(),
        }
    }

    #[test]
    fn insert_select_maps_columns_by_position() {
        let maps = column_mappings_of(
            "INSERT INTO dst (fund_code, cjje) SELECT fund_code, qsje FROM jsmx_temp",
        );

        let cjje = find_mapping(&maps, "cjje");
        assert_eq!(cjje.target_table.as_deref(), Some("dst"));
        assert_eq!(cjje.kind, MappingKind::Direct);
        assert_eq!(cjje.sources, vec![col(Some("jsmx_temp"), "qsje")]);
        assert_eq!(
            cjje.expression, None,
            "a plain copy needs no expression text"
        );
    }

    #[test]
    fn insert_select_resolves_source_table_through_alias() {
        let maps = column_mappings_of(
            "INSERT INTO dst (v) SELECT t.qsje FROM jsmx_temp t WHERE t.bcrq = '20260101'",
        );
        assert_eq!(
            find_mapping(&maps, "v").sources,
            vec![col(Some("jsmx_temp"), "qsje")]
        );
    }

    #[test]
    fn aggregate_is_labelled_with_its_function() {
        let maps = column_mappings_of(
            "INSERT INTO dat_fund_cjqs (fund_code, total) \
             SELECT fund_code, SUM(cjje) FROM mid_yjqs_detail GROUP BY fund_code",
        );

        let total = find_mapping(&maps, "total");
        assert_eq!(
            total.kind,
            MappingKind::Aggregated {
                function: "SUM".to_string(),
                distinct: false
            }
        );
        assert_eq!(total.sources, vec![col(Some("mid_yjqs_detail"), "cjje")]);
    }

    #[test]
    fn count_distinct_records_the_distinct_flag() {
        let maps = column_mappings_of("INSERT INTO d (n) SELECT COUNT(DISTINCT gddm) FROM t");
        assert_eq!(
            find_mapping(&maps, "n").kind,
            MappingKind::Aggregated {
                function: "COUNT".to_string(),
                distinct: true
            }
        );
    }

    #[test]
    fn arithmetic_over_two_columns_is_derived_from_both() {
        let maps = column_mappings_of(
            "INSERT INTO dat_fund_cjqs (accural_tax) SELECT abs(accrual - accrual2) FROM mid_yjqs_detail",
        );

        let m = find_mapping(&maps, "accural_tax");
        assert_eq!(m.kind, MappingKind::Derived);
        assert_eq!(
            m.sources,
            vec![
                col(Some("mid_yjqs_detail"), "accrual"),
                col(Some("mid_yjqs_detail"), "accrual2"),
            ]
        );
        assert!(
            m.expression.as_deref().unwrap_or("").contains("abs"),
            "expression text should survive: {:?}",
            m.expression
        );
    }

    #[test]
    fn decode_records_its_input_column_but_not_its_branch_literals() {
        let maps = view_column_mappings(
            "v_bond",
            &[],
            "SELECT decode(bond_accrual_type, '1', '3', '2', '13', bond_accrual_type) AS accrual_mapped FROM par_sys_bond",
        );

        let m = find_mapping(&maps, "accrual_mapped");
        assert_eq!(m.kind, MappingKind::Derived);
        assert_eq!(
            m.sources,
            vec![col(Some("par_sys_bond"), "bond_accrual_type")],
            "branch literals are outputs, not upstream data"
        );
    }

    #[test]
    fn a_constant_column_records_the_literal() {
        let maps = view_column_mappings("v_x", &[], "SELECT '1500' AS bond_kind FROM par_sys_bond");
        let m = find_mapping(&maps, "bond_kind");
        assert_eq!(
            m.sources,
            vec![ColumnSource::Literal {
                value: "'1500'".to_string()
            }]
        );
    }

    #[test]
    fn update_set_maps_each_assignment() {
        let maps = column_mappings_of(
            "UPDATE dat_trd t SET t.net_amount = decode(t.bs, 'B', t.cjje + t.fee, t.cjje - t.fee)",
        );

        let m = find_mapping(&maps, "net_amount");
        assert_eq!(m.target_table.as_deref(), Some("dat_trd"));
        assert_eq!(m.kind, MappingKind::Derived);
        assert_eq!(
            m.sources,
            vec![
                col(Some("dat_trd"), "bs"),
                col(Some("dat_trd"), "cjje"),
                col(Some("dat_trd"), "fee"),
            ]
        );
    }

    #[test]
    fn insert_values_maps_literals_and_columns() {
        let maps = column_mappings_of("INSERT INTO t (a, b) VALUES ('0', 42)");
        assert_eq!(
            find_mapping(&maps, "a").sources,
            vec![ColumnSource::Literal {
                value: "'0'".to_string()
            }]
        );
        assert_eq!(
            find_mapping(&maps, "b").sources,
            vec![ColumnSource::Literal {
                value: "42".to_string()
            }]
        );
    }

    /// Every union branch feeds the same target column. This needs both the extractor's
    /// set-operation walk and the parser's chain fix (c2j/ogsql-parser#318).
    #[test]
    fn every_union_branch_contributes_to_the_same_target() {
        let maps = column_mappings_of(
            "INSERT INTO dst (v) \
             SELECT a FROM t1 UNION ALL SELECT b FROM t2 UNION ALL SELECT c FROM t3",
        );

        assert_eq!(
            sources_for(&maps, "v"),
            vec![
                &col(Some("t1"), "a"),
                &col(Some("t2"), "b"),
                &col(Some("t3"), "c"),
            ]
        );
    }

    /// A view over a union names its output columns from the first branch; later branches
    /// select differently-named columns into the same position.
    #[test]
    fn union_view_aligns_renamed_branches_by_position() {
        let maps = view_column_mappings(
            "v_par_bond",
            &[],
            "SELECT security_id, bond_kind FROM par_sys_bond \
             UNION ALL SELECT security_id, entrust_type AS bond_kind FROM par_sys_entrust \
             UNION ALL SELECT security_id, fund_type FROM par_sys_annuity_fund",
        );

        assert_eq!(
            sources_for(&maps, "bond_kind"),
            vec![
                &col(Some("par_sys_bond"), "bond_kind"),
                &col(Some("par_sys_entrust"), "entrust_type"),
                &col(Some("par_sys_annuity_fund"), "fund_type"),
            ],
            "the third branch has no alias, so only its position identifies it"
        );
        for m in maps.iter().filter(|m| m.target_column == "bond_kind") {
            assert_eq!(m.position, Some(1), "all three sit in select position 1");
        }
    }

    #[test]
    fn declared_view_columns_win_over_branch_aliases() {
        let maps = view_column_mappings("v_named", &["out_a", "out_b"], "SELECT x, y FROM t");
        assert_eq!(
            find_mapping(&maps, "out_a").sources,
            vec![col(Some("t"), "x")]
        );
        assert_eq!(
            find_mapping(&maps, "out_b").sources,
            vec![col(Some("t"), "y")]
        );
    }

    /// `SELECT *` cannot be aligned against a column list without a schema. Emitting
    /// mappings anyway would shift every column by one, which is worse than emitting none.
    #[test]
    fn select_star_yields_no_mappings_rather_than_shifted_ones() {
        let maps = column_mappings_of("INSERT INTO dst (a, b) SELECT * FROM src");
        assert!(
            maps.is_empty(),
            "star select should not be aligned positionally: {maps:#?}"
        );
    }

    #[test]
    fn unqualified_column_over_a_join_leaves_the_owner_unresolved() {
        let maps = column_mappings_of(
            "INSERT INTO dst (v) SELECT amount FROM t1 JOIN t2 ON t1.id = t2.id",
        );
        assert_eq!(
            find_mapping(&maps, "v").sources,
            vec![col(None, "amount")],
            "an unqualified column over two tables has no single owner"
        );
    }
}

/// Verification tests for ogsql-parser's ability to extract SQL statements
/// from stored procedure bodies. These tests validate the foundation for
/// the `search_sql` feature extension to cover Procedure/Function nodes.
#[cfg(test)]
mod procedure_sql_extraction {
    use super::*;
    use ogsql_parser::ast::plpgsql::PlStatement;
    use ogsql_parser::ast::Statement;
    use ogsql_parser::{walk_pl_block, Tokenizer};

    /// A simple visitor that collects SQL texts from PL/pgSQL procedure bodies.
    struct ProcedureSqlCollector {
        sql_texts: Vec<(String, Option<String>)>, // (sql_text, statement_kind)
    }

    impl ProcedureSqlCollector {
        fn new() -> Self {
            Self {
                sql_texts: Vec::new(),
            }
        }
    }

    impl ogsql_parser::Visitor for ProcedureSqlCollector {
        fn visit_pl_statement(
            &mut self,
            stmt: &ogsql_parser::ast::plpgsql::PlStatement,
        ) -> VisitorResult {
            match stmt {
                PlStatement::SqlStatement {
                    sql_text,
                    statement,
                    ..
                } => {
                    let kind = match statement.as_ref() {
                        Statement::Select(_) => Some("SELECT".to_string()),
                        Statement::Insert(_) => Some("INSERT".to_string()),
                        Statement::Update(_) => Some("UPDATE".to_string()),
                        Statement::Delete(_) => Some("DELETE".to_string()),
                        Statement::Merge(_) => Some("MERGE".to_string()),
                        _ => None,
                    };
                    self.sql_texts.push((sql_text.clone(), kind));
                }
                PlStatement::Sql(sql_text) => {
                    self.sql_texts.push((sql_text.clone(), None));
                }
                _ => {}
            }
            VisitorResult::Continue
        }
    }

    fn parse_procedure_and_collect(sql: &str) -> Vec<(String, Option<String>)> {
        let tokens = Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let stmts = parser.parse_with_text();

        let mut collector = ProcedureSqlCollector::new();
        for info in &stmts {
            match &info.statement {
                Statement::CreateProcedure(p) => {
                    if let Some(ref block) = p.block {
                        walk_pl_block(&mut collector, block);
                    }
                }
                Statement::CreateFunction(f) => {
                    if let Some(ref block) = f.block {
                        walk_pl_block(&mut collector, block);
                    }
                }
                _ => {}
            }
        }
        collector.sql_texts
    }

    // ──── Basic SQL statement extraction ────

    #[test]
    fn extract_select_from_procedure_body() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE get_users()
            AS BEGIN
                SELECT * FROM t_users WHERE status = 'ACTIVE';
            END;
            /
        "#;
        let results = parse_procedure_and_collect(sql);
        assert!(!results.is_empty(), "should extract at least one SQL");
        let has_select = results
            .iter()
            .any(|(text, kind)| kind.as_deref() == Some("SELECT") && text.contains("t_users"));
        assert!(
            has_select,
            "should find SELECT with t_users, got: {:?}",
            results
        );
    }

    #[test]
    fn extract_insert_from_procedure_body() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE add_user(p_name VARCHAR)
            AS BEGIN
                INSERT INTO t_users(name, status) VALUES(p_name, 'PENDING');
            END;
            /
        "#;
        let results = parse_procedure_and_collect(sql);
        let has_insert = results
            .iter()
            .any(|(text, kind)| kind.as_deref() == Some("INSERT") && text.contains("t_users"));
        assert!(
            has_insert,
            "should find INSERT into t_users, got: {:?}",
            results
        );
    }

    #[test]
    fn extract_update_from_procedure_body() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE activate_user(p_id INT)
            AS BEGIN
                UPDATE t_users SET status = 'ACTIVE' WHERE id = p_id;
            END;
            /
        "#;
        let results = parse_procedure_and_collect(sql);
        let has_update = results
            .iter()
            .any(|(text, kind)| kind.as_deref() == Some("UPDATE") && text.contains("t_users"));
        assert!(has_update, "should find UPDATE t_users, got: {:?}", results);
    }

    #[test]
    fn extract_delete_from_procedure_body() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE remove_user(p_id INT)
            AS BEGIN
                DELETE FROM t_users WHERE id = p_id;
            END;
            /
        "#;
        let results = parse_procedure_and_collect(sql);
        let has_delete = results
            .iter()
            .any(|(text, kind)| kind.as_deref() == Some("DELETE") && text.contains("t_users"));
        assert!(
            has_delete,
            "should find DELETE from t_users, got: {:?}",
            results
        );
    }

    #[test]
    fn extract_multiple_sql_statements_from_body() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE process_order(p_id INT)
            AS
            BEGIN
                UPDATE t_orders SET status = 'PROCESSING' WHERE id = p_id;
                INSERT INTO t_order_log(order_id, action) VALUES(p_id, 'PROCESSING');
                DELETE FROM t_pending WHERE order_id = p_id;
            END;
            /
        "#;
        let results = parse_procedure_and_collect(sql);
        assert!(
            results.len() >= 3,
            "should extract at least 3 SQL statements, got {}: {:?}",
            results.len(),
            results
        );
    }

    // ──── Function bodies ────

    #[test]
    fn extract_sql_from_function_body() {
        let sql = r#"
            CREATE OR REPLACE FUNCTION count_users() RETURNS INT
            AS $$
            DECLARE v_count INT;
            BEGIN
                SELECT COUNT(*) INTO v_count FROM t_users;
                RETURN v_count;
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let results = parse_procedure_and_collect(sql);
        let has_select = results
            .iter()
            .any(|(text, kind)| kind.as_deref() == Some("SELECT") && text.contains("t_users"));
        assert!(
            has_select,
            "should find SELECT from function body, got: {:?}",
            results
        );
    }

    // ──── Control flow nesting ────

    #[test]
    fn extract_sql_from_if_block() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE conditional_update(p_id INT, p_action VARCHAR)
            AS
            BEGIN
                IF p_action = 'activate' THEN
                    UPDATE t_users SET status = 'ACTIVE' WHERE id = p_id;
                ELSE
                    UPDATE t_users SET status = 'INACTIVE' WHERE id = p_id;
                END IF;
            END;
            /
        "#;
        let results = parse_procedure_and_collect(sql);
        let update_count = results
            .iter()
            .filter(|(text, kind)| kind.as_deref() == Some("UPDATE") && text.contains("t_users"))
            .count();
        assert!(
            update_count >= 2,
            "should find 2 UPDATE statements in IF/ELSE, got {}: {:?}",
            update_count,
            results
        );
    }

    #[test]
    fn extract_sql_from_loop() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE batch_process()
            AS
            BEGIN
                FOR i IN 1..10 LOOP
                    INSERT INTO t_log(msg) VALUES('processing');
                END LOOP;
            END;
            /
        "#;
        let results = parse_procedure_and_collect(sql);
        let has_insert = results
            .iter()
            .any(|(text, kind)| kind.as_deref() == Some("INSERT") && text.contains("t_log"));
        assert!(
            has_insert,
            "should find INSERT inside FOR loop, got: {:?}",
            results
        );
    }

    #[test]
    fn extract_sql_from_nested_blocks() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE nested_blocks()
            AS
            BEGIN
                SELECT * FROM t_config;

                BEGIN
                    INSERT INTO t_audit(action) VALUES('inner_block');
                END;
            END;
            /
        "#;
        let results = parse_procedure_and_collect(sql);
        assert!(
            results.len() >= 2,
            "should find SQL from both outer and inner blocks, got {}: {:?}",
            results.len(),
            results
        );
    }

    #[test]
    fn extract_sql_from_exception_handler() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE with_exception()
            AS
            BEGIN
                UPDATE t_orders SET status = 'DONE' WHERE id = 1;
            EXCEPTION
                WHEN OTHERS THEN
                    INSERT INTO t_error_log(msg) VALUES('order failed');
            END;
            /
        "#;
        let results = parse_procedure_and_collect(sql);
        assert!(
            results.len() >= 2,
            "should find SQL from both body and exception handler, got {}: {:?}",
            results.len(),
            results
        );
        let has_insert = results
            .iter()
            .any(|(text, kind)| kind.as_deref() == Some("INSERT") && text.contains("t_error_log"));
        assert!(
            has_insert,
            "should find INSERT in exception handler, got: {:?}",
            results
        );
    }

    // ──── sql_text quality ────

    #[test]
    fn sql_text_is_searchable_raw_text() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE search_test()
            AS BEGIN
                SELECT id, name, status FROM t_users WHERE status = 'ACTIVE' ORDER BY id;
            END;
            /
        "#;
        let results = parse_procedure_and_collect(sql);
        let select_result = results
            .iter()
            .find(|(_, kind)| kind.as_deref() == Some("SELECT"));
        assert!(select_result.is_some(), "should find SELECT statement");

        let sql_text = &select_result.unwrap().0;
        // sql_text should contain the raw SQL that can be searched
        let lower = sql_text.to_lowercase();
        assert!(
            lower.contains("select"),
            "sql_text should contain 'select': {}",
            sql_text
        );
        assert!(
            lower.contains("t_users"),
            "sql_text should contain 't_users': {}",
            sql_text
        );
        assert!(
            lower.contains("where"),
            "sql_text should contain 'where': {}",
            sql_text
        );
    }

    // ──── Package body procedures ────

    #[test]
    fn extract_sql_from_package_body_procedure() {
        let sql = r#"
            CREATE OR REPLACE PACKAGE BODY pkg_users AS
                PROCEDURE activate(p_id INT) IS
                BEGIN
                    UPDATE t_users SET status = 'ACTIVE' WHERE id = p_id;
                END;
            END pkg_users;
        "#;
        let tokens = Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let stmts = parser.parse_with_text();

        let mut collector = ProcedureSqlCollector::new();
        for info in &stmts {
            if let Statement::CreatePackageBody(pkg) = &info.statement {
                for item in &pkg.items {
                    if let ogsql_parser::ast::PackageItem::Procedure(p) = item {
                        if let Some(ref block) = p.block {
                            walk_pl_block(&mut collector, block);
                        }
                    }
                }
            }
        }

        let has_update = collector
            .sql_texts
            .iter()
            .any(|(text, kind)| kind.as_deref() == Some("UPDATE") && text.contains("t_users"));
        assert!(
            has_update,
            "should find UPDATE in package body procedure, got: {:?}",
            collector.sql_texts
        );
    }

    // ──── Dialect variations ────

    #[test]
    fn extract_from_as_begin_end_procedure() {
        // openGauss/GaussDB: AS BEGIN ... END; /
        let sql = r#"
            CREATE OR REPLACE PROCEDURE test_as()
            AS BEGIN
                SELECT 1 FROM dual;
            END;
            /
        "#;
        let results = parse_procedure_and_collect(sql);
        assert!(
            !results.is_empty(),
            "AS BEGIN..END procedure should yield SQL"
        );
    }

    #[test]
    fn extract_from_is_begin_end_procedure() {
        // Oracle-compatible: IS BEGIN ... END;
        let sql = r#"
            CREATE OR REPLACE PROCEDURE test_is()
            IS
            BEGIN
                SELECT 1 FROM dual;
            END;
        "#;
        let results = parse_procedure_and_collect(sql);
        assert!(
            !results.is_empty(),
            "IS BEGIN..END procedure should yield SQL"
        );
    }

    #[test]
    fn extract_from_plpgsql_dollar_quoted() {
        // PostgreSQL-style: $$ ... $$ LANGUAGE plpgsql
        let sql = r#"
            CREATE OR REPLACE FUNCTION test_dollar() RETURNS VOID
            AS $$
            BEGIN
                SELECT * FROM t_config;
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let results = parse_procedure_and_collect(sql);
        assert!(
            !results.is_empty(),
            "$$-quoted function body should yield SQL"
        );
    }

    // ──── Edge cases ────

    #[test]
    fn procedure_with_no_body_produces_no_sql() {
        let sql = "CREATE PROCEDURE no_body() AS BEGIN NULL; END; /";
        let results = parse_procedure_and_collect(sql);
        // NULL statement should not produce SQL texts
        let non_null = results
            .iter()
            .filter(|(text, _)| !text.trim().eq_ignore_ascii_case("null"))
            .count();
        assert_eq!(
            non_null, 0,
            "procedure with only NULL should produce no searchable SQL, got: {:?}",
            results
        );
    }

    #[test]
    fn procedure_with_declare_block() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE with_declare()
            AS $$
            DECLARE
                v_count INT;
                v_name VARCHAR(100);
            BEGIN
                SELECT COUNT(*) INTO v_count FROM t_users;
                INSERT INTO t_log(cnt) VALUES(v_count);
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let results = parse_procedure_and_collect(sql);
        assert!(
            results.len() >= 2,
            "should find SQL despite DECLARE block, got {}: {:?}",
            results.len(),
            results
        );
    }

    #[test]
    fn cursor_declaration_sql_extracted() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE process_items(p_id INT)
            AS $$
            DECLARE
                CURSOR c_items(p_cat INT) IS
                    SELECT id, name, price
                    FROM products
                    WHERE category_id = p_cat
                    FOR UPDATE;
            BEGIN
                FOR rec IN c_items(p_id) LOOP
                    NULL;
                END LOOP;
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let results = extract_body_sql_from_sql(sql);
        assert!(
            !results.is_empty(),
            "cursor SQL should be extracted, got empty"
        );
        assert!(
            results.iter().any(|s| s.sql_text.contains("products")),
            "cursor SQL should contain table name, got: {:?}",
            results
        );
    }

    #[test]
    fn cursor_declaration_kind_is_select() {
        let sql = r#"
            CREATE OR REPLACE FUNCTION get_orders(p_status TEXT)
            RETURNS REFCURSOR AS $$
            DECLARE
                ref REFCURSOR;
                CURSOR c_orders IS
                    SELECT o.id, o.total
                    FROM orders o
                    WHERE o.status = p_status;
            BEGIN
                OPEN c_orders;
                RETURN c_orders;
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let results = extract_body_sql_from_sql(sql);
        let cursor_sql = results.iter().find(|s| s.sql_text.contains("orders"));
        assert!(cursor_sql.is_some(), "cursor SQL should be extracted");
        if let Some(cs) = cursor_sql {
            assert_eq!(
                cs.kind, "SELECT",
                "cursor should be kind SELECT, got: {}",
                cs.kind
            );
        }
    }

    #[test]
    fn perform_query_extracted() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE check_health()
            AS $$
            BEGIN
                PERFORM 1 FROM dual WHERE status = 'OK';
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let results = extract_body_sql_from_sql(sql);
        let has_perform = results.iter().any(|s| s.sql_text.contains("dual"));
        assert!(
            has_perform,
            "PERFORM query should be extracted, got: {:?}",
            results
        );
    }

    #[test]
    fn perform_function_call_extracted() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE setup_session()
            AS $$
            BEGIN
                PERFORM set_config('work_mem', '256MB', true);
                PERFORM pg_sleep(0.1);
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let results = extract_body_sql_from_sql(sql);
        let has_set_config = results.iter().any(|s| s.sql_text.contains("set_config"));
        assert!(
            has_set_config,
            "PERFORM function call should be extracted, got: {:?}",
            results
        );
    }

    #[test]
    fn execute_immediate_sql_extracted() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE dynamic_update(p_id INT)
            AS $$
            BEGIN
                EXECUTE IMMEDIATE 'UPDATE t_items SET status = ''DONE'' WHERE id = ' || p_id;
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let results = extract_body_sql_from_sql(sql);
        let has_update = results.iter().any(|s| s.sql_text.contains("UPDATE"));
        assert!(
            has_update,
            "EXECUTE IMMEDIATE SQL should be extracted, got: {:?}",
            results
        );
    }

    #[test]
    fn execute_immediate_with_parsed_query_kind() {
        let sql = r#"
            CREATE OR REPLACE PROCEDURE archive_old(p_days INT)
            AS $$
            BEGIN
                EXECUTE 'DELETE FROM t_logs WHERE created_at < NOW() - INTERVAL ''1 day'' * p_days';
            END;
            $$ LANGUAGE plpgsql;
        "#;
        let results = extract_body_sql_from_sql(sql);
        if let Some(ex) = results.iter().find(|s| s.sql_text.contains("DELETE")) {
            assert_eq!(
                ex.kind, "DELETE",
                "EXECUTE with parsed DELETE should have kind DELETE"
            );
        }
    }

    fn extract_body_sql_from_sql(sql: &str) -> Vec<ProcedureBodySql> {
        let tokens = Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let stmts = parser.parse_with_text();
        for info in &stmts {
            match &info.statement {
                Statement::CreateProcedure(p) => {
                    if let Some(ref block) = p.block {
                        return extract_body_sql(block);
                    }
                }
                Statement::CreateFunction(f) => {
                    if let Some(ref block) = f.block {
                        return extract_body_sql(block);
                    }
                }
                _ => {}
            }
        }
        Vec::new()
    }

    #[test]
    fn operator_any_extracted_as_builtin() {
        let sql = "CREATE OR REPLACE PROCEDURE test_any() AS $$
        BEGIN
            FOR r IN (SELECT * FROM t1 WHERE col > ANY(SELECT col FROM t2)) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let stmts = ogsql_parser::Parser::parse_sql(sql).0;
        let mut extractor = CallExtractor::new(
            std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
            std::collections::HashSet::new(),
        );
        ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

        let any_edges: Vec<_> = extractor
            .edges
            .iter()
            .filter(|e| e.builtin_meta.is_some() && e.callee_name == "ANY")
            .collect();
        assert!(
            !any_edges.is_empty(),
            "expected ANY operator to be extracted"
        );
        assert_eq!(
            any_edges[0].builtin_meta.as_ref().unwrap().category,
            "Operator"
        );
        assert_eq!(
            any_edges[0].builtin_meta.as_ref().unwrap().domain,
            "Comparison"
        );
    }

    #[test]
    fn operator_exists_extracted_as_builtin() {
        let sql = "CREATE OR REPLACE PROCEDURE test_exists() AS $$
        BEGIN
            FOR r IN (SELECT * FROM t1 WHERE EXISTS(SELECT 1 FROM t2 WHERE t2.id = t1.id)) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let stmts = ogsql_parser::Parser::parse_sql(sql).0;
        let mut extractor = CallExtractor::new(
            std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
            std::collections::HashSet::new(),
        );
        ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

        let exists_edges: Vec<_> = extractor
            .edges
            .iter()
            .filter(|e| e.builtin_meta.is_some() && e.callee_name == "EXISTS")
            .collect();
        assert!(
            !exists_edges.is_empty(),
            "expected EXISTS operator to be extracted"
        );
        assert_eq!(
            exists_edges[0].builtin_meta.as_ref().unwrap().category,
            "Operator"
        );
        assert_eq!(
            exists_edges[0].builtin_meta.as_ref().unwrap().domain,
            "Predicate"
        );
    }

    #[test]
    fn operator_in_subquery_extracted_as_builtin() {
        let sql = "CREATE OR REPLACE PROCEDURE test_in() AS $$
        BEGIN
            FOR r IN (SELECT * FROM t1 WHERE col IN (SELECT col FROM t2)) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let stmts = ogsql_parser::Parser::parse_sql(sql).0;
        let mut extractor = CallExtractor::new(
            std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
            std::collections::HashSet::new(),
        );
        ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

        let in_edges: Vec<_> = extractor
            .edges
            .iter()
            .filter(|e| e.builtin_meta.is_some() && e.callee_name == "IN")
            .collect();
        assert!(!in_edges.is_empty(), "expected IN operator to be extracted");
    }

    #[test]
    fn operator_all_extracted_as_builtin() {
        let sql = "CREATE OR REPLACE PROCEDURE test_all() AS $$
        BEGIN
            FOR r IN (SELECT * FROM t1 WHERE col > ALL(SELECT col FROM t2)) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let stmts = ogsql_parser::Parser::parse_sql(sql).0;
        let mut extractor = CallExtractor::new(
            std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
            std::collections::HashSet::new(),
        );
        ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

        let all_edges: Vec<_> = extractor
            .edges
            .iter()
            .filter(|e| e.builtin_meta.is_some() && e.callee_name == "ALL")
            .collect();
        assert!(
            !all_edges.is_empty(),
            "expected ALL operator to be extracted"
        );
    }

    #[test]
    fn operator_some_kept_separate_from_any() {
        let sql = "CREATE OR REPLACE PROCEDURE test_some() AS $$
        BEGIN
            FOR r IN (SELECT * FROM t1 WHERE col = SOME(SELECT col FROM t2)) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let stmts = ogsql_parser::Parser::parse_sql(sql).0;
        let mut extractor = CallExtractor::new(
            std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
            std::collections::HashSet::new(),
        );
        ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

        let some_edges: Vec<_> = extractor
            .edges
            .iter()
            .filter(|e| e.builtin_meta.is_some() && e.callee_name == "SOME")
            .collect();
        assert!(
            !some_edges.is_empty(),
            "expected SOME operator to be extracted as 'SOME'"
        );

        let any_edges: Vec<_> = extractor
            .edges
            .iter()
            .filter(|e| e.builtin_meta.is_some() && e.callee_name == "ANY")
            .collect();
        assert!(any_edges.is_empty(), "SOME should NOT create an ANY node");
    }

    #[test]
    fn operator_not_in_extracted_as_builtin() {
        let sql = "CREATE OR REPLACE PROCEDURE test_not_in() AS $$
        BEGIN
            FOR r IN (SELECT * FROM t1 WHERE col NOT IN (SELECT col FROM t2)) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let stmts = ogsql_parser::Parser::parse_sql(sql).0;
        let mut extractor = CallExtractor::new(
            std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
            std::collections::HashSet::new(),
        );
        ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

        let edges: Vec<_> = extractor
            .edges
            .iter()
            .filter(|e| e.builtin_meta.is_some() && e.callee_name == "NOT_IN")
            .collect();
        assert!(
            !edges.is_empty(),
            "expected NOT_IN operator to be extracted"
        );
        assert_eq!(edges[0].builtin_meta.as_ref().unwrap().domain, "Predicate");
    }

    #[test]
    fn hint_tablescan_extracted_as_builtin() {
        let sql = "CREATE OR REPLACE PROCEDURE test_hint() AS $$
        DECLARE
            r RECORD;
        BEGIN
            SELECT /*+ tablescan(t1) */ * INTO r FROM t1;
        END;
        $$ LANGUAGE plpgsql;";
        let stmts = ogsql_parser::Parser::parse_sql(sql).0;
        let mut extractor = CallExtractor::new(
            std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
            std::collections::HashSet::new(),
        );
        ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

        let hint_edges: Vec<_> = extractor
            .edges
            .iter()
            .filter(|e| e.builtin_meta.is_some() && e.callee_name == "tablescan")
            .collect();
        assert!(
            !hint_edges.is_empty(),
            "expected tablescan hint to be extracted"
        );
        assert_eq!(
            hint_edges[0].builtin_meta.as_ref().unwrap().category,
            "Hint"
        );
        assert_eq!(
            hint_edges[0].builtin_meta.as_ref().unwrap().domain,
            "QueryPlan"
        );
    }

    #[test]
    fn hint_nestloop_extracted_as_builtin() {
        let sql = "CREATE OR REPLACE PROCEDURE test_nestloop() AS $$
        BEGIN
            FOR r IN (SELECT /*+ nestloop(t1 t2) */ * FROM t1 JOIN t2 ON t1.id = t2.id) LOOP
                NULL;
            END LOOP;
        END;
        $$ LANGUAGE plpgsql;";
        let stmts = ogsql_parser::Parser::parse_sql(sql).0;
        let mut extractor = CallExtractor::new(
            std::sync::Arc::new(std::path::PathBuf::from("test.sql")),
            std::collections::HashSet::new(),
        );
        ogsql_parser::walk_statement(&mut extractor, &stmts[0].statement);

        let nestloop_edges: Vec<_> = extractor
            .edges
            .iter()
            .filter(|e| e.builtin_meta.is_some() && e.callee_name == "nestloop")
            .collect();
        assert!(
            !nestloop_edges.is_empty(),
            "expected nestloop hint to be extracted"
        );
        assert_eq!(
            nestloop_edges[0].builtin_meta.as_ref().unwrap().category,
            "Hint"
        );
    }
}
