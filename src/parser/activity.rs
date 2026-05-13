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
    ActionKind, ActivityDiagram, ActivityStmt, Diagram, ElseIfBranch, LayoutDirection, NoteAttach,
    NotePosition, PartitionKind, Skinparam, SwitchCase,
};
use crate::parser::lexer::{BodyLine, UmlBlock};

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

    fn parse_if(&mut self, line_no: usize) -> Result<ActivityStmt> {
        let raw = self.lines[self.pos].text.trim().to_string();
        self.pos += 1;
        let (cond, then_label) = parse_if_head(&raw);

        let (then_branch, term) = self.parse_stmts(Terminator::EndIf)?;
        let mut elseifs: Vec<ElseIfBranch> = Vec::new();
        let mut else_label: Option<String> = None;
        let mut else_branch: Option<Vec<ActivityStmt>> = None;
        let mut hit = term;

        loop {
            match hit.kind {
                Terminator::ElseIf => {
                    let (cond, label) = parse_elseif_head(&hit.raw);
                    let (br, next) = self.parse_stmts(Terminator::EndIf)?;
                    elseifs.push(ElseIfBranch {
                        cond,
                        label,
                        branch: br,
                        line: hit.line,
                    });
                    hit = next;
                }
                Terminator::Else => {
                    else_label = parse_else_label(&hit.raw);
                    let (br, next) = self.parse_stmts(Terminator::EndIf)?;
                    else_branch = Some(br);
                    hit = next;
                }
                Terminator::EndIf | Terminator::Eof => break,
                _ => break,
            }
        }

        Ok(ActivityStmt::If {
            cond,
            then_label,
            then_branch,
            elseifs,
            else_label,
            else_branch,
            line: line_no,
        })
    }

    fn parse_while(&mut self, line_no: usize) -> Result<ActivityStmt> {
        let raw = self.lines[self.pos].text.trim().to_string();
        self.pos += 1;
        let (cond, is_label) = parse_while_head(&raw);
        let (body, term) = self.parse_stmts(Terminator::EndWhile)?;
        let not_label = parse_endwhile_label(&term.raw);
        Ok(ActivityStmt::While {
            cond,
            is_label,
            not_label,
            body,
            line: line_no,
        })
    }

    fn parse_repeat(&mut self, line_no: usize) -> Result<ActivityStmt> {
        let raw = self.lines[self.pos].text.trim().to_string();
        self.pos += 1;
        // First-action sugar: `repeat :foo;` opens the loop and emits
        // `:foo;` as the first body statement. We synthesise that.
        let mut body: Vec<ActivityStmt> = Vec::new();
        if let Some(label_with_semi) = raw.strip_prefix("repeat :") {
            let label = label_with_semi.trim_end_matches(';').to_string();
            body.push(ActivityStmt::Action {
                label: vec![label],
                kind: ActionKind::Rectangle,
                color: None,
                url: None,
                notes: Vec::new(),
                edge_label: self.pending_arrow.take(),
                line: line_no,
            });
        }

        // Gather body until `repeat while (…)` arrives. We allow a
        // `backward :label;` directive inside the loop — sniff it as we
        // pull statements.
        let (mut more_body, term) = self.parse_stmts(Terminator::RepeatWhile)?;
        body.append(&mut more_body);
        let backward = take_backward(&mut body);

        let (cond, is_label, not_label) = parse_repeat_while_head(&term.raw);
        Ok(ActivityStmt::Repeat {
            body,
            backward,
            cond,
            is_label,
            not_label,
            line: line_no,
        })
    }

    fn parse_fork(&mut self, line_no: usize) -> Result<ActivityStmt> {
        // Consume the `fork` opener already at self.pos.
        self.pos += 1;
        let mut branches: Vec<Vec<ActivityStmt>> = Vec::new();
        let mut merge = true;
        loop {
            let (br, term) = self.parse_stmts(Terminator::EndFork)?;
            branches.push(br);
            match term.kind {
                Terminator::ForkAgain => continue,
                Terminator::EndFork => {
                    merge = parse_fork_end_merge(&term.raw);
                    break;
                }
                _ => break,
            }
        }
        Ok(ActivityStmt::Fork {
            branches,
            merge,
            line: line_no,
        })
    }

    fn parse_split(&mut self, line_no: usize) -> Result<ActivityStmt> {
        self.pos += 1;
        let mut branches: Vec<Vec<ActivityStmt>> = Vec::new();
        let merge = true;
        loop {
            let (br, term) = self.parse_stmts(Terminator::EndSplit)?;
            branches.push(br);
            match term.kind {
                Terminator::SplitAgain => continue,
                Terminator::EndSplit => break,
                _ => break,
            }
        }
        Ok(ActivityStmt::Split {
            branches,
            merge,
            line: line_no,
        })
    }

    fn parse_switch(&mut self, line_no: usize) -> Result<ActivityStmt> {
        let raw = self.lines[self.pos].text.trim().to_string();
        self.pos += 1;
        let cond = parse_paren_arg(&raw, "switch").unwrap_or_default();

        let mut cases: Vec<SwitchCase> = Vec::new();
        // PlantUML silently allows statements between `switch (…)` and
        // the first `case (…)` — eat them under a synthetic empty case.
        let (intro, mut term) = self.parse_stmts(Terminator::Case)?;
        if !intro.is_empty() {
            cases.push(SwitchCase {
                value: String::new(),
                branch: intro,
                line: line_no,
            });
        }
        while term.kind == Terminator::Case {
            let value = parse_paren_arg(&term.raw, "case").unwrap_or_default();
            let case_line = term.line;
            let (branch, next) = self.parse_stmts(Terminator::Case)?;
            cases.push(SwitchCase {
                value,
                branch,
                line: case_line,
            });
            term = next;
        }

        Ok(ActivityStmt::Switch {
            cond,
            cases,
            line: line_no,
        })
    }

    fn parse_partition(
        &mut self,
        kind: PartitionKind,
        raw: &str,
        line_no: usize,
    ) -> Result<ActivityStmt> {
        self.pos += 1;
        let (label, color) = parse_partition_head(raw, kind);
        let (body, _term) = self.parse_stmts(Terminator::BraceClose)?;
        Ok(ActivityStmt::Partition {
            kind,
            label,
            color,
            body,
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

// --------------------------------------------------------------------------
// Small parsing helpers.
// --------------------------------------------------------------------------

fn is_comment(s: &str) -> bool {
    s.starts_with('\'') || s.starts_with("/'")
}

fn strip_kw<'s>(s: &'s str, kw: &str) -> Option<&'s str> {
    let bytes = s.as_bytes();
    if !s.starts_with(kw) {
        return None;
    }
    // Either the keyword is the whole string, or it's followed by
    // whitespace.
    if s.len() == kw.len() {
        return Some("");
    }
    let next = bytes[kw.len()];
    if next.is_ascii_whitespace() {
        Some(&s[kw.len() + 1..])
    } else {
        None
    }
}

fn match_terminator(raw: &str) -> Option<Terminator> {
    if raw == "endif" {
        return Some(Terminator::EndIf);
    }
    if raw == "else" || raw.starts_with("else ") || raw.starts_with("else(") {
        return Some(Terminator::Else);
    }
    if raw.starts_with("elseif (")
        || raw.starts_with("elseif(")
        || raw.starts_with("else if (")
        || raw.starts_with("else if(")
    {
        return Some(Terminator::ElseIf);
    }
    if raw == "endwhile"
        || raw.starts_with("endwhile ")
        || raw.starts_with("endwhile(")
        || raw.starts_with("endwhile(")
    {
        return Some(Terminator::EndWhile);
    }
    if raw.starts_with("repeat while") {
        return Some(Terminator::RepeatWhile);
    }
    if raw == "end fork"
        || raw == "endfork"
        || raw.starts_with("end fork ")
        || raw == "end merge"
    {
        return Some(Terminator::EndFork);
    }
    if raw == "fork again" {
        return Some(Terminator::ForkAgain);
    }
    if raw == "end split" || raw == "endsplit" {
        return Some(Terminator::EndSplit);
    }
    if raw == "split again" {
        return Some(Terminator::SplitAgain);
    }
    if raw == "endswitch" || raw == "end switch" {
        return Some(Terminator::EndSwitch);
    }
    if raw.starts_with("case (") || raw.starts_with("case(") {
        return Some(Terminator::Case);
    }
    if raw == "}" {
        return Some(Terminator::BraceClose);
    }
    if raw == "end note" || raw == "endnote" {
        return Some(Terminator::EndNote);
    }
    None
}

fn terminator_keyword(t: Terminator) -> &'static str {
    match t {
        Terminator::Eof => "<eof>",
        Terminator::EndIf => "endif",
        Terminator::Else => "else",
        Terminator::ElseIf => "elseif",
        Terminator::EndWhile => "endwhile",
        Terminator::RepeatWhile => "repeat while",
        Terminator::EndFork => "end fork",
        Terminator::ForkAgain => "fork again",
        Terminator::EndSplit => "end split",
        Terminator::SplitAgain => "split again",
        Terminator::EndSwitch => "endswitch",
        Terminator::Case => "case",
        Terminator::BraceClose => "}",
        Terminator::EndNote => "end note",
    }
}

