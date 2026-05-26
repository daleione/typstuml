//! Shared driver for the tree-shaped parsers (mind-map and WBS). Both walk
//! the body line-by-line and assemble a [`crate::ir::TreeNode`] via a depth
//! stack; the only real differences are the per-marker grammar (handled by a
//! caller-supplied closure) and which `Diagram` variant the root is wrapped
//! in (handled by the caller after `build_tree` returns).

use crate::diagnostics::{CompatMode, Diagnostic, Error, Level, Result};
use crate::ir::TreeNode;
use crate::parser::common::strip_keyword_trimmed;
use crate::parser::lexer::{BodyLine, UmlBlock};

/// A marker line parsed into its tree depth (1 = root) and the node it
/// produces, pre-`children` so the depth stack can attach it under its parent.
pub(crate) struct ParsedMarker {
    pub depth: usize,
    pub node: TreeNode,
}

/// Follow a child-index path from `root` and return the node it points at.
pub(crate) fn walk_mut<'a>(root: &'a mut TreeNode, path: &[usize]) -> &'a mut TreeNode {
    let mut cur = root;
    for &i in path {
        cur = &mut cur.children[i];
    }
    cur
}

/// Drive the depth-stack tree assembly shared by mind-map and WBS.
///
/// Skips the common decoration preamble (`title` / `caption` / `header` /
/// `footer` / `skinparam`), then for each remaining line calls `parse_marker`
/// and attaches the result under its parent. `kind` (`"mindmap"` / `"WBS"`)
/// only flavors diagnostic messages. Returns the assembled root, the optional
/// title, and any diagnostics; the caller wraps the root in its `Diagram`.
pub(crate) fn build_tree(
    block: &UmlBlock,
    compat: CompatMode,
    kind: &str,
    parse_marker: impl Fn(&[BodyLine], usize) -> std::result::Result<(ParsedMarker, usize), String>,
) -> Result<(TreeNode, Option<String>, Vec<Diagnostic>)> {
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

        let consumed = match parse_marker(&block.body, i) {
            Ok((parsed, advance)) => {
                let depth = parsed.depth;
                let new_node = parsed.node;
                if depth == 1 {
                    if root.is_some() {
                        let msg =
                            format!("second {kind} root at line {} (only one root is allowed)", line.line);
                        report_or_push(&mut diagnostics, compat, line.line, msg)?;
                        advance
                    } else {
                        root = Some(new_node);
                        path = Vec::new();
                        advance
                    }
                } else if root.is_none() {
                    let msg = format!(
                        "{kind} node at depth {depth} appears before any root at line {}",
                        line.line
                    );
                    report_or_push(&mut diagnostics, compat, line.line, msg)?;
                    advance
                } else if depth > path.len() + 2 {
                    // path.len() == current_depth - 1 of the last node, so the
                    // legal next depth is path.len() + 1 (sibling) or + 2
                    // (child). Anything further is a depth jump.
                    let msg = format!(
                        "{kind} depth jumped to {depth} without an intermediate parent at line {}",
                        line.line
                    );
                    report_or_push(&mut diagnostics, compat, line.line, msg)?;
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
                report_or_push(&mut diagnostics, compat, line.line, msg)?;
                1
            }
        };
        i += consumed;
    }

    let root = root.ok_or_else(|| Error::Parse {
        line: block.start_line,
        message: format!(
            "@start{} block has no root node (expected a leading marker line)",
            block.kind_tag
        ),
    })?;

    Ok((root, title, diagnostics))
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
        // First line: everything after `:`. May include the closing `;`.
        if let Some(end) = first_text.find(';') {
            label_lines.push(first_text[..end].to_string());
            return Ok((label_lines, consumed));
        }
        label_lines.push(first_text.to_string());
        // Continuation lines until one contains `;`. Verbatim text — we
        // intentionally don't trim, so authors can preserve indentation
        // inside multi-line labels.
        while start + consumed < body.len() {
            let cont = &body[start + consumed];
            consumed += 1;
            if let Some(end) = cont.text.find(';') {
                label_lines.push(cont.text[..end].to_string());
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
