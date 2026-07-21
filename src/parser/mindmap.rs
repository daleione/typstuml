//! Mind-map parser (`@startmindmap … @endmindmap`).
//!
//! Marker grammar (PlantUML-faithful, verified against 1.2024.7):
//!
//! ```text
//! [\t ]*  ([*]+ | [+]+ | [-]+ | [#]+)  (_)?  (\[#color\])?  \s*  <label>
//! ```
//!
//! - Raw depth = leading whitespace + marker run length; only relative
//!   ordering matters (the shared driver clamps jumps), which is what
//!   makes both the `#` markdown form and the indented single-`*` form
//!   work.
//! - Sides: `+` → right, `-` → left, `*` / `#` → neutral (resolved by
//!   the `left side` / `right side` directives, else auto → right).
//! - Repeated depth-1 nodes produce multiple stacked mind maps.
//! - `[#color]` accepts hex or a PlantUML color name (`[#lightgreen]`).
//!
//! Multi-line labels use the `:line1\nline2;` command form. Body
//! directives (`left side`, `top to bottom direction`, …) surface as
//! [`ParsedItem::Directive`]. Errors degrade to warnings under
//! `--compat warn` (default) and become `Error::Parse` under `strict`.

use crate::diagnostics::{CompatMode, Diagnostic, Result};
use crate::ir::{Diagram, MindMapDiagram, NodeShape, NodeSide, TreeNode};
use crate::parser::lexer::{BodyLine, UmlBlock};
use crate::parser::tree::{self, ParsedItem, ParsedMarker, TreeDirective};

pub fn parse(block: &UmlBlock, compat: CompatMode) -> Result<(Diagram, Vec<Diagnostic>)> {
    let (roots, meta, diagnostics) =
        tree::build_forest(block, compat, "mindmap", parse_marker_line)?;
    Ok((
        Diagram::MindMap(MindMapDiagram {
            name: block.name.clone(),
            title: meta.title,
            roots,
            direction: meta.direction,
        }),
        diagnostics,
    ))
}

