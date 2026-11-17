//! 严格扫描与解析配置 SQL 源文件，不忽略 I/O 或解析错误。

use crate::error::{CodeWebError, Result};
use crate::project::Project;
use globset::{Glob, GlobSetBuilder};
use ogsql_parser::{Parser, StatementInfo, Tokenizer};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub(super) fn files(project: &Project) -> Result<BTreeSet<PathBuf>> {
    let config = &project.config().analysis;
    let mut builder = GlobSetBuilder::new();
    for pattern in &config.exclude {
        builder.add(Glob::new(pattern).map_err(|error| error_at(project.root(), error))?);
    }
    let exclude = builder
        .build()
        .map_err(|error| error_at(project.root(), error))?;
    let roots = if config.paths.is_empty() {
        vec![project.root().to_path_buf()]
    } else {
        config
            .paths
            .iter()
            .map(|p| project.root().join(p))
            .collect()
    };
    let mut files = BTreeSet::new();
    for root in roots {
        let walker = walkdir::WalkDir::new(&root)
            .into_iter()
            .filter_entry(|entry| {
                let relative = entry.path().strip_prefix(&root).unwrap_or(entry.path());
                if exclude.is_match(relative) {
                    return false;
                }
                if entry.file_type().is_dir() {
                    let mut with_slash = relative.as_os_str().to_os_string();
                    with_slash.push("/");
                    if exclude.is_match(Path::new(&with_slash)) {
                        return false;
                    }
                }
                true
            });
        for entry in walker {
            let entry = entry.map_err(|error| error_at(&root, error))?;
            let relative = entry.path().strip_prefix(&root).unwrap_or(entry.path());
            if exclude.is_match(relative) {
                continue;
            }
            if entry.file_type().is_symlink() {
                return Err(error_at(
                    entry.path(),
                    "symbolic source links are not proof-complete",
                ));
            }
            if entry.file_type().is_file()
                && entry.path().extension().is_some_and(|ext| ext == "sql")
            {
                files.insert(std::fs::canonicalize(entry.path()).map_err(|source| {
                    CodeWebError::FileRead {
                        path: entry.path().to_path_buf(),
                        source,
                    }
                })?);
            }
        }
    }
    Ok(files)
}

pub(super) fn parse_file(path: &Path) -> Result<(Vec<StatementInfo>, Vec<String>)> {
    let bytes = std::fs::read(path).map_err(|source| CodeWebError::FileRead {
        path: path.to_path_buf(),
        source,
    })?;
    let sql = match String::from_utf8(bytes) {
        Ok(sql) => sql,
        Err(error) => encoding_rs::GBK
            .decode_without_bom_handling_and_without_replacement(error.as_bytes())
            .ok_or_else(|| error_at(path, "invalid UTF-8/GBK SQL source"))?
            .into_owned(),
    };
    let tokens = match Tokenizer::new(&sql).tokenize() {
        Ok(tokens) => tokens,
        Err(error) => return Ok((Vec::new(), vec![format!("{}: {error}", path.display())])),
    };
    let mut parser = Parser::with_source(tokens, sql.clone());
    let mut statements = parser.parse_with_text();
    let mut diagnostics: Vec<_> = parser
        .errors()
        .iter()
        .map(|error| format!("{}: {error}", path.display()))
        .collect();
    for info in &mut statements {
        let start = offset(&sql, info.start_line, info.start_col);
        let end = offset(&sql, info.end_line, info.end_col);
        match start.zip(end).and_then(|(start, end)| sql.get(start..end)) {
            Some(raw) => info.sql_text = raw.to_owned(),
            None => diagnostics.push(format!(
                "{}: invalid statement source range {}:{}..{}:{}",
                path.display(),
                info.start_line,
                info.start_col,
                info.end_line,
                info.end_col
            )),
        }
    }
    Ok((statements, diagnostics))
}

// 公开解析位置的 column 按 UTF-8 字节计数，而非 Unicode 字符数。
fn offset(sql: &str, line: usize, column: usize) -> Option<usize> {
    let start = if line == 1 {
        0
    } else {
        sql.match_indices('\n')
            .nth(line.checked_sub(2)?)
            .map(|(index, _)| index + 1)?
    };
    let end = sql
        .get(start..)?
        .find('\n')
        .map_or(sql.len(), |index| start + index);
    let offset = start.checked_add(column.checked_sub(1)?)?;
    (offset <= end && sql.is_char_boundary(offset)).then_some(offset)
}

fn error_at(path: &Path, error: impl std::fmt::Display) -> CodeWebError {
    CodeWebError::ExportError {
        message: format!("schema export {}: {error}", path.display()),
    }
}
