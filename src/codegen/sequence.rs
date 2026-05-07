//! Sequence diagram codegen.
//!
//! Two paths exist:
//!   - [`SequenceDiagram::Raw`] — body emitted verbatim into a `seq-puml(...)`
//!     call. Kept as a fallback even though the native parser doesn't
//!     produce it; future loose-mode error recovery can fall back here.
//!   - [`SequenceDiagram::Structured`] — re-serialize the AST to a normalized
//!     PUML body (one node per line) and hand that off to `seq-puml`. This
//!     keeps blockcell as the single rendering authority while letting the
//!     Rust side own diagnostics, line numbers, and skinparam translation.

use std::fmt::Write as _;

use crate::ir::{
    Branch, FragmentKind, NotePosition, Participant, SequenceDiagram, SequenceHints, Skinparam,
    Step, StructuredSequence,
};

const PT_PER_PARTICIPANT: u32 = 110;
const MIN_WIDTH_PT: u32 = 360;
const MAX_WIDTH_PT: u32 = 1200;
const LABEL_BUDGET_CHARS: u32 = 20;
const PT_PER_OVERFLOW_CHAR: u32 = 4;

pub fn emit(out: &mut String, seq: &SequenceDiagram) {
    match seq {
        SequenceDiagram::Raw {
            title, body, hints, ..
        } => {
            if let Some(title) = title {
                emit_title(out, title);
            }
            let width_pt = compute_width_pt(hints);
            out.push_str(&format!("#seq-puml(width: {width_pt}pt, "));
            out.push_str(&typst_string_literal(body));
            out.push_str(")\n");
        }
        SequenceDiagram::Structured(seq) => emit_structured(out, seq),
    }
}

fn emit_structured(out: &mut String, seq: &StructuredSequence) {
    // Skinparams that map cleanly to Typst document-level defaults are emitted
    // before the seq-puml call. Everything else stays on the IR for now —
    // the mapping intentionally targets a small high-frequency subset.
    emit_skinparam_preamble(out, &seq.skinparams);

    if let Some(title) = &seq.title {
        emit_title(out, title);
    }

    let body = serialize_body(seq);
    let hints = SequenceHints {
        participants: seq.participants.len().max(2) as u32,
        max_label_chars: longest_label_chars(seq),
    };
    let width_pt = compute_width_pt(&hints);
    out.push_str(&format!("#seq-puml(width: {width_pt}pt, "));
    out.push_str(&typst_string_literal(&body));
    out.push_str(")\n");
}

fn emit_title(out: &mut String, title: &str) {
    out.push_str("#align(center)[*");
    out.push_str(&typst_escape(title));
    out.push_str("*]\n\n");
}

