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
    Diagram, LayoutDirection, RegionGroup, RegionOrient, Skinparam, StateDiagram, StateKind,
    StateNode,
};
use crate::parser::lexer::{BodyLine, UmlBlock};

mod element;
mod note;
mod scan;

#[cfg(test)]
mod tests;

use scan::*;

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
