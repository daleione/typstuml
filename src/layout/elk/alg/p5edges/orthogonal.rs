//! Port of `org.eclipse.elk.alg.layered.p5edges.orthogonal` — the
//! orthogonal routing generator that assigns each layer gap its routing
//! slots (which drive the layer x-positions) and computes edge bend
//! points. EPL-2.0.
//!
//! Collapses `OrthogonalRoutingGenerator` + `HyperEdgeSegment` +
//! `HyperEdgeSegmentDependency` + `HyperEdgeCycleDetector` + the
//! WEST_TO_EAST direction strategy into index-based structures over the
//! arena. Segments/dependencies reference each other by index (Rust
//! ownership); dependency add/remove/reverse maintain the segment
//! incidence lists.
//!
//! Scope: WEST_TO_EAST only (flat/architecture, internal rightward).
//! Cycle breaking (incl. the `HyperEdgeSegmentSplitter` for critical
//! cycles) uses the shared `JavaRandom`, continuing the stream from
//! crossing minimization.

use std::collections::BTreeSet;

use super::super::graph::{LGraphArena, LNodeId, LPortId, NodeType};
use super::super::options::PortSide;
use super::super::random::JavaRandom;

/// Normalize a side into the rightward frame (NORTH→WEST, SOUTH→EAST).
fn rightward(side: PortSide) -> PortSide {
    match side {
        PortSide::North => PortSide::West,
        PortSide::South => PortSide::East,
        s => s,
    }
}

const TOLERANCE: f64 = 1e-3;
const CRITICAL_CONFLICTS_DETECTED: i32 = -1;
const CONFLICT_THRESHOLD_FACTOR: f64 = 0.5;
const CRITICAL_CONFLICT_THRESHOLD_FACTOR: f64 = 0.2;
const CONFLICT_PENALTY: i32 = 1;
const CROSSING_PENALTY: i32 = 16;

#[derive(Clone, Copy, PartialEq, Eq)]
enum DepType {
    Regular,
    Critical,
}

struct Dependency {
    source: Option<usize>,
    target: Option<usize>,
    weight: i32,
    dep_type: DepType,
}

#[derive(Default)]
struct Segment {
    ports: Vec<LPortId>,
    mark: i32,
    routing_slot: i32,
    start: f64, // NaN = unset
    end: f64,
    incoming: Vec<f64>, // sorted
    outgoing: Vec<f64>, // sorted
    out_deps: Vec<usize>,
    in_deps: Vec<usize>,
    in_weight: i32,
    out_weight: i32,
    critical_in_weight: i32,
    critical_out_weight: i32,
    split_partner: Option<usize>,
    split_by: Option<usize>,
}

impl Segment {
    fn new() -> Segment {
        Segment { start: f64::NAN, end: f64::NAN, ..Default::default() }
    }
    fn length(&self) -> f64 {
        self.end - self.start
    }
    fn represents_hyperedge(&self) -> bool {
        self.incoming.len() + self.outgoing.len() > 2
    }
    fn is_dummy(&self) -> bool {
        self.split_partner.is_some() && self.split_by.is_none()
    }
    fn recompute_extent(&mut self) {
        self.start = f64::NAN;
        self.end = f64::NAN;
        for list in [&self.incoming, &self.outgoing] {
            if let (Some(&first), Some(&last)) = (list.first(), list.last()) {
                self.start = if self.start.is_nan() { first } else { self.start.min(first) };
                self.end = if self.end.is_nan() { last } else { self.end.max(last) };
            }
        }
    }
}

/// The routing generator: one instance per layer gap (per ELK).
pub struct RoutingGen<'a> {
    arena: &'a mut LGraphArena,
    edge_spacing: f64,
    conflict_threshold: f64,
    critical_conflict_threshold: f64,
    segments: Vec<Segment>,
    deps: Vec<Dependency>,
}

impl<'a> RoutingGen<'a> {
    pub fn new(arena: &'a mut LGraphArena, edge_spacing: f64) -> Self {
        RoutingGen {
            arena,
            edge_spacing,
            conflict_threshold: CONFLICT_THRESHOLD_FACTOR * edge_spacing,
            critical_conflict_threshold: 0.0,
            segments: Vec::new(),
            deps: Vec::new(),
        }
    }

    // -- port coord (WEST_TO_EAST strategy) --

