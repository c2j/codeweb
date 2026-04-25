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

#[cfg(test)]
mod tests {
    use super::*;
    use ogsql_parser::{walk_statement, Tokenizer};

    fn extract_edges(sql: &str) -> Vec<CallEdge> {
        let tokens = Tokenizer::new(sql).tokenize().unwrap();
        let mut parser = ogsql_parser::Parser::with_source(tokens, sql.to_string());
        let stmts = parser.parse_with_text();
        let mut all_edges = Vec::new();
        for info in &stmts {
            let mut extractor = CallExtractor::new(PathBuf::from("test.sql"));
            walk_statement(&mut extractor, &info.statement);
            all_edges.extend(extractor.edges);
        }
        all_edges
    }

    #[test]
    fn standalone_procedure_call() {
        let sql = "CREATE PROCEDURE a() AS $$ BEGIN b(); END; $$;";
        let edges = extract_edges(sql);
        assert_eq!(edges.len(), 1);
        assert_eq!(
            edges[0].caller,
            Some(ProcedureId {
                schema: None,
                name: "a".to_string()
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
            Some(ProcedureId {
                schema: None,
                name: "a".to_string()
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

        let mut extractor = CallExtractor::new(PathBuf::from("test.sql"));

        for info in &stmts {
            if let ogsql_parser::ast::Statement::CreatePackageBody(pkg) = &info.statement {
                for item in &pkg.items {
                    if let ogsql_parser::ast::PackageItem::Procedure(p) = item {
                        if let Some(ref block) = p.block {
                            extractor.current_procedure = Some(ProcedureId {
                                schema: None,
                                name: "pkg_api.do_work".to_string(),
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
}
