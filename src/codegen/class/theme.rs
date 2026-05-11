//! Skinparam / `!theme` handling and PUML color resolution.
//!
//! `emit_skinparam_preamble` writes the top-of-document `#set page` /
//! `#set text` directives and returns a `PaintOverrides` struct the
//! emitter consults when filling in `#class-layout` arguments.

use crate::ir::Skinparam;

use super::text::typst_str_escape;

/// Per-class-layout overrides resolved from `skinparam` and `!theme`
/// directives. Values left as `None` fall through to the painter's
/// built-in defaults.
#[derive(Default, Clone)]
pub(super) struct PaintOverrides {
    pub(super) class_fill: Option<String>,
    pub(super) class_stroke_color: Option<String>,
    pub(super) edge_color: Option<String>,
    pub(super) package_fill: Option<String>,
    pub(super) package_stroke_color: Option<String>,
}

pub(super) fn emit_skinparam_preamble(
    out: &mut String,
    params: &[Skinparam],
) -> PaintOverrides {
    let mut text_args: Vec<String> = Vec::new();
    let mut page_fill: Option<String> = None;
    let mut overrides = PaintOverrides::default();
    // Optionally expand a `!theme NAME` value into a synthetic skinparam
    // sequence (handled here so all theme names funnel through the same
    // override resolution).
    let expanded = expand_theme(params);
    for p in expanded.iter() {
        // Both PascalCase and camelCase variants appear in real-world
        // PlantUML; normalize to lowercase for lookup.
        let key = p.key.to_ascii_lowercase();
        match key.as_str() {
            "backgroundcolor" => {
                if let Some(color) = puml_color_to_typst(&p.value) {
                    page_fill = Some(color);
                }
            }
            "defaultfontname" | "defaultfontfamily" => {
                let trimmed = p.value.trim_matches('"');
                if !trimmed.is_empty() {
                    text_args.push(format!("font: \"{}\"", typst_str_escape(trimmed)));
                }
            }
            "defaultfontsize" => {
                if let Ok(pt) = p.value.trim().parse::<u32>() {
                    text_args.push(format!("size: {pt}pt"));
                }
            }
            "classbackgroundcolor" => {
                overrides.class_fill = puml_color_to_typst(&p.value);
            }
            "classbordercolor" | "classborder" => {
                overrides.class_stroke_color = puml_color_to_typst(&p.value);
            }
            "arrowcolor" => {
                overrides.edge_color = puml_color_to_typst(&p.value);
            }
            "packagebackgroundcolor" | "packagebackground" => {
                overrides.package_fill = puml_color_to_typst(&p.value);
            }
            "packagebordercolor" => {
                overrides.package_stroke_color = puml_color_to_typst(&p.value);
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
    overrides
}

/// Expand `!theme <name>` into a flat list of synthetic skinparams plus
/// the original list. PlantUML has dozens of themes; we ship a tiny
/// subset (vibrant, plain, amiga, cerulean) — unknown theme names are
/// passed through with no expansion, so `!theme some-other` silently
/// keeps the default styling rather than failing.
fn expand_theme(params: &[Skinparam]) -> Vec<Skinparam> {
    let mut out: Vec<Skinparam> = Vec::with_capacity(params.len());
    for p in params {
        let key = p.key.to_ascii_lowercase();
        if key == "theme" || key == "!theme" {
            let theme = p.value.trim().to_ascii_lowercase();
            for (k, v) in builtin_theme(&theme) {
                out.push(Skinparam {
                    key: k.to_string(),
                    value: v.to_string(),
                    line: p.line,
                });
            }
            continue;
        }
        out.push(p.clone());
    }
    out
}

fn builtin_theme(name: &str) -> &'static [(&'static str, &'static str)] {
    match name {
        "plain" | "default" => &[],
        "vibrant" => &[
            ("backgroundColor", "#FFFEF7"),
            ("classBackgroundColor", "#FFFB96"),
            ("classBorderColor", "#5C5400"),
            ("packageBackgroundColor", "#FFFCEA"),
            ("packageBorderColor", "#9C8800"),
            ("arrowColor", "#5C5400"),
        ],
        "amiga" => &[
            ("backgroundColor", "#0044AA"),
            ("classBackgroundColor", "#FFFFFF"),
            ("classBorderColor", "#000000"),
            ("arrowColor", "#FFFFFF"),
        ],
        "cerulean" => &[
            ("backgroundColor", "#FFFFFF"),
            ("classBackgroundColor", "#E5F0FA"),
            ("classBorderColor", "#2780E3"),
            ("arrowColor", "#2780E3"),
            ("packageBackgroundColor", "#F4F8FC"),
        ],
        _ => &[],
    }
}

/// Best-effort PUML color → Typst color expression. Mirrors
/// `sequence.rs::puml_color_to_typst`; once class is in, we should
/// extract this helper to a shared module.
pub(super) fn puml_color_to_typst(raw: &str) -> Option<String> {
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
