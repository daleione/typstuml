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
use crate::runtime::{self, Format, DEFAULT_PNG_SCALE};
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

    /// Disable the measure double-pass protocol.
    ///
    /// By default, class diagrams are first compiled in a `metadata`-only
    /// pass to measure each node's true rendered size; those sizes feed
    /// the second compile (the actual render). `--no-measure` skips
    /// pass-1 and falls back to the Rust-side heuristic estimator —
    /// useful for benchmarking and regression comparison.
    #[arg(long, global = true)]
    pub no_measure: bool,

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

    /// Re-render whenever the input or any included file changes.
    Watch(WatchArgs),

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

#[derive(ClapArgs, Debug, Clone)]
pub struct WatchArgs {
    /// Input file. Stdin (`-`) is not supported in watch mode.
    #[arg(value_name = "INPUT")]
    pub input: PathBuf,

    /// Output file. Required — watch always writes to a real file so
    /// external viewers can pick up the change.
    #[arg(value_name = "OUTPUT")]
    pub output: PathBuf,

    /// Output format. If omitted, inferred from `<output>`'s extension; if
    /// neither is given, defaults to `svg`.
    #[arg(short = 'f', long, value_enum)]
    pub format: Option<Format>,

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
    pub measure: bool,
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
            measure: !args.no_measure,
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
        Command::Watch(c) => run_watch(c, &global),
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
    let parsed = parse_input(input, global)?;

    let theme = Theme {
        preamble: args.preamble.clone(),
    };
    let typst_source =
        build_typst_source(&parsed.document, &theme, parsed.source_dir.as_deref(), global)?;

    let format = resolve_format(args.format, args.output.as_deref());
    let rendered = runtime::render(typst_source, parsed.source_dir, format)?;
    for w in &rendered.warnings {
        global.print_warning(w);
    }
    write_output(args.output.as_deref(), &rendered.bytes)
}

fn run_check(args: CheckArgs, global: &GlobalCtx) -> Result<()> {
    let parsed = parse_input(&args.input, global)?;
    if global.verbosity != Verbosity::Quiet {
        eprintln!(
            "typstuml: parse OK ({} diagram(s))",
            parsed.document.diagrams.len()
        );
    }
    Ok(())
}

fn run_emit(args: EmitArgs, global: &GlobalCtx) -> Result<()> {
    let parsed = parse_input(&args.input, global)?;
    let theme = Theme {
        preamble: args.preamble.clone(),
    };
    let typst_source =
        build_typst_source(&parsed.document, &theme, parsed.source_dir.as_deref(), global)?;
    write_output(args.output.as_deref(), typst_source.as_bytes())
}

/// Render once — used by both the initial pass and every change-triggered
/// re-render in watch mode. Returns the canonical include set so the
/// watcher can subscribe to include-side changes.
fn render_compile(
    input: &Path,
    output: &Path,
    format: Option<Format>,
    preamble: Option<&Path>,
    global: &GlobalCtx,
) -> Result<Vec<PathBuf>> {
    let parsed = parse_input(input, global)?;
    let theme = Theme {
        preamble: preamble.map(Path::to_path_buf),
    };
    let typst_source =
        build_typst_source(&parsed.document, &theme, parsed.source_dir.as_deref(), global)?;
    let fmt = resolve_format(format, Some(output));
    let rendered = runtime::render(typst_source, parsed.source_dir, fmt)?;
    for w in &rendered.warnings {
        global.print_warning(w);
    }
    write_output(Some(output), &rendered.bytes)?;
    Ok(parsed.includes)
}

