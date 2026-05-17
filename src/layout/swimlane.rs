//! Swimlane activity layout — consumes `swimlane-probe` measurements
//! (one per action, plus one per lane label) and produces the lane
//! columns, per-node placements, and polyline edges that the
//! `swimlane-layout` Typst painter consumes.
//!
//! Approach mirrors PlantUML's swimlane model (one global tree, lane
//! membership stamped per leaf, cross-lane connectors as a "snake" of
//! down → across → down) but resolves all geometry in Rust before
//! emitting any Typst — so the painter never has to know about
//! interceptors or per-lane translates.
//!
//! Only the linear case (top-level actions and markers, optionally with
//! intra-lane compound constructs like `if`/`while` that stay within a
//! single lane) is handled here. A `|lane|` switch *inside* a nested
//! `if`/`while`/`fork` body is left for a follow-up — those will emit
//! today as a dropped switch via the existing recursive emit path.
//!
//! Units throughout: typographic points (Typst's `pt`).

use crate::runtime::MeasurementSet;

#[derive(Debug, Clone)]
pub struct LaneInput {
    pub label: String,
    pub color: Option<String>,
}

#[derive(Debug, Clone)]
pub struct NodeInput {
    pub probe_id: String,
    pub lane: usize,
}

#[derive(Debug, Clone)]
pub struct LaneRect {
    pub label: String,
    pub color: Option<String>,
    pub x_pt: f64,
    pub width_pt: f64,
}

#[derive(Debug, Clone)]
pub struct NodePlacement {
    pub x_pt: f64,
    pub y_pt: f64,
    pub width_pt: f64,
    pub height_pt: f64,
    pub lane: usize,
}

#[derive(Debug, Clone)]
pub struct EdgePoly {
    pub points: Vec<(f64, f64)>,
    pub arrow: bool,
}

#[derive(Debug, Clone)]
pub struct SwimlaneLayout {
    pub lanes: Vec<LaneRect>,
    pub nodes: Vec<NodePlacement>,
    pub edges: Vec<EdgePoly>,
    pub header_h_pt: f64,
    pub body_h_pt: f64,
}

const MIN_LANE_WIDTH_PT: f64 = 110.0;
const LANE_SIDE_PAD_PT: f64 = 16.0;
const HEADER_HEIGHT_PT: f64 = 28.0;
const INTRA_GAP_PT: f64 = 22.0;
const CROSS_GAP_PT: f64 = 36.0;
const BODY_TOP_PAD_PT: f64 = 12.0;
const BODY_BOT_PAD_PT: f64 = 12.0;
const FALLBACK_NODE_W_PT: f64 = 80.0;
const FALLBACK_NODE_H_PT: f64 = 24.0;