/// Map a small skinparam subset to a Typst preamble that takes effect before
/// the diagram renders. Unknown / unmapped keys are silently skipped — they
/// stay on the IR for future iterations.
fn emit_skinparam_preamble(out: &mut String, params: &[Skinparam]) {
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
                    text_args.push(format!("font: \"{}\"", typst_str_escape(trimmed)));
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

/// Best-effort PUML color → Typst color expression. Handles `#RRGGBB`,
/// `#RGB`, and a small set of named colors (mirrors blockcell's `_parse-color`
/// table).
fn puml_color_to_typst(raw: &str) -> Option<String> {
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

fn longest_label_chars(seq: &StructuredSequence) -> u32 {
    let mut max = 0u32;
    fn walk(steps: &[Step], max: &mut u32) {
        for step in steps {
            match step {
                Step::Message { label, .. } => {
                    if let Some(l) = label {
                        *max = (*max).max(l.chars().count() as u32);
                    }
                }
                Step::Note { text, .. } => {
                    for line in text.lines() {
                        *max = (*max).max(line.chars().count() as u32);
                    }
                }
                Step::Fragment { branches, .. } => {
                    for b in branches {
                        walk(&b.steps, max);
                    }
                }
                _ => {}
            }
        }
    }
    walk(&seq.steps, &mut max);
    max
}

// ---- Body serialization ----------------------------------------------------
//
// We re-emit the structured AST as canonical PUML text and let blockcell's
// `seq-puml` parse and render it. Indentation is for human readability only.

fn serialize_body(seq: &StructuredSequence) -> String {
    let mut out = String::new();
    for p in &seq.participants {
        write_participant_decl(&mut out, p);
    }
    if !seq.participants.is_empty() {
        out.push('\n');
    }
    write_steps(&mut out, &seq.steps, 0);
    out
}

fn write_participant_decl(out: &mut String, p: &Participant) {
    let _ = write!(out, "{} ", p.kind.keyword());
    if p.id == p.display {
        if needs_quoting(&p.id) {
            let _ = write!(out, "\"{}\"", p.id);
        } else {
            let _ = write!(out, "{}", p.id);
        }
    } else {
        let _ = write!(out, "\"{}\" as {}", p.display, p.id);
    }
    if let Some(color) = &p.color {
        let _ = write!(out, " {}", color);
    }
    out.push('\n');
}

fn needs_quoting(s: &str) -> bool {
    s.contains(char::is_whitespace) || s.is_empty()
}

fn write_steps(out: &mut String, steps: &[Step], depth: usize) {
    for step in steps {
        write_step(out, step, depth);
    }
}

fn write_step(out: &mut String, step: &Step, depth: usize) {
    let indent = "  ".repeat(depth);
    match step {
        Step::Message {
            from,
            to,
            arrow,
            label,
            ..
        } => {
            let _ = write!(out, "{indent}{from} {arrow} {to}");
            if let Some(l) = label {
                let _ = write!(out, " : {l}");
            }
            out.push('\n');
        }
        Step::Note {
            position,
            participants,
            text,
            ..
        } => write_note(out, &indent, *position, participants, text),
        Step::Divider { label, .. } => {
            let _ = writeln!(out, "{indent}== {label} ==");
        }
        Step::Autonumber { raw, .. } => {
            if raw.is_empty() {
                let _ = writeln!(out, "{indent}autonumber");
            } else {
                let _ = writeln!(out, "{indent}autonumber {raw}");
            }
        }
        Step::Activate {
            participant, color, ..
        } => match color {
            Some(c) => {
                let _ = writeln!(out, "{indent}activate {participant} {c}");
            }
            None => {
                let _ = writeln!(out, "{indent}activate {participant}");
            }
        },
        Step::Deactivate { participant, .. } => {
            let _ = writeln!(out, "{indent}deactivate {participant}");
        }
        Step::Create(p) => {
            let _ = write!(out, "{indent}create ");
            // `write_participant_decl` already includes a newline.
            write_participant_decl(out, p);
        }
        Step::Destroy { participant, .. } => {
            let _ = writeln!(out, "{indent}destroy {participant}");
        }
        Step::Return { label, .. } => match label {
            Some(l) => {
                let _ = writeln!(out, "{indent}return {l}");
            }
            None => {
                let _ = writeln!(out, "{indent}return");
            }
        },
        Step::Fragment {
            kind,
            label,
            branches,
            ..
        } => write_fragment(out, depth, *kind, label.as_deref(), branches),
    }
}

fn write_note(
    out: &mut String,
    indent: &str,
    position: NotePosition,
    participants: &[String],
    text: &str,
) {
    let pos_kw = match position {
        NotePosition::Over => "over",
        NotePosition::LeftOf => "left of",
        NotePosition::RightOf => "right of",
    };
    let target = participants.join(", ");
    let header = if target.is_empty() {
        format!("{indent}note {pos_kw}")
    } else {
        format!("{indent}note {pos_kw} {target}")
    };
    if text.contains('\n') {
        let _ = writeln!(out, "{header}");
        for line in text.lines() {
            let _ = writeln!(out, "{indent}  {line}");
        }
        let _ = writeln!(out, "{indent}end note");
    } else {
        let _ = writeln!(out, "{header} : {text}");
    }
}

fn write_fragment(
    out: &mut String,
    depth: usize,
    kind: FragmentKind,
    label: Option<&str>,
    branches: &[Branch],
) {
    let indent = "  ".repeat(depth);
    let head = kind.keyword();
    let mut iter = branches.iter();
    let first = match iter.next() {
        Some(b) => b,
        None => {
            // No branches at all — emit an empty `<head>\nend` for round-trip.
            let _ = writeln!(out, "{indent}{head}");
            let _ = writeln!(out, "{indent}end");
            return;
        }
    };
    match label {
        Some(l) if !l.is_empty() => {
            let _ = writeln!(out, "{indent}{head} {l}");
        }
        _ => {
            let _ = writeln!(out, "{indent}{head}");
        }
    }
    write_steps(out, &first.steps, depth + 1);
    for branch in iter {
        match &branch.label {
            Some(l) if !l.is_empty() => {
                let _ = writeln!(out, "{indent}else {l}");
            }
            _ => {
                let _ = writeln!(out, "{indent}else");
            }
        }
        write_steps(out, &branch.steps, depth + 1);
    }
    let _ = writeln!(out, "{indent}end");
}

// ---- Width heuristic + helpers ---------------------------------------------

fn compute_width_pt(hints: &SequenceHints) -> u32 {
    let participants = hints.participants.max(2);
    let base = PT_PER_PARTICIPANT * participants;
    let extra = hints
        .max_label_chars
        .saturating_sub(LABEL_BUDGET_CHARS)
        .saturating_mul(PT_PER_OVERFLOW_CHAR);
    (base + extra).clamp(MIN_WIDTH_PT, MAX_WIDTH_PT)
}

fn typst_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    out.push_str(&typst_str_escape(s));
    out.push('"');
    out
}

fn typst_str_escape(s: &str) -> String {
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

fn typst_escape(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('*', "\\*")
        .replace('_', "\\_")
        .replace('#', "\\#")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::ParticipantKind;

    fn p(id: &str, display: &str) -> Participant {
        Participant {
            kind: ParticipantKind::Participant,
            id: id.into(),
            display: display.into(),
            color: None,
            line: 1,
        }
    }

    #[test]
    fn width_clamps_to_minimum() {
        let h = SequenceHints {
            participants: 1,
            max_label_chars: 0,
        };
        assert_eq!(compute_width_pt(&h), MIN_WIDTH_PT);
    }

    #[test]
    fn width_clamps_to_maximum() {
        let h = SequenceHints {
            participants: 50,
            max_label_chars: 1000,
        };
        assert_eq!(compute_width_pt(&h), MAX_WIDTH_PT);
    }

    #[test]
    fn long_labels_pad_width() {
        let baseline = compute_width_pt(&SequenceHints {
            participants: 4,
            max_label_chars: 0,
        });
        let padded = compute_width_pt(&SequenceHints {
            participants: 4,
            max_label_chars: 60,
        });
        assert!(padded > baseline);
    }

    #[test]
    fn serialize_basic_message() {
        let seq = StructuredSequence {
            participants: vec![p("A", "A"), p("B", "B")],
            steps: vec![Step::Message {
                from: "A".into(),
                to: "B".into(),
                arrow: "->".into(),
                label: Some("hi".into()),
                line: 1,
            }],
            ..Default::default()
        };
        let body = serialize_body(&seq);
        assert!(body.contains("participant A"));
        assert!(body.contains("A -> B : hi"));
    }

    #[test]
    fn serialize_alt_with_else_round_trips() {
        let seq = StructuredSequence {
            participants: vec![p("A", "A"), p("B", "B")],
            steps: vec![Step::Fragment {
                kind: FragmentKind::Alt,
                label: Some("cond".into()),
                branches: vec![
                    Branch {
                        label: None,
                        steps: vec![Step::Message {
                            from: "A".into(),
                            to: "B".into(),
                            arrow: "->".into(),
                            label: Some("yes".into()),
                            line: 2,
                        }],
                    },
                    Branch {
                        label: Some("other".into()),
                        steps: vec![Step::Message {
                            from: "A".into(),
                            to: "B".into(),
                            arrow: "->".into(),
                            label: Some("no".into()),
                            line: 4,
                        }],
                    },
                ],
                line: 1,
            }],
            ..Default::default()
        };
        let body = serialize_body(&seq);
        assert!(body.contains("alt cond"));
        assert!(body.contains("else other"));
        assert!(body.contains("end"));
    }

    #[test]
    fn skinparam_preamble_emits_page_fill_and_text() {
        let mut out = String::new();
        emit_skinparam_preamble(
            &mut out,
            &[
                Skinparam {
                    key: "backgroundColor".into(),
                    value: "#EEE".into(),
                    line: 1,
                },
                Skinparam {
                    key: "defaultFontSize".into(),
                    value: "14".into(),
                    line: 2,
                },
            ],
        );
        assert!(out.contains("#set page(fill: rgb(\"#EEE\"))"));
        assert!(out.contains("size: 14pt"));
    }
}
