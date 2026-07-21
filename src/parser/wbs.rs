//! Work-Breakdown-Structure parser (`@startwbs … @endwbs`).
//!
//! Walks the body line-by-line, classifies each non-blank/non-comment line
//! as either decoration (`title`) or a marker line, and assembles the
//! resulting [`TreeNode`] tree using a depth stack.
//!
//! Marker line grammar (PlantUML compat subset, see
//! `docs/mindmap-wbs-plan.md` §3.3):
//!
//! ```text
//! [\t ]*  ([*+-]+)  ( <|> | _ )*  (\[#color\])?  (\(code\))?  (\s+ <label>)?
//! ```
//!
//! `<` / `>` are parsed into [`NodeSide`] but v1 codegen ignores them
//! (renders all children below the parent). `_` flips [`NodeShape`] to
//! `Line` — also kept in the IR for M3 painter pickup. Multi-line labels
//! use the `:line1\nline2;` command form: when the label after the marker
//! starts with `:` and isn't terminated by `;` on the same line, subsequent
//! body lines are appended verbatim until a `;`-terminated line is seen.
//!
//! Errors that occur on a single marker line never abort parsing; they
//! degrade to warnings under `--compat warn`/`loose` and are promoted to
//! `Error::Parse` only under `--compat strict`. A skipped line drops its
//! whole subtree (no synthetic placeholder is invented).

use crate::diagnostics::{CompatMode, Diagnostic, Result};
use crate::ir::{Diagram, NodeShape, NodeSide, TreeNode, WbsDiagram};
use crate::parser::common::strip_keyword_trimmed as strip_keyword;
use crate::parser::lexer::{BodyLine, UmlBlock};
use crate::parser::tree::{self, ParsedItem, ParsedMarker};

pub fn parse(block: &UmlBlock, compat: CompatMode) -> Result<(Diagram, Vec<Diagnostic>)> {
    let (root, title, diagnostics) = tree::build_tree(block, compat, "WBS", parse_marker_line)?;
    Ok((
        Diagram::Wbs(WbsDiagram {
            name: block.name.clone(),
            title,
            root,
        }),
        diagnostics,
    ))
}

