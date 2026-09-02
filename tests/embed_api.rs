#![cfg(feature = "embed-typst")]

use std::io::Write;

use typstuml::diagnostics::Level;
use typstuml::render::{render_source, Format};

const SEQUENCE: &str = "@startuml\nAlice -> Bob: hello\n@enduml\n";

#[test]
fn render_source_returns_in_memory_svg_and_size_from_a_worker_thread() {
    let untouched = tempfile::tempdir().expect("temporary observation directory");

    let rendered = std::thread::spawn(|| render_source(SEQUENCE, Format::Svg))
        .join()
        .expect("render thread does not panic")
        .expect("sequence renders");

    assert!(String::from_utf8_lossy(&rendered.bytes).contains("<svg"));
    let size = rendered
        .size
        .expect("single-page output has a natural size");
    assert!(size.width_pt > 0.0, "width was {}", size.width_pt);
    assert!(size.height_pt > 0.0, "height was {}", size.height_pt);
    assert_eq!(rendered.page_sizes, vec![size]);
    assert!(rendered.warnings.is_empty(), "{:?}", rendered.warnings);
    assert_eq!(
        std::fs::read_dir(untouched.path())
            .expect("read observation directory")
            .count(),
        0,
        "in-memory rendering must not create output or temporary files"
    );
}

#[test]
fn memory_mode_reports_include_without_reading_it() {
    let source = format!("!include {}\n{SEQUENCE}", env!("CARGO_MANIFEST_DIR"));
    let rendered = render_source(&source, Format::Svg).expect("diagram still renders");
    assert!(rendered.warnings.iter().any(|warning| {
        warning.level == Level::Warning
            && warning.message.contains("filesystem access is disabled")
            && warning.message.contains("!include")
    }));
}

#[test]
fn root_none_cannot_read_a_file_from_the_current_working_directory() {
    let mut file = tempfile::Builder::new()
        .prefix("typstuml-cwd-denied-")
        .suffix(".txt")
        .tempfile_in(std::env::current_dir().expect("current directory"))
        .expect("create cwd sentinel");
    file.write_all(b"secret from cwd").expect("write sentinel");
    let name = file
        .path()
        .file_name()
        .expect("sentinel filename")
        .to_string_lossy();

    let error = typstuml::runtime::render(
        format!("#read(\"{name}\")"),
        None,
        typstuml::runtime::Format::Svg,
    )
    .expect_err("root=None must not fall back to cwd");

    assert!(error.to_string().contains("access denied"));
    assert!(!error.to_diagnostics().is_empty());
}

#[test]
fn rooted_world_allows_descendants_but_rejects_parent_escape() {
    let base = tempfile::tempdir().expect("temporary project parent");
    let root = base.path().join("project");
    std::fs::create_dir(&root).expect("create project root");
    std::fs::write(root.join("inside.txt"), "inside").expect("write inside file");
    std::fs::write(base.path().join("secret.txt"), "outside").expect("write outside file");

    let rendered = typstuml::runtime::render(
        "#read(\"inside.txt\")".into(),
        Some(root.clone()),
        typstuml::runtime::Format::Svg,
    )
    .expect("descendant is readable with an explicit root");
    assert!(String::from_utf8_lossy(&rendered.bytes).contains("<svg"));

    typstuml::runtime::render(
        "#read(\"../secret.txt\")".into(),
        Some(root),
        typstuml::runtime::Format::Svg,
    )
    .expect_err("parent traversal must not escape the project root");
}
