mod backend;
mod documents;
mod syntax;

use anyhow::Result;
use std::{env, fs, path::PathBuf, process::ExitCode};
use tower_lsp::lsp_types::DiagnosticSeverity;
use tower_lsp::{LspService, Server};
use tracing_subscriber::EnvFilter;

use crate::backend::Backend;

#[tokio::main]
async fn main() -> Result<ExitCode> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("proverif_lsp_rs=info,warn")),
        )
        .with_writer(std::io::stderr)
        .init();

    if let Some(exit) = run_cli_mode()? {
        return Ok(exit);
    }

    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) = LspService::new(Backend::new);
    Server::new(stdin, stdout, socket).serve(service).await;
    Ok(ExitCode::SUCCESS)
}

fn run_cli_mode() -> Result<Option<ExitCode>> {
    let mut args = env::args_os().skip(1);
    let Some(first) = args.next() else {
        return Ok(None);
    };

    if first != "--check" {
        return Ok(None);
    }

    let path = PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow::anyhow!("missing file path after --check"))?,
    );
    let json = matches!(args.next().as_deref(), Some(flag) if flag == "--json");
    if args.next().is_some() {
        anyhow::bail!("unexpected extra arguments");
    }

    let source = fs::read_to_string(&path)?;
    let parsed = syntax::parse(&source)?;
    let diagnostics = parsed.diagnostics();

    if json {
        print_json(&path, &diagnostics);
    } else {
        print_human(&path, &diagnostics);
    }

    Ok(Some(if diagnostics.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }))
}

fn print_human(path: &PathBuf, diagnostics: &[tower_lsp::lsp_types::Diagnostic]) {
    if diagnostics.is_empty() {
        println!("{}: OK", path.display());
        return;
    }

    for diagnostic in diagnostics {
        let severity = match diagnostic.severity {
            Some(DiagnosticSeverity::ERROR) => "error",
            Some(DiagnosticSeverity::WARNING) => "warning",
            Some(DiagnosticSeverity::INFORMATION) => "info",
            Some(DiagnosticSeverity::HINT) => "hint",
            _ => "unknown",
        };
        let start = diagnostic.range.start;
        let end = diagnostic.range.end;
        println!(
            "{}:{}:{}-{}:{}: {}: {}",
            path.display(),
            start.line + 1,
            start.character + 1,
            end.line + 1,
            end.character + 1,
            severity,
            diagnostic.message
        );
    }
}

fn print_json(path: &PathBuf, diagnostics: &[tower_lsp::lsp_types::Diagnostic]) {
    println!("[");
    for (idx, diagnostic) in diagnostics.iter().enumerate() {
        let comma = if idx + 1 == diagnostics.len() { "" } else { "," };
        println!(
            "  {{\"path\":\"{}\",\"message\":{:?},\"line\":{},\"column\":{},\"end_line\":{},\"end_column\":{}}}{}",
            escape_json_string(&path.display().to_string()),
            diagnostic.message,
            diagnostic.range.start.line + 1,
            diagnostic.range.start.character + 1,
            diagnostic.range.end.line + 1,
            diagnostic.range.end.character + 1,
            comma
        );
    }
    println!("]");
}

fn escape_json_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}
