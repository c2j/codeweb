use crate::error::{CodeWebError, Result};
use ogsql_parser::{StatementInfo, Tokenizer};
use std::path::{Path, PathBuf};

pub struct ParsedFile {
    pub path: PathBuf,
    pub statements: Vec<StatementInfo>,
}

pub struct AllParsedFiles {
    pub sql_files: Vec<ParsedFile>,
    pub java_files: Vec<crate::parser::java_loader::JavaParsedFile>,
    pub ibatis_files: Vec<crate::parser::ibatis_loader::IbatisParsedFile>,
    pub java_method_results: Vec<crate::parser::java_method::JavaParseResult>,
}

pub fn load_all_files(input: &Path) -> Result<AllParsedFiles> {
    let scanned = crate::parser::scanner::scan_directory(input);

    if scanned.sql_files.is_empty() && scanned.java_files.is_empty() && scanned.xml_files.is_empty()
    {
        return Err(CodeWebError::NoFilesFound {
            path: input.to_path_buf(),
        });
    }

    let sql_files = parse_sql_files(&scanned.sql_files);
    let java_files = crate::parser::java_loader::load_java_files_from_paths(&scanned.java_files);
    let ibatis_files =
        crate::parser::ibatis_loader::load_ibatis_files_from_paths(&scanned.xml_files);
    let java_method_results =
        crate::parser::java_method::parse_java_files_from_paths(&scanned.java_files);

    Ok(AllParsedFiles {
        sql_files,
        java_files,
        ibatis_files,
        java_method_results,
    })
}

pub fn load_sql_files(input: &Path) -> Result<Vec<ParsedFile>> {
    let scanned = crate::parser::scanner::scan_directory(input);
    if scanned.sql_files.is_empty() {
        return Err(CodeWebError::NoFilesFound {
            path: input.to_path_buf(),
        });
    }
    Ok(parse_sql_files(&scanned.sql_files))
}

pub fn parse_sql_files(paths: &[PathBuf]) -> Vec<ParsedFile> {
    use rayon::prelude::*;

    paths
        .par_iter()
        .filter_map(|path| {
            parse_file(path).ok().map(|statements| ParsedFile {
                path: path.clone(),
                statements,
            })
        })
        .collect()
}

fn parse_file(path: &Path) -> std::result::Result<Vec<StatementInfo>, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read error: {}", e))?;
    let sql = String::from_utf8(bytes)
        .unwrap_or_else(|e| String::from_utf8_lossy(e.as_bytes()).into_owned());

    let tokens = Tokenizer::new(&sql)
        .tokenize()
        .map_err(|e| format!("tokenize error: {}", e))?;

    let mut parser = ogsql_parser::Parser::with_source(tokens, sql);
    let stmts = parser.parse_with_text();

    let file_str = path.to_string_lossy().to_string();
    if !parser.errors().is_empty() {
        for err in parser.errors() {
            crate::parse_log::warn(&file_str, &err.to_string());
        }
    }
    crate::parse_log::info(&file_str, &format!("{} statements parsed", stmts.len()));

    Ok(stmts)
}
