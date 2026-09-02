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

use crate::parser::common::{is_ident_continue, is_ident_start};

use crate::diagnostics::{CompatMode, Diagnostic, Error, Result};

#[derive(Clone, Debug, Default)]
pub struct Preprocessed {
    pub text: String,
    pub diagnostics: Vec<Diagnostic>,
    /// Canonicalized paths of every file pulled in via `!include`. Used by
    /// `watch` mode to subscribe to include-side changes.
    pub includes: Vec<PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct Config {
    pub include_paths: Vec<PathBuf>,
    pub source_dir: Option<PathBuf>,
    /// Root established by the original input file. Nested includes update
    /// `source_dir`, but must remain inside this root. Explicit include paths
    /// are additional roots deliberately granted by the CLI caller.
    pub project_root: Option<PathBuf>,
    /// Whether `!include` may read real files. The default is deliberately
    /// false so in-memory callers cannot reach the process working directory
    /// (including through absolute include paths).
    pub allow_filesystem: bool,
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
        includes: state.includes,
    })
}

#[derive(Default)]
struct State {
    defines: HashMap<String, String>,
    in_progress: HashSet<PathBuf>,
    diagnostics: Vec<Diagnostic>,
    /// Canonical include paths, in the order they were first resolved.
    includes: Vec<PathBuf>,
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
            state.diagnostics.push(Diagnostic::warning_at(line_no, msg));
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
            state.diagnostics.push(Diagnostic::warning_at(
                line_no,
                format!("malformed !define: {rest:?}"),
            ));
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
    if !config.allow_filesystem {
        let msg = format!(
            "filesystem access is disabled; cannot resolve !include {path:?} in memory mode"
        );
        if compat == CompatMode::Strict {
            return Err(Error::Parse {
                line: line_no,
                message: msg,
            });
        }
        state.diagnostics.push(Diagnostic::warning_at(line_no, msg));
        return Ok(());
    }

    let resolved = match resolve_include(path, config) {
        IncludeResolution::Found(path) => path,
        IncludeResolution::Missing => {
            let msg = format!("could not resolve !include {path:?}");
            if compat == CompatMode::Strict {
                return Err(Error::Parse {
                    line: line_no,
                    message: msg,
                });
            }
            state.diagnostics.push(Diagnostic::warning_at(line_no, msg));
            return Ok(());
        }
        IncludeResolution::Denied => {
            let msg = format!(
                "filesystem access denied: !include {path:?} escapes the project/include roots"
            );
            if compat == CompatMode::Strict {
                return Err(Error::Parse {
                    line: line_no,
                    message: msg,
                });
            }
            state.diagnostics.push(Diagnostic::warning_at(line_no, msg));
            return Ok(());
        }
    };

    let canonical = resolved.canonicalize().unwrap_or_else(|_| resolved.clone());
    if !state.in_progress.insert(canonical.clone()) {
        return Err(Error::Parse {
            line: line_no,
            message: format!("circular !include: {}", resolved.display()),
        });
    }
    if !state.includes.contains(&canonical) {
        state.includes.push(canonical.clone());
    }

    let content = std::fs::read_to_string(&resolved).map_err(|e| Error::Io {
        path: resolved.clone(),
        source: e,
    })?;

    let nested_config = Config {
        source_dir: resolved.parent().map(Path::to_path_buf),
        include_paths: config.include_paths.clone(),
        project_root: config.project_root.clone(),
        allow_filesystem: config.allow_filesystem,
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

enum IncludeResolution {
    Found(PathBuf),
    Missing,
    Denied,
}

fn resolve_include(path: &str, config: &Config) -> IncludeResolution {
    let p = PathBuf::from(path);
    let mut candidates = Vec::new();
    if p.is_absolute() {
        candidates.push(p.clone());
    }
    if let Some(dir) = &config.source_dir {
        candidates.push(dir.join(&p));
    }
    candidates.extend(config.include_paths.iter().map(|dir| dir.join(&p)));

    let roots = config
        .project_root
        .iter()
        .chain(config.include_paths.iter())
        .filter_map(|root| root.canonicalize().ok())
        .collect::<Vec<_>>();
    let mut denied = false;
    for candidate in candidates {
        if !candidate.exists() {
            continue;
        }
        let Ok(canonical) = candidate.canonicalize() else {
            continue;
        };
        if roots.iter().any(|root| canonical.starts_with(root)) {
            return IncludeResolution::Found(canonical);
        }
        denied = true;
    }
    if denied {
        IncludeResolution::Denied
    } else {
        IncludeResolution::Missing
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn define_substitutes_at_word_boundary_only() {
        let mut defines = HashMap::new();
        defines.insert("BAR".to_string(), "baz".to_string());
        // BAR replaced; BARN left alone; bar (lowercase) left alone.
        assert_eq!(substitute_defines("BAR BARN bar", &defines), "baz BARN bar");
    }

    #[test]
    fn empty_defines_passes_through() {
        let defines = HashMap::new();
        assert_eq!(substitute_defines("anything", &defines), "anything");
    }
}
