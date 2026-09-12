mod common;
use typstuml::render::{
    emit_layout_with_language, emit_probe_bundle, emit_typst_with_language,
    render_source_with_language,
};
use typstuml::{
    codegen::ImportStrategy,
    parser::InputLanguage,
    runtime::{Format, Measurement, MeasurementSet},
};

#[test]
fn supported_shapes_render_and_measure_without_fallback() {
    for source in [
        include_str!("fixtures/mermaid/node-shapes.mmd"),
        include_str!("fixtures/mermaid/subgraph-nested.mmd"),
        "flowchart TB; subgraph \"Title; %% [detail\"; A; end",
    ] {
        let r = render_source_with_language(source, InputLanguage::Mermaid, Format::Svg).unwrap();
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        assert!(String::from_utf8(r.bytes).unwrap().contains("<svg"));
        let parsed =
            typstuml::parser::mermaid::parse(source, typstuml::diagnostics::CompatMode::Strict)
                .unwrap();
        let no_measure = typstuml::codegen::emit(
            &parsed.document,
            &Default::default(),
            None,
            ImportStrategy::VirtualFs,
        )
        .unwrap();
        typstuml::runtime::render(no_measure, None, Format::Svg).unwrap();
    }
}
#[test]
fn labels_are_literal_and_probe_ids_do_not_collide() {
    let source = "flowchart TB; a-b[\"**literal**\"]; a_b[Long label]; a-b -->|\"//text//\"| a_b";
    let bundle = emit_probe_bundle(source, InputLanguage::Mermaid).unwrap();
    assert_eq!(bundle.expected_ids.len(), 3);
    MeasurementSet::validate_expected_ids(&bundle.expected_ids).unwrap();
    let output = emit_typst_with_language(source, InputLanguage::Mermaid).unwrap();
    assert!(output.contains("#text(\"**literal**\")"));
    assert!(output.contains("#text(\"//text//\")"));
    assert!(!output.contains("#strong["));
    assert!(!output.contains("#emph["));
}
#[test]
fn strict_measurement_wire_rejects_incomplete_or_foreign_data() {
    let source = "flowchart TB; A-->B";
    let bundle = emit_probe_bundle(source, InputLanguage::Mermaid).unwrap();
    let mut set = MeasurementSet::default();
    assert!(emit_layout_with_language(source, InputLanguage::Mermaid, Some(&set)).is_err());
    for id in &bundle.expected_ids {
        set.insert_checked(id.clone(), Measurement::new(40.0, 30.0))
            .unwrap();
    }
    assert!(emit_layout_with_language(source, InputLanguage::Mermaid, Some(&set)).is_ok());
    assert!(set
        .insert_checked(bundle.expected_ids[0].clone(), Measurement::new(1.0, 1.0))
        .is_err());
    assert!(set
        .insert_checked("invalid".into(), Measurement::new(f64::NAN, 1.0))
        .is_err());
    set.insert_checked("foreign".into(), Measurement::new(1.0, 1.0))
        .unwrap();
    assert!(emit_layout_with_language(source, InputLanguage::Mermaid, Some(&set)).is_err());
}
#[test]
fn cli_language_suffix_and_explicit_stdin() {
    use assert_cmd::Command;
    Command::cargo_bin("typstuml")
        .unwrap()
        .args(["check", "tests/fixtures/mermaid/node-shapes.mmd"])
        .assert()
        .success();
    Command::cargo_bin("typstuml")
        .unwrap()
        .args(["--lang", "mermaid", "check", "-"])
        .write_stdin("flowchart TB; A-->B")
        .assert()
        .success();
    Command::cargo_bin("typstuml")
        .unwrap()
        .args([
            "--include",
            ".",
            "check",
            "tests/fixtures/mermaid/node-shapes.mmd",
        ])
        .assert()
        .failure();
}

