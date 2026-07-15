//! Port of `org.eclipse.elk.alg.layered.p3order.LayerSweepCrossingMinimizer`
//! and the objects `GraphInfoHolder` wires together — the barycenter
//! heuristic (`BarycenterHeuristic`), the port distributors
//! (`AbstractBarycenterPortDistributor` + `NodeRelative`/`LayerTotal`),
//! the constraint resolver (`ForsterConstraintResolver`), and
//! `SweepCopy`. EPL-2.0 (see `../LICENSE.md`).
//!
//! This is the **hierarchy-aware** minimizer: `initialize` walks the
//! nesting tree breadth-first, building one [`GraphInfo`] per graph, and
//! `LayerSweepTypeDecider` decides per graph whether it sweeps
//! *bottom-up* (independently, then feeds its parent's port order) or is
//! swept *into* by its parent (`sweepInHierarchicalNode`). A single flat
//! graph is the degenerate case (no children → no dive), byte-exact with
//! the earlier flat-only port (the flat fixtures guard that).
//!
//! Scope kept from the flat port: `ForsterConstraintResolver` is a no-op
//! (no in-layer successor constraints), `crossingCounterNode/PortInfluence`
//! default 0 (plain `minimizeCrossingsWithCounter`, no model-order
//! penalty), crossing minimizer is `BarycenterHeuristic`.
//!
//! Random handshake: every graph's `GraphInfoHolder` wires its
//! `BarycenterHeuristic` to **that graph's own** `RANDOM` property — the
//! root's is the shared crossing-min random (same object the orchestrator
//! draws direction booleans from and `set_seed`s per try), a nested
//! graph's is its own post-cycle-break `Random(1)`. So barycenter
//! jitters/fills/randomize draw from the graph being swept, while
//! orchestrator-level draws (run-seed `next_long`, per-try `set_seed` +
//! direction `next_boolean`) stay on the root random. `initialize`
//! additionally draws one `next_boolean` per graph (NodeRelative vs
//! LayerTotal distributor) from that graph's own random, in BFS order.
//!
//! **Orientation**: everywhere port sides drive hierarchical decisions we
//! read a normalized rightward side (NORTH→WEST, SOUTH→EAST) — see
//! [`super::layer_sweep_type_decider`] — because this port skips ELK's
//! import rotation, leaving group/external ports on NORTH/SOUTH.

use super::super::graph::{LGraphArena, LGraphId, LNodeId, LPortId, NodeType};
use super::super::options::{HierarchyHandling, OrderingStrategy, PortConstraints, PortSide};
use super::super::random::JavaRandom;
use super::all_crossings_counter::count_all_crossings;

const RANDOM_AMOUNT: f32 = 0.07;

/// Normalize a side into the rightward frame (NORTH→WEST, SOUTH→EAST).
fn rightward(side: PortSide) -> PortSide {
    match side {
        PortSide::North => PortSide::West,
        PortSide::South => PortSide::East,
        s => s,
    }
}

/// `ELK_DBG=1` mirrors the elkjs instrumentation dump (SRC/PO/EDRAW/FILL/
/// ESW/ECR lines) so both sides' random-draw streams can be diffed
/// textually. Dev-only; zero draws of its own.
fn dbg_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("ELK_DBG").is_ok())
}

#[derive(Clone, Default)]
struct BaryState {
    summed_weight: f64,
    degree: i32,
    barycenter: Option<f64>,
    visited: bool,
}

/// A saved node+port order (Java `SweepCopy`, flat subset).
#[derive(Clone)]
struct SweepCopy {
    node_order: Vec<Vec<LNodeId>>,
    port_orders: Vec<Vec<Vec<LPortId>>>,
}

/// Per-graph state (Java `GraphInfoHolder`).
struct GraphInfo {
    lgraph: LGraphId,
    order: Vec<Vec<LNodeId>>,
    bary: Vec<Vec<BaryState>>,
    node_pos: Vec<Vec<usize>>,
    // Java declares these float[]; GWT (the oracle) stores JS doubles and
    // does double arithmetic on them, so the port mirrors that with f64.
    port_ranks: Vec<f64>,
    port_bary: Vec<f64>,
    node_relative: bool,
    parent_node: Option<LNodeId>,
    child_graph_ids: Vec<usize>,
    dont_sweep_into: bool,
    has_external_ports: bool,
    consider_model_order: bool,
    first_try: bool,
    second_try: bool,
    currently_best: Option<SweepCopy>,
    best: Option<SweepCopy>,
}

/// Public entry: run phase-3 crossing minimization on `graph` (and, if it
/// is a compound graph, its whole nesting subtree), threading the shared
/// `random` (post cycle-break state).
pub fn layer_sweep_crossing_minimizer(
    arena: &mut LGraphArena,
    graph: LGraphId,
    random: &mut JavaRandom,
    child_randoms: &mut std::collections::HashMap<LGraphId, JavaRandom>,
) {
    let hierarchical =
        arena.graphs[graph.0].props.hierarchy_handling == HierarchyHandling::IncludeChildren;
    let n_layers = arena.graphs[graph.0].layers.len();
    let empty = n_layers == 0 || arena.graphs[graph.0].layers.iter().all(|l| l.nodes.is_empty());
    let single_node = n_layers == 1 && arena.graphs[graph.0].layers[0].nodes.len() == 1;
    if empty || (single_node && !hierarchical) {
        return;
    }
    let mut orch = Orchestrator::initialize(arena, graph, random, child_randoms);
    orch.minimize_crossings();
    orch.transfer_node_and_port_orders_to_graph();
}

