//! Error types and diagnostic helpers.
//!
//! Per design doc §10.3, unsupported syntax is bucketed into three behaviors —
//! `error`, `warn + degrade`, `ignore` — selected via the `--compat` flag.
//! M0 routes "Strict" through [`Error::Unsupported`] and lets Warn/Loose fall
//! through with a [`Diagnostic`].

use std::fmt;
use std::path::PathBuf;

use thiserror::Error;

/// User-controllable strictness for unsupported syntax / behaviors.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum CompatMode {
    /// Fail on any unsupported construct.
    Strict,
    /// Warn on stderr and degrade gracefully.
    #[default]
    Warn,
    /// Silently ignore unsupported constructs.
    Loose,
}

#[derive(Error, Debug)]
pub enum Error {
    #[error("I/O error on {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("parse error at line {line}: {message}")]
    Parse { line: usize, message: String },

    #[error("unsupported {kind}: {detail}")]
    Unsupported { kind: &'static str, detail: String },

    #[error("Typst compilation failed:\n{0}")]
    TypstCompile(String),

    #[error("invalid CLI usage: {0}")]
    Cli(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Diagnostic message attached to a source location.
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub level: Level,
    pub line: Option<usize>,
    pub message: String,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Level {
    Warning,
    Error,
}

impl fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let tag = match self.level {
            Level::Warning => "warning",
            Level::Error => "error",
        };
        match self.line {
            Some(n) => write!(f, "{tag} (line {n}): {}", self.message),
            None => write!(f, "{tag}: {}", self.message),
        }
    }
}
