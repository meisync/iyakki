use super::*;

const SIZES: [(f32, f32); 5] = [(1280.0, 720.0), (1920.0, 1080.0), (2560.0, 1600.0), (1024.0, 600.0), (800.0, 480.0)];
const GAP: f32 = 6.0;

fn area(w: f32, h: f32) -> R {
    R::new(0.0, 0.0, w, h)
}

fn overlaps(a: &R, b: &R) -> bool {
    a.x < b.x + b.w - 0.01 && b.x < a.x + a.w - 0.01 && a.y < b.y + b.h - 0.01 && b.y < a.y + a.h - 0.01
}

#[test]
fn every_template_covers_the_display_without_overlap_at_every_size() {
    for t in TEMPLATES {
        for (w, h) in SIZES {
            let l = Layout::from_template(t.id);
            let a = area(w, h);
            let rs = l.rects(a, GAP);
            assert_eq!(rs.len(), l.leaf_count(), "{}", t.id);
            for (i, r) in rs.iter().enumerate() {
                assert!(r.w > 0.0 && r.h > 0.0, "{} pane {i} empty at {w}x{h}", t.id);
                assert!(r.x >= -0.01 && r.y >= -0.01 && r.x + r.w <= w + 0.01 && r.y + r.h <= h + 0.01, "{} pane {i} outside", t.id);
                for (j, q) in rs.iter().enumerate().skip(i + 1) {
                    assert!(!overlaps(r, q), "{} panes {i} and {j} overlap at {w}x{h}", t.id);
                }
            }
            // the panes plus the divider strips fill the display: area sum + gaps = total
            let panes: f32 = rs.iter().map(|r| r.w * r.h).sum();
            let divs: f32 = l.dividers(a, GAP, GAP).iter().map(|d| d.rect.w * d.rect.h).sum();
            let covered = panes + divs;
            // the gap strips of nested splits overlap at crossings (the 2x2 grid), so allow a tiny excess
            assert!((covered - w * h).abs() <= GAP * GAP * 4.0 + 8.0 * (w + h), "{} covers {covered} of {}", t.id, w * h);
            // the outer edges are reached
            let right = rs.iter().map(|r| r.x + r.w).fold(0.0f32, f32::max);
            let bottom = rs.iter().map(|r| r.y + r.h).fold(0.0f32, f32::max);
            assert!((right - w).abs() <= 1.01 && (bottom - h).abs() <= 1.01, "{} edges {right}x{bottom}", t.id);
        }
    }
}

#[test]
fn the_table_has_unique_ids_and_the_numbered_keys_one_to_nine() {
    let mut ids: Vec<_> = TEMPLATES.iter().map(|t| t.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), TEMPLATES.len());
    for n in 1..=9u8 {
        assert!(template_for_key(n).is_some(), "key {n}");
    }
    assert_eq!(template_for_key(1).unwrap().id, "single");
    assert!(template_for_key(0).is_none() && template_for_key(10).is_none());
}

#[test]
fn ratios_are_what_the_names_say() {
    let a = area(1000.0, 600.0);
    let near = |x: f32, y: f32, tol: f32| (x - y).abs() <= tol;
    let w = |id: &str| Layout::from_template(id).rects(a, 0.0).iter().map(|r| r.w).collect::<Vec<_>>();
    let rs = w("cols-30-70");
    assert!(near(rs[0], 300.0, 1.0) && near(rs[1], 700.0, 1.0), "{rs:?}");
    let rs = w("cols-70-30");
    assert!(near(rs[0], 700.0, 1.0) && near(rs[1], 300.0, 1.0));
    let rs = w("cols-25-75");
    assert!(near(rs[0], 250.0, 1.0));
    let rs = w("cols-75-25");
    assert!(near(rs[0], 750.0, 1.0));
    let rs = w("cols-60-40");
    assert!(near(rs[0], 600.0, 1.0));
    let rs = w("thirds");
    assert!(rs.iter().all(|x| near(*x, 333.0, 2.0)), "{rs:?}");
    let h: Vec<f32> = Layout::from_template("rows-50-50").rects(a, 0.0).iter().map(|r| r.h).collect();
    assert!(near(h[0], 300.0, 1.0) && near(h[1], 300.0, 1.0));
    let g = Layout::from_template("grid").rects(a, 0.0);
    assert_eq!(g.len(), 4);
    assert!(near(g[0].w, 500.0, 1.0) && near(g[0].h, 300.0, 1.0) && g[3].x > 490.0 && g[3].y > 290.0);
}

