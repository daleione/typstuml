//! Cuca (description-family) layout benchmark.
//!
//! Synthesizes PUML sources of varying size to measure:
//!   • `parse` — text → IR
//!   • `parse + emit` — text → IR → Typst source (covers compound layout)
//!
//! Sizes: 10 / 50 / 100 / 200 entities distributed across 5 packages.
//! The 50-node bucket is the M3 design-doc reference (target < 100 ms
//! for the layout phase alone on a typical dev machine).
//!
//! Run with `cargo bench` (criterion). Use
//! `cargo bench -- --save-baseline pre-m3` to capture a baseline, then
//! `cargo bench -- --baseline pre-m3` after changes for a delta report.

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use typstuml::codegen;
use typstuml::diagnostics::CompatMode;
use typstuml::parser::{self, preprocessor::Config};
use typstuml::theme::Theme;

const ENTITY_COUNTS: &[usize] = &[10, 50, 100, 200];
const CLUSTERS: usize = 5;

/// Build a PUML source with `n` classes spread across `CLUSTERS`
/// packages, plus a chain of cross-cluster edges that exercises the
/// hierarchical Sugiyama ranking. Each cluster gets `n / CLUSTERS`
/// entities; edges go from entity_i to entity_{i+1} so the DAG has a
/// long path the layout has to rank.
fn synthesize_puml(n: usize) -> String {
    let per_cluster = (n + CLUSTERS - 1) / CLUSTERS;
    let mut out = String::from("@startuml\n");
    let mut entity_idx = 0usize;
    for c in 0..CLUSTERS {
        out.push_str(&format!("package \"P{c}\" {{\n"));
        for _ in 0..per_cluster {
            if entity_idx >= n {
                break;
            }
            out.push_str(&format!("  class E{entity_idx}\n"));
            entity_idx += 1;
        }
        out.push_str("}\n");
    }
    // Chain edges so the DAG has a non-trivial longest path; every
    // ~per_cluster edges crosses a cluster boundary.
    for i in 0..n.saturating_sub(1) {
        out.push_str(&format!("E{i} --> E{}\n", i + 1));
    }
    out.push_str("@enduml\n");
    out
}

fn bench_parse(c: &mut Criterion) {
    let mut group = c.benchmark_group("cuca_parse");
    for &n in ENTITY_COUNTS {
        let src = synthesize_puml(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &src, |b, src| {
            let cfg = Config::default();
            b.iter(|| {
                let out = parser::parse(black_box(src), CompatMode::Warn, &cfg).unwrap();
                black_box(out);
            });
        });
    }
    group.finish();
}

fn bench_emit(c: &mut Criterion) {
    let mut group = c.benchmark_group("cuca_parse_emit");
    let theme = Theme::default();
    let cfg = Config::default();
    for &n in ENTITY_COUNTS {
        let src = synthesize_puml(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &src, |b, src| {
            b.iter(|| {
                let out = parser::parse(black_box(src), CompatMode::Warn, &cfg).unwrap();
                let typst_src = codegen::emit(&out.document, &theme, None).unwrap();
                black_box(typst_src);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_parse, bench_emit);
criterion_main!(benches);
