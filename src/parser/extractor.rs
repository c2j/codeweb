use crate::graph::{ProcedureId, SourceLocation};
use ogsql_parser::ast::plpgsql::{PlExecuteStmt, PlProcedureCall, PlStatement};
use ogsql_parser::ast::{CallFuncStatement, Statement};
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
        if let PlStatement::Execute(PlExecuteStmt {
            parsed_query: None,
            string_expr,
            ..
        }) = stmt
        {
            let raw = format!("{:?}", string_expr);
            self.push_call(&raw, true, 0);
        }
        VisitorResult::Continue
    }
}
