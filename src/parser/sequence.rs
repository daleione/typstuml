//! Native Sequence diagram parser.
//!
//! Hand-written line scanner that matches `blockcell.seq-puml`'s behavior on
//! the supported P0 subset. Produces a structured AST ([`StructuredSequence`])
//! with per-step source lines so diagnostics can point back at the original
//! `.puml`. Unparseable lines emit a [`Diagnostic`] (or hard error in
//! `--compat strict`) and are dropped from the AST.

use crate::diagnostics::{CompatMode, Diagnostic, Error, Level, Result};
use crate::ir::{
    Branch, Diagram, FragmentKind, NotePosition, Participant, ParticipantKind, SequenceDiagram,
    Skinparam, Step, StructuredSequence,
};
use crate::parser::lexer::{BodyLine, UmlBlock};

pub fn parse(block: &UmlBlock, compat: CompatMode) -> Result<(Diagram, Vec<Diagnostic>)> {
    let mut parser = Parser::new(&block.body, compat);
    parser.run()?;
    let mut seq = parser.seq;
    seq.name = block.name.clone();
    Ok((
        Diagram::Sequence(SequenceDiagram::Structured(seq)),
        parser.diagnostics,
    ))
}

struct Parser<'a> {
    lines: &'a [BodyLine],
    pos: usize,
    compat: CompatMode,
    seq: StructuredSequence,
    frag_stack: Vec<FragmentInProgress>,
    note_state: Option<NoteInProgress>,
    diagnostics: Vec<Diagnostic>,
}

struct FragmentInProgress {
    kind: FragmentKind,
    label: Option<String>,
    branches: Vec<Branch>,
    line: usize,
}

struct NoteInProgress {
    position: NotePosition,
    participants: Vec<String>,
    accumulated: Vec<String>,
    line: usize,
}

impl<'a> Parser<'a> {
    fn new(lines: &'a [BodyLine], compat: CompatMode) -> Self {
        Self {
            lines,
            pos: 0,
            compat,
            seq: StructuredSequence::default(),
            frag_stack: Vec::new(),
            note_state: None,
            diagnostics: Vec::new(),
        }
    }

