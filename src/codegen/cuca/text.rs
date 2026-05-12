//! Text / markup helpers: Creole-lite → Typst markup conversion and
//! the various escaping routines codegen uses for class labels.

use std::fmt::Write as _;

use super::theme::puml_color_to_typst;

/// Convert PlantUML Creole-lite markup to Typst markup. Handles
/// `**bold**`, `//italic//`, literal `\n` (line break), and
/// `<color:NAME>…</color>`. All other characters are escaped via
/// `escape_one`. Nested formatting works (e.g. `**//foo//**`)
/// because the body of each construct is recursed into.
pub(super) fn creole_to_typst(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"**") {
            if let Some(end) = find_marker(bytes, i + 2, b"**") {
                let body = &s[i + 2..end];
                out.push_str("#strong[");
                out.push_str(&creole_to_typst(body));
                out.push(']');
                i = end + 2;
                continue;
            }
        }
        if bytes[i..].starts_with(b"//") {
            if let Some(end) = find_marker(bytes, i + 2, b"//") {
                let body = &s[i + 2..end];
                out.push_str("#emph[");
                out.push_str(&creole_to_typst(body));
                out.push(']');
                i = end + 2;
                continue;
            }
        }
        if bytes[i..].starts_with(b"\\n") {
            out.push_str(" \\ ");
            i += 2;
            continue;
        }
        if bytes[i..].starts_with(b"<color:") {
            let after_open = i + b"<color:".len();
            if let Some(rel) = bytes[after_open..].iter().position(|&b| b == b'>') {
                let color_end = after_open + rel;
                let color = &s[after_open..color_end];
                let body_start = color_end + 1;
                if let Some(rel_close) = s[body_start..].find("</color>") {
                    let body = &s[body_start..body_start + rel_close];
                    let typst_color = puml_color_to_typst(color)
                        .unwrap_or_else(|| "black".to_string());
                    let _ = write!(out, "#text(fill: {})[", typst_color);
                    out.push_str(&creole_to_typst(body));
                    out.push(']');
                    i = body_start + rel_close + b"</color>".len();
                    continue;
                }
            }
        }
        // Default: escape one char and advance by its UTF-8 length.
        let ch = s[i..].chars().next().unwrap();
        out.push_str(&escape_one(ch));
        i += ch.len_utf8();
    }
    out
}

fn find_marker(bytes: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from >= bytes.len() {
        return None;
    }
    let n = needle.len();
    let mut i = from;
    while i + n <= bytes.len() {
        if &bytes[i..i + n] == needle {
            return Some(i);
        }
        i += 1;
    }
    None
}

fn escape_one(c: char) -> String {
    match c {
        '\\' => "\\\\".into(),
        '*' | '_' | '#' | '$' | '`' | '~' | '@' | '<' | '>' | '[' | ']' | '{' | '}' => {
            format!("\\{c}")
        }
        _ => c.to_string(),
    }
}

pub(super) fn typst_str_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out
}

pub(super) fn typst_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('#', "\\#")
}