struct Orchestrator<'a> {
    arena: &'a mut LGraphArena,
    /// The root graph's random — orchestrator draws (per-try `set_seed` +
    /// direction booleans) and the root's own barycenter draws.
    random: &'a mut JavaRandom,
    /// Each nested graph's own random (post cycle-break `Random(1)`); the
    /// graph's barycenter jitters/fills/randomize draw from here. Left in
    /// the caller's map so later phases (P4/P5 run per graph bottom-up in
    /// ELK's hierarchical driver) see the post-phase-3 state.
    child_randoms: &'a mut std::collections::HashMap<LGraphId, JavaRandom>,
    graphs: Vec<GraphInfo>,
    to_sweep: Vec<usize>,
    changed: Vec<bool>,
    run_seed: i64,
}

impl<'a> Orchestrator<'a> {
    // -- debug dump helpers (ELK_DBG) ----------------------------------

    fn dbg_real_count(&self, gi: usize) -> usize {
        self.graphs[gi]
            .order
            .iter()
            .flatten()
            .filter(|&&n| self.arena.nodes[n.0].node_type == NodeType::Normal)
            .count()
    }

    fn dbg_node_name(&self, node: LNodeId) -> String {
        self.arena.nodes[node.0].props.origin.clone().unwrap_or_else(|| "·".into())
    }

    // -- initialize (BFS build) ---------------------------------------

    fn initialize(
        arena: &'a mut LGraphArena,
        root: LGraphId,
        random: &'a mut JavaRandom,
        child_randoms: &'a mut std::collections::HashMap<LGraphId, JavaRandom>,
    ) -> Self {
        if dbg_on() {
            let (hi, lo) = random.seed_hi_lo();
            eprintln!("INITSTATE seedhi={hi} seedlo={lo}");
        }
        let run_seed = random.next_long();
        if dbg_on() {
            // GWT long limbs (l: low 22 bits, m: mid 22, h: high 20) for
            // direct comparison with the instrumented elkjs dump.
            let v = run_seed as u64;
            eprintln!(
                "RUNSEED hi={} m={} l={}",
                (v >> 44) & 0xFFFFF,
                (v >> 22) & 0x3FFFFF,
                v & 0x3FFFFF
            );
        }

        let mut lgraphs: Vec<LGraphId> = vec![root];
        let mut graphs: Vec<GraphInfo> = Vec::new();
        let mut to_sweep: Vec<usize> = Vec::new();

        let mut i = 0;
        while i < lgraphs.len() {
            let g = lgraphs[i];
            arena.graphs[g.0].id = i;

            // currentNodeOrder + node.id (pos in layer) + dense port.id.
            let order: Vec<Vec<LNodeId>> =
                arena.graphs[g.0].layers.iter().map(|l| l.nodes.clone()).collect();
            for layer in &order {
                for (pos, &node) in layer.iter().enumerate() {
                    arena.nodes[node.0].id = pos;
                }
            }
            let mut n_ports = 0usize;
            for layer in &order {
                for &node in layer {
                    for &p in &arena.nodes[node.0].ports {
                        arena.ports[p.0].id = n_ports;
                        n_ports += 1;
                    }
                }
            }

            // Port distributor choice: `ISweepPortDistributor.create` draws
            // one `nextBoolean` from *this graph's own* random
            // (`getProperty(graph, RANDOM)`), which for the root is the
            // shared crossing-min random (post `nextLong`) and for a nested
            // graph is its own post-cycle-break `Random(1)`. Only the
            // sweeps below use the shared root random.
            let node_relative = if i == 0 {
                random.next_boolean()
            } else {
                child_randoms
                    .get_mut(&g)
                    .expect("nested graph missing its per-graph random")
                    .next_boolean()
            };

            // childGraphs in currentNodeOrder traversal order; append to
            // the BFS list so their graph.id = position here.
            let mut child_graph_ids: Vec<usize> = Vec::new();
            for layer in &order {
                for &node in layer {
                    if let Some(ng) = arena.nodes[node.0].nested_graph {
                        child_graph_ids.push(lgraphs.len());
                        lgraphs.push(ng);
                    }
                }
            }

            let dont_sweep_into = super::layer_sweep_type_decider::use_bottom_up(arena, g, false);
            let has_external_ports = arena.graphs[g.0].props.graph_properties.external_ports;
            let consider_model_order =
                arena.graphs[g.0].props.consider_model_order != OrderingStrategy::None;

            let bary = order.iter().map(|l| vec![BaryState::default(); l.len()]).collect();
            let node_pos = order.iter().map(|l| (0..l.len()).collect()).collect();

            graphs.push(GraphInfo {
                lgraph: g,
                order,
                bary,
                node_pos,
                port_ranks: vec![0.0; n_ports],
                port_bary: vec![0.0; n_ports],
                node_relative,
                parent_node: arena.graphs[g.0].parent_node,
                child_graph_ids,
                dont_sweep_into,
                has_external_ports,
                consider_model_order,
                first_try: false,
                // Both default false; `compare` sets `first_try` for model-order
                // graphs. For a no-model-order graph both stay false, so a
                // first-sweep unknown barycenter is *filled* (random) — which is
                // what group_1/group_3 need and what an earlier `second_try=true`
                // default wrongly suppressed. (Model-order graphs' `first_try`
                // dominates try 0, so this is invisible to stress-flat.)
                second_try: false,
                currently_best: None,
                best: None,
            });
            if dont_sweep_into {
                to_sweep.insert(0, i);
            }
            i += 1;
        }

        let changed = vec![false; graphs.len()];
        Orchestrator { arena, random, child_randoms, graphs, to_sweep, changed, run_seed }
    }

    // -- per-graph randoms (BarycenterHeuristic draws) ------------------

