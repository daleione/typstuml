//! Detects which PlantUML diagram type a block contains.
//!
//! Strategy: trust the opening `@start<kind>` tag whenever it's present.
//! Only fall back to body-content sniffing when the file is in fragment
//! mode (no tags at all) or the kind is the catch-all `uml`.

use crate::parser::lexer::{BodyLine, UmlBlock};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DiagramKind {
    Sequence,
    State,
    Activity,
    MindMap,
    Wbs,
    Json,
    Yaml,
    /// Cuca diagram family — class / component / deployment / use case
    /// / object. One IR, one parser, shape-by-shape painter dispatch.
    /// See `docs/cuca-diagram-design.md`.
    Cuca,
    Er,
    Gantt,
    Salt,
    Timing,
    Network,
    Ditaa,
    Unknown,
}

pub fn detect(block: &UmlBlock) -> DiagramKind {
    match block.kind_tag.as_str() {
        "mindmap" => DiagramKind::MindMap,
        "wbs" => DiagramKind::Wbs,
        "json" => DiagramKind::Json,
        "yaml" => DiagramKind::Yaml,
        "salt" => DiagramKind::Salt,
        "gantt" => DiagramKind::Gantt,
        "ditaa" => DiagramKind::Ditaa,
        // "uml" or empty (fragment mode) — sniff the body.
        _ => sniff_body(&block.body),
    }
}

/// Keywords that ONLY appear in sequence diagrams. Hard signal.
const UNAMBIGUOUS_SEQ: &[&str] = &["participant", "autonumber"];

/// Keywords ambiguous between sequence (participant kind) and cuca
/// (desc-family leaf shape). Need other evidence to disambiguate.
const SHARED_SEQ_CUCA: &[&str] = &[
    "actor", "boundary", "control", "entity", "database", "collections", "queue",
];

/// Strong cuca signals: anything from this list locks in DiagramKind::Cuca.
const UNAMBIGUOUS_CUCA: &[&str] = &[
    "class ",
    "interface ",
    "abstract ",
    "enum ",
    "annotation ",
    "struct ",
    "exception ",
    "protocol ",
    "object ",
    "package ",
    "namespace ",
    "together ",
    "folder ",
    "frame ",
    "component ",
    "usecase ",
    "node ",
    "cloud ",
    "stack ",
    "storage ",
    "artifact ",
    "agent ",
    "person ",
    "rectangle ",
    "card ",
    "file ",
    "hexagon ",
    "action ",
    "process ",
    "label ",
    "port ",
    "() ",
];

/// Sequence arrow forms. Different from cuca relation arrows (which are
/// `--|>` / `--*` / `..>` etc) — sequence uses short `->` / `-->` with
/// spaces on both sides AND a `:` label or a single message receiver.
const SEQUENCE_ARROWS: &[&str] = &[
    " -> ", " <- ", " ->> ", " <<- ", " ->o ", " o<- ", " ->x ",
];

/// Cuca-only inline shorthand starters: `[Foo]`, `(Foo)` (not `()`,
/// which is also a lollipop), `:Foo:` (only at start of name segment).
fn is_cuca_shorthand_start(t: &str) -> bool {
    if t.starts_with('[') {
        return true;
    }
    if t.starts_with('(') && !t.starts_with("()") {
        return true;
    }
    // `:Foo:` actor shorthand — colon at start, another colon later in
    // the line (no `->` in between since that's a sequence form).
    if t.starts_with(':') && t[1..].find(':').is_some() && !t.contains(" -> ") {
        return true;
    }
    false
}

