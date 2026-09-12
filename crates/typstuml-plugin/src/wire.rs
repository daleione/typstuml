//! CBOR measurement decoding shared by the versioned host adapters.
use serde::Deserialize;

/// Wire shape for one measurement: codegen-assigned ID + dimensions.
/// `row_centers` is optional (only record-graph probes write it); other
/// probes may omit this field; omission decodes as an empty array.
#[derive(Deserialize)]
struct ProbeEntry {
    id: String,
    width_pt: f64,
    height_pt: f64,
    #[serde(default)]
    row_centers: Vec<f64>,
}

pub(super) enum Validation {
    Legacy,
    Strict,
}
pub(super) fn decode(
    bytes: &[u8],
    validation: Validation,
) -> Result<typstuml::runtime::MeasurementSet, String> {
    let probes: Vec<ProbeEntry> =
        ciborium::from_reader(bytes).map_err(|e| format!("decode measurements: {e}"))?;
    let mut set = typstuml::runtime::MeasurementSet::default();
    for p in probes {
        let m = typstuml::runtime::Measurement {
            width_pt: p.width_pt,
            height_pt: p.height_pt,
            row_centers: p.row_centers,
        };
        match validation {
            Validation::Legacy => set.insert(p.id, m),
            Validation::Strict => set.insert_checked(p.id, m).map_err(|e| e.to_string())?,
        }
    }
    Ok(set)
}
