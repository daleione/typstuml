//! Compound-graph rank assignment (Sander 1996, simplified) — the real
//! M7 aspect-ratio fix, replacing the earlier post-hoc gap-folding
//! attempt (tried and reverted; see
//! docs/cuca-architecture-layout-redesign.md's M7 write-up for why
//! that approach was unsound).
//!
//! The flat longest-path ranker assigns every node a *global* rank in
//! one pass, so a long chain inside one cluster inflates the rank
//! *numbers* used by every sibling cluster too — a cluster whose own
//! members would fit in 2 ranks can end up spanning 6 because
//! unrelated content elsewhere in the diagram occupies the ranks in
//! between. This module ranks each cluster's own sub-DAG in
//! isolation (bottom-up, so a cluster's height only reflects its own
//! content), then ranks the top-level graph treating each cluster as
//! a single compound item with a fixed duration (its own height),
//! then pushes the resulting per-cluster start ranks back down so
//! every real node's global rank = the sum of its ancestor clusters'
//! local starts plus its own innermost local rank.
//!
//! This is deliberately *not* full Sander: cross-cluster edges are
//! rank-constrained at the cluster level (an edge from anywhere in A
//! to anywhere in B requires B's start rank to clear A's *end* rank,
//! not just the specific member's rank) rather than through per-edge
//! border-node proxies. That's a conservative approximation — it can
//! occasionally reserve one rank gap more than strictly necessary
//! between two clusters — but it can never violate edge ordering, and
//! it's what actually fixes the aspect-ratio problem (a cluster's own
//! extent no longer depends on what other clusters are doing).

use std::collections::{HashMap, HashSet, VecDeque};

use crate::layout::dag::{NodeHandle, DAG};
use crate::layout::sugiyama::hierarchy::{ClusterId, HierarchyMap};

/// A direct member of some scope (a cluster, or the virtual top-level
/// scope): either a real node or a child cluster (already ranked, with
/// a known height, by the time it's used as an `Item`).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Item {
    Node(NodeHandle),
    Cluster(ClusterId),
}

/// One scope's ranking result: how many ranks its own content spans,
/// and each direct item's rank *local* to this scope (0-based).
struct SubRank {
    height: usize,
    local_rank: HashMap<Item, usize>,
}

/// Result of compound ranking: the per-node global ranks, plus every
/// real edge the ranker had to *reverse for layout* (returned in its
/// original direction). Reversal is how a collapse-only cycle is
/// resolved — same as ELK's cycle breaker and `normalize_dag`'s
/// back-edge handling one level down: the losing edge is drawn
/// pointing "up" instead of dragging its endpoint's whole cluster
/// apart. The caller must flip the corresponding entries in its own
/// edge endpoint lists (the `dag` itself is already updated in
/// place).
pub(crate) struct ClusterRankResult {
    pub ranks: Vec<usize>,
    pub reversed: Vec<(NodeHandle, NodeHandle)>,
}

