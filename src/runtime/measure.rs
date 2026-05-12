//! Pass-1 of the measure double-pass protocol.
//!
//! `run` compiles a probe-only Typst document, queries it for all elements
//! tagged with `<typstuml_measure>` (placed by blockcell's `*-probe`
//! functions), and decodes the resulting `metadata((id, w, h))` dicts into
//! a [`MeasurementSet`] keyed by codegen-assigned IDs.
//!
//! See `docs/measure-protocol.md` for the full protocol — this module
//! implements §4.4 (Rust ingestion) and §5 (the public API).
//!
//! The label name `typstuml_measure` is fixed by protocol — using a `-`
//! would make Typst's lexer parse it as subtraction and break label
//! attachment.

use std::collections::HashMap;
use std::path::PathBuf;

use typst::foundations::{Label, Selector, Value};
use typst::layout::PagedDocument;
use typst::utils::PicoStr;

use crate::diagnostics::{Error, Result};

use super::world::TypstWorld;

/// The natural width / height of a single probed content piece, in
/// typographic points. Codegen uses these to size node bboxes before
/// running layout.
///
/// `row_centers` is populated only for record-graph probes (one entry
/// per row, the local-frame vertical centre used as the edge anchor).
/// Class / package probes leave it empty.
#[derive(Clone, Debug, PartialEq)]
pub struct Measurement {
    pub width_pt: f64,
    pub height_pt: f64,
    pub row_centers: Vec<f64>,
}

impl Measurement {
    pub fn new(width_pt: f64, height_pt: f64) -> Self {
        Self {
            width_pt,
            height_pt,
            row_centers: Vec::new(),
        }
    }
}

/// Map from codegen-assigned probe ID → measurement. Built once per
/// pass-1 compile and threaded down through codegen so every consumer
/// of `text_width_pt` is replaced with a `set.get(&id)` lookup.
#[derive(Default, Debug, Clone)]
pub struct MeasurementSet {
    items: HashMap<String, Measurement>,
}

