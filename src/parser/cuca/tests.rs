use super::parse;
use crate::diagnostics::{CompatMode, Level};
use crate::ir::{
    ArrowHead, ClassFamilyKind, CucaDiagram, Diagram, Direction, Entity, EntityKindData, LineStyle,
    Member, USymbol, Visibility,
};
use crate::parser::lexer::{BodyLine, UmlBlock};

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

fn parse_ok(body: &[&str]) -> CucaDiagram {
    let (diagram, _) = parse(&block(body), CompatMode::Warn).expect("parse ok");
    match diagram {
        Diagram::Cuca(c) => c,
        _ => panic!("expected cuca diagram"),
    }
}

/// Helper: pattern-match the entity's compartment data and panic
/// loudly if it isn't a class-family entity.
fn compartment(e: &Entity) -> (ClassFamilyKind, &Option<String>, &[Member], &[Member]) {
    match &e.kind_data {
        EntityKindData::Compartment {
            kind,
            generic,
            fields,
            methods,
        } => (*kind, generic, fields.as_slice(), methods.as_slice()),
        other => panic!("expected compartment entity, got {other:?}"),
    }
}

#[test]
fn parses_class_with_inline_members() {
    let c = parse_ok(&[
        "class Foo {",
        "  + name: String",
        "  - count: int",
        "  + getName(): String",
        "}",
    ]);
    assert_eq!(c.entities.len(), 1);
    let foo = &c.entities[0];
    assert_eq!(foo.id, "Foo");
    let (kind, _, fields, methods) = compartment(foo);
    assert_eq!(kind, ClassFamilyKind::Class);
    assert_eq!(fields.len(), 2);
    assert_eq!(methods.len(), 1);
    assert_eq!(fields[0].visibility, Visibility::Public);
    assert_eq!(fields[1].visibility, Visibility::Private);
    assert_eq!(methods[0].body, "getName(): String");
}

#[test]
fn parses_inheritance() {
    let c = parse_ok(&["class A", "class B", "B --|> A"]);
    assert_eq!(c.relations.len(), 1);
    let r = &c.relations[0];
    assert_eq!(r.from, "B");
    assert_eq!(r.to, "A");
    assert_eq!(r.head_from, ArrowHead::None);
    assert_eq!(r.head_to, ArrowHead::TriangleOpen);
    assert_eq!(r.line_style, LineStyle::Solid);
}

#[test]
fn parses_realization_dashed() {
    let c = parse_ok(&["class A", "interface I", "A ..|> I"]);
    let r = &c.relations[0];
    assert_eq!(r.head_to, ArrowHead::TriangleOpen);
    assert_eq!(r.line_style, LineStyle::Dashed);
}