/// Compute a global rank for every node in `dag`, honoring
/// `hierarchy`'s cluster structure. Ranks are 0-based and dense
/// enough to feed straight into `DAG::set_node_levels`. With an empty
/// hierarchy this reduces to ordinary critical-path longest-path
/// ranking over the whole graph (every "cluster" duration is 1, so
/// the algorithm below degenerates to the flat case automatically).
///
/// Cross-cluster edges that close a collapse-only cycle (e.g. a
/// diagram whose packages form `A → B → C → A` even though the node
/// graph is acyclic) cannot all point downward without stretching
/// some cluster across the whole diagram. The earlier design "fixed"
/// those by pushing the violated edge's destination node forward
/// after ranking — which moved *one member* of a cluster while its
/// siblings stayed put, re-inflating the cluster's rank span and
/// undoing exactly what this module exists to do. Instead, the losing
/// edges are now reversed in `dag` (and reported in
/// [`ClusterRankResult::reversed`]) so every cluster keeps its
/// compact span and the edge is simply drawn upward.
pub(crate) fn compute(dag: &mut DAG, hierarchy: &HierarchyMap) -> ClusterRankResult {
    // Primary cycle breaking: ELK-style weighted greedy per scope
    // (see `break_scope_cycles`). After this the per-scope item graphs
    // are acyclic, so the drop-and-flip loop below is a pure backstop.
    let mut reversed: Vec<(NodeHandle, NodeHandle)> = break_scope_cycles(dag, hierarchy);
    let mut flipped_pairs: HashSet<(usize, usize)> = reversed
        .iter()
        .map(|(u, v)| {
            (u.get_index().min(v.get_index()), u.get_index().max(v.get_index()))
        })
        .collect();
    let mut ranks;
    // Each iteration either converges (no dropped constraints) or
    // flips at least one edge; the pair-set guard makes flips finite.
    // The cap is a safety net far above any real diagram's cycle
    // count.
    let mut guard = 0usize;
    loop {
        let (r, dropped) = compute_once(dag, hierarchy);
        ranks = r;
        guard += 1;
        if dropped.is_empty() || guard > 64 {
            break;
        }
        // Group parallel duplicates so two edges u→v flip together
        // (handling them one-by-one would flip the first and "restore"
        // the second, leaving one edge in each direction — a cycle).
        let mut groups: HashMap<(usize, usize), (NodeHandle, NodeHandle, usize)> = HashMap::new();
        for (u, v) in dropped {
            groups
                .entry((u.get_index(), v.get_index()))
                .and_modify(|g| g.2 += 1)
                .or_insert((u, v, 1));
        }
        for &(u, v, count) in groups.values() {
            for _ in 0..count {
                dag.remove_edge(u, v);
            }
        }
        let mut flipped_any = false;
        for (u, v, count) in groups.into_values() {
            let key =
                (u.get_index().min(v.get_index()), u.get_index().max(v.get_index()));
            // Keep the original direction when this pair was already
            // flipped once (never oscillate), or when a longer path
            // still forces u before v — reversing would create a real
            // node-level cycle, and the surviving path enforces the
            // rank order anyway.
            let keep = flipped_pairs.contains(&key) || dag.is_reachable(u, v);
            for _ in 0..count {
                if keep {
                    dag.add_edge(u, v);
                } else {
                    dag.add_edge(v, u);
                    reversed.push((u, v));
                }
            }
            if !keep {
                flipped_pairs.insert(key);
                flipped_any = true;
            }
        }
        if !flipped_any {
            break;
        }
    }

    // Last-resort backstop for the paths above that keep an edge
    // without proof of an enforcing longer path (oscillation guard,
    // iteration cap): one forward relaxation sweep in real topological
    // order repairs any remaining `rank[dst] <= rank[src]` violation.
    // In the normal converged case this finds nothing and changes
    // nothing — cluster compactness is preserved.
    let topo_index = topo_index(dag);
    let mut order: Vec<usize> = (0..dag.len()).collect();
    order.sort_by_key(|&i| topo_index[i]);
    for i in order {
        let h = NodeHandle::new(i);
        for &succ in dag.successors(h) {
            let s = succ.get_index();
            if ranks[s] <= ranks[i] {
                ranks[s] = ranks[i] + 1;
            }
        }
    }

    if std::env::var("TYPSTUML_DEBUG_RANKS").is_ok() {
        eprintln!("cluster_rank: ranks={ranks:?}");
        eprintln!(
            "cluster_rank: reversed={:?}",
            reversed.iter().map(|(u, v)| (u.get_index(), v.get_index())).collect::<Vec<_>>()
        );
    }
    ClusterRankResult { ranks, reversed }
}

