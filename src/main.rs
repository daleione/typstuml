// The `typstuml` binary is native-only AND requires the `embed-typst`
// feature — it's the CLI front-end and orchestrates a real Typst compile.
// The library crate still builds for wasm32 (see `lib.rs`) and without the
// feature, but the bin target has no meaning in those configurations, so
// it degrades to an empty `main`.
#[cfg(all(not(target_arch = "wasm32"), feature = "embed-typst"))]
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

#[cfg(any(target_arch = "wasm32", not(feature = "embed-typst")))]
fn main() {}
