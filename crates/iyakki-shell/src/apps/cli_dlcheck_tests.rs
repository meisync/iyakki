//! The "Check for updates" control of the CLIs tab (signed downloads manifest): its words and its layout at the four screen sizes.

use super::*;
use crate::apps::tests::cx_with_backend;

fn draw(app: &mut CliApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(9, 11, 20));
    app.render(&mut g, R::new(0.0, 46.0, w as f32, h as f32 - 46.0), 1.0, cx, 3);
    g
}

fn write_state(cx: &AppCx, result: &str, notice: Option<&str>) {
    let d = cx.services.clis.manifest_dir();
    std::fs::create_dir_all(&d).unwrap();
    let j = serde_json::json!({ "at": iyakki_agent::dlmanifest::fetch::now_s() - 3 * 3600, "result": result, "notice": notice });
    std::fs::write(d.join("fetch-state.json"), j.to_string()).unwrap();
}

#[test]
fn the_check_control_and_its_result_fit_at_every_screen_size() {
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let (cx, _d) = cx_with_backend(&format!("dlcheck-{w}"), iyakki_agent::chat::Unconfigured);
        write_state(&cx, "refused: signature did not match", Some("Download list updated: 3 items changed (kubectl, helm, k3s)"));
        let mut app = CliApp::new(&cx);
        app.handle(&Event::Tab(1), &cx);
        let g = draw(&mut app, &cx, w, h);
        let b = g.hits.iter().find(|x| x.label == "Check for updates").unwrap_or_else(|| panic!("{w}x{h}: no control"));
        assert!(matches!(b.ev, Event::Key(Key::Char('r'))));
        let r = b.rect;
        assert!(r.x >= 0.0 && r.y >= 46.0 && r.x + r.w <= w as f32 && r.y + r.h <= h as f32, "{w}x{h}: {r:?}");
        // it does not sit on the first row of the list
        let first = g.hits.iter().filter(|x| matches!(x.ev, Event::Row(_))).map(|x| x.rect.y).fold(f32::MAX, f32::min);
        assert!(r.y + r.h <= first + 0.5, "{w}x{h}: the control overlaps the list");
        for hit in &g.hits {
            let q = hit.rect;
            assert!(q.x >= -0.5 && q.y >= -0.5 && q.x + q.w <= w as f32 + 0.5 && q.y + q.h <= h as f32 + 0.5, "{w}x{h}: {} outside", hit.label);
        }
    }
}

#[test]
fn the_status_line_uses_plain_words() {
    use iyakki_agent::dlmanifest::fetch::{ago, status_line, CheckStatus};
    for (res, want) in [("up to date", "Checked 3 h ago: up to date"), ("no connection", "Checked 3 h ago: no connection"), ("refused: signature did not match", "Checked 3 h ago: refused: signature did not match")] {
        let s = CheckStatus { at: Some(1000), result: res.into(), ..Default::default() };
        assert_eq!(status_line(&s, 1000 + 3 * 3600), want);
    }
    assert_eq!(ago(10, 10), "just now");
}
