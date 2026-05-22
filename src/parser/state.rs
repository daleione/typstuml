//! State-diagram parser (PlantUML `@startuml` with `[*]` / `state`).
//!
//! Line-oriented scan over the lexer's body. The S1 scope is flat: simple
//! states, pseudostates (initial / final / choice / fork / join / history /
//! synchro bar), one-line `entry/exit/do` body rows, and transitions with
//! `event [guard] / action` labels. Composite `state X { … }` blocks and
//! concurrent `--` / `||` dividers are recognized but warned + skipped — they
//! land in S2 / S3 (see `docs/state-diagram-design.md`).
//!
//! Errors degrade to warnings under `--compat warn` (default) and become
//! `Error::Parse` under `--compat strict`, matching the activity / cuca parser.

use std::collections::HashMap;

use crate::diagnostics::{CompatMode, Diagnostic, Error, Level, Result};
use crate::ir::{
    BorderStyle, Diagram, Direction, LayoutDirection, LineStyle, NoteAnchor, NotePosition,
    RegionGroup, RegionOrient, Skinparam, StateDiagram, StateKind, StateNode, StateNote,
    Transition,
};
use crate::parser::lexer::{BodyLine, UmlBlock};

/// Synthetic id for the diagram-level initial pseudostate (`[*]` as source).
const INITIAL_ID: &str = "__initial__";
/// Synthetic id for the diagram-level final pseudostate (`[*]` as target).
const FINAL_ID: &str = "__final__";

pub fn parse(block: &UmlBlock, compat: CompatMode) -> Result<(Diagram, Vec<Diagnostic>)> {
    let mut parser = Parser::new(&block.body, compat);
    parser.run()?;
    let mut diag = parser.diag;
    diag.name = block.name.clone();
    Ok((Diagram::State(diag), parser.diagnostics))
}

/// One entry on the composite-state stack. Tracks the composite's id and,
/// as `--` / `||` dividers are seen, the concurrent-region partitions of
/// its children. A node created while this is on top of the stack is
/// appended to the current (last) partition.
struct RegionBuilder {
    id: String,
    /// Set by the first divider seen inside this composite. `--` →
    /// `Horizontal`, `||` → `Vertical`. PlantUML forbids mixing the two;
    /// we warn and keep the first.
    orient: Option<RegionOrient>,
    /// Region partitions, in declaration order. Always has at least one
    /// entry (the implicit first region); a second entry appears only
    /// after a divider.
    partitions: Vec<Vec<String>>,
}

struct Parser<'a> {
    lines: &'a [BodyLine],
    pos: usize,
    compat: CompatMode,
    diag: StateDiagram,
    diagnostics: Vec<Diagnostic>,
    /// id → index into `diag.nodes`, for auto-create + addfield lookup.
    index: HashMap<String, usize>,
    /// Floating-note id (`note "..." as Nx`) → index into `diag.notes`,
    /// so a later `Nx .. State` line can attach its link target.
    float_note_ids: HashMap<String, usize>,
    /// Stack of enclosing composite states. A node created while the stack
    /// is non-empty becomes a child of the top entry's current region.
    parent_stack: Vec<RegionBuilder>,
}

impl<'a> Parser<'a> {
    fn new(lines: &'a [BodyLine], compat: CompatMode) -> Self {
        Self {
            lines,
            pos: 0,
            compat,
            diag: StateDiagram::default(),
            diagnostics: Vec::new(),
            index: HashMap::new(),
            float_note_ids: HashMap::new(),
            parent_stack: Vec::new(),
        }
    }

    /// The composite state currently being parsed into, if any.
    fn current_parent(&self) -> Option<String> {
        self.parent_stack.last().map(|rb| rb.id.clone())
    }

    /// The current `(composite id, region index)` — the scope used to make
    /// `[*]` pseudostates unique per concurrent region. `None` at the
    /// diagram top level.
    fn current_region(&self) -> Option<(String, usize)> {
        self.parent_stack
            .last()
            .map(|rb| (rb.id.clone(), rb.partitions.len() - 1))
    }

    /// Handle a `--` / `||` concurrent-region divider line. Opens a new
    /// region partition in the enclosing composite.
    fn handle_divider(&mut self, raw: &str, line_no: usize) -> Result<()> {
        let orient = if raw.starts_with('|') {
            RegionOrient::Vertical
        } else {
            RegionOrient::Horizontal
        };
        if self.parent_stack.is_empty() {
            return self.report(
                line_no,
                "concurrent region divider ('--' / '||') outside a composite state",
            );
        }
        let rb = self.parent_stack.last_mut().unwrap();
        let mut mixed = false;
        match rb.orient {
            None => rb.orient = Some(orient),
            Some(existing) if existing != orient => mixed = true,
            Some(_) => {}
        }
        rb.partitions.push(Vec::new());
        if mixed {
            self.warn(
                line_no,
                "mixed '--' and '||' dividers in one composite; keeping the first orientation",
            );
        }
        Ok(())
    }

    /// Pop a composite off the stack. When it accumulated more than one
    /// region partition, record a [`RegionGroup`] for codegen.
    fn finish_composite(&mut self, rb: RegionBuilder) {
        if rb.partitions.len() > 1 {
            self.diag.regions.push(RegionGroup {
                composite_id: rb.id,
                orientation: rb.orient.unwrap_or(RegionOrient::Horizontal),
                partitions: rb.partitions,
            });
        }
    }

    fn warn(&mut self, line: usize, msg: impl Into<String>) {
        self.diagnostics.push(Diagnostic {
            level: Level::Warning,
            line: Some(line),
            message: msg.into(),
        });
    }

    /// Report an unsupported / malformed construct: hard error under
    /// `--compat strict`, a warning otherwise.
    fn report(&mut self, line: usize, msg: impl Into<String>) -> Result<()> {
        let msg = msg.into();
        if self.compat == CompatMode::Strict {
            return Err(Error::Parse { line, message: msg });
        }
        self.warn(line, msg);
        Ok(())
    }