    fn port_pos_on_hypernode(&self, p: LPortId) -> f64 {
        let n = self.arena.ports[p.0].owner.unwrap();
        self.arena.nodes[n.0].position.y
            + self.arena.ports[p.0].position.y
            + self.arena.ports[p.0].anchor.y
    }
    fn absolute_anchor_y(&self, p: LPortId) -> f64 {
        let n = self.arena.ports[p.0].owner.unwrap();
        self.arena.nodes[n.0].position.y
            + self.arena.ports[p.0].position.y
            + self.arena.ports[p.0].anchor.y
    }

    // -- segment creation --

    fn create_hyper_edge_segments(
        &mut self,
        layer: Option<&[LNodeId]>,
        port_side: PortSide,
        port_to_seg: &mut std::collections::HashMap<LPortId, usize>,
    ) {
        let Some(nodes) = layer else { return };
        for &node in nodes {
            // node.getPorts(OUTPUT, portSide): ports on `port_side` with outgoing edges.
            let ports: Vec<LPortId> = self.arena.nodes[node.0]
                .ports
                .iter()
                .copied()
                .filter(|&p| {
                    // Normalized rightward frame (NORTH→WEST, SOUTH→EAST):
                    // group-node ports keep N/S because this port skips
                    // ELK's import rotation.
                    rightward(self.arena.ports[p.0].side) == port_side
                        && !self.arena.ports[p.0].outgoing_edges.is_empty()
                })
                .collect();
            for p in ports {
                if !port_to_seg.contains_key(&p) {
                    let seg = self.segments.len();
                    self.segments.push(Segment::new());
                    self.add_port_positions(seg, p, port_to_seg);
                }
            }
        }
    }

    fn add_port_positions(
        &mut self,
        seg: usize,
        port: LPortId,
        port_to_seg: &mut std::collections::HashMap<LPortId, usize>,
    ) {
        port_to_seg.insert(port, seg);
        self.segments[seg].ports.push(port);
        let pos = self.port_pos_on_hypernode(port);
        // getSourcePortSide = EAST → incoming; else outgoing.
        if rightward(self.arena.ports[port.0].side) == PortSide::East {
            insert_sorted(&mut self.segments[seg].incoming, pos);
        } else {
            insert_sorted(&mut self.segments[seg].outgoing, pos);
        }
        self.segments[seg].recompute_extent();
        // connected ports (both directions).
        let connected = self.connected_ports(port);
        for other in connected {
            if !port_to_seg.contains_key(&other) {
                self.add_port_positions(seg, other, port_to_seg);
            }
        }
    }

    fn connected_ports(&self, port: LPortId) -> Vec<LPortId> {
        // Java LPort.getConnectedPorts(): incoming sources then outgoing targets.
        let mut out = Vec::new();
        for &e in &self.arena.ports[port.0].incoming_edges {
            out.push(self.arena.edges[e.0].source.unwrap());
        }
        for &e in &self.arena.ports[port.0].outgoing_edges {
            out.push(self.arena.edges[e.0].target.unwrap());
        }
        out
    }

    // -- dependency management --

    fn add_dep(&mut self, source: usize, target: usize, weight: i32, dep_type: DepType) {
        let di = self.deps.len();
        self.deps.push(Dependency { source: Some(source), target: Some(target), weight, dep_type });
        self.segments[source].out_deps.push(di);
        self.segments[target].in_deps.push(di);
    }
    fn remove_dep(&mut self, di: usize) {
        if let Some(s) = self.deps[di].source.take() {
            self.segments[s].out_deps.retain(|&x| x != di);
        }
        if let Some(t) = self.deps[di].target.take() {
            self.segments[t].in_deps.retain(|&x| x != di);
        }
    }
    fn reverse_dep(&mut self, di: usize) {
        let (s, t) = (self.deps[di].source, self.deps[di].target);
        if let Some(s) = s {
            self.segments[s].out_deps.retain(|&x| x != di);
        }
        if let Some(t) = t {
            self.segments[t].in_deps.retain(|&x| x != di);
        }
        self.deps[di].source = t;
        self.deps[di].target = s;
        if let Some(ns) = self.deps[di].source {
            self.segments[ns].out_deps.push(di);
        }
        if let Some(nt) = self.deps[di].target {
            self.segments[nt].in_deps.push(di);
        }
    }

    // -- public routeEdges --

