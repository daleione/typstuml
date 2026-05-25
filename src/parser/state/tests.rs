use super::{parse, FINAL_ID, INITIAL_ID};
use crate::diagnostics::CompatMode;
use crate::ir::{
    BorderStyle, Diagram, Direction, LayoutDirection, LineStyle, NoteAnchor, NotePosition,
    RegionOrient, StateDiagram, StateKind,
};
use crate::parser::lexer::extract_uml_blocks;

fn parse_str(src: &str) -> StateDiagram {
    let blocks = extract_uml_blocks(src);
    let (d, _) = parse(&blocks[0], CompatMode::Warn).unwrap();
    match d {
        Diagram::State(s) => s,
        _ => panic!("expected state diagram"),
    }
}

#[test]
fn simple_states_and_transition() {
    let d = parse_str("@startuml\nstate A\nstate B\nA --> B\n@enduml\n");
    assert_eq!(d.nodes.len(), 2);
    assert_eq!(d.transitions.len(), 1);
    assert_eq!(d.transitions[0].from, "A");
    assert_eq!(d.transitions[0].to, "B");
}

#[test]
fn initial_and_final() {
    let d = parse_str("@startuml\n[*] --> A\nA --> [*]\n@enduml\n");
    let kinds: Vec<_> = d.nodes.iter().map(|n| (n.id.as_str(), n.kind)).collect();
    assert!(kinds.contains(&(INITIAL_ID, StateKind::Initial)));
    assert!(kinds.contains(&(FINAL_ID, StateKind::Final)));
    assert_eq!(d.transitions.len(), 2);
    assert_eq!(d.transitions[0].from, INITIAL_ID);
    assert_eq!(d.transitions[1].to, FINAL_ID);
}

#[test]
fn quoted_alias() {
    let d = parse_str("@startuml\nstate \"Long Name\" as L\nstate B as \"Bee\"\n@enduml\n");
    let l = d.nodes.iter().find(|n| n.id == "L").unwrap();
    assert_eq!(l.display, "Long Name");
    let b = d.nodes.iter().find(|n| n.id == "B").unwrap();
    assert_eq!(b.display, "Bee");
}

#[test]
fn stereotype_shortcuts() {
    let d =
        parse_str("@startuml\nstate C <<choice>>\nstate F <<fork>>\nstate J <<join>>\n@enduml\n");
    assert_eq!(d.nodes[0].kind, StateKind::Choice);
    assert_eq!(d.nodes[1].kind, StateKind::Fork);
    assert_eq!(d.nodes[2].kind, StateKind::Join);
}

#[test]
fn transition_label_three_parts() {
    let d = parse_str("@startuml\nA --> B : evt [guard] / act()\n@enduml\n");
    let t = &d.transitions[0];
    assert_eq!(t.event.as_deref(), Some("evt"));
    assert_eq!(t.guard.as_deref(), Some("guard"));
    assert_eq!(t.action.as_deref(), Some("act()"));
}

#[test]
fn transition_label_event_only() {
    let d = parse_str("@startuml\nA --> B : just an event\n@enduml\n");
    let t = &d.transitions[0];
    assert_eq!(t.event.as_deref(), Some("just an event"));
    assert!(t.guard.is_none());
    assert!(t.action.is_none());
}

#[test]
fn reverse_arrow_swaps_endpoints() {
    let d = parse_str("@startuml\nB <-- A\n@enduml\n");
    assert_eq!(d.transitions[0].from, "A");
    assert_eq!(d.transitions[0].to, "B");
}

#[test]
fn direction_and_style_hints() {
    let d = parse_str("@startuml\nA -up-> B\nA -[#blue,dashed]-> C\n@enduml\n");
    assert_eq!(d.transitions[0].direction, Some(Direction::Up));
    assert_eq!(d.transitions[1].line_style, LineStyle::Dashed);
    assert_eq!(d.transitions[1].color.as_deref(), Some("#blue"));
}

#[test]
fn colors_and_border_style() {
    let d = parse_str(
        "@startuml\n\
         state A #LightBlue\n\
         state B ##[dashed]red\n\
         state C ##[bold]#888888\n\
         @enduml\n",
    );
    let a = d.nodes.iter().find(|n| n.id == "A").unwrap();
    assert_eq!(a.fill.as_deref(), Some("#LightBlue"));
    let b = d.nodes.iter().find(|n| n.id == "B").unwrap();
    assert_eq!(b.border_style, Some(BorderStyle::Dashed));
    assert_eq!(b.border_color.as_deref(), Some("#red"));
    let c = d.nodes.iter().find(|n| n.id == "C").unwrap();
    assert_eq!(c.border_style, Some(BorderStyle::Bold));
    assert_eq!(c.border_color.as_deref(), Some("#888888"));
}

#[test]
fn left_to_right_direction() {
    let d = parse_str("@startuml\nleft to right direction\nstate A\n@enduml\n");
    assert_eq!(d.direction, LayoutDirection::LeftToRight);
}

