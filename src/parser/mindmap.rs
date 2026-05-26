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

use crate::diagnostics::{CompatMode, Diagnostic, Result};
use crate::ir::{Diagram, MindMapDiagram, NodeShape, NodeSide, TreeNode};
use crate::parser::lexer::{BodyLine, UmlBlock};
use crate::parser::tree::{self, ParsedMarker};

pub fn parse(block: &UmlBlock, compat: CompatMode) -> Result<(Diagram, Vec<Diagnostic>)> {
    let (root, title, diagnostics) = tree::build_tree(block, compat, "mindmap", parse_marker_line)?;
    Ok((
        Diagram::MindMap(MindMapDiagram {
            name: block.name.clone(),
            title,
            root,
        }),
        diagnostics,
    ))
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
    let (label, consumed) = tree::parse_label(body, start, label_part, "mindmap")?;

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
