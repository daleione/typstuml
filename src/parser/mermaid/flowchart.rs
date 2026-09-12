use super::ast::{Chain, EdgeSpec, NodeRef};
use super::lexer::Statement;
use crate::diagnostics::{Error, Result};
use crate::ir::{FlowShape, LineStyle, LineWeight};

pub struct Cursor<'a> {
    pub rest: &'a str,
    line: usize,
}
impl<'a> Cursor<'a> {
    pub fn new(rest: &'a str, line: usize) -> Self {
        Self { rest, line }
    }
    pub fn ws(&mut self) {
        self.rest = self.rest.trim_start();
    }
    pub fn unsupported(&self, message: &str) -> Error {
        Error::Unsupported {
            kind: "Mermaid syntax",
            detail: format!("line {}: {message}: {:?}", self.line, self.rest),
        }
    }
    pub fn id(&mut self) -> Result<String> {
        self.ws();
        if !self
            .rest
            .starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        {
            return Err(self.unsupported("expected ASCII node ID"));
        }
        let mut end = 0;
        for (i, c) in self.rest.char_indices() {
            if !(c.is_ascii_alphanumeric() || c == '_' || c == '-') {
                break;
            }
            if c == '-'
                && ["-->", "---", "-.->", "--"]
                    .iter()
                    .any(|t| self.rest[i..].starts_with(t))
            {
                break;
            }
            end = i + c.len_utf8();
        }
        let id = self.rest[..end].to_string();
        self.rest = &self.rest[end..];
        Ok(id)
    }
    pub fn label(&mut self, end: &str) -> Result<String> {
        let mut quoted = false;
        for (i, c) in self.rest.char_indices() {
            if c == '"' {
                quoted = !quoted;
            }
            if !quoted && self.rest[i..].starts_with(end) {
                let value = decode_label(self.rest[..i].trim(), self.line)?;
                self.rest = &self.rest[i + end.len()..];
                return Ok(value);
            }
        }
        Err(Error::Parse {
            line: self.line,
            message: format!("unclosed label; expected {end:?}"),
        })
    }
    fn node(&mut self) -> Result<NodeRef> {
        let id = self.id()?;
        self.ws();
        if self.rest.starts_with("@{") {
            return Err(self.unsupported("shape declarations are not supported"));
        }
        let shapes = [
            ("((", "))", FlowShape::Circle),
            ("([", "])", FlowShape::Stadium),
            ("[(", ")]", FlowShape::Cylinder),
            ("[", "]", FlowShape::Rect),
            ("(", ")", FlowShape::Rounded),
            ("{", "}", FlowShape::Diamond),
            (">", "]", FlowShape::Asymmetric),
        ];
        for (open, close, shape) in shapes {
            if self.rest.starts_with(open) {
                self.rest = &self.rest[open.len()..];
                let label = self.label(close)?;
                return Ok(NodeRef {
                    id,
                    label: Some(label),
                    shape: Some(shape),
                });
            }
        }
        Ok(NodeRef {
            id,
            label: None,
            shape: None,
        })
    }
    fn edge(&mut self) -> Result<EdgeSpec> {
        self.ws();
        let mut spec = EdgeSpec {
            arrow: true,
            style: LineStyle::Solid,
            weight: LineWeight::Normal,
            label: None,
        };
        if self.rest.starts_with("-.->") {
            spec.style = LineStyle::Dashed;
            self.rest = &self.rest[4..];
        } else if self.rest.starts_with("-->") {
            self.rest = &self.rest[3..];
        } else if self.rest.starts_with("==>") {
            spec.weight = LineWeight::Thick;
            self.rest = &self.rest[3..];
        } else if self.rest.starts_with("---") {
            self.rest = &self.rest[3..];
            spec.arrow = false;
            if self.rest.starts_with(['o', 'x', '-']) {
                return Err(self.unsupported("unsupported edge ending"));
            }
        } else if self.rest.starts_with("--") {
            self.rest = &self.rest[2..];
            spec.label = Some(self.label("-->")?);
        } else {
            return Err(self.unsupported("expected supported edge token"));
        }
        self.ws();
        if self.rest.starts_with('|') {
            if spec.label.is_some() {
                return Err(self.unsupported("duplicate edge label"));
            }
            self.rest = &self.rest[1..];
            spec.label = Some(self.label("|")?);
            self.ws();
        }
        Ok(spec)
    }
}

pub fn chain(statement: &Statement<'_>, owner: Option<usize>) -> Result<Chain> {
    let mut cursor = Cursor::new(statement.text, statement.span.line);
    let mut nodes = vec![cursor.node()?];
    let mut edges = Vec::new();
    cursor.ws();
    while !cursor.rest.is_empty() {
        edges.push(cursor.edge()?);
        nodes.push(cursor.node()?);
        cursor.ws();
    }
    Ok(Chain {
        nodes,
        edges,
        owner,
        span: statement.span,
    })
}

pub fn decode_label(raw: &str, line: usize) -> Result<String> {
    let quoted = raw.starts_with('"') && raw.ends_with('"') && raw.len() >= 2;
    let text = if quoted { &raw[1..raw.len() - 1] } else { raw };
    if (text.starts_with('`') && text.ends_with('`'))
        || text
            .as_bytes()
            .windows(2)
            .any(|w| w[0] == b'<' && (w[1].is_ascii_alphabetic() || w[1] == b'/'))
    {
        return Err(Error::Unsupported {
            kind: "Mermaid label",
            detail: format!("line {line}: HTML/Markdown labels are not supported"),
        });
    }
    if text.contains('"') || (!quoted && text.contains(['[', ']', '(', ')', '{', '}'])) {
        return Err(Error::Unsupported {
            kind: "Mermaid label",
            detail: format!("line {line}: quote labels containing reserved characters"),
        });
    }
    let text = text.replace("\r\n", "\n");
    if !quoted {
        return Ok(text);
    }
    let mut out = String::new();
    let mut tail = text.as_str();
    while let Some(i) = tail.find('#') {
        out.push_str(&tail[..i]);
        tail = &tail[i..];
        let Some(end) = tail.find(';') else {
            out.push_str(tail);
            return Ok(out);
        };
        let name = &tail[1..end];
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
            out.push('#');
            tail = &tail[1..];
            continue;
        }
        let value = match name {
            "quot" => '"',
            "amp" => '&',
            "lt" => '<',
            "gt" => '>',
            "nbsp" => '\u{a0}',
            _ if name.bytes().all(|b| b.is_ascii_digit()) => name
                .parse::<u32>()
                .ok()
                .and_then(char::from_u32)
                .ok_or_else(|| Error::Parse {
                    line,
                    message: "invalid Unicode entity".into(),
                })?,
            _ => {
                return Err(Error::Unsupported {
                    kind: "Mermaid entity",
                    detail: format!("line {line}: #{name};"),
                })
            }
        };
        out.push(value);
        tail = &tail[end + 1..];
    }
    out.push_str(tail);
    Ok(out)
}