    pub fn route_edges(
        &mut self,
        source_layer: Option<&[LNodeId]>,
        target_layer: Option<&[LNodeId]>,
        start_pos: f64,
        random: &mut JavaRandom,
    ) -> i32 {
        let mut port_to_seg = std::collections::HashMap::new();
        self.create_hyper_edge_segments(source_layer, PortSide::East, &mut port_to_seg);
        self.create_hyper_edge_segments(target_layer, PortSide::West, &mut port_to_seg);

        self.critical_conflict_threshold =
            CRITICAL_CONFLICT_THRESHOLD_FACTOR * self.minimum_horizontal_segment_distance();

        let mut critical_dep_count = 0;
        for first in 0..self.segments.len().saturating_sub(1) {
            for second in (first + 1)..self.segments.len() {
                critical_dep_count += self.create_dependency_if_necessary(first, second);
            }
        }

        if critical_dep_count >= 2 {
            self.break_critical_cycles(random);
        }
        self.break_non_critical_cycles(random);
        self.topological_numbering();

        // bend points + slots count
        let mut rank_count = -1;
        let seg_indices: Vec<usize> = (0..self.segments.len()).collect();
        for si in seg_indices {
            if (self.segments[si].start - self.segments[si].end).abs() < TOLERANCE {
                continue;
            }
            rank_count = rank_count.max(self.segments[si].routing_slot);
            self.calculate_bend_points(si, start_pos);
        }
        rank_count + 1
    }

    fn minimum_horizontal_segment_distance(&self) -> f64 {
        let mut incoming: Vec<f64> = Vec::new();
        let mut outgoing: Vec<f64> = Vec::new();
        for s in &self.segments {
            incoming.extend_from_slice(&s.incoming);
            outgoing.extend_from_slice(&s.outgoing);
        }
        minimum_difference(&incoming).min(minimum_difference(&outgoing))
    }

    fn create_dependency_if_necessary(&mut self, i1: usize, i2: usize) -> i32 {
        if (self.segments[i1].start - self.segments[i1].end).abs() < TOLERANCE
            || (self.segments[i2].start - self.segments[i2].end).abs() < TOLERANCE
        {
            return 0;
        }
        let conflicts1 = self.count_conflicts(&self.segments[i1].outgoing, &self.segments[i2].incoming);
        let conflicts2 = self.count_conflicts(&self.segments[i2].outgoing, &self.segments[i1].incoming);
        let critical = conflicts1 == CRITICAL_CONFLICTS_DETECTED || conflicts2 == CRITICAL_CONFLICTS_DETECTED;
        let mut critical_count = 0;
        if critical {
            if conflicts1 == CRITICAL_CONFLICTS_DETECTED {
                self.add_dep(i2, i1, 1, DepType::Critical);
                critical_count += 1;
            }
            if conflicts2 == CRITICAL_CONFLICTS_DETECTED {
                self.add_dep(i1, i2, 1, DepType::Critical);
                critical_count += 1;
            }
        } else {
            let mut crossings1 =
                count_crossings(&self.segments[i1].outgoing, self.segments[i2].start, self.segments[i2].end);
            crossings1 +=
                count_crossings(&self.segments[i2].incoming, self.segments[i1].start, self.segments[i1].end);
            let mut crossings2 =
                count_crossings(&self.segments[i2].outgoing, self.segments[i1].start, self.segments[i1].end);
            crossings2 +=
                count_crossings(&self.segments[i1].incoming, self.segments[i2].start, self.segments[i2].end);
            let dv1 = CONFLICT_PENALTY * conflicts1 + CROSSING_PENALTY * crossings1;
            let dv2 = CONFLICT_PENALTY * conflicts2 + CROSSING_PENALTY * crossings2;
            if dv1 < dv2 {
                self.add_dep(i1, i2, dv2 - dv1, DepType::Regular);
            } else if dv1 > dv2 {
                self.add_dep(i2, i1, dv1 - dv2, DepType::Regular);
            } else if dv1 > 0 && dv2 > 0 {
                self.add_dep(i1, i2, 0, DepType::Regular);
                self.add_dep(i2, i1, 0, DepType::Regular);
            }
        }
        critical_count
    }

