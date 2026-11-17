//! 独立于图存储的静态表结构投影。

use super::schema_source as source;
use super::schema_text as text;

use crate::error::Result;
use crate::graph::format::format_data_type;
use crate::project::Project;
use ogsql_parser::ast::{
    ColumnConstraint, CreateIndexStatement, CreateTableStatement, DataType, ObjectName, Statement,
    TableConstraint,
};

/// 配置 SQL 源文件的结构快照；诊断存在时所有表均标记不完备。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaExport {
    /// 源文件顺序确定的显式表定义。
    pub tables: Vec<TableSchemaExport>,
    /// 无法证明完整的源语句或解析错误。
    pub diagnostics: Vec<String>,
}

/// 表结构；不完备的定义不得用于静态安全证明。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableSchemaExport {
    /// 已折叠未引用标识符大小写的限定名分量。
    pub name: Vec<String>,
    /// 按声明顺序排列的列。
    pub columns: Vec<ColumnSchemaExport>,
    /// 内联及表级约束，原始声明保留名称、引用动作及延迟属性。
    pub constraints: Vec<ConstraintSchemaExport>,
    /// 独立 CREATE INDEX 的完整定义，包含表达式与部分索引谓词。
    pub indexes: Vec<IndexSchemaExport>,
    /// 是否已完整枚举配置源文件中的独立索引。
    pub indexes_complete: bool,
    /// 是否存在未建模或有歧义的结构语义。
    pub has_opaque_ddl: bool,
}

/// 列的声明元数据，不代表数据库内核规范化后的类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ColumnSchemaExport {
    /// 列名。
    pub name: String,
    /// 保留声明精度及长度的类型。
    pub source_type: String,
    /// 是否允许 NULL。
    pub nullable: bool,
    /// 默认表达式原文；None 表示无 DEFAULT，完备性仍由表标志约束。
    pub default_expr_raw: Option<String>,
}

/// 表数据约束类别；NULL 和 DEFAULT 分别存储在列字段中。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintKind {
    /// 主键。
    PrimaryKey,
    /// 唯一约束。
    Unique,
    /// 检查表达式。
    Check,
    /// 外键引用。
    ForeignKey,
}

/// 原始约束声明，避免将未建模的约束选项静默丢失。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConstraintSchemaExport {
    /// 约束类型。
    pub kind: ConstraintKind,
    /// 内联声明所在列；表级约束为 None。
    pub column: Option<String>,
    /// CREATE TABLE 列表中的完整元素（列声明或表约束）。
    pub declaration_sql: String,
}

/// 独立索引；表达式、排序、NULL 顺序、方法与谓词保留在原始 DDL 中。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexSchemaExport {
    /// 显式索引名；未命名为 None。
    pub name: Option<Vec<String>>,
    /// 是否要求唯一性。
    pub unique: bool,
    /// 完整 CREATE INDEX 原文，可由消费者重新解析。
    pub definition_sql: String,
}

pub(super) fn export(project: &Project) -> Result<SchemaExport> {
    let mut export = SchemaExport {
        tables: Vec::new(),
        diagnostics: Vec::new(),
    };
    let mut indexes = Vec::new();
    for file in source::files(project)? {
        let (statements, diagnostics) = source::parse_file(&file)?;
        export.diagnostics.extend(diagnostics);
        for info in statements {
            match info.statement {
                Statement::CreateTable(table) => {
                    let table = table_schema(&table, &info.sql_text);
                    if table.has_opaque_ddl {
                        export.diagnostics.push(format!(
                            "{}: unmodeled table definition {}",
                            file.display(),
                            table.name.join(".")
                        ));
                    }
                    export.tables.push(table);
                }
                Statement::CreateIndex(index) => indexes.push((index.node, info.sql_text)),
                Statement::Empty
                | Statement::Comment(_)
                | Statement::CreateProcedure(_)
                | Statement::CreateFunction(_) => {}
                // 其余语句尚未证明不改变结构，不能当成「没有索引」。
                _ => export
                    .diagnostics
                    .push(format!("{}: unmodeled schema statement", file.display())),
            }
        }
    }
    for (index, sql) in indexes {
        attach_index(&mut export, &index, &sql);
    }
    reject_duplicates(&mut export);
    if !export.diagnostics.is_empty() {
        for table in &mut export.tables {
            table.indexes_complete = false;
            table.has_opaque_ddl = true;
        }
    }
    Ok(export)
}

