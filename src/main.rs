// The `typstuml` binary is native-only — it's the CLI front-end. The library
// crate still builds for wasm32 (see `lib.rs`), but the bin target has no
// meaning there, so it degrades to an empty `main`.
#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    use std::process::ExitCode;
    match typstuml::cli::run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("typstuml: {err}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn main() {}
