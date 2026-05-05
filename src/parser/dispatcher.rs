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
    Class,
    Component,
    UseCase,
    Deployment,
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

fn sniff_body(body: &[BodyLine]) -> DiagramKind {
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

        // Sequence — participant declaration or arrow.
        if PARTICIPANT_KEYWORDS
            .iter()
            .any(|kw| t.starts_with(&format!("{kw} ")) || t == *kw)
        {
            return DiagramKind::Sequence;
        }
        if SEQUENCE_ARROWS.iter().any(|a| t.contains(a)) {
            return DiagramKind::Sequence;
        }

        if t.starts_with("[*]") || t.starts_with("state ") {
            return DiagramKind::State;
        }
        if t == "start"
            || t == "stop"
            || t == "end"
            || t.starts_with(':')
            || t.starts_with("if (")
        {
            return DiagramKind::Activity;
        }
        if t.starts_with('*') || t.starts_with('+') || t.starts_with('-') {
            return DiagramKind::MindMap;
        }
        if t.starts_with('{') {
            return DiagramKind::Json;
        }
        if t.starts_with("class ") || t.starts_with("interface ") || t.starts_with("abstract ") {
            return DiagramKind::Class;
        }
        if t.starts_with('[') {
            return DiagramKind::Component;
        }
        return DiagramKind::Unknown;
    }
    DiagramKind::Unknown
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
