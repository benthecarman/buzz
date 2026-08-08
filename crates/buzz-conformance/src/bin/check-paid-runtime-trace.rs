use std::{io::Read, process::ExitCode};

use buzz_conformance::paid_agent_runtime::{check_runtime_jsonl, RuntimeCheckerConfig};

fn main() -> ExitCode {
    let mut jsonl = String::new();
    if let Err(error) = std::io::stdin().read_to_string(&mut jsonl) {
        eprintln!("read paid-runtime trace: {error}");
        return ExitCode::FAILURE;
    }
    match check_runtime_jsonl(&jsonl, &RuntimeCheckerConfig::default()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