#[test]
fn parses_composition_with_mult_and_label() {
    let c = parse_ok(&[r#"A "1" *-- "*" B : owns"#]);
    let r = &c.relations[0];
    assert_eq!(r.from, "A");
    assert_eq!(r.to, "B");
    assert_eq!(r.head_from, ArrowHead::DiamondFilled);
    assert_eq!(r.mult_from.as_deref(), Some("1"));
    assert_eq!(r.mult_to.as_deref(), Some("*"));
    assert_eq!(r.label.as_deref(), Some("owns"));
}

#[test]
fn parses_member_add_line() {
    let c = parse_ok(&["class A", "A : + foo()"]);
    let (_, _, _, methods) = compartment(&c.entities[0]);
    assert_eq!(methods.len(), 1);
    assert_eq!(methods[0].body, "foo()");
}

#[test]
fn parses_static_and_abstract_modifiers() {
    let c = parse_ok(&[
        "class A {",
        "  {static} count: int",
        "  {abstract} render(): void",
        "}",
    ]);
    let (_, _, fields, methods) = compartment(&c.entities[0]);
    assert_eq!(fields.len(), 1);
    assert!(fields[0].is_static);
    assert_eq!(methods.len(), 1);
    assert!(methods[0].is_abstract);
}

#[test]
fn parses_generic_and_stereotype() {
    let c = parse_ok(&[r#"class Repo<T> <<Service>> #LightBlue"#]);
    let e = &c.entities[0];
    assert_eq!(e.id, "Repo");
    let (_, generic, _, _) = compartment(e);
    assert_eq!(generic.as_deref(), Some("T"));
    assert_eq!(e.stereotype.as_deref(), Some("Service"));
    assert_eq!(e.fill.as_deref(), Some("#LightBlue"));
}

#[test]
fn parses_alias() {
    let c = parse_ok(&[r#"class "Long Name" as Foo"#]);
    let e = &c.entities[0];
    assert_eq!(e.id, "Foo");
    assert_eq!(e.display, "Long Name");
}

#[test]
fn parses_alias_unquoted() {
    // `class Foo as Bar` — id is the alias, display keeps the original
    // name. Pre-fix, both id and display became `Bar`.
    let c = parse_ok(&["class Foo as Bar"]);
    let e = &c.entities[0];
    assert_eq!(e.id, "Bar");
    assert_eq!(e.display, "Foo");
}

#[test]
fn parses_alias_with_quoted_display() {
    // `class Foo as "Long Foo"` — id stays `Foo`, display is the quoted form.
    let c = parse_ok(&[r#"class Foo as "Long Foo""#]);
    let e = &c.entities[0];
    assert_eq!(e.id, "Foo");
    assert_eq!(e.display, "Long Foo");
}

#[test]
fn parses_package_visibility() {
    let c = parse_ok(&["class A {", "  ~ helper(): void", "}"]);
    let (_, _, _, methods) = compartment(&c.entities[0]);
    assert_eq!(methods[0].visibility, Visibility::Package);
    assert_eq!(methods[0].body, "helper(): void");
}

#[test]
fn classifier_modifier_maps_to_static() {
    let c = parse_ok(&["class A {", "  {classifier} factory(): A", "}"]);
    let (_, _, _, methods) = compartment(&c.entities[0]);
    assert!(methods[0].is_static, "{{classifier}} should set is_static");
}

#[test]
fn auto_creates_unknown_endpoint() {
    let c = parse_ok(&["class A", "A --> B"]);
    assert_eq!(c.entities.len(), 2);
    assert!(c.entities.iter().any(|e| e.id == "B"));
}

#[test]
fn unrecognized_warns() {
    let (_d, diags) = parse(&block(&["frobnicate"]), CompatMode::Warn).unwrap();
    assert_eq!(diags.len(), 1);
    assert_eq!(diags[0].level, Level::Warning);
}

#[test]
fn parses_note_on_link_inline() {
    let c = parse_ok(&["class A", "class B", "A --> B", "note on link : reads from"]);
    let r = &c.relations[0];
    assert_eq!(r.note.as_deref(), Some("reads from"));
}

#[test]
fn parses_note_on_link_multiline() {
    let c = parse_ok(&[
        "class A",
        "class B",
        "A --> B",
        "note left on link",
        "  body line 1",
        "end note",
    ]);
    let r = &c.relations[0];
    assert_eq!(r.note.as_deref().unwrap().trim(), "body line 1");
}

#[test]
fn parses_note_over_two_targets() {
    let c = parse_ok(&["class A", "class B", "note over A, B : shared invariant"]);
    let note = c
        .entities
        .iter()
        .find(|e| e.usymbol == USymbol::Note)
        .unwrap();
    assert_eq!(note.kind_data.note_body(), Some("shared invariant"));
    // Two auto-relations, one for each target.
    assert_eq!(c.relations.len(), 2);
    assert!(c.relations.iter().any(|r| r.to == "A"));
    assert!(c.relations.iter().any(|r| r.to == "B"));
    for r in &c.relations {
        assert_eq!(r.line_style, LineStyle::Dashed);
        assert_eq!(r.from, note.id);
    }
}

#[test]
fn parses_anchored_note_inline() {
    let c = parse_ok(&["class Foo", "note left of Foo : just a hint"]);
    // Entities: Foo + the auto-generated note.
    assert_eq!(c.entities.len(), 2);
    let note = c
        .entities
        .iter()
        .find(|e| e.usymbol == USymbol::Note)
        .unwrap();
    assert_eq!(note.kind_data.note_body(), Some("just a hint"));
    // Auto-generated id starts with `__note_`.
    assert!(note.id.starts_with("__note_"));
    // Auto-relation: dashed, no heads, direction Left.
    assert_eq!(c.relations.len(), 1);
    let r = &c.relations[0];
    assert_eq!(r.from, note.id);
    assert_eq!(r.to, "Foo");
    assert_eq!(r.line_style, LineStyle::Dashed);
    assert_eq!(r.head_from, ArrowHead::None);
    assert_eq!(r.head_to, ArrowHead::None);
    assert_eq!(r.direction, Some(Direction::Left));
}

#[test]
fn parses_anchored_note_multiline() {
    let c = parse_ok(&[
        "class Foo",
        "note right of Foo",
        "  first line",
        "  second line",
        "end note",
    ]);
    let note = c
        .entities
        .iter()
        .find(|e| e.usymbol == USymbol::Note)
        .unwrap();
    let body = note.kind_data.note_body().unwrap();
    assert!(body.contains("first line"));
    assert!(body.contains("second line"));
    assert_eq!(c.relations[0].direction, Some(Direction::Right));
}

#[test]
fn parses_quoted_note_with_alias() {
    let c = parse_ok(&["class Foo", "note \"hello world\" as N1", "N1 .. Foo"]);
    let note = c.entities.iter().find(|e| e.id == "N1").unwrap();
    assert_eq!(note.usymbol, USymbol::Note);
    assert_eq!(note.kind_data.note_body(), Some("hello world"));
    // User-written N1 .. Foo should produce one relation; no auto-rel.
    assert_eq!(c.relations.len(), 1);
    let r = &c.relations[0];
    assert_eq!(r.from, "N1");
    assert_eq!(r.to, "Foo");
    assert_eq!(r.line_style, LineStyle::Dashed);
}

#[test]
fn parses_package_with_nested_class() {
    let c = parse_ok(&[
        "package \"Domain\" {",
        "  class Order",
        "  class LineItem",
        "}",
        "class External",
    ]);
    assert_eq!(c.entities.len(), 3);
    assert_eq!(c.containers.len(), 1);
    let pkg = &c.containers[0];
    assert_eq!(pkg.usymbol, USymbol::Package);
    assert!(!pkg.together);
    assert_eq!(pkg.label, "Domain");
    assert_eq!(pkg.children_entities, vec!["Order", "LineItem"]);
}

#[test]
fn parses_nested_namespaces() {
    let c = parse_ok(&[
        "namespace outer {",
        "  namespace inner {",
        "    class Inner",
        "  }",
        "  class Mid",
        "}",
    ]);
    assert_eq!(c.containers.len(), 2);
    let outer = c.containers.iter().find(|c| c.label == "outer").unwrap();
    let inner = c.containers.iter().find(|c| c.label == "inner").unwrap();
    // outer holds Mid + a child container ref.
    assert!(outer.children_entities.contains(&"Mid".to_string()));
    assert_eq!(outer.children_containers.len(), 1);
    // inner holds Inner.
    assert_eq!(inner.children_entities, vec!["Inner"]);
}

#[test]
fn parses_together_block() {
    let c = parse_ok(&["together {", "  class A", "  class B", "}"]);
    assert_eq!(c.containers.len(), 1);
    let t = &c.containers[0];
    assert!(t.together);
    assert!(t.label.is_empty());
    assert_eq!(t.children_entities, vec!["A", "B"]);
}

#[test]
fn parses_association_class_left_couple() {
    let c = parse_ok(&["class A", "class B", "class C", "A -- B", "(A, B) .. C"]);
    // Two relations: the regular A--B and the couple .. C.
    assert_eq!(c.relations.len(), 2);
    let assoc = &c.relations[1];
    assert_eq!(assoc.from_couple, Some(("A".into(), "B".into())));
    assert_eq!(assoc.to, "C");
    assert_eq!(assoc.line_style, LineStyle::Dashed);
}

#[test]
fn parses_association_class_right_couple_normalizes() {
    // `C -- (A, B)` — the couple is on the right; parser swaps so
    // the IR consistently has from_couple + to.
    let c = parse_ok(&["class A", "class B", "class C", "C -- (A, B)"]);
    let assoc = &c.relations[0];
    assert_eq!(assoc.from_couple, Some(("A".into(), "B".into())));
    assert_eq!(assoc.to, "C");
}

#[test]
fn parses_lollipop_decl() {
    let c = parse_ok(&["() Foo"]);
    assert_eq!(c.entities.len(), 1);
    let e = &c.entities[0];
    assert_eq!(e.usymbol, USymbol::Interface);
    assert_eq!(e.id, "Foo");
}

#[test]
fn lollipop_in_relation_auto_creates_circle() {
    // `(Iface)` references a lollipop — auto-create as Interface
    // (lollipop). `class A` declared explicitly stays Class.
    let c = parse_ok(&["class A", "A --> (Iface)"]);
    let iface = c.entities.iter().find(|e| e.id == "Iface").unwrap();
    assert_eq!(iface.usymbol, USymbol::Interface);
}

#[test]
fn parses_custom_stereotype_marker_with_color() {
    let c = parse_ok(&["class Robot <<(R, #FF8800) Service>>"]);
    let e = &c.entities[0];
    assert_eq!(e.stereotype.as_deref(), Some("Service"));
    let marker = e.stereotype_marker.as_ref().unwrap();
    assert_eq!(marker.letter, "R");
    assert_eq!(marker.color.as_deref(), Some("#FF8800"));
}

#[test]
fn parses_custom_marker_without_color() {
    let c = parse_ok(&["class Foo <<(X) something>>"]);
    let e = &c.entities[0];
    assert_eq!(e.stereotype.as_deref(), Some("something"));
    let marker = e.stereotype_marker.as_ref().unwrap();
    assert_eq!(marker.letter, "X");
    assert!(marker.color.is_none());
}

#[test]
fn parses_member_port() {
    let c = parse_ok(&[
        "class A {",
        "  + name: String",
        "}",
        "class B",
        "A::name --> B",
    ]);
    // Two classes, no phantom `A::name` entity.
    assert_eq!(c.entities.len(), 2);
    let r = &c.relations[0];
    assert_eq!(r.from, "A");
    assert_eq!(r.from_port.as_deref(), Some("name"));
    assert_eq!(r.to, "B");
    assert!(r.to_port.is_none());
}

#[test]
fn member_port_on_target_side() {
    let c = parse_ok(&[
        "class A",
        "class B {",
        "  + value: int",
        "}",
        "A --> B::value",
    ]);
    let r = &c.relations[0];
    assert!(r.from_port.is_none());
    assert_eq!(r.to_port.as_deref(), Some("value"));
}

#[test]
fn parses_edge_inline_color() {
    let c = parse_ok(&["class A", "class B", "A -[#red]-> B"]);
    let r = &c.relations[0];
    assert_eq!(r.color.as_deref(), Some("#red"));
}

#[test]
fn parses_edge_color_with_extra_modifier() {
    let c = parse_ok(&["class A", "class B", "A -[#abcdef,bold]-> B"]);
    let r = &c.relations[0];
    assert_eq!(r.color.as_deref(), Some("#abcdef"));
}

#[test]
fn parses_hide_directives() {
    let c = parse_ok(&["hide circle", "hide methods", "hide stereotype", "class A"]);
    assert!(c.hide.circle);
    assert!(c.hide.methods);
    assert!(c.hide.stereotype);
    assert!(!c.hide.fields);
}

#[test]
fn show_reverses_hide() {
    let c = parse_ok(&["hide circle", "show circle", "class A"]);
    assert!(!c.hide.circle);
}

#[test]
fn parses_freestanding_note_block() {
    let c = parse_ok(&["note as N1", "  body line", "end note"]);
    let note = &c.entities[0];
    assert_eq!(note.usymbol, USymbol::Note);
    assert_eq!(note.id, "N1");
    assert_eq!(note.kind_data.note_body().unwrap().trim(), "body line");
    // No auto relation for freestanding form.
    assert!(c.relations.is_empty());
}

// --- M5-partial / M6: desc family + inline shorthand ---

#[test]
fn parses_component_keyword() {
    let c = parse_ok(&["component Foo"]);
    assert_eq!(c.entities.len(), 1);
    assert_eq!(c.entities[0].usymbol, USymbol::Component);
    assert_eq!(c.entities[0].id, "Foo");
    assert!(matches!(
        c.entities[0].kind_data,
        EntityKindData::Plain { .. }
    ));
}

#[test]
fn parses_actor_keyword() {
    let c = parse_ok(&["actor Bob"]);
    assert_eq!(c.entities[0].usymbol, USymbol::Actor);
}

#[test]
fn parses_usecase_keyword() {
    let c = parse_ok(&["usecase Login"]);
    assert_eq!(c.entities[0].usymbol, USymbol::UseCase);
}

#[test]
fn parses_database_as_leaf() {
    let c = parse_ok(&[r#"database "User DB" as UDB"#]);
    assert_eq!(c.entities[0].usymbol, USymbol::Database);
    assert_eq!(c.entities[0].id, "UDB");
    assert_eq!(c.entities[0].display, "User DB");
}

#[test]
fn parses_database_as_container() {
    // `database "X" {` opens a cluster, not a leaf.
    let c = parse_ok(&[r#"database "Cluster" {"#, "class Inner", "}"]);
    assert_eq!(c.containers.len(), 1);
    assert_eq!(c.containers[0].usymbol, USymbol::Database);
    assert_eq!(c.containers[0].label, "Cluster");
    assert_eq!(c.containers[0].children_entities, vec!["Inner"]);
}

#[test]
fn parses_component_shorthand() {
    let c = parse_ok(&["[WebApp]"]);
    assert_eq!(c.entities[0].usymbol, USymbol::Component);
    assert_eq!(c.entities[0].id, "WebApp");
}

#[test]
fn parses_usecase_shorthand() {
    let c = parse_ok(&["(Login)"]);
    assert_eq!(c.entities[0].usymbol, USymbol::UseCase);
    assert_eq!(c.entities[0].id, "Login");
}

#[test]
fn parses_actor_shorthand() {
    let c = parse_ok(&[":Bob:"]);
    assert_eq!(c.entities[0].usymbol, USymbol::Actor);
    assert_eq!(c.entities[0].id, "Bob");
}

#[test]
fn parses_socket_open_head() {
    // `Foo -( Bar` — right-end socket (PlantUML LinkDecor.PARENTHESIS).
    let c = parse_ok(&["class Foo", "class Bar", "Foo -( Bar"]);
    assert_eq!(c.relations.len(), 1);
    let r = &c.relations[0];
    assert_eq!(r.from, "Foo");
    assert_eq!(r.to, "Bar");
    assert_eq!(r.head_to, ArrowHead::SocketOpen);
    assert_eq!(r.head_from, ArrowHead::None);
}

#[test]
fn parses_socket_closed_head() {
    // `Foo )- Bar` — left-end socket.
    let c = parse_ok(&["class Foo", "class Bar", "Foo )- Bar"]);
    let r = &c.relations[0];
    assert_eq!(r.head_from, ArrowHead::SocketClosed);
    assert_eq!(r.head_to, ArrowHead::None);
}

#[test]
fn shorthand_does_not_swallow_relation_line() {
    // `[A] --> [B]` is a relation line with two component shorthand
    // endpoints. parse_inline_shorthand must reject it (trailing
    // `--> [B]` isn't `as`/`<<`/`#`) so parse_relation gets to run.
    // The endpoint-cleanup (strip `[…]` brackets from auto-created
    // ids and tag them as USymbol::Component) is a follow-up — for
    // now we just confirm the inline-shorthand parser bows out so
    // the relation parser can take the line.
    let c = parse_ok(&["[A] --> [B]"]);
    assert_eq!(c.relations.len(), 1, "relation must still be parsed");
}

#[test]
fn couple_form_not_misread_as_shorthand() {
    // `(A, B) .. C` is the association-class couple form. The comma
    // inside parens disambiguates it from `(Foo)` usecase shorthand.
    let c = parse_ok(&["class A", "class B", "class C", "(A, B) .. C"]);
    assert_eq!(c.entities.len(), 3);
    assert_eq!(c.relations.len(), 1);
    assert_eq!(c.relations[0].from_couple, Some(("A".into(), "B".into())));
    assert_eq!(c.relations[0].to, "C");
}