#[test]
fn pane_numbers_follow_reading_order_and_hit_testing_finds_them() {
    let l = Layout::from_template("main-stack");
    let a = area(1000.0, 600.0);
    let rs = l.rects(a, GAP);
    assert_eq!(rs.len(), 3);
    assert!(rs[0].x < rs[1].x && rs[1].y < rs[2].y);
    assert_eq!(l.pane_at(a, GAP, 10, 10), Some(0));
    assert_eq!(l.pane_at(a, GAP, 990, 10), Some(1));
    assert_eq!(l.pane_at(a, GAP, 990, 590), Some(2));
    // the gap between panes belongs to none
    assert_eq!(l.pane_at(a, GAP, (rs[0].x + rs[0].w + 2.0) as i32, 300), None);
    assert!(l.divider_at(a, GAP, 12.0, (rs[0].x + rs[0].w + 2.0) as i32, 300).is_some());
    assert_eq!(l.focus_order(), vec![0, 1, 2]);
}

#[test]
fn focus_cycles_forward_and_back() {
    let l = Layout::from_template("grid");
    assert_eq!(l.next_focus(0, false), 1);
    assert_eq!(l.next_focus(3, false), 0);
    assert_eq!(l.next_focus(0, true), 3);
    let one = Layout::single();
    assert_eq!(one.next_focus(0, false), 0);
}

#[test]
fn a_divider_drag_moves_snaps_and_stops_at_the_minimum_sizes() {
    let a = area(1000.0, 600.0);
    let mut l = Layout::from_template("cols-50-50");
    let mins = [(200.0, 100.0), (200.0, 100.0)];
    // to about 30 %: snaps to exactly 0.30
    let r = l.drag(a, 0.0, 0, 305.0, &mins).unwrap();
    assert!((r - 0.30).abs() < 1e-4, "{r}");
    // 41 %: not near enough to a snap? 0.40 is within 2 % -> snaps
    let r = l.drag(a, 0.0, 0, 412.0, &mins).unwrap();
    assert!((r - 0.40).abs() < 1e-4, "{r}");
    // 45 % is more than 2 % from 0.40 and 0.50: stays free
    let r = l.drag(a, 0.0, 0, 450.0, &mins).unwrap();
    assert!((r - 0.45).abs() < 0.005, "{r}");
    // far left: the first pane keeps its minimum width
    l.drag(a, 0.0, 0, 5.0, &mins);
    let rs = l.rects(a, 0.0);
    assert!(rs[0].w >= 199.0, "{:?}", rs[0]);
    // far right: the second keeps its
    l.drag(a, 0.0, 0, 995.0, &mins);
    let rs = l.rects(a, 0.0);
    assert!(rs[1].w >= 199.0, "{:?}", rs[1]);
    assert!(l.ratio(0).unwrap() <= 0.8 + 1e-4);
}

#[test]
fn the_grid_columns_stay_aligned_because_the_splits_are_linked() {
    let a = area(1000.0, 600.0);
    let mut l = Layout::from_template("grid");
    let mins = vec![(100.0, 80.0); 4];
    // split 0 is the rows, 1 and 3? preorder: 0 stack, 1 side (top), 2 side (bottom)
    let r = l.drag(a, 0.0, 1, 300.0, &mins).unwrap();
    assert!((r - 0.30).abs() < 1e-3);
    let rs = l.rects(a, 0.0);
    assert!((rs[0].w - rs[2].w).abs() < 1.01 && (rs[1].x - rs[3].x).abs() < 1.01, "{rs:?}");
}

#[test]
fn a_nested_drag_respects_the_minimum_of_everything_below_it() {
    let a = area(1000.0, 600.0);
    let mut l = Layout::from_template("thirds");
    let mins = vec![(250.0, 100.0); 3];
    // split 0 separates pane 0 from the two on the right: they need 250 + 250 + gap
    l.drag(a, 0.0, 0, 900.0, &mins);
    let rs = l.rects(a, 0.0);
    assert!(rs[1].w >= 249.0 && rs[2].w >= 249.0, "{rs:?}");
}