fn run_watch(args: WatchArgs, global: &GlobalCtx) -> Result<()> {
    use notify::EventKind;
    use notify_debouncer_full::new_debouncer;
    use std::collections::HashSet;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    if args.input == Path::new("-") {
        return Err(Error::Cli(
            "watch mode requires a real input file (stdin not supported)".into(),
        ));
    }
    let input_canon = canonicalize(&args.input)?;

    // Initial render. Non-fatal: a parse error here just means we wait for
    // the user to fix and save.
    let render = || {
        render_compile(
            &args.input,
            &args.output,
            args.format,
            args.preamble.as_deref(),
            global,
        )
    };
    let mut tracked: HashSet<PathBuf> = HashSet::new();
    tracked.insert(input_canon.clone());
    match render() {
        Ok(includes) => {
            report_rendered(&args.output, None);
            tracked.extend(includes.into_iter());
        }
        Err(e) => report_error(&e),
    }

    let (tx, rx) = mpsc::channel();
    let mut debouncer = new_debouncer(Duration::from_millis(150), None, move |res| {
        let _ = tx.send(res);
    })
    .map_err(|e| Error::Cli(format!("watcher init failed: {e}")))?;

    // We subscribe to the *parent directories* of every tracked file rather
    // than the files themselves. On macOS (and most editors elsewhere)
    // saves are implemented via temp-file + rename, which the inode-level
    // single-file watcher misses but a directory watcher catches.
    let mut watched_dirs: HashSet<PathBuf> = HashSet::new();
    sync_watched_dirs(&mut debouncer, &mut watched_dirs, &tracked)?;

    eprintln!(
        "typstuml: watching {} → {} (Ctrl-C to stop)",
        args.input.display(),
        args.output.display()
    );

    for batch in rx {
        let events = match batch {
            Ok(events) => events,
            Err(errs) => {
                for e in errs {
                    eprintln!("typstuml: watcher error: {e}");
                }
                continue;
            }
        };

        // Filter to events that mention a path we actually care about.
        // The directory watcher fires for unrelated siblings too.
        let touched = events.iter().any(|ev| {
            // Skip access-only events — we only care about content changes.
            !matches!(ev.event.kind, EventKind::Access(_))
                && ev.event.paths.iter().any(|p| {
                    let resolved = p.canonicalize().unwrap_or_else(|_| p.clone());
                    tracked.contains(&resolved) || tracked.contains(p)
                })
        });
        if !touched {
            continue;
        }

        let started = Instant::now();
        match render() {
            Ok(includes) => {
                report_rendered(&args.output, Some(started.elapsed()));
                let mut next: HashSet<PathBuf> = HashSet::new();
                next.insert(input_canon.clone());
                next.extend(includes.into_iter());
                if next != tracked {
                    tracked = next;
                    if let Err(e) = sync_watched_dirs(&mut debouncer, &mut watched_dirs, &tracked) {
                        report_error(&e);
                    }
                }
            }
            Err(e) => report_error(&e),
        }
    }
    Ok(())
}

/// Bring the watcher's directory subscription in sync with `tracked` by
/// adding any missing parent dirs and unwatching ones no longer needed.
fn sync_watched_dirs(
    debouncer: &mut notify_debouncer_full::Debouncer<
        notify::RecommendedWatcher,
        notify_debouncer_full::RecommendedCache,
    >,
    currently_watched: &mut std::collections::HashSet<PathBuf>,
    tracked: &std::collections::HashSet<PathBuf>,
) -> Result<()> {
    use notify::RecursiveMode;
    use std::collections::HashSet;

    let want: HashSet<PathBuf> = tracked
        .iter()
        .filter_map(|p| p.parent().map(Path::to_path_buf))
        .collect();

    for dir in want.difference(currently_watched) {
        debouncer
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|e| Error::Cli(format!("watch {}: {e}", dir.display())))?;
    }
    for dir in currently_watched.difference(&want) {
        let _ = debouncer.unwatch(dir);
    }
    *currently_watched = want;
    Ok(())
}

fn report_rendered(output: &Path, elapsed: Option<std::time::Duration>) {
    let stamp = current_time_short();
    match elapsed {
        Some(dt) => eprintln!(
            "[{stamp}] typstuml: rendered {} in {} ms",
            output.display(),
            dt.as_millis()
        ),
        None => eprintln!("[{stamp}] typstuml: rendered {}", output.display()),
    }
}

fn report_error(err: &Error) {
    let stamp = current_time_short();
    eprintln!("[{stamp}] typstuml: error: {err}");
}

