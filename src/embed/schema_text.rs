//! 从词法字节区间取原文，并用公开 AST 复验类型及默认表达式。

use super::schema::{ConstraintKind, ConstraintSchemaExport};
use ogsql_parser::ast::{
    ColumnConstraint, ColumnDef, CreateTableStatement, Expr, SelectStatement, SelectTarget,
    Statement, TableConstraint,
};
use ogsql_parser::token::{Token, TokenWithSpan};
use ogsql_parser::{Parser, Tokenizer};

pub(super) struct ColumnText {
    pub name: String,
    pub source_type: String,
    pub default_expr_raw: Option<String>,
}

pub(super) fn columns(table: &CreateTableStatement, sql: &str) -> Option<Vec<ColumnText>> {
    let items = table_items(sql)?;
    let mut columns = Vec::new();
    for item in items {
        let parsed = parse(&format!("CREATE TABLE __schema_export ({item})"))?;
        let [Statement::CreateTable(single)] = parsed.as_slice() else {
            return None;
        };
        if let [column] = single.columns.as_slice() {
            if table.columns.get(columns.len()) != Some(column) {
                return None;
            }
            columns.push(column_text(column, item)?);
        } else if single.constraints.len() != 1 {
            return None;
        }
    }
    (columns.len() == table.columns.len()).then_some(columns)
}

pub(super) fn constraints(sql: &str) -> Option<Vec<ConstraintSchemaExport>> {
    let mut result = Vec::new();
    for item in table_items(sql)? {
        let parsed = parse(&format!("CREATE TABLE __schema_export ({item})"))?;
        let [Statement::CreateTable(table)] = parsed.as_slice() else {
            return None;
        };
        for column in &table.columns {
            let name = column_text(column, item)?.name;
            for constraint in &column.constraints {
                let kind = match constraint {
                    ColumnConstraint::NotNull
                    | ColumnConstraint::Null
                    | ColumnConstraint::Default(_) => continue,
                    ColumnConstraint::PrimaryKey => ConstraintKind::PrimaryKey,
                    ColumnConstraint::Unique => ConstraintKind::Unique,
                    ColumnConstraint::Check(_) => ConstraintKind::Check,
                    ColumnConstraint::References { .. } => ConstraintKind::ForeignKey,
                };
                result.push(ConstraintSchemaExport {
                    kind,
                    column: Some(name.clone()),
                    declaration_sql: item.to_owned(),
                });
            }
        }
        for constraint in &table.constraints {
            let kind = match constraint {
                TableConstraint::PrimaryKey { .. } => ConstraintKind::PrimaryKey,
                TableConstraint::Unique { .. } => ConstraintKind::Unique,
                TableConstraint::Check(_) => ConstraintKind::Check,
                TableConstraint::ForeignKey { .. } => ConstraintKind::ForeignKey,
            };
            result.push(ConstraintSchemaExport {
                kind,
                column: None,
                declaration_sql: item.to_owned(),
            });
        }
    }
    Some(result)
}

pub(super) fn table_items(sql: &str) -> Option<Vec<&str>> {
    let tokens = tokens(sql)?;
    let open = tokens.iter().position(|t| t.token == Token::LParen)?;
    let mut start = open + 1;
    let mut depth = 0;
    let mut items = Vec::new();
    for (pos, token) in tokens.iter().enumerate().skip(start) {
        match token.token {
            Token::LParen | Token::LBracket => depth += 1,
            Token::RParen if depth == 0 => {
                if start < pos {
                    items.push(slice(sql, &tokens[start..pos])?);
                }
                return Some(items);
            }
            Token::RParen | Token::RBracket => depth -= 1,
            Token::Comma if depth == 0 => {
                items.push(slice(sql, &tokens[start..pos])?);
                start = pos + 1;
            }
            _ => {}
        }
    }
    None
}

fn column_text(column: &ColumnDef, sql: &str) -> Option<ColumnText> {
    let tokens = tokens(sql)?;
    let name = match &tokens.first()?.token {
        Token::QuotedIdent(name) => name.clone(),
        _ => column.name.to_lowercase(),
    };
    let mut source_type = None;
    for end in 2..=tokens.len() {
        let raw = slice(sql, &tokens[1..end])?;
        if type_matches(raw, column) {
            source_type = Some(raw.to_owned());
            break;
        }
    }
    let default = column.constraints.iter().find_map(|c| match c {
        ColumnConstraint::Default(expr) => Some(expr),
        _ => None,
    });
    let default_expr_raw = match default {
        None => None,
        Some(expected) => Some(default_text(sql, &tokens, expected)?.to_owned()),
    };
    Some(ColumnText {
        name,
        source_type: source_type?,
        default_expr_raw,
    })
}

fn type_matches(raw: &str, column: &ColumnDef) -> bool {
    let Some(parsed) = parse(&format!("CREATE TABLE __schema_export (c {raw})")) else {
        return false;
    };
    let [Statement::CreateTable(table)] = parsed.as_slice() else {
        return false;
    };
    let [found] = table.columns.as_slice() else {
        return false;
    };
    let mut expected = column.clone();
    expected.name = "c".into();
    expected.constraints.clear();
    expected.comment = None;
    expected.compress_mode = None;
    expected.charset = None;
    expected.collate = None;
    expected.on_update = None;
    expected.generated = None;
    expected.encrypted_with = None;
    found == &expected
}

fn default_text<'a>(sql: &'a str, tokens: &[TokenWithSpan], expected: &Expr) -> Option<&'a str> {
    let start = tokens
        .iter()
        .position(|t| t.token == Token::Keyword(ogsql_parser::token::Keyword::DEFAULT))?
        + 1;
    for end in start + 1..=tokens.len() {
        let raw = slice(sql, &tokens[start..end])?;
        let Some(parsed) = parse(&format!("SELECT {raw}")) else {
            continue;
        };
        let [Statement::Select(select)] = parsed.as_slice() else {
            continue;
        };
        let expected = SelectStatement {
            targets: vec![SelectTarget::Expr(expected.clone(), None)],
            ..Default::default()
        };
        let mut found = select.node.clone();
        found.raw_body = None;
        if found == expected {
            return Some(raw);
        }
    }
    None
}

pub(super) fn parse(sql: &str) -> Option<Vec<Statement>> {
    let mut parser = Parser::with_source(Tokenizer::new(sql).tokenize().ok()?, sql.to_owned());
    let statements = parser.parse_with_text();
    if !parser.errors().is_empty() {
        return None;
    }
    Some(
        statements
            .into_iter()
            .map(|info| info.statement)
            .filter(|s| !matches!(s, Statement::Empty))
            .collect(),
    )
}

fn tokens(sql: &str) -> Option<Vec<TokenWithSpan>> {
    Some(
        Tokenizer::new(sql)
            .tokenize()
            .ok()?
            .into_iter()
            .filter(|t| !matches!(t.token, Token::Eof | Token::Comment(_)))
            .collect(),
    )
}

fn slice<'a>(sql: &'a str, tokens: &[TokenWithSpan]) -> Option<&'a str> {
    sql.get(tokens.first()?.span.start..tokens.last()?.span.end)
}
