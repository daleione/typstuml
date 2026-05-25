//! State-declaration and transition parsing for the state-diagram parser,
//! including endpoint resolution (`[*]`, `[H]`, `==sync==`, plain refs).

use crate::diagnostics::Result;
use crate::ir::{Direction, StateKind, Transition};

use super::scan::{
    count_arrow_dashes, find_arrow, parse_arrow_direction, parse_arrow_style, parse_name_part,
    scoped_pseudo_id, split_label, split_top_colon, strip_history_suffix, strip_synchro,
    strip_trailing_color, unquote,
};
use super::{Parser, RegionBuilder, FINAL_ID, INITIAL_ID};

impl<'a> Parser<'a> {
    pub(super) fn parse_state_decl(&mut self, rest: &str, line_no: usize) -> Result<()> {
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

    pub(super) fn parse_transition(&mut self, raw: &str, line_no: usize) -> Result<()> {
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
            return self.report(
                line_no,
                format!("transition has an invalid endpoint: {raw:?}"),
            );
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
            let scope = self.current_region().map(
                |(id, ri)| {
                    if ri == 0 {
                        id
                    } else {
                        format!("{id}#{ri}")
                    }
                },
            );
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
        if let Some((kind, id, scope)) = strip_history_suffix(tok, self.current_parent().as_deref())
        {
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
}