    fn run(&mut self) -> Result<()> {
        self.skip_preamble()?;
        while self.pos < self.lines.len() {
            let raw = self.lines[self.pos].text.trim().to_string();
            let line_no = self.lines[self.pos].line;
            self.pos += 1;
            if raw.is_empty() || is_comment(&raw) {
                continue;
            }
            self.handle_line(&raw, line_no)?;
        }
        // Unclosed composite blocks: warn once and recover.
        if !self.parent_stack.is_empty() {
            let last = self.lines.last().map(|l| l.line).unwrap_or(0);
            let open: Vec<RegionBuilder> = self.parent_stack.drain(..).collect();
            let names: Vec<String> = open.iter().map(|rb| rb.id.clone()).collect();
            for rb in open {
                self.finish_composite(rb);
            }
            self.report(
                last,
                format!("unclosed composite state(s): {}", names.join(", ")),
            )?;
        }
        Ok(())
    }

    /// Consume leading directives (`title`, `skinparam`, `hide/show`,
    /// `!theme`, direction). Unlike activity this is non-exclusive: state
    /// diagrams interleave directives with content, so directive handling
    /// also lives in `handle_line`. This just fast-forwards a pure header.
    fn skip_preamble(&mut self) -> Result<()> {
        while self.pos < self.lines.len() {
            let raw = self.lines[self.pos].text.trim();
            if raw.is_empty() || is_comment(raw) {
                self.pos += 1;
                continue;
            }
            if self.try_directive(raw) {
                self.pos += 1;
                continue;
            }
            break;
        }
        Ok(())
    }

    /// Handle a directive line. Returns `true` when the line was a
    /// directive (and is now consumed by the caller).
    fn try_directive(&mut self, raw: &str) -> bool {
        if let Some(rest) = strip_kw(raw, "title") {
            let t = rest.trim();
            if !t.is_empty() {
                self.diag.title = Some(t.to_string());
            }
            return true;
        }
        if let Some(rest) = strip_kw(raw, "skinparam") {
            let rest = rest.trim();
            if let Some((k, v)) = rest.split_once(char::is_whitespace) {
                self.diag.skinparams.push(Skinparam {
                    key: k.trim().to_string(),
                    value: v.trim().to_string(),
                    line: 0,
                });
            }
            return true;
        }
        if raw == "hide empty description" {
            self.diag.hide_empty_description = true;
            return true;
        }
        if raw == "show empty description" {
            self.diag.hide_empty_description = false;
            return true;
        }
        if raw.starts_with("hide ")
            || raw.starts_with("show ")
            || raw.starts_with("!theme")
            || raw.starts_with("!pragma")
            || raw.starts_with("caption")
            || raw.starts_with("header")
            || raw.starts_with("footer")
            || raw.starts_with("scale")
        {
            return true;
        }
        if raw.starts_with("left to right direction") || raw == "left to right" {
            self.diag.direction = LayoutDirection::LeftToRight;
            return true;
        }
        if raw.starts_with("top to bottom direction") || raw == "top to bottom" {
            self.diag.direction = LayoutDirection::TopToBottom;
            return true;
        }
        false
    }

    fn handle_line(&mut self, raw: &str, line_no: usize) -> Result<()> {
        if self.try_directive(raw) {
            return Ok(());
        }
        // Concurrent region divider — `--` / `||`.
        if is_divider(raw) {
            return self.handle_divider(raw, line_no);
        }
        // Composite-state close.
        if raw == "}" {
            match self.parent_stack.pop() {
                Some(rb) => self.finish_composite(rb),
                None => return self.report(line_no, "unmatched '}'"),
            }
            return Ok(());
        }
        // Note.
        if raw.starts_with("note ") || raw == "note" || raw.starts_with("note\"") {
            return self.parse_note(raw, line_no);
        }
        // State declaration (may open a composite block).
        if let Some(rest) = strip_kw(raw, "state") {
            return self.parse_state_decl(rest, line_no);
        }
        // Floating-note connector: `Nx .. State` / `State .. Nx`. Only when
        // the line has no arrow and no top-level `:` (so `A : do..it` body
        // rows aren't misread as a connector).
        if find_arrow(raw).is_none() && split_top_colon(raw).is_none() {
            if let Some((left, right)) = split_dotted(raw) {
                return self.parse_note_link(left, right, raw, line_no);
            }
        }
        // Transition.
        if find_arrow(raw).is_some() {
            return self.parse_transition(raw, line_no);
        }
        // `Id : body` — append a body row to an existing / new state.
        if let Some((head, body)) = split_top_colon(raw) {
            let id = unquote(head.trim());
            if !id.is_empty() {
                let idx = self.ensure_node(&id, line_no);
                self.diag.nodes[idx].body.push(body.trim().to_string());
                return Ok(());
            }
        }
        self.report(line_no, format!("unrecognized state-diagram line: {raw:?}"))
    }