fn reject_duplicates(export: &mut SchemaExport) {
    let mut tables = std::collections::BTreeSet::new();
    let mut indexes = std::collections::BTreeSet::new();
    for table in &export.tables {
        let mut columns = std::collections::BTreeSet::new();
        if !tables.insert(&table.name)
            || table.columns.iter().any(|c| !columns.insert(&c.name))
            || table
                .indexes
                .iter()
                .filter_map(|index| index.name.as_ref())
                .any(|name| !indexes.insert(name))
        {
            export.diagnostics.push(format!(
                "duplicate schema definition: {}",
                table.name.join(".")
            ));
        }
    }
}

fn attach_index(export: &mut SchemaExport, index: &CreateIndexStatement, sql: &str) {
    let name = object_name(&index.table);
    let matches: Vec<_> = export
        .tables
        .iter_mut()
        .filter(|table| table.name == name)
        .collect();
    let raw_matches = text::parse(sql).is_some_and(|parsed| matches!(parsed.as_slice(), [Statement::CreateIndex(found)] if found.node == *index));
    if matches.len() != 1 || !raw_matches {
        export.diagnostics.push(format!(
            "unresolved or ambiguous index on {}",
            name.join(".")
        ));
        return;
    }
    for table in matches {
        table.has_opaque_ddl |= index.if_not_exists || index.concurrent;
        table.indexes.push(IndexSchemaExport {
            name: index.name.as_ref().map(object_name),
            unique: index.unique,
            definition_sql: sql.trim().to_owned(),
        });
    }
}

fn object_name(name: &ObjectName) -> Vec<String> {
    name.iter()
        .map(|id| {
            if id.quote_style.is_some() {
                id.value.clone()
            } else {
                id.value.to_lowercase()
            }
        })
        .collect()
}

fn table_schema(table: &CreateTableStatement, sql: &str) -> TableSchemaExport {
    let raw = text::columns(table, sql);
    let constraints = text::constraints(sql);
    let mut opaque = raw.is_none()
        || constraints.is_none()
        || table.temporary
        || table.unlogged
        || table.if_not_exists
        || !table.like_clauses.is_empty()
        || !table.inherits.is_empty()
        || table.partition_by.is_some()
        || table.subpartition_by.is_some()
        || table.subpartitions_count.is_some()
        || table.distribute_by.is_some()
        || table.to_group.is_some()
        || table.tablespace.is_some()
        || table.on_commit.is_some()
        || !table.options.is_empty()
        || !table.table_options.is_empty()
        || table.compress.is_some()
        || table.ilm.is_some()
        || table.row_movement.is_some();
    let columns = table.columns.iter().enumerate().map(|(index, column)| {
        let raw = raw.as_ref().and_then(|columns| columns.get(index));
        opaque |= column.compress_mode.is_some() || column.charset.is_some()
            || column.collate.is_some() || column.on_update.is_some()
            || column.generated.is_some() || column.encrypted_with.is_some()
            || matches!(column.data_type, DataType::Serial | DataType::SmallSerial | DataType::BigSerial);
        ColumnSchemaExport {
            name: raw.map_or_else(|| column.name.clone(), |raw| raw.name.clone()),
            source_type: raw.map_or_else(|| format_data_type(&column.data_type), |raw| raw.source_type.clone()),
            nullable: !column.constraints.contains(&ColumnConstraint::NotNull)
                && !column.constraints.contains(&ColumnConstraint::PrimaryKey)
                && !table.constraints.iter().any(|c| matches!(c, TableConstraint::PrimaryKey { columns, .. } if columns.contains(&column.name))),
            default_expr_raw: raw.and_then(|raw| raw.default_expr_raw.clone()),
        }
    }).collect();
    TableSchemaExport {
        name: object_name(&table.name),
        columns,
        constraints: constraints.unwrap_or_default(),
        indexes: Vec::new(),
        indexes_complete: true,
        has_opaque_ddl: opaque,
    }
}
