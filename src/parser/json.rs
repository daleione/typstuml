//! JSON diagram parser.
//!
//! PlantUML's `@startjson … @endjson` block is just a JSON document — we
//! delegate the heavy lifting to `serde_json` and wrap the parsed value in a
//! [`JsonDiagram`]. A leading `title` line (PUML-style, before the JSON body)
//! is supported; everything else inside the block must be valid JSON.
//!
//! `serde_json` reports errors with absolute byte offsets relative to the
//! string we feed it. We translate those back to original-source line numbers
//! using the [`BodyLine`](crate::parser::lexer::BodyLine) metadata kept by the lexer.

use crate::diagnostics::{CompatMode, Diagnostic, Error, Result};
use crate::ir::{Diagram, JsonDiagram};
use crate::parser::common::split_off_title;
use crate::parser::lexer::UmlBlock;

pub fn parse(block: &UmlBlock, _compat: CompatMode) -> Result<(Diagram, Vec<Diagnostic>)> {
    let (title, json_lines) = split_off_title(&block.body);

    if json_lines.is_empty() {
        return Err(Error::Parse {
            line: block.start_line,
            message: "@startjson block has no JSON content".into(),
        });
    }

    // Re-join the JSON portion preserving newlines so serde_json offsets can
    // map back to original lines.
    let mut joined = String::new();
    for (i, l) in json_lines.iter().enumerate() {
        if i > 0 {
            joined.push('\n');
        }
        joined.push_str(&l.text);
    }

    let root: serde_json::Value = serde_json::from_str(&joined).map_err(|e| {
        // serde_json line numbers are 1-based within the slice we passed.
        let local_line = e.line();
        let original_line = json_lines
            .get(local_line.saturating_sub(1))
            .map(|l| l.line)
            .unwrap_or(block.start_line);
        Error::Parse {
            line: original_line,
            message: format!("invalid JSON: {e}"),
        }
    })?;

    Ok((
        Diagram::Json(JsonDiagram {
            name: block.name.clone(),
            title,
            root,
        }),
        Vec::new(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parser::lexer::BodyLine;

    fn block(body: &[&str]) -> UmlBlock {
        UmlBlock {
            start_line: 1,
            kind_tag: "json".into(),
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

    #[test]
    fn parses_simple_object() {
        let (diagram, _) = parse(&block(&["{\"a\": 1, \"b\": [2, 3]}"]), CompatMode::Warn).unwrap();
        let Diagram::Json(j) = diagram else {
            panic!("expected json")
        };
        assert!(j.root.is_object());
        assert_eq!(j.root["a"], 1);
        assert_eq!(j.root["b"][1], 3);
    }

    #[test]
    fn extracts_title_then_json() {
        let (diagram, _) = parse(
            &block(&["title People", "[", "  \"Alice\",", "  \"Bob\"", "]"]),
            CompatMode::Warn,
        )
        .unwrap();
        let Diagram::Json(j) = diagram else { panic!() };
        assert_eq!(j.title.as_deref(), Some("People"));
        assert!(j.root.is_array());
        assert_eq!(j.root[0], "Alice");
    }

    #[test]
    fn syntax_error_maps_to_original_line() {
        // body line 2 (`,`) is the broken line — but it's at original index 2 too.
        let res = parse(&block(&["{", "  \"k\": ,", "}"]), CompatMode::Warn);
        let err = res.unwrap_err();
        match err {
            Error::Parse { line, .. } => assert_eq!(line, 2),
            other => panic!("expected Parse error, got {other:?}"),
        }
    }
}
