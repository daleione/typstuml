//! Note-declaration parsing (`note … of`, `note on link`, floating
//! `note "…" as Nx` + `..` connectors) for the state-diagram parser.
//!
//! NOTE: these are impl methods that mutate parser state (consume lines,
//! anchor notes to nodes) — intentionally not shared with `sequence::note`
//! (returns a `NoteParse` enum) or `cuca::note` (returns decl structs). The
//! action models differ by design; see `sequence::note` for the rationale.

use crate::diagnostics::Result;
use crate::ir::{NoteAnchor, NotePosition, StateNote};

use super::scan::{split_top_colon, strip_phrase, unquote};
use super::Parser;

impl<'a> Parser<'a> {
    /// Parse a `note …` declaration. Forms: `note (left|right|top|bottom)
    /// of Foo [: body]`, `note on link [: body]`, and the floating
    /// `note "body" as Nx` / `note as Nx … end note`. A note with no inline
    /// `: body` continues until a line equal to `end note`.
    pub(super) fn parse_note(&mut self, raw: &str, line_no: usize) -> Result<()> {
        let rest = raw.strip_prefix("note").unwrap_or(raw).trim_start();
        // `note on link` — bind to the most recently parsed transition.
        if let Some(after) = strip_phrase(rest, "on link") {
            return self.parse_note_on_link(after, line_no);
        }
        // `note "..." as Nx` / `note as Nx … end note` — floating note.
        if rest.starts_with('"') || strip_phrase(rest, "as").is_some() {
            return self.parse_floating_note(rest, raw, line_no);
        }
        // Side keyword.
        let (side, after) = if let Some(a) = strip_phrase(rest, "left of") {
            (NotePosition::LeftOf, a)
        } else if let Some(a) = strip_phrase(rest, "right of") {
            (NotePosition::RightOf, a)
        } else if let Some(a) = strip_phrase(rest, "top of") {
            (NotePosition::LeftOf, a)
        } else if let Some(a) = strip_phrase(rest, "bottom of") {
            (NotePosition::RightOf, a)
        } else {
            if !raw.contains(':') {
                self.skip_note_block(raw);
            }
            return self.report(
                line_no,
                "unsupported note form (use `note left/right of <state>`, \
                 `note on link`, or `note \"…\" as <id>`)",
            );
        };
        // `after` is `Foo` or `Foo : body` or `"Foo" : body`.
        let (anchor_tok, inline_body) = match split_top_colon(after) {
            Some((h, b)) => (h.trim(), Some(b.trim().to_string())),
            None => (after.trim(), None),
        };
        let node_id = unquote(anchor_tok);
        if node_id.is_empty() {
            return self.report(line_no, format!("malformed note: {raw:?}"));
        }
        // The anchored state must resolve to a node — auto-create if needed.
        self.ensure_node(&node_id, line_no);

        let body = match inline_body {
            Some(b) => b,
            None => {
                // Multi-line: collect until `end note`.
                let mut rows: Vec<String> = Vec::new();
                while self.pos < self.lines.len() {
                    let t = self.lines[self.pos].text.trim().to_string();
                    self.pos += 1;
                    if t == "end note" || t == "endnote" {
                        break;
                    }
                    rows.push(t);
                }
                rows.join("\n")
            }
        };

        self.diag.notes.push(StateNote {
            anchor: NoteAnchor::OfNode { node_id, side },
            body,
            line: line_no,
        });
        Ok(())
    }

    /// Parse `note on link [: body]` — a note bound to the transition
    /// declared just above it. A bodyless form continues until `end note`.
    fn parse_note_on_link(&mut self, after: &str, line_no: usize) -> Result<()> {
        let body = match split_top_colon(after) {
            Some((_, b)) => b.trim().to_string(),
            None => {
                let mut rows: Vec<String> = Vec::new();
                while self.pos < self.lines.len() {
                    let t = self.lines[self.pos].text.trim().to_string();
                    self.pos += 1;
                    if t == "end note" || t == "endnote" {
                        break;
                    }
                    rows.push(t);
                }
                rows.join("\n")
            }
        };
        if self.diag.transitions.is_empty() {
            return self.report(line_no, "`note on link` with no preceding transition");
        }
        let transition_idx = self.diag.transitions.len() - 1;
        self.diag.notes.push(StateNote {
            anchor: NoteAnchor::OnLink { transition_idx },
            body,
            line: line_no,
        });
        Ok(())
    }

    /// Parse a floating note: `note "body" as Nx` (inline quoted body) or
    /// `note as Nx … end note` (body runs until `end note`). The note is
    /// registered under its alias so a later `Nx .. State` line can attach
    /// a link target.
    fn parse_floating_note(&mut self, rest: &str, raw: &str, line_no: usize) -> Result<()> {
        let (body, alias) = if let Some(after_open) = rest.strip_prefix('"') {
            let Some(close) = after_open.find('"') else {
                return self.report(line_no, format!("unterminated note string: {raw:?}"));
            };
            let body = after_open[..close].to_string();
            let tail = after_open[close + 1..].trim_start();
            let Some(alias) = strip_phrase(tail, "as") else {
                return self.report(line_no, format!("floating note missing `as <id>`: {raw:?}"));
            };
            (body, unquote(alias.trim()))
        } else if let Some(alias) = strip_phrase(rest, "as") {
            let alias = unquote(alias.trim());
            let mut rows: Vec<String> = Vec::new();
            while self.pos < self.lines.len() {
                let t = self.lines[self.pos].text.trim().to_string();
                self.pos += 1;
                if t == "end note" || t == "endnote" {
                    break;
                }
                rows.push(t);
            }
            (rows.join("\n"), alias)
        } else {
            return self.report(line_no, format!("malformed floating note: {raw:?}"));
        };
        if alias.is_empty() {
            return self.report(line_no, format!("floating note has an empty id: {raw:?}"));
        }
        let note_idx = self.diag.notes.len();
        self.diag.notes.push(StateNote {
            anchor: NoteAnchor::Floating {
                id: alias.clone(),
                links: Vec::new(),
            },
            body,
            line: line_no,
        });
        self.float_note_ids.insert(alias, note_idx);
        Ok(())
    }

    /// Parse a `Nx .. State` / `State .. Nx` floating-note connector. One
    /// side must be a registered floating-note alias; the other is the
    /// linked state (auto-created if new).
    pub(super) fn parse_note_link(
        &mut self,
        left: &str,
        right: &str,
        raw: &str,
        line_no: usize,
    ) -> Result<()> {
        let l = unquote(left);
        let r = unquote(right);
        let (note_idx, target) = if let Some(&ni) = self.float_note_ids.get(&l) {
            (ni, r)
        } else if let Some(&ni) = self.float_note_ids.get(&r) {
            (ni, l)
        } else {
            return self.report(
                line_no,
                format!("`..` connector with no floating note: {raw:?}"),
            );
        };
        if target.is_empty() {
            return self.report(line_no, format!("note connector missing a state: {raw:?}"));
        }
        self.ensure_node(&target, line_no);
        if let NoteAnchor::Floating { links, .. } = &mut self.diag.notes[note_idx].anchor {
            links.push(target);
        }
        Ok(())
    }

    /// Skip a multi-line `note … end note` block when the opening line
    /// didn't carry an inline `:` body.
    fn skip_note_block(&mut self, opening: &str) {
        if opening.contains(':') {
            return; // single-line note
        }
        while self.pos < self.lines.len() {
            let t = self.lines[self.pos].text.trim().to_string();
            self.pos += 1;
            if t == "end note" || t == "endnote" {
                break;
            }
        }
    }
}
