//! Error types and diagnostic helpers.
//!
//! Per design doc §10.3, unsupported syntax is bucketed into three behaviors —
//! `error`, `warn + degrade`, `ignore` — selected via the `--compat` flag.
//! Strict bubbles up as [`Error::Unsupported`]; Warn/Loose fall through with
//! a [`Diagnostic`].

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

    #[error("Typst compilation failed:\n{message}")]
    TypstCompile {
        message: String,
        diagnostics: Vec<Diagnostic>,
    },

    #[error("invalid CLI usage: {0}")]
    Cli(String),

    /// Internal protocol violation in the measure double-pass — a probe ID
    /// was emitted by codegen but not echoed back by Typst, or the metadata
    /// shape didn't match. Indicates a TypstUML bug, not a user error.
    #[error("measure protocol violation: {0}")]
    MeasureProtocol(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    /// Convert any public error variant into diagnostics suitable for an
    /// editor's inline error UI. Typst compilation errors retain each original
    /// diagnostic; simpler error variants become a single synthesized entry.
    pub fn to_diagnostics(&self) -> Vec<Diagnostic> {
        match self {
            Self::TypstCompile { diagnostics, .. } => diagnostics.clone(),
            Self::Parse { line, message } => vec![Diagnostic::error_at(*line, message.clone())],
            _ => vec![Diagnostic::error(self.to_string())],
        }
    }
}

/// Diagnostic message attached to a source location.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Diagnostic {
    pub level: Level,
    /// Virtual or real source path, when one is available.
    pub path: Option<String>,
    pub line: Option<usize>,
    pub column: Option<usize>,
    pub message: String,
    pub hints: Vec<String>,
}

impl Diagnostic {
    pub fn new(level: Level, line: Option<usize>, message: impl Into<String>) -> Self {
        Self {
            level,
            path: None,
            line,
            column: None,
            message: message.into(),
            hints: Vec::new(),
        }
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(Level::Warning, None, message)
    }

    pub fn warning_at(line: usize, message: impl Into<String>) -> Self {
        Self::new(Level::Warning, Some(line), message)
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(Level::Error, None, message)
    }

    pub fn error_at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line: Some(line),
            ..Self::error(message)
        }
    }
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
        match (&self.path, self.line, self.column) {
            (Some(path), Some(line), Some(column)) => {
                write!(f, "{tag} ({path}:{line}:{column}): {}", self.message)
            }
            (Some(path), Some(line), None) => {
                write!(f, "{tag} ({path}:{line}): {}", self.message)
            }
            (_, Some(line), _) => write!(f, "{tag} (line {line}): {}", self.message),
            _ => write!(f, "{tag}: {}", self.message),
        }
    }
}
