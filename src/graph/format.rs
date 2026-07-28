use ogsql_parser::ast::{DataType, Expr, Ident, Literal, ObjectName, TimeZoneInfo};

/// Converts an ogsql-parser `DataType` to its human-readable SQL type string.
pub fn format_data_type(dt: &DataType) -> String {
    match dt {
        DataType::Boolean => "BOOLEAN".to_string(),
        DataType::TinyInt(n) => match n {
            Some(len) => format!("TINYINT({})", len),
            None => "TINYINT".to_string(),
        },
        DataType::SmallInt(n) => match n {
            Some(len) => format!("SMALLINT({})", len),
            None => "SMALLINT".to_string(),
        },
        DataType::Integer(n) => match n {
            Some(len) => format!("INTEGER({})", len),
            None => "INTEGER".to_string(),
        },
        DataType::BigInt(n) => match n {
            Some(len) => format!("BIGINT({})", len),
            None => "BIGINT".to_string(),
        },
        DataType::Real => "REAL".to_string(),
        DataType::Float(n) => match n {
            Some(prec) => format!("FLOAT({})", prec),
            None => "FLOAT".to_string(),
        },
        DataType::Double => "DOUBLE PRECISION".to_string(),
        DataType::Numeric(p, s) => match (p, s) {
            (Some(p), Some(s)) => format!("NUMERIC({},{})", p, s),
            (Some(p), None) => format!("NUMERIC({})", p),
            (None, _) => "NUMERIC".to_string(),
        },
        DataType::Char(n) => match n {
            Some(len) => format!("CHAR({})", len),
            None => "CHAR".to_string(),
        },
        DataType::Varchar(n) => match n {
            Some(len) => format!("VARCHAR({})", len),
            None => "VARCHAR".to_string(),
        },
        DataType::Text => "TEXT".to_string(),
        DataType::Bytea => "BYTEA".to_string(),
        DataType::Timestamp(p, tz) => format_timestamp(p, tz),
        DataType::Timestamptz(p) => match p {
            Some(prec) => format!("TIMESTAMPTZ({})", prec),
            None => "TIMESTAMPTZ".to_string(),
        },
        DataType::Date => "DATE".to_string(),
        DataType::Time(p, tz) => format_time(p, tz),
        DataType::Interval(it) => match it {
            Some(it) => format!("INTERVAL {:?}", it),
            None => "INTERVAL".to_string(),
        },
        DataType::Json => "JSON".to_string(),
        DataType::Jsonb => "JSONB".to_string(),
        DataType::Uuid => "UUID".to_string(),
        DataType::Bit(n) => match n {
            Some(len) => format!("BIT({})", len),
            None => "BIT".to_string(),
        },
        DataType::Varbit(n) => match n {
            Some(len) => format!("VARBIT({})", len),
            None => "VARBIT".to_string(),
        },
        DataType::Serial => "SERIAL".to_string(),
        DataType::SmallSerial => "SMALLSERIAL".to_string(),
        DataType::BigSerial => "BIGSERIAL".to_string(),
        DataType::BinaryFloat => "BINARY_FLOAT".to_string(),
        DataType::BinaryDouble => "BINARY_DOUBLE".to_string(),
        DataType::Array(inner) => format!("{}[]", format_data_type(inner)),
        DataType::Custom(name, params) => format_custom_type(name, params),
    }
}

fn format_timestamp(p: &Option<u32>, tz: &Option<TimeZoneInfo>) -> String {
    let base = match p {
        Some(prec) => format!("TIMESTAMP({})", prec),
        None => "TIMESTAMP".to_string(),
    };
    match tz {
        Some(TimeZoneInfo::WithTimeZone) => format!("{} WITH TIME ZONE", base),
        Some(TimeZoneInfo::WithoutTimeZone) => format!("{} WITHOUT TIME ZONE", base),
        None => base,
    }
}

fn format_time(p: &Option<u32>, tz: &Option<TimeZoneInfo>) -> String {
    let base = match p {
        Some(prec) => format!("TIME({})", prec),
        None => "TIME".to_string(),
    };
    match tz {
        Some(TimeZoneInfo::WithTimeZone) => format!("{} WITH TIME ZONE", base),
        Some(TimeZoneInfo::WithoutTimeZone) => format!("{} WITHOUT TIME ZONE", base),
        None => base,
    }
}

fn format_custom_type(name: &ObjectName, params: &[Expr]) -> String {
    let name_str = name
        .iter()
        .map(Ident::to_string)
        .collect::<Vec<_>>()
        .join(".");
    if params.is_empty() {
        name_str
    } else {
        let params_str = params.iter().map(format_expr).collect::<Vec<_>>().join(",");
        format!("{}({})", name_str, params_str)
    }
}

/// Converts an ogsql-parser `Expr` to a human-readable SQL expression string.
///
/// Handles the most common expression types. For complex or rare variants,
/// falls back to the Debug representation.
pub fn format_expr(expr: &Expr) -> String {
    match expr {
        Expr::Literal(lit) => format_literal(lit),
        Expr::TypeCast {
            expr: inner,
            type_name,
            ..
        } => format!("{}::{}", format_expr(inner), format_data_type(type_name)),
        Expr::FunctionCall { name, args, .. } => {
            let name_str = name
                .iter()
                .map(Ident::to_string)
                .collect::<Vec<_>>()
                .join(".");
            let args_str = args.iter().map(format_expr).collect::<Vec<_>>().join(", ");
            format!("{}({})", name_str, args_str)
        }
        Expr::UnaryOp { op, expr: inner } if op == "-" => {
            format!("-{}", format_expr(inner))
        }
        Expr::ColumnRef(parts) => parts
            .iter()
            .map(Ident::to_string)
            .collect::<Vec<_>>()
            .join("."),
        Expr::PlVariable(parts) => parts
            .iter()
            .map(Ident::to_string)
            .collect::<Vec<_>>()
            .join("."),
        Expr::Parameter(n) => format!("${}", n),
        Expr::MyBatisParam(s) => format!("#{{{}}}", s),
        Expr::MyBatisRawExpr(s) => s.clone(),
        Expr::JdbcParam => "?".to_string(),
        Expr::Array(items) => {
            let items_str = items.iter().map(format_expr).collect::<Vec<_>>().join(", ");
            format!("[{}]", items_str)
        }
        Expr::Parenthesized(inner) => format!("({})", format_expr(inner)),
        Expr::FieldAccess { object, field } => {
            format!("{}.{}", format_expr(object), field)
        }
        // Fallback for all other expression types
        _ => format!("{:?}", expr),
    }
}

fn format_literal(lit: &Literal) -> String {
    match lit {
        Literal::Integer(n) => n.to_string(),
        Literal::Float(s) => s.clone(),
        Literal::String(s) => format!("'{}'", s),
        Literal::EscapeString(s) => format!("E'{}'", s),
        Literal::Null => "NULL".to_string(),
        Literal::Boolean(b) => b.to_string(),
        Literal::BitString(s) => format!("B'{}'", s),
        Literal::HexString(s) => format!("X'{}'", s),
        // Fallback for less common literal types
        _ => format!("{:?}", lit),
    }
}