    /// `next_float` from graph `gi`'s own random (Java: the heuristic's
    /// `random` = the graph's `RANDOM` property; the root — BFS index 0 —
    /// shares the orchestrator random).
    fn next_float_for(&mut self, gi: usize) -> f32 {
        if gi == 0 {
            self.random.next_float()
        } else {
            self.child_randoms
                .get_mut(&self.graphs[gi].lgraph)
                .expect("nested graph missing its per-graph random")
                .next_float()
        }
    }

    /// `next_double` from graph `gi`'s own random (randomize barycenters).
    fn next_double_for(&mut self, gi: usize) -> f64 {
        if gi == 0 {
            self.random.next_double()
        } else {
            self.child_randoms
                .get_mut(&self.graphs[gi].lgraph)
                .expect("nested graph missing its per-graph random")
                .next_double()
        }
    }

    // -- orchestration ------------------------------------------------

    fn minimize_crossings(&mut self) {
        for k in 0..self.to_sweep.len() {
            let gi = self.to_sweep[k];
            if !self.graphs[gi].order.is_empty()
                && self.graphs[gi].order.iter().any(|l| !l.is_empty())
            {
                self.compare_different_randomized_layouts(gi);
                if self.graphs[gi].parent_node.is_some() {
                    self.set_port_order_on_parent_graph(gi);
                }
            }
        }
    }

    fn compare_different_randomized_layouts(&mut self, gi: usize) {
        self.random.set_seed(self.run_seed);
        for c in self.changed.iter_mut() {
            *c = false;
        }
        if self.graphs[gi].consider_model_order {
            self.graphs[gi].first_try = true;
        }
        let thoroughness = self.arena.graphs[self.graphs[gi].lgraph.0].props.thoroughness.max(1);
        let mut best_crossings = i32::MAX;
        for _ in 0..thoroughness {
            let crossings = self.minimize_crossings_with_counter(gi);
            if crossings < best_crossings {
                best_crossings = crossings;
                self.save_all_node_orders_of_changed_graphs();
                if best_crossings == 0 {
                    break;
                }
            }
        }
    }

    fn minimize_crossings_with_counter(&mut self, gi: usize) -> i32 {
        if dbg_on() {
            eprintln!(
                "MCWC real={} FT={} ST={}",
                self.dbg_real_count(gi),
                self.graphs[gi].first_try,
                self.graphs[gi].second_try
            );
        }
        let mut is_forward = self.random.next_boolean();

        let initial = self.count_current_number_of_crossings(gi);
        if initial == 0 && self.graphs[gi].first_try {
            return 0;
        }

        if (!self.graphs[gi].first_try && !self.graphs[gi].second_try)
            || !self.graphs[gi].consider_model_order
        {
            self.set_first_layer_order(gi, is_forward);
        } else {
            is_forward = self.graphs[gi].first_try;
        }
        self.sweep_reducing_crossings(gi, is_forward, true);
        // Post-first-sweep flag update. In ELK source this is two ifs over
        // FIRST_TRY / SECOND_TRY, but both `Property` constants share the id
        // string "firstTryWithInitialOrder" and `Property.equals/hashCode`
        // are id-based — they are ONE property-map entry, a single flag F.
        // The aliased net effect of `if(ST){ST=false} if(FT){FT=false;
        // ST=true}` is simply F=false (the second if never fires once the
        // first did). Verified against instrumented elkjs (try0 reads both
        // true, try1+ both false). We keep two fields but clear both, which
        // is state-machine-equivalent.
        self.graphs[gi].first_try = false;
        self.graphs[gi].second_try = false;

        let mut crossings = self.count_current_number_of_crossings(gi);
        if dbg_on() {
            eprintln!(
                "ESW fwd={is_forward} first=true cr={crossings} nc={}",
                self.random.draws()
            );
        }
        let mut old;
        loop {
            self.set_currently_best();
            if crossings == 0 {
                return 0;
            }
            is_forward = !is_forward;
            old = crossings;
            self.sweep_reducing_crossings(gi, is_forward, false);
            crossings = self.count_current_number_of_crossings(gi);
            if dbg_on() {
                eprintln!(
                    "ECR after fwd={is_forward} cr={crossings} nc={}",
                    self.random.draws()
                );
            }
            if old <= crossings {
                break;
            }
        }
        old
    }

    /// Java `countCurrentNumberOfCrossings`: this graph plus every child
    /// it sweeps into (i.e. not bottom-up).
    fn count_current_number_of_crossings(&self, gi: usize) -> i32 {
        let own = count_all_crossings(self.arena, &self.graphs[gi].order);
        if dbg_on() {
            eprintln!("CAC real={} total={own}", self.dbg_real_count(gi));
        }
        let mut total = own;
        for &ci in &self.graphs[gi].child_graph_ids {
            if !self.graphs[ci].dont_sweep_into {
                total += self.count_current_number_of_crossings(ci);
            }
        }
        total
    }

