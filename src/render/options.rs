//! Entry-point defaults. The pipeline consumes explicit policies, never node kinds.
use crate::{
    codegen::{CodegenOptions, OutputTarget, Typography},
    diagnostics::CompatMode,
    parser::InputLanguage,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MeasurementPolicy {
    Required,
    BestEffort,
    Disabled,
}

#[derive(Clone, Copy, Debug)]
pub struct RenderOptions {
    pub language: InputLanguage,
    pub compat: CompatMode,
    pub measurement: MeasurementPolicy,
    pub codegen: CodegenOptions,
}

impl RenderOptions {
    /// Defaults retained by the original language-selecting APIs.
    pub fn for_language(language: InputLanguage) -> Self {
        let (compat, measurement) = match language {
            InputLanguage::PlantUml => (CompatMode::Warn, MeasurementPolicy::BestEffort),
            InputLanguage::Mermaid => (CompatMode::Strict, MeasurementPolicy::Required),
        };
        Self {
            language,
            compat,
            measurement,
            codegen: CodegenOptions::default(),
        }
    }

    pub fn for_plugin(language: InputLanguage) -> Self {
        let mut options = Self::for_language(language);
        options.codegen.target = OutputTarget::Embedded;
        options.codegen.typography = match language {
            InputLanguage::PlantUml => Typography::DefaultSize,
            InputLanguage::Mermaid => Typography::Inherit,
        };
        options
    }
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self::for_language(InputLanguage::PlantUml)
    }
}