fn positions(source: &str) -> Vec<(f64, f64, f64, f64)> {
    let out = emit_typst_with_language(source, InputLanguage::Mermaid).unwrap();
    out.lines()
        .filter(|l| l.contains("kind: \"flow-"))
        .map(|l| {
            let number = |key: &str| {
                l.split(key)
                    .nth(1)
                    .unwrap()
                    .split("pt")
                    .next()
                    .unwrap()
                    .parse::<f64>()
                    .unwrap()
            };
            (
                number("x: "),
                number("y: "),
                number("width: "),
                number("height: "),
            )
        })
        .collect()
}
#[test]
fn rendered_rank_direction_and_circle_containment() {
    for (direction, horizontal, reverse) in [
        ("TD", false, false),
        ("BT", false, true),
        ("LR", true, false),
        ("RL", true, true),
    ] {
        let source = format!("flowchart {direction}; A[First] --> B[Second]");
        let p = positions(&source);
        let (a, b) = if horizontal {
            (p[0].0, p[1].0)
        } else {
            (p[0].1, p[1].1)
        };
        assert_eq!(a > b, reverse, "{direction}: {p:?}");
        assert_ne!(a, b);
        let svg =
            render_source_with_language(&source, InputLanguage::Mermaid, Format::Svg).unwrap();
        assert!(svg.warnings.is_empty());
    }
    let source = "flowchart TB; A((\"Wide label\nSecond line\nThird line\"))";
    let p = positions(source);
    assert!((p[0].2 - p[0].3).abs() < 0.01);
    render_source_with_language(source, InputLanguage::Mermaid, Format::Svg).unwrap();
}
#[test]
fn cycles_self_loops_and_long_branch_labels_render() {
    for direction in ["TB", "BT", "LR", "RL"] {
        let source = format!("flowchart {direction}; A{{Question}} -->|A very long affirmative answer| B((Result)); A -->|Another long negative answer| C>Retry]; C --> A; B --> B");
        let r = render_source_with_language(&source, InputLanguage::Mermaid, Format::Svg).unwrap();
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
        let output = emit_typst_with_language(&source, InputLanguage::Mermaid).unwrap();
        assert!(output.contains("label-pos:"));
    }
}

#[test]
fn fixture_goldens_and_measured_svg() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mermaid");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    paths.sort();
    for path in paths {
        let source = std::fs::read_to_string(&path).unwrap();
        let name = path.file_stem().unwrap().to_str().unwrap();
        let parsed =
            typstuml::parser::mermaid::parse(&source, typstuml::diagnostics::CompatMode::Strict)
                .unwrap();
        let emitted = typstuml::codegen::emit(
            &parsed.document,
            &Default::default(),
            None,
            ImportStrategy::VirtualFs,
        )
        .unwrap();
        common::assert_golden_in("mermaid", name, &emitted);
        let rendered =
            render_source_with_language(&source, InputLanguage::Mermaid, Format::Svg).unwrap();
        assert!(
            rendered.warnings.is_empty(),
            "{name}: {:?}",
            rendered.warnings
        );
        let svg = String::from_utf8(rendered.bytes).unwrap();
        assert!(common::svg_viewbox_width(&svg).unwrap() > 20.0, "{name}");
    }
}

#[test]
fn watch_rebuilds_mermaid_after_source_changes() {
    use std::{
        process::{Child, Command, Stdio},
        time::{Duration, Instant},
    };
    struct Watcher(Child);
    impl Drop for Watcher {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("flow.mmd");
    let output = dir.path().join("flow.svg");
    std::fs::write(&input, "flowchart TB; A-->B").unwrap();
    let _watcher = Watcher(
        Command::new(env!("CARGO_BIN_EXE_typstuml"))
            .arg("watch")
            .arg(&input)
            .arg(&output)
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let wait = |previous: &[u8]| {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(bytes) = std::fs::read(&output) {
                if bytes != previous && bytes.trim_ascii_end().ends_with(b"</svg>") {
                    break bytes;
                }
            }
            assert!(
                Instant::now() < deadline,
                "watch did not produce a complete updated SVG"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let initial = wait(&[]);
    std::fs::write(&input, "flowchart TB; A-->B-->C").unwrap();
    let updated = wait(&initial);
    assert_ne!(updated, initial);
}
