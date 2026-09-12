use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "florui", about = "Florui project CLI")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Generate a minimal compiling application.
    New { name: String },
    /// Launch the native preview host and watch a CSS fixture for changes.
    Dev {
        #[arg(long, default_value = "fixtures/dev/app.css")]
        fixture: PathBuf,
    },
    /// Run component, lifecycle, geometry, and visual test suites.
    Test,
    /// Produce or open reference comparison artifacts.
    Compare,
    /// Produce a release/debug artifact for a supported target.
    Build {
        #[arg(long, default_value = "native")]
        target: String,
    },
}

fn main() -> ExitCode {
    match Cli::parse().command {
        Command::Dev { fixture } => run_dev(fixture),
        Command::New { name } => not_implemented(&format!("`florui new {name}`")),
        Command::Test => not_implemented("`florui test`"),
        Command::Compare => not_implemented("`florui compare`"),
        Command::Build { target } => not_implemented(&format!("`florui build --target {target}`")),
    }
}

fn run_dev(fixture: PathBuf) -> ExitCode {
    if !fixture.exists() {
        eprintln!("fixture not found: {}", fixture.display());
        return ExitCode::FAILURE;
    }
    match florui_devtools::preview::run(fixture) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}

fn not_implemented(command: &str) -> ExitCode {
    eprintln!("{command} is not implemented yet.");
    ExitCode::FAILURE
}