/// ELK-parity cycle breaking: run Eades–Lin–Smyth greedy cycle
/// breaking on every scope's item multigraph (each cluster's direct
/// members, plus the virtual root scope), *weighted by real edge
/// count* — exactly what ELK's `GreedyCycleBreaker` sees, since it
/// counts parallel edges individually. The weighting matters for
/// which edge loses: a package with fan-out 3 and fan-in 1 has the
/// largest outflow−inflow and is pulled to the front of the order, so
/// the back-edge lands on its *incoming* side (on the reference
/// architecture diagram: `REST → api` gets reversed and the REST
/// interface sinks to the bottom next to its caller, matching
/// elkjs/draw-uml), instead of on whatever edge happened to close the
/// cycle last in topological order.
///
/// Ties on outflow−inflow are broken by model order (the smallest
/// original node index inside the item — declaration order), the same
/// spirit as ELK's `considerModelOrder`.
///
/// Every real edge behind a backward item-edge is reversed in `dag`
/// and reported. Scopes are independent (each real edge belongs to
/// exactly one scope — its endpoints' LCA), and after this pass every
/// scope's item graph is acyclic, which also keeps the node-level dag
/// acyclic: any node-level cycle would have to leave some item at its
/// outermost crossing scope and come back, but all crossing edges
/// there now point forward in that scope's linear order.
fn break_scope_cycles(dag: &mut DAG, hierarchy: &HierarchyMap) -> Vec<(NodeHandle, NodeHandle)> {
    let n = dag.len();
    let direct_owner: Vec<Option<ClusterId>> =
        (0..n).map(|i| hierarchy.cluster_of(NodeHandle::new(i))).collect();

    let mut scopes: Vec<Vec<Item>> = Vec::new();
    scopes.push(
        (0..n)
            .filter(|&i| direct_owner[i].is_none())
            .map(|i| Item::Node(NodeHandle::new(i)))
            .chain(
                (0..hierarchy.clusters.len())
                    .filter(|&c| hierarchy.clusters[c].parent.is_none())
                    .map(Item::Cluster),
            )
            .collect(),
    );
    for c in 0..hierarchy.clusters.len() {
        scopes.push(
            hierarchy.clusters[c]
                .direct_nodes
                .iter()
                .map(|&h| Item::Node(h))
                .chain(hierarchy.clusters[c].direct_children.iter().map(|&ch| Item::Cluster(ch)))
                .collect(),
        );
    }

    let mut reversed: Vec<(NodeHandle, NodeHandle)> = Vec::new();
    for items in scopes {
        let m = items.len();
        if m < 2 {
            continue;
        }
        let index_of: HashMap<Item, usize> =
            items.iter().enumerate().map(|(i, &it)| (it, i)).collect();
        let owning_item = |h: NodeHandle| -> Option<usize> {
            if let Some(&i) = index_of.get(&Item::Node(h)) {
                return Some(i);
            }
            let mut cur = direct_owner[h.get_index()];
            while let Some(c) = cur {
                if let Some(&i) = index_of.get(&Item::Cluster(c)) {
                    return Some(i);
                }
                cur = hierarchy.clusters[c].parent;
            }
            None
        };

        // Item multigraph: real edges grouped per ordered item pair.
        let mut real: HashMap<(usize, usize), Vec<(NodeHandle, NodeHandle)>> = HashMap::new();
        for (a, &it) in items.iter().enumerate() {
            for h in nodes_under(it, hierarchy) {
                for &succ in dag.successors(h) {
                    let Some(b) = owning_item(succ) else {
                        continue;
                    };
                    if a != b {
                        real.entry((a, b)).or_default().push((h, succ));
                    }
                }
            }
        }
        if real.is_empty() {
            continue;
        }

        // Model-order key: smallest original node index inside the item.
        let model_key: Vec<usize> = items
            .iter()
            .map(|&it| {
                nodes_under(it, hierarchy).iter().map(|h| h.get_index()).min().unwrap_or(usize::MAX)
            })
            .collect();

        let pos = els_positions(m, &real, &model_key);

        // Reverse every real edge behind a backward item-edge; sort
        // pairs for a deterministic reversal report.
        let mut pairs: Vec<&(usize, usize)> = real.keys().collect();
        pairs.sort();
        for &&(a, b) in &pairs {
            if pos[a] > pos[b] {
                for &(u, v) in &real[&(a, b)] {
                    dag.remove_edge(u, v);
                    dag.add_edge(v, u);
                    reversed.push((u, v));
                }
            }
        }
    }
    reversed
}

/// Eades–Lin–Smyth greedy linear arrangement over `m` items with
/// weighted edges (`weight = number of real edges`): repeatedly peel
/// sinks to the right end and sources to the left end; when neither
/// exists (a cycle), move the item with maximum outflow−inflow to the
/// left end (ties: smallest `model_key`). Returns each item's
/// position; edges pointing right-to-left in this order are the ones
/// to reverse.
fn els_positions(
    m: usize,
    real: &HashMap<(usize, usize), Vec<(NodeHandle, NodeHandle)>>,
    model_key: &[usize],
) -> Vec<usize> {
    let mut out_w = vec![0usize; m];
    let mut in_w = vec![0usize; m];
    let mut out_adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); m];
    let mut in_adj: Vec<Vec<(usize, usize)>> = vec![Vec::new(); m];
    for (&(a, b), edges) in real {
        let w = edges.len();
        out_w[a] += w;
        in_w[b] += w;
        out_adj[a].push((b, w));
        in_adj[b].push((a, w));
    }

    let mut alive = vec![true; m];
    let mut left: Vec<usize> = Vec::new();
    let mut right: Vec<usize> = Vec::new(); // built back-to-front
    let mut remaining = m;
    let remove = |u: usize,
                      alive: &mut Vec<bool>,
                      out_w: &mut Vec<usize>,
                      in_w: &mut Vec<usize>| {
        alive[u] = false;
        for &(y, w) in &out_adj[u] {
            if alive[y] {
                in_w[y] -= w;
            }
        }
        for &(x, w) in &in_adj[u] {
            if alive[x] {
                out_w[x] -= w;
            }
        }
    };

    while remaining > 0 {
        let mut progressed = true;
        while progressed {
            progressed = false;
            // Sinks (including isolated items) peel to the right end.
            for u in 0..m {
                if alive[u] && out_w[u] == 0 {
                    remove(u, &mut alive, &mut out_w, &mut in_w);
                    right.push(u);
                    remaining -= 1;
                    progressed = true;
                }
            }
            // Sources peel to the left end.
            for u in 0..m {
                if alive[u] && in_w[u] == 0 && out_w[u] > 0 {
                    remove(u, &mut alive, &mut out_w, &mut in_w);
                    left.push(u);
                    remaining -= 1;
                    progressed = true;
                }
            }
        }
        if remaining == 0 {
            break;
        }
        // A cycle: take the item with max outflow−inflow (ELK's greedy
        // choice), ties by model order.
        let u = (0..m)
            .filter(|&u| alive[u])
            .max_by_key(|&u| (out_w[u] as i64 - in_w[u] as i64, std::cmp::Reverse(model_key[u])))
            .unwrap();
        remove(u, &mut alive, &mut out_w, &mut in_w);
        left.push(u);
        remaining -= 1;
    }

    let mut pos = vec![0usize; m];
    for (p, &u) in left.iter().chain(right.iter().rev()).enumerate() {
        pos[u] = p;
    }
    pos
}

