use super::*;

fn block(body: &[&str]) -> UmlBlock {
    UmlBlock {
        start_line: 1,
        kind_tag: "uml".into(),
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

fn parse_ok(body: &[&str]) -> StructuredSequence {
    let (diagram, _) = parse(&block(body), CompatMode::Warn).expect("parse ok");
    match diagram {
        Diagram::Sequence(SequenceDiagram::Structured(s)) => s,
        _ => panic!("expected structured sequence"),
    }
}

#[test]
fn parses_basic_message() {
    let s = parse_ok(&["A -> B : hi"]);
    assert_eq!(s.steps.len(), 1);
    match &s.steps[0] {
        Step::Message {
            from,
            to,
            arrow,
            label,
            ..
        } => {
            assert_eq!(from, "A");
            assert_eq!(to, "B");
            assert_eq!(arrow, "->");
            assert_eq!(label.as_deref(), Some("hi"));
        }
        other => panic!("expected message, got {other:?}"),
    }
}

#[test]
fn parses_color_arrow() {
    let s = parse_ok(&["Alice -[#red]-> Bob : hello"]);
    match &s.steps[0] {
        Step::Message { arrow, .. } => assert_eq!(arrow, "-[#red]->"),
        _ => panic!(),
    }
}

#[test]
fn parses_unspaced_arrow() {
    // PlantUML accepts arrows without whitespace around them — we used
    // to drop these as "unsupported" because the declared participants
    // then looked unreferenced to the seq-puml validator downstream.
    let s = parse_ok(&[
        "actor Bob #red",
        "participant Alice",
        "Alice->Bob: Authentication Request",
        "Bob->Alice: Authentication Response",
    ]);
    assert_eq!(s.steps.len(), 2, "expected both messages parsed");
    match &s.steps[0] {
        Step::Message { from, to, arrow, label, .. } => {
            assert_eq!(from, "Alice");
            assert_eq!(to, "Bob");
            assert_eq!(arrow, "->");
            assert_eq!(label.as_deref(), Some("Authentication Request"));
        }
        _ => panic!("expected message step"),
    }
}

#[test]
fn identifier_ending_in_letter_does_not_swallow_arrow_head() {
    // Without whitespace, the trailing `o` belongs to the identifier
    // `Otto`, not the arrow head — we only fold `o`/`x` into the arrow
    // when separated by whitespace.
    let s = parse_ok(&["Otto->Alice : hi"]);
    match &s.steps[0] {
        Step::Message { from, to, arrow, .. } => {
            assert_eq!(from, "Otto");
            assert_eq!(arrow, "->");
            assert_eq!(to, "Alice");
        }
        _ => panic!(),
    }
}

#[test]
fn parses_o_head_arrow_with_spaces() {
    // With whitespace around the arrow, the `o` head is unambiguous.
    let s = parse_ok(&["Alice ->o Bob : hi"]);
    match &s.steps[0] {
        Step::Message { arrow, to, .. } => {
            assert_eq!(arrow, "->o");
            assert_eq!(to, "Bob");
        }
        _ => panic!(),
    }
}

#[test]
fn participants_with_alias() {
    let s = parse_ok(&[
        r#"participant "Alice 张" as A"#,
        "actor Bob",
        "A -> Bob : hi",
    ]);
    assert_eq!(s.participants.len(), 2);
    assert_eq!(s.participants[0].id, "A");
    assert_eq!(s.participants[0].display, "Alice 张");
    assert_eq!(s.participants[0].kind, ParticipantKind::Participant);
    assert_eq!(s.participants[1].id, "Bob");
    assert_eq!(s.participants[1].kind, ParticipantKind::Actor);
}

#[test]
fn participants_with_unquoted_alias() {
    // PlantUML: `participant <Display> as <Id>` — the token after `as`
    // is the alias used in messages.
    let s = parse_ok(&[
        "participant Participant as Foo",
        "actor Actor as Foo1",
        "Foo -> Foo1 : hi",
    ]);
    assert_eq!(s.participants.len(), 2);
    assert_eq!(s.participants[0].id, "Foo");
    assert_eq!(s.participants[0].display, "Participant");
    assert_eq!(s.participants[1].id, "Foo1");
    assert_eq!(s.participants[1].display, "Actor");
}

#[test]
fn fragment_alt_with_else_and_nested() {
    let s = parse_ok(&[
        "alt cond",
        "  A -> B : x",
        "  loop forever",
        "    B -> A : y",
        "  end",
        "else other",
        "  A -> B : z",
        "end",
    ]);
    assert_eq!(s.steps.len(), 1);
    match &s.steps[0] {
        Step::Fragment { kind, branches, .. } => {
            assert_eq!(*kind, FragmentKind::Alt);
            assert_eq!(branches.len(), 2);
            // First branch has 2 steps: a message and a nested loop fragment.
            assert_eq!(branches[0].steps.len(), 2);
            assert!(matches!(
                branches[0].steps[1],
                Step::Fragment {
                    kind: FragmentKind::Loop,
                    ..
                }
            ));
            assert_eq!(branches[1].label.as_deref(), Some("other"));
        }
        _ => panic!("expected fragment"),
    }
}

#[test]
fn note_over_two_participants() {
    let s = parse_ok(&["A -> B : x", "note over A, B : a comment"]);
    match s.steps.last().unwrap() {
        Step::Note {
            participants,
            text,
            position,
            ..
        } => {
            assert_eq!(*position, NotePosition::Over);
            assert_eq!(participants, &["A".to_string(), "B".to_string()]);
            assert_eq!(text, "a comment");
        }
        _ => panic!("expected note"),
    }
}

#[test]
fn multiline_note_accumulates() {
    let s = parse_ok(&[
        "note over A",
        "  line one",
        "  line two",
        "end note",
        "A -> B : after",
    ]);
    match &s.steps[0] {
        Step::Note { text, .. } => {
            assert!(text.contains("line one"));
            assert!(text.contains("line two"));
        }
        _ => panic!("expected note"),
    }
}

#[test]
fn skinparam_collected() {
    let s = parse_ok(&["skinparam backgroundColor #EEE", "A -> B : x"]);
    assert_eq!(s.skinparams.len(), 1);
    assert_eq!(s.skinparams[0].key, "backgroundColor");
    assert_eq!(s.skinparams[0].value, "#EEE");
}

#[test]
fn divider_and_autonumber() {
    let s = parse_ok(&[
        "autonumber 10 5",
        "A -> B : x",
        "== checkpoint ==",
        "B -> A : y",
    ]);
    assert!(matches!(s.steps[0], Step::Autonumber { .. }));
    assert!(matches!(s.steps[2], Step::Divider { .. }));
}

#[test]
fn unrecognized_line_emits_warning() {
    let (_diagram, diags) =
        parse(&block(&["frobnicate the foozle"]), CompatMode::Warn).unwrap();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].level, Level::Warning);
}

#[test]
fn strict_mode_fails_on_unrecognized() {
    let res = parse(&block(&["frobnicate the foozle"]), CompatMode::Strict);
    assert!(res.is_err());
}