    fn count_conflicts(&self, posis1: &[f64], posis2: &[f64]) -> i32 {
        let mut conflicts = 0;
        if posis1.is_empty() || posis2.is_empty() {
            return 0;
        }
        let (mut i1, mut i2) = (0usize, 0usize);
        let mut pos1 = posis1[0];
        let mut pos2 = posis2[0];
        loop {
            if pos1 > pos2 - self.critical_conflict_threshold
                && pos1 < pos2 + self.critical_conflict_threshold
            {
                return CRITICAL_CONFLICTS_DETECTED;
            } else if pos1 > pos2 - self.conflict_threshold && pos1 < pos2 + self.conflict_threshold {
                conflicts += 1;
            }
            if pos1 <= pos2 && i1 + 1 < posis1.len() {
                i1 += 1;
                pos1 = posis1[i1];
            } else if pos2 <= pos1 && i2 + 1 < posis2.len() {
                i2 += 1;
                pos2 = posis2[i2];
            } else {
                break;
            }
        }
        conflicts
    }

    fn break_critical_cycles(&mut self, random: &mut JavaRandom) {
        let cycle_deps = self.detect_cycles(true, random);
        self.split_segments(cycle_deps);
    }

    // -- HyperEdgeSegmentSplitter --

    fn split_segments(&mut self, deps_to_resolve: Vec<usize>) {
        if deps_to_resolve.is_empty() {
            return;
        }
        let mut free_areas = self.find_free_areas();
        let to_split = self.decide_which_segments_to_split(&deps_to_resolve);
        // split smallest first
        let mut ordered = to_split;
        ordered.sort_by(|&a, &b| self.segments[a].length().total_cmp(&self.segments[b].length()));
        for seg in ordered {
            self.split(seg, &mut free_areas);
        }
    }

    fn find_free_areas(&self) -> Vec<(f64, f64)> {
        let mut coords: Vec<f64> = Vec::new();
        for s in &self.segments {
            coords.extend_from_slice(&s.incoming);
            coords.extend_from_slice(&s.outgoing);
        }
        coords.sort_by(|a, b| a.total_cmp(b));
        let mut areas = Vec::new();
        let ct = self.critical_conflict_threshold;
        for i in 1..coords.len() {
            if coords[i] - coords[i - 1] >= 2.0 * ct {
                areas.push((coords[i - 1] + ct, coords[i] - ct));
            }
        }
        areas
    }

    fn decide_which_segments_to_split(&mut self, deps: &[usize]) -> Vec<usize> {
        let mut set: Vec<usize> = Vec::new();
        for &di in deps {
            let source = self.deps[di].source.unwrap();
            let target = self.deps[di].target.unwrap();
            if set.contains(&source) || set.contains(&target) {
                continue;
            }
            let (mut to_split, mut causing) = (source, target);
            if self.segments[source].represents_hyperedge()
                && !self.segments[target].represents_hyperedge()
            {
                to_split = target;
                causing = source;
            }
            set.push(to_split);
            self.segments[to_split].split_by = Some(causing);
        }
        set
    }

    fn split(&mut self, seg: usize, free_areas: &mut Vec<(f64, f64)>) {
        let split_pos = self.compute_split_position(seg, free_areas);
        // split_at appends the new partner segment (Java: segments.add(...)).
        let partner = self.split_at(seg, split_pos);
        self.update_split_dependencies(seg, partner);
    }

    fn split_at(&mut self, seg: usize, split_pos: f64) -> usize {
        let partner = self.segments.len();
        self.segments.push(Segment::new());
        self.segments[partner].split_partner = Some(seg);
        self.segments[seg].split_partner = Some(partner);
        let outgoing = std::mem::take(&mut self.segments[seg].outgoing);
        self.segments[partner].outgoing = outgoing;
        self.segments[seg].outgoing.push(split_pos);
        self.segments[partner].incoming.push(split_pos);
        self.segments[seg].recompute_extent();
        self.segments[partner].recompute_extent();
        for di in self.segments[seg].in_deps.clone() {
            self.remove_dep(di);
        }
        for di in self.segments[seg].out_deps.clone() {
            self.remove_dep(di);
        }
        partner
    }

    fn update_split_dependencies(&mut self, seg: usize, partner: usize) {
        let causing = self.segments[seg].split_by.unwrap();
        self.add_dep(seg, causing, 1, DepType::Critical);
        self.add_dep(causing, partner, 1, DepType::Critical);
        for other in 0..self.segments.len() {
            if other != causing && other != seg && other != partner {
                self.create_dependency_if_necessary(other, seg);
                self.create_dependency_if_necessary(other, partner);
            }
        }
    }

