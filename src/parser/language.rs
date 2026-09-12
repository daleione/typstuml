use crate::diagnostics::{Error, Result};

/// Explicit input syntax. Existing entry points continue to use PlantUML.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InputLanguage {
    #[default]
    PlantUml,
    Mermaid,
}

impl std::str::FromStr for InputLanguage {
    type Err = Error;
    fn from_str(value: &str) -> Result<Self> {
        match value {
            "puml" => Ok(Self::PlantUml),
            "mermaid" => Ok(Self::Mermaid),
            _ => Err(Error::Cli(format!(
                "unknown input language {value:?}; expected puml or mermaid"
            ))),
        }
    }
}
