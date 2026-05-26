//! Shared low-level Typst-emission helpers used across the per-diagram code
//! generators: string / markup escaping, PlantUML color translation, the
//! skinparam preamble, and indentation.
//!
//! Only verbatim-shared helpers live here. Note that `tree_emit::typst_color`
//! is intentionally NOT here — it emits bare Typst color names (`red`) rather
//! than `rgb("#…")`, a different output contract. The activity parser also
//! keeps a local `typst_escape` that escapes a wider set (`[ ] \``) than
//! [`escape_markup_min`].

use crate::ir::Skinparam;

/// Escape a string for embedding inside a Typst double-quoted string literal
/// (only `\` and `"` are special there).
pub(crate) fn escape_string(s: &str) -> String {
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

/// Escape the full set of Typst markup specials for text emitted into a
/// `[content]` block (record graphs and tree diagrams).
pub(crate) fn escape_markup(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '*' | '_' | '#' | '$' | '`' | '~' | '@' | '<' | '>' | '[' | ']' | '{' | '}' => {
                out.push('\\');
                out.push(c);
            }
            _ => out.push(c),
        }
    }
    out
}

/// Minimal markup escaping (`\ * _ #`) used by the sequence and cuca text
/// paths, where `[`/`]`/`` ` `` are not treated as special.
pub(crate) fn escape_markup_min(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('#', "\\#")
}

/// Translate a PlantUML color (a named color or `#hex`) into a Typst
/// `rgb("#…")` literal. Returns `None` when the value is neither a known name
/// nor a valid 3/6-digit hex code.
pub(crate) fn puml_color_to_typst(raw: &str) -> Option<String> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }
    let hex = s.strip_prefix('#').unwrap_or(s);
    let lower = hex.to_ascii_lowercase();
    let named = match lower.as_str() {
        "red" => Some("#FF0000"),
        "blue" => Some("#0000FF"),
        "green" => Some("#008000"),
        "yellow" => Some("#FFFF00"),
        "orange" => Some("#FFA500"),
        "purple" => Some("#800080"),
        "pink" => Some("#FFC0CB"),
        "black" => Some("#000000"),
        "white" => Some("#FFFFFF"),
        "gray" | "grey" => Some("#808080"),
        "lightblue" => Some("#ADD8E6"),
        "lightgreen" => Some("#90EE90"),
        "lightyellow" => Some("#FFFFE0"),
        "lightgray" | "lightgrey" => Some("#D3D3D3"),
        "darkblue" => Some("#00008B"),
        "darkgreen" => Some("#006400"),
        "darkred" => Some("#8B0000"),
        "gold" => Some("#FFD700"),
        "cyan" | "aqua" => Some("#00FFFF"),
        "magenta" => Some("#FF00FF"),
        _ => None,
    };
    let final_hex = match named {
        Some(h) => h.trim_start_matches('#').to_string(),
        None => {
            if hex.chars().all(|c| c.is_ascii_hexdigit()) && (hex.len() == 3 || hex.len() == 6) {
                hex.to_string()
            } else {
                return None;
            }
        }
    };
    Some(format!("rgb(\"#{}\")", final_hex))
}

/// Emit a `#set page(fill: …)` / `#set text(...)` preamble from the document's
/// skinparams, if any of the supported keys are present. Shared by the
/// sequence and activity emitters (cuca has its own richer handler that
/// returns paint overrides).
pub(crate) fn emit_skinparam_preamble(out: &mut String, params: &[Skinparam]) {
    let mut text_args: Vec<String> = Vec::new();
    let mut page_fill: Option<String> = None;

    for p in params {
        match p.key.as_str() {
            "backgroundColor" | "BackgroundColor" => {
                if let Some(color) = puml_color_to_typst(&p.value) {
                    page_fill = Some(color);
                }
            }
            "defaultFontName" | "DefaultFontName" | "defaultFontFamily" => {
                let trimmed = p.value.trim_matches('"');
                if !trimmed.is_empty() {
                    text_args.push(format!("font: \"{}\"", escape_string(trimmed)));
                }
            }
            "defaultFontSize" | "DefaultFontSize" => {
                if let Ok(pt) = p.value.trim().parse::<u32>() {
                    text_args.push(format!("size: {pt}pt"));
                }
            }
            _ => {}
        }
    }

    let had_page_fill = page_fill.is_some();
    if let Some(color) = page_fill {
        out.push_str(&format!("#set page(fill: {color})\n"));
    }
    if !text_args.is_empty() {
        out.push_str(&format!("#set text({})\n", text_args.join(", ")));
    }
    if had_page_fill || !text_args.is_empty() {
        out.push('\n');
    }
}

/// Push `level` levels of two-space indentation onto `out`.
pub(crate) fn indent(out: &mut String, level: usize) {
    for _ in 0..level {
        out.push_str("  ");
    }
}
