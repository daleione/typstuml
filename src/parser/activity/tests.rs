use super::parse;
use crate::diagnostics::CompatMode;
use crate::ir::{ActionKind, ActivityDiagram, ActivityStmt, Diagram};
use crate::parser::lexer::extract_uml_blocks;

fn parse_str(s: &str) -> ActivityDiagram {
    let blocks = extract_uml_blocks(s);
    let (d, _diags) = parse(&blocks[0], CompatMode::Warn).unwrap();
    match d {
        Diagram::Activity(a) => a,
        _ => panic!("expected Activity"),
    }
}

#[test]
fn linear() {
    let a = parse_str(
        "@startuml\n\
         start\n\
         :hello;\n\
         :world;\n\
         stop\n\
         @enduml\n",
    );
    assert_eq!(a.body.len(), 4);
    assert!(matches!(a.body[0], ActivityStmt::Start { .. }));
    assert!(matches!(a.body[3], ActivityStmt::Stop { .. }));
    if let ActivityStmt::Action { label, .. } = &a.body[1] {
        assert_eq!(label, &vec!["hello".to_string()]);
    } else {
        panic!("expected Action");
    }
}

#[test]
fn if_else() {
    let a = parse_str(
        "@startuml\n\
         if (ok?) then (yes)\n\
         :A;\n\
         else (no)\n\
         :B;\n\
         endif\n\
         @enduml\n",
    );
    assert_eq!(a.body.len(), 1);
    match &a.body[0] {
        ActivityStmt::If {
            cond,
            then_label,
            then_branch,
            else_label,
            else_branch,
            elseifs,
            ..
        } => {
            assert_eq!(cond, "ok?");
            assert_eq!(then_label.as_deref(), Some("yes"));
            assert_eq!(else_label.as_deref(), Some("no"));
            assert_eq!(then_branch.len(), 1);
            assert_eq!(else_branch.as_ref().unwrap().len(), 1);
            assert!(elseifs.is_empty());
        }
        _ => panic!("expected If"),
    }
}

#[test]
fn repeat_with_cond() {
    let a = parse_str(
        "@startuml\n\
         repeat\n\
         :work;\n\
         repeat while (more?) is (yes) not (no)\n\
         @enduml\n",
    );
    match &a.body[0] {
        ActivityStmt::Repeat {
            cond,
            is_label,
            not_label,
            body,
            ..
        } => {
            assert_eq!(cond.as_deref(), Some("more?"));
            assert_eq!(is_label.as_deref(), Some("yes"));
            assert_eq!(not_label.as_deref(), Some("no"));
            assert_eq!(body.len(), 1);
        }
        _ => panic!("expected Repeat"),
    }
}

#[test]
fn fork_branches() {
    let a = parse_str(
        "@startuml\n\
         fork\n\
         :A;\n\
         fork again\n\
         :B;\n\
         fork again\n\
         :C;\n\
         end fork\n\
         @enduml\n",
    );
    match &a.body[0] {
        ActivityStmt::Fork {
            branches, merge, ..
        } => {
            assert_eq!(branches.len(), 3);
            assert!(*merge);
        }
        _ => panic!("expected Fork"),
    }
}

#[test]
fn switch_with_cases() {
    let a = parse_str(
        "@startuml\n\
         switch (k)\n\
         case (a)\n\
         :A;\n\
         case (b)\n\
         :B;\n\
         endswitch\n\
         @enduml\n",
    );
    match &a.body[0] {
        ActivityStmt::Switch { cond, cases, .. } => {
            assert_eq!(cond, "k");
            assert_eq!(cases.len(), 2);
            assert_eq!(cases[0].value, "a");
            assert_eq!(cases[1].value, "b");
        }
        _ => panic!("expected Switch"),
    }
}

#[test]
fn multiline_action() {
    let a = parse_str(
        "@startuml\n\
         :line one\nline two;\n\
         @enduml\n",
    );
    if let ActivityStmt::Action { label, .. } = &a.body[0] {
        assert_eq!(label.len(), 2);
        assert_eq!(label[0], "line one");
        assert_eq!(label[1], "line two");
    } else {
        panic!("expected Action");
    }
}

#[test]
fn stereotype_action() {
    let a = parse_str(
        "@startuml\n\
         :ping; <<input>>\n\
         @enduml\n",
    );
    if let ActivityStmt::Action { kind, .. } = &a.body[0] {
        assert_eq!(*kind, ActionKind::Input);
    } else {
        panic!("expected Action");
    }
}
