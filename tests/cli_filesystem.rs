#![cfg(feature = "cli")]

use assert_cmd::Command;
use predicates::prelude::*;

const SEQUENCE: &str = "@startuml\nAlice -> Bob: hello\n@enduml\n";

#[test]
fn file_input_resolves_puml_include_relative_to_the_project() {
    let project = tempfile::tempdir().expect("temporary CLI project");
    let main = project.path().join("main.puml");
    let included = project.path().join("included.puml");
    let output = project.path().join("out.svg");
    std::fs::write(&main, "!include included.puml\n").expect("write main source");
    std::fs::write(included, SEQUENCE).expect("write included source");

    Command::cargo_bin("typstuml")
        .expect("typstuml binary")
        .arg(main)
        .arg(&output)
        .assert()
        .success();

    assert!(std::fs::read_to_string(output)
        .expect("read SVG")
        .contains("<svg"));
}

#[test]
fn stdin_typst_preamble_cannot_read_from_the_command_working_directory() {
    let cwd = tempfile::tempdir().expect("temporary command cwd");
    let preamble = cwd.path().join("preamble.typ");
    std::fs::write(&preamble, "#let leaked = read(\"secret.txt\")\n").expect("write preamble");
    std::fs::write(cwd.path().join("secret.txt"), "secret").expect("write secret");

    Command::cargo_bin("typstuml")
        .expect("typstuml binary")
        .current_dir(cwd.path())
        .args(["--preamble", "preamble.typ", "-f", "svg", "-", "-"])
        .write_stdin(SEQUENCE)
        .assert()
        .failure()
        .stderr(predicate::str::contains("access denied"));
}

#[test]
fn file_input_typst_root_rejects_parent_traversal() {
    let base = tempfile::tempdir().expect("temporary project parent");
    let project = base.path().join("project");
    std::fs::create_dir(&project).expect("create project");
    let main = project.join("main.puml");
    let preamble = project.join("preamble.typ");
    std::fs::write(&main, SEQUENCE).expect("write main source");
    std::fs::write(&preamble, "#let leaked = read(\"../secret.txt\")\n").expect("write preamble");
    std::fs::write(base.path().join("secret.txt"), "secret").expect("write secret");

    Command::cargo_bin("typstuml")
        .expect("typstuml binary")
        .arg("--preamble")
        .arg(preamble)
        .arg(main)
        .arg("-")
        .assert()
        .failure()
        .stderr(predicate::str::contains("escape the project root"));
}

#[test]
fn file_input_include_cannot_escape_the_project_root() {
    let base = tempfile::tempdir().expect("temporary project parent");
    let project = base.path().join("project");
    std::fs::create_dir(&project).expect("create project");
    let main = project.join("main.puml");
    std::fs::write(&main, "!include ../secret.puml\n").expect("write main source");
    std::fs::write(base.path().join("secret.puml"), SEQUENCE).expect("write outside include");

    Command::cargo_bin("typstuml")
        .expect("typstuml binary")
        .args(["--compat", "strict"])
        .arg(main)
        .arg("-")
        .assert()
        .failure()
        .stderr(predicate::str::contains("escapes the project/include roots"));
}
