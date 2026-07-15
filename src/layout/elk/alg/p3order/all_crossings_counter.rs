//! Port of
//! `org.eclipse.elk.alg.layered.p3order.counting.AllCrossingsCounter`
//! for the flat draw-uml scope.
//!
//! Scope: every anonymous per-edge port carries exactly one edge, so
//! there are **no hyperedges** (`HyperedgeCrossingsCounter` never runs);
//! a proper layering has **no in-layer edges** and there are **no
//! north/south ports**. `countAllCrossings` therefore reduces to summing
//! `countCrossingsBetweenLayers` over adjacent layer pairs (the WEST
//! side-0 and EAST last-layer in-layer terms are structurally 0). We
//! assert those preconditions rather than porting the hyperedge/NS
//! machinery.

use super::super::graph::{LGraphArena, LNodeId, NodeType};
use super::crossings_counter::CrossingsCounter;

/// Sum of all edge crossings in the current node order. The counter's
/// position scratch array is indexed by each port's arena id, so it is
/// sized to `arena.ports.len()`.
pub fn count_all_crossings(arena: &LGraphArena, order: &[Vec<LNodeId>]) -> i32 {
    if order.is_empty() {
        return 0;
    }
    debug_assert!(
        order.iter().flatten().all(|&n| arena.nodes[n.0].node_type != NodeType::NorthSouthPort),
        "north/south ports are outside the flat crossing-count scope"
    );
    let mut counter = CrossingsCounter::new(arena, vec![0; arena.ports.len()]);
    let mut crossings = 0;
    for i in 0..order.len().saturating_sub(1) {
        crossings += counter.count_crossings_between_layers(&order[i], &order[i + 1]);
    }
    crossings
}