    /// Parse a `note …` declaration. Forms: `note (left|right|top|bottom)
    /// of Foo [: body]`, `note on link [: body]`, and the floating
    /// `note "body" as Nx` / `note as Nx … end note`. A note with no inline
    /// `: body` continues until a line equal to `end note`.
    fn parse_note(&mut self, raw: &str, line_no: usize) -> Result<()> {
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
    fn parse_note_link(
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

    fn parse_state_decl(&mut self, rest: &str, line_no: usize) -> Result<()> {
        let mut s = rest.trim();
        // Composite open: `state X {` — and the rare one-line `state X { }`.
        let mut is_composite = false;
        let mut immediate_close = false;
        if let Some(brace) = s.find('{') {
            is_composite = true;
            let after = s[brace + 1..].trim();
            if after == "}" {
                immediate_close = true;
            } else if !after.is_empty() {
                self.warn(line_no, format!("ignoring content after '{{': {after:?}"));
            }
            s = s[..brace].trim_end();
        } else if let Some(stripped) = s.strip_suffix(" begin") {
            is_composite = true;
            s = stripped.trim_end();
        }

        // Split off an inline `: body` row.
        let mut body: Option<String> = None;
        if let Some((head, b)) = split_top_colon(s) {
            body = Some(b.trim().to_string());
            // `head` is borrowed from `s`; reborrow as a slice of `s`.
            let head_len = head.len();
            s = s[..head_len].trim_end();
        }

        // Split off `#color` / `##[style]color` (trailing).
        let (s2, fill, border_style, border_color) = strip_trailing_color(s);
        let mut s = s2.trim();

        // Split off `<<stereotype>>`.
        let mut stereotype: Option<String> = None;
        if let Some(start) = s.find("<<") {
            if let Some(end) = s[start..].find(">>") {
                let st = s[start + 2..start + end].trim().to_string();
                stereotype = Some(st);
                s = s[..start].trim_end();
            }
        }

        let (id, display) = parse_name_part(s);
        if id.is_empty() {
            return self.report(line_no, format!("malformed state declaration: {rest:?}"));
        }

        let kind = stereotype
            .as_deref()
            .and_then(StateKind::from_stereotype)
            .unwrap_or(StateKind::Simple);

        let idx = self.ensure_node(&id, line_no);
        let node = &mut self.diag.nodes[idx];
        // An explicit declaration upgrades the placeholder created by an
        // earlier transition reference.
        if display != id {
            node.display = display;
        }
        if kind != StateKind::Simple {
            node.kind = kind;
        }
        if node.stereotype.is_none() {
            node.stereotype = stereotype;
        }
        if fill.is_some() {
            node.fill = fill;
        }
        if border_style.is_some() {
            node.border_style = border_style;
        }
        if border_color.is_some() {
            node.border_color = border_color;
        }
        if let Some(b) = body {
            if !b.is_empty() {
                node.body.push(b);
            }
        }
        node.line = line_no;

        // Composite block: mark the kind and descend into it.
        if is_composite {
            self.diag.nodes[idx].kind = StateKind::Composite;
            if !immediate_close {
                self.parent_stack.push(RegionBuilder {
                    id,
                    orient: None,
                    partitions: vec![Vec::new()],
                });
            }
        }
        Ok(())
    }

    fn parse_transition(&mut self, raw: &str, line_no: usize) -> Result<()> {
        let Some((a_start, a_end)) = find_arrow(raw) else {
            return self.report(line_no, format!("malformed transition: {raw:?}"));
        };
        let left = raw[..a_start].trim();
        let arrow = &raw[a_start..a_end];
        let right_full = raw[a_end..].trim();

        // Split the right side into `target` and optional `: label`.
        let (target_tok, label) = match split_top_colon(right_full) {
            Some((t, l)) => (t.trim(), Some(l.trim().to_string())),
            None => (right_full, None),
        };

        if left.is_empty() || target_tok.is_empty() {
            return self.report(line_no, format!("transition missing endpoint: {raw:?}"));
        }

        let reverse = arrow.contains('<') && !arrow.contains('>');
        let (src_tok, dst_tok) = if reverse {
            (target_tok, left)
        } else {
            (left, target_tok)
        };

        let from = self.resolve_endpoint(src_tok, true, line_no);
        let to = self.resolve_endpoint(dst_tok, false, line_no);
        let (Some(from), Some(to)) = (from, to) else {
            return self.report(line_no, format!("transition has an invalid endpoint: {raw:?}"));
        };

        let (line_style, color) = parse_arrow_style(arrow);
        let direction = parse_arrow_direction(arrow);
        // PlantUML: single-dash `->` (or a left/right hint) is a horizontal
        // link; double-dash `-->` (or an up/down hint) is a vertical rank
        // edge. The dash count is the tie-breaker when no hint is given.
        let dashes = count_arrow_dashes(arrow);
        let horizontal = match direction {
            Some(Direction::Left) | Some(Direction::Right) => true,
            Some(Direction::Up) | Some(Direction::Down) => false,
            None => dashes <= 1,
        };
        // dot's minlen = dashes − 1 (`-->` = 1, `--->` = 2, …); floor 1 for
        // any rank edge.
        let min_rank = dashes.saturating_sub(1).max(1);
        let (event, guard, action) = match &label {
            Some(l) => split_label(l),
            None => (None, None, None),
        };

        self.diag.transitions.push(Transition {
            from,
            to,
            event,
            guard,
            action,
            line_style,
            color,
            direction,
            horizontal,
            min_rank,
            line: line_no,
        });
        Ok(())
    }

    /// Resolve a transition endpoint token to a node id, creating the node
    /// if necessary. `is_source` decides whether a bare `[*]` becomes the
    /// initial or the final pseudostate. Returns `None` for a token that
    /// can't be a valid endpoint.
    fn resolve_endpoint(&mut self, tok: &str, is_source: bool, line_no: usize) -> Option<String> {
        let tok = tok.trim();
        if tok.is_empty() {
            return None;
        }
        if tok == "[*]" {
            // Initial / final pseudostates are scoped to the enclosing
            // composite *and concurrent region*: `[*]` inside region 1 of
            // `state Foo { … }` is distinct from region 0's `[*]` and from
            // the diagram-level one.
            let scope = self.current_region().map(|(id, ri)| {
                if ri == 0 {
                    id
                } else {
                    format!("{id}#{ri}")
                }
            });
            let (base, kind) = if is_source {
                (INITIAL_ID, StateKind::Initial)
            } else {
                (FINAL_ID, StateKind::Final)
            };
            let id = scoped_pseudo_id(base, scope.as_deref());
            self.ensure_pseudo(&id, kind, line_no);
            return Some(id);
        }
        // History endpoints: `[H]`, `[H*]`, and the `Composite[H]` form.
        // A bare `[H]` is scoped to the enclosing composite.
        if let Some((kind, id, scope)) = strip_history_suffix(tok, self.current_parent().as_deref()) {
            let nidx = self.ensure_pseudo(&id, kind, line_no);
            // The `Composite[H]` form scopes the history pseudostate to the
            // named composite even when the transition line sits outside that
            // composite's block (e.g. `Suspended --> Operating[H]` at top
            // level). `ensure_pseudo` parents by lexical scope, which is wrong
            // here, so re-parent into the named composite so it lays out
            // inside the frame, matching PlantUML.
            if let Some(scope_id) = scope {
                self.reparent_into(nidx, &id, &scope_id);
            }
            return Some(id);
        }
        // Synchronization bar: `==Name==`.
        if let Some(name) = strip_synchro(tok) {
            let id = format!("__sync__{name}");
            let idx = self.ensure_pseudo(&id, StateKind::SynchroBar, line_no);
            self.diag.nodes[idx].display = name;
            return Some(id);
        }
        // Ordinary state reference (quoted display allowed but unusual).
        let id = unquote(tok);
        if id.is_empty() {
            return None;
        }
        self.ensure_node(&id, line_no);
        Some(id)
    }

    /// Register a freshly created node under the current composite parent
    /// (if any) — sets its `parent` and appends it to the parent's
    /// `children`. `idx` is the new node's index.
    fn attach_to_parent(&mut self, idx: usize, id: &str) {
        let Some(rb) = self.parent_stack.last_mut() else {
            return;
        };
        let pid = rb.id.clone();
        rb.partitions
            .last_mut()
            .expect("region builder always has a partition")
            .push(id.to_string());
        self.diag.nodes[idx].parent = Some(pid.clone());
        if let Some(&pidx) = self.index.get(&pid) {
            self.diag.nodes[pidx].children.push(id.to_string());
        }
    }

    /// Move node `idx` (id `id`) under composite `parent_id`, fixing up both
    /// the old parent's and the new parent's `children` lists. No-op when it
    /// is already parented there or when `parent_id` doesn't resolve.
    fn reparent_into(&mut self, idx: usize, id: &str, parent_id: &str) {
        if self.diag.nodes[idx].parent.as_deref() == Some(parent_id) {
            return;
        }
        let Some(&pidx) = self.index.get(parent_id) else {
            return;
        };
        // Detach from the lexical parent that `ensure_pseudo` assigned.
        if let Some(old) = self.diag.nodes[idx].parent.clone() {
            if let Some(&oidx) = self.index.get(&old) {
                self.diag.nodes[oidx].children.retain(|c| c != id);
            }
        }
        self.diag.nodes[idx].parent = Some(parent_id.to_string());
        if !self.diag.nodes[pidx].children.iter().any(|c| c == id) {
            self.diag.nodes[pidx].children.push(id.to_string());
        }
    }

    /// Get-or-create a plain node, returning its index.
    fn ensure_node(&mut self, id: &str, line_no: usize) -> usize {
        if let Some(&idx) = self.index.get(id) {
            return idx;
        }
        let idx = self.diag.nodes.len();
        self.diag.nodes.push(StateNode {
            id: id.to_string(),
            display: id.to_string(),
            kind: StateKind::Simple,
            body: Vec::new(),
            fill: None,
            border_style: None,
            border_color: None,
            stereotype: None,
            children: Vec::new(),
            parent: None,
            line: line_no,
        });
        self.index.insert(id.to_string(), idx);
        self.attach_to_parent(idx, id);
        idx
    }

    /// Get-or-create a pseudostate node with a fixed `kind`, returning its
    /// index. Pseudostates carry no visible label.
    fn ensure_pseudo(&mut self, id: &str, kind: StateKind, line_no: usize) -> usize {
        if let Some(&idx) = self.index.get(id) {
            return idx;
        }
        let idx = self.diag.nodes.len();
        self.diag.nodes.push(StateNode {
            id: id.to_string(),
            display: String::new(),
            kind,
            body: Vec::new(),
            fill: None,
            border_style: None,
            border_color: None,
            stereotype: None,
            children: Vec::new(),
            parent: None,
            line: line_no,
        });
        self.index.insert(id.to_string(), idx);
        self.attach_to_parent(idx, id);
        idx
    }
}

// ---------------------------------------------------------------------------
// Free helpers — no parser state.
// ---------------------------------------------------------------------------

fn is_comment(s: &str) -> bool {
    s.starts_with('\'') || s.starts_with("/'")
}

/// Strip a leading multi-word phrase (e.g. `"left of"`) when it is followed
/// by whitespace or end-of-string. Returns the trimmed remainder.
fn strip_phrase<'s>(s: &'s str, phrase: &str) -> Option<&'s str> {
    let s = s.trim_start();
    let rest = s.strip_prefix(phrase)?;
    if rest.is_empty() || rest.starts_with(char::is_whitespace) {
        Some(rest.trim_start())
    } else {
        None
    }
}