    fn sweep_reducing_crossings(&mut self, gi: usize, forward: bool, first_sweep: bool) {
        if dbg_on() {
            let (sh, sl) = self.random.seed_hi_lo();
            eprintln!(
                "SRC real={} fwd={forward} first={first_sweep} nc={} sh={sh} sl={sl}",
                self.dbg_real_count(gi),
                self.random.draws()
            );
        }
        let length = self.graphs[gi].order.len();
        let first = if forward { 0 } else { length - 1 };
        self.distribute_ports_while_sweeping(gi, first, forward);
        let first_layer = self.graphs[gi].order[first].clone();
        self.sweep_in_hierarchical_nodes(gi, &first_layer, forward, first_sweep);

        let range: Vec<usize> =
            if forward { (1..length).collect() } else { (0..length - 1).rev().collect() };
        let count_as_first =
            first_sweep && !self.graphs[gi].first_try && !self.graphs[gi].second_try;
        for idx in range {
            if dbg_on() {
                eprintln!(
                    "PO real={} fwd={forward} first={first_sweep} FT={} ST={} preOrdered={count_as_first}",
                    self.dbg_real_count(gi),
                    self.graphs[gi].first_try,
                    self.graphs[gi].second_try
                );
            }
            self.minimize_crossings_layer(gi, idx, forward, count_as_first);
            self.distribute_ports_while_sweeping(gi, idx, forward);
            let layer = self.graphs[gi].order[idx].clone();
            self.sweep_in_hierarchical_nodes(gi, &layer, forward, first_sweep);
        }
        self.changed[gi] = true;
    }

    // -- hierarchical dive --------------------------------------------

    fn sweep_in_hierarchical_nodes(
        &mut self,
        _gi: usize,
        layer: &[LNodeId],
        forward: bool,
        first_sweep: bool,
    ) {
        for &node in layer {
            if let Some(ng) = self.arena.nodes[node.0].nested_graph {
                let ci = self.arena.graphs[ng.0].id;
                if !self.graphs[ci].dont_sweep_into {
                    self.sweep_in_hierarchical_node(node, ci, forward, first_sweep);
                }
            }
        }
    }

    fn sweep_in_hierarchical_node(
        &mut self,
        parent_node: LNodeId,
        ci: usize,
        forward: bool,
        first_sweep: bool,
    ) {
        let len = self.graphs[ci].order.len();
        let start = if forward { 0 } else { len - 1 };
        let first_node = self.graphs[ci].order[start][0];
        if self.arena.nodes[first_node.0].node_type == NodeType::ExternalPort {
            let side = if forward { PortSide::West } else { PortSide::East };
            let sorted = self.sort_port_dummies_by_port_positions(parent_node, start, ci, side);
            self.graphs[ci].order[start] = sorted;
        } else {
            self.set_first_layer_order(ci, forward);
        }
        self.sweep_reducing_crossings(ci, forward, first_sweep);
        self.sort_ports_by_dummy_positions_in_last_layer(ci, parent_node, forward);
    }

    /// Java `sortPortDummiesByPortPositions`: order the child's entry
    /// layer's external-port dummies by the parent's ports on `side`
    /// (normalized), in `inNorthSouthEastWestOrder`.
    fn sort_port_dummies_by_port_positions(
        &self,
        parent_node: LNodeId,
        start_layer: usize,
        ci: usize,
        side: PortSide,
    ) -> Vec<LNodeId> {
        let ports = self.in_nsew_order(parent_node, side);
        let expected = self.graphs[ci].order[start_layer].len();
        let mut sorted: Vec<LNodeId> = Vec::with_capacity(expected);
        for p in ports {
            if self.arena.ports[p.0].props.inside_connections {
                if let Some(dummy) = self.arena.ports[p.0].props.port_dummy {
                    sorted.push(dummy);
                }
            }
        }
        assert_eq!(
            sorted.len(),
            expected,
            "expected {expected} hierarchical port dummies, found {}",
            sorted.len()
        );
        sorted
    }

    /// Java `sortPortsByDummyPositionsInLastLayer`: after sweeping a
    /// child, reorder the parent node's ports (on the exit side) to match
    /// the child's last-layer external-port dummy order.
    fn sort_ports_by_dummy_positions_in_last_layer(
        &mut self,
        ci: usize,
        parent: LNodeId,
        on_rightmost: bool,
    ) {
        let len = self.graphs[ci].order.len();
        let end = if on_rightmost { len - 1 } else { 0 };
        let last_layer = self.graphs[ci].order[end].clone();
        let mut j = if on_rightmost { 0i64 } else { last_layer.len() as i64 - 1 };
        let step: i64 = if on_rightmost { 1 } else { -1 };
        // Only proceed if the boundary node is an external-port dummy.
        let first = last_layer[if on_rightmost { 0 } else { last_layer.len() - 1 }];
        if self.arena.nodes[first.0].node_type != NodeType::ExternalPort {
            return;
        }
        let exit_side = if on_rightmost { PortSide::East } else { PortSide::West };
        let mut ports = self.arena.nodes[parent.0].ports.clone();
        let mut dbg_log: Vec<String> = Vec::new();
        for (i, slot) in ports.iter_mut().enumerate() {
            let p = *slot;
            if rightward(self.arena.ports[p.0].side) == exit_side
                && self.arena.ports[p.0].props.inside_connections
            {
                let dummy = last_layer[j as usize];
                if let Some(origin) = self.arena.nodes[dummy.0].props.origin_port {
                    if dbg_on() {
                        let tgts: Vec<String> = self.arena.ports[origin.0]
                            .incoming_edges
                            .iter()
                            .chain(self.arena.ports[origin.0].outgoing_edges.iter())
                            .map(|&e| {
                                let other = if self.arena.edges[e.0].source == Some(origin) {
                                    self.arena.edges[e.0].target.unwrap()
                                } else {
                                    self.arena.edges[e.0].source.unwrap()
                                };
                                let on = self.arena.ports[other.0].owner.unwrap();
                                self.arena.nodes[on.0]
                                    .props
                                    .origin
                                    .clone()
                                    .unwrap_or_else(|| format!("T{:?}", self.arena.nodes[on.0].node_type))
                            })
                            .collect();
                        dbg_log.push(format!("slot{i}<-j{j}({})", tgts.join("+")));
                    }
                    *slot = origin;
                }
                j += step;
            }
        }
        if dbg_on() && !dbg_log.is_empty() {
            let pname = self.dbg_node_name(parent);
            eprintln!("SPBD parent={pname} right={on_rightmost} {}", dbg_log.join(" "));
        }
        self.arena.nodes[parent.0].ports = ports;
    }

