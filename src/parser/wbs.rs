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

use crate::diagnostics::{CompatMode, Diagnostic, Error, Level, Result};
use crate::ir::{Diagram, NodeShape, NodeSide, TreeNode, WbsDiagram};
use crate::parser::lexer::{BodyLine, UmlBlock};

pub fn parse(block: &UmlBlock, compat: CompatMode) -> Result<(Diagram, Vec<Diagnostic>)> {
    let mut diagnostics = Vec::new();
    let mut title: Option<String> = None;
    let mut root: Option<TreeNode> = None;
    // Depth -> ancestor index path through the tree being built. Each path
    // step is "child index in that node's children array". Walking the
    // path from `root` reaches the node currently sitting at that depth.
    let mut path: Vec<usize> = Vec::new();

    let mut i = 0;
    while i < block.body.len() {
        let line = &block.body[i];
        let trimmed = line.text.trim();

        if trimmed.is_empty() || trimmed.starts_with('\'') || trimmed.starts_with("/'") {
            i += 1;
            continue;
        }

        if let Some(rest) = strip_keyword(trimmed, "title") {
            if !rest.is_empty() {
                title = Some(rest.to_string());
            }
            i += 1;
            continue;
        }
        // caption/header/footer accepted as decoration but not yet rendered;
        // silently swallow so they don't trip the marker parser.
        if strip_keyword(trimmed, "caption").is_some()
            || strip_keyword(trimmed, "header").is_some()
            || strip_keyword(trimmed, "footer").is_some()
        {
            i += 1;
            continue;
        }
        // Skinparam lines aren't honoured yet; treat them as warn-level
        // unsupported instead of as failed marker lines.
        if strip_keyword(trimmed, "skinparam").is_some() {
            diagnostics.push(Diagnostic {
                level: Level::Warning,
                line: Some(line.line),
                message: "skinparam is not yet honoured for WBS diagrams".into(),
            });
            i += 1;
            continue;
        }

        // Marker line. Consume any continuation lines for multi-line labels.
        let consumed = match parse_marker_line(&block.body, i) {
            Ok((node, advance)) => {
                let depth = node.label_depth;
                let new_node = node.into_tree_node();
                if depth == 1 {
                    if root.is_some() {
                        let msg = format!(
                            "second WBS root at line {} (only one `*` root is allowed)",
                            line.line
                        );
                        if compat == CompatMode::Strict {
                            return Err(Error::Parse {
                                line: line.line,
                                message: msg,
                            });
                        }
                        diagnostics.push(Diagnostic {
                            level: Level::Warning,
                            line: Some(line.line),
                            message: msg,
                        });
                        advance
                    } else {
                        root = Some(new_node);
                        path = Vec::new();
                        advance
                    }
                } else if root.is_none() {
                    let msg = format!(
                        "WBS node at depth {depth} appears before any `*` root at line {}",
                        line.line
                    );
                    if compat == CompatMode::Strict {
                        return Err(Error::Parse {
                            line: line.line,
                            message: msg,
                        });
                    }
                    diagnostics.push(Diagnostic {
                        level: Level::Warning,
                        line: Some(line.line),
                        message: msg,
                    });
                    advance
                } else if depth > path.len() + 2 {
                    // path.len() == current_depth - 1 of the last node, so the
                    // legal next depth is path.len() + 1 (sibling of last) or
                    // path.len() + 2 (child of last). Anything further is a
                    // depth jump.
                    let msg = format!(
                        "WBS depth jumped to {depth} without an intermediate parent at line {}",
                        line.line
                    );
                    if compat == CompatMode::Strict {
                        return Err(Error::Parse {
                            line: line.line,
                            message: msg,
                        });
                    }
                    diagnostics.push(Diagnostic {
                        level: Level::Warning,
                        line: Some(line.line),
                        message: msg,
                    });
                    advance
                } else {
                    // Trim path so its length matches depth-2 (i.e. the path
                    // from root to the new node's intended parent has
                    // depth-1 entries; we pop until we're pointing at that
                    // parent).
                    let parent_depth = depth - 1;
                    while path.len() > parent_depth - 1 {
                        path.pop();
                    }
                    let parent = walk_mut(root.as_mut().unwrap(), &path);
                    let new_index = parent.children.len();
                    parent.children.push(new_node);
                    path.push(new_index);
                    advance
                }
            }
            Err(msg) => {
                if compat == CompatMode::Strict {
                    return Err(Error::Parse {
                        line: line.line,
                        message: msg,
                    });
                }
                diagnostics.push(Diagnostic {
                    level: Level::Warning,
                    line: Some(line.line),
                    message: msg,
                });
                1
            }
        };
        i += consumed;
    }

    let root = root.ok_or_else(|| Error::Parse {
        line: block.start_line,
        message: "@startwbs block has no root node (expected a leading `*` line)".into(),
    })?;

    Ok((
        Diagram::Wbs(WbsDiagram {
            name: block.name.clone(),
            title,
            root,
        }),
        diagnostics,
    ))
}

/// Strip a leading PlantUML keyword (`title`, `caption`, …) and return the
/// remaining trimmed text. Returns `None` if the line doesn't start with the
/// keyword followed by whitespace or end-of-line.
fn strip_keyword<'a>(line: &'a str, kw: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(kw)?;
    if rest.is_empty() {
        return Some(rest);
    }
    if rest.starts_with(char::is_whitespace) {
        return Some(rest.trim());
    }
    None
}

