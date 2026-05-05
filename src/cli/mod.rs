//! CLI entry point — argument parsing and orchestration.
//!
//! Mirrors the surface defined in design doc §9. The `clap::ValueEnum` impls
//! for [`crate::diagnostics::CompatMode`] and [`crate::runtime::Format`]
//! live in this module so the library types stay free of CLI-framework
//! coupling.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use clap::builder::PossibleValue;
use clap::{Parser, ValueEnum};

use crate::diagnostics::{CompatMode, Error, Result};
use crate::parser;
use crate::runtime::{self, Format};
use crate::theme::Theme;

#[derive(Parser, Debug)]
#[command(
    name = "typstuml",
    version,
    about = "TypstUML — render PlantUML diagrams to SVG / PDF / PNG via Typst",
    long_about = None,
)]
pub struct Args {
    /// Input file. Use `-` for stdin.
    #[arg(value_name = "INPUT")]
    pub input: PathBuf,

    /// Output file. Mutually exclusive with `--stdout` / `--check` / `--emit-typst`.
    #[arg(short = 'o', long, value_name = "PATH")]
    pub output: Option<PathBuf>,

    /// Output format. If omitted, inferred from `--output`'s extension; if
    /// neither is given, defaults to `svg`.
    #[arg(short = 'f', long, value_enum)]
    pub format: Option<Format>,

    /// Theme name. Currently a passthrough — full `skinparam` / `!theme`
    /// mapping is parsed off the source, not from this flag.
    #[arg(short = 't', long)]
    pub theme: Option<String>,

    /// Custom Typst preamble injected before each diagram.
    #[arg(long, value_name = "FILE")]
    pub typst_template: Option<PathBuf>,

    /// Write rendered output to stdout instead of a file.
    #[arg(long)]
    pub stdout: bool,

    /// Parse only — no rendering. Exit non-zero on parse errors.
    #[arg(long)]
    pub check: bool,

    /// Emit the generated Typst source instead of rendering.
    #[arg(long)]
    pub emit_typst: bool,

    /// Additional search path for `!include`. Repeatable.
    #[arg(long, value_name = "DIR")]
    pub include: Vec<PathBuf>,

    /// How strict to be about unsupported PlantUML syntax.
    #[arg(long, value_enum, default_value_t = CompatMode::Warn)]
    pub compat: CompatMode,

    /// (not yet implemented) Watch the input file for changes and re-render.
    #[arg(long)]
    pub watch: bool,

    /// (not yet implemented) Emit machine-readable JSON diagnostics.
    #[arg(long)]
    pub json: bool,
}

/// Top-level entry — parses argv and drives the pipeline.
pub fn run() -> Result<()> {
    run_with(Args::parse())
}

pub fn run_with(args: Args) -> Result<()> {
    if args.watch {
        eprintln!("typstuml: --watch is not yet implemented; ignoring");
    }
    if args.json {
        eprintln!("typstuml: --json is not yet implemented; ignoring");
    }

    let source_text = read_input(&args.input)?;
    let source_dir = if args.input == Path::new("-") {
        None
    } else {
        args.input.parent().map(Path::to_path_buf)
    };

    let config = parser::Config {
        include_paths: args.include.clone(),
        source_dir: source_dir.clone(),
    };
    let (document, diagnostics) = parser::parse(&source_text, args.compat, &config)?;
    for diag in &diagnostics {
        eprintln!("{diag}");
    }

    if document.diagrams.is_empty() {
        return Err(Error::Cli("no supported diagrams found in input".into()));
    }

    if args.check {
        eprintln!("typstuml: parse OK ({} diagram(s))", document.diagrams.len());
        return Ok(());
    }

    let theme = Theme {
        name: args.theme.clone(),
        typst_template: args.typst_template.clone(),
    };
    let typst_source = crate::codegen::emit(&document, &theme)?;

    if args.emit_typst {
        write_output(&args, typst_source.into_bytes())?;
        return Ok(());
    }

    let format = resolve_format(&args);
    let rendered = runtime::render(typst_source, source_dir, format)?;
    for w in &rendered.warnings {
        eprintln!("{w}");
    }
    write_output(&args, rendered.bytes)
}

fn read_input(path: &Path) -> Result<String> {
    if path == Path::new("-") {
        let mut buf = String::new();
        std::io::stdin()
            .read_to_string(&mut buf)
            .map_err(|e| Error::Io {
                path: PathBuf::from("<stdin>"),
                source: e,
            })?;
        Ok(buf)
    } else {
        std::fs::read_to_string(path).map_err(|e| Error::Io {
            path: path.to_path_buf(),
            source: e,
        })
    }
}

fn write_output(args: &Args, bytes: Vec<u8>) -> Result<()> {
    if args.stdout {
        std::io::stdout().write_all(&bytes).map_err(|e| Error::Io {
            path: PathBuf::from("<stdout>"),
            source: e,
        })?;
        return Ok(());
    }
    let out = args
        .output
        .as_ref()
        .ok_or_else(|| Error::Cli("missing --output (or pass --stdout)".into()))?;
    std::fs::write(out, &bytes).map_err(|e| Error::Io {
        path: out.clone(),
        source: e,
    })
}

fn resolve_format(args: &Args) -> Format {
    if let Some(f) = args.format {
        return f;
    }
    args.output
        .as_deref()
        .and_then(Format::infer_from_path)
        .unwrap_or(Format::Svg)
}

// -- CLI value-enum impls for library types ----------------------------------
//
// Keeping these here means `diagnostics::CompatMode` and `runtime::Format`
// don't need to know about clap. Orphan rules are satisfied because both
// types are local to this crate.

impl ValueEnum for CompatMode {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::Strict, Self::Warn, Self::Loose]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        Some(match self {
            Self::Strict => PossibleValue::new("strict"),
            Self::Warn => PossibleValue::new("warn"),
            Self::Loose => PossibleValue::new("loose"),
        })
    }
}

impl ValueEnum for Format {
    fn value_variants<'a>() -> &'a [Self] {
        &[Self::Svg, Self::Pdf, Self::Png]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        Some(match self {
            Self::Svg => PossibleValue::new("svg"),
            Self::Pdf => PossibleValue::new("pdf"),
            Self::Png => PossibleValue::new("png"),
        })
    }
}
