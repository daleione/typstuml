//! Pure-Rust port of the ELK `layered` layout algorithm (work in
//! progress — see `docs/elk-port-plan.md` for the milestone plan and
//! `tools/elk-oracle/` for the elkjs ground-truth harness).
//!
//! Verification model: every ported phase must reproduce elkjs's
//! output *numerically* on identical inputs. The [`graph`] module is
//! the shared ELK JSON graph model (the exact format elkjs consumes
//! and produces), and [`compare`] is the coordinate-diff reporter the
//! milestone tests use.
//!
//! Licensing note: this module (E1) is original schema/harness code.
//! From E2 on, files ported from Eclipse ELK's Java sources land under
//! `src/layout/elk/alg/` with an EPL-2.0 LICENSE and provenance notes,
//! kept separate from the MIT-licensed rest of the crate.

pub mod adapter;
pub mod alg;
pub mod compare;
pub mod graph;
