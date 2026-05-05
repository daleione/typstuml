//! Sequence diagram parser.
//!
//! M0 strategy: capture the raw body of the block as an opaque string and
//! let `blockcell`'s `seq-puml` Typst function do the heavy lifting. We
//! still scan the body once to compute layout hints (participant count,
//! longest label) so codegen doesn't have to.
//!
//! M1 replaces this with a native Rust parser that mirrors `seq-puml.typ`'s
//! behavior, producing a structured AST + golden-test-friendly diagnostics.

use std::collections::HashSet;

use crate::diagnostics::{CompatMode, Diagnostic, Result};
use crate::ir::{Diagram, SequenceDiagram, SequenceHints};
use crate::parser::lexer::UmlBlock;

const PARTICIPANT_KEYWORDS: &[&str] = &[
    "participant",
    "actor",
    "boundary",
    "control",
    "entity",
    "database",
    "collections",
    "queue",
];

const SEQUENCE_ARROWS: &[&str] = &[
    " -> ", " --> ", " <- ", " <-- ", " ->> ", " <<- ", " ->o ", " o<- ", " ->x ",
];

pub fn parse(block: &UmlBlock, _compat: CompatMode) -> Result<(Diagram, Vec<Diagnostic>)> {
    let body = block
        .body
        .iter()
        .map(|l| l.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    let title = extract_title(&body);
    let hints = scan_hints(&body);

    let diagram = Diagram::Sequence(SequenceDiagram::Raw {
        title,
        body,
        name: block.name.clone(),
        hints,
    });
    Ok((diagram, Vec::new()))
}

fn extract_title(body: &str) -> Option<String> {
    for line in body.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("title") {
            let rest = rest.trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    None
}

fn scan_hints(body: &str) -> SequenceHints {
    let mut declared = 0u32;
    let mut endpoints: HashSet<&str> = HashSet::new();
    let mut max_label = 0usize;

    for line in body.lines() {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }

        if PARTICIPANT_KEYWORDS
            .iter()
            .any(|kw| t.starts_with(&format!("{kw} ")))
        {
            declared += 1;
            continue;
        }

        if let Some((before, after)) = split_on_arrow(t) {
            endpoints.insert(before.trim());
            // The right side may carry a `: label` payload.
            let (rhs, label) = match after.split_once(':') {
                Some((r, l)) => (r.trim(), Some(l.trim())),
                None => (after.trim(), None),
            };
            endpoints.insert(rhs);
            if let Some(l) = label {
                max_label = max_label.max(l.chars().count());
            }
        } else if let Some((_, label)) = t.split_once(':') {
            // `note over A, B : label` and similar.
            max_label = max_label.max(label.trim().chars().count());
        }
    }

    let participants = declared.max(endpoints.len() as u32).max(2);

    SequenceHints {
        participants,
        max_label_chars: max_label as u32,
    }
}

fn split_on_arrow(line: &str) -> Option<(&str, &str)> {
    SEQUENCE_ARROWS
        .iter()
        .find_map(|a| line.split_once(a))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_declared_participants() {
        let h = scan_hints("participant A\nparticipant B\nactor C\nA -> B : hi\n");
        assert_eq!(h.participants, 3);
    }

    #[test]
    fn falls_back_to_arrow_endpoints() {
        let h = scan_hints("A -> B : hi\nB -> C : hello\n");
        assert_eq!(h.participants, 3);
    }

    #[test]
    fn captures_longest_label() {
        let h = scan_hints("A -> B : short\nB -> A : a much longer message label\n");
        assert_eq!(h.max_label_chars as usize, "a much longer message label".len());
    }
}
