//! Compare the production Rust ELK pipeline with a recorded oracle pass.
//! Usage: cargo run --example elk_compare -- <stages.json> [pass1|pass2]
use std::{error::Error, fs, process::ExitCode};
use typstuml::layout::elk::{
    alg::{graph::LGraphArena, hierarchical, transform},
    compare::coord_diff,
    graph::ElkNode,
};

fn run() -> Result<bool, Box<dyn Error>> {
    let mut args = std::env::args_os().skip(1);
    let file = args
        .next()
        .ok_or("usage: elk_compare <stages.json> [pass1|pass2]")?;
    let pass = args.next().unwrap_or_else(|| "pass2".into());
    let pass = pass.to_str().ok_or("pass must be pass1 or pass2")?;
    if !matches!(pass, "pass1" | "pass2") || args.next().is_some() {
        return Err("usage: elk_compare <stages.json> [pass1|pass2]".into());
    }
    let stages: serde_json::Value = serde_json::from_str(&fs::read_to_string(file)?)?;
    let input: ElkNode = serde_json::from_value(stages[format!("{pass}Input")].clone())?;
    let expected: ElkNode = serde_json::from_value(stages[format!("{pass}Output")].clone())?;
    let mut arena = LGraphArena::default();
    let root = transform::import_graph(&mut arena, &input);
    let result = hierarchical::layout_compound(&mut arena, root);
    let actual = transform::apply_layout_compound(&arena, root, &input, &result.reference_graphs);
    let differences = coord_diff(&expected, &actual, 0.5);
    if differences.is_empty() {
        println!("All coordinates match {pass}Output within 0.5pt");
        return Ok(true);
    }
    eprintln!(
        "{} coordinate differences from {pass}Output:",
        differences.len()
    );
    for difference in &differences {
        eprintln!("  {difference:?}");
    }
    Ok(false)
}

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("elk_compare: {error}");
            ExitCode::from(2)
        }
    }
}