/// One full bottom-up + push-down ranking pass over the current state
/// of `dag`. Returns the ranks plus every real edge whose item-level
/// constraint had to be dropped because it would close a collapse-only
/// cycle (see [`compute`], which decides what to do about them).
fn compute_once(dag: &DAG, hierarchy: &HierarchyMap) -> (Vec<usize>, Vec<(NodeHandle, NodeHandle)>) {
    let n = dag.len();
    let direct_owner: Vec<Option<ClusterId>> =
        (0..n).map(|i| hierarchy.cluster_of(NodeHandle::new(i))).collect();

    // A cluster's own internal chain is acyclic (it's a sub-DAG of the
    // whole), but *collapsing* a cluster into one item for its
    // parent's ranking can still introduce a cycle that doesn't exist
    // at the node level: if a cluster has both an outgoing edge (to
    // node X) and, further along some other path, an incoming edge
    // (from a node reachable from X), the two collapsed item-edges
    // point at each other. `topo_index` lets `rank_items` break such
    // cycles the same way `normalize_dag` breaks an ordinary
    // back-edge — by dropping whichever crossing edge would close the
    // loop, keeping the earlier one (in the *real*, guaranteed-acyclic
    // node order).
    let topo_index = topo_index(dag);

    let depths = cluster_depths(hierarchy);
    let max_depth = depths.iter().copied().max().unwrap_or(0);
    let mut sub_ranks: Vec<Option<SubRank>> = (0..hierarchy.clusters.len()).map(|_| None).collect();
    let mut dropped: Vec<(NodeHandle, NodeHandle)> = Vec::new();

    // Bottom-up: deepest clusters first, so a parent can treat its
    // already-ranked children as fixed-height compound items.
    for d in (0..=max_depth).rev() {
        for c in 0..hierarchy.clusters.len() {
            if depths[c] != d {
                continue;
            }
            let items: Vec<Item> = hierarchy.clusters[c]
                .direct_nodes
                .iter()
                .map(|&h| Item::Node(h))
                .chain(hierarchy.clusters[c].direct_children.iter().map(|&ch| Item::Cluster(ch)))
                .collect();
            let (sub, drops) =
                rank_items(&items, dag, hierarchy, &direct_owner, &sub_ranks, &topo_index);
            sub_ranks[c] = Some(sub);
            dropped.extend(drops);
        }
    }

    // Virtual top-level scope: every node/cluster with no parent.
    let root_items: Vec<Item> = (0..n)
        .filter(|&i| direct_owner[i].is_none())
        .map(|i| Item::Node(NodeHandle::new(i)))
        .chain(
            (0..hierarchy.clusters.len())
                .filter(|&c| hierarchy.clusters[c].parent.is_none())
                .map(Item::Cluster),
        )
        .collect();
    let (root_result, root_drops) =
        rank_items(&root_items, dag, hierarchy, &direct_owner, &sub_ranks, &topo_index);
    dropped.extend(root_drops);

    // Push down: absolute start rank for every cluster, computed
    // top-down (ascending depth) now that each level's local ranks are
    // known from the bottom-up pass above.
    let mut cluster_start: Vec<usize> = vec![0; hierarchy.clusters.len()];
    for &item in &root_items {
        if let Item::Cluster(c) = item {
            cluster_start[c] = root_result.local_rank[&item];
        }
    }
    for d in 0..=max_depth {
        for c in 0..hierarchy.clusters.len() {
            if depths[c] != d {
                continue;
            }
            let Some(parent) = hierarchy.clusters[c].parent else {
                continue; // top-level cluster: start already set above
            };
            let parent_start = cluster_start[parent];
            let local = sub_ranks[parent].as_ref().unwrap().local_rank[&Item::Cluster(c)];
            cluster_start[c] = parent_start + local;
        }
    }

    let mut ranks = vec![0usize; n];
    for (i, rank) in ranks.iter_mut().enumerate() {
        let h = NodeHandle::new(i);
        *rank = match direct_owner[i] {
            None => root_result.local_rank[&Item::Node(h)],
            Some(c) => cluster_start[c] + sub_ranks[c].as_ref().unwrap().local_rank[&Item::Node(h)],
        };
    }

    (ranks, dropped)
}