fn sniff_body(body: &[BodyLine]) -> DiagramKind {
    // Two-pass: collect evidence first, then decide. Sharing this
    // logic across `actor` / `database` / etc. is what keeps a use-case
    // diagram (which uses `actor` + no `participant`) from being mis-
    // detected as a sequence diagram.
    let mut seen_cuca_strong = false;
    let mut seen_seq_strong = false;
    let mut seen_shared_kw = false;
    let mut seen_sequence_arrow = false;

    for line in body {
        let t = line.text.trim();
        if t.is_empty() || t.starts_with('\'') || t.starts_with("/'") {
            continue;
        }
        if t.starts_with("skinparam")
            || t.starts_with("title")
            || t.starts_with("hide")
            || t.starts_with("show")
            || t.starts_with("!theme")
            || t.starts_with('!')
        {
            continue;
        }
        // Java-style annotations sit above class declarations. Treat
        // them as transparent so a body that begins with
        // `@Entity\nclass Foo` is still recognized as a cuca diagram.
        if t.starts_with('@')
            && !t.starts_with("@start")
            && !t.starts_with("@end")
        {
            continue;
        }

        // Hard signals: any unambiguous keyword commits immediately.
        if UNAMBIGUOUS_SEQ
            .iter()
            .any(|kw| t.starts_with(&format!("{kw} ")) || t == *kw)
        {
            return DiagramKind::Sequence;
        }
        if UNAMBIGUOUS_CUCA.iter().any(|h| t.starts_with(h)) {
            seen_cuca_strong = true;
        }
        if is_cuca_shorthand_start(t) {
            seen_cuca_strong = true;
        }
        if SHARED_SEQ_CUCA
            .iter()
            .any(|kw| t.starts_with(&format!("{kw} ")) || t == *kw)
        {
            seen_shared_kw = true;
        }
        if SEQUENCE_ARROWS.iter().any(|a| t.contains(a)) {
            seen_sequence_arrow = true;
            seen_seq_strong = true;
        }

        // Sub-kinds: hard signals that aren't yet decided by the above.
        if t.starts_with("[*]") || t.starts_with("state ") {
            return DiagramKind::State;
        }
        if t == "start"
            || t == "stop"
            || t == "end"
            || t == "fork"
            || t == "fork;"
            || t == "split"
            || t == "split;"
            || t == "repeat"
            || t == "detach"
            || t == "kill"
            || t.starts_with("if (")
            || t.starts_with("if(")
            || t.starts_with("while (")
            || t.starts_with("while(")
            || t.starts_with("repeat :")
            || t.starts_with("switch (")
            || t.starts_with("switch(")
            || t.starts_with("partition ")
        {
            return DiagramKind::Activity;
        }
        // An activity `:label;` action always carries a `;` terminator
        // on the same line (or on a subsequent multi-line continuation).
        // We require the `;` here so cuca's `:Foo:` actor shorthand
        // (which never ends with `;`) doesn't get misclassified.
        if t.starts_with(':') && t.contains(';') && !seen_cuca_strong {
            return DiagramKind::Activity;
        }
        if !seen_cuca_strong && (t.starts_with('*') || t.starts_with('+') || t == "-" || t.starts_with("- ")) {
            return DiagramKind::MindMap;
        }
        if t.starts_with('{') {
            return DiagramKind::Json;
        }
    }

    // Decision: cuca wins if there's any strong cuca signal. Sequence
    // wins if there's a sequence arrow without a cuca signal, OR a
    // shared keyword without a cuca signal. Otherwise unknown.
    if seen_cuca_strong {
        DiagramKind::Cuca
    } else if seen_seq_strong || seen_shared_kw || seen_sequence_arrow {
        DiagramKind::Sequence
    } else {
        DiagramKind::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(kind: &str, body: &[&str]) -> UmlBlock {
        UmlBlock {
            start_line: 1,
            kind_tag: kind.to_string(),
            name: None,
            body: body
                .iter()
                .enumerate()
                .map(|(i, t)| BodyLine {
                    line: i + 1,
                    text: (*t).to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn explicit_tag_wins_over_body() {
        // Body looks like a sequence diagram, but the tag says mindmap.
        let b = block("mindmap", &["A -> B"]);
        assert_eq!(detect(&b), DiagramKind::MindMap);
    }

    #[test]
    fn uml_falls_through_to_body_sniff() {
        let b = block("uml", &["participant A", "A -> B"]);
        assert_eq!(detect(&b), DiagramKind::Sequence);
    }

    #[test]
    fn fragment_uses_body_sniff() {
        let b = block("", &["A -> B : hi"]);
        assert_eq!(detect(&b), DiagramKind::Sequence);
    }
}
