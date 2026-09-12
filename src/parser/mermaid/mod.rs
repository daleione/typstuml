//! Native Mermaid flowchart subset. Parsing never invokes the PlantUML preprocessor.
mod ast;
mod flowchart;
mod lexer;
mod lower;

use super::ParseOutput;
use crate::diagnostics::{CompatMode, Diagnostic, Error, Result};
use crate::ir::*;
use lexer::SourceSpan;
use std::collections::HashSet;

fn recover(
    error: Error,
    span: SourceSpan,
    compat: CompatMode,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<()> {
    if compat == CompatMode::Strict || !matches!(error, Error::Unsupported { .. }) {
        return Err(error);
    }
    if compat == CompatMode::Warn {
        let mut diagnostic =
            Diagnostic::warning_at(span.line, format!("{error}; entire statement skipped"));
        diagnostic.column = Some(span.column);
        diagnostics.push(diagnostic);
    }
    Ok(())
}

pub fn parse(source: &str, compat: CompatMode) -> Result<ParseOutput> {
    let statements = lexer::statements(source.trim_start_matches('\u{feff}'))?;
    let mut diagnostics = Vec::new();
    let mut iter = statements.into_iter();
    let header = loop {
        let statement = iter.next().ok_or_else(|| Error::Parse {
            line: 1,
            message: "expected Mermaid flowchart header".into(),
        })?;
        if statement.text.starts_with("%%{") {
            recover(
                Error::Unsupported {
                    kind: "Mermaid directive",
                    detail: statement.text.into(),
                },
                statement.span,
                compat,
                &mut diagnostics,
            )?;
        } else {
            break statement;
        }
    };
    let words: Vec<_> = header.text.split_whitespace().collect();
    if !matches!(words[0], "flowchart" | "graph") {
        return Err(Error::Unsupported {
            kind: "Mermaid diagram type",
            detail: format!(
                "line {}: {:?}; only flowchart/graph supported",
                header.span.line, words[0]
            ),
        });
    }
    if words.len() != 2 {
        return Err(Error::Parse {
            line: header.span.line,
            message: "expected flowchart/graph followed by TD, TB, BT, LR or RL".into(),
        });
    }
    let (direction, reverse_rank) = match words[1] {
        "TD" | "TB" => (LayoutDirection::TopToBottom, false),
        "BT" => (LayoutDirection::TopToBottom, true),
        "LR" => (LayoutDirection::LeftToRight, false),
        "RL" => (LayoutDirection::LeftToRight, true),
        _ => {
            return Err(Error::Parse {
                line: header.span.line,
                message: format!("unknown flowchart direction {:?}", words[1]),
            })
        }
    };
    let mut diag = FlowchartDiagram {
        direction,
        reverse_rank,
        ..FlowchartDiagram::default()
    };
    let mut stack: Vec<usize> = Vec::new();
    let mut subgraphs = HashSet::new();
    let mut chains = Vec::new();
    for statement in iter {
        let keyword = statement.text.split_whitespace().next().unwrap_or("");
        if matches!(keyword, "flowchart" | "graph") {
            return Err(Error::Parse {
                line: statement.span.line,
                message: "only one Mermaid diagram per source is supported".into(),
            });
        }
        if statement.text == "end" {
            if stack.pop().is_none() {
                return Err(Error::Parse {
                    line: statement.span.line,
                    message: "unexpected subgraph end".into(),
                });
            }
            continue;
        }
        if keyword == "subgraph" {
            let raw = statement.text[8..].trim();
            let (id, label) = if raw.starts_with('"') {
                (
                    raw.to_owned(),
                    flowchart::decode_label(raw, statement.span.line)?,
                )
            } else if let Some(i) = raw.find('[') {
                let mut cursor = flowchart::Cursor::new(raw[..i].trim(), statement.span.line);
                let id = cursor.id()?;
                cursor.ws();
                if !cursor.rest.is_empty() {
                    return Err(cursor.unsupported("invalid subgraph ID"));
                }
                let mut cursor = flowchart::Cursor::new(&raw[i + 1..], statement.span.line);
                let label = cursor.label("]")?;
                cursor.ws();
                if !cursor.rest.is_empty() {
                    return Err(cursor.unsupported("unexpected text after subgraph label"));
                }
                (id, label)
            } else {
                if raw.is_empty() {
                    return Err(Error::Parse {
                        line: statement.span.line,
                        message: "subgraph needs a label".into(),
                    });
                }
                (
                    raw.to_owned(),
                    flowchart::decode_label(raw, statement.span.line)?,
                )
            };
            if !subgraphs.insert(id.clone()) {
                return Err(Error::Parse {
                    line: statement.span.line,
                    message: format!("duplicate subgraph {id:?}"),
                });
            }
            let i = diag.subgraphs.len();
            diag.subgraphs.push(FlowSubgraph {
                id,
                label,
                nodes: vec![],
                children: vec![],
                line: statement.span.line,
            });
            if let Some(parent) = stack.last() {
                diag.subgraphs[*parent].children.push(i);
            }
            stack.push(i);
            continue;
        }
        let result = if statement.text.starts_with("%%{")
            || matches!(
                keyword,
                "direction" | "classDef" | "class" | "style" | "linkStyle" | "click"
            ) {
            Err(Error::Unsupported {
                kind: "Mermaid statement",
                detail: format!("line {}: {}", statement.span.line, statement.text),
            })
        } else {
            flowchart::chain(&statement, stack.last().copied())
        };
        match result {
            Ok(chain) => chains.push(chain),
            Err(error) => recover(error, statement.span, compat, &mut diagnostics)?,
        }
    }
    if let Some(owner) = stack.last() {
        return Err(Error::Parse {
            line: diag.subgraphs[*owner].line,
            message: "unclosed subgraph".into(),
        });
    }
    let diagram = lower::lower(diag, chains, &subgraphs, compat, &mut diagnostics)?;
    Ok(ParseOutput {
        document: Document {
            diagrams: vec![Diagram::Flowchart(diagram)],
        },
        diagnostics,
        includes: vec![],
    })
}

#[cfg(test)]
mod tests;