    fn run(&mut self) -> Result<()> {
        while self.pos < self.lines.len() {
            let body_line = &self.lines[self.pos];
            self.pos += 1;
            let line_no = body_line.line;
            let raw = body_line.text.trim();

            if let Some(note) = self.note_state.as_mut() {
                if is_end_note(raw) {
                    self.close_multiline_note();
                } else {
                    note.accumulated.push(strip_inline_comment(raw).to_string());
                }
                continue;
            }

            if raw.is_empty() || is_comment(raw) {
                continue;
            }
            if is_skip_directive(raw) {
                continue;
            }
            if let Some(rest) = strip_prefix_keyword(raw, "skinparam") {
                self.handle_skinparam(rest, line_no);
                continue;
            }
            if let Some(rest) = strip_prefix_keyword(raw, "autoactivate") {
                let arg = rest.trim().to_ascii_lowercase();
                self.seq.autoactivate =
                    matches!(arg.as_str(), "on" | "yes" | "true" | "");
                continue;
            }
            if let Some(rest) = strip_prefix_keyword(raw, "title") {
                let t = rest.trim();
                if !t.is_empty() {
                    self.seq.title = Some(t.to_string());
                }
                continue;
            }

            if raw == "end" {
                self.handle_end(line_no)?;
                continue;
            }
            if let Some(rest) = strip_prefix_keyword(raw, "else") {
                self.handle_else(rest.trim(), line_no)?;
                continue;
            }
            if let Some((stripped, has_block)) = strip_participant_block_open(raw) {
                if let Some(mut p) = parse_participant(&stripped, line_no) {
                    if has_block {
                        p.display_block = Some(self.consume_display_block());
                    }
                    self.register_participant(p);
                    continue;
                }
            } else if let Some(p) = parse_participant(raw, line_no) {
                self.register_participant(p);
                continue;
            }
            if let Some((kind, label)) = parse_fragment_start(raw) {
                self.frag_stack.push(FragmentInProgress {
                    kind,
                    label,
                    branches: vec![Branch::default()],
                    line: line_no,
                });
                continue;
            }
            if let Some(divider) = parse_divider(raw) {
                self.push_step(Step::Divider {
                    label: divider,
                    line: line_no,
                });
                continue;
            }
            if let Some(rest) = strip_prefix_keyword(raw, "autonumber") {
                self.push_step(Step::Autonumber {
                    raw: rest.trim().to_string(),
                    line: line_no,
                });
                continue;
            }
            if let Some(rest) = strip_prefix_keyword(raw, "activate") {
                let (target, color) = split_target_color(rest.trim());
                self.push_step(Step::Activate {
                    participant: target.to_string(),
                    color,
                    line: line_no,
                });
                continue;
            }
            if let Some(rest) = strip_prefix_keyword(raw, "deactivate") {
                self.push_step(Step::Deactivate {
                    participant: rest.trim().to_string(),
                    line: line_no,
                });
                continue;
            }
            if let Some(rest) = strip_prefix_keyword(raw, "destroy") {
                self.push_step(Step::Destroy {
                    participant: rest.trim().to_string(),
                    line: line_no,
                });
                continue;
            }
            if let Some(rest) = strip_prefix_keyword(raw, "create") {
                self.handle_create(rest.trim(), line_no);
                continue;
            }
            if let Some(rest) = strip_prefix_keyword(raw, "return") {
                let label = rest.trim();
                self.push_step(Step::Return {
                    label: if label.is_empty() {
                        None
                    } else {
                        Some(label.to_string())
                    },
                    line: line_no,
                });
                continue;
            }
            if let Some(parsed) = parse_note_line(raw, line_no) {
                match parsed {
                    NoteParse::Single(step) => self.push_step(step),
                    NoteParse::MultilineStart {
                        position,
                        participants,
                    } => {
                        self.note_state = Some(NoteInProgress {
                            position,
                            participants,
                            accumulated: Vec::new(),
                            line: line_no,
                        });
                    }
                }
                continue;
            }
            if let Some(mut step) = parse_message(raw, line_no) {
                if let Step::Message { from, to, .. } = &mut step {
                    self.normalize_endpoint(from, line_no);
                    self.normalize_endpoint(to, line_no);
                }
                self.push_step(step);
                continue;
            }

            self.unsupported(raw, line_no)?;
        }

        if let Some(note) = self.note_state.take() {
            self.warn_or_err(
                Level::Warning,
                Some(note.line),
                format!(
                    "unterminated multi-line note (started at line {})",
                    note.line
                ),
            )?;
        }
        while let Some(frag) = self.frag_stack.pop() {
            self.warn_or_err(
                Level::Warning,
                Some(frag.line),
                format!(
                    "unterminated {} fragment (missing `end`)",
                    frag.kind.keyword()
                ),
            )?;
            // Best-effort: still capture what we have.
            let step = Step::Fragment {
                kind: frag.kind,
                label: frag.label,
                branches: frag.branches,
                line: frag.line,
            };
            self.push_step(step);
        }
        Ok(())
    }

    fn push_step(&mut self, step: Step) {
        if let Some(frag) = self.frag_stack.last_mut() {
            frag.branches
                .last_mut()
                .expect("fragment always has at least one branch")
                .steps
                .push(step);
        } else {
            self.seq.steps.push(step);
        }
    }

    /// Resolve a message endpoint that may carry inline participant syntax:
    /// `"Display"`, `"Display" as Alias`, or a bare unquoted name. Mutates
    /// `endpoint` in place to hold the canonical id and implicitly declares
    /// the participant when the endpoint introduced one. `\n` inside the
    /// quoted display is preserved literally — codegen splits on it later.
    fn normalize_endpoint(&mut self, endpoint: &mut String, line_no: usize) {
        let trimmed = endpoint.trim();
        // Skip boundary anchors (`[`, `]`) — those are figure edges, not real
        // participants. Blockcell handles them specially.
        if trimmed == "[" || trimmed == "]" {
            return;
        }
        let (id, display) = if let Some((quoted, after)) = strip_leading_quoted(trimmed) {
            let after = after.trim();
            let disp = unescape_display(&quoted);
            if let Some(alias) = strip_prefix_keyword(after, "as")
                .map(str::trim)
                .filter(|a| !a.is_empty())
            {
                (alias.to_string(), disp)
            } else {
                (disp.clone(), disp)
            }
        } else {
            // Bare name — register to lock in source order; display == id.
            (trimmed.to_string(), trimmed.to_string())
        };
        *endpoint = id.clone();
        self.register_participant(Participant {
            kind: ParticipantKind::Participant,
            id,
            display,
            display_block: None,
            color: None,
            line: line_no,
        });
    }

