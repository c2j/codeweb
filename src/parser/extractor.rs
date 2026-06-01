use crate::graph::{AccessMode, RoutineId, RoutineKind, SourceLocation, WriteKind};
use ogsql_parser::ast::plpgsql::{PlExecuteStmt, PlProcedureCall, PlStatement};
use ogsql_parser::ast::{
    CallFuncStatement, DataType, Expr, InsertStatement, JoinType as AstJoinType, Literal,
    ObjectName, SelectStatement, SelectTarget, SequenceFunc, Statement, TableRef as AstTableRef,
    UpdateStatement, WhenClause,
};
use ogsql_parser::{Visitor, VisitorResult};
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct CallEdge {
    pub caller: Option<RoutineId>,
    pub callee_name: String,
    pub is_dynamic: bool,
    pub location: SourceLocation,
}

pub struct CallExtractor {
    pub current_procedure: Option<RoutineId>,
    pub edges: Vec<CallEdge>,
    file: Arc<std::path::PathBuf>,
}

impl CallExtractor {
    pub fn new(file: Arc<std::path::PathBuf>) -> Self {
        Self {
            current_procedure: None,
            edges: Vec::new(),
            file,
        }
    }

    fn make_location(&self, line: usize) -> SourceLocation {
        SourceLocation {
            file: self.file.clone(),
            line,
        }
    }

    pub fn push_call(&mut self, callee: &str, is_dynamic: bool, line: usize) {
        self.edges.push(CallEdge {
            caller: self.current_procedure.clone(),
            callee_name: callee.to_string(),
            is_dynamic,
            location: self.make_location(line),
        });
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
}

impl Visitor for CallExtractor {
    fn visit_statement(&mut self, stmt: &Statement) -> VisitorResult {
        match stmt {
            Statement::CreateProcedure(p) => {
                let id = RoutineId::from_object_name(&p.name, RoutineKind::Procedure);
                self.current_procedure = Some(id);
            }
            Statement::CreateFunction(f) => {
                let id = RoutineId::from_object_name(&f.name, RoutineKind::Function);
                self.current_procedure = Some(id);
            }
            _ => {}
        }
        VisitorResult::Continue
    }

    fn visit_call(&mut self, call: &CallFuncStatement) -> VisitorResult {
        let name: String = call.func_name.join(".");
        self.push_call(&name, false, 0);
        VisitorResult::Continue
    }

    fn visit_procedure_call(&mut self, call: &PlProcedureCall) -> VisitorResult {
        let name: String = call.name.join(".");
        self.push_call(&name, false, 0);
        VisitorResult::Continue
    }

    fn visit_pl_statement(&mut self, stmt: &PlStatement) -> VisitorResult {
        match stmt {
            PlStatement::Execute(ogsql_parser::ast::Spanned {
                node:
                    PlExecuteStmt {
                        parsed_query: None,
                        string_expr,
                        ..
                    },
                ..
            }) => {
                let raw = format!("{:?}", string_expr);
                self.push_call(&raw, true, 0);
            }
            PlStatement::Sql(sql_text) => {
                self.extract_call_from_sql_text(sql_text);
            }
            _ => {}
        }
        VisitorResult::Continue
    }