/// Critical-path ranking shared by every scope (a cluster's direct
/// members, or the virtual top-level scope): build an item-level DAG
/// from real edges that cross between two *different* items of this
/// scope (found by walking each endpoint up its cluster ancestor
/// chain until it lands on one of `items`), give each item a
/// "duration" (1 for a real node, or its precomputed height for a
/// cluster), and rank via duration-aware longest path (critical-path
/// scheduling: `rank[v] = max over predecessors u of rank[u] +
/// duration[u]`).
fn rank_items(
    items: &[Item],
    dag: &DAG,
    hierarchy: &HierarchyMap,
    direct_owner: &[Option<ClusterId>],
    sub_ranks: &[Option<SubRank>],
    topo_index: &[usize],
) -> (SubRank, Vec<(NodeHandle, NodeHandle)>) {
    let index_of: HashMap<Item, usize> = items.iter().enumerate().map(|(i, &it)| (it, i)).collect();
    let duration = |it: Item| -> usize {
        match it {
            Item::Node(_) => 1,
            Item::Cluster(c) => sub_ranks[c].as_ref().unwrap().height,
        }
    };
    let owning_item = |h: NodeHandle| -> Option<Item> {
        if index_of.contains_key(&Item::Node(h)) {
            return Some(Item::Node(h));
        }
        let mut cur = direct_owner[h.get_index()];
        while let Some(c) = cur {
            if index_of.contains_key(&Item::Cluster(c)) {
                return Some(Item::Cluster(c));
            }
            cur = hierarchy.clusters[c].parent;
        }
        None
    };

    let m = items.len();
    // Collect every candidate item-edge first (deduped), tagged with
    // the *real* node's global topo position — collapsing a cluster
    // to one item can manufacture a cycle that doesn't exist at the
    // node level (see `compute`'s comment on `topo_index`), so edges
    // are added in real-topological order and any candidate that
    // would close a cycle (its target already reaches its source) is
    // dropped, mirroring how `normalize_dag` drops/reverses an
    // ordinary back-edge one level down.
    let mut candidates: Vec<(usize, usize, usize)> = Vec::new();
    let mut seen_edges: HashSet<(usize, usize)> = HashSet::new();
    // Every real edge behind each item pair, so a dropped constraint
    // can be reported (and reversed) as its concrete node-level edges.
    let mut real_edges_of: HashMap<(usize, usize), Vec<(NodeHandle, NodeHandle)>> = HashMap::new();
    for (a, &it) in items.iter().enumerate() {
        for h in nodes_under(it, hierarchy) {
            for &succ in dag.successors(h) {
                let Some(target_item) = owning_item(succ) else {
                    continue;
                };
                let b = index_of[&target_item];
                if a != b {
                    real_edges_of.entry((a, b)).or_default().push((h, succ));
                    if seen_edges.insert((a, b)) {
                        candidates.push((a, b, topo_index[h.get_index()]));
                    }
                }
            }
        }
    }
    candidates.sort_by_key(|&(_, _, t)| t);

    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); m];
    let mut indeg = vec![0usize; m];
    let mut dropped: Vec<(NodeHandle, NodeHandle)> = Vec::new();
    for (a, b, _) in candidates {
        if reaches(&adj, b, a) {
            // Would close a collapse-only cycle — drop the constraint
            // and report its real edges for the caller to reverse.
            dropped.extend(real_edges_of[&(a, b)].iter().copied());
            continue;
        }
        adj[a].push(b);
        indeg[b] += 1;
    }

    let mut rank = vec![0usize; m];
    let mut indeg_mut = indeg.clone();
    let mut queue: VecDeque<usize> = (0..m).filter(|&i| indeg_mut[i] == 0).collect();
    let mut visited = 0usize;
    while let Some(u) = queue.pop_front() {
        visited += 1;
        let du = rank[u] + duration(items[u]);
        for &v in &adj[u] {
            if du > rank[v] {
                rank[v] = du;
            }
            indeg_mut[v] -= 1;
            if indeg_mut[v] == 0 {
                queue.push_back(v);
            }
        }
    }
    debug_assert_eq!(visited, m, "cluster_rank: item-level graph must be acyclic");

    let height = (0..m).map(|i| rank[i] + duration(items[i])).max().unwrap_or(0);
    let local_rank: HashMap<Item, usize> =
        items.iter().enumerate().map(|(i, &it)| (it, rank[i])).collect();
    (SubRank { height, local_rank }, dropped)
}