    /// Consume body lines that follow a `participant Foo [` opener, stopping
    /// at the line whose trimmed content is `]`. The closer is consumed but
    /// not returned. If we hit EOF without seeing a closer the partial body
    /// is returned as-is — better to render a malformed label than crash.
    fn consume_display_block(&mut self) -> Vec<String> {
        let mut body = Vec::new();
        while self.pos < self.lines.len() {
            let line = &self.lines[self.pos];
            self.pos += 1;
            let trimmed = line.text.trim();
            if trimmed == "]" {
                break;
            }
            body.push(trimmed.to_string());
        }
        body
    }

    fn register_participant(&mut self, p: Participant) {
        if !self
            .seq
            .participants
            .iter()
            .any(|existing| existing.id == p.id)
        {
            self.seq.participants.push(p);
        }
    }

    fn handle_create(&mut self, rest: &str, line_no: usize) {
        // `create [<keyword>] <participant ...>` — keyword is optional and
        // defaults to `participant`.
        let body = if let Some(p) = parse_participant(rest, line_no) {
            Some(p)
        } else {
            // `create A` (no keyword) — synthesize a participant declaration.
            let body = format!("participant {rest}");
            parse_participant(&body, line_no)
        };
        if let Some(p) = body {
            self.register_participant(p.clone());
            self.push_step(Step::Create(p));
        }
    }

    fn close_multiline_note(&mut self) {
        if let Some(note) = self.note_state.take() {
            let text = note.accumulated.join("\n");
            self.push_step(Step::Note {
                position: note.position,
                participants: note.participants,
                text,
                line: note.line,
            });
        }
    }

    fn handle_end(&mut self, line_no: usize) -> Result<()> {
        let Some(frag) = self.frag_stack.pop() else {
            self.warn_or_err(
                Level::Warning,
                Some(line_no),
                "stray `end` with no open fragment".to_string(),
            )?;
            return Ok(());
        };
        let step = Step::Fragment {
            kind: frag.kind,
            label: frag.label,
            branches: frag.branches,
            line: frag.line,
        };
        self.push_step(step);
        Ok(())
    }

    fn handle_else(&mut self, label: &str, line_no: usize) -> Result<()> {
        let Some(top_kind) = self.frag_stack.last().map(|f| f.kind) else {
            return self.warn_or_err(
                Level::Warning,
                Some(line_no),
                "stray `else` with no open fragment".to_string(),
            );
        };
        if !top_kind.has_else() {
            return self.warn_or_err(
                Level::Warning,
                Some(line_no),
                format!(
                    "`else` only valid inside `alt` / `critical`, not `{}`",
                    top_kind.keyword()
                ),
            );
        }
        let label = strip_color_prefixes(label);
        let label = if label.is_empty() {
            None
        } else {
            Some(label.to_string())
        };
        self.frag_stack
            .last_mut()
            .expect("checked above")
            .branches
            .push(Branch {
                label,
                steps: Vec::new(),
            });
        Ok(())
    }

    fn handle_skinparam(&mut self, rest: &str, line_no: usize) {
        let rest = rest.trim();
        if rest.is_empty() {
            return;
        }
        let (key, value) = match rest.split_once(char::is_whitespace) {
            Some((k, v)) => (k.trim().to_string(), v.trim().to_string()),
            None => (rest.to_string(), String::new()),
        };
        self.seq.skinparams.push(Skinparam {
            key,
            value,
            line: line_no,
        });
    }

    fn unsupported(&mut self, raw: &str, line_no: usize) -> Result<()> {
        let head: String = raw
            .split_whitespace()
            .next()
            .unwrap_or("")
            .chars()
            .take(40)
            .collect();
        self.warn_or_err(
            Level::Warning,
            Some(line_no),
            format!("unrecognized sequence syntax (starts with {head:?})"),
        )
    }

    fn warn_or_err(&mut self, level: Level, line: Option<usize>, message: String) -> Result<()> {
        if self.compat == CompatMode::Strict && level == Level::Warning {
            return Err(Error::Parse {
                line: line.unwrap_or(0),
                message,
            });
        }
        if self.compat == CompatMode::Loose {
            return Ok(());
        }
        self.diagnostics.push(Diagnostic {
            level,
            line,
            message,
        });
        Ok(())
    }
}

// ---- Per-line parsers ------------------------------------------------------