    fn compute_split_position(&mut self, seg: usize, free_areas: &mut Vec<(f64, f64)>) -> f64 {
        let (seg_start, seg_end) = (self.segments[seg].start, self.segments[seg].end);
        let mut first = -1i64;
        let mut last = -1i64;
        for (i, area) in free_areas.iter().enumerate() {
            if area.0 > seg_end {
                break;
            } else if area.1 >= seg_start {
                if first < 0 {
                    first = i as i64;
                }
                last = i as i64;
            }
        }
        let mut split_pos = (seg_start + seg_end) / 2.0;
        if first >= 0 {
            let best = self.choose_best_area_index(seg, free_areas, first as usize, last as usize);
            split_pos = (free_areas[best].0 + free_areas[best].1) / 2.0;
            use_area(free_areas, best, self.critical_conflict_threshold);
        }
        split_pos
    }

    fn choose_best_area_index(
        &self,
        seg: usize,
        free_areas: &[(f64, f64)],
        from: usize,
        to: usize,
    ) -> usize {
        let mut best = from;
        if from < to {
            // simulate split: split_segment keeps incoming, partner keeps outgoing.
            let sim_split_in = self.segments[seg].incoming.clone();
            let sim_partner_out = self.segments[seg].outgoing.clone();
            let mut best_rating = self.rate_area(seg, &sim_split_in, &sim_partner_out, free_areas[from]);
            for i in (from + 1)..=to {
                let r = self.rate_area(seg, &sim_split_in, &sim_partner_out, free_areas[i]);
                if is_better(free_areas[i], r, free_areas[best], best_rating) {
                    best = i;
                    best_rating = r;
                }
            }
        }
        best
    }

    fn rate_area(
        &self,
        seg: usize,
        split_in: &[f64],
        partner_out: &[f64],
        area: (f64, f64),
    ) -> (i32, i32) {
        let centre = (area.0 + area.1) / 2.0;
        // split segment: incoming = split_in, outgoing = [centre]
        let split_out = vec![centre];
        let split_start = min_ext(split_in, &split_out);
        let split_end = max_ext(split_in, &split_out);
        // partner: incoming = [centre], outgoing = partner_out
        let partner_in = vec![centre];
        let partner_start = min_ext(&partner_in, partner_out);
        let partner_end = max_ext(&partner_in, partner_out);

        let mut dependencies = 0;
        let mut crossings = 0;
        let mut consider = |s_out: &[f64], s_in: &[f64], s_start: f64, s_end: f64, other: usize| {
            let (o_start, o_end) = (self.segments[other].start, self.segments[other].end);
            let c1 = count_crossings(s_out, o_start, o_end)
                + count_crossings(&self.segments[other].incoming, s_start, s_end);
            let c2 = count_crossings(&self.segments[other].outgoing, s_start, s_end)
                + count_crossings(s_in, o_start, o_end);
            if c1 == c2 {
                if c1 > 0 {
                    dependencies += 2;
                    crossings += c1;
                }
            } else {
                dependencies += 1;
                crossings += c1.min(c2);
            }
        };
        for &di in &self.segments[seg].in_deps {
            let other = self.deps[di].source.unwrap();
            consider(&split_out, split_in, split_start, split_end, other);
            consider(partner_out, &partner_in, partner_start, partner_end, other);
        }
        for &di in &self.segments[seg].out_deps {
            let other = self.deps[di].target.unwrap();
            consider(&split_out, split_in, split_start, split_end, other);
            consider(partner_out, &partner_in, partner_start, partner_end, other);
        }
        dependencies += 2;
        let split_by = self.segments[seg].split_by.unwrap();
        let (sb_start, sb_end) = (self.segments[split_by].start, self.segments[split_by].end);
        crossings += count_crossings(&split_out, sb_start, sb_end)
            + count_crossings(&self.segments[split_by].incoming, split_start, split_end);
        crossings += count_crossings(&self.segments[split_by].outgoing, partner_start, partner_end)
            + count_crossings(&partner_in, sb_start, sb_end);
        (dependencies, crossings)
    }

    fn break_non_critical_cycles(&mut self, random: &mut JavaRandom) {
        let cycle_deps = self.detect_cycles(false, random);
        for di in cycle_deps {
            if self.deps[di].weight == 0 {
                self.remove_dep(di);
            } else {
                self.reverse_dep(di);
            }
        }
    }

    // -- HyperEdgeCycleDetector.detectCycles --