/// True iff `to` is reachable from `from` in the (partially built)
/// item adjacency. Callers use `reaches(&adj, b, a)` before adding a
/// candidate edge `a -> b`: if `b` already reaches `a`, adding `a ->
/// b` would close a cycle (`a -> b -> ... -> a`), so the candidate is
/// dropped instead.
fn reaches(adj: &[Vec<usize>], from: usize, to: usize) -> bool {
    if from == to {
        return true;
    }
    let mut seen = vec![false; adj.len()];
    let mut stack = vec![from];
    seen[from] = true;
    while let Some(u) = stack.pop() {
        for &v in &adj[u] {
            if v == to {
                return true;
            }
            if !seen[v] {
                seen[v] = true;
                stack.push(v);
            }
        }
    }
    false
}

/// Global topological order of every real node in `dag`, as a
/// `topo_index[node] = position` lookup. `dag` is guaranteed acyclic
/// by the time ranking runs (`normalize_dag` already reversed any
/// back-edges), so this always succeeds.
fn topo_index(dag: &DAG) -> Vec<usize> {
    let n = dag.len();
    let mut indeg = vec![0usize; n];
    for i in 0..n {
        for &s in dag.successors(NodeHandle::new(i)) {
            indeg[s.get_index()] += 1;
        }
    }
    let mut queue: VecDeque<usize> = (0..n).filter(|&i| indeg[i] == 0).collect();
    let mut index = vec![0usize; n];
    let mut pos = 0usize;
    while let Some(u) = queue.pop_front() {
        index[u] = pos;
        pos += 1;
        for &v in dag.successors(NodeHandle::new(u)) {
            let vi = v.get_index();
            indeg[vi] -= 1;
            if indeg[vi] == 0 {
                queue.push_back(vi);
            }
        }
    }
    debug_assert_eq!(pos, n, "dag must already be acyclic by rank time");
    index
}

/// Every real node handle inside `it` — itself for a node, or every
/// (possibly nested) descendant for a cluster.
fn nodes_under(it: Item, hierarchy: &HierarchyMap) -> Vec<NodeHandle> {
    match it {
        Item::Node(h) => vec![h],
        Item::Cluster(c) => {
            let mut v = hierarchy.clusters[c].direct_nodes.clone();
            for &ch in &hierarchy.clusters[c].direct_children {
                v.extend(nodes_under(Item::Cluster(ch), hierarchy));
            }
            v
        }
    }
}

/// Depth of each cluster: 0 for top-level, 1 for direct children of a
/// top-level cluster, etc. (Same algorithm as
/// `tighten.rs::compute_depths`; duplicated locally since that one is
/// private to its module and this is a handful of lines.)
fn cluster_depths(hierarchy: &HierarchyMap) -> Vec<usize> {
    let n = hierarchy.clusters.len();
    let mut depths = vec![0; n];
    let mut changed = true;
    while changed {
        changed = false;
        for i in 0..n {
            if let Some(p) = hierarchy.clusters[i].parent {
                let d = depths[p] + 1;
                if depths[i] != d {
                    depths[i] = d;
                    changed = true;
                }
            }
        }
    }
    depths
}

#[cfg(test)]
mod tests {
    use super::*;

    fn linear_dag(n: usize) -> DAG {
        let mut dag = DAG::new();
        for _ in 0..n {
            dag.new_node();
        }
        for i in 0..n - 1 {
            dag.add_edge(NodeHandle::new(i), NodeHandle::new(i + 1));
        }
        dag
    }

