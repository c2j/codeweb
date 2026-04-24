use crate::graph::{ProcedureId, SourceLocation};
use ogsql_parser::ast::plpgsql::{PlExecuteStmt, PlProcedureCall, PlStatement};
use ogsql_parser::ast::{
    CallFuncStatement, ObjectName, SelectStatement, Statement, TableRef as AstTableRef,
};
use ogsql_parser::{Visitor, VisitorResult};
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct CallEdge {
    pub caller: Option<ProcedureId>,
    pub callee_name: String,
    pub is_dynamic: bool,
    pub location: SourceLocation,
}

pub struct CallExtractor {
    pub current_procedure: Option<ProcedureId>,
    pub edges: Vec<CallEdge>,
    file: PathBuf,
}

impl CallExtractor {
    pub fn new(file: PathBuf) -> Self {
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
                let id = ProcedureId::from_object_name(&p.name);
                self.current_procedure = Some(id);
            }
            Statement::CreateFunction(f) => {
                let id = ProcedureId::from_object_name(&f.name);
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
            PlStatement::Execute(PlExecuteStmt {
                parsed_query: None,
                string_expr,
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
                let stmt = Statement::Select(query.as_ref().clone());
                ogsql_parser::walk_statement(self, &stmt);
            }
            _ => {}
        }
    }
}

#[derive(Debug, Clone)]
pub struct TableRef {
    pub name: String,
    pub schema: Option<String>,
}

pub struct TableRefExtractor {
    pub tables: Vec<TableRef>,
}

impl TableRefExtractor {
    pub fn new() -> Self {
        Self { tables: Vec::new() }
    }

    fn add_table(&mut self, name: &ObjectName) {
        if name.is_empty() {
            return;
        }
        let table_ref = if name.len() == 1 {
            TableRef {
                schema: None,
                name: name[0].clone(),
            }
        } else {
            TableRef {
                schema: Some(name[..name.len() - 1].join(".")),
                name: name[name.len() - 1].clone(),
            }
        };
        if !self
            .tables
            .iter()
            .any(|t| t.schema == table_ref.schema && t.name == table_ref.name)
        {
            self.tables.push(table_ref);
        }
    }

    fn extract_from_table_refs(&mut self, table_refs: &[AstTableRef]) {
        for tr in table_refs {
            match tr {
                AstTableRef::Table { name, .. } => {
                    self.add_table(name);
                }
                AstTableRef::Join { left, right, .. } => {
                    self.extract_from_table_refs(std::slice::from_ref(left));
                    self.extract_from_table_refs(std::slice::from_ref(right));
                }
                AstTableRef::Pivot { source, .. } | AstTableRef::Unpivot { source, .. } => {
                    self.extract_from_table_refs(std::slice::from_ref(source));
                }
                _ => {}
            }
        }
    }
}

impl Visitor for TableRefExtractor {
    fn visit_select(&mut self, select: &ogsql_parser::ast::SelectStatement) -> VisitorResult {
        self.extract_from_table_refs(&select.from);
        VisitorResult::Continue
    }

    fn visit_insert(&mut self, insert: &ogsql_parser::ast::InsertStatement) -> VisitorResult {
        self.add_table(&insert.table);
        VisitorResult::Continue
    }

    fn visit_update(&mut self, update: &ogsql_parser::ast::UpdateStatement) -> VisitorResult {
        self.extract_from_table_refs(&update.tables);
        self.extract_from_table_refs(&update.from);
        VisitorResult::Continue
    }

    fn visit_delete(&mut self, delete: &ogsql_parser::ast::DeleteStatement) -> VisitorResult {
        self.extract_from_table_refs(&delete.tables);
        self.extract_from_table_refs(&delete.using);
        VisitorResult::Continue
    }
}