/// Strip a leading keyword followed by whitespace (or the whole string).
fn strip_kw<'s>(s: &'s str, kw: &str) -> Option<&'s str> {
    if !s.starts_with(kw) {
        return None;
    }
    if s.len() == kw.len() {
        return Some("");
    }
    let next = s.as_bytes()[kw.len()];
    if next.is_ascii_whitespace() {
        Some(s[kw.len() + 1..].trim_start())
    } else {
        None
    }
}

/// A concurrent-region divider line: `--`, `----`, `||`, etc. (nothing
/// else on the line).
fn is_divider(s: &str) -> bool {
    (s.len() >= 2 && s.chars().all(|c| c == '-')) || (s.len() >= 2 && s.chars().all(|c| c == '|'))
}

/// Find the transition arrow inside `s`, returning its byte span.
///
/// Recognizes `->`, `-->`, `<-`, `<--`, direction hints (`-up->`,
/// `-l->`), bracketed styles (`-[#blue,dashed]->`), and an optional
/// leading `x` cross-start. An arrow must contain at least one `-` and
/// at least one of `<` / `>`.
fn find_arrow(s: &str) -> Option<(usize, usize)> {
    let b = s.as_bytes();
    let n = b.len();
    let mut i = 0;
    while i < n {
        let c = b[i];
        let could_start = c == b'-'
            || c == b'<'
            || (c == b'x' && i + 1 < n && (b[i + 1] == b'-' || b[i + 1] == b'<'));
        if could_start {
            let mut j = i;
            let mut saw_lt = false;
            if b[j] == b'x' {
                j += 1;
            }
            if j < n && b[j] == b'<' {
                saw_lt = true;
                j += 1;
            }
            let mut saw_dash = false;
            while j < n && b[j] == b'-' {
                saw_dash = true;
                j += 1;
            }
            // optional [style]
            if j < n && b[j] == b'[' {
                while j < n && b[j] != b']' {
                    j += 1;
                }
                if j < n {
                    j += 1;
                }
            }
            // optional direction word
            while j < n && b[j].is_ascii_alphabetic() {
                j += 1;
            }
            // optional [style]
            if j < n && b[j] == b'[' {
                while j < n && b[j] != b']' {
                    j += 1;
                }
                if j < n {
                    j += 1;
                }
            }
            while j < n && b[j] == b'-' {
                saw_dash = true;
                j += 1;
            }
            let mut saw_gt = false;
            if j < n && b[j] == b'>' {
                saw_gt = true;
                j += 1;
            }
            if saw_dash && (saw_gt || saw_lt) {
                return Some((i, j));
            }
        }
        i += 1;
    }
    None
}