    #[test]
    fn flat_graph_matches_plain_longest_path() {
        // A -> B -> C, no clusters: ranks should be 0, 1, 2.
        let mut dag = linear_dag(3);
        let hierarchy = HierarchyMap::new();
        let result = compute(&mut dag, &hierarchy);
        assert_eq!(result.ranks, vec![0, 1, 2]);
        assert!(result.reversed.is_empty());
    }

    #[test]
    fn cluster_chain_does_not_inflate_sibling_cluster() {
        // PkgA{A0 -> A1 -> A2 -> A3} (a long internal chain, needs 4
        // ranks) and PkgB{B0} with NO edges to/from PkgA at all. Under
        // the old flat ranking both would still get independent global
        // ranks trivially in this exact case (no shared edges), so
        // this test targets the real regression: a *root-level*
        // stranger between them must not stretch across A's span
        // artificially, and PkgB must get its own compact height (1),
        // not "however many ranks A happens to span".
        let mut dag = DAG::new();
        for _ in 0..5 {
            dag.new_node();
        }
        dag.add_edge(NodeHandle::new(0), NodeHandle::new(1));
        dag.add_edge(NodeHandle::new(1), NodeHandle::new(2));
        dag.add_edge(NodeHandle::new(2), NodeHandle::new(3));
        // node 4 = B0, no edges.

        let mut hierarchy = HierarchyMap::new();
        let pkg_a = hierarchy.add_cluster(None);
        let pkg_b = hierarchy.add_cluster(None);
        for i in 0..4 {
            hierarchy.assign_node(NodeHandle::new(i), pkg_a);
        }
        hierarchy.assign_node(NodeHandle::new(4), pkg_b);

        let ranks = compute(&mut dag, &hierarchy).ranks;
        // PkgA's chain: local ranks 0,1,2,3 (height 4).
        assert_eq!(ranks[0], 0);
        assert_eq!(ranks[1], 1);
        assert_eq!(ranks[2], 2);
        assert_eq!(ranks[3], 3);
        // PkgB has no edges to PkgA, so at the top level it's free to
        // rank at 0 alongside PkgA — it must NOT be forced to span
        // ranks 0..3 just because PkgA does.
        assert_eq!(ranks[4], 0);
    }

    #[test]
    fn cross_cluster_edge_orders_clusters_by_full_extent() {
        // PkgA{A0 -> A1} (height 2), edge A1 -> B0 (in PkgB). PkgB
        // must start at rank >= PkgA's *end* (2), not at A1's local
        // rank (1) — the whole cluster is treated as a block.
        let mut dag = DAG::new();
        for _ in 0..3 {
            dag.new_node();
        }
        dag.add_edge(NodeHandle::new(0), NodeHandle::new(1)); // A0 -> A1
        dag.add_edge(NodeHandle::new(1), NodeHandle::new(2)); // A1 -> B0

        let mut hierarchy = HierarchyMap::new();
        let pkg_a = hierarchy.add_cluster(None);
        let pkg_b = hierarchy.add_cluster(None);
        hierarchy.assign_node(NodeHandle::new(0), pkg_a);
        hierarchy.assign_node(NodeHandle::new(1), pkg_a);
        hierarchy.assign_node(NodeHandle::new(2), pkg_b);

        let ranks = compute(&mut dag, &hierarchy).ranks;
        assert_eq!(ranks[0], 0);
        assert_eq!(ranks[1], 1);
        assert_eq!(ranks[2], 2); // PkgA's height is 2, so PkgB starts at 2.
    }

    #[test]
    fn nested_cluster_offsets_compose() {
        // Outer{ Inner{ Leaf0 -> Leaf1 }, Other0 }, edge Other0 -> Leaf0
        // is impossible (would need same-cluster reverse); instead
        // test that Inner's local ranks land correctly offset inside
        // Outer when Outer also has a sibling direct node ranked
        // before Inner.
        let mut dag = DAG::new();
        for _ in 0..3 {
            dag.new_node();
        }
        dag.add_edge(NodeHandle::new(0), NodeHandle::new(1)); // Other0 -> Leaf0
        dag.add_edge(NodeHandle::new(1), NodeHandle::new(2)); // Leaf0 -> Leaf1

        let mut hierarchy = HierarchyMap::new();
        let outer = hierarchy.add_cluster(None);
        let inner = hierarchy.add_cluster(Some(outer)); // already wires outer.direct_children
        hierarchy.assign_node(NodeHandle::new(0), outer); // Other0 direct in Outer
        hierarchy.assign_node(NodeHandle::new(1), inner);
        hierarchy.assign_node(NodeHandle::new(2), inner);

        let ranks = compute(&mut dag, &hierarchy).ranks;
        // Inner's own local ranks: Leaf0=0, Leaf1=1 (height 2).
        // Outer's direct items: Other0 (duration 1), Inner (duration 2).
        // Edge Other0 -> Leaf0 crosses to Inner (Leaf0's owning item at
        // Outer's scope is Inner), so Inner must start after Other0:
        // Other0 local rank 0, Inner local rank >= 1.
        assert_eq!(ranks[0], 0); // Other0
        assert_eq!(ranks[1], 1); // Leaf0 = Outer-local Inner-start(1) + 0
        assert_eq!(ranks[2], 2); // Leaf1 = 1 + 1
    }