fn parse_marker_line(
    body: &[BodyLine],
    start: usize,
) -> std::result::Result<(ParsedItem, usize), String> {
    let line = &body[start];
    let trimmed = line.text.trim();

    // Body directives.
    match trimmed {
        "left side" => return Ok((ParsedItem::Directive(TreeDirective::LeftSide), 1)),
        "right side" => return Ok((ParsedItem::Directive(TreeDirective::RightSide), 1)),
        "top to bottom direction" => {
            return Ok((ParsedItem::Directive(TreeDirective::TopToBottom), 1))
        }
        "left to right direction" => {
            return Ok((ParsedItem::Directive(TreeDirective::LeftToRight), 1))
        }
        _ => {}
    }

    let raw = line.text.trim_start_matches([' ', '\t']);
    let indent = line.text.len() - raw.len();

    // 1. Markers — must be a homogeneous run of `*`, `+`, `-`, or `#`
    //    (the markdown-header form). Mixed runs (e.g. `*+`) are rejected
    //    so the side assignment stays unambiguous.
    let first = raw.chars().next().ok_or("empty mindmap line".to_string())?;
    if !matches!(first, '*' | '+' | '-' | '#') {
        return Err(format!(
            "expected `*`, `+`, `-`, or `#` marker at line {}",
            line.line
        ));
    }
    let marker_len = raw.chars().take_while(|c| *c == first).count();
    // Raw depth: indentation + run length. `+`/`-` count the same as
    // `*` — a single `+` at column 0 is a root, exactly like PlantUML
    // (the old `+1` normalization predated the multi-root support and
    // disagreed with PlantUML's rendering).
    let depth = indent + marker_len;
    let side = match first {
        '+' => NodeSide::Right,
        '-' => NodeSide::Left,
        _ => NodeSide::Default,
    };
    let mut rest = &raw[marker_len..];

    // The character right after the marker run must NOT be one of the other
    // marker chars — that catches `*+` style typos that would otherwise be
    // silently accepted as `marker=*` then `+` floating.
    if let Some(c) = rest.chars().next() {
        if matches!(c, '*' | '+' | '-' | '#') {
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
                // Hex or a color name — both pass through as `#…`; the
                // shared resolver (`crate::colors`) sorts them out at
                // emit time.
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
        ParsedItem::Marker(ParsedMarker {
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
        }),
        consumed,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::MapDirection;

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
        assert_eq!(m.roots.len(), 1);
        assert_eq!(m.roots[0].label, vec!["Brain"]);
        assert_eq!(m.roots[0].children.len(), 2);
        assert_eq!(m.roots[0].children[0].side, NodeSide::Default);
    }

    #[test]
    fn plus_minus_depth_equals_marker_length() {
        // `+ OS` is a root; `++` / `--` are its children with sides.
        let (d, _) = parse(
            &block(&["+ OS", "++ Ubuntu", "+++ Mint", "-- Windows"]),
            CompatMode::Warn,
        )
        .unwrap();
        let m = mm(d);
        assert_eq!(m.roots.len(), 1);
        let root = &m.roots[0];
        assert_eq!(root.label, vec!["OS"]);
        assert_eq!(root.children[0].label, vec!["Ubuntu"]);
        assert_eq!(root.children[0].side, NodeSide::Right);
        assert_eq!(root.children[0].children[0].label, vec!["Mint"]);
        assert_eq!(root.children[1].label, vec!["Windows"]);
        assert_eq!(root.children[1].side, NodeSide::Left);
    }

    #[test]
    fn markdown_headers_build_depth() {
        let (d, _) = parse(
            &block(&["# root", "## first", "### second", "## another"]),
            CompatMode::Warn,
        )
        .unwrap();
        let m = mm(d);
        let root = &m.roots[0];
        assert_eq!(root.label, vec!["root"]);
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].children[0].label, vec!["second"]);
    }

    #[test]
    fn indented_stars_nest_by_indentation() {
        let (d, _) = parse(
            &block(&[
                "* root",
                "    * first",
                "        * second",
                "        * another second",
                "    * another first",
            ]),
            CompatMode::Warn,
        )
        .unwrap();
        let m = mm(d);
        assert_eq!(m.roots.len(), 1, "indented stars must not fork roots");
        let root = &m.roots[0];
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].children.len(), 2);
    }

    #[test]
    fn multiple_roots_are_kept() {
        let (d, diags) = parse(
            &block(&["* Root 1", "** Foo", "* Root 2", "** Lorem"]),
            CompatMode::Warn,
        )
        .unwrap();
        let m = mm(d);
        assert!(diags.is_empty(), "multiroot is legal: {diags:?}");
        assert_eq!(m.roots.len(), 2);
        assert_eq!(m.roots[1].label, vec!["Root 2"]);
        assert_eq!(m.roots[1].children[0].label, vec!["Lorem"]);
    }

    #[test]
    fn side_directives_reassign_neutral_markers() {
        let (d, _) = parse(
            &block(&[
                "+ root",
                "** r1",
                "left side",
                "-- l1",
                "** l2",
            ]),
            CompatMode::Warn,
        )
        .unwrap();
        let m = mm(d);
        let root = &m.roots[0];
        assert_eq!(root.children[0].side, NodeSide::Default); // before directive
        assert_eq!(root.children[1].side, NodeSide::Left); // explicit `-`
        assert_eq!(root.children[2].side, NodeSide::Left); // `*` after `left side`
    }

    #[test]
    fn top_to_bottom_directive_sets_direction() {
        let (d, _) = parse(
            &block(&["top to bottom direction", "* 1", "** 2"]),
            CompatMode::Warn,
        )
        .unwrap();
        assert_eq!(mm(d).direction, MapDirection::TopToBottom);
    }

    #[test]
    fn depth_jump_clamps_to_child() {
        let (d, diags) = parse(&block(&["* Root", "*** deep"]), CompatMode::Warn).unwrap();
        let m = mm(d);
        assert!(diags.is_empty(), "jump clamps silently: {diags:?}");
        assert_eq!(m.roots[0].children.len(), 1);
        assert_eq!(m.roots[0].children[0].label, vec!["deep"]);
    }

    #[test]
    fn rejects_mixed_marker_chars() {
        let (d, diags) = parse(&block(&["* Root", "*+ mixed"]), CompatMode::Warn).unwrap();
        let m = mm(d);
        assert!(m.roots[0].children.is_empty());
        assert!(diags.iter().any(|x| x.message.contains("mixes")));
    }

    #[test]
    fn parses_color_names_and_underscore_shape() {
        let (d, _) = parse(
            &block(&["* Root", "**_ thin", "**[#FFAA88] hexed", "**[#lightgreen] named"]),
            CompatMode::Warn,
        )
        .unwrap();
        let m = mm(d);
        let root = &m.roots[0];
        assert_eq!(root.children[0].shape, NodeShape::Line);
        assert_eq!(root.children[1].fill.as_deref(), Some("#FFAA88"));
        assert_eq!(root.children[2].fill.as_deref(), Some("#lightgreen"));
    }

    #[test]
    fn multiline_label() {
        let (d, _) = parse(
            &block(&["*:Root header", "second line;", "** child"]),
            CompatMode::Warn,
        )
        .unwrap();
        let m = mm(d);
        assert_eq!(m.roots[0].label, vec!["Root header", "second line"]);
        assert_eq!(m.roots[0].children.len(), 1);
        assert_eq!(m.roots[0].children[0].label, vec!["child"]);
    }

    #[test]
    fn multiline_code_block_decodes_escapes_and_strips_fences() {
        // PlantUML's own docs escape semicolons as <U+003B> inside code
        // samples precisely because a line ending in `;` closes the
        // multi-line label — the lone `;` line is the real terminator.
        let (d, _) = parse(
            &block(&[
                "* root",
                "**:Example 1",
                "<code>",
                "template <typename T>",
                "void f1()<U+003B>",
                "</code>",
                ";",
            ]),
            CompatMode::Warn,
        )
        .unwrap();
        let m = mm(d);
        let child = &m.roots[0].children[0];
        assert_eq!(child.label[0], "Example 1");
        assert!(
            child.label.iter().any(|l| l == "void f1();"),
            "escape decoded, fences stripped: {:?}",
            child.label
        );
        assert!(
            !child.label.iter().any(|l| l.contains("<code>")),
            "got: {:?}",
            child.label
        );
    }

    #[test]
    fn style_block_classes_fill_via_stereotype() {
        let (d, _) = parse(
            &block(&[
                "<style>",
                "mindmapDiagram {",
                "  .green {",
                "    BackgroundColor lightgreen",
                "  }",
                "  .rose {",
                "    BackgroundColor #FFBBCC",
                "  }",
                "}",
                "</style>",
                "* Colors",
                "** Green <<green>>",
                "** Rose <<rose>>",
                "** Plain <<unknown>>",
            ]),
            CompatMode::Warn,
        )
        .unwrap();
        let m = mm(d);
        let root = &m.roots[0];
        assert_eq!(root.children[0].label, vec!["Green"]);
        assert_eq!(root.children[0].fill.as_deref(), Some("#lightgreen"));
        assert_eq!(root.children[1].fill.as_deref(), Some("#FFBBCC"));
        // Unknown class: stereotype stripped, no fill.
        assert_eq!(root.children[2].label, vec!["Plain"]);
        assert_eq!(root.children[2].fill, None);
    }

    #[test]
    fn extracts_title() {
        let (d, _) = parse(&block(&["title Cognitive map", "* Brain"]), CompatMode::Warn).unwrap();
        let m = mm(d);
        assert_eq!(m.title.as_deref(), Some("Cognitive map"));
    }

    #[test]
    fn rejects_block_without_root() {
        let res = parse(&block(&["title only"]), CompatMode::Warn);
        assert!(matches!(res, Err(crate::diagnostics::Error::Parse { .. })));
    }
}