fn is_comment(line: &str) -> bool {
    line.starts_with('\'') || line.starts_with("/'")
}

fn is_end_note(line: &str) -> bool {
    matches!(line, "end note" | "endnote" | "endrnote" | "endhnote")
}

fn is_skip_directive(line: &str) -> bool {
    const HEADS: &[&str] = &[
        "@startuml",
        "@enduml",
        "hide ",
        "show ",
        "header ",
        "footer ",
        "mainframe ",
        "newpage",
        "!theme",
        "!pragma",
        "scale ",
        "left to right",
        "top to bottom",
        "box ",
        "end box",
    ];
    HEADS
        .iter()
        .any(|h| line == h.trim() || line.starts_with(h))
}

fn strip_prefix_keyword<'a>(line: &'a str, keyword: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(keyword)?;
    match rest.chars().next() {
        None => Some(rest),
        Some(c) if c.is_whitespace() => Some(rest),
        _ => None,
    }
}

fn strip_inline_comment(line: &str) -> &str {
    if let Some(idx) = line.find(" '") {
        line[..idx].trim_end()
    } else {
        line
    }
}

const PARTICIPANT_KEYWORDS: &[&str] = &[
    "participant",
    "actor",
    "boundary",
    "control",
    "entity",
    "database",
    "collections",
    "queue",
];

/// If `line` is a participant declaration whose trailing token is `[`
/// (opening a multi-line `[ … ]` rich-content block), return the line with
/// the trailing `[` stripped along with a `true` flag. Returns `None` when
/// the line does not start with a participant keyword, leaving the caller to
/// fall back to the regular parser. The `false` flag form is unused today
/// but reserved for a future single-line `[…]` variant.
fn strip_participant_block_open(line: &str) -> Option<(String, bool)> {
    let starts_with_pkw = PARTICIPANT_KEYWORDS
        .iter()
        .any(|kw| strip_prefix_keyword(line, kw).is_some());
    if !starts_with_pkw {
        return None;
    }
    let trimmed = line.trim_end();
    let stripped = trimmed.strip_suffix('[')?;
    Some((stripped.trim_end().to_string(), true))
}

fn parse_participant(line: &str, line_no: usize) -> Option<Participant> {
    let (kw, rest) = PARTICIPANT_KEYWORDS
        .iter()
        .find_map(|kw| strip_prefix_keyword(line, kw).map(|r| (*kw, r.trim())))?;
    let kind = ParticipantKind::from_keyword(kw)?;

    let mut rest = rest.to_string();
    let color = pop_trailing_color(&mut rest);
    pop_trailing_order(&mut rest);

    let (id, display) = parse_alias(rest.trim())?;
    Some(Participant {
        kind,
        id,
        display,
        display_block: None,
        color,
        line: line_no,
    })
}

/// Strip a trailing ` #color` token; returns it (with the `#`) if present.
fn pop_trailing_color(rest: &mut String) -> Option<String> {
    let trimmed = rest.trim_end();
    let bytes = trimmed.as_bytes();
    let mut i = bytes.len();
    while i > 0 && !bytes[i - 1].is_ascii_whitespace() {
        i -= 1;
    }
    if i == 0 {
        return None;
    }
    let token = trimmed[i..].to_string();
    if !token.starts_with('#') {
        return None;
    }
    let kept = trimmed[..i].trim_end().to_string();
    *rest = kept;
    Some(token)
}

fn pop_trailing_order(rest: &mut String) {
    // Match `\s+order\s+\d+\s*$`, case-insensitive.
    let trimmed = rest.trim_end();
    let lower = trimmed.to_ascii_lowercase();
    let Some(idx) = lower.rfind(" order ") else {
        return;
    };
    let after = trimmed[idx + " order ".len()..].trim();
    if after.is_empty() || !after.chars().all(|c| c.is_ascii_digit()) {
        return;
    }
    let kept = trimmed[..idx].trim_end().to_string();
    *rest = kept;
}

