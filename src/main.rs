// The `typstuml` binary is native-only and requires the `cli`
// feature — it's the CLI front-end and orchestrates a real Typst compile.
// The target's `required-features` declaration keeps it out of library-only
// builds; the fallback below only matters when building all targets on wasm.
#[cfg(all(not(target_arch = "wasm32"), feature = "cli"))]
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

#[cfg(any(target_arch = "wasm32", not(feature = "cli")))]
fn main() {}
