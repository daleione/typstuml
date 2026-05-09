//! CLI entry point — argument parsing and orchestration.
//!
//! Surface (A-plan, subcommand-based):
//!
//! ```text
//! typstuml [GLOBAL] <input> [output]              # implicit `compile`
//! typstuml [GLOBAL] compile  <input> [output]
//! typstuml [GLOBAL] check    <input>
//! typstuml [GLOBAL] emit     <input> [output]
//! typstuml [GLOBAL] diagrams
//! ```
//!
//! `<input>` may be `-` to read from stdin. `<output>` may be `-` or omitted
//! to write to stdout. Output format is inferred from the output extension
//! and falls back to `--format`, then `svg`.
//!
//! `clap::ValueEnum` impls for [`crate::diagnostics::CompatMode`] and
//! [`crate::runtime::Format`] live in this module so the library types stay
//! free of CLI-framework coupling.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use clap::builder::PossibleValue;
use clap::{Args as ClapArgs, Parser, Subcommand, ValueEnum};

use crate::diagnostics::{CompatMode, Error, Result};
use crate::parser;
use crate::runtime::{self, Format};
use crate::theme::Theme;

/// Top-level CLI definition.
///
/// Global options (`--include`, `--compat`, `-q`, `-v`, `--color`) propagate
/// to every subcommand via `global = true`. The implicit `compile` form is
/// implemented by flattening [`CompileArgs`] at the top level: when no
/// subcommand is given, the flattened args drive `compile`.
#[derive(Parser, Debug)]
#[command(
    name = "typstuml",
    version,
    about = "TypstUML — render PlantUML diagrams to SVG / PDF / PNG via Typst",
    long_about = None,
    arg_required_else_help = true,
    args_conflicts_with_subcommands = true,
)]
pub struct Args {
    /// Additional search path for `!include`. Repeatable.
    #[arg(short = 'I', long, value_name = "DIR", global = true)]
    pub include: Vec<PathBuf>,

    /// How strict to be about unsupported PlantUML syntax.
    ///
    /// `strict` errors on any unsupported construct; `warn` (default) logs
    /// and skips it; `loose` silently ignores it.
    #[arg(long, value_enum, default_value_t = CompatMode::Warn, global = true)]
    pub compat: CompatMode,

    /// Suppress informational stderr output (warnings still shown).
    #[arg(short = 'q', long, global = true, conflicts_with = "verbose")]
    pub quiet: bool,

    /// Verbose stderr output.
    #[arg(short = 'v', long, global = true)]
    pub verbose: bool,

    /// Implicit-compile positional args used when no subcommand is given.
    #[command(flatten)]
    pub compile: CompileArgs,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand, Debug, Clone)]
pub enum Command {
    /// Render a PlantUML file to SVG / PDF / PNG (default action).
    Compile(CompileArgs),

    /// Parse only — exit non-zero on parse errors.
    Check(CheckArgs),

    /// Emit the generated Typst source instead of rendering.
    Emit(EmitArgs),

    /// List supported diagram types.
    Diagrams,
}

/// Args shared by the implicit top-level form and the explicit `compile`
/// subcommand. All fields are optional so both forms parse cleanly.
#[derive(ClapArgs, Debug, Default, Clone)]
pub struct CompileArgs {
    /// Input file. Use `-` to read from stdin.
    #[arg(value_name = "INPUT")]
    pub input: Option<PathBuf>,

    /// Output file. Use `-` or omit to write to stdout.
    #[arg(value_name = "OUTPUT")]
    pub output: Option<PathBuf>,

    /// Output format. If omitted, inferred from `<output>`'s extension; if
    /// neither is given, defaults to `svg`.
    #[arg(short = 'f', long, value_enum)]
    pub format: Option<Format>,

    /// Custom Typst preamble injected before each diagram (advanced).
    #[arg(long, value_name = "FILE", hide = true)]
    pub preamble: Option<PathBuf>,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct CheckArgs {
    /// Input file. Use `-` to read from stdin.
    #[arg(value_name = "INPUT")]
    pub input: PathBuf,
}

#[derive(ClapArgs, Debug, Clone)]
pub struct EmitArgs {
    /// Input file. Use `-` to read from stdin.
    #[arg(value_name = "INPUT")]
    pub input: PathBuf,

    /// Output file. Use `-` or omit to write to stdout.
    #[arg(value_name = "OUTPUT")]
    pub output: Option<PathBuf>,

    /// Custom Typst preamble injected before each diagram (advanced).
    #[arg(long, value_name = "FILE", hide = true)]
    pub preamble: Option<PathBuf>,
}

/// Verbosity bucket derived from `-q` / `-v`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Verbosity {
    Quiet,
    #[default]
    Normal,
    Verbose,
}

/// Per-invocation context shared by every subcommand.
#[derive(Clone, Debug)]
pub struct GlobalCtx {
    pub include: Vec<PathBuf>,
    pub compat: CompatMode,
    pub verbosity: Verbosity,
}

impl GlobalCtx {
    fn from_args(args: &Args) -> Self {
        let verbosity = if args.quiet {
            Verbosity::Quiet
        } else if args.verbose {
            Verbosity::Verbose
        } else {
            Verbosity::Normal
        };
        Self {
            include: args.include.clone(),
            compat: args.compat,
            verbosity,
        }
    }