/// Which terminator keywords are accepted while we're scanning under
/// `parent`. `Eof` accepts only `Eof` (everything else is unexpected at
/// top level). For nested blocks, the inner block accepts its own
/// matching closer plus the few "soft" siblings (`else`, `elseif`,
/// `case`, `fork again`, `split again`) so the caller can keep iterating.
fn terminator_is_compatible(parent: Terminator, candidate: Terminator) -> bool {
    if candidate == Terminator::Eof {
        return parent == Terminator::Eof;
    }
    match parent {
        Terminator::Eof => false,
        Terminator::EndIf => matches!(
            candidate,
            Terminator::EndIf | Terminator::Else | Terminator::ElseIf
        ),
        Terminator::EndWhile => candidate == Terminator::EndWhile,
        Terminator::RepeatWhile => candidate == Terminator::RepeatWhile,
        Terminator::EndFork => matches!(candidate, Terminator::EndFork | Terminator::ForkAgain),
        Terminator::EndSplit => matches!(candidate, Terminator::EndSplit | Terminator::SplitAgain),
        Terminator::EndSwitch | Terminator::Case => {
            matches!(candidate, Terminator::EndSwitch | Terminator::Case)
        }
        Terminator::BraceClose => candidate == Terminator::BraceClose,
        Terminator::EndNote => candidate == Terminator::EndNote,
        // ForkAgain / SplitAgain / Else / ElseIf themselves can't be
        // "parents" — they're soft transitions, not block openers.
        Terminator::ForkAgain
        | Terminator::SplitAgain
        | Terminator::Else
        | Terminator::ElseIf => false,
    }
}