/// Owned intermediate produced by `parse_marker_line` — pre-`children` so
/// the depth-stack code can attach it under its parent before recursing.
struct ParsedMarker {
    label_depth: usize,
    node: TreeNode,
}

impl ParsedMarker {
    fn into_tree_node(self) -> TreeNode {
        self.node
    }
}

/// Parse the marker line at `body[start]` plus any continuation lines if it
/// opens a multi-line `:label;` block. Returns the parsed node and the
/// number of body lines consumed (>= 1).
fn parse_marker_line(body: &[BodyLine], start: usize) -> std::result::Result<(ParsedMarker, usize), String> {
    let line = &body[start];
    let raw = line.text.trim_start_matches([' ', '\t']);

    // 1. Markers.
    let marker_len = raw
        .chars()
        .take_while(|c| matches!(c, '*' | '+' | '-'))
        .count();
    if marker_len == 0 {
        return Err(format!(
            "expected a `*`, `+`, or `-` marker at line {}",
            line.line
        ));
    }
    let depth = marker_len;
    let mut rest = &raw[marker_len..];

    // 2. Optional decorations: `<` / `>` (one of, either side of `_`) and
    //    `_` (shape modifier). PlantUML accepts them in either order. We
    //    tolerate whitespace between the marker and decorations (PlantUML
    //    itself doesn't, but users routinely write `* < Foo` and the
    //    leniency costs nothing).
    let mut side = NodeSide::Default;
    let mut shape = NodeShape::Box;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix('<') {
            if side != NodeSide::Default {
                return Err(format!(
                    "duplicate WBS direction marker at line {}",
                    line.line
                ));
            }
            side = NodeSide::Left;
            rest = after;
        } else if let Some(after) = rest.strip_prefix('>') {
            if side != NodeSide::Default {
                return Err(format!(
                    "duplicate WBS direction marker at line {}",
                    line.line
                ));
            }
            side = NodeSide::Right;
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
        id = Some(after_as.to_string());
        return Ok((
            ParsedMarker {
                label_depth: depth,
                node: TreeNode {
                    label: vec![id.clone().unwrap_or_default()],
                    side,
                    shape,
                    fill,
                    id,
                    line: line.line,
                    children: Vec::new(),
                },
            },
            1,
        ));
    }

    // 6. Multi-line `:line1\nline2;` form.
    if let Some(first) = label_part.strip_prefix(':') {
        let mut label_lines = Vec::new();
        let mut consumed = 1;

        // First line: everything after `:`. May include the closing `;`.
        if let Some(end) = first.find(';') {
            label_lines.push(first[..end].to_string());
            return Ok((
                ParsedMarker {
                    label_depth: depth,
                    node: TreeNode {
                        label: label_lines,
                        side,
                        shape,
                        fill,
                        id,
                        line: line.line,
                        children: Vec::new(),
                    },
                },
                consumed,
            ));
        }
        label_lines.push(first.to_string());

        // Continuation lines until one ends with `;`. Verbatim text — we
        // intentionally don't trim, so authors can preserve indentation
        // inside multi-line labels.
        while start + consumed < body.len() {
            let cont = &body[start + consumed];
            consumed += 1;
            if let Some(end) = cont.text.find(';') {
                label_lines.push(cont.text[..end].to_string());
                return Ok((
                    ParsedMarker {
                        label_depth: depth,
                        node: TreeNode {
                            label: label_lines,
                            side,
                            shape,
                            fill,
                            id,
                            line: line.line,
                            children: Vec::new(),
                        },
                    },
                    consumed,
                ));
            }
            label_lines.push(cont.text.clone());
        }

        return Err(format!(
            "unterminated multi-line WBS label opened at line {} (missing `;`)",
            line.line
        ));
    }

    // 7. Single-line label.
    let label = if label_part.is_empty() {
        Vec::new()
    } else {
        vec![label_part.to_string()]
    };

    Ok((
        ParsedMarker {
            label_depth: depth,
            node: TreeNode {
                label,
                side,
                shape,
                fill,
                id,
                line: line.line,
                children: Vec::new(),
            },
        },
        1,
    ))
}

/// Walk `path` (children indices from root) and return a mutable reference
/// to the node at the path's end. Panics on a malformed path; the caller is
/// responsible for keeping `path` in sync with the tree being built.
fn walk_mut<'a>(root: &'a mut TreeNode, path: &[usize]) -> &'a mut TreeNode {
    let mut cur = root;
    for &i in path {
        cur = &mut cur.children[i];
    }
    cur
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn warns_on_depth_jump() {
        let (d, diags) = parse(
            &block(&["* Root", "*** Skipped grandchild"]),
            CompatMode::Warn,
        )
        .unwrap();
        let w = wbs(d);
        // Root keeps no children — the malformed line was dropped.
        assert!(w.root.children.is_empty());
        assert_eq!(diags.len(), 1);
        assert_eq!(diags[0].level, Level::Warning);
    }

    #[test]
    fn strict_mode_rejects_depth_jump() {
        let res = parse(
            &block(&["* Root", "*** Skipped grandchild"]),
            CompatMode::Strict,
        );
        assert!(matches!(res, Err(Error::Parse { .. })));
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
