//! Shared driver for the tree-shaped parsers (mind-map and WBS). Both walk
//! the body line-by-line and assemble [`crate::ir::TreeNode`] trees via a
//! raw-depth stack; the per-marker grammar is a caller-supplied closure and
//! the caller wraps the result in its `Diagram` variant.
//!
//! Depth semantics (PlantUML-faithful, verified against 1.2024.7):
//!
//! - A marker line's *raw depth* is leading whitespace + marker run length
//!   (`    *` and `**` both mean "deeper than `*`"). Only the *relative*
//!   ordering matters: a node is attached under the nearest shallower
//!   ancestor, so jumps like `*` → `***` clamp to one level instead of
//!   erroring — exactly how PlantUML's indented syntax works.
//! - A node at (or above) the current root's raw depth starts a **new
//!   root**. Mind maps keep every root (stacked at render time); WBS keeps
//!   the first and warns.
//!
//! The driver also owns the mind-map body directives (`left side`,
//! `right side`, `top to bottom direction`), minimal `<style>` block
//! support (`.class { BackgroundColor <color> }` + `<<class>>`
//! stereotypes), `<U+XXXX>` unicode escapes, and `<code>` fence
//! stripping in multi-line labels.

use std::collections::HashMap;

use crate::diagnostics::{CompatMode, Diagnostic, Error, Level, Result};
use crate::ir::{MapDirection, NodeSide, TreeNode};
use crate::parser::common::strip_keyword_trimmed;
use crate::parser::lexer::{BodyLine, UmlBlock};

/// A marker line parsed into its raw depth and the node it produces,
/// pre-`children` so the depth stack can attach it under its parent.
pub(crate) struct ParsedMarker {
    pub depth: usize,
    pub node: TreeNode,
}

/// One successfully parsed body line: a tree node or a layout directive.
pub(crate) enum ParsedItem {
    Marker(ParsedMarker),
    Directive(TreeDirective),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum TreeDirective {
    /// `left side` — subsequent side-neutral (`*` / `#`) nodes go left.
    LeftSide,
    /// `right side` — subsequent side-neutral nodes go right.
    RightSide,
    /// `top to bottom direction` — transpose the whole mind map.
    TopToBottom,
    /// `left to right direction` — the default; accepted for symmetry.
    LeftToRight,
}

/// Everything `build_forest` extracts from a block besides the trees.
pub(crate) struct ForestMeta {
    pub title: Option<String>,
    pub direction: MapDirection,
}

/// Follow a child-index path from `root` and return the node it points at.
pub(crate) fn walk_mut<'a>(root: &'a mut TreeNode, path: &[usize]) -> &'a mut TreeNode {
    let mut cur = root;
    for &i in path {
        cur = &mut cur.children[i];
    }
    cur
}