fn partition_kind(raw: &str) -> Option<PartitionKind> {
    for (kw, kind) in [
        ("partition ", PartitionKind::Partition),
        ("package ", PartitionKind::Package),
        ("rectangle ", PartitionKind::Rectangle),
        ("card ", PartitionKind::Card),
        ("group ", PartitionKind::Group),
    ] {
        if raw.starts_with(kw) {
            return Some(kind);
        }
    }
    None
}

fn parse_partition_head(raw: &str, kind: PartitionKind) -> (String, Option<String>) {
    let kw = match kind {
        PartitionKind::Partition => "partition",
        PartitionKind::Package => "package",
        PartitionKind::Rectangle => "rectangle",
        PartitionKind::Card => "card",
        PartitionKind::Group => "group",
    };
    let rest = raw.strip_prefix(kw).unwrap_or(raw).trim_start();
    // Strip trailing `{`.
    let rest = rest.trim_end_matches('{').trim_end();
    // Optional leading `#color` between keyword and label.
    let (color, rest) = if let Some(after) = rest.strip_prefix('#') {
        let mut it = after.splitn(2, char::is_whitespace);
        let c = it.next().unwrap_or("");
        let r = it.next().unwrap_or("").trim_start();
        (Some(format!("#{c}")), r)
    } else {
        (None, rest)
    };
    // Quoted label `"foo bar"`.
    let label = if let Some(after) = rest.strip_prefix('"') {
        if let Some(end) = after.find('"') {
            after[..end].to_string()
        } else {
            after.to_string()
        }
    } else {
        rest.split_whitespace().next().unwrap_or("").to_string()
    };
    (label, color)
}