/// Parse `[#color,dashed]` style spec embedded in an arrow.
fn parse_arrow_style(arrow: &str) -> (LineStyle, Option<String>) {
    let mut style = LineStyle::Solid;
    let mut color = None;
    if let Some(start) = arrow.find('[') {
        if let Some(end) = arrow[start..].find(']') {
            let inner = &arrow[start + 1..start + end];
            for part in inner.split(',') {
                let p = part.trim();
                if let Some(c) = p.strip_prefix('#') {
                    color = Some(format!("#{c}"));
                } else if p.starts_with('#') {
                    color = Some(p.to_string());
                } else {
                    match p.to_ascii_lowercase().as_str() {
                        "dashed" => style = LineStyle::Dashed,
                        "dotted" => style = LineStyle::Dotted,
                        "bold" | "plain" => {}
                        c if !c.is_empty() => color = Some(p.to_string()),
                        _ => {}
                    }
                }
            }
        }
    }
    (style, color)
}

/// Count the dashes that make up an arrow's shaft, ignoring any `-`
/// that sits inside a `[...]` style group. `->` is 1, `-->` / `-up->` /
/// `-[#blue]->` are 2.
fn count_arrow_dashes(arrow: &str) -> usize {
    let mut count = 0;
    let mut in_bracket = false;
    for c in arrow.chars() {
        match c {
            '[' => in_bracket = true,
            ']' => in_bracket = false,
            '-' if !in_bracket => count += 1,
            _ => {}
        }
    }
    count
}

/// Parse a direction hint (`up` / `down` / `left` / `right` and the
/// one-letter forms) from an arrow's letters.
fn parse_arrow_direction(arrow: &str) -> Option<Direction> {
    // Letters that sit between the dashes, ignoring any `[...]` style.
    let mut cleaned = String::new();
    let mut in_bracket = false;
    for c in arrow.chars() {
        match c {
            '[' => in_bracket = true,
            ']' => in_bracket = false,
            c if !in_bracket && c.is_ascii_alphabetic() => cleaned.push(c.to_ascii_lowercase()),
            _ => {}
        }
    }
    match cleaned.as_str() {
        "up" | "u" => Some(Direction::Up),
        "down" | "d" | "do" => Some(Direction::Down),
        "left" | "l" | "le" => Some(Direction::Left),
        "right" | "r" | "ri" => Some(Direction::Right),
        _ => None,
    }
}

/// Split a transition label into `(event, guard, action)`. The grammar is
/// `event [guard] / action` with all three parts optional.
fn split_label(label: &str) -> (Option<String>, Option<String>, Option<String>) {
    let label = label.trim();
    if label.is_empty() {
        return (None, None, None);
    }
    // Action: everything after the first `/` at bracket depth 0.
    let mut depth = 0i32;
    let mut slash_at = None;
    for (i, c) in label.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => depth -= 1,
            '/' if depth <= 0 => {
                slash_at = Some(i);
                break;
            }
            _ => {}
        }
    }
    let (head, action) = match slash_at {
        Some(i) => (label[..i].trim(), Some(label[i + 1..].trim().to_string())),
        None => (label, None),
    };
    // Guard: the first `[...]` group inside `head`.
    let mut event = head.to_string();
    let mut guard = None;
    if let Some(start) = head.find('[') {
        if let Some(end_rel) = head[start..].find(']') {
            guard = Some(head[start + 1..start + end_rel].trim().to_string());
            let before = head[..start].trim();
            let after = head[start + end_rel + 1..].trim();
            event = if after.is_empty() {
                before.to_string()
            } else if before.is_empty() {
                after.to_string()
            } else {
                format!("{before} {after}")
            };
        }
    }
    let norm = |s: String| {
        let t = s.trim().to_string();
        if t.is_empty() {
            None
        } else {
            Some(t)
        }
    };
    (norm(event), guard.and_then(norm), action.and_then(norm))
}

/// Find the first top-level `:` (not inside quotes or `[]`) and split the
/// string there. Returns `(head, tail)` with the `:` removed.
fn split_top_colon(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    let mut in_quote = false;
    let mut depth = 0i32;
    for i in 0..b.len() {
        match b[i] {
            b'"' => in_quote = !in_quote,
            b'[' if !in_quote => depth += 1,
            b']' if !in_quote => depth -= 1,
            b':' if !in_quote && depth <= 0 => {
                return Some((&s[..i], &s[i + 1..]));
            }
            _ => {}
        }
    }
    None
}