    fn print_warning(&self, msg: &impl std::fmt::Display) {
        if self.verbosity != Verbosity::Quiet {
            eprintln!("{msg}");
        }
    }

    fn print_info(&self, msg: &str) {
        if self.verbosity == Verbosity::Verbose {
            eprintln!("{msg}");
        }
    }
}

/// Top-level entry — parses argv and drives the pipeline.
pub fn run() -> Result<()> {
    run_with(Args::parse())
}

pub fn run_with(args: Args) -> Result<()> {
    let global = GlobalCtx::from_args(&args);
    let command = args
        .command
        .clone()
        .unwrap_or_else(|| Command::Compile(args.compile.clone()));

    match command {
        Command::Compile(c) => run_compile(c, &global),
        Command::Check(c) => run_check(c, &global),
        Command::Emit(c) => run_emit(c, &global),
        Command::Diagrams => {
            print_diagrams();
            Ok(())
        }
    }
}

// -- Subcommand implementations --------------------------------------------

fn run_compile(args: CompileArgs, global: &GlobalCtx) -> Result<()> {
    let input = args
        .input
        .as_deref()
        .ok_or_else(|| Error::Cli("missing INPUT (use `-` for stdin)".into()))?;
    let (document, source_dir) = parse_input(input, global)?;

    let theme = Theme {
        preamble: args.preamble.clone(),
    };
    let typst_source = crate::codegen::emit(&document, &theme)?;

    let format = resolve_format(args.format, args.output.as_deref());
    let rendered = runtime::render(typst_source, source_dir, format)?;
    for w in &rendered.warnings {
        global.print_warning(w);
    }
    write_output(args.output.as_deref(), &rendered.bytes)
}

fn run_check(args: CheckArgs, global: &GlobalCtx) -> Result<()> {
    let (document, _) = parse_input(&args.input, global)?;
    if global.verbosity != Verbosity::Quiet {
        eprintln!(
            "typstuml: parse OK ({} diagram(s))",
            document.diagrams.len()
        );
    }
    Ok(())
}

fn run_emit(args: EmitArgs, global: &GlobalCtx) -> Result<()> {
    let (document, _) = parse_input(&args.input, global)?;
    let theme = Theme {
        preamble: args.preamble.clone(),
    };
    let typst_source = crate::codegen::emit(&document, &theme)?;
    write_output(args.output.as_deref(), typst_source.as_bytes())
}

fn print_diagrams() {
    // Listed in pipeline order: native renderers first, then dispatcher-only
    // (parsed but not yet rendered) types. Mirrors `parser::dispatcher`.
    println!("Supported diagram types:");
    println!("  sequence  — @startuml / @enduml (lifeline grid)");
    println!("  json      — @startjson / @endjson");
    println!("  yaml      — @startyaml / @endyaml");
    println!("  wbs       — @startwbs / @endwbs");
    println!("  mindmap   — @startmindmap / @endmindmap");
    println!();
    println!("Recognized but not yet rendered (run with --compat warn|loose):");
    println!("  class, component, use case, deployment, state, activity,");
    println!("  gantt, timing, salt, network, er, ditaa");
}

// -- Pipeline helpers ------------------------------------------------------

fn parse_input(
    input: &Path,
    global: &GlobalCtx,
) -> Result<(crate::ir::Document, Option<PathBuf>)> {
    let source_text = read_input(input)?;
    let source_dir = if input == Path::new("-") {
        None
    } else {
        input.parent().map(Path::to_path_buf)
    };

    let config = parser::Config {
        include_paths: global.include.clone(),
        source_dir: source_dir.clone(),
    };
    let (document, diagnostics) = parser::parse(&source_text, global.compat, &config)?;
    for diag in &diagnostics {
        global.print_warning(diag);
    }

    if document.diagrams.is_empty() {
        return Err(Error::Cli("no supported diagrams found in input".into()));
    }
    global.print_info(&format!(
        "typstuml: parsed {} diagram(s) from {}",
        document.diagrams.len(),
        display_path(input),
    ));
    Ok((document, source_dir))
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

/// Write `bytes` to the resolved sink: `None` or `Some("-")` → stdout;
/// anything else → file.
fn write_output(path: Option<&Path>, bytes: &[u8]) -> Result<()> {
    match path {
        None => write_stdout(bytes),
        Some(p) if p == Path::new("-") => write_stdout(bytes),
        Some(p) => std::fs::write(p, bytes).map_err(|e| Error::Io {
            path: p.to_path_buf(),
            source: e,
        }),
    }
}

fn write_stdout(bytes: &[u8]) -> Result<()> {
    std::io::stdout().write_all(bytes).map_err(|e| Error::Io {
        path: PathBuf::from("<stdout>"),
        source: e,
    })
}

/// Pick the output format. Explicit `--format` wins; otherwise infer from
/// the output path extension; otherwise default to SVG.
fn resolve_format(explicit: Option<Format>, output: Option<&Path>) -> Format {
    if let Some(f) = explicit {
        return f;
    }
    output
        .filter(|p| *p != Path::new("-"))
        .and_then(Format::infer_from_path)
        .unwrap_or(Format::Svg)
}

fn display_path(p: &Path) -> String {
    if p == Path::new("-") {
        "<stdin>".into()
    } else {
        p.display().to_string()
    }
}

// -- CLI value-enum impls for library types ---------------------------------
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

