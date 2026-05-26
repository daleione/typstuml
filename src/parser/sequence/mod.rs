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
mod fragment;
mod message;
mod note;
mod participant;
mod scan;
#[cfg(test)]
mod tests;

use crate::parser::common::{is_comment, strip_leading_quoted, strip_prefix_keyword};
use crate::parser::lexer::{BodyLine, UmlBlock};

use fragment::parse_fragment_start;
use message::{parse_message, split_target_color};
use note::{parse_note_line, NoteParse};
use participant::{parse_participant, strip_participant_block_open};
use scan::{
    is_end_note, is_skip_directive, parse_divider, strip_color_prefixes, strip_inline_comment,
    unescape_display,
};

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