#[test]
fn horizontal_classification() {
    let d = parse_str(
        "@startuml\n\
         A -> B\n\
         A --> C\n\
         A -right-> D\n\
         A -down-> E\n\
         A -[#red]-> F\n\
         @enduml\n",
    );
    // `->` single dash → horizontal.
    assert!(d.transitions[0].horizontal);
    // `-->` double dash → vertical rank edge.
    assert!(!d.transitions[1].horizontal);
    // `-right->` hint → horizontal regardless of dash count.
    assert!(d.transitions[2].horizontal);
    // `-down->` hint → vertical.
    assert!(!d.transitions[3].horizontal);
    // `-[#red]->` is the two-dash form → vertical (bracket dash ignored).
    assert!(!d.transitions[4].horizontal);
}

#[test]
fn addfield_appends_body() {
    let d = parse_str("@startuml\nstate A\nA : entry / start()\nA : exit / stop()\n@enduml\n");
    let a = d.nodes.iter().find(|n| n.id == "A").unwrap();
    assert_eq!(a.body, vec!["entry / start()", "exit / stop()"]);
}

#[test]
fn inline_body_in_decl() {
    let d = parse_str("@startuml\nstate A : do / work()\n@enduml\n");
    let a = d.nodes.iter().find(|n| n.id == "A").unwrap();
    assert_eq!(a.body, vec!["do / work()"]);
}

#[test]
fn self_loop() {
    let d = parse_str("@startuml\nstate A\nA --> A : retry\n@enduml\n");
    assert_eq!(d.transitions.len(), 1);
    assert_eq!(d.transitions[0].from, "A");
    assert_eq!(d.transitions[0].to, "A");
}

#[test]
fn auto_create_from_transition() {
    let d = parse_str("@startuml\nFoo --> Bar\n@enduml\n");
    assert!(d.nodes.iter().any(|n| n.id == "Foo"));
    assert!(d.nodes.iter().any(|n| n.id == "Bar"));
}

#[test]
fn title_directive() {
    let d = parse_str("@startuml\ntitle My Machine\nstate A\n@enduml\n");
    assert_eq!(d.title.as_deref(), Some("My Machine"));
}

#[test]
fn composite_states_nest() {
    let d = parse_str(
        "@startuml\n\
         state Outer {\n\
           state Inner1\n\
           state Inner2\n\
           Inner1 --> Inner2\n\
           state Deep {\n\
             state Leaf\n\
           }\n\
         }\n\
         state Sibling\n\
         Outer --> Sibling\n\
         @enduml\n",
    );
    let outer = d.nodes.iter().find(|n| n.id == "Outer").unwrap();
    assert_eq!(outer.kind, StateKind::Composite);
    assert!(outer.children.contains(&"Inner1".to_string()));
    assert!(outer.children.contains(&"Inner2".to_string()));
    assert!(outer.children.contains(&"Deep".to_string()));
    let inner1 = d.nodes.iter().find(|n| n.id == "Inner1").unwrap();
    assert_eq!(inner1.parent.as_deref(), Some("Outer"));
    let deep = d.nodes.iter().find(|n| n.id == "Deep").unwrap();
    assert_eq!(deep.kind, StateKind::Composite);
    assert!(deep.children.contains(&"Leaf".to_string()));
    let leaf = d.nodes.iter().find(|n| n.id == "Leaf").unwrap();
    assert_eq!(leaf.parent.as_deref(), Some("Deep"));
    // `Sibling` is top-level.
    let sib = d.nodes.iter().find(|n| n.id == "Sibling").unwrap();
    assert_eq!(sib.parent, None);
}

#[test]
fn composite_with_alias() {
    let d = parse_str("@startuml\nstate \"Long Name\" as C {\n  state X\n}\n@enduml\n");
    let c = d.nodes.iter().find(|n| n.id == "C").unwrap();
    assert_eq!(c.kind, StateKind::Composite);
    assert_eq!(c.display, "Long Name");
    assert!(c.children.contains(&"X".to_string()));
}

#[test]
fn unmatched_brace_warns() {
    let blocks = extract_uml_blocks("@startuml\nstate A\n}\n@enduml\n");
    let (_, diags) = parse(&blocks[0], CompatMode::Warn).unwrap();
    assert!(diags.iter().any(|x| x.message.contains("unmatched")));
    let r = parse(&blocks[0], CompatMode::Strict);
    assert!(r.is_err());
}

#[test]
fn unclosed_composite_recovers() {
    let blocks = extract_uml_blocks("@startuml\nstate C {\n  state X\n@enduml\n");
    let (d, diags) = parse(&blocks[0], CompatMode::Warn).unwrap();
    let s = match d {
        Diagram::State(s) => s,
        _ => panic!(),
    };
    assert!(s.nodes.iter().any(|n| n.id == "X"));
    assert!(diags.iter().any(|x| x.message.contains("unclosed")));
}

