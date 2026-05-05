use std::process::ExitCode;

fn main() -> ExitCode {
    match typstuml::cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("typstuml: {err}");
            ExitCode::FAILURE
        }
    }
}
