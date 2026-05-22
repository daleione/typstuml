//! Network simplex (Gansner, Koutsofios, North & Vo — "A Technique for
//! Drawing Directed Graphs", §2.3 / §4.2), mirroring graphviz
//! `lib/common/ns.c`.
//!
//! Solves: assign a `rank` to every node minimising
//! `Σ weight(e) · (rank[head(e)] − rank[tail(e)])`
//! subject to `rank[head(e)] − rank[tail(e)] ≥ minlen(e)` for every edge.
//!
//! `dot` uses this for both the rank (y) assignment and — via an
//! auxiliary graph — the x-coordinate assignment. We use real-valued
//! ranks so the x-auxiliary graph's pixel-sized separations work
//! directly. The pivot loop recomputes cut values from scratch each step
//! (O(pivots·E)); diagrams are small, so the simpler-but-correct form is
//! preferred over graphviz's incremental `treeupdate`.

const EPS: f64 = 1e-7;
const MAX_PIVOTS: usize = 4000;

#[derive(Clone)]
struct Edge {
    tail: usize,
    head: usize,
    minlen: f64,
    weight: f64,
    tree: bool,
}

/// Minimise total weighted edge length subject to the minlen constraints.
/// `edges` are `(tail, head, minlen, weight)`. Returns one rank per node.
/// The input must be acyclic (a DAG); callers feed lowered Sugiyama
/// graphs and the x-auxiliary graph, both DAGs.
pub fn solve(n: usize, edges: &[(usize, usize, f64, f64)]) -> Vec<f64> {
    solve_impl(n, edges, false)
}

/// As [`solve`], but with dot's left-right balancing applied afterwards
/// (graphviz `LR_balance`, ns.c): a node whose tree edge has cut value 0
/// can slide within a slack range without changing total edge length, so
/// it is centred in that range. Used for the x-coordinate assignment so
/// symmetric structures (a choice fanning to two balanced branches, a
/// straight spine beside a sibling) lay out symmetrically instead of
/// jammed against one neighbour. Not used for rank (y) assignment.
pub fn solve_balanced(n: usize, edges: &[(usize, usize, f64, f64)]) -> Vec<f64> {
    solve_impl(n, edges, true)
}

fn solve_impl(n: usize, edges: &[(usize, usize, f64, f64)], balance: bool) -> Vec<f64> {
    if n == 0 {
        return Vec::new();
    }
    let mut e: Vec<Edge> = edges
        .iter()
        .map(|&(tail, head, minlen, weight)| Edge {
            tail,
            head,
            minlen,
            weight,
            tree: false,
        })
        .collect();

    let mut rank = init_rank(n, &e);
    if e.is_empty() {
        return rank;
    }
    feasible_tree(n, &mut e, &mut rank);

    let mut iters = 0;
    while iters < MAX_PIVOTS {
        iters += 1;
        let Some(leave) = leave_edge(n, &e, &rank) else {
            break;
        };
        let (in_head, _) = components(n, &e, leave);
        // Entering edge: a non-tree edge crossing back (tail in the head
        // component, head in the tail component) with minimum slack.
        let mut enter = None;
        let mut best = f64::INFINITY;
        for (i, ed) in e.iter().enumerate() {
            if ed.tree {
                continue;
            }
            if in_head[ed.tail] && !in_head[ed.head] {
                let s = slack(&rank, ed);
                if s < best {
                    best = s;
                    enter = Some(i);
                }
            }
        }
        let Some(enter) = enter else { break };
        let delta = slack(&rank, &e[enter]);
        if delta > EPS {
            for v in 0..n {
                if in_head[v] {
                    rank[v] += delta;
                }
            }
        }
        e[leave].tree = false;
        e[enter].tree = true;
    }

    if balance {
        balance_lr(n, &e, &mut rank);
    }
    normalize(&mut rank);
    rank
}