pub fn solve(
    lanes_in: &[LaneInput],
    nodes_in: &[NodeInput],
    lane_label_probe_ids: &[Option<String>],
    measurements: &MeasurementSet,
) -> SwimlaneLayout {
    // Per-lane width = max(label width, max node width across the lane)
    // + side padding, clamped to MIN_LANE_WIDTH_PT.
    let lane_widths: Vec<f64> = (0..lanes_in.len())
        .map(|i| {
            let label_w = lane_label_probe_ids
                .get(i)
                .and_then(|id_opt| id_opt.as_ref())
                .and_then(|id| measurements.get(id))
                .map(|m| m.width_pt)
                .unwrap_or(0.0);
            let content_w = nodes_in
                .iter()
                .filter(|n| n.lane == i)
                .filter_map(|n| measurements.get(&n.probe_id))
                .map(|m| m.width_pt)
                .fold(0.0_f64, f64::max);
            (label_w.max(content_w) + 2.0 * LANE_SIDE_PAD_PT).max(MIN_LANE_WIDTH_PT)
        })
        .collect();

    let mut lane_xs = Vec::with_capacity(lanes_in.len());
    let mut cursor = 0.0;
    for &w in &lane_widths {
        lane_xs.push(cursor);
        cursor += w;
    }

    let lane_rects: Vec<LaneRect> = lanes_in
        .iter()
        .enumerate()
        .map(|(i, lane)| LaneRect {
            label: lane.label.clone(),
            color: lane.color.clone(),
            x_pt: lane_xs[i],
            width_pt: lane_widths[i],
        })
        .collect();

    // Stack nodes vertically in global source order. Cross-lane
    // transitions get a wider vertical gap so the horizontal arm of the
    // snake doesn't crowd either endpoint.
    let mut placements: Vec<NodePlacement> = Vec::with_capacity(nodes_in.len());
    let mut y = BODY_TOP_PAD_PT;
    for (i, n) in nodes_in.iter().enumerate() {
        let m = measurements.get(&n.probe_id);
        let w = m.as_ref().map(|m| m.width_pt).unwrap_or(FALLBACK_NODE_W_PT);
        let h = m.as_ref().map(|m| m.height_pt).unwrap_or(FALLBACK_NODE_H_PT);
        if i > 0 {
            let prev_lane = nodes_in[i - 1].lane;
            y += if prev_lane != n.lane { CROSS_GAP_PT } else { INTRA_GAP_PT };
        }
        let lane_center = lane_xs[n.lane] + lane_widths[n.lane] / 2.0;
        placements.push(NodePlacement {
            x_pt: lane_center - w / 2.0,
            y_pt: y,
            width_pt: w,
            height_pt: h,
            lane: n.lane,
        });
        y += h;
    }
    let body_h_pt = y + BODY_BOT_PAD_PT;

    // Edges: one per consecutive pair. Intra-lane is a 2-pt straight
    // line; cross-lane is the canonical PlantUML 4-pt snake.
    let mut edges = Vec::with_capacity(placements.len().saturating_sub(1));
    for i in 1..placements.len() {
        let prev = &placements[i - 1];
        let next = &placements[i];
        let prev_x = prev.x_pt + prev.width_pt / 2.0;
        let prev_y = prev.y_pt + prev.height_pt;
        let next_x = next.x_pt + next.width_pt / 2.0;
        let next_y = next.y_pt;
        if prev.lane == next.lane {
            edges.push(EdgePoly {
                points: vec![(prev_x, prev_y), (next_x, next_y)],
                arrow: true,
            });
        } else {
            let mid_y = (prev_y + next_y) / 2.0;
            edges.push(EdgePoly {
                points: vec![
                    (prev_x, prev_y),
                    (prev_x, mid_y),
                    (next_x, mid_y),
                    (next_x, next_y),
                ],
                arrow: true,
            });
        }
    }

    SwimlaneLayout {
        lanes: lane_rects,
        nodes: placements,
        edges,
        header_h_pt: HEADER_HEIGHT_PT,
        body_h_pt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::Measurement;

    fn ms() -> MeasurementSet {
        let mut set = MeasurementSet::default();
        set.insert("n0".into(), Measurement::new(60.0, 20.0));
        set.insert("n1".into(), Measurement::new(80.0, 24.0));
        set.insert("n2".into(), Measurement::new(70.0, 24.0));
        set.insert("lane0".into(), Measurement::new(50.0, 14.0));
        set.insert("lane1".into(), Measurement::new(45.0, 14.0));
        set
    }

    #[test]
    fn lane_revisit_consolidates_columns() {
        let lanes = vec![
            LaneInput { label: "A".into(), color: None },
            LaneInput { label: "B".into(), color: None },
        ];
        let nodes = vec![
            NodeInput { probe_id: "n0".into(), lane: 0 },
            NodeInput { probe_id: "n1".into(), lane: 1 },
            NodeInput { probe_id: "n2".into(), lane: 0 },
        ];
        let labels = vec![Some("lane0".into()), Some("lane1".into())];
        let layout = solve(&lanes, &nodes, &labels, &ms());
        assert_eq!(layout.lanes.len(), 2);
        // 3rd node is back in lane 0 — its x should re-use lane 0's centre.
        let lane0_center = layout.lanes[0].x_pt + layout.lanes[0].width_pt / 2.0;
        let third_center = layout.nodes[2].x_pt + layout.nodes[2].width_pt / 2.0;
        assert!((third_center - lane0_center).abs() < 0.001);
    }

    #[test]
    fn cross_lane_edge_is_a_snake() {
        let lanes = vec![
            LaneInput { label: "A".into(), color: None },
            LaneInput { label: "B".into(), color: None },
        ];
        let nodes = vec![
            NodeInput { probe_id: "n0".into(), lane: 0 },
            NodeInput { probe_id: "n1".into(), lane: 1 },
        ];
        let labels = vec![Some("lane0".into()), Some("lane1".into())];
        let layout = solve(&lanes, &nodes, &labels, &ms());
        assert_eq!(layout.edges.len(), 1);
        assert_eq!(layout.edges[0].points.len(), 4);
    }

    #[test]
    fn intra_lane_edge_is_straight() {
        let lanes = vec![LaneInput { label: "A".into(), color: None }];
        let nodes = vec![
            NodeInput { probe_id: "n0".into(), lane: 0 },
            NodeInput { probe_id: "n1".into(), lane: 0 },
        ];
        let labels = vec![Some("lane0".into())];
        let layout = solve(&lanes, &nodes, &labels, &ms());
        assert_eq!(layout.edges[0].points.len(), 2);
    }
}