/// Parse the head of an `if (...)` opener and return `(cond, then_label)`.
fn parse_if_head(raw: &str) -> (String, Option<String>) {
    let after = raw.strip_prefix("if").unwrap_or(raw).trim_start();
    let (cond, rest) = take_paren(after);
    // Optional `then (label)` clause.
    let rest = rest.trim_start();
    let then_label = parse_then_clause(rest);
    (cond, then_label)
}

fn parse_elseif_head(raw: &str) -> (String, Option<String>) {
    let after = raw
        .strip_prefix("elseif")
        .or_else(|| raw.strip_prefix("else if"))
        .unwrap_or(raw)
        .trim_start();
    let (cond, rest) = take_paren(after);
    let then_label = parse_then_clause(rest.trim_start());
    (cond, then_label)
}

fn parse_then_clause(s: &str) -> Option<String> {
    // Common shapes: `then (foo)`, `(foo) then`, or omitted. Also accept
    // an optional `is (…)` clause (used by `if` / `while`) which carries
    // the affirmative label.
    let s = s.trim();
    if let Some(after) = s.strip_prefix("then") {
        let after = after.trim_start();
        if let Some(after_open) = after.strip_prefix('(') {
            if let Some(end) = after_open.find(')') {
                return Some(after_open[..end].trim().to_string());
            }
        }
        return None;
    }
    if let Some(after) = s.strip_prefix("is") {
        let after = after.trim_start();
        if let Some(after_open) = after.strip_prefix('(') {
            if let Some(end) = after_open.find(')') {
                return Some(after_open[..end].trim().to_string());
            }
        }
    }
    None
}

fn parse_else_label(raw: &str) -> Option<String> {
    let after = raw.strip_prefix("else").unwrap_or(raw).trim_start();
    let after = after.strip_prefix('(')?;
    let end = after.find(')')?;
    Some(after[..end].trim().to_string())
}

fn parse_while_head(raw: &str) -> (String, Option<String>) {
    let after = raw.strip_prefix("while").unwrap_or(raw).trim_start();
    let (cond, rest) = take_paren(after);
    let is_label = parse_then_clause(rest.trim_start());
    (cond, is_label)
}

fn parse_endwhile_label(raw: &str) -> Option<String> {
    let after = raw.strip_prefix("endwhile").unwrap_or(raw).trim_start();
    let after = after.strip_prefix('(')?;
    let end = after.find(')')?;
    Some(after[..end].trim().to_string())
}

fn parse_repeat_while_head(raw: &str) -> (Option<String>, Option<String>, Option<String>) {
    let after = raw
        .strip_prefix("repeat while")
        .unwrap_or(raw)
        .trim_start();
    if after.is_empty() {
        return (None, None, None);
    }
    let (cond, mut rest) = take_paren(after);
    let mut is_label: Option<String> = None;
    let mut not_label: Option<String> = None;
    rest = rest.trim_start();
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("is") {
            let after = after.trim_start();
            if let Some(after_open) = after.strip_prefix('(') {
                if let Some(end) = after_open.find(')') {
                    is_label = Some(after_open[..end].trim().to_string());
                    rest = after_open[end + 1..].trim_start();
                    continue;
                }
            }
            break;
        }
        if let Some(after) = rest.strip_prefix("not") {
            let after = after.trim_start();
            if let Some(after_open) = after.strip_prefix('(') {
                if let Some(end) = after_open.find(')') {
                    not_label = Some(after_open[..end].trim().to_string());
                    rest = after_open[end + 1..].trim_start();
                    continue;
                }
            }
            break;
        }
        break;
    }
    (Some(cond), is_label, not_label)
}