    fn detect_cycles(&mut self, critical_only: bool, random: &mut JavaRandom) -> Vec<usize> {
        let n = self.segments.len();
        let mut sources: std::collections::VecDeque<usize> = Default::default();
        let mut sinks: std::collections::VecDeque<usize> = Default::default();
        // initialize
        let mut next_mark = -1;
        for si in 0..n {
            self.segments[si].mark = next_mark;
            next_mark -= 1;
            let (mut in_w, mut out_w, mut cin, mut cout) = (0, 0, 0, 0);
            for &di in &self.segments[si].in_deps {
                if self.deps[di].dep_type == DepType::Critical {
                    cin += self.deps[di].weight;
                }
            }
            for &di in &self.segments[si].out_deps {
                if self.deps[di].dep_type == DepType::Critical {
                    cout += self.deps[di].weight;
                }
            }
            if critical_only {
                in_w = cin;
                out_w = cout;
            } else {
                for &di in &self.segments[si].in_deps {
                    in_w += self.deps[di].weight;
                }
                for &di in &self.segments[si].out_deps {
                    out_w += self.deps[di].weight;
                }
            }
            self.segments[si].in_weight = in_w;
            self.segments[si].critical_in_weight = cin;
            self.segments[si].out_weight = out_w;
            self.segments[si].critical_out_weight = cout;
            if out_w == 0 {
                sinks.push_back(si);
            } else if in_w == 0 {
                sources.push_back(si);
            }
        }

        // computeLinearOrderingMarks: unprocessed ordered by mark ascending
        // = reverse creation order (initial marks -(i+1)).
        let mut unprocessed: BTreeSet<i32> = (0..n as i32).map(|i| -(i + 1)).collect();
        let mark_base = n as i32;
        let mut next_sink_mark = mark_base - 1;
        let mut next_source_mark = mark_base + 1;
        let seg_of_mark = |m: i32| -> usize { (-m - 1) as usize };

        while !unprocessed.is_empty() {
            while let Some(sink) = sinks.pop_front() {
                unprocessed.remove(&-(sink as i32 + 1));
                self.segments[sink].mark = next_sink_mark;
                next_sink_mark -= 1;
                self.update_neighbors(sink, &mut sources, &mut sinks, critical_only);
            }
            while let Some(source) = sources.pop_front() {
                unprocessed.remove(&-(source as i32 + 1));
                self.segments[source].mark = next_source_mark;
                next_source_mark += 1;
                self.update_neighbors(source, &mut sources, &mut sinks, critical_only);
            }
            if unprocessed.is_empty() {
                break;
            }
            let mut max_outflow = i32::MIN;
            let mut max_segments: Vec<usize> = Vec::new();
            let mut forced = false;
            // iterate unprocessed in TreeSet (mark-ascending) order.
            let order: Vec<i32> = unprocessed.iter().copied().collect();
            for m in order {
                let si = seg_of_mark(m);
                if !critical_only
                    && self.segments[si].critical_out_weight > 0
                    && self.segments[si].critical_in_weight <= 0
                {
                    max_segments.clear();
                    max_segments.push(si);
                    forced = true;
                    break;
                }
                let outflow = self.segments[si].out_weight - self.segments[si].in_weight;
                if outflow >= max_outflow {
                    if outflow > max_outflow {
                        max_segments.clear();
                        max_outflow = outflow;
                    }
                    max_segments.push(si);
                }
            }
            let _ = forced;
            if !max_segments.is_empty() {
                let max_node = max_segments[random.next_int(max_segments.len() as i32) as usize];
                unprocessed.remove(&-(max_node as i32 + 1));
                self.segments[max_node].mark = next_source_mark;
                next_source_mark += 1;
                self.update_neighbors(max_node, &mut sources, &mut sinks, critical_only);
            }
        }
        // shift marks below base up.
        let shift_base = n as i32 + 1;
        for si in 0..n {
            if self.segments[si].mark < mark_base {
                self.segments[si].mark += shift_base;
            }
        }
        // collect left-pointing deps
        let mut result = Vec::new();
        for si in 0..n {
            for &di in &self.segments[si].out_deps {
                if !critical_only || self.deps[di].dep_type == DepType::Critical {
                    let t = self.deps[di].target.unwrap();
                    if self.segments[si].mark > self.segments[t].mark {
                        result.push(di);
                    }
                }
            }
        }
        result
    }