/// Split on the first run of two or more `.` (a `..` floating-note
/// connector, also `...` / `....`). Returns the trimmed `(left, right)`
/// when both sides are non-empty.
fn split_dotted(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'.' {
            let start = i;
            while i < b.len() && b[i] == b'.' {
                i += 1;
            }
            if i - start >= 2 {
                let left = s[..start].trim();
                let right = s[i..].trim();
                if !left.is_empty() && !right.is_empty() {
                    return Some((left, right));
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

/// Parse the name portion of a state declaration:
/// `Foo`, `"Display"`, `Foo as "Display"`, `"Display" as Foo`.
/// Returns `(id, display)`.
fn parse_name_part(s: &str) -> (String, String) {
    let s = s.trim();
    if s.is_empty() {
        return (String::new(), String::new());
    }
    // `... as ...` — split on a top-level ` as ` (outside quotes).
    if let Some((lhs, rhs)) = split_as(s) {
        let lhs = lhs.trim();
        let rhs = rhs.trim();
        let lhs_quoted = lhs.starts_with('"');
        let rhs_quoted = rhs.starts_with('"');
        if lhs_quoted && !rhs_quoted {
            // "Display" as Code
            return (unquote(rhs), unquote(lhs));
        }
        // Code as "Display"  (also the both-bare / both-quoted fallback)
        return (unquote(lhs), unquote(rhs));
    }
    let id = unquote(s);
    (id.clone(), id)
}

/// Split on a top-level ` as ` token (case-sensitive, outside quotes).
fn split_as(s: &str) -> Option<(&str, &str)> {
    let b = s.as_bytes();
    let mut in_quote = false;
    let mut i = 0;
    while i + 4 <= b.len() {
        if b[i] == b'"' {
            in_quote = !in_quote;
        }
        if !in_quote
            && b[i].is_ascii_whitespace()
            && &s[i + 1..(i + 3).min(s.len())] == "as"
            && i + 3 < b.len()
            && b[i + 3].is_ascii_whitespace()
        {
            return Some((&s[..i], &s[i + 4..]));
        }
        i += 1;
    }
    None
}

/// Strip surrounding double quotes if present.
fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}

/// Detect a trailing `#color` / `##[style]color` on a declaration line.
/// Returns `(remainder, fill, border_style, border_color)`.
fn strip_trailing_color(
    s: &str,
) -> (&str, Option<String>, Option<BorderStyle>, Option<String>) {
    let s = s.trim_end();
    // `##[style]color` — border spec.
    if let Some(hashes) = s.rfind("##") {
        // Make sure this `##` starts a token (preceded by space or start).
        let ok = hashes == 0 || s.as_bytes()[hashes - 1].is_ascii_whitespace();
        if ok {
            let spec = &s[hashes + 2..];
            let mut border_style = None;
            let mut rest = spec;
            if let Some(close) = spec.find(']') {
                if spec.starts_with('[') {
                    let style = &spec[1..close];
                    border_style = match style.to_ascii_lowercase().as_str() {
                        "dashed" => Some(BorderStyle::Dashed),
                        "dotted" => Some(BorderStyle::Dotted),
                        "bold" => Some(BorderStyle::Bold),
                        _ => None,
                    };
                    rest = &spec[close + 1..];
                }
            }
            // The remainder after the optional `[style]` is the border
            // color — either a `#hex` token or a bare color name.
            let rest = rest.trim();
            let border_color = if rest.is_empty() {
                None
            } else if rest.starts_with('#') {
                Some(rest.to_string())
            } else {
                Some(format!("#{rest}"))
            };
            return (s[..hashes].trim_end(), None, border_style, border_color);
        }
    }
    // `#color` — fill.
    if let Some(hash) = s.rfind('#') {
        let ok = hash == 0 || s.as_bytes()[hash - 1].is_ascii_whitespace();
        let token = &s[hash..];
        let looks_color = token.len() > 1
            && token[1..]
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_');
        if ok && looks_color {
            return (s[..hash].trim_end(), Some(token.to_string()), None, None);
        }
    }
    (s, None, None, None)
}

/// Build a scoped synthetic id for a pseudostate: `base` alone at the
/// diagram level, `base + scope` when nested inside composite `scope`.
fn scoped_pseudo_id(base: &str, scope: Option<&str>) -> String {
    match scope {
        Some(s) => format!("{base}{s}"),
        None => base.to_string(),
    }
}

/// `[H]` / `[H*]` / `Composite[H]` / `Composite[H*]` → `(kind, id, scope)`.
/// A bare `[H]` is scoped to `parent`; the `Composite[H]` form uses its
/// explicit prefix as the scope. `scope` is the composite the history
/// pseudostate belongs to (so it lays out *inside* that frame even when the
/// transition line sits outside the composite's block).
fn strip_history_suffix(
    tok: &str,
    parent: Option<&str>,
) -> Option<(StateKind, String, Option<String>)> {
    let (prefix, kind, base) = if let Some(p) = tok.strip_suffix("[H*]") {
        (p, StateKind::DeepHistory, "__deephistory__")
    } else if let Some(p) = tok.strip_suffix("[H]") {
        (p, StateKind::History, "__history__")
    } else {
        return None;
    };
    let scope = if prefix.is_empty() { parent } else { Some(prefix) };
    Some((kind, scoped_pseudo_id(base, scope), scope.map(str::to_string)))
}

/// `==Name==` → `Name`.
fn strip_synchro(tok: &str) -> Option<String> {
    let t = tok.trim();
    if t.starts_with("==") && t.ends_with("==") && t.len() > 4 {
        let inner = t.trim_matches('=').trim();
        if !inner.is_empty() {
            return Some(inner.to_string());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::lexer::extract_uml_blocks;

    fn parse_str(src: &str) -> StateDiagram {
        let blocks = extract_uml_blocks(src);
        let (d, _) = parse(&blocks[0], CompatMode::Warn).unwrap();
        match d {
            Diagram::State(s) => s,
            _ => panic!("expected state diagram"),
        }
    }

    #[test]
    fn simple_states_and_transition() {
        let d = parse_str("@startuml\nstate A\nstate B\nA --> B\n@enduml\n");
        assert_eq!(d.nodes.len(), 2);
        assert_eq!(d.transitions.len(), 1);
        assert_eq!(d.transitions[0].from, "A");
        assert_eq!(d.transitions[0].to, "B");
    }

    #[test]
    fn initial_and_final() {
        let d = parse_str("@startuml\n[*] --> A\nA --> [*]\n@enduml\n");
        let kinds: Vec<_> = d.nodes.iter().map(|n| (n.id.as_str(), n.kind)).collect();
        assert!(kinds.contains(&(INITIAL_ID, StateKind::Initial)));
        assert!(kinds.contains(&(FINAL_ID, StateKind::Final)));
        assert_eq!(d.transitions.len(), 2);
        assert_eq!(d.transitions[0].from, INITIAL_ID);
        assert_eq!(d.transitions[1].to, FINAL_ID);
    }

    #[test]
    fn quoted_alias() {
        let d = parse_str("@startuml\nstate \"Long Name\" as L\nstate B as \"Bee\"\n@enduml\n");
        let l = d.nodes.iter().find(|n| n.id == "L").unwrap();
        assert_eq!(l.display, "Long Name");
        let b = d.nodes.iter().find(|n| n.id == "B").unwrap();
        assert_eq!(b.display, "Bee");
    }

    #[test]
    fn stereotype_shortcuts() {
        let d = parse_str(
            "@startuml\nstate C <<choice>>\nstate F <<fork>>\nstate J <<join>>\n@enduml\n",
        );
        assert_eq!(d.nodes[0].kind, StateKind::Choice);
        assert_eq!(d.nodes[1].kind, StateKind::Fork);
        assert_eq!(d.nodes[2].kind, StateKind::Join);
    }

    #[test]
    fn transition_label_three_parts() {
        let d = parse_str("@startuml\nA --> B : evt [guard] / act()\n@enduml\n");
        let t = &d.transitions[0];
        assert_eq!(t.event.as_deref(), Some("evt"));
        assert_eq!(t.guard.as_deref(), Some("guard"));
        assert_eq!(t.action.as_deref(), Some("act()"));
    }

    #[test]
    fn transition_label_event_only() {
        let d = parse_str("@startuml\nA --> B : just an event\n@enduml\n");
        let t = &d.transitions[0];
        assert_eq!(t.event.as_deref(), Some("just an event"));
        assert!(t.guard.is_none());
        assert!(t.action.is_none());
    }

    #[test]
    fn reverse_arrow_swaps_endpoints() {
        let d = parse_str("@startuml\nB <-- A\n@enduml\n");
        assert_eq!(d.transitions[0].from, "A");
        assert_eq!(d.transitions[0].to, "B");
    }

    #[test]
    fn direction_and_style_hints() {
        let d = parse_str("@startuml\nA -up-> B\nA -[#blue,dashed]-> C\n@enduml\n");
        assert_eq!(d.transitions[0].direction, Some(Direction::Up));
        assert_eq!(d.transitions[1].line_style, LineStyle::Dashed);
        assert_eq!(d.transitions[1].color.as_deref(), Some("#blue"));
    }

    #[test]
    fn colors_and_border_style() {
        let d = parse_str(
            "@startuml\n\
             state A #LightBlue\n\
             state B ##[dashed]red\n\
             state C ##[bold]#888888\n\
             @enduml\n",
        );
        let a = d.nodes.iter().find(|n| n.id == "A").unwrap();
        assert_eq!(a.fill.as_deref(), Some("#LightBlue"));
        let b = d.nodes.iter().find(|n| n.id == "B").unwrap();
        assert_eq!(b.border_style, Some(BorderStyle::Dashed));
        assert_eq!(b.border_color.as_deref(), Some("#red"));
        let c = d.nodes.iter().find(|n| n.id == "C").unwrap();
        assert_eq!(c.border_style, Some(BorderStyle::Bold));
        assert_eq!(c.border_color.as_deref(), Some("#888888"));
    }

    #[test]
    fn left_to_right_direction() {
        let d = parse_str("@startuml\nleft to right direction\nstate A\n@enduml\n");
        assert_eq!(d.direction, LayoutDirection::LeftToRight);
    }

    #[test]
    fn horizontal_classification() {
        let d = parse_str(
            "@startuml\n\
             A -> B\n\
             A --> C\n\
             A -right-> D\n\
             A -down-> E\n\
             A -[#red]-> F\n\
             @enduml\n",
        );
        // `->` single dash → horizontal.
        assert!(d.transitions[0].horizontal);
        // `-->` double dash → vertical rank edge.
        assert!(!d.transitions[1].horizontal);
        // `-right->` hint → horizontal regardless of dash count.
        assert!(d.transitions[2].horizontal);
        // `-down->` hint → vertical.
        assert!(!d.transitions[3].horizontal);
        // `-[#red]->` is the two-dash form → vertical (bracket dash ignored).
        assert!(!d.transitions[4].horizontal);
    }

    #[test]
    fn addfield_appends_body() {
        let d = parse_str("@startuml\nstate A\nA : entry / start()\nA : exit / stop()\n@enduml\n");
        let a = d.nodes.iter().find(|n| n.id == "A").unwrap();
        assert_eq!(a.body, vec!["entry / start()", "exit / stop()"]);
    }

    #[test]
    fn inline_body_in_decl() {
        let d = parse_str("@startuml\nstate A : do / work()\n@enduml\n");
        let a = d.nodes.iter().find(|n| n.id == "A").unwrap();
        assert_eq!(a.body, vec!["do / work()"]);
    }

    #[test]
    fn self_loop() {
        let d = parse_str("@startuml\nstate A\nA --> A : retry\n@enduml\n");
        assert_eq!(d.transitions.len(), 1);
        assert_eq!(d.transitions[0].from, "A");
        assert_eq!(d.transitions[0].to, "A");
    }

    #[test]
    fn auto_create_from_transition() {
        let d = parse_str("@startuml\nFoo --> Bar\n@enduml\n");
        assert!(d.nodes.iter().any(|n| n.id == "Foo"));
        assert!(d.nodes.iter().any(|n| n.id == "Bar"));
    }

    #[test]
    fn title_directive() {
        let d = parse_str("@startuml\ntitle My Machine\nstate A\n@enduml\n");
        assert_eq!(d.title.as_deref(), Some("My Machine"));
    }

    #[test]
    fn composite_states_nest() {
        let d = parse_str(
            "@startuml\n\
             state Outer {\n\
               state Inner1\n\
               state Inner2\n\
               Inner1 --> Inner2\n\
               state Deep {\n\
                 state Leaf\n\
               }\n\
             }\n\
             state Sibling\n\
             Outer --> Sibling\n\
             @enduml\n",
        );
        let outer = d.nodes.iter().find(|n| n.id == "Outer").unwrap();
        assert_eq!(outer.kind, StateKind::Composite);
        assert!(outer.children.contains(&"Inner1".to_string()));
        assert!(outer.children.contains(&"Inner2".to_string()));
        assert!(outer.children.contains(&"Deep".to_string()));
        let inner1 = d.nodes.iter().find(|n| n.id == "Inner1").unwrap();
        assert_eq!(inner1.parent.as_deref(), Some("Outer"));
        let deep = d.nodes.iter().find(|n| n.id == "Deep").unwrap();
        assert_eq!(deep.kind, StateKind::Composite);
        assert!(deep.children.contains(&"Leaf".to_string()));
        let leaf = d.nodes.iter().find(|n| n.id == "Leaf").unwrap();
        assert_eq!(leaf.parent.as_deref(), Some("Deep"));
        // `Sibling` is top-level.
        let sib = d.nodes.iter().find(|n| n.id == "Sibling").unwrap();
        assert_eq!(sib.parent, None);
    }

    #[test]
    fn composite_with_alias() {
        let d = parse_str("@startuml\nstate \"Long Name\" as C {\n  state X\n}\n@enduml\n");
        let c = d.nodes.iter().find(|n| n.id == "C").unwrap();
        assert_eq!(c.kind, StateKind::Composite);
        assert_eq!(c.display, "Long Name");
        assert!(c.children.contains(&"X".to_string()));
    }

    #[test]
    fn unmatched_brace_warns() {
        let blocks = extract_uml_blocks("@startuml\nstate A\n}\n@enduml\n");
        let (_, diags) = parse(&blocks[0], CompatMode::Warn).unwrap();
        assert!(diags.iter().any(|x| x.message.contains("unmatched")));
        let r = parse(&blocks[0], CompatMode::Strict);
        assert!(r.is_err());
    }

    #[test]
    fn unclosed_composite_recovers() {
        let blocks = extract_uml_blocks("@startuml\nstate C {\n  state X\n@enduml\n");
        let (d, diags) = parse(&blocks[0], CompatMode::Warn).unwrap();
        let s = match d {
            Diagram::State(s) => s,
            _ => panic!(),
        };
        assert!(s.nodes.iter().any(|n| n.id == "X"));
        assert!(diags.iter().any(|x| x.message.contains("unclosed")));
    }

    #[test]
    fn note_single_and_multiline() {
        let d = parse_str(
            "@startuml\n\
             state A\n\
             note right of A : a quick note\n\
             note left of A\n\
             line one\n\
             line two\n\
             end note\n\
             @enduml\n",
        );
        assert_eq!(d.notes.len(), 2);
        match &d.notes[0].anchor {
            NoteAnchor::OfNode { node_id, side } => {
                assert_eq!(node_id, "A");
                assert_eq!(*side, NotePosition::RightOf);
            }
            _ => panic!("expected OfNode"),
        }
        assert_eq!(d.notes[0].body, "a quick note");
        assert_eq!(d.notes[1].body, "line one\nline two");
        match &d.notes[1].anchor {
            NoteAnchor::OfNode { side, .. } => assert_eq!(*side, NotePosition::LeftOf),
            _ => panic!(),
        }
    }

    #[test]
    fn concurrent_regions_split() {
        let d = parse_str(
            "@startuml\n\
             state Active {\n\
               [*] --> NumOff\n\
               NumOff --> NumOn : press\n\
               --\n\
               [*] --> CapsOff\n\
               CapsOff --> CapsOn : press\n\
             }\n\
             @enduml\n",
        );
        assert_eq!(d.regions.len(), 1);
        let rg = &d.regions[0];
        assert_eq!(rg.composite_id, "Active");
        assert_eq!(rg.orientation, RegionOrient::Horizontal);
        assert_eq!(rg.partitions.len(), 2);
        assert!(rg.partitions[0].contains(&"NumOff".to_string()));
        assert!(rg.partitions[1].contains(&"CapsOff".to_string()));
        // Each region gets its own scoped `[*]`.
        let initials = d
            .nodes
            .iter()
            .filter(|n| n.kind == StateKind::Initial)
            .count();
        assert_eq!(initials, 2);
    }

    #[test]
    fn vertical_divider_orientation() {
        let d = parse_str("@startuml\nstate S {\n  state A\n  ||\n  state B\n}\n@enduml\n");
        assert_eq!(d.regions.len(), 1);
        assert_eq!(d.regions[0].orientation, RegionOrient::Vertical);
    }

    #[test]
    fn divider_outside_composite_warns() {
        let blocks = extract_uml_blocks("@startuml\nstate A\n--\nstate B\n@enduml\n");
        let (_, diags) = parse(&blocks[0], CompatMode::Warn).unwrap();
        assert!(diags
            .iter()
            .any(|x| x.message.contains("outside a composite")));
    }

    #[test]
    fn note_on_link_binds_last_transition() {
        let d = parse_str(
            "@startuml\nA --> B : go\nnote on link : crossing\n@enduml\n",
        );
        assert_eq!(d.notes.len(), 1);
        match &d.notes[0].anchor {
            NoteAnchor::OnLink { transition_idx } => assert_eq!(*transition_idx, 0),
            _ => panic!("expected OnLink"),
        }
        assert_eq!(d.notes[0].body, "crossing");
    }

    #[test]
    fn note_on_link_multiline() {
        let d = parse_str(
            "@startuml\nA --> B\nnote on link\nfirst\nsecond\nend note\n@enduml\n",
        );
        assert_eq!(d.notes.len(), 1);
        assert_eq!(d.notes[0].body, "first\nsecond");
    }

    #[test]
    fn floating_note_with_link() {
        let d = parse_str(
            "@startuml\n\
             [*] --> Foo\n\
             note \"a floating note\" as N1\n\
             N1 .. Foo\n\
             @enduml\n",
        );
        assert_eq!(d.notes.len(), 1);
        match &d.notes[0].anchor {
            NoteAnchor::Floating { id, links } => {
                assert_eq!(id, "N1");
                assert_eq!(links, &["Foo".to_string()]);
            }
            _ => panic!("expected Floating"),
        }
        assert_eq!(d.notes[0].body, "a floating note");
    }

    #[test]
    fn floating_note_unconnected() {
        let d = parse_str("@startuml\nstate A\nnote \"lonely\" as N9\n@enduml\n");
        assert_eq!(d.notes.len(), 1);
        match &d.notes[0].anchor {
            NoteAnchor::Floating { id, links } => {
                assert_eq!(id, "N9");
                assert!(links.is_empty());
            }
            _ => panic!("expected Floating"),
        }
    }

    #[test]
    fn entry_exit_point_stereotypes() {
        let d = parse_str(
            "@startuml\n\
             state S {\n\
               state e <<entryPoint>>\n\
               state x <<exitPoint>>\n\
             }\n\
             @enduml\n",
        );
        let e = d.nodes.iter().find(|n| n.id == "e").unwrap();
        assert_eq!(e.kind, StateKind::EntryPoint);
        let x = d.nodes.iter().find(|n| n.id == "x").unwrap();
        assert_eq!(x.kind, StateKind::ExitPoint);
    }

    #[test]
    fn floating_note_link_reversed_and_multi() {
        let d = parse_str(
            "@startuml\n\
             note \"n\" as N1\n\
             A .. N1\n\
             N1 .. B\n\
             @enduml\n",
        );
        match &d.notes[0].anchor {
            NoteAnchor::Floating { links, .. } => {
                assert_eq!(links, &["A".to_string(), "B".to_string()]);
            }
            _ => panic!("expected Floating"),
        }
    }
}
