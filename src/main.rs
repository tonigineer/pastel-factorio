//! Command line entry point:  pastel <command>  (run from the project folder).
use std::process::ExitCode;

use pastel::cli::Invocation;

fn main() -> ExitCode {
    match Invocation::from_env().run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e:#}");
            ExitCode::FAILURE
        }
    }
}
