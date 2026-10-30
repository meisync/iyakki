// The consent dialog over a live (animating) page must not flicker: while it is open no partial repaint of the page may reach the
// screen, and a whole redraw puts the dialog back pixel for pixel. Included (include!) into the browser's test module.

#[test]
fn a_live_page_never_repaints_over_the_open_consent_dialog() {
    let (_fake, cx, mut a, _g) = in_full("consent_live", "--fps 60");
    ask_for(&mut a, &cx, 2);
    let (w, h) = (1280u32, 720u32);
    let mut g = frame(&mut a, &cx, w, h);
    let base = g.pm.data().to_vec();
    let backdrop = g.hits.iter().find(|x| x.label == "backdrop").map(|x| x.rect).expect("the scrim");
    let s = (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0);
    let (dw, dh) = ((640.0 * s).min(backdrop.w - 40.0 * s), (250.0 * s).min(backdrop.h - 16.0 * s));
    let dlg = R::new(backdrop.x + (backdrop.w - dw) / 2.0, backdrop.y + ((backdrop.h - dh) / 2.0).max(8.0 * s), dw, dh);
    let frames0 = a.full.as_ref().unwrap().frames;
    let (mut live, mut whole) = (0, 0);
    for _ in 0..120 {
        a.handle(&Event::Tick, &cx);
        match a.redraw_kind() {
            Redraw::Live => {
                live += 1;
                if a.live_draw(&mut g).is_some() {
                    // the backend presents exactly this buffer: it must still be the scene with scrim and dialog
                    assert_eq!(g.pm.data(), &base[..], "a partial repaint reached the screen under the dialog");
                }
            }
            Redraw::Full => {
                whole += 1;
                g = frame(&mut a, &cx, w, h);
                for y in (dlg.y as u32)..((dlg.y + dlg.h) as u32) {
                    for x in (dlg.x as u32)..((dlg.x + dlg.w) as u32) {
                        assert_eq!(px(&g, x as f32, y as f32), px_of(&base, w, x, y), "dialog pixel ({x},{y}) changed between presented frames");
                    }
                }
            }
            Redraw::None => {}
        }
        std::thread::sleep(std::time::Duration::from_millis(4));
    }
    assert!(a.full.as_ref().unwrap().frames > frames0 + 3, "the page kept animating behind the dialog");
    assert_eq!(live, 0, "no partial page repaint is asked for while the dialog is open ({whole} whole)");
    // answered: the page is live again
    key(&mut a, &cx, Key::Char('1'));
    assert!(a.media_ask.is_none());
}

fn px_of(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let o = ((y * w + x) * 4) as usize;
    buf[o..o + 4].try_into().unwrap()
}