    #[test]
    fn collapse_only_cycle_is_broken_not_panicking() {
        // PkgA{A0}, PkgB{B0}: A0 -> B0 (leaves A, enters B) and,
        // further down a different path, C -> A0 where C is itself
        // reachable from B0 (B0 -> C -> A0). At the *node* level this
        // is a perfectly valid DAG (A0 -> B0 -> C -> A0 would be a
        // cycle, so it can't exist — instead build the real acyclic
        // shape: A0 -> B0, B0 -> C, and *C -> A1* where A1 is a
        // *different* PkgA member — collapsing PkgA to one item for
        // root-scope ranking still creates an item-level cycle
        // (PkgA -> PkgB -> C -> PkgA) purely from the collapse, since
        // A0 and A1 are different real nodes but the same item.
        let mut dag = DAG::new();
        for _ in 0..4 {
            dag.new_node();
        }
        dag.add_edge(NodeHandle::new(0), NodeHandle::new(1)); // A0 -> B0
        dag.add_edge(NodeHandle::new(1), NodeHandle::new(2)); // B0 -> C
        dag.add_edge(NodeHandle::new(2), NodeHandle::new(3)); // C -> A1

        let mut hierarchy = HierarchyMap::new();
        let pkg_a = hierarchy.add_cluster(None);
        let pkg_b = hierarchy.add_cluster(None);
        hierarchy.assign_node(NodeHandle::new(0), pkg_a); // A0
        hierarchy.assign_node(NodeHandle::new(3), pkg_a); // A1
        hierarchy.assign_node(NodeHandle::new(1), pkg_b); // B0
        // node 2 (C) stays root-level, unclustered.

        // Must not panic (the debug_assert in rank_items would fire on
        // an unbroken cycle); the losing edge C -> A1 must come back
        // as a layout reversal instead of dragging A1 below C (which
        // would stretch PkgA's rank span across the whole graph).
        let result = compute(&mut dag, &hierarchy);
        let ranks = &result.ranks;
        assert!(ranks[0] < ranks[1], "A0 must rank before B0");
        assert!(ranks[1] < ranks[2], "B0 must rank before C");
        assert_eq!(
            result.reversed,
            vec![(NodeHandle::new(2), NodeHandle::new(3))],
            "C -> A1 closes the item-level cycle and must be reversed"
        );
        // PkgA stays compact: A1 ranks alongside A0, not after C.
        assert_eq!(ranks[3], ranks[0]);
    }

    #[test]
    fn every_dag_edge_respects_rank_order_after_cycle_breaking() {
        // Same shape as the cycle test above, but assert the property
        // that actually matters for the Sugiyama pipeline downstream:
        // after `compute` returns, *every* edge left in the dag (which
        // now points each reversed edge in its layout direction) has
        // strictly increasing rank — that's what dummy insertion
        // requires.
        let mut dag = DAG::new();
        for _ in 0..4 {
            dag.new_node();
        }
        dag.add_edge(NodeHandle::new(0), NodeHandle::new(1));
        dag.add_edge(NodeHandle::new(1), NodeHandle::new(2));
        dag.add_edge(NodeHandle::new(2), NodeHandle::new(3));

        let mut hierarchy = HierarchyMap::new();
        let pkg_a = hierarchy.add_cluster(None);
        let pkg_b = hierarchy.add_cluster(None);
        hierarchy.assign_node(NodeHandle::new(0), pkg_a);
        hierarchy.assign_node(NodeHandle::new(3), pkg_a);
        hierarchy.assign_node(NodeHandle::new(1), pkg_b);

        let ranks = compute(&mut dag, &hierarchy).ranks;
        for i in 0..dag.len() {
            for &succ in dag.successors(NodeHandle::new(i)) {
                assert!(
                    ranks[succ.get_index()] > ranks[i],
                    "edge {i} -> {} must strictly increase rank; got {} -> {}",
                    succ.get_index(),
                    ranks[i],
                    ranks[succ.get_index()]
                );
            }
        }
    }
}