fn current_time_short() -> String {
    use time::OffsetDateTime;
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    format!(
        "{:02}:{:02}:{:02}",
        now.hour(),
        now.minute(),
        now.second()
    )
}

fn canonicalize(p: &Path) -> Result<PathBuf> {
    p.canonicalize().map_err(|e| Error::Io {
        path: p.to_path_buf(),
        source: e,
    })
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

/// Build the pass-2 Typst source for `doc`, running the measure
/// double-pass when the global context has it enabled and any diagram
/// needs probing. `source_dir` becomes the runtime root used to
/// resolve local `#image()` / `#read()` references during pass-1; we
/// reuse it so user-preamble paths work the same as in pass-2.
///
/// The orchestration intentionally lives in CLI rather than `codegen`
/// so the codegen crate doesn't pull in `runtime::measure` (one less
/// cycle in the module graph). Codegen exposes the two halves —
/// `emit_probes` for pass-1 source, `emit` accepting an optional
/// `MeasurementSet` for pass-2 — and CLI glues them together.
fn build_typst_source(
    doc: &crate::ir::Document,
    theme: &Theme,
    source_dir: Option<&Path>,
    global: &GlobalCtx,
) -> Result<String> {
    let measurement_root = source_dir
        .map(Path::to_path_buf)
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    if !global.measure {
        return crate::codegen::emit(doc, theme, None);
    }

    let Some((probe_source, expected_ids)) = crate::codegen::emit_probes(doc, theme)? else {
        // No measurement-aware diagrams — skip pass-1 entirely.
        return crate::codegen::emit(doc, theme, None);
    };

    let expected_refs: Vec<&str> = expected_ids.iter().map(String::as_str).collect();
    let start = std::time::Instant::now();
    let set = match runtime::measure::run(probe_source, measurement_root, &expected_refs) {
        Ok(s) => s,
        Err(e) => {
            // Falling back to heuristic is the safe behavior — never
            // block rendering on a measure-protocol failure. Surface
            // the error as a warning so misconfigurations don't go
            // silently wrong.
            global.print_warning(&format!(
                "typstuml: warning: measure pass failed ({e}); falling back to heuristic",
            ));
            return crate::codegen::emit(doc, theme, None);
        }
    };
    let elapsed_ms = start.elapsed().as_millis();
    global.print_info(&format!(
        "measure: {} probes, {elapsed_ms}ms",
        set.len()
    ));

    crate::codegen::emit(doc, theme, Some(&set))
}

/// Result of `parse_input`: the document plus the bookkeeping that
/// downstream code (codegen, watch) needs.
struct Parsed {
    document: crate::ir::Document,
    source_dir: Option<PathBuf>,
    /// Canonical paths of every `!include`d file; empty when input is stdin.
    includes: Vec<PathBuf>,
}

fn parse_input(input: &Path, global: &GlobalCtx) -> Result<Parsed> {
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
    let parsed = parser::parse(&source_text, global.compat, &config)?;
    for diag in &parsed.diagnostics {
        global.print_warning(diag);
    }

    if parsed.document.diagrams.is_empty() {
        return Err(Error::Cli("no supported diagrams found in input".into()));
    }
    global.print_info(&format!(
        "typstuml: parsed {} diagram(s) from {}",
        parsed.document.diagrams.len(),
        display_path(input),
    ));
    Ok(Parsed {
        document: parsed.document,
        source_dir,
        includes: parsed.includes,
    })
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
        // CLI doesn't expose `--png-scale` yet, so the only PNG variant
        // surfaced through clap is the default-scale one.
        &[Self::Svg, Self::Pdf, Self::Png { scale: DEFAULT_PNG_SCALE }]
    }

    fn to_possible_value(&self) -> Option<PossibleValue> {
        Some(match self {
            Self::Svg => PossibleValue::new("svg"),
            Self::Pdf => PossibleValue::new("pdf"),
            Self::Png { .. } => PossibleValue::new("png"),
        })
    }
}

