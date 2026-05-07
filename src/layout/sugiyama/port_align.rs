//! Per-node perpendicular-axis solver. Runs after Brandes-Kopf and
//! before `edge_fix`/`compact`.
//!
//! BK places each node at the centre of its rank's sibling stack. For
//! record graphs that produces two related defects: children drift away
//! from the row anchor that references them (so parent→child edges
//! sweep), and narrow rank-2 children get pinned behind wide rank-1
//! siblings on the rank-axis compaction. Aligning child perpendicular
//! position to the parent's row anchor fixes both.
//!
//! Operates in TB-internal coords (perpendicular axis = x), so the LR
//! transpose wrapper makes this serve LR diagrams as well. The pass is
//! a no-op unless at least one edge carries a `source_perp_offset` /
//! `target_perp_offset` hint, so non-record diagrams are unaffected.
//!
//! Algorithm: block-coordinate descent on a convex quadratic. Each
//! outer iteration solves one rank exactly (treating other ranks as
//! fixed) via PAV — pool-adjacent-violators isotonic regression with
//! per-edge sibling gaps as offsets. PAV avoids the failure mode of
//! per-node Gauss-Seidel-with-clamp, which gets stuck at constraint
//! boundaries when two adjacent siblings both want to move in the same
//! direction.
//!
//! Acceptance guard: the candidate is rejected (and BK coordinates
//! restored) if it inflates perpendicular extent beyond the budget or
//! produces same-rank overlap. See `accept` for the exact criteria.

use crate::layout::dag::NodeHandle;
use crate::layout::graph::VisualGraph;

const W_PORT: f64 = 1.0;
const W_CONN: f64 = 1.0;
const W_STAB: f64 = 0.05;
const MAX_ITERS: usize = 100;
const CONVERGENCE_EPS: f64 = 0.01;
/// How much perpendicular growth we allow before rejecting the candidate
/// and falling back to BK. Tightened to a near-zero margin (just enough
/// to absorb floating-point noise) because the docs/path-B analysis on
/// `person.puml` shows that for record graphs with one wide rank-1
/// sibling, any meaningful extent growth doesn't translate into
/// rank-axis compactness — `compact::do_it` still sees the wide sibling
/// as a perpendicular blocker. A future cross-rank-overlap objective
/// can revisit this multiplier.
const EXTENT_BUDGET_MULT: f64 = 1.005;
const EXTENT_BUDGET_PAD: f64 = 0.5;
const ACCEPT_IMPROVEMENT_EPS: f64 = 1e-3;

/// One port-alignment constraint: `t_v - t_u == delta`. `delta` is
/// derived from the edge's perpendicular-axis offsets and the source /
/// target box sizes, all in pre-transpose units (which equal
/// internal-TB perpendicular units, since perp_offset is a length).
#[derive(Debug, Clone, Copy)]
struct Alignment {
    u: NodeHandle,
    v: NodeHandle,
    delta: f64,
}

pub(crate) fn do_it(vg: &mut VisualGraph) {
    let alignments = collect_alignments(vg);
    if alignments.is_empty() {
        return;
    }

    let n_nodes = vg.num_nodes();
    let bk_centers: Vec<f64> = (0..n_nodes)
        .map(|i| vg.pos(NodeHandle::from(i)).center().x)
        .collect();

    let mut centers = bk_centers.clone();
    for _ in 0..MAX_ITERS {
        let mut max_move: f64 = 0.0;
        for r in 0..vg.dag.num_levels() {
            let row = vg.dag.row(r).clone();
            if row.is_empty() {
                continue;
            }
            let mut targets = Vec::with_capacity(row.len());
            let mut precisions = Vec::with_capacity(row.len());
            for &n in &row {
                let (t, p) = target_and_precision(vg, n, &centers, &bk_centers, &alignments);
                targets.push(t);
                precisions.push(p);
            }
            let mut gaps = Vec::with_capacity(row.len().saturating_sub(1));
            for i in 0..row.len().saturating_sub(1) {
                let ha = vg.pos(row[i]).size(true).x / 2.0;
                let hb = vg.pos(row[i + 1]).size(true).x / 2.0;
                gaps.push(ha + hb);
            }
            let new_t = pav(&targets, &precisions, &gaps);
            for (i, &n) in row.iter().enumerate() {
                let delta = (new_t[i] - centers[n.get_index()]).abs();
                centers[n.get_index()] = new_t[i];
                if delta > max_move {
                    max_move = delta;
                }
            }
        }
        if max_move < CONVERGENCE_EPS {
            break;
        }
    }

    if !accept(vg, &centers, &bk_centers, &alignments) {
        return;
    }

    apply_centers(vg, &centers);
    super::simple::align_to_left(vg);
}