/// Parse `"Long Name" as alias`, `alias as "Long Name"`, or a bare name.
/// Returns `(canonical_id, display_label)`.
fn parse_alias(rest: &str) -> Option<(String, String)> {
    if rest.is_empty() {
        return None;
    }
    if let Some((quoted, after)) = strip_leading_quoted(rest) {
        let display = unescape_display(&quoted);
        let after = after.trim();
        if let Some(alias) = strip_prefix_keyword(after, "as").map(str::trim) {
            if !alias.is_empty() {
                return Some((alias.to_string(), display));
            }
        }
        return Some((display.clone(), display));
    }
    let mut parts = rest.splitn(2, char::is_whitespace);
    let first = parts.next()?;
    let tail = parts.next().unwrap_or("").trim_start();
    if let Some(after_as) = strip_prefix_keyword(tail, "as").map(str::trim) {
        if let Some((quoted, _)) = strip_leading_quoted(after_as) {
            // `id as "Display"` — first token is the id, quoted is the display.
            return Some((first.to_string(), unescape_display(&quoted)));
        }
        // `Display as id` — token after `as` is the alias used in messages.
        return Some((after_as.to_string(), first.to_string()));
    }
    Some((first.to_string(), rest.to_string()))
}

/// Interpret PlantUML escape sequences in a quoted display string. Only `\n`
/// is meaningful for our renderer; everything else passes through unchanged.
fn unescape_display(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') => out.push('\n'),
                Some(other) => {
                    out.push('\\');
                    out.push(other);
                }
                None => out.push('\\'),
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn strip_leading_quoted(s: &str) -> Option<(String, &str)> {
    let s = s.trim_start();
    if !s.starts_with('"') {
        return None;
    }
    let after = &s[1..];
    let close = after.find('"')?;
    let inner = after[..close].to_string();
    Some((inner, &after[close + 1..]))
}

fn parse_divider(line: &str) -> Option<String> {
    let inner = line.strip_prefix("==")?.strip_suffix("==")?;
    let trimmed = inner.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn split_target_color(rest: &str) -> (&str, Option<String>) {
    if let Some((before, after)) = rest.rsplit_once(char::is_whitespace) {
        if after.starts_with('#') {
            return (before.trim(), Some(after.to_string()));
        }
    }
    (rest, None)
}

fn parse_fragment_start(line: &str) -> Option<(FragmentKind, Option<String>)> {
    let (head, rest) = match line.split_once(char::is_whitespace) {
        Some((h, r)) => (h, r.trim()),
        None => (line, ""),
    };
    let kind = FragmentKind::from_keyword(head)?;
    let stripped = strip_color_prefixes(rest);
    Some((
        kind,
        if stripped.is_empty() {
            None
        } else {
            Some(stripped.to_string())
        },
    ))
}

/// Strip leading `#color ` tokens used by PUML to tint fragment headers.
fn strip_color_prefixes(s: &str) -> &str {
    let mut s = s.trim_start();
    while let Some(rest) = s.strip_prefix('#') {
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let after = &rest[end..];
        if after.is_empty() {
            return rest[end..].trim_start();
        }
        s = after.trim_start();
    }
    s
}

enum NoteParse {
    Single(Step),
    MultilineStart {
        position: NotePosition,
        participants: Vec<String>,
    },
}

fn parse_note_line(line: &str, line_no: usize) -> Option<NoteParse> {
    // Accept `note`, `rnote`, `hnote` (alternate styles) — keyword only.
    let after_kw = strip_prefix_keyword(line, "note")
        .or_else(|| strip_prefix_keyword(line, "rnote"))
        .or_else(|| strip_prefix_keyword(line, "hnote"))?
        .trim_start();

    if let Some(over_rest) = strip_prefix_keyword(after_kw, "over") {
        return Some(parse_note_over(over_rest.trim(), line_no));
    }
    if let Some(rest) = strip_prefix_keyword(after_kw, "left") {
        return Some(parse_note_side(NotePosition::LeftOf, rest.trim(), line_no));
    }
    if let Some(rest) = strip_prefix_keyword(after_kw, "right") {
        return Some(parse_note_side(NotePosition::RightOf, rest.trim(), line_no));
    }
    None
}

fn parse_note_over(rest: &str, line_no: usize) -> NoteParse {
    let (targets, label) = match rest.split_once(':') {
        Some((t, l)) => (t.trim(), Some(l.trim().to_string())),
        None => (rest.trim(), None),
    };
    let participants: Vec<String> = if targets.is_empty() {
        Vec::new()
    } else {
        targets.split(',').map(|t| t.trim().to_string()).collect()
    };
    match label {
        Some(text) => NoteParse::Single(Step::Note {
            position: NotePosition::Over,
            participants,
            text,
            line: line_no,
        }),
        None => NoteParse::MultilineStart {
            position: NotePosition::Over,
            participants,
        },
    }
}

fn parse_note_side(position: NotePosition, rest: &str, line_no: usize) -> NoteParse {
    // `note left of X : t`, `note right : t`, etc. We keep target list empty
    // when none was given — codegen serializes back to `note left/right` and
    // lets seq-puml resolve `__last__`.
    let rest = rest.strip_prefix("of").map(str::trim_start).unwrap_or(rest);
    let (targets, label) = match rest.split_once(':') {
        Some((t, l)) => (t.trim(), Some(l.trim().to_string())),
        None => (rest.trim(), None),
    };
    let participants: Vec<String> = if targets.is_empty() {
        Vec::new()
    } else {
        targets.split(',').map(|t| t.trim().to_string()).collect()
    };
    match label {
        Some(text) => NoteParse::Single(Step::Note {
            position,
            participants,
            text,
            line: line_no,
        }),
        None => NoteParse::MultilineStart {
            position,
            participants,
        },
    }
}

fn parse_message(line: &str, line_no: usize) -> Option<Step> {
    let split = split_arrow(line)?;
    let from = split.from.to_string();
    let arrow = split.arrow.to_string();

    // After the arrow we have: <to-token> [<suffix>] [: label]
    let (to_token, label) = match split.rest.split_once(':') {
        Some((t, l)) => (t.trim(), Some(l.trim().to_string())),
        None => (split.rest.trim(), None),
    };
    if to_token.is_empty() {
        return None;
    }
    // Strip a trailing suffix (`+`, `-`, `*`, `!`, or combos). We drop it on
    // the floor — seq-puml ignores it on most arrows, and the activate
    // semantics we'd need round-trip via explicit `activate`/`deactivate`.
    let to = strip_message_suffix(to_token).to_string();

    Some(Step::Message {
        from,
        to,
        arrow,
        label,
        line: line_no,
    })
}

fn strip_message_suffix(tok: &str) -> &str {
    tok.trim_end_matches(|c: char| matches!(c, '+' | '-' | '*' | '!'))
}

struct ArrowSplit<'a> {
    from: &'a str,
    arrow: &'a str,
    rest: &'a str,
}

/// Find the `<from> <arrow> <rest>` shape on a single line.
///
/// PlantUML accepts both spaced (`A -> B`) and unspaced (`A->B`) arrows, so
/// we scan for runs of arrow-shaft chars (`-<>/\\`, optionally with `[…]`
/// for inline colors) rather than splitting on whitespace. A run only
/// counts as an arrow if it actually contains `-`; runs that turn out to be
/// just `<` or `>` (e.g. inside an angle-bracketed identifier) get skipped
/// and scanning continues.
///
/// `o` / `x` arrow heads (`->o`, `->x`) need whitespace around the arrow to
/// be recognised, since both letters are otherwise valid identifier chars
/// (`Otto->Alice` is unambiguously `from=Otto, arrow=->`). With whitespace,
/// the leading/trailing alphanumeric boundary disambiguates and the head
/// gets folded into the arrow token below.
fn split_arrow(line: &str) -> Option<ArrowSplit<'_>> {
    let bytes = line.as_bytes();
    let mut search = 0;

    loop {
        // Skip ahead to the next arrow-shaft char.
        while search < bytes.len() && !is_arrow_shaft(bytes[search]) {
            search += 1;
        }
        if search >= bytes.len() {
            return None;
        }
        let start = search;

        // Extend through contiguous shaft chars + inline `[...]` color blocks.
        let mut end = start;
        while end < bytes.len() {
            let b = bytes[end];
            if is_arrow_shaft(b) {
                end += 1;
            } else if b == b'[' {
                let mut j = end + 1;
                while j < bytes.len() && bytes[j] != b']' {
                    j += 1;
                }
                if j >= bytes.len() {
                    break; // unterminated `[`; bail on this run
                }
                end = j + 1;
            } else {
                break;
            }
        }

        // Whitespace-bounded arrows can extend through `o` / `x` heads on
        // either side (e.g. `->o`, `x<-`). Without whitespace, the same
        // letters could be part of an identifier, so we only fold them in
        // when separated from the surrounding text by whitespace.
        let mut arrow_start = start;
        let mut arrow_end = end;
        if arrow_start > 0
            && matches!(bytes[arrow_start - 1], b'o' | b'x')
            && (arrow_start == 1 || bytes[arrow_start - 2].is_ascii_whitespace())
        {
            arrow_start -= 1;
        }
        if arrow_end < bytes.len()
            && matches!(bytes[arrow_end], b'o' | b'x')
            && (arrow_end + 1 == bytes.len() || bytes[arrow_end + 1].is_ascii_whitespace())
        {
            arrow_end += 1;
        }

        let candidate = &line[arrow_start..arrow_end];
        if !is_arrow_token(candidate) {
            // e.g. a lone `<` from `Class<Generic>` — skip past and try again.
            search = end.max(start + 1);
            continue;
        }

        let from = line[..arrow_start].trim();
        let rest = line[arrow_end..].trim_start();
        if from.is_empty() || rest.is_empty() {
            return None;
        }
        return Some(ArrowSplit {
            from,
            arrow: candidate,
            rest,
        });
    }
}