    fn set_port_order_on_parent_graph(&mut self, gi: usize) {
        if self.graphs[gi].has_external_ports && self.graphs[gi].best.is_some() {
            let best = self.graphs[gi].best.clone().unwrap();
            let parent = self.graphs[gi].parent_node.unwrap();
            // Restore the best node order onto the child's `order` so the
            // parent-port sort reads the chosen layout.
            self.graphs[gi].order = best.node_order.clone();
            self.sort_ports_by_dummy_positions_in_last_layer(gi, parent, true);
            self.sort_ports_by_dummy_positions_in_last_layer(gi, parent, false);
            self.arena.nodes[parent.0].props.port_constraints = PortConstraints::FixedOrder;
        }
    }

    /// `CrossMinUtil.inNorthSouthEastWestOrder` with local normalization:
    /// ports of `node` on the normalized `side`; EAST/NORTH keep list
    /// order, SOUTH/WEST are reversed.
    fn in_nsew_order(&self, node: LNodeId, side: PortSide) -> Vec<LPortId> {
        let mut ports: Vec<LPortId> = self.arena.nodes[node.0]
            .ports
            .iter()
            .copied()
            .filter(|&p| rightward(self.arena.ports[p.0].side) == side)
            .collect();
        // Java iterates EAST/NORTH in list order and SOUTH/WEST reversed —
        // judged on the *rotated* side, since ELK sorts after its import
        // rotation. In the normalized frame that means: reverse WEST
        // (original NORTH or WEST), keep EAST (original SOUTH or EAST).
        if matches!(side, PortSide::South | PortSide::West) {
            ports.reverse();
        }
        ports
    }

    // -- SweepCopy save/restore ---------------------------------------

    fn snapshot(&self, gi: usize) -> SweepCopy {
        let node_order = self.graphs[gi].order.clone();
        let port_orders = self.graphs[gi]
            .order
            .iter()
            .map(|layer| layer.iter().map(|&n| self.arena.nodes[n.0].ports.clone()).collect())
            .collect();
        SweepCopy { node_order, port_orders }
    }

    fn set_currently_best(&mut self) {
        for gi in 0..self.graphs.len() {
            if self.changed[gi] {
                self.graphs[gi].currently_best = Some(self.snapshot(gi));
            }
        }
    }

    fn save_all_node_orders_of_changed_graphs(&mut self) {
        for gi in 0..self.graphs.len() {
            if self.changed[gi] {
                self.graphs[gi].best = self.graphs[gi].currently_best.clone();
            }
        }
    }

    fn transfer_node_and_port_orders_to_graph(&mut self) {
        for gi in 0..self.graphs.len() {
            let Some(best) = self.graphs[gi].best.clone() else {
                continue;
            };
            let g = self.graphs[gi].lgraph;
            for i in 0..best.node_order.len() {
                for (j, &node) in best.node_order[i].iter().enumerate() {
                    self.arena.nodes[node.0].id = j;
                    self.arena.nodes[node.0].ports = best.port_orders[i][j].clone();
                    if !self.arena.nodes[node.0].props.port_constraints.is_order_fixed() {
                        self.arena.nodes[node.0].props.port_constraints = PortConstraints::FixedOrder;
                    }
                }
                self.arena.graphs[g.0].layers[i].nodes = best.node_order[i].clone();
            }
        }
    }

    // -- BarycenterHeuristic (per graph) ------------------------------

    fn minimize_crossings_layer(&mut self, gi: usize, free_index: usize, forward: bool, is_first_sweep: bool) {
        let length = self.graphs[gi].order.len();
        let is_first_layer = free_index == if forward { 0 } else { length - 1 };
        if !is_first_layer {
            let fixed = self.graphs[gi].order[if forward { free_index - 1 } else { free_index + 1 }].clone();
            self.calculate_port_ranks(gi, &fixed, forward);
        }
        // `preOrdered = !isFirstSweep || isExternalPortDummy(order[i][0])`
        // (BarycenterHeuristic.minimizeCrossings): a free layer led by an
        // external-port dummy — a compound boundary layer — is always
        // treated as pre-ordered, even on a first sweep.
        let first_node = self.graphs[gi].order[free_index][0];
        let pre_ordered = !is_first_sweep
            || self.arena.nodes[first_node.0].node_type == NodeType::ExternalPort;
        let mut nodes = self.graphs[gi].order[free_index].clone();
        self.minimize_crossings_nodes(gi, &mut nodes, pre_ordered, false, forward);
        self.graphs[gi].order[free_index] = nodes;
        if dbg_on() {
            let row: Vec<String> = self.graphs[gi].order[free_index]
                .iter()
                .map(|&n| {
                    let b = self
                        .state(gi, n)
                        .barycenter
                        .map(|v| format!("{v:.8}"))
                        .unwrap_or_else(|| "null".into());
                    format!("{}:{b}", self.dbg_node_name(n))
                })
                .collect();
            eprintln!("MINL i={free_index} po={pre_ordered} [{}]", row.join(" "));
        }
    }

    fn set_first_layer_order(&mut self, gi: usize, forward: bool) {
        let length = self.graphs[gi].order.len();
        let start = if forward { 0 } else { length.saturating_sub(1) };
        let mut nodes = self.graphs[gi].order[start].clone();
        self.minimize_crossings_nodes(gi, &mut nodes, false, true, forward);
        self.graphs[gi].order[start] = nodes;
    }

