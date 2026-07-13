//! Skinparam / `!theme` handling and PUML color resolution.
//!
//! `emit_skinparam_preamble` writes the top-of-document `#set page` /
//! `#set text` directives and returns a `PaintOverrides` struct the
//! emitter consults when filling in `#cuca-layout` arguments.

use crate::ir::Skinparam;

use super::text::typst_str_escape;

/// `skinparam linetype` — selects the edge-routing engine for this
/// diagram. See `docs/cuca-architecture-layout-redesign.md` §3.4.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LineMode {
    /// PlantUML/dot-style direct cubic beziers (the existing line-of-
    /// sight → Manhattan → pathplan → straight chain).
    Spline,
    /// Rounded orthogonal routing (§3.4's grid + A* router).
    Ortho,
    /// Like `Ortho` but with sharp (unrounded) corners.
    Polyline,
}

/// Per-cuca-layout overrides resolved from `skinparam` and `!theme`
/// directives. Values left as `None` fall through to the painter's
/// built-in defaults.
#[derive(Default, Clone)]
pub(super) struct PaintOverrides {
    pub(super) class_fill: Option<String>,
    pub(super) class_stroke_color: Option<String>,
    pub(super) edge_color: Option<String>,
    pub(super) package_fill: Option<String>,
    pub(super) package_stroke_color: Option<String>,
    /// `None` = no explicit `skinparam linetype` — the caller decides
    /// the default from the diagram's shape mix (desc-flavor → Ortho).
    pub(super) line_mode: Option<LineMode>,
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
            "linetype" => {
                overrides.line_mode = match p.value.trim().to_ascii_lowercase().as_str() {
                    "ortho" => Some(LineMode::Ortho),
                    "polyline" => Some(LineMode::Polyline),
                    "spline" | "curved" => Some(LineMode::Spline),
                    _ => overrides.line_mode,
                };
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

pub(super) use crate::codegen::common::puml_color_to_typst;
