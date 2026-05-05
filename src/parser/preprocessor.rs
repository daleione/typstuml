//! PlantUML preprocessor.
//!
//! Supports the safe-to-implement subset from design doc §11:
//!
//! - `!include`        file inlining (relative to source dir or any `--include` path)
//! - constant `!define NAME value` substituted at identifier word boundaries
//!
//! `!function`, `!procedure`, `!ifdef` and friends emit a diagnostic in
//! `Warn` / `Loose` and a hard error in `Strict`. Includes are cycle-checked
//! (canonicalized path set) so circular `!include` graphs surface as a parse
//! error instead of a stack overflow.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::diagnostics::{CompatMode, Diagnostic, Error, Level, Result};

#[derive(Clone, Debug, Default)]
pub struct Preprocessed {
    pub text: String,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub include_paths: Vec<PathBuf>,
    pub source_dir: Option<PathBuf>,
}

pub fn run(source: &str, compat: CompatMode) -> Result<Preprocessed> {
    run_with(source, compat, &Config::default())
}

pub fn run_with(source: &str, compat: CompatMode, config: &Config) -> Result<Preprocessed> {
    let mut state = State::default();
    let text = run_inner(source, compat, config, &mut state)?;
    Ok(Preprocessed {
        text,
        diagnostics: state.diagnostics,
    })
}

#[derive(Default)]
struct State {
    defines: HashMap<String, String>,
    in_progress: HashSet<PathBuf>,
    diagnostics: Vec<Diagnostic>,
}

fn run_inner(
    source: &str,
    compat: CompatMode,
    config: &Config,
    state: &mut State,
) -> Result<String> {
    let mut out = String::with_capacity(source.len());

    for (idx, line) in source.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = line.trim_start();

        if let Some(rest) = trimmed.strip_prefix("!define") {
            handle_define(rest, line_no, state);
            continue;
        }

        if let Some(rest) = trimmed.strip_prefix("!include") {
            handle_include(rest.trim(), line_no, compat, config, state, &mut out)?;
            continue;
        }

        if is_unsupported_directive(trimmed) {
            let head = trimmed.split_whitespace().next().unwrap_or("?");
            let msg = format!("unsupported preprocessor directive: {head:?}");
            if compat == CompatMode::Strict {
                return Err(Error::Parse {
                    line: line_no,
                    message: msg,
                });
            }
            state.diagnostics.push(Diagnostic {
                level: Level::Warning,
                line: Some(line_no),
                message: msg,
            });
            continue;
        }

        out.push_str(&substitute_defines(line, &state.defines));
        out.push('\n');
    }

    Ok(out)
}

fn handle_define(rest: &str, line_no: usize, state: &mut State) {
    let rest = rest.trim();
    match rest.split_once(char::is_whitespace) {
        Some((name, value)) => {
            state
                .defines
                .insert(name.trim().to_string(), value.trim().to_string());
        }
        None => {
            state.diagnostics.push(Diagnostic {
                level: Level::Warning,
                line: Some(line_no),
                message: format!("malformed !define: {rest:?}"),
            });
        }
    }
}

fn handle_include(
    raw: &str,
    line_no: usize,
    compat: CompatMode,
    config: &Config,
    state: &mut State,
    out: &mut String,
) -> Result<()> {
    let path = raw.trim().trim_matches('"');
    let Some(resolved) = resolve_include(path, config) else {
        let msg = format!("could not resolve !include {path:?}");
        if compat == CompatMode::Strict {
            return Err(Error::Parse {
                line: line_no,
                message: msg,
            });
        }
        state.diagnostics.push(Diagnostic {
            level: Level::Warning,
            line: Some(line_no),
            message: msg,
        });
        return Ok(());
    };

    let canonical = resolved.canonicalize().unwrap_or_else(|_| resolved.clone());
    if !state.in_progress.insert(canonical.clone()) {
        return Err(Error::Parse {
            line: line_no,
            message: format!("circular !include: {}", resolved.display()),
        });
    }

    let content = std::fs::read_to_string(&resolved).map_err(|e| Error::Io {
        path: resolved.clone(),
        source: e,
    })?;

    let nested_config = Config {
        source_dir: resolved.parent().map(Path::to_path_buf),
        include_paths: config.include_paths.clone(),
    };
    let nested = run_inner(&content, compat, &nested_config, state)?;
    out.push_str(&nested);
    if !out.ends_with('\n') {
        out.push('\n');
    }

    state.in_progress.remove(&canonical);
    Ok(())
}

fn is_unsupported_directive(trimmed: &str) -> bool {
    const TOKENS: &[&str] = &[
        "!function",
        "!procedure",
        "!ifdef",
        "!ifndef",
        "!if ",
        "!else",
        "!elseif",
        "!endif",
        "!while",
        "!endwhile",
    ];
    TOKENS.iter().any(|tok| trimmed.starts_with(tok))
}

fn resolve_include(path: &str, config: &Config) -> Option<PathBuf> {
    let p = PathBuf::from(path);
    if p.is_absolute() && p.exists() {
        return Some(p);
    }
    if let Some(dir) = &config.source_dir {
        let candidate = dir.join(&p);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    config
        .include_paths
        .iter()
        .map(|d| d.join(&p))
        .find(|c| c.exists())
}

/// Replace `!define`d names with their values, but only at identifier
/// boundaries (so `BAR` doesn't substitute inside `BARN`).
fn substitute_defines(line: &str, defines: &HashMap<String, String>) -> String {
    if defines.is_empty() {
        return line.to_string();
    }
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < bytes.len() {
        if !is_ident_start(bytes[i]) {
            out.push(bytes[i] as char);
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && is_ident_continue(bytes[i]) {
            i += 1;
        }
        let word = &line[start..i];
        match defines.get(word) {
            Some(value) => out.push_str(value),
            None => out.push_str(word),
        }
    }
    out
}

fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_'
}

fn is_ident_continue(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn define_substitutes_at_word_boundary_only() {
        let mut defines = HashMap::new();
        defines.insert("BAR".to_string(), "baz".to_string());
        // BAR replaced; BARN left alone; bar (lowercase) left alone.
        assert_eq!(
            substitute_defines("BAR BARN bar", &defines),
            "baz BARN bar"
        );
    }

    #[test]
    fn empty_defines_passes_through() {
        let defines = HashMap::new();
        assert_eq!(substitute_defines("anything", &defines), "anything");
    }
}