    fn update_neighbors(
        &mut self,
        node: usize,
        sources: &mut std::collections::VecDeque<usize>,
        sinks: &mut std::collections::VecDeque<usize>,
        critical_only: bool,
    ) {
        let out_deps = self.segments[node].out_deps.clone();
        for di in out_deps {
            if critical_only && self.deps[di].dep_type != DepType::Critical {
                continue;
            }
            let target = self.deps[di].target.unwrap();
            if self.segments[target].mark < 0 && self.deps[di].weight > 0 {
                self.segments[target].in_weight -= self.deps[di].weight;
                if self.deps[di].dep_type == DepType::Critical {
                    self.segments[target].critical_in_weight -= self.deps[di].weight;
                }
                if self.segments[target].in_weight <= 0 && self.segments[target].out_weight > 0 {
                    sources.push_back(target);
                }
            }
        }
        let in_deps = self.segments[node].in_deps.clone();
        for di in in_deps {
            if critical_only && self.deps[di].dep_type != DepType::Critical {
                continue;
            }
            let source = self.deps[di].source.unwrap();
            if self.segments[source].mark < 0 && self.deps[di].weight > 0 {
                self.segments[source].out_weight -= self.deps[di].weight;
                if self.deps[di].dep_type == DepType::Critical {
                    self.segments[source].critical_out_weight -= self.deps[di].weight;
                }
                if self.segments[source].out_weight <= 0 && self.segments[source].in_weight > 0 {
                    sinks.push_back(source);
                }
            }
        }
    }

    fn topological_numbering(&mut self) {
        let n = self.segments.len();
        let mut sources: std::collections::VecDeque<usize> = Default::default();
        let mut rightward_targets: Vec<usize> = Vec::new();
        for si in 0..n {
            self.segments[si].in_weight = self.segments[si].in_deps.len() as i32;
            self.segments[si].out_weight = self.segments[si].out_deps.len() as i32;
            if self.segments[si].in_weight == 0 {
                sources.push_back(si);
            }
            if self.segments[si].out_weight == 0 && self.segments[si].incoming.is_empty() {
                rightward_targets.push(si);
            }
        }
        let mut max_rank = -1;
        while let Some(node) = sources.pop_front() {
            let out_deps = self.segments[node].out_deps.clone();
            for di in out_deps {
                let target = self.deps[di].target.unwrap();
                let ns = (self.segments[node].routing_slot + 1).max(self.segments[target].routing_slot);
                self.segments[target].routing_slot = ns;
                max_rank = max_rank.max(ns);
                self.segments[target].in_weight -= 1;
                if self.segments[target].in_weight == 0 {
                    sources.push_back(target);
                }
            }
        }
        if max_rank > -1 {
            for &si in &rightward_targets {
                self.segments[si].routing_slot = max_rank;
            }
            let mut queue: std::collections::VecDeque<usize> = rightward_targets.into_iter().collect();
            while let Some(node) = queue.pop_front() {
                let in_deps = self.segments[node].in_deps.clone();
                for di in in_deps {
                    let source = self.deps[di].source.unwrap();
                    if !self.segments[source].incoming.is_empty() {
                        continue;
                    }
                    self.segments[source].routing_slot =
                        self.segments[source].routing_slot.min(self.segments[node].routing_slot - 1);
                    self.segments[source].out_weight -= 1;
                    if self.segments[source].out_weight == 0 {
                        queue.push_back(source);
                    }
                }
            }
        }
    }

    // -- WestToEast.calculateBendPoints --

    fn calculate_bend_points(&mut self, si: usize, start_pos: f64) {
        // Dummy (split-partner) segments are handled when their partner is processed.
        if self.segments[si].is_dummy() {
            return;
        }
        let segment_x = start_pos + self.segments[si].routing_slot as f64 * self.edge_spacing;
        let split_partner = self.segments[si].split_partner;
        let ports = self.segments[si].ports.clone();
        for port in ports {
            let source_y = self.absolute_anchor_y(port);
            for e in self.arena.ports[port.0].outgoing_edges.clone() {
                if self.arena.edge_is_self_loop(e) {
                    continue;
                }
                let target = self.arena.edges[e.0].target.unwrap();
                let target_y = self.absolute_anchor_y(target);
                if (source_y - target_y).abs() > TOLERANCE {
                    let mut current_x = segment_x;
                    self.arena.edges[e.0]
                        .bend_points
                        .push(super::super::math::KVector::new(current_x, source_y));
                    if let Some(partner) = split_partner {
                        let split_y = self.segments[partner].incoming[0];
                        self.arena.edges[e.0]
                            .bend_points
                            .push(super::super::math::KVector::new(current_x, split_y));
                        current_x = start_pos + self.segments[partner].routing_slot as f64 * self.edge_spacing;
                        self.arena.edges[e.0]
                            .bend_points
                            .push(super::super::math::KVector::new(current_x, split_y));
                    }
                    self.arena.edges[e.0]
                        .bend_points
                        .push(super::super::math::KVector::new(current_x, target_y));
                }
            }
        }
    }
}

