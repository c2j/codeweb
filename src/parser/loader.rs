use crate::error::{CodeWebError, Result};
use ogsql_parser::{StatementInfo, Tokenizer};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

pub struct ParsedFile {
    pub path: PathBuf,
    pub statements: Vec<StatementInfo>,
}

pub struct AllParsedFiles {
    pub sql_files: Vec<ParsedFile>,
    pub java_count: usize,
    pub ibatis_count: usize,
}

pub fn load_all_files(input: &Path) -> Result<AllParsedFiles> {
    let sql_files = load_sql_files_inner(input);
    let java_files = crate::parser::java_loader::load_java_files(input);
    let ibatis_files = crate::parser::ibatis_loader::load_ibatis_files(input);

    let total = sql_files.len() + java_files.len() + ibatis_files.len();
    if total == 0 {
        return Err(CodeWebError::NoFilesFound {
            path: input.to_path_buf(),
        });
    }

    Ok(AllParsedFiles {
        sql_files,
        java_count: java_files.len(),
        ibatis_count: ibatis_files.len(),
    })
}

pub fn load_sql_files(input: &Path) -> Result<Vec<ParsedFile>> {
    let files = load_sql_files_inner(input);
    if files.is_empty() {
        return Err(CodeWebError::NoFilesFound {
            path: input.to_path_buf(),
        });
    }
    Ok(files)
}

fn load_sql_files_inner(input: &Path) -> Vec<ParsedFile> {
    let sql_files = collect_files_by_ext(input, "sql");
    let mut parsed = Vec::new();

    for path in sql_files {
        match parse_file(&path) {
            Ok(stmts) => parsed.push(ParsedFile {
                path,
                statements: stmts,
            }),
            Err(e) => {
                eprintln!("warning: skipping {}: {}", path.display(), e);
            }
        }
    }

    parsed
}

fn collect_files_by_ext(input: &Path, ext: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if input.is_file() {
        if input.extension().is_some_and(|e| e == ext) {
            files.push(input.to_path_buf());
        }
    } else {
        for entry in WalkDir::new(input).into_iter().filter_map(|e| e.ok()) {
            let path = entry.into_path();
            if path.extension().is_some_and(|e| e == ext) {
                files.push(path);
            }
        }
    }
    files
}

fn parse_file(path: &Path) -> std::result::Result<Vec<StatementInfo>, String> {
    let sql = std::fs::read_to_string(path).map_err(|e| format!("read error: {}", e))?;

    let tokens = Tokenizer::new(&sql)
        .tokenize()
        .map_err(|e| format!("tokenize error: {}", e))?;

    let mut parser = ogsql_parser::Parser::with_source(tokens, sql);
    let stmts = parser.parse_with_text();

    if !parser.errors().is_empty() {
        let first_err = &parser.errors()[0];
        eprintln!(
            "warning: parse errors in {}: {} ({} total errors)",
            path.display(),
            first_err,
            parser.errors().len()
        );
    }

    Ok(stmts)
}
