//! Sparse orthogonal visibility grid.
//!
//! Grid lines are placed exactly at each obstacle's clearance-expanded
//! boundary (plus the query endpoints), so every grid cell is either
//! *fully* inside or *fully* outside any given obstacle — no partial-
//! overlap ambiguity, and no need for segment/box clipping math to
//! decide whether a grid edge is blocked.

use crate::layout::pathplan::Box as Obstacle;

pub(super) struct Grid {
    pub xs: Vec<f64>,
    pub ys: Vec<f64>,
    /// `blocked_h[i][j]` = the horizontal edge from `(xs[i], ys[j])` to
    /// `(xs[i + 1], ys[j])` crosses an obstacle's interior.
    blocked_h: Vec<Vec<bool>>,
    /// `blocked_v[i][j]` = the vertical edge from `(xs[i], ys[j])` to
    /// `(xs[i], ys[j + 1])` crosses an obstacle's interior.
    blocked_v: Vec<Vec<bool>>,
}

fn sorted_unique(mut v: Vec<f64>) -> Vec<f64> {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    v
}

impl Grid {
    pub fn build(extra_points: &[(f64, f64)], obstacles: &[Obstacle], clearance: f64) -> Grid {
        let mut xs: Vec<f64> = Vec::with_capacity(obstacles.len() * 2 + extra_points.len());
        let mut ys: Vec<f64> = Vec::with_capacity(obstacles.len() * 2 + extra_points.len());
        for ob in obstacles {
            xs.push(ob.min.x - clearance);
            xs.push(ob.max.x + clearance);
            ys.push(ob.min.y - clearance);
            ys.push(ob.max.y + clearance);
        }
        for &(x, y) in extra_points {
            xs.push(x);
            ys.push(y);
        }
        let xs = sorted_unique(xs);
        let ys = sorted_unique(ys);

        let nx = xs.len();
        let ny = ys.len();
        let mut blocked_h = vec![vec![false; ny]; nx.saturating_sub(1)];
        let mut blocked_v = vec![vec![false; ny.saturating_sub(1)]; nx];

        for ob in obstacles {
            let (bx0, bx1) = (ob.min.x - clearance, ob.max.x + clearance);
            let (by0, by1) = (ob.min.y - clearance, ob.max.y + clearance);
            for i in 0..nx.saturating_sub(1) {
                let mid_x = (xs[i] + xs[i + 1]) / 2.0;
                if mid_x <= bx0 || mid_x >= bx1 {
                    continue;
                }
                for (j, &y) in ys.iter().enumerate() {
                    if y > by0 && y < by1 {
                        blocked_h[i][j] = true;
                    }
                }
            }
            for j in 0..ny.saturating_sub(1) {
                let mid_y = (ys[j] + ys[j + 1]) / 2.0;
                if mid_y <= by0 || mid_y >= by1 {
                    continue;
                }
                for (i, &x) in xs.iter().enumerate() {
                    if x > bx0 && x < bx1 {
                        blocked_v[i][j] = true;
                    }
                }
            }
        }

        Grid {
            xs,
            ys,
            blocked_h,
            blocked_v,
        }
    }

    pub fn index_of(coords: &[f64], v: f64) -> Option<usize> {
        coords
            .iter()
            .position(|&c| (c - v).abs() < 1e-6)
    }

    pub fn horizontal_open(&self, i: usize, j: usize) -> bool {
        !self.blocked_h[i][j]
    }

    pub fn vertical_open(&self, i: usize, j: usize) -> bool {
        !self.blocked_v[i][j]
    }

    pub fn width(&self) -> usize {
        self.xs.len()
    }

    pub fn height(&self) -> usize {
        self.ys.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::geometry::Point;

    #[test]
    fn single_obstacle_blocks_crossing_edges() {
        let ob = Obstacle::new(Point::new(10.0, 10.0), Point::new(20.0, 20.0));
        let grid = Grid::build(&[(0.0, 15.0), (30.0, 15.0)], &[ob], 2.0);
        // xs should include 0, 8 (10-2), 22 (20+2), 30.
        assert!(grid.xs.contains(&0.0));
        assert!(grid.xs.contains(&8.0));
        assert!(grid.xs.contains(&22.0));
        assert!(grid.xs.contains(&30.0));
        // The horizontal edge crossing the obstacle's x-span at y=15
        // (strictly inside [8,22]) must be blocked.
        let i = Grid::index_of(&grid.xs, 8.0).unwrap();
        let j = Grid::index_of(&grid.ys, 15.0).unwrap();
        assert!(!grid.horizontal_open(i, j));
    }
}