fn insert_sorted(list: &mut Vec<f64>, value: f64) {
    let mut i = 0;
    while i < list.len() {
        if list[i] == value {
            return;
        } else if list[i] > value {
            break;
        }
        i += 1;
    }
    list.insert(i, value);
}

fn minimum_difference(numbers: &[f64]) -> f64 {
    let mut v: Vec<f64> = numbers.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    v.dedup();
    let mut min_diff = f64::MAX;
    for w in v.windows(2) {
        min_diff = min_diff.min(w[1] - w[0]);
    }
    min_diff
}

/// Extent (min start / max end) of two sorted coordinate lists.
fn min_ext(a: &[f64], b: &[f64]) -> f64 {
    let mut m = f64::NAN;
    for list in [a, b] {
        if let Some(&f) = list.first() {
            m = if m.is_nan() { f } else { m.min(f) };
        }
    }
    m
}
fn max_ext(a: &[f64], b: &[f64]) -> f64 {
    let mut m = f64::NAN;
    for list in [a, b] {
        if let Some(&l) = list.last() {
            m = if m.is_nan() { l } else { m.max(l) };
        }
    }
    m
}

/// Java `HyperEdgeSegmentSplitter.useArea`: split the used free area.
fn use_area(free_areas: &mut Vec<(f64, f64)>, index: usize, ct: f64) {
    let old = free_areas.remove(index);
    let size = old.1 - old.0;
    if size / 2.0 >= ct {
        let mut insert = index;
        let centre = (old.0 + old.1) / 2.0;
        if old.0 <= centre - ct {
            free_areas.insert(insert, (old.0, centre - ct));
            insert += 1;
        }
        if centre + ct <= old.1 {
            free_areas.insert(insert, (centre + ct, old.1));
        }
    }
}

/// Java `HyperEdgeSegmentSplitter.isBetter`. Rating = (dependencies,
/// crossings); area = (start, end).
fn is_better(curr_area: (f64, f64), curr: (i32, i32), best_area: (f64, f64), best: (i32, i32)) -> bool {
    if curr.1 < best.1 {
        true
    } else if curr.1 == best.1 {
        if curr.0 < best.0 {
            true
        } else if curr.0 == best.0 {
            (curr_area.1 - curr_area.0) > (best_area.1 - best_area.0)
        } else {
            false
        }
    } else {
        false
    }
}

fn count_crossings(posis: &[f64], start: f64, end: f64) -> i32 {
    let mut crossings = 0;
    for &pos in posis {
        if pos > end {
            break;
        } else if pos >= start {
            crossings += 1;
        }
    }
    crossings
}

/// Whether a graph has any node needing routing (used to short-circuit).
pub fn has_nodes(arena: &LGraphArena, graph: super::super::graph::LGraphId) -> bool {
    arena.graphs[graph.0].layers.iter().any(|l| l.nodes.iter().any(|&n| {
        matches!(arena.nodes[n.0].node_type, NodeType::Normal | NodeType::LongEdge)
    }))
}

/// Route all edges (P5) and assign layer x-positions, threading the
/// shared random. Returns the graph width. Combines the router shell
/// (`super::route_and_place`) with the per-gap routing generator.
pub fn route_orthogonal(
    arena: &mut LGraphArena,
    graph: super::super::graph::LGraphId,
    random: &mut JavaRandom,
) -> f64 {
    let edge_spacing = arena.graphs[graph.0].props.spacing.edge_edge_between_layers;
    // route_and_place calls `route` per gap; each call builds a fresh
    // generator, matching ELK's per-gap segment/dependency graph while
    // sharing the random stream.
    super::route_and_place(arena, graph, |arena, left, right, start_pos| {
        let left_nodes = left.map(|li| super::layer_nodes(arena, graph, li));
        let right_nodes = right.map(|li| super::layer_nodes(arena, graph, li));
        let mut gen = RoutingGen::new(arena, edge_spacing);
        gen.route_edges(left_nodes.as_deref(), right_nodes.as_deref(), start_pos, random)
    })
}
