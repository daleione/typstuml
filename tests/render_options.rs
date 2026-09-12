use typstuml::{
    codegen::{self, CodegenOptions, ImportStrategy, OutputTarget, Typography},
    diagnostics::CompatMode,
    parser::{self, InputLanguage},
    render::{self, MeasurementPolicy, RenderOptions},
};

#[test]
fn compatibility_can_be_selected_independently_of_input_language() {
    let source = "flowchart TB; A-->B; classDef muted fill:red";
    assert!(render::emit_typst_with_language(source, InputLanguage::Mermaid).is_err());
    let mut options = RenderOptions::for_language(InputLanguage::Mermaid);
    options.compat = CompatMode::Warn;
    options.measurement = MeasurementPolicy::Disabled;
    let output = render::emit_typst_with_options(source, options).unwrap();
    assert!(output.contains("#flowchart-layout("));
    assert!(output.contains("from: 0, to: 1"));
}

#[test]
fn target_and_typography_are_independent_for_both_ir_families() {
    for (language, source) in [
        (InputLanguage::Mermaid, "flowchart TB; A-->B"),
        (InputLanguage::PlantUml, "@startuml\nclass A\n@enduml"),
    ] {
        let doc =
            parser::parse_with_language(source, language, CompatMode::Strict, &Default::default())
                .unwrap()
                .document;
        for target in [OutputTarget::Document, OutputTarget::Embedded] {
            for typography in [Typography::DefaultSize, Typography::Inherit] {
                let options = CodegenOptions { target, typography };
                let probes = codegen::emit_probes_with_options(&doc, &Default::default(), options)
                    .unwrap()
                    .unwrap()
                    .0;
                let output =
                    codegen::emit_with_options(&doc, &Default::default(), None, options).unwrap();
                for source in [&probes, &output] {
                    assert_eq!(
                        source.contains("#set page"),
                        target == OutputTarget::Document
                    );
                    assert_eq!(
                        source.contains("#set text(size: 10pt)"),
                        typography == Typography::DefaultSize
                    );
                }
            }
        }
        for legacy in [
            ImportStrategy::VirtualFs,
            ImportStrategy::EvalScope,
            ImportStrategy::EvalScopeInherit,
        ] {
            assert_eq!(
                codegen::emit(&doc, &Default::default(), None, legacy).unwrap(),
                codegen::emit_with_options(&doc, &Default::default(), None, legacy.into()).unwrap()
            );
        }
    }
}

#[test]
fn mixed_ir_document_has_distinct_painters_and_disjoint_probes() {
    let mut doc = parser::parse(
        "@startuml\nclass A\n@enduml",
        CompatMode::Strict,
        &Default::default(),
    )
    .unwrap()
    .document;
    let flow = parser::parse_with_language(
        "flowchart TB; A-->B",
        InputLanguage::Mermaid,
        CompatMode::Strict,
        &Default::default(),
    )
    .unwrap()
    .document;
    doc.diagrams.extend(flow.diagrams);
    let (probes, ids) = codegen::emit_probes(&doc, &Default::default(), ImportStrategy::VirtualFs)
        .unwrap()
        .unwrap();
    assert!(probes.contains("#cuca-probe("));
    assert!(probes.contains("#flowchart-probe("));
    assert_eq!(ids.len(), 3);
    let expected: Vec<_> = ids.iter().map(String::as_str).collect();
    let measured = typstuml::runtime::measure::run(probes, None, &expected).unwrap();
    let output = codegen::emit(
        &doc,
        &Default::default(),
        Some(&measured),
        ImportStrategy::VirtualFs,
    )
    .unwrap();
    assert!(output.contains("#cuca-layout("));
    assert!(output.contains("#flowchart-layout("));
    typstuml::runtime::render(output, None, typstuml::runtime::Format::Pdf).unwrap();
}
