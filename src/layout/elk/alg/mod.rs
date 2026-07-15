//! Rust port of Eclipse ELK's `layered` algorithm — EPL-2.0, see
//! `LICENSE.md` and `README.md` in this directory. Upstream:
//! eclipse-elk/elk v0.11.0, `plugins/org.eclipse.elk.alg.layered/`.
//!
//! Mechanical conventions of the port (behavior is otherwise kept
//! line-for-line with the Java sources named in each module header):
//! - Java object references become arena indices ([`graph::LGraphArena`]
//!   owns every element; `LNodeId` & friends are typed indices).
//! - `null` becomes `Option`.
//! - The Java `IProperty` maps become plain structs with one field per
//!   option actually used by the ported scope (draw-uml's
//!   configuration; see docs/elk-port-plan.md §2).

pub mod compaction;
pub mod components;
pub mod compound;
pub mod graph;
pub mod hierarchical;
pub mod high_degree;
pub mod intermediate;
pub mod layer_constraint;
pub mod math;
pub mod network_simplex;
pub mod options;
pub mod p1_cycles;
pub mod p2_layers;
pub mod p3order;
pub mod p4nodes;
pub mod p5edges;
pub mod preserve_order;
pub mod random;
pub mod spacings;
pub mod transform;
