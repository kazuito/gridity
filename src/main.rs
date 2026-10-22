mod cli;
mod color;
mod compose;
mod detect;
mod error;
mod guide;
mod layout;
mod output;
mod probe;
mod render;
mod select;
mod source;
mod text;

use std::process::ExitCode;

use clap::Parser;
use serde_json::json;

use cli::{Cli, Command};
use error::{CliError, ErrorKind};

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return report_parse_error(err),
    };
    let json = match &cli.command {
        Command::Render(args) => args.json,
        Command::Probe(args) => args.json,
        Command::Guide(_) => false,
    };
    configure_threads(match &cli.command {
        Command::Render(args) => args.jobs,
        _ => None,
    });

    let result = match cli.command {
        Command::Render(args) => render::run(*args),
        Command::Probe(args) => probe::run(args),
        Command::Guide(args) => guide::run(args.topic.as_deref()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => report(&err, json),
    }
}

/// Argument errors from clap keep their usual stderr text; with --json they are also reported on stdout.
fn report_parse_error(err: clap::Error) -> ExitCode {
    use clap::error::ErrorKind as ClapKind;
    let is_info = matches!(err.kind(), ClapKind::DisplayHelp | ClapKind::DisplayVersion);
    let wants_json = std::env::args().any(|a| a == "--json");
    if !is_info && wants_json {
        let rendered = err.to_string();
        let message = rendered
            .lines()
            .next()
            .unwrap_or_default()
            .trim_start_matches("error: ");
        let body = json!({
            "error": {
                "kind": ErrorKind::Usage.name(),
                "code": ErrorKind::Usage.exit_code(),
                "message": message,
                "hint": "run with --help for usage",
            }
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&body).unwrap_or_default()
        );
    }
    let _ = err.print();
    ExitCode::from(if is_info {
        0
    } else {
        ErrorKind::Usage.exit_code() as u8
    })
}

fn configure_threads(jobs: Option<usize>) {
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let threads = jobs.unwrap_or(cores.min(8)).max(1);
    let _ = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global();
}

/// Prints the error to stderr (and as JSON on stdout with --json) and maps it to an exit code.
fn report(err: &anyhow::Error, json: bool) -> ExitCode {
    let cli_error = err.downcast_ref::<CliError>();
    let kind = cli_error.map(|e| e.kind).unwrap_or(ErrorKind::Runtime);
    let message = format!("{err:#}");
    let hint = cli_error.and_then(|e| e.hint.clone());

    eprintln!("error: {message}");
    if let Some(hint) = &hint {
        eprintln!("hint: {hint}");
    }
    if json {
        let body = json!({
            "error": { "kind": kind.name(), "code": kind.exit_code(), "message": message, "hint": hint }
        });
        println!(
            "{}",
            serde_json::to_string_pretty(&body).unwrap_or_default()
        );
    }
    ExitCode::from(kind.exit_code() as u8)
}
