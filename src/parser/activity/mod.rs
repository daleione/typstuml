//! Activity-diagram parser (PlantUML `activitydiagram3` / new syntax).
//!
//! Recursive descent over the lexer's line stream. The block-oriented syntax
//! (`if … endif` / `repeat … repeat while` / `while … endwhile` /
//! `fork … end fork` / `split … end split` / `switch … endswitch` /
//! `partition { … }` / `note … end note`) is nested by design, so we keep a
//! mutable cursor and call back into sub-parsers that consume up to the
//! matching terminator keyword.
//!
//! Out-of-scope for the first cut (see `docs/activity-diagram-design.md`):
//!   - swimlanes are captured into the IR (`Stmt::SwimlaneSwitch`) but the
//!     codegen drops them;
//!   - notes are captured but the codegen drops them;
//!   - the legacy `(*) -> "X"` arrow syntax warns and skips;
//!   - `goto` / `label` / `break` are captured but the codegen drops them.
//!
//! Errors degrade to warnings under `--compat warn` (default) and become
//! `Error::Parse` under `--compat strict`, matching the mindmap / cuca parser.

use crate::diagnostics::{CompatMode, Diagnostic, Error, Level, Result};
use crate::ir::{
    ActionKind, ActivityDiagram, ActivityStmt, Diagram, LayoutDirection, NoteAttach, Skinparam,
};
use crate::parser::lexer::{BodyLine, UmlBlock};

mod blocks;
mod scan;

#[cfg(test)]
mod tests;

use scan::*;

pub fn parse(block: &UmlBlock, compat: CompatMode) -> Result<(Diagram, Vec<Diagnostic>)> {
    let mut parser = Parser::new(&block.body, compat);
    parser.skip_preamble()?;
    let (body, _term) = parser.parse_stmts(Terminator::Eof)?;
    let mut diag = parser.diag;
    diag.name = block.name.clone();
    diag.body = body;
    Ok((Diagram::Activity(diag), parser.diagnostics))
}

struct Parser<'a> {
    lines: &'a [BodyLine],
    pos: usize,
    compat: CompatMode,
    diag: ActivityDiagram,
    diagnostics: Vec<Diagnostic>,
    /// Edge label sitting on the `pending_arrow` slot — attached to the
    /// next action when it lands.
    pending_arrow: Option<String>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Terminator {
    Eof,
    EndIf,
    Else,
    ElseIf,
    EndWhile,
    RepeatWhile,
    EndFork,
    ForkAgain,
    EndSplit,
    SplitAgain,
    EndSwitch,
    Case,
    BraceClose,
    EndNote,
}

#[derive(Clone, Debug)]
struct TerminatorHit {
    /// Which terminator keyword closed this statement list.
    kind: Terminator,
    /// Raw line that produced it (e.g. `else (no)`), trimmed.
    raw: String,
    line: usize,
}

impl<'a> Parser<'a> {
    fn new(lines: &'a [BodyLine], compat: CompatMode) -> Self {
        Self {
            lines,
            pos: 0,
            compat,
            diag: ActivityDiagram::default(),
            diagnostics: Vec::new(),
            pending_arrow: None,
        }
    }