/// Parse the marker line at `body[start]` plus any continuation lines if it
/// opens a multi-line `:label;` block. Returns the parsed node and the
/// number of body lines consumed (>= 1).
fn parse_marker_line(body: &[BodyLine], start: usize) -> std::result::Result<(ParsedItem, usize), String> {
    let line = &body[start];
    let raw = line.text.trim_start_matches([' ', '\t']);

    // 1. Markers. WBS accepts "arithmetic notation": mixed runs of
    //    `+` / `-` (and `*`), where depth is the run length and the
    //    LAST character picks the side — `++-` is a depth-3 node
    //    hanging left, `----` a depth-4 node hanging left (verified
    //    against PlantUML 1.2024.7). Leading whitespace adds to the
    //    raw depth, same as the mind-map form.
    let indent = line.text.len() - raw.len();
    let marker: Vec<char> = raw
        .chars()
        .take_while(|c| matches!(c, '*' | '+' | '-'))
        .collect();
    let marker_len = marker.len();
    if marker_len == 0 {
        return Err(format!(
            "expected a `*`, `+`, or `-` marker at line {}",
            line.line
        ));
    }
    let depth = indent + marker_len;
    let mut rest = &raw[marker_len..];

    // 2. Optional decorations: `<` / `>` (one of, either side of `_`) and
    //    `_` (shape modifier). PlantUML accepts them in either order. We
    //    tolerate whitespace between the marker and decorations (PlantUML
    //    itself doesn't, but users routinely write `* < Foo` and the
    //    leniency costs nothing). An explicit `<` / `>` overrides the
    //    arithmetic-notation side from the marker run.
    let mut side = match marker.last() {
        Some('+') => NodeSide::Right,
        Some('-') => NodeSide::Left,
        _ => NodeSide::Default,
    };
    let mut explicit_side = false;
    let mut shape = NodeShape::Box;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix('<') {
            if explicit_side {
                return Err(format!(
                    "duplicate WBS direction marker at line {}",
                    line.line
                ));
            }
            side = NodeSide::Left;
            explicit_side = true;
            rest = after;
        } else if let Some(after) = rest.strip_prefix('>') {
            if explicit_side {
                return Err(format!(
                    "duplicate WBS direction marker at line {}",
                    line.line
                ));
            }
            side = NodeSide::Right;
            explicit_side = true;
            rest = after;
        } else if let Some(after) = rest.strip_prefix('_') {
            if shape != NodeShape::Box {
                return Err(format!(
                    "duplicate WBS shape modifier at line {}",
                    line.line
                ));
            }
            shape = NodeShape::Line;
            rest = after;
        } else {
            break;
        }
    }

    // 3. Optional `[#color]`.
    let mut fill = None;
    rest = rest.trim_start();
    if let Some(after) = rest.strip_prefix('[') {
        let close = after
            .find(']')
            .ok_or_else(|| format!("unclosed `[#color]` at line {}", line.line))?;
        let inner = &after[..close];
        if let Some(c) = inner.strip_prefix('#') {
            if c.is_empty() {
                return Err(format!("empty color in `[#…]` at line {}", line.line));
            }
            fill = Some(format!("#{c}"));
        } else {
            return Err(format!(
                "WBS `[…]` decoration must be a `#color` at line {}",
                line.line
            ));
        }
        rest = &after[close + 1..];
    }

    // 4. Optional `(code)`.
    let mut id = None;
    rest = rest.trim_start();
    if let Some(after) = rest.strip_prefix('(') {
        let close = after
            .find(')')
            .ok_or_else(|| format!("unclosed `(code)` at line {}", line.line))?;
        let code = &after[..close];
        if code.is_empty() {
            return Err(format!("empty code in `(…)` at line {}", line.line));
        }
        id = Some(code.to_string());
        rest = &after[close + 1..];
    }

    // 5. Optional whitespace then the label, or `as code` form.
    let label_part = rest.trim_start();

    if let Some(after_as) = strip_keyword(label_part, "as") {
        if id.is_some() {
            return Err(format!(
                "WBS node has both `(code)` and `as code` at line {}",
                line.line
            ));
        }
        if after_as.is_empty() {
            return Err(format!("`as` requires a code at line {}", line.line));
        }
        let code = after_as.to_string();
        return Ok((
            ParsedItem::Marker(ParsedMarker {
                depth,
                node: TreeNode {
                    label: vec![code.clone()],
                    side,
                    shape,
                    fill,
                    id: Some(code),
                    line: line.line,
                    children: Vec::new(),
                },
            }),
            1,
        ));
    }

    // 6. Single-line label, or the multi-line `:line1 … ;` form.
    let (label, consumed) = tree::parse_label(body, start, label_part, "WBS")?;

    // PlantUML "skipping a layer": `_` with NO label removes the node
    // completely — zero-size phantom, children report to the
    // grandparent visually.
    let shape = if shape == NodeShape::Line && label.is_empty() && id.is_none() {
        NodeShape::Phantom
    } else {
        shape
    };

    Ok((
        ParsedItem::Marker(ParsedMarker {
            depth,
            node: TreeNode {
                label,
                side,
                shape,
                fill,
                id,
                line: line.line,
                children: Vec::new(),
            },
        }),
        consumed,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::Error;

    fn block(body: &[&str]) -> UmlBlock {
        UmlBlock {
            start_line: 1,
            kind_tag: "wbs".into(),
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

    fn wbs(d: Diagram) -> WbsDiagram {
        match d {
            Diagram::Wbs(w) => w,
            other => panic!("expected Wbs, got {other:?}"),
        }
    }

    #[test]
    fn parses_three_level_tree() {
        let (d, diags) = parse(
            &block(&[
                "* Root",
                "** A",
                "*** A1",
                "*** A2",
                "** B",
                "*** B1",
            ]),
            CompatMode::Warn,
        )
        .unwrap();
        assert!(diags.is_empty());
        let w = wbs(d);
        assert_eq!(w.root.label, vec!["Root"]);
        assert_eq!(w.root.children.len(), 2);
        assert_eq!(w.root.children[0].label, vec!["A"]);
        assert_eq!(w.root.children[0].children.len(), 2);
        assert_eq!(w.root.children[1].label, vec!["B"]);
        assert_eq!(w.root.children[1].children[0].label, vec!["B1"]);
    }

    #[test]
    fn extracts_title() {
        let (d, _) = parse(
            &block(&["title Org chart", "* CEO", "** CTO"]),
            CompatMode::Warn,
        )
        .unwrap();
        let w = wbs(d);
        assert_eq!(w.title.as_deref(), Some("Org chart"));
        assert_eq!(w.root.label, vec!["CEO"]);
    }

    #[test]
    fn parses_color_shape_and_direction() {
        let (d, _) = parse(
            &block(&["* Root", "**< _ A", "**>[#FF0000] B"]),
            CompatMode::Warn,
        )
        .unwrap();
        let w = wbs(d);
        assert_eq!(w.root.children[0].side, NodeSide::Left);
        assert_eq!(w.root.children[0].shape, NodeShape::Line);
        assert_eq!(w.root.children[0].label, vec!["A"]);
        assert_eq!(w.root.children[1].side, NodeSide::Right);
        assert_eq!(w.root.children[1].fill.as_deref(), Some("#FF0000"));
    }

    #[test]
    fn parses_code_alias_paren_form() {
        let (d, _) = parse(&block(&["* (root) Root", "** A"]), CompatMode::Warn).unwrap();
        let w = wbs(d);
        assert_eq!(w.root.id.as_deref(), Some("root"));
        assert_eq!(w.root.label, vec!["Root"]);
    }

    #[test]
    fn parses_multiline_label() {
        let (d, _) = parse(
            &block(&[
                "* :root header",
                "second line",
                "third;",
                "** child",
            ]),
            CompatMode::Warn,
        )
        .unwrap();
        let w = wbs(d);
        assert_eq!(w.root.label.len(), 3);
        assert_eq!(w.root.label[0], "root header");
        assert_eq!(w.root.label[2], "third");
        assert_eq!(w.root.children.len(), 1);
        assert_eq!(w.root.children[0].label, vec!["child"]);
    }

    #[test]
    fn arithmetic_notation_last_char_picks_side() {
        // PlantUML WBS "arithmetic notation": mixed `+`/`-` runs, depth
        // = run length, LAST char picks the side (`++-` → depth 3,
        // left). Verified against PlantUML 1.2024.7.
        let (d, diags) = parse(
            &block(&[
                "+ Root",
                "++ A",
                "+++ A1",
                "++- B",
                "+++- B1",
                "---- deep-left",
            ]),
            CompatMode::Warn,
        )
        .unwrap();
        assert!(diags.is_empty(), "arithmetic runs are legal: {diags:?}");
        let w = wbs(d);
        assert_eq!(w.root.label, vec!["Root"]);
        // Depth = run length: `++` → level 2, `++-`/`+++` → level 3, ….
        let a = &w.root.children[0];
        assert_eq!(a.side, NodeSide::Right);
        assert_eq!(a.children[0].side, NodeSide::Right); // +++ A1
        let b = &a.children[1];
        assert_eq!(b.label, vec!["B"]);
        assert_eq!(b.side, NodeSide::Left); // ++-
        assert_eq!(b.children[0].label, vec!["B1"]);
        assert_eq!(b.children[0].side, NodeSide::Left); // +++-
        assert_eq!(b.children[1].label, vec!["deep-left"]);
        assert_eq!(b.children[1].side, NodeSide::Left); // ----
    }

    #[test]
    fn labelless_underscore_becomes_phantom() {
        // "Skipping a layer": `_` with no label removes the node.
        let (d, _) = parse(
            &block(&["* Root", "**_", "*** E5", "*** E6", "**_ labeled"]),
            CompatMode::Warn,
        )
        .unwrap();
        let w = wbs(d);
        let phantom = &w.root.children[0];
        assert_eq!(phantom.shape, NodeShape::Phantom);
        assert_eq!(phantom.children.len(), 2);
        assert_eq!(phantom.children[0].label, vec!["E5"]);
        // With a label, `_` stays a visible boxless node.
        assert_eq!(w.root.children[1].shape, NodeShape::Line);
        assert_eq!(w.root.children[1].label, vec!["labeled"]);
    }

    #[test]
    fn explicit_angle_overrides_arithmetic_side() {
        let (d, _) = parse(&block(&["+ Root", "++> right", "++< left"]), CompatMode::Warn)
            .unwrap();
        let w = wbs(d);
        assert_eq!(w.root.children[0].side, NodeSide::Right);
        // `<` wins over the `+` run's right side.
        assert_eq!(w.root.children[1].side, NodeSide::Left);
    }

    #[test]
    fn depth_jump_clamps_to_child() {
        // PlantUML attaches a skipped-level node under the nearest
        // shallower ancestor instead of erroring; the shared driver's
        // raw-depth stack mirrors that (it also enables the indented
        // single-`*` syntax).
        let (d, diags) = parse(
            &block(&["* Root", "*** Skipped grandchild"]),
            CompatMode::Warn,
        )
        .unwrap();
        let w = wbs(d);
        assert!(diags.is_empty(), "clamps silently: {diags:?}");
        assert_eq!(w.root.children.len(), 1);
        assert_eq!(w.root.children[0].label, vec!["Skipped grandchild"]);
    }

    #[test]
    fn rejects_block_without_root() {
        let res = parse(&block(&["title only"]), CompatMode::Warn);
        assert!(matches!(res, Err(Error::Parse { .. })));
    }

    #[test]
    fn skips_comments_and_blank_lines() {
        let (d, _) = parse(
            &block(&["' a comment", "", "* Root", "  ", "** A"]),
            CompatMode::Warn,
        )
        .unwrap();
        let w = wbs(d);
        assert_eq!(w.root.label, vec!["Root"]);
        assert_eq!(w.root.children.len(), 1);
    }

    #[test]
    fn second_root_warns_and_is_dropped() {
        let (d, diags) = parse(
            &block(&["* Root", "** A", "* OtherRoot"]),
            CompatMode::Warn,
        )
        .unwrap();
        let w = wbs(d);
        assert_eq!(w.root.label, vec!["Root"]);
        assert_eq!(w.root.children.len(), 1);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("second WBS root"));
    }
}
