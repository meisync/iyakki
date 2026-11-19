//! Where an app's embedded viewers go on its page and who gets a click (F-SDK-14). The agent keeps what is mounted
//! (`iyakki_agent_api::TileEmbed`); this file is the geometry: the rectangle of each viewer on the screen (a box in page pixels, or the box of an
//! element the page named), cut to the part of the page that is on screen, the topmost viewer under a point and the point in the viewer's own
//! pixels. It draws nothing and asks nobody: the tile paints what these functions return.

use iyakki_agent_api::TileEmbed;

use crate::ui::R;

/// One viewer placed on the screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Placed {
    pub id: u32,
    pub kind: String,
    /// The whole rectangle of the viewer (it may reach beyond the page's visible area).
    pub rect: R,
    /// The part that is on screen: nothing outside it is drawn or takes a click.
    pub visible: R,
}

fn target_rect(target_json: &str, element: &dyn Fn(&str) -> Option<R>) -> Option<R> {
    let v: serde_json::Value = serde_json::from_str(target_json).ok()?;
    match &v {
        serde_json::Value::String(id) => element(id),
        serde_json::Value::Object(o) => {
            let n = |k: &str| o.get(k).and_then(|x| x.as_f64()).filter(|x| x.is_finite() && *x >= 0.0).map(|x| x as f32);
            Some(R::new(n("x")?, n("y")?, n("w")?, n("h")?))
        }
        _ => None,
    }
}

fn clip(r: R, to: R) -> Option<R> {
    let (x0, y0) = (r.x.max(to.x), r.y.max(to.y));
    let (x1, y1) = ((r.x + r.w).min(to.x + to.w), (r.y + r.h).min(to.y + to.h));
    (x1 - x0 >= 1.0 && y1 - y0 >= 1.0).then(|| R::new(x0, y0, x1 - x0, y1 - y0))
}

/// Place every viewer. `origin` is where the page's top-left is on the screen (already scrolled), `scale` the screen scale, `view` the area of the
/// page on screen, `element` the box of an element by id in page pixels. A viewer whose target is unknown or wholly off screen is left out.
pub fn place(embeds: &[TileEmbed], origin: (f32, f32), scale: f32, view: R, element: &dyn Fn(&str) -> Option<R>) -> Vec<Placed> {
    embeds
        .iter()
        .filter_map(|e| {
            let t = target_rect(&e.target_json, element)?;
            let rect = R::new(origin.0 + t.x * scale, origin.1 + t.y * scale, t.w * scale, t.h * scale);
            Some(Placed { id: e.id, kind: e.kind.clone(), rect, visible: clip(rect, view)? })
        })
        .collect()
}

/// The viewer under a point, topmost (the one mounted last) first, and the point in the viewer's own pixels. A point in the cut-off part or in
/// no viewer belongs to the page.
pub fn route(placed: &[Placed], x: f32, y: f32, scale: f32) -> Option<(u32, f32, f32)> {
    placed.iter().rev().find(|p| p.visible.contains(x as i32, y as i32)).map(|p| (p.id, (x - p.rect.x) / scale, (y - p.rect.y) / scale))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(id: u32, target: &str) -> TileEmbed {
        TileEmbed { id, kind: "markdown".into(), target_json: target.into(), text: String::new(), hosts: vec![], profile: String::new() }
    }

    fn none(_: &str) -> Option<R> {
        None
    }

    #[test]
    fn a_box_is_placed_scaled_scrolled_and_cut_to_the_visible_page() {
        let view = R::new(0.0, 40.0, 800.0, 400.0);
        // scrolled 100 px up: the page starts at y = -60 on a 2x screen
        let p = place(&[e(1, r#"{"x":10,"y":50,"w":300,"h":100}"#)], (20.0, -60.0), 2.0, view, &none);
        assert_eq!(p[0].rect, R::new(40.0, 40.0, 600.0, 200.0));
        assert_eq!(p[0].visible, p[0].rect);
        // half of it is under the bar: cut
        let p = place(&[e(1, r#"{"x":0,"y":0,"w":300,"h":100}"#)], (0.0, 0.0), 1.0, view, &none);
        assert_eq!(p[0].visible, R::new(0.0, 40.0, 300.0, 60.0));
        // wholly off screen, an unknown element, and junk are left out
        assert!(place(&[e(1, r#"{"x":0,"y":900,"w":300,"h":100}"#), e(2, r#""nothing""#), e(3, "17"), e(4, r#"{"x":-1,"y":0,"w":3,"h":3}"#)], (0.0, 0.0), 1.0, view, &none).is_empty());
    }

    #[test]
    fn an_element_target_follows_the_element_the_page_laid_out() {
        let view = R::new(0.0, 0.0, 800.0, 600.0);
        let el = |id: &str| (id == "chart").then(|| R::new(5.0, 70.0, 400.0, 200.0));
        let p = place(&[e(7, r#""chart""#)], (100.0, 10.0), 1.0, view, &el);
        assert_eq!(p[0].rect, R::new(105.0, 80.0, 400.0, 200.0));
    }

    #[test]
    fn input_goes_to_the_topmost_viewer_in_its_own_pixels_and_the_cut_off_part_belongs_to_the_page() {
        let view = R::new(0.0, 40.0, 800.0, 400.0);
        let p = place(&[e(1, r#"{"x":0,"y":0,"w":300,"h":200}"#), e(2, r#"{"x":100,"y":50,"w":300,"h":200}"#)], (0.0, 0.0), 2.0, view, &none);
        assert_eq!(route(&p, 250.0, 150.0, 2.0), Some((2, 25.0, 25.0)), "the later viewer is on top");
        assert_eq!(route(&p, 50.0, 100.0, 2.0), Some((1, 25.0, 50.0)));
        assert_eq!(route(&p, 50.0, 20.0, 2.0), None, "above the visible page: the bar's, not the viewer's");
        assert_eq!(route(&p, 790.0, 445.0, 2.0), None, "inside the viewer's whole box but below the visible page: the page's");
    }
}