    fn warn(&mut self, line: usize, msg: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            level: Level::Warning,
            line: Some(line),
            message: msg.into(),
        });
    }

    /// Run the small directive head before the first statement. Handles
    /// `title`, `skinparam`, `hide/show`, `!theme`, `left to right
    /// direction` — anything that isn't a real activity statement.
    fn skip_preamble(&mut self) -> Result<()> {
        while self.pos < self.lines.len() {
            let raw = self.lines[self.pos].text.trim();
            if raw.is_empty() || is_comment(raw) {
                self.pos += 1;
                continue;
            }
            let line_no = self.lines[self.pos].line;
            if let Some(rest) = strip_kw(raw, "title") {
                let t = rest.trim();
                if !t.is_empty() {
                    self.diag.title = Some(t.to_string());
                }
                self.pos += 1;
                continue;
            }
            if strip_kw(raw, "caption").is_some()
                || strip_kw(raw, "header").is_some()
                || strip_kw(raw, "footer").is_some()
            {
                self.pos += 1;
                continue;
            }
            if let Some(rest) = strip_kw(raw, "skinparam") {
                self.handle_skinparam(rest, line_no);
                self.pos += 1;
                continue;
            }
            if raw.starts_with("hide ")
                || raw.starts_with("show ")
                || raw.starts_with("!theme")
                || raw.starts_with("!pragma")
                || raw == "hide"
                || raw == "show"
            {
                self.pos += 1;
                continue;
            }
            if raw.starts_with("left to right direction") || raw == "left to right" {
                self.diag.direction = LayoutDirection::LeftToRight;
                self.pos += 1;
                continue;
            }
            if raw.starts_with("top to bottom direction") || raw == "top to bottom" {
                self.diag.direction = LayoutDirection::TopToBottom;
                self.pos += 1;
                continue;
            }
            // Unrecognised — bail out of preamble; the main parser loop
            // takes over from here.
            return Ok(());
        }
        Ok(())
    }

    fn handle_skinparam(&mut self, rest: &str, line_no: usize) {
        let t = rest.trim();
        if t.is_empty() {
            return;
        }
        // Block form `skinparam <prefix> { … }` is not supported in M0 —
        // PlantUML lets these scope to a sub-key family. Drop with a
        // warning, mirroring sequence parser behaviour.
        if t.ends_with('{') {
            self.warn(line_no, "skinparam block form is not yet supported");
            return;
        }
        let mut it = t.splitn(2, char::is_whitespace);
        let key = it.next().unwrap_or("").to_string();
        let value = it.next().unwrap_or("").trim().to_string();
        if key.is_empty() {
            return;
        }
        self.diag.skinparams.push(Skinparam {
            key,
            value,
            line: line_no,
        });
    }

    /// Parse statements until any of `until`'s siblings is hit. Returns the
    /// gathered body plus the terminator that closed it. Unexpected
    /// terminators inside nested blocks raise diagnostics but do not
    /// abort recovery.
    fn parse_stmts(&mut self, until: Terminator) -> Result<(Vec<ActivityStmt>, TerminatorHit)> {
        let mut body = Vec::new();
        while self.pos < self.lines.len() {
            let raw = self.lines[self.pos].text.trim().to_string();
            let line_no = self.lines[self.pos].line;

            if raw.is_empty() || is_comment(&raw) {
                self.pos += 1;
                continue;
            }
            // Per-block terminators come first so a stray `endif` doesn't
            // get misclassified as a stray keyword.
            if let Some(term) = match_terminator(&raw) {
                if terminator_is_compatible(until, term) {
                    self.pos += 1;
                    return Ok((
                        body,
                        TerminatorHit {
                            kind: term,
                            raw,
                            line: line_no,
                        },
                    ));
                }
                // Terminator doesn't match — warn and skip so we keep
                // parsing rather than wedging on a misplaced keyword.
                self.warn(
                    line_no,
                    format!("unexpected `{}` here; dropped", terminator_keyword(term)),
                );
                if self.compat == CompatMode::Strict {
                    return Err(Error::Parse {
                        line: line_no,
                        message: format!("unexpected `{}`", terminator_keyword(term)),
                    });
                }
                self.pos += 1;
                continue;
            }

            // Recognise the various statement openers. Order matters —
            // more specific prefixes before more general ones.
            if raw == "start" {
                self.pos += 1;
                body.push(ActivityStmt::Start { line: line_no });
                continue;
            }
            if raw == "stop" {
                self.pos += 1;
                body.push(ActivityStmt::Stop { line: line_no });
                continue;
            }
            if raw == "end" {
                self.pos += 1;
                body.push(ActivityStmt::End { line: line_no });
                continue;
            }
            if raw == "detach" || raw == "kill" {
                self.pos += 1;
                body.push(ActivityStmt::Detach { line: line_no });
                continue;
            }
            if raw == "break" {
                self.pos += 1;
                body.push(ActivityStmt::Break { line: line_no });
                continue;
            }
            if let Some(rest) = strip_kw(&raw, "label") {
                // `label foo` — register a goto target.
                let name = rest.trim().trim_end_matches(';').trim().to_string();
                if !name.is_empty() {
                    body.push(ActivityStmt::GotoLabel {
                        name,
                        line: line_no,
                    });
                }
                self.pos += 1;
                continue;
            }
            if let Some(rest) = strip_kw(&raw, "goto") {
                let name = rest.trim().trim_end_matches(';').trim().to_string();
                if !name.is_empty() {
                    body.push(ActivityStmt::Goto {
                        name,
                        line: line_no,
                    });
                }
                self.pos += 1;
                continue;
            }
            if raw.starts_with("if (") || raw.starts_with("if(") {
                let stmt = self.parse_if(line_no)?;
                body.push(stmt);
                continue;
            }
            if raw.starts_with("while (") || raw.starts_with("while(") {
                let stmt = self.parse_while(line_no)?;
                body.push(stmt);
                continue;
            }
            if raw == "repeat" || raw.starts_with("repeat :") {
                let stmt = self.parse_repeat(line_no)?;
                body.push(stmt);
                continue;
            }
            if raw == "fork" || raw == "fork;" {
                let stmt = self.parse_fork(line_no)?;
                body.push(stmt);
                continue;
            }
            if raw == "split" || raw == "split;" {
                let stmt = self.parse_split(line_no)?;
                body.push(stmt);
                continue;
            }
            if raw.starts_with("switch (") || raw.starts_with("switch(") {
                let stmt = self.parse_switch(line_no)?;
                body.push(stmt);
                continue;
            }
            if let Some(kind) = partition_kind(&raw) {
                if raw.ends_with('{') {
                    let stmt = self.parse_partition(kind, &raw, line_no)?;
                    body.push(stmt);
                    continue;
                }
            }
            // `|Lane|` swimlane switch — keep an explicit case so it doesn't
            // accidentally collide with arrow / action prefixes.
            if let Some(switch) = parse_swimlane(&raw, line_no) {
                body.push(switch);
                self.pos += 1;
                continue;
            }
            // Arrow modifier `->[#color] label;`. PlantUML allows it before
            // the next action; we stash the label and apply it.
            if raw.starts_with("->") || raw.starts_with("-[") {
                if let Some(label) = parse_arrow_label(&raw) {
                    self.pending_arrow = Some(label);
                }
                self.pos += 1;
                continue;
            }
            if raw.starts_with(':') {
                let stmt = self.parse_action(line_no)?;
                body.push(stmt);
                continue;
            }
            if raw.starts_with("note ") || raw.starts_with("floating note") {
                let attach = self.parse_note(&raw, line_no)?;
                if let Some(attach) = attach {
                    if let Some(prev) = last_action_mut(&mut body) {
                        prev.push(attach);
                    } else {
                        self.warn(line_no, "note without a preceding action; dropped");
                    }
                }
                continue;
            }
            if raw.starts_with("(*)") || raw.starts_with("-->") {
                // Legacy `(*) -> "X"` or `--> "Y"` activity beta syntax.
                self.warn(
                    line_no,
                    "legacy activity arrow syntax is not supported; skipped",
                );
                self.pos += 1;
                continue;
            }

            // Anything else is unknown.
            let msg = format!("unknown activity statement: `{raw}`");
            if self.compat == CompatMode::Strict {
                return Err(Error::Parse {
                    line: line_no,
                    message: msg,
                });
            }
            self.warn(line_no, msg);
            self.pos += 1;
        }

        // EOF before we saw the requested terminator. For top-level
        // `Eof` that's expected; for inner blocks it's an unterminated
        // construct.
        if until == Terminator::Eof {
            Ok((
                body,
                TerminatorHit {
                    kind: Terminator::Eof,
                    raw: String::new(),
                    line: self.last_line(),
                },
            ))
        } else {
            self.warn(
                self.last_line(),
                format!(
                    "missing `{}`; auto-closing at end of block",
                    terminator_keyword(until)
                ),
            );
            Ok((
                body,
                TerminatorHit {
                    kind: until,
                    raw: String::new(),
                    line: self.last_line(),
                },
            ))
        }
    }

    fn last_line(&self) -> usize {
        self.lines.last().map(|l| l.line).unwrap_or(0)
    }

    fn parse_action(&mut self, line_no: usize) -> Result<ActivityStmt> {
        // Pull lines until we hit a `;` terminator. Action labels may span
        // multiple physical lines (PlantUML CommandActivityLong3). Color
        // prefix `#color :label;` is treated as deprecated in PlantUML
        // and warned about — but we still accept it.
        let mut label_lines: Vec<String> = Vec::new();
        let mut color: Option<String> = None;
        let mut url: Option<String> = None;
        let mut kind = ActionKind::Rectangle;

        // First line: strip `#color` prefix if present, then the leading `:`.
        let first = self.lines[self.pos].text.trim().to_string();
        self.pos += 1;
        let mut first_body = first.as_str();
        if let Some(rest) = first_body.strip_prefix('#') {
            // `#LightBlue :label;` — color prefix (deprecated).
            let mut it = rest.splitn(2, char::is_whitespace);
            if let Some(c) = it.next() {
                color = Some(format!("#{c}"));
            }
            first_body = it.next().unwrap_or("").trim_start();
        }
        let first_after_colon = first_body
            .strip_prefix(':')
            .map(str::to_string)
            .unwrap_or_else(|| first_body.to_string());

        // Look for `;` on the first line. If present, label is single-line
        // (everything before `;`); the suffix after `;` may carry
        // stereotype / URL.
        let mut suffix = String::new();
        if let Some(idx) = first_after_colon.find(';') {
            let head = first_after_colon[..idx].to_string();
            if !head.is_empty() {
                label_lines.push(head);
            } else {
                label_lines.push(String::new());
            }
            suffix = first_after_colon[idx + 1..].trim().to_string();
        } else {
            label_lines.push(first_after_colon);
            // Keep eating lines until we see `;`.
            while self.pos < self.lines.len() {
                let raw = self.lines[self.pos].text.clone();
                self.pos += 1;
                let trimmed_end = raw.trim_end();
                if let Some(idx) = trimmed_end.find(';') {
                    let head = trimmed_end[..idx].to_string();
                    if !head.is_empty() {
                        label_lines.push(head);
                    }
                    suffix = trimmed_end[idx + 1..].trim().to_string();
                    break;
                }
                label_lines.push(trimmed_end.to_string());
            }
        }

        // Parse trailing `<<stereotype>>` (shape) and `[[url]]` from `suffix`.
        let mut rest = suffix.as_str().trim();
        while !rest.is_empty() {
            if let Some(after_open) = rest.strip_prefix("<<") {
                if let Some(close) = after_open.find(">>") {
                    let stereo = after_open[..close].trim();
                    kind = ActionKind::from_stereotype(stereo);
                    rest = after_open[close + 2..].trim_start();
                    continue;
                }
            }
            if let Some(after_open) = rest.strip_prefix("[[") {
                if let Some(close) = after_open.find("]]") {
                    let inside = after_open[..close].trim();
                    // PlantUML allows `[[url label]]`; take just the URL.
                    let u = inside.split_whitespace().next().unwrap_or("").to_string();
                    if !u.is_empty() {
                        url = Some(u);
                    }
                    rest = after_open[close + 2..].trim_start();
                    continue;
                }
            }
            // Unknown trailing token — drop quietly to avoid noise.
            break;
        }

        let edge_label = self.pending_arrow.take();

        Ok(ActivityStmt::Action {
            label: label_lines,
            kind,
            color,
            url,
            notes: Vec::new(),
            edge_label,
            line: line_no,
        })
    }
    /// Parse a `note` or `floating note` line. Returns the attach when the
    /// note belongs to a previous action; `None` when the note is
    /// floating / unsupported and we just dropped it with a warning.
    fn parse_note(&mut self, raw: &str, line_no: usize) -> Result<Option<NoteAttach>> {
        if raw.starts_with("floating note") {
            self.warn(line_no, "floating note is not yet supported; dropped");
            self.pos += 1;
            return Ok(None);
        }
        // `note left :body;` (single line) or `note left … end note` (block).
        let after = raw.strip_prefix("note").unwrap_or(raw).trim_start();
        let (side, after) = split_note_side(after);
        if side.is_none() {
            self.warn(
                line_no,
                "note here must be `note left` or `note right`; dropped",
            );
            self.pos += 1;
            return Ok(None);
        }
        let side = side.unwrap();
        // Single-line form: `note left : body`
        if let Some(idx) = after.find(':') {
            let body = after[idx + 1..].trim().to_string();
            self.pos += 1;
            return Ok(Some(NoteAttach {
                position: side,
                text: vec![body],
                line: line_no,
            }));
        }
        // Block form: collect lines until `end note`.
        self.pos += 1;
        let mut text: Vec<String> = Vec::new();
        while self.pos < self.lines.len() {
            let l = &self.lines[self.pos];
            let t = l.text.trim();
            self.pos += 1;
            if t == "end note" || t == "endnote" {
                break;
            }
            text.push(l.text.trim_end().to_string());
        }
        Ok(Some(NoteAttach {
            position: side,
            text,
            line: line_no,
        }))
    }
}