fn parse_fork_end_merge(raw: &str) -> bool {
    // `end fork` / `end fork merge` → merge=true.
    // `end fork no merge` / `end fork nomerge` → merge=false.
    let t = raw.trim();
    if t.contains("no merge") || t.contains("nomerge") {
        return false;
    }
    true
}

fn parse_paren_arg(raw: &str, kw: &str) -> Option<String> {
    let after = raw.strip_prefix(kw).unwrap_or(raw).trim_start();
    let after = after.strip_prefix('(')?;
    let end = after.rfind(')')?;
    Some(after[..end].trim().to_string())
}

fn take_paren(s: &str) -> (String, &str) {
    if let Some(after) = s.strip_prefix('(') {
        if let Some(end) = after.find(')') {
            return (after[..end].trim().to_string(), &after[end + 1..]);
        }
    }
    (String::new(), s)
}

fn parse_arrow_label(raw: &str) -> Option<String> {
    // `-> foo;` or `-[#color,dashed]-> foo;`.
    let s = raw.trim_end_matches(';').trim();
    let after = if let Some(after) = s.strip_prefix("->") {
        after
    } else if let Some(open) = s.find("[") {
        // Skip the `-[…]->` decoration.
        let after_open = &s[open + 1..];
        let close = after_open.find(']')?;
        let rest = &after_open[close + 1..];
        let after = rest.strip_prefix("->")?;
        after
    } else {
        return None;
    };
    let label = after.trim();
    if label.is_empty() {
        None
    } else {
        Some(label.to_string())
    }
}

fn parse_swimlane(raw: &str, line_no: usize) -> Option<ActivityStmt> {
    if !raw.starts_with('|') {
        return None;
    }
    // `|Lane|` or `|#color| Lane |`.
    let rest = raw.trim_start_matches('|');
    let (color, label_part) = if let Some(after) = rest.strip_prefix('#') {
        let mut it = after.splitn(2, '|');
        let c = it.next().unwrap_or("").to_string();
        let label_part = it.next().unwrap_or("");
        (Some(format!("#{c}")), label_part)
    } else {
        (None, rest)
    };
    let end = label_part.find('|')?;
    let label = label_part[..end].trim().to_string();
    Some(ActivityStmt::SwimlaneSwitch {
        label,
        color,
        line: line_no,
    })
}

fn split_note_side(s: &str) -> (Option<NotePosition>, &str) {
    if let Some(rest) = s.strip_prefix("left") {
        return (Some(NotePosition::LeftOf), rest.trim_start());
    }
    if let Some(rest) = s.strip_prefix("right") {
        return (Some(NotePosition::RightOf), rest.trim_start());
    }
    (None, s)
}

/// Walk back over the most recently pushed statements and return the
/// notes-vector of the trailing `Action`, if any. Skips intervening
/// `SwimlaneSwitch` / `GotoLabel` / `Goto` / `Break` (which can't carry
/// notes themselves).
fn last_action_mut(body: &mut [ActivityStmt]) -> Option<&mut Vec<NoteAttach>> {
    for stmt in body.iter_mut().rev() {
        match stmt {
            ActivityStmt::Action { notes, .. } => return Some(notes),
            ActivityStmt::SwimlaneSwitch { .. }
            | ActivityStmt::GotoLabel { .. }
            | ActivityStmt::Goto { .. }
            | ActivityStmt::Break { .. } => continue,
            _ => return None,
        }
    }
    None
}