/// dot's `LR_balance` (ns.c): centre any node whose tree edge has cut
/// value 0 within its feasible slide range. The tree edge is tight (one
/// end of the range); the min-slack non-tree edge crossing the cut bounds
/// the other end at `delta`, so shifting the head-side component by
/// `delta/2` centres it. Single forward pass over tree edges, matching dot.
fn balance_lr(n: usize, e: &[Edge], rank: &mut [f64]) {
    for i in 0..e.len() {
        if !e[i].tree {
            continue;
        }
        if cutvalue(n, e, i).abs() > EPS {
            continue;
        }
        let (in_head, _) = components(n, e, i);
        // Slide range of the head component: bounded by the smallest slack
        // among non-tree edges running from the head side back to the tail
        // side (increasing the head ranks tightens them).
        let mut delta = f64::INFINITY;
        for ed in e.iter() {
            if ed.tree {
                continue;
            }
            if in_head[ed.tail] && !in_head[ed.head] {
                delta = delta.min(slack(rank, ed));
            }
        }
        if delta.is_finite() && delta > EPS {
            for v in 0..n {
                if in_head[v] {
                    rank[v] += delta / 2.0;
                }
            }
        }
    }
}

fn slack(rank: &[f64], e: &Edge) -> f64 {
    rank[e.head] - rank[e.tail] - e.minlen
}

/// Longest-path feasible ranking over the DAG.
fn init_rank(n: usize, e: &[Edge]) -> Vec<f64> {
    let mut indeg = vec![0usize; n];
    let mut out: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, ed) in e.iter().enumerate() {
        indeg[ed.head] += 1;
        out[ed.tail].push(i);
    }
    let mut rank = vec![0.0f64; n];
    let mut queue: Vec<usize> = (0..n).filter(|&v| indeg[v] == 0).collect();
    let mut qi = 0;
    let mut seen = 0;
    while qi < queue.len() {
        let u = queue[qi];
        qi += 1;
        seen += 1;
        for &ei in &out[u] {
            let ed = &e[ei];
            let cand = rank[u] + ed.minlen;
            if cand > rank[ed.head] {
                rank[ed.head] = cand;
            }
            indeg[ed.head] -= 1;
            if indeg[ed.head] == 0 {
                queue.push(ed.head);
            }
        }
    }
    debug_assert_eq!(seen, n, "init_rank: graph not acyclic");
    rank
}

/// Build a tight spanning tree, shifting component ranks until every node
/// is reachable through slack-0 edges. Marks `tree` on the chosen edges.
fn feasible_tree(n: usize, e: &mut [Edge], rank: &mut [f64]) {
    loop {
        for ed in e.iter_mut() {
            ed.tree = false;
        }
        let (size, in_tree) = tight_tree(n, e, rank);
        if size >= n {
            return;
        }
        // Minimum-slack edge with exactly one endpoint in the tree.
        let mut best = f64::INFINITY;
        let mut best_e = None;
        for (i, ed) in e.iter().enumerate() {
            if in_tree[ed.tail] != in_tree[ed.head] {
                let s = slack(rank, ed).abs();
                if s < best {
                    best = s;
                    best_e = Some(i);
                }
            }
        }
        let Some(bi) = best_e else {
            // Disconnected: pin the first out-of-tree node to the tree
            // baseline so the loop terminates.
            for v in 0..n {
                if !in_tree[v] {
                    rank[v] = 0.0;
                }
            }
            // Connect with zero-cost virtual handling: just stop — the
            // remaining components are independent and already feasible.
            return;
        };
        let mut d = slack(rank, &e[bi]);
        if in_tree[e[bi].head] {
            d = -d;
        }
        if d != 0.0 {
            for v in 0..n {
                if in_tree[v] {
                    rank[v] += d;
                }
            }
        }
    }
}

/// Grow a maximal subtree of slack-0 edges from node 0; mark those edges
/// `tree`. Returns (#nodes reached, membership).
fn tight_tree(n: usize, e: &mut [Edge], rank: &[f64]) -> (usize, Vec<bool>) {
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, ed) in e.iter().enumerate() {
        adj[ed.tail].push(i);
        adj[ed.head].push(i);
    }
    let mut in_tree = vec![false; n];
    let mut stack = vec![0usize];
    in_tree[0] = true;
    let mut size = 1;
    while let Some(u) = stack.pop() {
        for &ei in &adj[u] {
            if slack(rank, &e[ei]).abs() > EPS {
                continue;
            }
            let other = if e[ei].tail == u { e[ei].head } else { e[ei].tail };
            if !in_tree[other] {
                in_tree[other] = true;
                e[ei].tree = true;
                size += 1;
                stack.push(other);
            }
        }
    }
    (size, in_tree)
}

