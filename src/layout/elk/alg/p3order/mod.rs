//! Port of ELK `layered` phase 3 (`org.eclipse.elk.alg.layered.p3order`):
//! crossing minimization. EPL-2.0 (see `../LICENSE.md`).
//!
//! Filled in incrementally (see docs/elk-port-plan.md §4, E5 拆解).
//! Currently ported: the counting primitives
//! (`p3order/counting/BinaryIndexedTree`).

pub mod all_crossings_counter;
pub mod binary_indexed_tree;
pub mod crossings_counter;
pub mod layer_sweep;
pub mod layer_sweep_type_decider;