    fn minimize_crossings_nodes(
        &mut self,
        gi: usize,
        nodes: &mut [LNodeId],
        pre_ordered: bool,
        randomize: bool,
        forward: bool,
    ) {
        if randomize {
            for &node in nodes.iter() {
                let b = self.next_double_for(gi);
                let st = self.state_mut(gi, node);
                st.barycenter = Some(b);
                st.summed_weight = b;
                st.degree = 1;
            }
        } else {
            self.calculate_barycenters(gi, nodes, forward);
            self.fill_in_unknown_barycenters(gi, nodes, pre_ordered);
        }
        if nodes.len() > 1 {
            nodes.sort_by(|&a, &b| {
                match (self.state(gi, a).barycenter, self.state(gi, b).barycenter) {
                    (Some(x), Some(y)) => x.total_cmp(&y),
                    (Some(_), None) => std::cmp::Ordering::Less,
                    (None, Some(_)) => std::cmp::Ordering::Greater,
                    (None, None) => std::cmp::Ordering::Equal,
                }
            });
        }
    }

    fn calculate_barycenters(&mut self, gi: usize, nodes: &[LNodeId], forward: bool) {
        for &node in nodes {
            self.state_mut(gi, node).visited = false;
        }
        for &node in nodes {
            self.calculate_barycenter(gi, node, forward);
        }
    }

    fn calculate_barycenter(&mut self, gi: usize, node: LNodeId, forward: bool) {
        if self.state(gi, node).visited {
            return;
        }
        self.state_mut(gi, node).visited = true;
        {
            let st = self.state_mut(gi, node);
            st.degree = 0;
            st.summed_weight = 0.0;
            st.barycenter = None;
        }
        let node_layer = self.arena.nodes[node.0].layer;
        let mut fixed: Vec<(usize, LNodeId, Option<usize>)> = Vec::new();
        for &port in &self.arena.nodes[node.0].ports {
            let edges = if forward {
                &self.arena.ports[port.0].incoming_edges
            } else {
                &self.arena.ports[port.0].outgoing_edges
            };
            for &edge in edges {
                let fixed_port = if forward {
                    self.arena.edges[edge.0].source.unwrap()
                } else {
                    self.arena.edges[edge.0].target.unwrap()
                };
                let fixed_node = self.arena.ports[fixed_port.0].owner.unwrap();
                let fl = self.arena.nodes[fixed_node.0].layer;
                fixed.push((self.arena.ports[fixed_port.0].id, fixed_node, fl));
            }
        }
        for (fixed_port_id, fixed_node, fl) in fixed {
            if fl == node_layer {
                if fixed_node != node {
                    self.calculate_barycenter(gi, fixed_node, forward);
                    let (d, w) = {
                        let s = self.state(gi, fixed_node);
                        (s.degree, s.summed_weight)
                    };
                    let st = self.state_mut(gi, node);
                    st.degree += d;
                    st.summed_weight += w;
                }
            } else {
                let rank = self.graphs[gi].port_ranks[fixed_port_id] as f64;
                let st = self.state_mut(gi, node);
                st.summed_weight += rank;
                st.degree += 1;
            }
        }
        if self.state(gi, node).degree > 0 {
            if dbg_on() {
                eprintln!("EDRAW jitter node={}", self.dbg_node_name(node));
            }
            // GWT (the oracle) evaluates Java's float expression
            // `nextFloat() * RANDOM_AMOUNT - RANDOM_AMOUNT / 2` in double
            // precision with the f32-rounded literals — mirror that, not
            // Java's f32 arithmetic (differs at ~1e-8, enough to flip a
            // near-tie sort).
            let jitter = self.next_float_for(gi) as f64 * RANDOM_AMOUNT as f64
                - (RANDOM_AMOUNT / 2.0) as f64;
            let st = self.state_mut(gi, node);
            st.summed_weight += jitter;
            st.barycenter = Some(st.summed_weight / st.degree as f64);
        }
    }

    #[allow(clippy::needless_range_loop)]
    fn fill_in_unknown_barycenters(&mut self, gi: usize, nodes: &[LNodeId], pre_ordered: bool) {
        if pre_ordered {
            let mut last_value = -1.0f64;
            for idx in 0..nodes.len() {
                let value = self.state(gi, nodes[idx]).barycenter;
                let value = if let Some(v) = value {
                    v
                } else {
                    let mut next_value = last_value + 1.0;
                    for k in (idx + 1)..nodes.len() {
                        if let Some(x) = self.state(gi, nodes[k]).barycenter {
                            next_value = x;
                            break;
                        }
                    }
                    let v = (last_value + next_value) / 2.0;
                    let st = self.state_mut(gi, nodes[idx]);
                    st.barycenter = Some(v);
                    st.summed_weight = v;
                    st.degree = 1;
                    v
                };
                last_value = value;
            }
        } else {
            let mut max_bary = 0.0f64;
            for &node in nodes {
                if let Some(b) = self.state(gi, node).barycenter {
                    max_bary = max_bary.max(b);
                }
            }
            max_bary += 2.0;
            for &node in nodes {
                if self.state(gi, node).barycenter.is_none() {
                    if dbg_on() {
                        eprintln!("FILL node={}", self.dbg_node_name(node));
                    }
                    let value = self.next_float_for(gi) as f64 * max_bary - 1.0;
                    let st = self.state_mut(gi, node);
                    st.barycenter = Some(value);
                    st.summed_weight = value;
                    st.degree = 1;
                }
            }
        }
    }