fn collect_alignments(vg: &VisualGraph) -> Vec<Alignment> {
    let mut out = Vec::new();
    for (edge, chain) in vg.iter_edges() {
        let (s, r) = match (edge.source_perp_offset, edge.target_perp_offset) {
            (Some(s), Some(r)) => (s, r),
            _ => continue,
        };
        let u = chain[0];
        let v = chain[chain.len() - 1];
        // Source port (centered coords): port_u = t_u - size(u,false)/2 + s
        // Target port:                   port_v = t_v - size(v,false)/2 + r
        // port_u == port_v iff:
        //   t_v - t_u = (s - size(u,false)/2) - (r - size(v,false)/2)
        let half_u = vg.pos(u).size(false).x / 2.0;
        let half_v = vg.pos(v).size(false).x / 2.0;
        let delta = (s - half_u) - (r - half_v);
        out.push(Alignment { u, v, delta });
    }
    out
}

/// Per-node 1D quadratic minimum (treating other nodes fixed) plus the
/// total weight on the variable. Caller composes them across a rank
/// and feeds them to `pav`.
fn target_and_precision(
    vg: &VisualGraph,
    n: NodeHandle,
    centers: &[f64],
    bk_centers: &[f64],
    alignments: &[Alignment],
) -> (f64, f64) {
    let mut num = 0.0_f64;
    let mut den = 0.0_f64;

    if vg.is_connector(n) {
        let pred = vg.dag.single_pred(n);
        let succ = vg.dag.single_succ(n);
        match (pred, succ) {
            (Some(p), Some(s)) => {
                num += W_CONN * 0.5 * (centers[p.get_index()] + centers[s.get_index()]);
                den += W_CONN;
            }
            (Some(p), None) => {
                num += W_CONN * centers[p.get_index()];
                den += W_CONN;
            }
            (None, Some(s)) => {
                num += W_CONN * centers[s.get_index()];
                den += W_CONN;
            }
            (None, None) => {}
        }
        num += W_STAB * bk_centers[n.get_index()];
        den += W_STAB;
        return if den > 0.0 {
            (num / den, den)
        } else {
            (bk_centers[n.get_index()], W_STAB)
        };
    }

    for a in alignments {
        if a.u == n {
            num += W_PORT * (centers[a.v.get_index()] - a.delta);
            den += W_PORT;
        } else if a.v == n {
            num += W_PORT * (centers[a.u.get_index()] + a.delta);
            den += W_PORT;
        }
    }

    num += W_STAB * bk_centers[n.get_index()];
    den += W_STAB;

    (num / den, den)
}

/// Pool-Adjacent-Violators isotonic regression with per-position
/// offsets (the "gap" between consecutive variables). Solves
///
///   minimize  sum_i p_i * (t_i - target_i)^2
///   s.t.      t_{i+1} - t_i >= gaps[i]
///
/// in O(n) by working in shifted coords `u_i = t_i - cum_gaps[i]`,
/// where the constraint becomes plain monotonicity.
fn pav(targets: &[f64], precisions: &[f64], gaps: &[f64]) -> Vec<f64> {
    let n = targets.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![targets[0]];
    }
    debug_assert_eq!(precisions.len(), n);
    debug_assert_eq!(gaps.len(), n - 1);

    let mut cum = vec![0.0_f64; n];
    for i in 1..n {
        cum[i] = cum[i - 1] + gaps[i - 1];
    }

    #[derive(Clone)]
    struct Block {
        wsum: f64,
        w: f64,
        count: usize,
    }

    let mut blocks: Vec<Block> = Vec::with_capacity(n);
    for i in 0..n {
        let tu = targets[i] - cum[i];
        let mut blk = Block {
            wsum: precisions[i] * tu,
            w: precisions[i],
            count: 1,
        };
        while let Some(prev) = blocks.last() {
            // prev_avg > blk_avg means the new block violates monotonicity
            // and must be pooled with the previous one. Cross-multiply to
            // avoid division.
            if prev.wsum * blk.w > blk.wsum * prev.w {
                let popped = blocks.pop().unwrap();
                blk.wsum += popped.wsum;
                blk.w += popped.w;
                blk.count += popped.count;
            } else {
                break;
            }
        }
        blocks.push(blk);
    }

    let mut u = Vec::with_capacity(n);
    for b in &blocks {
        let avg = b.wsum / b.w;
        for _ in 0..b.count {
            u.push(avg);
        }
    }
    (0..n).map(|i| u[i] + cum[i]).collect()
}

