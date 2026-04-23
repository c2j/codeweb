mod error;
mod export;
mod graph;
mod parser;

use std::io::Write;
use std::path::PathBuf;

use clap::Parser;
use error::Result;
use graph::builder::GraphBuilder;
use graph::{CodeGraph, Node};

#[derive(Parser)]
#[command(
    name = "codeweb",
    version,
    about = "Semantic code graph analyzer for SQL stored procedures"
)]
struct Cli {
    /// Input file or directory containing SQL, Java, and XML files
    input: PathBuf,

    /// Output format
    #[arg(long, default_value = "dot", value_parser = ["dot", "json", "mermaid"])]
    format: String,

    /// Output file (stdout if omitted)
    #[arg(long)]
    output: Option<PathBuf>,

    /// Include unresolved/dynamic call nodes
    #[arg(long)]
    include_unresolved: bool,

    /// Only scan SQL files (ignore Java and XML)
    #[arg(long)]
    sql_only: bool,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {}", e);
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();

    let graph = if cli.sql_only {
        let files = parser::load_sql_files(&cli.input)?;
        eprintln!("loaded {} SQL file(s)", files.len());
        let builder = GraphBuilder::new();
        builder.build(&files)
    } else {
        let all = parser::load_all_files(&cli.input)?;
        eprintln!(
            "loaded {} SQL, {} Java, {} XML file(s)",
            all.sql_files.len(),
            all.java_count,
            all.ibatis_count
        );
        let builder = GraphBuilder::new();
        builder.build_all(&all, &cli.input)
    };

    print_stats(&graph, cli.include_unresolved);

    let output = match cli.format.as_str() {
        "dot" => export::dot::to_dot(&graph),
        "json" => export::json::to_json(&graph)?,
        "mermaid" => export::mermaid::to_mermaid(&graph),
        _ => unreachable!(),
    };

    match cli.output {
        Some(path) => {
            std::fs::write(&path, &output).map_err(|source| error::CodeWebError::FileRead {
                path: path.clone(),
                source,
            })?;
        }
        None => {
            std::io::stdout()
                .write_all(output.as_bytes())
                .map_err(|source| error::CodeWebError::ExportError {
                    message: source.to_string(),
                })?;
        }
    }

    Ok(())
}

fn print_stats(graph: &CodeGraph, include_unresolved: bool) {
    let mut procedures = 0usize;
    let mut unresolved = 0usize;
    let mut mappers = 0usize;
    let mut java_sql = 0usize;
    let mut java_methods = 0usize;
    let mut java_classes = 0usize;

    for idx in graph.node_indices() {
        match &graph[idx] {
            Node::Procedure { .. } => procedures += 1,
            Node::Unresolved { .. } => unresolved += 1,
            Node::MappedStatement { .. } => mappers += 1,
            Node::JavaSql { .. } => java_sql += 1,
            Node::JavaMethod { .. } => java_methods += 1,
            Node::JavaClass { .. } => java_classes += 1,
        }
    }

    let edges = graph.edge_count();

    if include_unresolved {
        eprintln!(
            "graph: {} procedures, {} mappers, {} java-sql, {} java-methods, {} java-classes, {} unresolved, {} edges",
            procedures, mappers, java_sql, java_methods, java_classes, unresolved, edges
        );
    } else {
        eprintln!(
            "graph: {} procedures, {} mappers, {} java-sql, {} java-methods, {} java-classes, {} edges",
            procedures, mappers, java_sql, java_methods, java_classes, edges
        );
    }
}