    fn state(&self, gi: usize, node: LNodeId) -> &BaryState {
        let l = self.arena.nodes[node.0].layer.unwrap();
        let id = self.arena.nodes[node.0].id;
        &self.graphs[gi].bary[l][id]
    }
    fn state_mut(&mut self, gi: usize, node: LNodeId) -> &mut BaryState {
        let l = self.arena.nodes[node.0].layer.unwrap();
        let id = self.arena.nodes[node.0].id;
        &mut self.graphs[gi].bary[l][id]
    }

    // -- port distribution (per graph) --------------------------------

    fn distribute_ports_while_sweeping(&mut self, gi: usize, current_index: usize, forward: bool) {
        self.update_node_positions(gi, current_index);
        let free_layer = self.graphs[gi].order[current_index].clone();
        let side = if forward { PortSide::West } else { PortSide::East };
        let length = self.graphs[gi].order.len();
        let not_first = if forward { current_index != 0 } else { current_index != length - 1 };
        if not_first {
            let fixed_layer =
                self.graphs[gi].order[if forward { current_index - 1 } else { current_index + 1 }].clone();
            self.calculate_port_ranks(gi, &fixed_layer, forward);
            for &node in &free_layer {
                self.distribute_ports(gi, node, side);
            }
            self.calculate_port_ranks(gi, &free_layer, !forward);
            for &node in &fixed_layer {
                // Java: `if (!hasNestedGraph(node))` — a hierarchical node's
                // port order on the away side was just set by the dive
                // (`sortPortsByDummyPositionsInLastLayer`) and must not be
                // re-sorted from this side.
                if self.arena.nodes[node.0].nested_graph.is_none() {
                    self.distribute_ports(gi, node, side.opposed());
                }
            }
        } else {
            for &node in &free_layer {
                self.distribute_ports(gi, node, side);
            }
        }
    }

    fn update_node_positions(&mut self, gi: usize, current_index: usize) {
        let layer = self.graphs[gi].order[current_index].clone();
        for (i, node) in layer.into_iter().enumerate() {
            let l = self.arena.nodes[node.0].layer.unwrap();
            let id = self.arena.nodes[node.0].id;
            self.graphs[gi].node_pos[l][id] = i;
        }
    }

    fn calculate_port_ranks(&mut self, gi: usize, layer: &[LNodeId], output: bool) {
        let mut consumed = 0.0f64;
        for &node in layer {
            consumed += self.calculate_port_ranks_node(gi, node, consumed, output);
        }
    }

    fn calculate_port_ranks_node(&mut self, gi: usize, node: LNodeId, rank_sum: f64, output: bool) -> f64 {
        let ports: Vec<LPortId> = self.arena.nodes[node.0]
            .ports
            .iter()
            .copied()
            .filter(|&p| {
                if output {
                    !self.arena.ports[p.0].outgoing_edges.is_empty()
                } else {
                    !self.arena.ports[p.0].incoming_edges.is_empty()
                }
            })
            .collect();
        if self.graphs[gi].node_relative {
            let incr = 1.0 / (ports.len() as f64 + 1.0);
            if output {
                let mut pos = rank_sum + incr;
                for p in ports {
                    self.graphs[gi].port_ranks[self.arena.ports[p.0].id] = pos;
                    pos += incr;
                }
            } else {
                let mut rest_pos = rank_sum + 1.0 - incr;
                for p in ports {
                    self.graphs[gi].port_ranks[self.arena.ports[p.0].id] = rest_pos;
                    rest_pos -= incr;
                }
            }
            1.0
        } else if output {
            let mut pos = 0.0f64;
            for p in ports {
                pos += 1.0;
                self.graphs[gi].port_ranks[self.arena.ports[p.0].id] = rank_sum + pos;
            }
            pos
        } else {
            let input_count = ports.len() as f64;
            let mut rest_pos = rank_sum + input_count;
            for p in ports {
                self.graphs[gi].port_ranks[self.arena.ports[p.0].id] = rest_pos;
                rest_pos -= 1.0;
            }
            input_count
        }
    }

    /// Java `distributePorts(node, side)` (node level). The Java body also
    /// distributes the literal SOUTH and NORTH port groups (north/south
    /// port dummy handling): in the oracle's rotated frame those groups
    /// are empty for every node in scope — our literal N/S group ports
    /// *are* the normalized E/W ones handled here — so the two extra
    /// calls are intentionally absent.
    fn distribute_ports(&mut self, gi: usize, node: LNodeId, side: PortSide) {
        if self.arena.nodes[node.0].props.port_constraints.is_order_fixed() {
            return;
        }
        let side_ports: Vec<LPortId> = self.arena.nodes[node.0]
            .ports
            .iter()
            .copied()
            .filter(|&p| rightward(self.arena.ports[p.0].side) == side)
            .collect();
        self.distribute_port_group(gi, node, &side_ports);
        self.sort_ports_of_node(gi, node);
    }

    /// Java `distributePorts(node, ports)` (port-group level):
    /// barycenters for the group, then extreme values for ports with
    /// in-layer connections.
    fn distribute_port_group(&mut self, gi: usize, node: LNodeId, ports: &[LPortId]) {
        let (in_layer_ports, min_bary, max_bary) =
            self.iterate_ports_and_collect_in_layer_ports(gi, node, ports);
        if !in_layer_ports.is_empty() {
            self.calculate_in_layer_ports_barycenter_values(
                gi,
                node,
                &in_layer_ports,
                min_bary,
                max_bary,
            );
        }
    }