impl MeasurementSet {
    pub fn get(&self, id: &str) -> Option<Measurement> {
        self.items.get(id).cloned()
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Insert a measurement directly. Kept `pub` so tests can build a set
    /// without going through `run`.
    pub fn insert(&mut self, id: String, m: Measurement) {
        self.items.insert(id, m);
    }
}

/// Run the measure pass: compile `probe_source` against a fresh
/// `TypstWorld` (which shares the process-wide font cache), then walk
/// every `<typstuml_measure>` metadata element in the resulting document.
///
/// `root` is the document root passed to `TypstWorld` for resolving local
/// `#image()` / `#read()` references in the user's preamble — same
/// meaning as in [`super::render`]. `expected_ids` is an optional sanity
/// check: any ID listed here that's missing from the returned set
/// surfaces as an `Error::MeasureProtocol`, catching codegen bugs where a
/// probe was emitted but not consumed (or vice versa).
pub fn run(
    probe_source: String,
    root: PathBuf,
    expected_ids: &[&str],
) -> Result<MeasurementSet> {
    let world = TypstWorld::new(root, probe_source);
    let warned = typst::compile::<PagedDocument>(&world);
    let document = warned.output.map_err(|errors| {
        Error::TypstCompile(super::format_typst_diagnostics(&world, &errors))
    })?;

    let label = Label::new(PicoStr::intern("typstuml_measure")).ok_or_else(|| {
        Error::MeasureProtocol(
            "label name 'typstuml_measure' interned as empty PicoStr (impossible)".to_string(),
        )
    })?;
    let selector = Selector::Label(label);
    let elements = document.introspector.query(&selector);

    let mut set = MeasurementSet::default();
    for content in &elements {
        let value = content
            .field_by_name("value")
            .map_err(|e| Error::MeasureProtocol(format!("metadata.value field: {e}")))?;
        let dict = match value {
            Value::Dict(d) => d,
            other => {
                return Err(Error::MeasureProtocol(format!(
                    "expected metadata.value to be a dict, got {:?}",
                    other
                )));
            }
        };

        let id = read_str(&dict, "id")?;
        let w = read_f64(&dict, "w")?;
        let h = read_f64(&dict, "h")?;
        let row_centers = read_f64_array_opt(&dict, "row_centers")?;

        let measurement = Measurement {
            width_pt: w,
            height_pt: h,
            row_centers,
        };
        if let Some(existing) = set.items.insert(id.clone(), measurement) {
            return Err(Error::MeasureProtocol(format!(
                "duplicate probe id {id:?}: previous = {:?}",
                existing
            )));
        }
    }

    for &id in expected_ids {
        if !set.items.contains_key(id) {
            return Err(Error::MeasureProtocol(format!(
                "probe id missing in pass-1 output: {id}"
            )));
        }
    }

    Ok(set)
}

fn read_str(dict: &typst::foundations::Dict, key: &str) -> Result<String> {
    match dict.get(key) {
        Ok(Value::Str(s)) => Ok(s.as_str().to_string()),
        Ok(other) => Err(Error::MeasureProtocol(format!(
            "expected metadata.{key} to be a string, got {:?}",
            other
        ))),
        Err(e) => Err(Error::MeasureProtocol(format!(
            "metadata missing field {key}: {e}"
        ))),
    }
}

fn read_f64(dict: &typst::foundations::Dict, key: &str) -> Result<f64> {
    match dict.get(key) {
        Ok(Value::Float(f)) => Ok(*f),
        // `.pt()` in Typst returns a float, but a probe author writing a
        // literal `0pt.pt()` could surface as an integer. Accept both.
        Ok(Value::Int(i)) => Ok(*i as f64),
        Ok(other) => Err(Error::MeasureProtocol(format!(
            "expected metadata.{key} to be a number, got {:?}",
            other
        ))),
        Err(e) => Err(Error::MeasureProtocol(format!(
            "metadata missing field {key}: {e}"
        ))),
    }
}

/// Read an optional `Value::Array` of floats / ints. Returns an empty
/// vec when the key is missing — record probes set it, class / package
/// probes don't.
fn read_f64_array_opt(dict: &typst::foundations::Dict, key: &str) -> Result<Vec<f64>> {
    match dict.get(key) {
        Ok(Value::Array(arr)) => arr
            .iter()
            .map(|v| match v {
                Value::Float(f) => Ok(*f),
                Value::Int(i) => Ok(*i as f64),
                other => Err(Error::MeasureProtocol(format!(
                    "expected metadata.{key}[] entry to be a number, got {:?}",
                    other
                ))),
            })
            .collect(),
        Ok(other) => Err(Error::MeasureProtocol(format!(
            "expected metadata.{key} to be an array, got {:?}",
            other
        ))),
        // Field absent → that's fine, not all probes carry it.
        Err(_) => Ok(Vec::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_returns_metadata_dimensions() {
        let source = r#"
#import "/blockcell/lib.typ": class-probe
#class-probe(id: "test-class", spec: (kind: "class", name: [Animal]))
"#;
        let set = run(source.to_string(), std::env::current_dir().unwrap(), &["test-class"])
            .expect("measure pass succeeds");
        let m = set.get("test-class").expect("test-class probe present");
        // Width / height are font-dependent; just sanity check positivity.
        assert!(m.width_pt > 0.0, "got width {}", m.width_pt);
        assert!(m.height_pt > 0.0, "got height {}", m.height_pt);
    }

    #[test]
    fn run_reports_missing_expected_id() {
        let source = r#"
#import "/blockcell/lib.typ": class-probe
#class-probe(id: "present", spec: (kind: "class", name: [A]))
"#;
        let err = run(
            source.to_string(),
            std::env::current_dir().unwrap(),
            &["present", "absent"],
        )
        .expect_err("missing id should fail");
        assert!(matches!(err, Error::MeasureProtocol(_)), "got {err:?}");
    }

    #[test]
    fn empty_source_yields_empty_set() {
        let set = run(String::new(), std::env::current_dir().unwrap(), &[])
            .expect("empty source compiles");
        assert!(set.is_empty());
    }
}
