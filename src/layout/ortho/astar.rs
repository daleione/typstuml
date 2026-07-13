//! A* shortest path over the orthogonal visibility grid, penalizing
//! direction changes so the result prefers long straight runs over
//! many small jogs (ELK's bend-minimizing orthogonal router, same
//! spirit).

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::layout::geometry::Point;

use super::grid::Grid;
use super::Dir;

#[derive(Clone, Copy)]
struct QueueEntry {
    cost: f64,
    state_idx: usize,
}

impl PartialEq for QueueEntry {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost
    }
}
impl Eq for QueueEntry {}
impl PartialOrd for QueueEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for QueueEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Reversed so BinaryHeap (a max-heap) pops the smallest cost.
        other.cost.partial_cmp(&self.cost).unwrap_or(Ordering::Equal)
    }
}

fn dir_between(ai: usize, aj: usize, bi: usize, bj: usize) -> Dir {
    if bi > ai {
        Dir::Right
    } else if bi < ai {
        Dir::Left
    } else if bj > aj {
        Dir::Down
    } else {
        Dir::Up
    }
}

/// `state_key` packs (i, j, dir) into a dense index over `dir_states =
/// 5` (4 directions + "no direction yet", used only at the start
/// node) so visited/cost arrays can be flat `Vec`s instead of a
/// hashmap.
fn dir_slot(dir: Option<Dir>) -> usize {
    match dir {
        None => 0,
        Some(Dir::Up) => 1,
        Some(Dir::Down) => 2,
        Some(Dir::Left) => 3,
        Some(Dir::Right) => 4,
    }
}
const DIR_SLOTS: usize = 5;

pub(super) fn shortest_path(
    grid: &Grid,
    start_i: usize,
    start_j: usize,
    end_i: usize,
    end_j: usize,
    bend_penalty: f64,
) -> Option<Vec<(usize, usize)>> {
    let nx = grid.width();
    let ny = grid.height();
    let n_states = nx * ny * DIR_SLOTS;
    let state_index = |i: usize, j: usize, dir: Option<Dir>| -> usize {
        (i * ny + j) * DIR_SLOTS + dir_slot(dir)
    };

    let mut dist = vec![f64::INFINITY; n_states];
    let mut prev: Vec<Option<usize>> = vec![None; n_states];
    let start_state = state_index(start_i, start_j, None);
    dist[start_state] = 0.0;

    let heuristic = |i: usize, j: usize| -> f64 {
        (grid.xs[i] - grid.xs[end_i]).abs() + (grid.ys[j] - grid.ys[end_j]).abs()
    };

    let mut heap = BinaryHeap::new();
    heap.push(QueueEntry {
        cost: heuristic(start_i, start_j),
        state_idx: start_state,
    });

    while let Some(QueueEntry { state_idx, .. }) = heap.pop() {
        let dir_s = state_idx % DIR_SLOTS;
        let ij = state_idx / DIR_SLOTS;
        let j = ij % ny;
        let i = ij / ny;
        let g = dist[state_idx];

        if i == end_i && j == end_j {
            return Some(reconstruct(prev, state_idx, ny));
        }

        let cur_dir = match dir_s {
            1 => Some(Dir::Up),
            2 => Some(Dir::Down),
            3 => Some(Dir::Left),
            4 => Some(Dir::Right),
            _ => None,
        };

        let mut neighbors: Vec<(usize, usize, f64)> = Vec::with_capacity(4);
        if i + 1 < nx && grid.horizontal_open(i, j) {
            neighbors.push((i + 1, j, grid.xs[i + 1] - grid.xs[i]));
        }
        if i > 0 && grid.horizontal_open(i - 1, j) {
            neighbors.push((i - 1, j, grid.xs[i] - grid.xs[i - 1]));
        }
        if j + 1 < ny && grid.vertical_open(i, j) {
            neighbors.push((i, j + 1, grid.ys[j + 1] - grid.ys[j]));
        }
        if j > 0 && grid.vertical_open(i, j - 1) {
            neighbors.push((i, j - 1, grid.ys[j] - grid.ys[j - 1]));
        }

        for (ni, nj, step_len) in neighbors {
            let nd = dir_between(i, j, ni, nj);
            let turn_cost = match cur_dir {
                Some(d) if d != nd => bend_penalty,
                _ => 0.0,
            };
            let ng = g + step_len + turn_cost;
            let nstate = state_index(ni, nj, Some(nd));
            if ng < dist[nstate] {
                dist[nstate] = ng;
                prev[nstate] = Some(state_idx);
                heap.push(QueueEntry {
                    cost: ng + heuristic(ni, nj),
                    state_idx: nstate,
                });
            }
        }
    }
    None
}

fn reconstruct(prev: Vec<Option<usize>>, end_state: usize, ny: usize) -> Vec<(usize, usize)> {
    let mut path = Vec::new();
    let mut cur = Some(end_state);
    while let Some(s) = cur {
        let ij = s / DIR_SLOTS;
        let j = ij % ny;
        let i = ij / ny;
        path.push((i, j));
        cur = prev[s];
    }
    path.reverse();
    path
}

pub(super) fn to_points(grid: &Grid, path: &[(usize, usize)]) -> Vec<Point> {
    path.iter()
        .map(|&(i, j)| Point::new(grid.xs[i], grid.ys[j]))
        .collect()
}