    /// Java `iteratePortsAndCollectInLayerPorts`: sum connected port
    /// ranks; a port with *any* in-layer edge is skipped entirely and
    /// collected instead. The `northSouthPort` branch never fires here:
    /// it exists for genuine NORTH_SOUTH_PORT dummies (out of scope), and
    /// our literal-N/S group ports are normalized E/W (in the rotated
    /// oracle frame they *are* E/W) — checking the raw side would wrongly
    /// route them through `PORT_DUMMY`.
    fn iterate_ports_and_collect_in_layer_ports(
        &mut self,
        gi: usize,
        node: LNodeId,
        ports: &[LPortId],
    ) -> (Vec<LPortId>, f64, f64) {
        let mut in_layer_ports: Vec<LPortId> = Vec::new();
        let mut min_barycenter = 0.0f64;
        let mut max_barycenter = 0.0f64;
        let node_layer = self.arena.nodes[node.0].layer;
        'port_iteration: for &port in ports {
            let mut sum = 0.0f64;
            for &out_edge in &self.arena.ports[port.0].outgoing_edges.clone() {
                let connected = self.arena.edges[out_edge.0].target.unwrap();
                let cnode = self.arena.ports[connected.0].owner.unwrap();
                if self.arena.nodes[cnode.0].layer == node_layer {
                    in_layer_ports.push(port);
                    continue 'port_iteration;
                }
                sum += self.graphs[gi].port_ranks[self.arena.ports[connected.0].id];
            }
            for &in_edge in &self.arena.ports[port.0].incoming_edges.clone() {
                let connected = self.arena.edges[in_edge.0].source.unwrap();
                let cnode = self.arena.ports[connected.0].owner.unwrap();
                if self.arena.nodes[cnode.0].layer == node_layer {
                    in_layer_ports.push(port);
                    continue 'port_iteration;
                }
                sum -= self.graphs[gi].port_ranks[self.arena.ports[connected.0].id];
            }
            let degree = (self.arena.ports[port.0].incoming_edges.len()
                + self.arena.ports[port.0].outgoing_edges.len()) as i32;
            if degree > 0 {
                let bary = sum / degree as f64;
                self.graphs[gi].port_bary[self.arena.ports[port.0].id] = bary;
                min_barycenter = min_barycenter.min(bary);
                max_barycenter = max_barycenter.max(bary);
            }
        }
        (in_layer_ports, min_barycenter, max_barycenter)
    }

    /// Java `calculateInLayerPortsBarycenterValues`: an in-layer port's
    /// barycenter is pushed past the group's min/max so it sorts to the
    /// top or bottom of its side, depending on whether its in-layer
    /// neighbors sit above or below the node. Sides in the normalized
    /// rightward frame.
    fn calculate_in_layer_ports_barycenter_values(
        &mut self,
        gi: usize,
        node: LNodeId,
        in_layer_ports: &[LPortId],
        min_barycenter: f64,
        max_barycenter: f64,
    ) {
        let node_layer = self.arena.nodes[node.0].layer;
        let l = node_layer.unwrap();
        let node_index_in_layer = self.graphs[gi].node_pos[l][self.arena.nodes[node.0].id] as f64 + 1.0;
        let layer_size = self.graphs[gi].order[l].len() as f64 + 1.0;
        for &in_layer_port in in_layer_ports {
            let mut sum = 0i32;
            let mut in_layer_connections = 0i32;
            let connected: Vec<LPortId> = self.arena.ports[in_layer_port.0]
                .incoming_edges
                .iter()
                .map(|&e| self.arena.edges[e.0].source.unwrap())
                .chain(
                    self.arena.ports[in_layer_port.0]
                        .outgoing_edges
                        .iter()
                        .map(|&e| self.arena.edges[e.0].target.unwrap()),
                )
                .collect();
            for cport in connected {
                let cnode = self.arena.ports[cport.0].owner.unwrap();
                if self.arena.nodes[cnode.0].layer == node_layer {
                    let cl = self.arena.nodes[cnode.0].layer.unwrap();
                    sum += self.graphs[gi].node_pos[cl][self.arena.nodes[cnode.0].id] as i32 + 1;
                    in_layer_connections += 1;
                }
            }
            let barycenter = sum as f64 / in_layer_connections as f64;
            let pid = self.arena.ports[in_layer_port.0].id;
            match rightward(self.arena.ports[in_layer_port.0].side) {
                PortSide::East => {
                    self.graphs[gi].port_bary[pid] = if barycenter < node_index_in_layer {
                        // take a low value in order to have the port above
                        min_barycenter - barycenter
                    } else {
                        max_barycenter + (layer_size - barycenter)
                    };
                }
                PortSide::West => {
                    self.graphs[gi].port_bary[pid] = if barycenter < node_index_in_layer {
                        max_barycenter + barycenter
                    } else {
                        min_barycenter - (layer_size - barycenter)
                    };
                }
                _ => {}
            }
        }
    }

    fn sort_ports_of_node(&mut self, gi: usize, node: LNodeId) {
        let mut ports = self.arena.nodes[node.0].ports.clone();
        ports.sort_by(|&p1, &p2| {
            let s1 = rightward(self.arena.ports[p1.0].side);
            let s2 = rightward(self.arena.ports[p2.0].side);
            if s1 != s2 {
                return (s1 as i32).cmp(&(s2 as i32));
            }
            let b1 = self.graphs[gi].port_bary[self.arena.ports[p1.0].id];
            let b2 = self.graphs[gi].port_bary[self.arena.ports[p2.0].id];
            if b1 == 0.0 && b2 == 0.0 {
                std::cmp::Ordering::Equal
            } else if b1 == 0.0 {
                std::cmp::Ordering::Less
            } else if b2 == 0.0 {
                std::cmp::Ordering::Greater
            } else {
                b1.total_cmp(&b2)
            }
        });
        self.arena.nodes[node.0].ports = ports;
    }
}
