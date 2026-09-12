//! Shared native / embedded measurement orchestration. Hosts own I/O and reporting.
use super::{MeasurementPolicy, RenderOptions};
use crate::{
    codegen,
    diagnostics::{Diagnostic, Result},
    ir::Document,
    runtime,
    theme::Theme,
};
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
use std::{path::PathBuf, time::Duration};

pub(crate) struct Prepared {
    pub source: String,
    pub warnings: Vec<Diagnostic>,
    #[cfg_attr(any(not(feature = "cli"), target_arch = "wasm32"), allow(dead_code))]
    pub measurement: Option<(usize, Duration)>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{diagnostics::CompatMode, parser::InputLanguage};

    #[test]
    fn measurement_failure_policy_is_explicit_for_both_diagram_families() {
        for (language, source, probe) in [
            (
                InputLanguage::Mermaid,
                "flowchart TB; A-->B",
                "flowchart-probe",
            ),
            (
                InputLanguage::PlantUml,
                "@startuml\nclass A\n@enduml",
                "cuca-probe",
            ),
        ] {
            let doc = crate::parser::parse_with_language(
                source,
                language,
                CompatMode::Strict,
                &Default::default(),
            )
            .unwrap()
            .document;
            let dir = tempfile::tempdir().unwrap();
            let preamble = dir.path().join("probe-failure.typ");
            std::fs::write(
                &preamble,
                format!("#let {probe}(..args) = panic(\"injected probe failure\")"),
            )
            .unwrap();
            let theme = Theme {
                preamble: Some(preamble),
            };
            let mut options = RenderOptions::for_language(language);
            options.measurement = MeasurementPolicy::Required;
            assert!(prepare(&doc, &theme, None, options).is_err());

            options.measurement = MeasurementPolicy::BestEffort;
            let result = prepare(&doc, &theme, None, options).unwrap();
            assert_eq!(result.warnings.len(), 1);
            runtime::render(result.source, None, runtime::Format::Svg).unwrap();

            options.measurement = MeasurementPolicy::Disabled;
            let result = prepare(&doc, &theme, None, options).unwrap();
            assert!(result.warnings.is_empty());
            assert!(result.measurement.is_none());
            runtime::render(result.source, None, runtime::Format::Svg).unwrap();
        }
    }
}

pub(crate) fn prepare(
    doc: &Document,
    theme: &Theme,
    root: Option<PathBuf>,
    options: RenderOptions,
) -> Result<Prepared> {
    let emit = |set| codegen::emit_with_options(doc, theme, set, options.codegen);
    let unmeasured = || {
        emit(None).map(|source| Prepared {
            source,
            warnings: vec![],
            measurement: None,
        })
    };
    if options.measurement == MeasurementPolicy::Disabled {
        return unmeasured();
    }
    let Some((source, ids)) = codegen::emit_probes_with_options(doc, theme, options.codegen)?
    else {
        return unmeasured();
    };
    let refs: Vec<_> = ids.iter().map(String::as_str).collect();
    #[cfg(not(target_arch = "wasm32"))]
    let start = Instant::now();
    match runtime::measure::run(source, root, &refs) {
        Ok(set) => {
            #[cfg(not(target_arch = "wasm32"))]
            let elapsed = start.elapsed();
            #[cfg(target_arch = "wasm32")]
            let elapsed = Duration::ZERO;
            Ok(Prepared {
                source: emit(Some(&set))?,
                warnings: vec![],
                measurement: Some((set.len(), elapsed)),
            })
        }
        Err(error) if options.measurement == MeasurementPolicy::BestEffort => {
            let mut result = unmeasured()?;
            result.warnings.push(Diagnostic::warning(format!(
                "measure pass failed ({error}); falling back to heuristic"
            )));
            Ok(result)
        }
        Err(error) => Err(error),
    }
}