/// Extract any `backward …` statements from `body` and return them as a
/// new list. PlantUML's `backward :foo;` lines mark statements that ride
/// the back-edge of a `repeat`; the parser stages them as ordinary
/// actions for simplicity, then this pass pulls them out by sniffing
/// label prefixes.
///
/// M0 implementation: we don't lex `backward` specially, so this returns
/// `None` for now. The IR field is preserved so future parser work can
/// fill it without an IR migration.
fn take_backward(_body: &mut Vec<ActivityStmt>) -> Option<Vec<ActivityStmt>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::lexer::extract_uml_blocks;

    fn parse_str(s: &str) -> ActivityDiagram {
        let blocks = extract_uml_blocks(s);
        let (d, _diags) = parse(&blocks[0], CompatMode::Warn).unwrap();
        match d {
            Diagram::Activity(a) => a,
            _ => panic!("expected Activity"),
        }
    }

    #[test]
    fn linear() {
        let a = parse_str(
            "@startuml\n\
             start\n\
             :hello;\n\
             :world;\n\
             stop\n\
             @enduml\n",
        );
        assert_eq!(a.body.len(), 4);
        assert!(matches!(a.body[0], ActivityStmt::Start { .. }));
        assert!(matches!(a.body[3], ActivityStmt::Stop { .. }));
        if let ActivityStmt::Action { label, .. } = &a.body[1] {
            assert_eq!(label, &vec!["hello".to_string()]);
        } else {
            panic!("expected Action");
        }
    }

    #[test]
    fn if_else() {
        let a = parse_str(
            "@startuml\n\
             if (ok?) then (yes)\n\
             :A;\n\
             else (no)\n\
             :B;\n\
             endif\n\
             @enduml\n",
        );
        assert_eq!(a.body.len(), 1);
        match &a.body[0] {
            ActivityStmt::If {
                cond,
                then_label,
                then_branch,
                else_label,
                else_branch,
                elseifs,
                ..
            } => {
                assert_eq!(cond, "ok?");
                assert_eq!(then_label.as_deref(), Some("yes"));
                assert_eq!(else_label.as_deref(), Some("no"));
                assert_eq!(then_branch.len(), 1);
                assert_eq!(else_branch.as_ref().unwrap().len(), 1);
                assert!(elseifs.is_empty());
            }
            _ => panic!("expected If"),
        }
    }

    #[test]
    fn repeat_with_cond() {
        let a = parse_str(
            "@startuml\n\
             repeat\n\
             :work;\n\
             repeat while (more?) is (yes) not (no)\n\
             @enduml\n",
        );
        match &a.body[0] {
            ActivityStmt::Repeat {
                cond,
                is_label,
                not_label,
                body,
                ..
            } => {
                assert_eq!(cond.as_deref(), Some("more?"));
                assert_eq!(is_label.as_deref(), Some("yes"));
                assert_eq!(not_label.as_deref(), Some("no"));
                assert_eq!(body.len(), 1);
            }
            _ => panic!("expected Repeat"),
        }
    }

    #[test]
    fn fork_branches() {
        let a = parse_str(
            "@startuml\n\
             fork\n\
             :A;\n\
             fork again\n\
             :B;\n\
             fork again\n\
             :C;\n\
             end fork\n\
             @enduml\n",
        );
        match &a.body[0] {
            ActivityStmt::Fork {
                branches, merge, ..
            } => {
                assert_eq!(branches.len(), 3);
                assert!(*merge);
            }
            _ => panic!("expected Fork"),
        }
    }

    #[test]
    fn switch_with_cases() {
        let a = parse_str(
            "@startuml\n\
             switch (k)\n\
             case (a)\n\
             :A;\n\
             case (b)\n\
             :B;\n\
             endswitch\n\
             @enduml\n",
        );
        match &a.body[0] {
            ActivityStmt::Switch { cond, cases, .. } => {
                assert_eq!(cond, "k");
                assert_eq!(cases.len(), 2);
                assert_eq!(cases[0].value, "a");
                assert_eq!(cases[1].value, "b");
            }
            _ => panic!("expected Switch"),
        }
    }

    #[test]
    fn multiline_action() {
        let a = parse_str(
            "@startuml\n\
             :line one\nline two;\n\
             @enduml\n",
        );
        if let ActivityStmt::Action { label, .. } = &a.body[0] {
            assert_eq!(label.len(), 2);
            assert_eq!(label[0], "line one");
            assert_eq!(label[1], "line two");
        } else {
            panic!("expected Action");
        }
    }

    #[test]
    fn stereotype_action() {
        let a = parse_str(
            "@startuml\n\
             :ping; <<input>>\n\
             @enduml\n",
        );
        if let ActivityStmt::Action { kind, .. } = &a.body[0] {
            assert_eq!(*kind, ActionKind::Input);
        } else {
            panic!("expected Action");
        }
    }
}