/// A tree edge has negative cut value: pick the most-negative one to
/// leave the tree.
fn leave_edge(n: usize, e: &[Edge], rank: &[f64]) -> Option<usize> {
    let mut best = -EPS;
    let mut best_e = None;
    for (i, ed) in e.iter().enumerate() {
        if !ed.tree {
            continue;
        }
        let cv = cutvalue(n, e, i);
        let _ = rank;
        if cv < best {
            best = cv;
            best_e = Some(i);
        }
    }
    best_e
}

/// Membership of the head-component after removing tree edge `te`
/// (the side containing `head(te)`), plus its complement is the rest.
fn components(n: usize, e: &[Edge], te: usize) -> (Vec<bool>, Vec<bool>) {
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for (i, ed) in e.iter().enumerate() {
        if ed.tree && i != te {
            adj[ed.tail].push(ed.head);
            adj[ed.head].push(ed.tail);
        }
    }
    let head = e[te].head;
    let mut in_head = vec![false; n];
    let mut stack = vec![head];
    in_head[head] = true;
    while let Some(u) = stack.pop() {
        for &w in &adj[u] {
            if !in_head[w] {
                in_head[w] = true;
                stack.push(w);
            }
        }
    }
    let in_tail: Vec<bool> = in_head.iter().map(|b| !b).collect();
    (in_head, in_tail)
}

/// Cut value of tree edge `te`: Σ weight of edges crossing tail→head
/// minus Σ weight crossing head→tail, over the component split.
fn cutvalue(n: usize, e: &[Edge], te: usize) -> f64 {
    let (in_head, _) = components(n, e, te);
    // tail component = !in_head.
    let mut cv = 0.0;
    for ed in e {
        let t_tail = !in_head[ed.tail];
        let h_head = in_head[ed.head];
        if t_tail && h_head {
            cv += ed.weight; // crosses tail→head (same sense as te)
        } else if !t_tail && !h_head {
            cv -= ed.weight; // crosses head→tail
        }
    }
    cv
}

fn normalize(rank: &mut [f64]) {
    let min = rank.iter().copied().fold(f64::INFINITY, f64::min);
    if min.is_finite() {
        for r in rank.iter_mut() {
            *r -= min;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: &[f64], b: &[f64]) {
        assert_eq!(a.len(), b.len());
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() < 1e-6, "got {a:?} want {b:?}");
        }
    }

    #[test]
    fn chain() {
        // 0→1→2, minlen 1 each → ranks 0,1,2.
        let r = solve(3, &[(0, 1, 1.0, 1.0), (1, 2, 1.0, 1.0)]);
        approx(&r, &[0.0, 1.0, 2.0]);
    }

    #[test]
    fn diamond_balances() {
        // 0→1, 0→2, 1→3, 2→3. All tight → 0,1,1,2.
        let r = solve(4, &[(0, 1, 1.0, 1.0), (0, 2, 1.0, 1.0), (1, 3, 1.0, 1.0), (2, 3, 1.0, 1.0)]);
        approx(&r, &[0.0, 1.0, 1.0, 2.0]);
    }

    #[test]
    fn minlen_two() {
        // 0→1 minlen 2 → rank diff ≥ 2.
        let r = solve(2, &[(0, 1, 2.0, 1.0)]);
        approx(&r, &[0.0, 2.0]);
    }

    #[test]
    fn slack_pulled_tight_by_weight() {
        // 0→2 (minlen1), 1→2 (minlen1), 0→1 (minlen1). Heavy 0→2 keeps
        // 2 close to 0+? Longest path: 0=0,1=1,2=2. NS keeps feasibility.
        let r = solve(3, &[(0, 1, 1.0, 1.0), (1, 2, 1.0, 1.0), (0, 2, 1.0, 1.0)]);
        // 2 must be ≥1 above 1, and ≥1 above 0 → 0,1,2.
        approx(&r, &[0.0, 1.0, 2.0]);
    }

    #[test]
    fn x_aux_aligns_chain() {
        // Edge-node trick: align a and b. nodes 0=a,1=b,2=edgenode.
        // edgenode→a (minlen0,w1), edgenode→b (minlen0,w1). No separation.
        // Both pulled to the edge node → equal rank (x).
        let r = solve(3, &[(2, 0, 0.0, 1.0), (2, 1, 0.0, 1.0)]);
        assert!((r[0] - r[1]).abs() < 1e-6, "a,b should align: {r:?}");
    }
}
