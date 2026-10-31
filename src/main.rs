mod cli;
mod color;
mod compose;
mod detect;
mod error;
mod export;
mod guide;
mod kind;
mod layout;
mod mcp;
mod output;
mod pagetext;
mod probe;
mod render;
mod ruler;
mod select;
mod source;
mod template;
mod text;

use std::panic::AssertUnwindSafe;
use std::process::ExitCode;

use clap::Parser;

use cli::{Cli, Command};
use error::{CliError, ErrorKind, Reason, describe};
use output::{ErrorInfo, ErrorReport, SCHEMA_VERSION};

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => return report_parse_error(err),
    };
    let json = cli.command.json();
    configure_threads(cli.command.jobs());

    // A panic still ends with an exit code and, with --json, a JSON error.
    let result = std::panic::catch_unwind(AssertUnwindSafe(|| match cli.command {
        Command::Render(args) => render::run(*args),
        Command::Export(args) => export::run(*args),
        Command::Probe(args) => probe::run(args),
        Command::Guide(args) => guide::run(args.topic.as_deref()),
        Command::Mcp(args) => mcp::run(args),
        Command::Version => {
            println!("gridity {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
    }))
    .unwrap_or_else(|panic| Err(error::from_panic(&*panic).into()));
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
        print_json_error(
            &CliError::new(ErrorKind::Usage, Reason::InvalidArgument, message)
                .with_hint("run with --help for usage"),
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
    let error = describe(err);
    eprintln!("error: {}", error.message);
    if let Some(hint) = &error.hint {
        eprintln!("hint: {hint}");
    }
    if json {
        print_json_error(&error);
    }
    ExitCode::from(error.kind.exit_code() as u8)
}

pub fn error_report(error: &CliError) -> ErrorReport {
    ErrorReport {
        schema_version: SCHEMA_VERSION,
        error: ErrorInfo {
            kind: error.kind.name().to_string(),
            code: error.kind.exit_code(),
            reason: error.reason.name().to_string(),
            message: error.message.clone(),
            hint: error.hint.clone(),
        },
    }
}

fn print_json_error(error: &CliError) {
    println!(
        "{}",
        serde_json::to_string_pretty(&error_report(error)).unwrap_or_default()
    );
}
