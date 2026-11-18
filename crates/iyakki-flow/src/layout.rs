//! Automatic placement of nodes: columns by depth along the longest path, rows within a column in the flow's node order.
//! The canvas uses it for "arrange", the assistant for the flows it builds, and an imported flow without positions.

use std::collections::BTreeMap;

use crate::model::*;

pub const COL_W: f32 = 240.0;
pub const ROW_H: f32 = 120.0;
pub const MARGIN: f32 = 40.0;

/// The depth (longest path from a root) of every node; a cycle's members get the depth they were reached at.
pub fn depths(flow: &Flow) -> BTreeMap<String, usize> {
    let mut depth: BTreeMap<String, usize> = flow.nodes.iter().map(|n| (n.id.clone(), 0)).collect();
    for _ in 0..flow.nodes.len().max(1) {
        let mut changed = false;
        for e in &flow.edges {
            let (Some(&a), Some(&b)) = (depth.get(&e.from), depth.get(&e.to)) else { continue };
            if b < a + 1 && a < flow.nodes.len() {
                depth.insert(e.to.clone(), a + 1);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    depth
}

/// Put every node in its column and row. Nodes keep the order they have in the flow within a column.
pub fn auto_layout(flow: &mut Flow) {
    let depth = depths(flow);
    let mut rows: BTreeMap<usize, usize> = BTreeMap::new();
    for n in &mut flow.nodes {
        let d = depth.get(&n.id).copied().unwrap_or(0);
        let r = rows.entry(d).or_insert(0);
        n.pos = Pos { x: MARGIN + d as f32 * COL_W, y: MARGIN + *r as f32 * ROW_H };
        *r += 1;
    }
}

/// True when two nodes sit on the same spot (a flow whose positions were never set).
pub fn unplaced(flow: &Flow) -> bool {
    flow.nodes.len() > 1 && flow.nodes.iter().all(|n| n.pos == Pos::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn columns_follow_the_longest_path_and_rows_the_order() {
        let mut f = Flow::new("f", "F");
        for id in ["s", "a", "b", "c", "e"] {
            f.nodes.push(Node::new(id, "notify", json!({})));
        }
        f.edges = vec![Edge::new("s", "a"), Edge::new("s", "b"), Edge::new("a", "c"), Edge::new("b", "c"), Edge::new("s", "c"), Edge::new("c", "e")];
        assert!(unplaced(&f));
        auto_layout(&mut f);
        let p = |id: &str| f.node(id).unwrap().pos;
        assert_eq!((p("s").x, p("s").y), (MARGIN, MARGIN));
        assert_eq!(p("a").x, MARGIN + COL_W);
        assert_eq!((p("b").x, p("b").y), (MARGIN + COL_W, MARGIN + ROW_H));
        assert_eq!(p("c").x, MARGIN + 2.0 * COL_W, "after its latest parent");
        assert_eq!(p("e").x, MARGIN + 3.0 * COL_W);
        assert!(!unplaced(&f));
    }

    #[test]
    fn a_cycle_does_not_hang() {
        let mut f = Flow::new("f", "F");
        for id in ["a", "b"] {
            f.nodes.push(Node::new(id, "notify", json!({})));
        }
        f.edges = vec![Edge::new("a", "b"), Edge::new("b", "a")];
        auto_layout(&mut f);
        assert_eq!(f.nodes.len(), 2);
    }
}