#[test]
fn note_single_and_multiline() {
    let d = parse_str(
        "@startuml\n\
         state A\n\
         note right of A : a quick note\n\
         note left of A\n\
         line one\n\
         line two\n\
         end note\n\
         @enduml\n",
    );
    assert_eq!(d.notes.len(), 2);
    match &d.notes[0].anchor {
        NoteAnchor::OfNode { node_id, side } => {
            assert_eq!(node_id, "A");
            assert_eq!(*side, NotePosition::RightOf);
        }
        _ => panic!("expected OfNode"),
    }
    assert_eq!(d.notes[0].body, "a quick note");
    assert_eq!(d.notes[1].body, "line one\nline two");
    match &d.notes[1].anchor {
        NoteAnchor::OfNode { side, .. } => assert_eq!(*side, NotePosition::LeftOf),
        _ => panic!(),
    }
}

#[test]
fn concurrent_regions_split() {
    let d = parse_str(
        "@startuml\n\
         state Active {\n\
           [*] --> NumOff\n\
           NumOff --> NumOn : press\n\
           --\n\
           [*] --> CapsOff\n\
           CapsOff --> CapsOn : press\n\
         }\n\
         @enduml\n",
    );
    assert_eq!(d.regions.len(), 1);
    let rg = &d.regions[0];
    assert_eq!(rg.composite_id, "Active");
    assert_eq!(rg.orientation, RegionOrient::Horizontal);
    assert_eq!(rg.partitions.len(), 2);
    assert!(rg.partitions[0].contains(&"NumOff".to_string()));
    assert!(rg.partitions[1].contains(&"CapsOff".to_string()));
    // Each region gets its own scoped `[*]`.
    let initials = d
        .nodes
        .iter()
        .filter(|n| n.kind == StateKind::Initial)
        .count();
    assert_eq!(initials, 2);
}

#[test]
fn vertical_divider_orientation() {
    let d = parse_str("@startuml\nstate S {\n  state A\n  ||\n  state B\n}\n@enduml\n");
    assert_eq!(d.regions.len(), 1);
    assert_eq!(d.regions[0].orientation, RegionOrient::Vertical);
}

#[test]
fn divider_outside_composite_warns() {
    let blocks = extract_uml_blocks("@startuml\nstate A\n--\nstate B\n@enduml\n");
    let (_, diags) = parse(&blocks[0], CompatMode::Warn).unwrap();
    assert!(diags
        .iter()
        .any(|x| x.message.contains("outside a composite")));
}

#[test]
fn note_on_link_binds_last_transition() {
    let d = parse_str("@startuml\nA --> B : go\nnote on link : crossing\n@enduml\n");
    assert_eq!(d.notes.len(), 1);
    match &d.notes[0].anchor {
        NoteAnchor::OnLink { transition_idx } => assert_eq!(*transition_idx, 0),
        _ => panic!("expected OnLink"),
    }
    assert_eq!(d.notes[0].body, "crossing");
}

#[test]
fn note_on_link_multiline() {
    let d = parse_str("@startuml\nA --> B\nnote on link\nfirst\nsecond\nend note\n@enduml\n");
    assert_eq!(d.notes.len(), 1);
    assert_eq!(d.notes[0].body, "first\nsecond");
}

#[test]
fn floating_note_with_link() {
    let d = parse_str(
        "@startuml\n\
         [*] --> Foo\n\
         note \"a floating note\" as N1\n\
         N1 .. Foo\n\
         @enduml\n",
    );
    assert_eq!(d.notes.len(), 1);
    match &d.notes[0].anchor {
        NoteAnchor::Floating { id, links } => {
            assert_eq!(id, "N1");
            assert_eq!(links, &["Foo".to_string()]);
        }
        _ => panic!("expected Floating"),
    }
    assert_eq!(d.notes[0].body, "a floating note");
}

#[test]
fn floating_note_unconnected() {
    let d = parse_str("@startuml\nstate A\nnote \"lonely\" as N9\n@enduml\n");
    assert_eq!(d.notes.len(), 1);
    match &d.notes[0].anchor {
        NoteAnchor::Floating { id, links } => {
            assert_eq!(id, "N9");
            assert!(links.is_empty());
        }
        _ => panic!("expected Floating"),
    }
}

#[test]
fn entry_exit_point_stereotypes() {
    let d = parse_str(
        "@startuml\n\
         state S {\n\
           state e <<entryPoint>>\n\
           state x <<exitPoint>>\n\
         }\n\
         @enduml\n",
    );
    let e = d.nodes.iter().find(|n| n.id == "e").unwrap();
    assert_eq!(e.kind, StateKind::EntryPoint);
    let x = d.nodes.iter().find(|n| n.id == "x").unwrap();
    assert_eq!(x.kind, StateKind::ExitPoint);
}

#[test]
fn floating_note_link_reversed_and_multi() {
    let d = parse_str(
        "@startuml\n\
         note \"n\" as N1\n\
         A .. N1\n\
         N1 .. B\n\
         @enduml\n",
    );
    match &d.notes[0].anchor {
        NoteAnchor::Floating { links, .. } => {
            assert_eq!(links, &["A".to_string(), "B".to_string()]);
        }
        _ => panic!("expected Floating"),
    }
}
