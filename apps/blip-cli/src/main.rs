use std::process::ExitCode;

use clap::Parser;

mod cli;
mod commands;
mod recording;

fn main() -> ExitCode {
    match commands::run(cli::Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("blip-cli: {error}");
            ExitCode::FAILURE
        }
    }
}