/// Drive the raw-depth-stack forest assembly shared by mind-map and WBS.
pub(crate) fn build_forest(
    block: &UmlBlock,
    compat: CompatMode,
    kind: &str,
    parse_marker: impl Fn(&[BodyLine], usize) -> std::result::Result<(ParsedItem, usize), String>,
) -> Result<(Vec<TreeNode>, ForestMeta, Vec<Diagnostic>)> {
    let mut diagnostics = Vec::new();
    let mut title: Option<String> = None;
    let mut direction = MapDirection::LeftToRight;
    let mut default_side = NodeSide::Default;
    let mut styles: HashMap<String, String> = HashMap::new();

    let mut roots: Vec<TreeNode> = Vec::new();
    // Raw depth of each node on the current ancestor chain (`levels[0]`
    // is the current root) and the matching child-index path
    // (`path.len() == levels.len() - 1` whenever a chain exists).
    let mut levels: Vec<usize> = Vec::new();
    let mut path: Vec<usize> = Vec::new();

    let mut i = 0;
    while i < block.body.len() {
        let line = &block.body[i];
        let trimmed = line.text.trim();

        if trimmed.is_empty() || trimmed.starts_with('\'') || trimmed.starts_with("/'") {
            i += 1;
            continue;
        }
        if let Some(rest) = strip_keyword_trimmed(trimmed, "title") {
            if !rest.is_empty() {
                title = Some(rest.to_string());
            }
            i += 1;
            continue;
        }
        // caption/header/footer accepted as decoration but not yet rendered;
        // silently swallow so they don't trip the marker parser.
        if strip_keyword_trimmed(trimmed, "caption").is_some()
            || strip_keyword_trimmed(trimmed, "header").is_some()
            || strip_keyword_trimmed(trimmed, "footer").is_some()
        {
            i += 1;
            continue;
        }
        if strip_keyword_trimmed(trimmed, "skinparam").is_some() {
            diagnostics.push(Diagnostic {
                level: Level::Warning,
                line: Some(line.line),
                message: format!("skinparam is not yet honoured for {kind} diagrams"),
            });
            i += 1;
            continue;
        }
        if trimmed == "<style>" {
            let consumed = parse_style_block(&block.body, i, &mut styles, &mut diagnostics);
            match consumed {
                Ok(n) => {
                    i += n;
                    continue;
                }
                Err(msg) => {
                    report_or_push(&mut diagnostics, compat, line.line, msg)?;
                    i += 1;
                    continue;
                }
            }
        }

        let consumed = match parse_marker(&block.body, i) {
            Ok((ParsedItem::Directive(d), advance)) => {
                match d {
                    TreeDirective::LeftSide => default_side = NodeSide::Left,
                    TreeDirective::RightSide => default_side = NodeSide::Right,
                    TreeDirective::TopToBottom => direction = MapDirection::TopToBottom,
                    TreeDirective::LeftToRight => direction = MapDirection::LeftToRight,
                }
                advance
            }
            Ok((ParsedItem::Marker(parsed), advance)) => {
                let depth = parsed.depth;
                let mut new_node = parsed.node;
                if new_node.side == NodeSide::Default {
                    new_node.side = default_side;
                }
                // Pop until the chain end is strictly shallower.
                while levels.last().map(|&l| l >= depth).unwrap_or(false) {
                    levels.pop();
                    path.pop();
                }
                if levels.is_empty() {
                    roots.push(new_node);
                    path.clear();
                } else {
                    let root = roots.last_mut().expect("levels non-empty implies a root");
                    let parent = walk_mut(root, &path);
                    let new_index = parent.children.len();
                    parent.children.push(new_node);
                    path.push(new_index);
                }
                levels.push(depth);
                advance
            }
            Err(msg) => {
                report_or_push(&mut diagnostics, compat, line.line, msg)?;
                1
            }
        };
        i += consumed;
    }

    if roots.is_empty() {
        return Err(Error::Parse {
            line: block.start_line,
            message: format!(
                "@start{} block has no root node (expected a leading marker line)",
                block.kind_tag
            ),
        });
    }

    for root in &mut roots {
        postprocess_labels(root, &styles);
    }

    Ok((
        roots,
        ForestMeta { title, direction },
        diagnostics,
    ))
}

/// Single-root wrapper for WBS: keeps the first root and warns about
/// the rest (strict mode errors).
pub(crate) fn build_tree(
    block: &UmlBlock,
    compat: CompatMode,
    kind: &str,
    parse_marker: impl Fn(&[BodyLine], usize) -> std::result::Result<(ParsedItem, usize), String>,
) -> Result<(TreeNode, Option<String>, Vec<Diagnostic>)> {
    let (mut roots, meta, mut diagnostics) = build_forest(block, compat, kind, parse_marker)?;
    if roots.len() > 1 {
        let msg = format!(
            "second {kind} root at line {} (only one root is supported)",
            roots[1].line
        );
        report_or_push(&mut diagnostics, compat, roots[1].line, msg)?;
        roots.truncate(1);
    }
    Ok((roots.remove(0), meta.title, diagnostics))
}

// ---------------------------------------------------------------------------
// <style> block
// ---------------------------------------------------------------------------

/// Minimal `<style>` support: collect `.class { … BackgroundColor <v> … }`
/// pairs anywhere inside the block; every other statement is ignored.
/// Returns the number of body lines consumed (including both fences).
fn parse_style_block(
    body: &[BodyLine],
    start: usize,
    styles: &mut HashMap<String, String>,
    _diagnostics: &mut [Diagnostic],
) -> std::result::Result<usize, String> {
    let mut current_class: Option<String> = None;
    let mut j = start + 1;
    while j < body.len() {
        let t = body[j].text.trim();
        if t == "</style>" {
            return Ok(j - start + 1);
        }
        if let Some(rest) = t.strip_prefix('.') {
            // `.name {` (brace optional on the same line)
            let name: String = rest
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if !name.is_empty() {
                current_class = Some(name);
            }
        } else if t == "}" {
            current_class = None;
        } else if let Some(value) = t.strip_prefix("BackgroundColor") {
            if let Some(class) = &current_class {
                let value = value.trim();
                if !value.is_empty() {
                    let spec = if value.starts_with('#') {
                        value.to_string()
                    } else {
                        format!("#{value}")
                    };
                    styles.insert(class.clone(), spec);
                }
            }
        }
        j += 1;
    }
    Err(format!(
        "unclosed <style> block opened at line {}",
        body[start].line
    ))
}

// ---------------------------------------------------------------------------
// Label post-processing
// ---------------------------------------------------------------------------

