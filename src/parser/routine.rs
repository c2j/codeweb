//! Routine signature data extracted alongside the graph (issue #181).
//!
//! A routine's graph node carries only its identity, so the declared parameters
//! travel in a side table keyed by the routine's `NodeKey` string — the same
//! pattern as `procedure_predicates` (#167). They are needed by
//! `codeweb columns --format seed-hints`, where the caller has to know which
//! parameters a routine expects (e.g. the date parameter) before any row can be
//! seeded.

/// A declared routine parameter (`p_i_date VARCHAR2`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RoutineParameter {
    pub name: String,
    /// `IN` / `OUT` / `IN OUT` / `INOUT`, when the declaration states one.
    pub mode: Option<String>,
    /// As the parser reports it, which lowercases keyword types: `VARCHAR2` in the
    /// source arrives as `varchar2`. Consumers must compare case-insensitively.
    pub data_type: String,
    pub default_value: Option<String>,
}

impl RoutineParameter {
    pub fn from_ast(param: &ogsql_parser::ast::RoutineParam) -> Self {
        Self {
            name: param.name.clone(),
            mode: param.mode.clone(),
            data_type: param.data_type.clone(),
            default_value: param.default_value.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_ast_keeps_name_mode_type_and_default() {
        let ast = ogsql_parser::ast::RoutineParam {
            name: "p_i_date".to_string(),
            mode: Some("IN".to_string()),
            data_type: "VARCHAR2".to_string(),
            default_value: Some("'20260101'".to_string()),
        };

        let param = RoutineParameter::from_ast(&ast);

        assert_eq!(param.name, "p_i_date");
        assert_eq!(param.mode.as_deref(), Some("IN"));
        assert_eq!(param.data_type, "VARCHAR2");
        assert_eq!(param.default_value.as_deref(), Some("'20260101'"));
    }
}
