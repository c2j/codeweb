use crate::graph::{AccessMode, RoutineId, RoutineKind, SourceLocation, WriteKind};
use ogsql_parser::ast::plpgsql::{PlExecuteStmt, PlProcedureCall, PlStatement};
use ogsql_parser::ast::{
    CallFuncStatement, DataType, Expr, Literal, ObjectName, SelectStatement, Statement,
    TableRef as AstTableRef,
};
use ogsql_parser::{Visitor, VisitorResult};
use std::collections::HashSet;
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
            PlStatement::Execute(ogsql_parser::ast::Spanned { node: PlExecuteStmt {
                parsed_query: None,
                string_expr,
                ..
            }, .. }) => {
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
                let stmt = Statement::Select(ogsql_parser::ast::Spanned { node: query.as_ref().clone(), span: None });
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
                    let stmt = Statement::Select(ogsql_parser::ast::Spanned { node: query.as_ref().clone(), span: None });
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
                    let stmt = Statement::Select(ogsql_parser::ast::Spanned { node: query.as_ref().clone(), span: None });
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
                let stmt = Statement::Select(ogsql_parser::ast::Spanned { node: insert_all.source.as_ref().clone(), span: None });
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
                let stmt = Statement::Select(ogsql_parser::ast::Spanned { node: insert_first.source.as_ref().clone(), span: None });
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
            let stmt = Statement::Select(ogsql_parser::ast::Spanned { node: select_stmt.as_ref().clone(), span: None });
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