/// Applied once per node after tree assembly:
/// 1. `<code>` / `</code>` fence lines are dropped (content kept verbatim;
///    monospace rendering is a follow-up).
/// 2. A trailing `<<class>>` stereotype is stripped and, when the class
///    was declared in a `<style>` block, resolves to the node fill
///    (an explicit `[#color]` wins).
/// 3. `<U+XXXX>` escapes decode to their character.
fn postprocess_labels(node: &mut TreeNode, styles: &HashMap<String, String>) {
    node.label.retain(|l| {
        let t = l.trim();
        t != "<code>" && t != "</code>"
    });
    for line in &mut node.label {
        if let Some((stripped, class)) = split_stereotype(line) {
            if node.fill.is_none() {
                if let Some(spec) = styles.get(&class) {
                    node.fill = Some(spec.clone());
                }
            }
            *line = stripped;
        }
        *line = decode_unicode_escapes(line);
    }
    node.label.retain(|l| !l.trim().is_empty());
    if node.label.is_empty() {
        node.label.push(" ".to_string());
    }
    for child in &mut node.children {
        postprocess_labels(child, styles);
    }
}

/// `"Green <<green>>"` → `("Green", "green")`. Only a trailing
/// stereotype counts; `<<…>>` mid-label is left alone.
fn split_stereotype(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_end();
    let rest = trimmed.strip_suffix(">>")?;
    let open = rest.rfind("<<")?;
    let name = rest[open + 2..].trim();
    if name.is_empty() || name.contains('<') || name.contains('>') {
        return None;
    }
    Some((trimmed[..open].trim_end().to_string(), name.to_string()))
}

/// Replace every `<U+XXXX>` (2–6 hex digits) with its character.
/// Malformed escapes stay verbatim.
fn decode_unicode_escapes(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(pos) = rest.find("<U+") {
        out.push_str(&rest[..pos]);
        let after = &rest[pos + 3..];
        if let Some(close) = after.find('>') {
            let hex = &after[..close];
            let valid = (2..=6).contains(&hex.len())
                && hex.chars().all(|c| c.is_ascii_hexdigit());
            if valid {
                if let Some(c) =
                    u32::from_str_radix(hex, 16).ok().and_then(char::from_u32)
                {
                    out.push(c);
                    rest = &after[close + 1..];
                    continue;
                }
            }
        }
        // Not a well-formed escape — emit `<U+` literally and move on.
        out.push_str("<U+");
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Hard error under `--compat strict`, a warning otherwise.
fn report_or_push(
    diagnostics: &mut Vec<Diagnostic>,
    compat: CompatMode,
    line: usize,
    message: String,
) -> Result<()> {
    if compat == CompatMode::Strict {
        return Err(Error::Parse { line, message });
    }
    diagnostics.push(Diagnostic {
        level: Level::Warning,
        line: Some(line),
        message,
    });
    Ok(())
}

/// Parse the label portion of a marker line, after the marker and decorations
/// have been stripped. Handles both a plain single-line label and the
/// multi-line `:line1 … ;` form, returning the label lines and how many body
/// lines were consumed (>= 1). `kind` only flavors the unterminated-label
/// error message.
pub(crate) fn parse_label(
    body: &[BodyLine],
    start: usize,
    label_part: &str,
    kind: &str,
) -> std::result::Result<(Vec<String>, usize), String> {
    let line_no = body[start].line;

    if let Some(first_text) = label_part.strip_prefix(':') {
        let mut label_lines = Vec::new();
        let mut consumed = 1;
        // First line: everything after `:`. Closes immediately when it
        // ENDS with `;` (interior semicolons — code samples — don't
        // terminate; same rule as continuation lines below).
        if let Some(without) = first_text.trim_end().strip_suffix(';') {
            label_lines.push(without.to_string());
            return Ok((label_lines, consumed));
        }
        label_lines.push(first_text.to_string());
        // Continuation lines until one ENDS with `;` (a bare `;` inside —
        // e.g. code samples — should not terminate early; PlantUML closes
        // on the line whose last character is the semicolon).
        while start + consumed < body.len() {
            let cont = &body[start + consumed];
            consumed += 1;
            let trimmed_end = cont.text.trim_end();
            if let Some(without) = trimmed_end.strip_suffix(';') {
                if !without.is_empty() {
                    label_lines.push(without.to_string());
                }
                return Ok((label_lines, consumed));
            }
            label_lines.push(cont.text.clone());
        }
        return Err(format!(
            "unterminated multi-line {kind} label opened at line {line_no} (missing `;`)"
        ));
    }

    let label = if label_part.is_empty() {
        Vec::new()
    } else {
        vec![label_part.to_string()]
    };
    Ok((label, 1))
}