fn is_arrow_shaft(b: u8) -> bool {
    matches!(b, b'-' | b'<' | b'>' | b'/' | b'\\')
}

fn is_arrow_token(s: &str) -> bool {
    if !s.contains('-') {
        return false;
    }
    // Allow only arrow-shape chars plus an inline `[#color]`. Walk byte by byte.
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if matches!(b, b'-' | b'<' | b'>' | b'o' | b'x' | b'/' | b'\\') {
            i += 1;
            continue;
        }
        if b == b'[' {
            // Skip until matching `]`.
            i += 1;
            while i < bytes.len() && bytes[i] != b']' {
                i += 1;
            }
            if i >= bytes.len() {
                return false;
            }
            i += 1;
            continue;
        }
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(body: &[&str]) -> UmlBlock {
        UmlBlock {
            start_line: 1,
            kind_tag: "uml".into(),
            name: None,
            body: body
                .iter()
                .enumerate()
                .map(|(i, t)| BodyLine {
                    line: i + 1,
                    text: (*t).to_string(),
                })
                .collect(),
        }
    }

    fn parse_ok(body: &[&str]) -> StructuredSequence {
        let (diagram, _) = parse(&block(body), CompatMode::Warn).expect("parse ok");
        match diagram {
            Diagram::Sequence(SequenceDiagram::Structured(s)) => s,
            _ => panic!("expected structured sequence"),
        }
    }

    #[test]
    fn parses_basic_message() {
        let s = parse_ok(&["A -> B : hi"]);
        assert_eq!(s.steps.len(), 1);
        match &s.steps[0] {
            Step::Message {
                from,
                to,
                arrow,
                label,
                ..
            } => {
                assert_eq!(from, "A");
                assert_eq!(to, "B");
                assert_eq!(arrow, "->");
                assert_eq!(label.as_deref(), Some("hi"));
            }
            other => panic!("expected message, got {other:?}"),
        }
    }

    #[test]
    fn parses_color_arrow() {
        let s = parse_ok(&["Alice -[#red]-> Bob : hello"]);
        match &s.steps[0] {
            Step::Message { arrow, .. } => assert_eq!(arrow, "-[#red]->"),
            _ => panic!(),
        }
    }

    #[test]
    fn parses_unspaced_arrow() {
        // PlantUML accepts arrows without whitespace around them — we used
        // to drop these as "unsupported" because the declared participants
        // then looked unreferenced to the seq-puml validator downstream.
        let s = parse_ok(&[
            "actor Bob #red",
            "participant Alice",
            "Alice->Bob: Authentication Request",
            "Bob->Alice: Authentication Response",
        ]);
        assert_eq!(s.steps.len(), 2, "expected both messages parsed");
        match &s.steps[0] {
            Step::Message { from, to, arrow, label, .. } => {
                assert_eq!(from, "Alice");
                assert_eq!(to, "Bob");
                assert_eq!(arrow, "->");
                assert_eq!(label.as_deref(), Some("Authentication Request"));
            }
            _ => panic!("expected message step"),
        }
    }

    #[test]
    fn identifier_ending_in_letter_does_not_swallow_arrow_head() {
        // Without whitespace, the trailing `o` belongs to the identifier
        // `Otto`, not the arrow head — we only fold `o`/`x` into the arrow
        // when separated by whitespace.
        let s = parse_ok(&["Otto->Alice : hi"]);
        match &s.steps[0] {
            Step::Message { from, to, arrow, .. } => {
                assert_eq!(from, "Otto");
                assert_eq!(arrow, "->");
                assert_eq!(to, "Alice");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn parses_o_head_arrow_with_spaces() {
        // With whitespace around the arrow, the `o` head is unambiguous.
        let s = parse_ok(&["Alice ->o Bob : hi"]);
        match &s.steps[0] {
            Step::Message { arrow, to, .. } => {
                assert_eq!(arrow, "->o");
                assert_eq!(to, "Bob");
            }
            _ => panic!(),
        }
    }

    #[test]
    fn participants_with_alias() {
        let s = parse_ok(&[
            r#"participant "Alice 张" as A"#,
            "actor Bob",
            "A -> Bob : hi",
        ]);
        assert_eq!(s.participants.len(), 2);
        assert_eq!(s.participants[0].id, "A");
        assert_eq!(s.participants[0].display, "Alice 张");
        assert_eq!(s.participants[0].kind, ParticipantKind::Participant);
        assert_eq!(s.participants[1].id, "Bob");
        assert_eq!(s.participants[1].kind, ParticipantKind::Actor);
    }

    #[test]
    fn participants_with_unquoted_alias() {
        // PlantUML: `participant <Display> as <Id>` — the token after `as`
        // is the alias used in messages.
        let s = parse_ok(&[
            "participant Participant as Foo",
            "actor Actor as Foo1",
            "Foo -> Foo1 : hi",
        ]);
        assert_eq!(s.participants.len(), 2);
        assert_eq!(s.participants[0].id, "Foo");
        assert_eq!(s.participants[0].display, "Participant");
        assert_eq!(s.participants[1].id, "Foo1");
        assert_eq!(s.participants[1].display, "Actor");
    }

    #[test]
    fn fragment_alt_with_else_and_nested() {
        let s = parse_ok(&[
            "alt cond",
            "  A -> B : x",
            "  loop forever",
            "    B -> A : y",
            "  end",
            "else other",
            "  A -> B : z",
            "end",
        ]);
        assert_eq!(s.steps.len(), 1);
        match &s.steps[0] {
            Step::Fragment { kind, branches, .. } => {
                assert_eq!(*kind, FragmentKind::Alt);
                assert_eq!(branches.len(), 2);
                // First branch has 2 steps: a message and a nested loop fragment.
                assert_eq!(branches[0].steps.len(), 2);
                assert!(matches!(
                    branches[0].steps[1],
                    Step::Fragment {
                        kind: FragmentKind::Loop,
                        ..
                    }
                ));
                assert_eq!(branches[1].label.as_deref(), Some("other"));
            }
            _ => panic!("expected fragment"),
        }
    }

    #[test]
    fn note_over_two_participants() {
        let s = parse_ok(&["A -> B : x", "note over A, B : a comment"]);
        match s.steps.last().unwrap() {
            Step::Note {
                participants,
                text,
                position,
                ..
            } => {
                assert_eq!(*position, NotePosition::Over);
                assert_eq!(participants, &["A".to_string(), "B".to_string()]);
                assert_eq!(text, "a comment");
            }
            _ => panic!("expected note"),
        }
    }

    #[test]
    fn multiline_note_accumulates() {
        let s = parse_ok(&[
            "note over A",
            "  line one",
            "  line two",
            "end note",
            "A -> B : after",
        ]);
        match &s.steps[0] {
            Step::Note { text, .. } => {
                assert!(text.contains("line one"));
                assert!(text.contains("line two"));
            }
            _ => panic!("expected note"),
        }
    }

    #[test]
    fn skinparam_collected() {
        let s = parse_ok(&["skinparam backgroundColor #EEE", "A -> B : x"]);
        assert_eq!(s.skinparams.len(), 1);
        assert_eq!(s.skinparams[0].key, "backgroundColor");
        assert_eq!(s.skinparams[0].value, "#EEE");
    }

    #[test]
    fn divider_and_autonumber() {
        let s = parse_ok(&[
            "autonumber 10 5",
            "A -> B : x",
            "== checkpoint ==",
            "B -> A : y",
        ]);
        assert!(matches!(s.steps[0], Step::Autonumber { .. }));
        assert!(matches!(s.steps[2], Step::Divider { .. }));
    }

    #[test]
    fn unrecognized_line_emits_warning() {
        let (_diagram, diags) =
            parse(&block(&["frobnicate the foozle"]), CompatMode::Warn).unwrap();
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, Level::Warning);
    }

    #[test]
    fn strict_mode_fails_on_unrecognized() {
        let res = parse(&block(&["frobnicate the foozle"]), CompatMode::Strict);
        assert!(res.is_err());
    }
}
