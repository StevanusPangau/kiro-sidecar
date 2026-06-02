mod cli;
mod config;
mod events;
mod git_utils;
mod kiro;
mod parallel;
mod paths;
mod profiles;
mod task_schema;
mod writer;

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    match cli::run().await {
        Ok(code) => exit_code(code),
        Err(error) => {
            eprintln!("kiro-sidecar: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn exit_code(code: i32) -> ExitCode {
    match u8::try_from(code) {
        Ok(value) => ExitCode::from(value),
        Err(_) => ExitCode::from(1),
    }
}
