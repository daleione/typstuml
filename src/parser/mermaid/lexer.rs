use crate::diagnostics::{Error, Result};

#[derive(Clone, Copy, Debug)]
pub struct SourceSpan {
    pub line: usize,
    pub column: usize,
}
#[derive(Debug)]
pub struct Statement<'a> {
    pub text: &'a str,
    pub span: SourceSpan,
}

/// Split statements using the same shape delimiters as the grammar. Labels
/// protect separators; asymmetric `>…]` is deliberately not a bracket pair.
pub fn statements<'a>(source: &'a str) -> Result<Vec<Statement<'a>>> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut close: Option<&str> = None;
    let mut quoted = false;
    let mut line_starts = vec![0];
    line_starts.extend(source.match_indices('\n').map(|(i, _)| i + 1));
    let span = |offset: usize| {
        let line_index = line_starts.partition_point(|&start| start <= offset) - 1;
        SourceSpan {
            line: line_index + 1,
            column: source[line_starts[line_index]..offset].chars().count() + 1,
        }
    };
    let push = |out: &mut Vec<Statement<'a>>, a: usize, b: usize| {
        let raw = &source[a..b];
        let text = raw.trim();
        if !text.is_empty() {
            let offset = a + raw.len() - raw.trim_start().len();
            out.push(Statement {
                text,
                span: span(offset),
            });
        }
    };
    while i < source.len() {
        let tail = &source[i..];
        let ch = tail.chars().next().unwrap();
        if let Some(end) = close {
            if ch == '"' {
                quoted = !quoted;
                i += 1;
                continue;
            }
            if !quoted && tail.starts_with(end) {
                i += end.len();
                close = None;
                continue;
            }
            i += ch.len_utf8();
            continue;
        }
        if ch == '"' {
            quoted = !quoted;
            i += 1;
            continue;
        }
        if quoted {
            i += ch.len_utf8();
            continue;
        }
        if tail.starts_with("%%{") {
            push(&mut out, start, i);
            let Some(end) = tail.find("}%%") else {
                return Err(Error::Parse {
                    line: span(i).line,
                    message: "unclosed Mermaid directive".into(),
                });
            };
            let end = i + end + 3;
            push(&mut out, i, end);
            i = end;
            start = end;
            continue;
        }
        if tail.starts_with("%%") {
            push(&mut out, start, i);
            i += tail.find('\n').unwrap_or(tail.len());
            start = i;
            continue;
        }
        if ch == ';' || ch == '\n' {
            push(&mut out, start, i);
            i += 1;
            start = i;
            continue;
        }
        // Skip arrow tokens before interpreting '>' as an asymmetric shape.
        if let Some(token) = ["-.->", "-->", "==>", "---", "--"]
            .iter()
            .find(|t| tail.starts_with(**t))
        {
            i += token.len();
            continue;
        }
        // Unknown arrow endings must remain unsupported syntax, rather than
        // opening a spurious asymmetric node label and swallowing the next line.
        if ch == '>'
            && !source[..i]
                .trim_end()
                .chars()
                .next_back()
                .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_')
        {
            i += 1;
            continue;
        }
        if let Some((open, end)) = [
            ("((", "))"),
            ("([", "])"),
            ("[(", ")]"),
            ("[", "]"),
            ("(", ")"),
            ("{", "}"),
            (">", "]"),
            ("|", "|"),
        ]
        .into_iter()
        .find(|(open, _)| tail.starts_with(open))
        {
            close = Some(end);
            i += open.len();
            continue;
        }
        i += ch.len_utf8();
    }
    if quoted || close.is_some() {
        let at = span(start);
        return Err(Error::Parse {
            line: at.line,
            message: format!("column {}: unclosed Mermaid label or quote", at.column),
        });
    }
    push(&mut out, start, source.len());
    Ok(out)
}
