//! YAML diagram parser.
//!
//! `@startyaml … @endyaml` wraps a YAML document. We delegate to
//! `serde_yaml_ng` (an actively-maintained drop-in fork of the original
//! `serde_yaml`) and deserialize directly into a `serde_json::Value` so the
//! resulting tree shares the JSON codegen path.
//!
//! A leading `title <text>` line (PUML-style, before the YAML body) is
//! supported and stripped before parsing — matching the JSON parser's
//! behavior. Comments / blank lines preceding the title are skipped.
//!
//! YAML errors carry an optional `Location` with a 1-based line number
//! relative to the slice we passed; we map that back to the original
//! source line via the lexer's [`BodyLine`] metadata.

use crate::diagnostics::{CompatMode, Diagnostic, Error, Result};
use crate::ir::{Diagram, YamlDiagram};
use crate::parser::lexer::{BodyLine, UmlBlock};

pub fn parse(block: &UmlBlock, _compat: CompatMode) -> Result<(Diagram, Vec<Diagnostic>)> {
    let (title, yaml_lines) = split_off_title(&block.body);

    if yaml_lines.is_empty() {
        return Err(Error::Parse {
            line: block.start_line,
            message: "@startyaml block has no YAML content".into(),
        });
    }

    let mut joined = String::new();
    for (i, l) in yaml_lines.iter().enumerate() {
        if i > 0 {
            joined.push('\n');
        }
        joined.push_str(&l.text);
    }

    let root: serde_json::Value = serde_yaml_ng::from_str(&joined).map_err(|e| {
        let local_line = e.location().map(|loc| loc.line()).unwrap_or(1);
        let original_line = yaml_lines
            .get(local_line.saturating_sub(1))
            .map(|l| l.line)
            .unwrap_or(block.start_line);
        Error::Parse {
            line: original_line,
            message: format!("invalid YAML: {e}"),
        }
    })?;

    Ok((
        Diagram::Yaml(YamlDiagram {
            name: block.name.clone(),
            title,
            root,
        }),
        Vec::new(),
    ))
}

/// Pull a leading `title <text>` line off the body. Comments / blanks
/// before the title are skipped along with it.
fn split_off_title(body: &[BodyLine]) -> (Option<String>, Vec<BodyLine>) {
    let mut title = None;
    let mut idx = 0;
    while idx < body.len() {
        let trimmed = body[idx].text.trim();
        if trimmed.is_empty() || trimmed.starts_with('\'') || trimmed.starts_with("/'") {
            idx += 1;
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix("title")
            .filter(|r| r.is_empty() || r.starts_with(char::is_whitespace))
        {
            let t = rest.trim();
            if !t.is_empty() {
                title = Some(t.to_string());
            }
            idx += 1;
            continue;
        }
        break;
    }
    (title, body[idx..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(body: &[&str]) -> UmlBlock {
        UmlBlock {
            start_line: 1,
            kind_tag: "yaml".into(),
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
    fn parses_simple_mapping() {
        let (diagram, _) = parse(
            &block(&["doe:", "  name: John", "  age: 30"]),
            CompatMode::Warn,
        )
        .unwrap();
        let Diagram::Yaml(y) = diagram else {
            panic!("expected yaml")
        };
        assert!(y.root.is_object());
        assert_eq!(y.root["doe"]["name"], "John");
        assert_eq!(y.root["doe"]["age"], 30);
    }

    #[test]
    fn extracts_title_then_yaml() {
        let (diagram, _) = parse(
            &block(&["title People", "- Alice", "- Bob"]),
            CompatMode::Warn,
        )
        .unwrap();
        let Diagram::Yaml(y) = diagram else {
            panic!()
        };
        assert_eq!(y.title.as_deref(), Some("People"));
        assert!(y.root.is_array());
        assert_eq!(y.root[0], "Alice");
    }

    #[test]
    fn flow_style_is_supported() {
        let (diagram, _) = parse(
            &block(&["{name: Alice, scores: [1, 2, 3]}"]),
            CompatMode::Warn,
        )
        .unwrap();
        let Diagram::Yaml(y) = diagram else {
            panic!()
        };
        assert_eq!(y.root["name"], "Alice");
        assert_eq!(y.root["scores"][2], 3);
    }

    #[test]
    fn null_variants_become_json_null() {
        let (diagram, _) = parse(
            &block(&["a: ~", "b: null", "c:"]),
            CompatMode::Warn,
        )
        .unwrap();
        let Diagram::Yaml(y) = diagram else {
            panic!()
        };
        assert!(y.root["a"].is_null());
        assert!(y.root["b"].is_null());
        assert!(y.root["c"].is_null());
    }

    #[test]
    fn syntax_error_maps_to_a_body_line() {
        // Tab indentation is invalid in YAML.
        let res = parse(
            &block(&["root:", "\tchild: 1"]),
            CompatMode::Warn,
        );
        let err = res.unwrap_err();
        match err {
            Error::Parse { line, .. } => assert!((1..=2).contains(&line)),
            other => panic!("expected Parse error, got {other:?}"),
        }
    }
}