fn apply_centers(vg: &mut VisualGraph, centers: &[f64]) {
    for (i, &c) in centers.iter().enumerate() {
        vg.pos_mut(NodeHandle::from(i)).set_x(c);
    }
}

fn accept(vg: &VisualGraph, centers: &[f64], bk_centers: &[f64], alignments: &[Alignment]) -> bool {
    if has_overlap(vg, centers) {
        return false;
    }
    let bk_extent = perp_extent(vg, bk_centers);
    let cand_extent = perp_extent(vg, centers);
    let budget =
        bk_extent.max(max_rank_extent(vg, bk_centers)) * EXTENT_BUDGET_MULT + EXTENT_BUDGET_PAD;
    if cand_extent > budget {
        return false;
    }
    let bk_score = score(bk_centers, alignments);
    let cand_score = score(centers, alignments);
    if cand_score > bk_score - ACCEPT_IMPROVEMENT_EPS {
        return false;
    }
    true
}

fn score(centers: &[f64], alignments: &[Alignment]) -> f64 {
    let mut s = 0.0;
    for a in alignments {
        let r = (centers[a.v.get_index()] - centers[a.u.get_index()]) - a.delta;
        s += W_PORT * r * r;
    }
    s
}

fn has_overlap(vg: &VisualGraph, centers: &[f64]) -> bool {
    for r in 0..vg.dag.num_levels() {
        let row = vg.dag.row(r);
        for win in row.windows(2) {
            let a = win[0];
            let b = win[1];
            let a_right = centers[a.get_index()] + vg.pos(a).size(true).x / 2.0;
            let b_left = centers[b.get_index()] - vg.pos(b).size(true).x / 2.0;
            if a_right > b_left + 1e-6 {
                return true;
            }
        }
    }
    false
}

fn perp_extent(vg: &VisualGraph, centers: &[f64]) -> f64 {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for n in vg.iter_nodes() {
        let half = vg.pos(n).size(true).x / 2.0;
        let c = centers[n.get_index()];
        lo = lo.min(c - half);
        hi = hi.max(c + half);
    }
    if lo.is_infinite() {
        0.0
    } else {
        hi - lo
    }
}

fn max_rank_extent(vg: &VisualGraph, centers: &[f64]) -> f64 {
    let mut best: f64 = 0.0;
    for r in 0..vg.dag.num_levels() {
        let row = vg.dag.row(r);
        if row.is_empty() {
            continue;
        }
        let mut lo = f64::INFINITY;
        let mut hi = f64::NEG_INFINITY;
        for &n in row {
            let half = vg.pos(n).size(true).x / 2.0;
            let c = centers[n.get_index()];
            lo = lo.min(c - half);
            hi = hi.max(c + half);
        }
        best = best.max(hi - lo);
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pav_passthrough_when_targets_already_feasible() {
        let t = pav(&[0.0, 5.0, 12.0], &[1.0; 3], &[3.0, 4.0]);
        assert!((t[0] - 0.0).abs() < 1e-9);
        assert!((t[1] - 5.0).abs() < 1e-9);
        assert!((t[2] - 12.0).abs() < 1e-9);
    }

    #[test]
    fn pav_pools_when_targets_too_close() {
        // Targets 0 and 0 with required gap 4: optimum splits the gap
        // around the average target, so t_0 = -2, t_1 = +2.
        let t = pav(&[0.0, 0.0], &[1.0, 1.0], &[4.0]);
        assert!((t[0] - (-2.0)).abs() < 1e-9, "got {}", t[0]);
        assert!((t[1] - 2.0).abs() < 1e-9, "got {}", t[1]);
    }

    #[test]
    fn pav_weighted_pull_reflects_precision_ratio() {
        // Heavy precision on the second target pulls the pair toward it.
        let t = pav(&[0.0, 0.0], &[1.0, 4.0], &[4.0]);
        // Pooled target average = (1*0 + 4*0)/5 = 0. Then split: t_0 = -gap*p1/(p0+p1) = -4*4/5 = -3.2, t_1 = +0.8.
        assert!((t[0] - (-3.2)).abs() < 1e-6, "got {}", t[0]);
        assert!((t[1] - 0.8).abs() < 1e-6, "got {}", t[1]);
    }
}
