//! Mind-map parser (`@startmindmap … @endmindmap`).
//!
//! Walks the body line-by-line and assembles a [`TreeNode`] tree using a
//! depth stack — same skeleton as the WBS parser, with a different marker
//! grammar:
//!
//! ```text
//! [\t ]*  ([*]+ | [+]+ | [-]+)  (_)?  (\[#color\])?  \s*  <label>
//! ```
//!
//! Side assignment per `docs/mindmap-wbs-plan.md` §3.2:
//! - `+` markers → right side
//! - `-` markers → left side
//! - `*` markers → default (codegen treats Default as right in v1)
//!
//! Multi-line labels use the `:line1\nline2;` command form, identical to
//! WBS. `(code)` / `as code` are NOT recognized — PlantUML mindmaps don't
//! support node aliases. Errors degrade to warnings under `--compat warn`
//! (default) and become `Error::Parse` under `--compat strict`.

use crate::diagnostics::{CompatMode, Diagnostic, Error, Level, Result};
use crate::ir::{Diagram, MindMapDiagram, NodeShape, NodeSide, TreeNode};
use crate::parser::common::strip_keyword_trimmed as strip_keyword;
use crate::parser::tree::walk_mut;
use crate::parser::lexer::{BodyLine, UmlBlock};

pub fn parse(block: &UmlBlock, compat: CompatMode) -> Result<(Diagram, Vec<Diagnostic>)> {
    let mut diagnostics = Vec::new();
    let mut title: Option<String> = None;
    let mut root: Option<TreeNode> = None;
    // Depth -> ancestor index path through the tree being built. Each path
    // step is the child index in that node's children array.
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
        if strip_keyword(trimmed, "caption").is_some()
            || strip_keyword(trimmed, "header").is_some()
            || strip_keyword(trimmed, "footer").is_some()
        {
            i += 1;
            continue;
        }
        if strip_keyword(trimmed, "skinparam").is_some() {
            diagnostics.push(Diagnostic {
                level: Level::Warning,
                line: Some(line.line),
                message: "skinparam is not yet honoured for mindmap diagrams".into(),
            });
            i += 1;
            continue;
        }

        let consumed = match parse_marker_line(&block.body, i) {
            Ok((parsed, advance)) => {
                let depth = parsed.depth;
                let new_node = parsed.node;
                if depth == 1 {
                    if root.is_some() {
                        let msg = format!(
                            "second mindmap root at line {} (only one root is allowed)",
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
                        "mindmap node at depth {depth} appears before any root at line {}",
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
                    let msg = format!(
                        "mindmap depth jumped to {depth} without an intermediate parent at line {}",
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
        message: "@startmindmap block has no root node (expected a leading marker line)".into(),
    })?;

    Ok((
        Diagram::MindMap(MindMapDiagram {
            name: block.name.clone(),
            title,
            root,
        }),
        diagnostics,
    ))
}

struct ParsedMarker {
    depth: usize,
    node: TreeNode,
}

fn parse_marker_line(
    body: &[BodyLine],
    start: usize,
) -> std::result::Result<(ParsedMarker, usize), String> {
    let line = &body[start];
    let raw = line.text.trim_start_matches([' ', '\t']);

    // 1. Markers — must be a homogeneous run of `*`, `+`, or `-`. Mixed
    //    runs (e.g. `*+`) are rejected so the side assignment stays
    //    unambiguous.
    let first = raw.chars().next().ok_or("empty mindmap line".to_string())?;
    if !matches!(first, '*' | '+' | '-') {
        return Err(format!(
            "expected `*`, `+`, or `-` marker at line {}",
            line.line
        ));
    }
    let marker_len = raw.chars().take_while(|c| *c == first).count();
    // PlantUML semantics: `*` markers are depth-as-counted (root is `*`,
    // children are `**`, `***`, …). `+`/`-` markers are exclusively non-root
    // — they always sit at least one level below the implicit `*` root, so
    // their depth is `count + 1`. This is why a single `+` lands as a depth-2
    // child of the root instead of becoming a second root.
    let (depth, side) = match first {
        '+' => (marker_len + 1, NodeSide::Right),
        '-' => (marker_len + 1, NodeSide::Left),
        _ => (marker_len, NodeSide::Default),
    };
    let mut rest = &raw[marker_len..];

    // The character right after the marker run must NOT be one of the other
    // marker chars — that catches `*+` style typos that would otherwise be
    // silently accepted as `marker=*` then `+` floating.
    if let Some(c) = rest.chars().next() {
        if matches!(c, '*' | '+' | '-') {
            return Err(format!(
                "mindmap marker run at line {} mixes `{first}` and `{c}` — pick one",
                line.line
            ));
        }
    }

    // 2. + 3. Optional `_` shape modifier and `[#color]` background, in
    //    either order. PlantUML's grammar puts `_` strictly before
    //    `[#color]`, but users routinely flip them and the leniency is
    //    free; we just consume whichever appears first, then loop.
    let mut shape = NodeShape::Box;
    let mut fill = None;
    loop {
        rest = rest.trim_start();
        if let Some(after) = rest.strip_prefix('_') {
            if shape == NodeShape::Line {
                return Err(format!(
                    "duplicate `_` shape modifier at line {}",
                    line.line
                ));
            }
            shape = NodeShape::Line;
            rest = after;
        } else if let Some(after) = rest.strip_prefix('[') {
            if fill.is_some() {
                return Err(format!(
                    "duplicate `[#color]` decoration at line {}",
                    line.line
                ));
            }
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
                    "mindmap `[…]` decoration must be a `#color` at line {}",
                    line.line
                ));
            }
            rest = &after[close + 1..];
        } else {
            break;
        }
    }

    // 4. Optional `:label\n…;` multi-line form, otherwise a plain label.
    let label_part = rest.trim_start();

    if let Some(first_text) = label_part.strip_prefix(':') {
        let mut label_lines = Vec::new();
        let mut consumed = 1;
        if let Some(end) = first_text.find(';') {
            label_lines.push(first_text[..end].to_string());
            return Ok((
                ParsedMarker {
                    depth,
                    node: TreeNode {
                        label: label_lines,
                        side,
                        shape,
                        fill,
                        id: None,
                        line: line.line,
                        children: Vec::new(),
                    },
                },
                consumed,
            ));
        }
        label_lines.push(first_text.to_string());
        while start + consumed < body.len() {
            let cont = &body[start + consumed];
            consumed += 1;
            if let Some(end) = cont.text.find(';') {
                label_lines.push(cont.text[..end].to_string());
                return Ok((
                    ParsedMarker {
                        depth,
                        node: TreeNode {
                            label: label_lines,
                            side,
                            shape,
                            fill,
                            id: None,
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
            "unterminated multi-line mindmap label opened at line {} (missing `;`)",
            line.line
        ));
    }

    let label = if label_part.is_empty() {
        Vec::new()
    } else {
        vec![label_part.to_string()]
    };

    Ok((
        ParsedMarker {
            depth,
            node: TreeNode {
                label,
                side,
                shape,
                fill,
                id: None,
                line: line.line,
                children: Vec::new(),
            },
        },
        1,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(body: &[&str]) -> UmlBlock {
        UmlBlock {
            start_line: 1,
            kind_tag: "mindmap".into(),
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

    fn mm(d: Diagram) -> MindMapDiagram {
        match d {
            Diagram::MindMap(m) => m,
            other => panic!("expected MindMap, got {other:?}"),
        }
    }

    #[test]
    fn star_form_classifies_root_as_default() {
        let (d, _) = parse(
            &block(&["* Brain", "** memory", "** vision"]),
            CompatMode::Warn,
        )
        .unwrap();
        let m = mm(d);
        assert_eq!(m.root.label, vec!["Brain"]);
        assert_eq!(m.root.children.len(), 2);
        assert_eq!(m.root.children[0].side, NodeSide::Default);
    }

    #[test]
    fn plus_minus_form_classifies_sides() {
        let (d, _) = parse(
            &block(&["* OS", "+ Engineering", "++ Backend", "- Marketing"]),
            CompatMode::Warn,
        )
        .unwrap();
        let m = mm(d);
        assert_eq!(m.root.children[0].label, vec!["Engineering"]);
        assert_eq!(m.root.children[0].side, NodeSide::Right);
        assert_eq!(m.root.children[0].children[0].side, NodeSide::Right);
        assert_eq!(m.root.children[1].side, NodeSide::Left);
    }

    #[test]
    fn rejects_mixed_marker_chars() {
        let (d, diags) = parse(&block(&["* Root", "*+ mixed"]), CompatMode::Warn).unwrap();
        let m = mm(d);
        assert!(m.root.children.is_empty());
        assert!(diags.iter().any(|x| x.message.contains("mixes")));
    }

    #[test]
    fn parses_color_and_underscore_shape() {
        let (d, _) = parse(
            &block(&["* Root", "+_ thin", "-[#FFAA88] orange"]),
            CompatMode::Warn,
        )
        .unwrap();
        let m = mm(d);
        assert_eq!(m.root.children[0].shape, NodeShape::Line);
        assert_eq!(m.root.children[1].fill.as_deref(), Some("#FFAA88"));
    }

    #[test]
    fn multiline_label() {
        let (d, _) = parse(
            &block(&["* :Root header", "second line;", "+ child"]),
            CompatMode::Warn,
        )
        .unwrap();
        let m = mm(d);
        assert_eq!(m.root.label, vec!["Root header", "second line"]);
        assert_eq!(m.root.children.len(), 1);
        assert_eq!(m.root.children[0].label, vec!["child"]);
    }

    #[test]
    fn extracts_title() {
        let (d, _) = parse(&block(&["title Cognitive map", "* Brain"]), CompatMode::Warn).unwrap();
        let m = mm(d);
        assert_eq!(m.title.as_deref(), Some("Cognitive map"));
    }

    #[test]
    fn warns_on_depth_jump() {
        let (d, diags) = parse(&block(&["* Root", "+++ skipped"]), CompatMode::Warn).unwrap();
        let m = mm(d);
        assert!(m.root.children.is_empty());
        assert!(!diags.is_empty());
    }

    #[test]
    fn strict_mode_rejects_orphan() {
        let res = parse(&block(&["+ orphan"]), CompatMode::Strict);
        assert!(matches!(res, Err(Error::Parse { .. })));
    }

    #[test]
    fn rejects_block_without_root() {
        let res = parse(&block(&["title only"]), CompatMode::Warn);
        assert!(matches!(res, Err(Error::Parse { .. })));
    }
}
