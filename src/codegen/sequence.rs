//! Sequence diagram codegen.
//!
//! M0 emits a single call to `seq-puml(<raw body>)`, which is the existing
//! Typst-side parser in `blockcell`. Width is computed from layout hints in
//! the IR — see [`compute_width_pt`] for the heuristic. M1 will replace
//! both this codegen and its hints once a structured AST exists.

use crate::ir::{SequenceDiagram, SequenceHints};

const PT_PER_PARTICIPANT: u32 = 110;
const MIN_WIDTH_PT: u32 = 360;
const MAX_WIDTH_PT: u32 = 1200;
const LABEL_BUDGET_CHARS: u32 = 20;
const PT_PER_OVERFLOW_CHAR: u32 = 4;

pub fn emit(out: &mut String, seq: &SequenceDiagram) {
    match seq {
        SequenceDiagram::Raw {
            title,
            body,
            hints,
            ..
        } => {
            if let Some(title) = title {
                out.push_str("#align(center)[*");
                out.push_str(&typst_escape(title));
                out.push_str("*]\n\n");
            }
            // Page is `width: auto`, and seq-puml's default `width: auto`
            // resolves to 100% of its container — without a fixed width,
            // columns collapse. Pick one explicitly from the hints.
            let width_pt = compute_width_pt(hints);
            out.push_str(&format!("#seq-puml(width: {width_pt}pt, "));
            out.push_str(&typst_string_literal(body));
            out.push_str(")\n");
        }
    }
}

fn compute_width_pt(hints: &SequenceHints) -> u32 {
    let participants = hints.participants.max(2);
    let base = PT_PER_PARTICIPANT * participants;
    let extra = hints
        .max_label_chars
        .saturating_sub(LABEL_BUDGET_CHARS)
        .saturating_mul(PT_PER_OVERFLOW_CHAR);
    (base + extra).clamp(MIN_WIDTH_PT, MAX_WIDTH_PT)
}

/// Wrap `s` as a Typst string literal — escape `\` and `"`, leave newlines
/// in place (Typst strings can span multiple lines).
fn typst_string_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            _ => out.push(c),
        }
    }
    out.push('"');
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

    #[test]
    fn width_clamps_to_minimum() {
        let h = SequenceHints {
            participants: 1,
            max_label_chars: 0,
        };
        // participants is bumped to 2, then 220pt < 360 → clamped to 360.
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
}
