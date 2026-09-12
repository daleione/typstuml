//! Real Typst host test. Build the stripped plugin first, then run this ignored test.
use std::{path::Path, process::Command};
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}

fn plugin_host() -> tempfile::TempDir {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let stage = tempfile::tempdir().unwrap();
    let wasm = root.join("target/wasm32-unknown-unknown/wasm-release/typstuml_plugin.wasm");
    assert!(
        wasm.is_file(),
        "build the plugin first: ./crates/typstuml-plugin/build.sh --no-opt"
    );
    copy_tree(&root.join("components"), &stage.path().join("blockcell"));
    std::fs::copy(
        root.join("crates/typstuml-plugin/package/lib.typ"),
        stage.path().join("lib.typ"),
    )
    .unwrap();
    std::fs::copy(wasm, stage.path().join("typstuml.wasm")).unwrap();
    stage
}

#[test]
#[ignore = "requires typst CLI and ./crates/typstuml-plugin/build.sh --no-opt"]
fn mixed_languages_and_reused_content_have_isolated_measurements() {
    let stage = plugin_host();
    let file = stage.path().join("test.typ");
    std::fs::write(
        &file,
        include_str!("fixtures/plugin/measurement-isolation.typ"),
    )
    .unwrap();
    let output = Command::new("typst")
        .args(["query"])
        .arg(&file)
        .args(["<result>", "--field", "value"])
        .output()
        .expect("typst CLI");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let values: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(values[0]["instances"], 4);
    assert_eq!(values[0]["probes"], 11);
}

#[test]
#[ignore = "requires typst CLI and ./crates/typstuml-plugin/build.sh --no-opt"]
fn versioned_wire_errors_are_reported_by_the_typst_host() {
    let stage = plugin_host();
    let file = stage.path().join("wire.typ");
    let prefix = r##"
#let p = plugin("typstuml.wasm")
#let src = bytes("flowchart TB; A")
#let lang = bytes("mermaid")
#let bundle = cbor(p.emit_probes_v2(lang, src))
#let row = (id: bundle.expected_ids.first(), width_pt: 40.0, height_pt: 30.0)
#assert.eq(int.from-bytes(p.protocol_version(), endian: "little"), 2)
#assert(str(p.emit_probes(bytes("@startuml\nclass A\n@enduml"))).starts-with("#set text"))
"##;
    for (call, expected) in [
        (
            "#p.emit_layout_v2(lang, src, cbor.encode(()))",
            "missing measurement",
        ),
        (
            "#p.emit_layout_v2(lang, src, cbor.encode((row, row)))",
            "duplicate measurement",
        ),
        (
            "#p.emit_layout_v2(lang, src, cbor.encode((row + (width_pt: -1.0),)))",
            "invalid dimensions",
        ),
        (
            "#p.emit_layout_v2(lang, src, cbor.encode((row, row + (id: \"foreign\"))))",
            "unexpected measurement",
        ),
        (
            "#p.emit_probes_v2(bytes(\"unknown\"), src)",
            "unknown input language",
        ),
    ] {
        std::fs::write(&file, format!("{prefix}\n{call}")).unwrap();
        let output = Command::new("typst")
            .arg("compile")
            .arg(&file)
            .arg(stage.path().join("bad.svg"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(
            String::from_utf8_lossy(&output.stderr).contains(expected),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