#[test]
fn nudging_the_divider_beside_a_pane_moves_it_five_percent_and_not_past_the_edge() {
    let a = area(1000.0, 600.0);
    let mut l = Layout::from_template("cols-50-50");
    let mins = [(150.0, 100.0), (150.0, 100.0)];
    // pane 0's divider is on its right: Right makes pane 0 bigger
    assert!(l.nudge(a, 0.0, 0, Side::Right, 0.05, &mins));
    assert!(l.ratio(0).unwrap() > 0.52);
    // pane 1's divider is on its left; pane 0 has none on its left
    assert!(!l.nudge(a, 0.0, 0, Side::Left, 0.05, &mins));
    assert!(l.nudge(a, 0.0, 1, Side::Left, 0.05, &mins));
    for _ in 0..40 {
        l.nudge(a, 0.0, 0, Side::Right, 0.05, &mins);
    }
    assert!(l.rects(a, 0.0)[1].w >= 149.0);
    // a stacked layout answers to Up and Down only
    let mut s = Layout::from_template("rows-50-50");
    assert!(!s.nudge(a, 0.0, 0, Side::Right, 0.05, &[(100.0, 100.0); 2]));
    assert!(s.nudge(a, 0.0, 0, Side::Down, 0.05, &[(100.0, 100.0); 2]));
}

#[test]
fn neighbours_and_snap_actions() {
    let a = area(1000.0, 600.0);
    let g = Layout::from_template("grid");
    assert_eq!(g.neighbour(a, GAP, 0, Side::Right), Some(1));
    assert_eq!(g.neighbour(a, GAP, 0, Side::Down), Some(2));
    assert_eq!(g.neighbour(a, GAP, 3, Side::Left), Some(2));
    assert_eq!(g.neighbour(a, GAP, 3, Side::Up), Some(1));
    assert_eq!(g.neighbour(a, GAP, 0, Side::Left), None);
    assert_eq!(g.neighbour(a, GAP, 0, Side::Up), None);
    let m = Layout::from_template("main-stack");
    assert_eq!(m.neighbour(a, GAP, 0, Side::Right), Some(1), "the nearest of the stacked ones with the biggest overlap, the first");
    assert_eq!(m.neighbour(a, GAP, 2, Side::Left), Some(0));
    assert_eq!(m.neighbour(a, GAP, 1, Side::Down), Some(2));
    // snapping
    let one = Layout::single();
    assert_eq!(one.snap(a, GAP, 0, Side::Left), Snap::Make { template: "cols-50-50", focus_after: 0 });
    assert_eq!(one.snap(a, GAP, 0, Side::Right), Snap::Make { template: "cols-50-50", focus_after: 1 });
    assert_eq!(one.snap(a, GAP, 0, Side::Up), Snap::Make { template: "rows-50-50", focus_after: 0 });
    assert_eq!(one.snap(a, GAP, 0, Side::Down), Snap::Make { template: "rows-50-50", focus_after: 1 });
    assert_eq!(g.snap(a, GAP, 0, Side::Right), Snap::Swap(1));
    assert_eq!(g.snap(a, GAP, 0, Side::Left), Snap::Nothing);
}

#[test]
fn templates_that_do_not_fit_a_small_display_are_found_by_the_minimum_sizes() {
    let small = area(1024.0, 600.0);
    let big = area(2560.0, 1600.0);
    let min = (500.0, 300.0);
    assert!(Layout::from_template("single").fits(small, GAP, min));
    assert!(Layout::from_template("cols-50-50").fits(small, GAP, min));
    assert!(!Layout::from_template("grid").fits(small, GAP, min));
    assert!(!Layout::from_template("thirds").fits(small, GAP, min));
    assert!(Layout::from_template("grid").fits(big, GAP, min));
    assert!(!Layout::from_template("cols-25-75").fits(small, GAP, min), "25% of 1024 is under 500");
}

#[test]
fn closing_a_pane_goes_to_a_template_with_one_pane_fewer() {
    assert_eq!(Layout::from_template("grid").after_close(), "thirds");
    assert_eq!(Layout::from_template("thirds").after_close(), "cols-50-50");
    assert_eq!(Layout::from_template("main-stack").after_close(), "cols-50-50");
    assert_eq!(Layout::from_template("cols-30-70").after_close(), "single");
    assert_eq!(Layout::single().after_close(), "single");
}

#[test]
fn degenerate_areas_do_not_panic() {
    for t in TEMPLATES {
        let l = Layout::from_template(t.id);
        for a in [area(0.0, 0.0), area(1.0, 1.0), area(5.0, 3000.0)] {
            let rs = l.rects(a, GAP);
            assert_eq!(rs.len(), l.leaf_count());
            let _ = l.dividers(a, GAP, 10.0);
            let _ = l.neighbour(a, GAP, 0, Side::Right);
        }
    }
    let mut l = Layout::from_template("grid");
    let _ = l.drag(area(0.0, 0.0), 6.0, 0, 3.0, &[]);
}
