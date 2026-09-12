use super::*;
fn diagram(body: &str) -> FlowchartDiagram {
    let parsed = parse(body, CompatMode::Strict).unwrap();
    let Diagram::Flowchart(d) = parsed.document.diagrams.into_iter().next().unwrap() else {
        panic!()
    };
    d
}
#[test]
fn shapes_chains_and_plain_text() {
    let d = diagram("flowchart TD; A[\"**literal**\"] --> B{Ready?} -->|yes| C([Done]); D((Circle)); E[(DB)]; F>Odd]; G(Round)");
    assert_eq!(d.nodes.len(), 7);
    assert_eq!(d.edges.len(), 2);
    assert_eq!(d.nodes[0].label, "**literal**");
    assert_eq!(d.edges[1].label.as_deref(), Some("yes"));
}
#[test]
fn delimiters_quotes_entities_and_ids() {
    let d = diagram("graph LR; a-b[\"x; %% [y] #34;\"]-->a_b; A -->|\"x; %% [y]\"| B; C[Don't]");
    assert_eq!(d.nodes[0].id, "a-b");
    assert_eq!(d.nodes[1].id, "a_b");
    assert_eq!(d.nodes[0].label, "x; %% [y] \"");
    assert_eq!(d.edges[1].label.as_deref(), Some("x; %% [y]"));
}
#[test]
fn label_updates_in_all_modes() {
    for mode in [CompatMode::Strict, CompatMode::Warn, CompatMode::Loose] {
        let p = parse("flowchart TB; A[old]; A[new]; A-->B", mode).unwrap();
        assert!(p.diagnostics.is_empty());
        let Diagram::Flowchart(d) = &p.document.diagrams[0] else {
            panic!()
        };
        assert_eq!(d.nodes[0].label, "new");
    }
}
#[test]
fn subgraph_owner_is_finalized_after_references() {
    let d = diagram("flowchart TB; X --> A; subgraph outer [Outer]; A[Inside]; subgraph inner [Inner]; B --> C; end; end");
    assert_eq!(d.subgraphs[0].nodes, ["A"]);
    assert_eq!(d.subgraphs[1].nodes, ["B", "C"]);
    assert_eq!(d.subgraphs[0].children, [1]);
}
#[test]
fn quoted_subgraph_titles_preserve_delimiters() {
    for title in ["Title [detail]", "Title [detail", "Title; %% [detail]"] {
        for header in [format!("\"{title}\""), format!("group [\"{title}\"]")] {
            let d = diagram(&format!("flowchart TB; subgraph {header}; A; end"));
            assert_eq!(d.subgraphs[0].label, title);
            assert_eq!(d.subgraphs[0].nodes, ["A"]);
        }
    }
}
#[test]
fn unsupported_chain_and_lower_conflict_are_atomic() {
    for body in ["A --> B ??? C", "A[new] --> B --> A{conflict}"] {
        let p = parse(
            &format!("flowchart TB; A[old]; {body}; Z"),
            CompatMode::Warn,
        )
        .unwrap();
        assert_eq!(p.diagnostics.len(), 1);
        let Diagram::Flowchart(d) = &p.document.diagrams[0] else {
            panic!()
        };
        assert_eq!(d.nodes.len(), 2);
        assert!(d.edges.is_empty());
        assert_eq!(d.nodes[0].label, "old");
    }
}
#[test]
fn errors_do_not_silently_change_structure() {
    for body in [
        "flowchart ZZ; A",
        "flowchart TB; A[open",
        "flowchart TB; subgraph x; A",
        "flowchart TB; end",
        "flowchart TB; A --> s; subgraph s; B; end",
        "flowchart TB; subgraph x; A; end; subgraph y; A; end",
    ] {
        assert!(parse(body, CompatMode::Loose).is_err(), "{body}");
    }
}
#[test]
fn directives_and_other_diagrams_are_explicit() {
    assert!(parse("%%{init: {}}%%\nflowchart TB; A", CompatMode::Strict).is_err());
    let p = parse("%%{init: {}}%%\nflowchart TB; A", CompatMode::Warn).unwrap();
    assert_eq!(p.diagnostics.len(), 1);
    assert!(parse("sequenceDiagram\nA->>B: Hi", CompatMode::Loose).is_err());
}
#[test]
fn direction_axis_and_polarity() {
    for (header, lr, reverse) in [
        ("TD", false, false),
        ("TB", false, false),
        ("BT", false, true),
        ("LR", true, false),
        ("RL", true, true),
    ] {
        let d = diagram(&format!("flowchart {header}; A-->B"));
        assert_eq!(d.direction == LayoutDirection::LeftToRight, lr);
        assert_eq!(d.reverse_rank, reverse);
        assert_eq!(d.edges[0].from, "A");
        assert_eq!(d.edges[0].to, "B");
    }
}

#[test]
fn unsupported_long_arrow_is_recovered_without_phantom_labels() {
    let p = parse("flowchart TB; A---->B; C", CompatMode::Warn).unwrap();
    assert_eq!(p.diagnostics.len(), 1);
    let Diagram::Flowchart(d) = &p.document.diagrams[0] else {
        panic!()
    };
    assert_eq!(d.nodes.len(), 1);
    assert_eq!(d.nodes[0].id, "C");
}
#[test]
fn quoted_edge_symbols_are_plain_text() {
    let d = diagram("flowchart TB; A[\"x --> y\"] -- \"a; %% b\" --> B");
    assert_eq!(d.nodes[0].label, "x --> y");
    assert_eq!(d.edges[0].label.as_deref(), Some("a; %% b"));
}