    fn visit_select(&mut self, select: &SelectStatement) -> VisitorResult {
        for tr in &select.from {
            self.extract_func_from_table_ref(tr);
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
}

impl TableAccessExtractor {
    pub fn new() -> Self {
        Self {
            accesses: Vec::new(),
        }
    }

    fn add_access(&mut self, name: &ObjectName, mode: AccessMode, write_kind: Option<WriteKind>) {
        if name.is_empty() {
            return;
        }
        let (schema, table_name) = if name.len() == 1 {
            (None, name[0].clone())
        } else {
            (
                Some(name[..name.len() - 1].join(".")),
                name[name.len() - 1].clone(),
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

    fn extract_reads_from_table_refs(&mut self, table_refs: &[AstTableRef]) {
        for tr in table_refs {
            match tr {
                AstTableRef::Table { name, .. } => {
                    self.add_access(name, AccessMode::Read, None);
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
                    self.add_access(name, AccessMode::Write, Some(kind));
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
                            self.extract_lock_read_from_object_name(name);
                        }
                    }
                    return VisitorResult::Continue;
                }
                ogsql_parser::ast::LockClause::Update { tables, .. } => {
                    self.extract_reads_from_table_refs(&select.from);
                    for name in tables {
                        self.extract_lock_read_from_object_name(name);
                    }
                    return VisitorResult::Continue;
                }
                ogsql_parser::ast::LockClause::Share { tables, .. } => {
                    self.extract_reads_from_table_refs(&select.from);
                    for name in tables {
                        self.extract_lock_read_from_object_name(name);
                    }
                    return VisitorResult::Continue;
                }
                ogsql_parser::ast::LockClause::NoKeyUpdate { tables, .. } => {
                    self.extract_reads_from_table_refs(&select.from);
                    for name in tables {
                        self.extract_lock_read_from_object_name(name);
                    }
                    return VisitorResult::Continue;
                }
                ogsql_parser::ast::LockClause::KeyShare { tables, .. } => {
                    self.extract_reads_from_table_refs(&select.from);
                    for name in tables {
                        self.extract_lock_read_from_object_name(name);
                    }
                    return VisitorResult::Continue;
                }
            }
        }

        self.extract_reads_from_table_refs(&select.from);
        VisitorResult::Continue
    }

    fn visit_insert(&mut self, insert: &ogsql_parser::ast::InsertStatement) -> VisitorResult {
        let write_kind = match &insert.source {
            ogsql_parser::ast::InsertSource::Select(_) => WriteKind::InsertSelect,
            _ => WriteKind::Insert,
        };
        self.add_access(&insert.table, AccessMode::Write, Some(write_kind));

        if let ogsql_parser::ast::InsertSource::Select(ref select_stmt) = insert.source {
            let stmt = Statement::Select(ogsql_parser::ast::Spanned {
                node: select_stmt.as_ref().clone(),
                span: None,
            });
            ogsql_parser::walk_statement(self, &stmt);
        }
        VisitorResult::Continue
    }

    fn visit_update(&mut self, update: &ogsql_parser::ast::UpdateStatement) -> VisitorResult {
        self.extract_writes_from_table_refs(&update.tables, WriteKind::Update);
        self.extract_reads_from_table_refs(&update.from);
        VisitorResult::Continue
    }

    fn visit_delete(&mut self, delete: &ogsql_parser::ast::DeleteStatement) -> VisitorResult {
        self.extract_writes_from_table_refs(&delete.tables, WriteKind::Delete);
        self.extract_reads_from_table_refs(&delete.using);
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
}

/// Table alias definition.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TableAlias {
    pub schema: Option<String>,
    pub table: String,
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
    clause_stack: Vec<ColumnContext>,
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
            clause_stack: Vec::new(),
        }
    }

    pub fn finish(self) -> ColumnAnalysis {
        ColumnAnalysis {
            column_refs: dedup_column_refs(self.column_refs),
            alias_map: self.alias_map,
            join_conditions: self.join_conditions,
            hard_filters: self.hard_filters,
            enum_mappings: self.enum_mappings,
            select_into: self.select_into,
            insert_columns: self.insert_columns,
            update_columns: self.update_columns,
        }
    }

    fn current_clause(&self) -> Option<ColumnContext> {
        self.clause_stack.last().copied()
    }

    fn resolve_alias(&self, prefix: &str) -> Option<&TableAlias> {
        self.alias_map.get(&prefix.to_lowercase())
    }

    fn add_column_ref(&mut self, names: &[String], extra_context: Option<ColumnContext>) {
        let (alias_prefix, column) = if names.len() >= 2 {
            (Some(names[0].clone()), names[names.len() - 1].clone())
        } else {
            (None, names[0].clone())
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
                AstTableRef::Table { name, alias, .. } => {
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
        left_names: &[String],
        right_names: &[String],
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

    fn add_hard_filter(&mut self, col_names: &[String], op: FilterOperator, val: FilterValue) {
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
    fn visit_select(&mut self, select: &SelectStatement) -> VisitorResult {
        // Collect aliases from FROM clause
        self.collect_aliases_from_table_refs(&select.from);

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

        VisitorResult::Continue
    }

    fn visit_insert(&mut self, insert: &InsertStatement) -> VisitorResult {
        let table_name = insert.table.last().cloned().unwrap_or_default();
        self.insert_columns.push(InsertColumnInfo {
            table: table_name,
            columns: insert.columns.clone(),
        });
        VisitorResult::Continue
    }

    fn visit_update(&mut self, update: &UpdateStatement) -> VisitorResult {
        // Collect aliases from update tables
        for tr in &update.tables {
            if let AstTableRef::Table {
                name,
                alias: Some(a),
                ..
            } = tr
            {
                let (schema, table) = split_schema_table(name);
                self.alias_map
                    .insert(a.to_lowercase(), TableAlias { schema, table });
            }
        }

        let table_name = update
            .tables
            .first()
            .and_then(|tr| {
                if let AstTableRef::Table { name, .. } = tr {
                    Some(name.last().cloned().unwrap_or_default())
                } else {
                    None
                }
            })
            .unwrap_or_default();

        let set_columns: Vec<String> = update
            .assignments
            .iter()
            .filter_map(|a| a.columns.first()?.last().cloned())
            .collect();

        if !table_name.is_empty() {
            self.update_columns.push(UpdateColumnInfo {
                table: table_name,
                set_columns,
            });
        }

        // Walk WHERE clause for column refs and filters
        if let Some(ref where_clause) = update.where_clause {
            self.clause_stack.push(ColumnContext::WhereClause);
            self.process_expr_for_joins_and_filters(where_clause, &AstJoinType::Inner, false);
            self.walk_expr_for_column_refs(where_clause);
            self.clause_stack.pop();
        }

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

fn format_expr_short(expr: &Expr) -> String {
    match expr {
        Expr::ColumnRef(names) => names.join("."),
        Expr::PlVariable(names) => names.join("."),
        Expr::Literal(lit) => format_literal_short(lit),
        Expr::FunctionCall { name, args, .. } => {
            let fname = name.last().cloned().unwrap_or_default();
            let arg_strs: Vec<String> = args.iter().map(format_expr_short).collect();
            format!("{}({})", fname, arg_strs.join(", "))
        }
        _ => format!("{:?}", expr),
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

fn split_alias_column(names: &[String]) -> (Option<String>, String) {
    if names.len() >= 2 {
        (Some(names[0].clone()), names[names.len() - 1].clone())
    } else {
        (None, names[0].clone())
    }
}

fn split_schema_table(name: &ObjectName) -> (Option<String>, String) {
    if name.len() == 1 {
        (None, name[0].clone())
    } else {
        (
            Some(name[..name.len() - 1].join(".")),
            name[name.len() - 1].clone(),
        )
    }
}

/// Check if an expression is a ColumnRef and return the names.
fn as_column_ref(expr: &Expr) -> Option<Vec<String>> {
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
            let mut extractor = CallExtractor::new(Arc::new(PathBuf::from("test.sql")));
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

        let mut extractor = CallExtractor::new(Arc::new(PathBuf::from("test.sql")));

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
        assert!(
            has_update,
            "should find UPDATE t_users, got: {:?}",
            results
        );
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
}
