//! Tests for pictures inside the assistant's answers: SVG drawings, data addresses and base64 images (chatpic.rs).

use super::tests::{agent_cx, ask, render, run_until, turn};
use super::*;
use crate::apps::chatpic::fixtures::*;
use crate::apps::tests::cx_with_backend;
use crate::apps::App;
use serde_json::json;

fn chat_with(answer: &str) -> ChatApp {
    let mut app = ChatApp::new();
    app.msgs.push(Message::user("draw me something"));
    app.msgs.push(Message::assistant(answer.to_string()));
    app
}

fn shot(app: &mut ChatApp, cx: &AppCx, w: u32) -> Gfx {
    let mut g = Gfx::new(w, 700);
    g.clear(rgb(9, 11, 20));
    app.render(&mut g, R::new(0.0, 46.0, w as f32, 654.0), 1.0, cx, 0);
    g
}

fn count(g: &Gfx, f: impl Fn(&[u8]) -> bool) -> usize {
    g.rgba().chunks_exact(4).filter(|p| f(p)).count()
}

fn is_blue(p: &[u8]) -> bool {
    p[0] < 40 && p[1] < 40 && p[2] > 220
}

/// How many near-white pixels lie inside the box around the blue ones (the lettering on the picture).
fn white_inside_blue(g: &Gfx) -> usize {
    let w = g.width() as usize;
    let px = g.rgba();
    let (mut x0, mut y0, mut x1, mut y1) = (usize::MAX, usize::MAX, 0, 0);
    for (i, p) in px.chunks_exact(4).enumerate() {
        if is_blue(p) {
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 == usize::MAX {
        return 0;
    }
    let mut n = 0;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let p = &px[(y * w + x) * 4..];
            if p[0] > 200 && p[1] > 200 && p[2] > 200 {
                n += 1;
            }
        }
    }
    n
}

fn hit(g: &Gfx, label: &str) -> Event {
    g.hits.iter().find(|h| h.label == label).unwrap_or_else(|| panic!("no {label:?} button in {:?}", g.hits.iter().map(|h| h.label.clone()).collect::<Vec<_>>())).ev.clone()
}

fn all_text(app: &ChatApp) -> String {
    app.view.layouts().iter().flat_map(|l| l.texts()).collect::<Vec<_>>().join(" ")
}

fn fence(lang: &str, body: &str) -> String {
    format!("Here is a greeting you can use:\n\n```{lang}\n{}\n```\n\nSave it as hello.svg.", body.trim())
}

#[test]
fn an_svg_code_block_is_drawn_as_a_picture_with_its_text_and_the_code_stays_folded() {
    let (cx, _d) = cx_with_backend("svgpic1", iyakki_agent::chat::Unconfigured);
    let mut app = chat_with(&fence("svg", HELLO_SVG));
    let g = shot(&mut app, &cx, 1100);
    let blue = count(&g, is_blue);
    assert!(blue > 12_000, "the blue rectangle is on screen: {blue}");
    assert!(white_inside_blue(&g) > 60, "and its lettering: {}", white_inside_blue(&g));
    let t = all_text(&app);
    assert!(t.contains("Copy SVG") && t.contains("Save .svg") && t.contains("Show code"), "{t}");
    assert!(t.contains("SVG drawing"), "{t}");
    assert!(!t.contains("<rect"), "the code is folded: {t}");
    assert!(t.contains("Here is a greeting") && t.contains("Save it as hello.svg"), "the text around it stays");
    // the same for xml / no language, and a bare element
    for md in [fence("xml", HELLO_SVG), fence("", HELLO_SVG), format!("Look:\n{}\nDone", HELLO_SVG.trim())] {
        let mut a = chat_with(&md);
        let g = shot(&mut a, &cx, 1100);
        assert!(count(&g, is_blue) > 12_000, "{}", &md[..40]);
    }
}

#[test]
fn a_picture_is_drawn_once_per_width_not_every_frame() {
    let (cx, _d) = cx_with_backend("svgpic2", iyakki_agent::chat::Unconfigured);
    let mut app = chat_with(&fence("svg", HELLO_SVG));
    for _ in 0..5 {
        shot(&mut app, &cx, 1100);
    }
    assert_eq!(app.pics.rasterised, 1);
    shot(&mut app, &cx, 500);
    assert_eq!(app.pics.rasterised, 2, "a new width draws it again, crisp for that size");
    shot(&mut app, &cx, 500);
    assert_eq!(app.pics.rasterised, 2);
    let laid = app.view.laid;
    shot(&mut app, &cx, 500);
    assert_eq!(app.view.laid, laid, "nothing is laid out again");
}

#[test]
fn the_picture_is_scaled_to_the_column_and_keeps_its_shape() {
    let (cx, _d) = cx_with_backend("svgpic3", iyakki_agent::chat::Unconfigured);
    let wide = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 2000 1000'><rect width='2000' height='1000' fill='blue'/></svg>";
    let mut app = chat_with(&fence("svg", wide));
    let g = shot(&mut app, &cx, 1100);
    let w = g.width() as usize;
    let (mut x0, mut x1, mut y0, mut y1) = (usize::MAX, 0, usize::MAX, 0);
    for (i, p) in g.rgba().chunks_exact(4).enumerate() {
        if is_blue(p) {
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            x1 = x1.max(x);
            y0 = y0.min(y);
            y1 = y1.max(y);
        }
    }
    let (pw, ph) = ((x1 - x0 + 1) as f32, (y1 - y0 + 1) as f32);
    assert!((pw - 560.0).abs() < 3.0, "capped at 560 px wide: {pw}");
    assert!((pw / ph - 2.0).abs() < 0.03, "aspect kept: {pw} x {ph}");
    // no size at all: a sensible default
    let mut app = chat_with(&fence("svg", "<svg xmlns='http://www.w3.org/2000/svg'><rect width='300' height='150' fill='blue'/></svg>"));
    let g = shot(&mut app, &cx, 1100);
    assert!(count(&g, is_blue) > 10_000);
    // a small screen: it shrinks to the column
    let mut app = chat_with(&fence("svg", HELLO_SVG));
    let g = shot(&mut app, &cx, 360);
    assert!(count(&g, is_blue) > 3000 && app.view.layouts().iter().all(|l| l.right_edge() <= 360.0));
}

#[test]
fn a_broken_svg_says_why_and_keeps_its_code_visible() {
    let (cx, _d) = cx_with_backend("svgpic4", iyakki_agent::chat::Unconfigured);
    let deep = format!("<svg xmlns='http://www.w3.org/2000/svg'>{}{}</svg>", "<g>".repeat(100), "</g>".repeat(100));
    let mut app = chat_with(&fence("svg", &deep));
    let g = shot(&mut app, &cx, 1100);
    let t = all_text(&app);
    assert!(t.contains("SVG could not be drawn: the SVG is nested too deeply"), "{t}");
    assert!(t.contains("<g><g><g>"), "the code is shown: {t}");
    assert!(t.contains("Copy SVG") && !t.contains("Save .svg") && !t.contains("Show code"), "{t}");
    assert_eq!(count(&g, is_blue), 0);
    // shown without a crash again and again
    shot(&mut app, &cx, 1100);
}

#[test]
fn hostile_svg_in_a_message_is_held_by_the_caps() {
    let (cx, _d) = cx_with_backend("svgpic5", iyakki_agent::chat::Unconfigured);
    let mut defs = String::from("<defs><g id='a0'><rect width='50' height='50' fill='blue'/><text x='1' y='9'>boom</text></g>");
    for i in 1..12 {
        defs.push_str(&format!("<g id='a{i}'>{}</g>", format!("<use href='#a{}'/>", i - 1).repeat(10)));
    }
    defs.push_str("</defs><use href='#a11'/>");
    let bomb = format!("<svg xmlns='http://www.w3.org/2000/svg' width='100000' height='100000'>{defs}</svg>");
    let t0 = std::time::Instant::now();
    let mut app = chat_with(&fence("svg", &bomb));
    let g = shot(&mut app, &cx, 1100);
    assert!(t0.elapsed().as_secs() < 20, "{:?}", t0.elapsed());
    assert!(g.width() == 1100);
    let r = app.pics.decoded(&PicSrc::new(PicKind::Svg, &bomb)).unwrap();
    assert_eq!(r.ext, "svg");
}

#[test]
fn copy_svg_puts_the_exact_source_on_the_clipboard() {
    let (cx, _d) = cx_with_backend("svgpic6", iyakki_agent::chat::Unconfigured);
    let mut app = chat_with(&fence("svg", HELLO_SVG));
    let g = shot(&mut app, &cx, 1100);
    crate::ui::clipboard::clear();
    app.handle(&hit(&g, "Copy SVG"), &cx);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some(HELLO_SVG.trim()));
}

#[test]
fn show_code_and_hide_code_toggle_the_source() {
    let (cx, _d) = cx_with_backend("svgpic7", iyakki_agent::chat::Unconfigured);
    let mut app = chat_with(&fence("svg", HELLO_SVG));
    let g = shot(&mut app, &cx, 1100);
    app.handle(&hit(&g, "Show code"), &cx);
    let g = shot(&mut app, &cx, 1100);
    let t = all_text(&app);
    assert!(t.contains("Hide code") && t.contains("<rect width=\"200\"") && t.contains("Hello SVG"), "{t}");
    assert!(count(&g, is_blue) > 12_000, "the picture stays");
    app.handle(&hit(&g, "Hide code"), &cx);
    shot(&mut app, &cx, 1100);
    assert!(!all_text(&app).contains("<rect"));
    assert_eq!(app.pics.rasterised, 1, "toggling does not draw it again");
}

#[test]
fn save_svg_asks_for_a_name_writes_inside_the_scope_with_unique_names_and_is_audited() {
    let (mut cx, _d) = cx_with_backend("svgpic8", iyakki_agent::chat::Unconfigured);
    let home = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-chatpic-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    cx.session.policy.fs_write = vec![home.display().to_string()];
    cx.session.policy.fs_read = vec![home.display().to_string()];
    let mut app = chat_with(&fence("svg", HELLO_SVG));
    app.save_home = Some(home.clone());
    let g = shot(&mut app, &cx, 1100);
    app.handle(&hit(&g, "Save .svg"), &cx);
    assert!(app.save.is_some(), "the form for a file name opens");
    let g = shot(&mut app, &cx, 1100);
    assert!(g.hits.iter().all(|h| h.label != "Copy SVG") || app.save.is_some());
    app.handle(&Event::Key(Key::Ctrl('s')), &cx);
    assert!(app.save.is_none(), "{:?}", app.save.as_ref().map(|s| s.0.error.clone()));
    let saved = home.join("Pictures/drawing.svg");
    assert_eq!(std::fs::read_to_string(&saved).unwrap(), HELLO_SVG.trim());
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("chat.picture.save") && l.contains("drawing.svg")));
    // again, with a name of the person's choosing, then the same name again
    for expect in ["logo.svg", "logo-2.svg"] {
        let g = shot(&mut app, &cx, 1100);
        app.handle(&hit(&g, "Save .svg"), &cx);
        app.save.as_mut().unwrap().0.set_value(0, "logo");
        app.handle(&Event::Key(Key::Ctrl('s')), &cx);
        assert!(home.join("Pictures").join(expect).exists(), "{expect}");
    }
    // a name cannot climb out of the folder
    let g = shot(&mut app, &cx, 1100);
    app.handle(&hit(&g, "Save .svg"), &cx);
    app.save.as_mut().unwrap().0.set_value(0, "../../evil");
    app.handle(&Event::Key(Key::Ctrl('s')), &cx);
    assert!(home.join("Pictures/evil.svg").exists() && !home.join("evil.svg").exists());
    // Escape closes the form without writing
    let g = shot(&mut app, &cx, 1100);
    app.handle(&hit(&g, "Save .svg"), &cx);
    app.handle(&Event::Key(Key::Esc), &cx);
    assert!(app.save.is_none());
    // a folder outside the write scope is refused: the form stays open with the reason, nothing is written
    let other = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-chatpic-other-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&other);
    std::fs::create_dir_all(&other).unwrap();
    app.save_home = Some(other.clone());
    let g = shot(&mut app, &cx, 1100);
    app.handle(&hit(&g, "Save .svg"), &cx);
    app.handle(&Event::Key(Key::Ctrl('s')), &cx);
    let f = &app.save.as_ref().expect("still open").0;
    assert!(f.error.as_deref().unwrap_or("").contains("permission denied"), "{:?}", f.error);
    assert!(!other.join("Pictures").exists() && std::fs::read_dir(&other).unwrap().count() == 0);
    assert!(!cx.audit.recent(30).iter().any(|l| l.contains("chat.picture.save") && l.contains("iyakki-chatpic-other")));
}

#[test]
fn an_image_sent_as_a_data_address_is_drawn_and_the_long_text_is_folded() {
    let (cx, _d) = cx_with_backend("datapic1", iyakki_agent::chat::Unconfigured);
    let png = png(160, 100, [0, 200, 0]);
    let url = format!("data:image/png;base64,{}", b64(&png));
    let is_green = |p: &[u8]| p[0] < 30 && p[1] > 180 && p[2] < 30;
    for md in [
        format!("Here is your image:\n\n{url}\n\nEnjoy."),
        format!("```\n{url}\n```"),
        format!("```text\n{url}\n```"),
        format!("![a green box]({url})"),
        format!("Here is the base64 image:\n\n```base64\n{}\n```", b64(&png)),
    ] {
        let mut app = chat_with(&md);
        let g = shot(&mut app, &cx, 1100);
        assert!(count(&g, is_green) > 12_000, "{}", &md[..60.min(md.len())]);
        let t = all_text(&app);
        assert!(t.contains("Show text") && t.contains("Save as .png"), "{t}");
        assert!(t.contains("data URL") || t.contains("base64 image"), "{t}");
        assert!(!t.contains("iVBOR"), "the long text is folded: {t}");
    }
    let mut app = chat_with(&format!("![x]({url})"));
    let g = shot(&mut app, &cx, 1100);
    assert!(all_text(&app).contains("image/png data URL · 0 KB") || all_text(&app).contains("image/png data URL"), "{}", all_text(&app));
    // expand, then collapse
    app.handle(&hit(&g, "Show text"), &cx);
    let g = shot(&mut app, &cx, 1100);
    let t = all_text(&app);
    assert!(t.contains("data:image/png;base64,iVBOR") && t.contains("Hide text") && t.contains("more characters"), "{t}");
    assert!(t.len() < 2500, "only the start of it: {}", t.len());
    app.handle(&hit(&g, "Hide text"), &cx);
    shot(&mut app, &cx, 1100);
    assert!(!all_text(&app).contains("iVBOR"));
    // Copy puts the whole address on the clipboard
    let g = shot(&mut app, &cx, 1100);
    crate::ui::clipboard::clear();
    app.handle(&hit(&g, "Copy data URL"), &cx);
    assert_eq!(crate::ui::clipboard::latest().as_deref(), Some(url.as_str()));
}

#[test]
fn jpeg_gif_webp_and_svg_data_addresses_are_drawn_too() {
    let (cx, _d) = cx_with_backend("datapic2", iyakki_agent::chat::Unconfigured);
    for (file, mime) in [("base420.jpg", "jpeg"), ("pal.gif", "gif"), ("webp_lossy.webp", "webp")] {
        let url = format!("data:image/{mime};base64,{}", b64(&web_data(file)));
        let mut app = chat_with(&url);
        let g = shot(&mut app, &cx, 1100);
        let t = all_text(&app);
        assert!(t.contains(&format!("image/{mime} data URL")) && !t.contains("could not"), "{file}: {t}");
        let ext = if mime == "jpeg" { "jpg" } else { mime };
        assert!(t.contains(&format!("Save as .{ext}")), "{t}");
        assert!(count(&g, |p| p[3] == 255 && (p[0] as u32 + p[1] as u32 + p[2] as u32) > 60) > 2000, "{file} is on screen");
    }
    let svg = "<svg xmlns='http://www.w3.org/2000/svg' width='80' height='60'><rect width='80' height='60' fill='blue'/></svg>";
    let enc = svg.replace('<', "%3C").replace('>', "%3E").replace('\'', "%27");
    for md in [format!("data:image/svg+xml;base64,{}", b64(svg.as_bytes())), format!("![logo](data:image/svg+xml;utf8,{enc})")] {
        let mut app = chat_with(&md);
        let g = shot(&mut app, &cx, 1100);
        assert!(count(&g, is_blue) > 5000, "{}: {} blue {}", &md[..40], all_text(&app), count(&g, is_blue));
        assert!(all_text(&app).contains("Save as .svg"));
    }
}

#[test]
fn broken_cut_off_and_oversized_images_show_a_note_and_a_short_piece_of_the_text() {
    let (cx, _d) = cx_with_backend("datapic3", iyakki_agent::chat::Unconfigured);
    let full = format!("data:image/png;base64,{}", b64(&png(60, 60, [9, 9, 9])));
    let cases = [
        (full[..full.len() / 2].to_string(), "could not be shown"),
        ("data:image/png;base64,!!!!notbase64!!!!".to_string(), "not valid"),
        ("data:image/png;base64,aGVsbG8gd29ybGQ=".to_string(), "not a PNG"),
        (format!("data:image/png;base64,{}", "A".repeat(crate::apps::chatpic::MAX_SOURCE + 50)), "8 MB"),
    ];
    for (md, why) in cases {
        let t0 = std::time::Instant::now();
        let mut app = chat_with(&format!("Here:\n{md}"));
        shot(&mut app, &cx, 1100);
        let t = all_text(&app);
        assert!(t.contains("The picture could not be shown") && t.contains(why), "{why}: {t}");
        assert!(t.contains("data:image/png;base64,"), "a piece of the text is shown");
        assert!(t.len() < 3000, "not the whole dump: {}", t.len());
        assert!(t.contains("Copy data URL") && !t.contains("Save as"), "{t}");
        assert!(t0.elapsed().as_secs() < 10, "{why}: {:?}", t0.elapsed());
    }
}

#[test]
fn a_long_base64_dump_does_not_slow_the_transcript_layout() {
    let (cx, _d) = cx_with_backend("datapic4", iyakki_agent::chat::Unconfigured);
    let big = noise_png(600, 600);
    let url = format!("data:image/png;base64,{}", b64(&big));
    assert!(url.len() > 1_500_000, "{}", url.len());
    let mut app = chat_with(&format!("Here it is:\n\n```\n{url}\n```\n"));
    let mut g = Gfx::new(1100, 700);
    let mut frame = |app: &mut ChatApp| {
        g.clear(rgb(9, 11, 20));
        app.render(&mut g, R::new(0.0, 46.0, 1100.0, 654.0), 1.0, &cx, 0);
    };
    let t0 = std::time::Instant::now();
    frame(&mut app);
    let first = t0.elapsed();
    let t = all_text(&app);
    assert!(t.len() < 2000 && t.contains("image/png data URL · "), "{}", t.len());
    // later frames and the next token of the stream cost almost nothing
    let laid_before = app.view.laid;
    let t1 = std::time::Instant::now();
    for _ in 0..5 {
        frame(&mut app);
    }
    let again = t1.elapsed();
    assert_eq!(app.view.laid, laid_before, "nothing is laid out again");
    app.msgs.last_mut().unwrap().content.push_str(" Hope you like it.");
    let t2 = std::time::Instant::now();
    frame(&mut app);
    let next = t2.elapsed();
    assert_eq!(app.pics.rasterised, 1);
    assert!(again.as_millis() / 5 < 600 && first.as_secs() < 10, "first {first:?}, five more {again:?}");
    assert!(next.as_millis() < 1500, "a new token re-reads the dump but does not draw it again: {next:?}");
    // a text dump that is not a picture does not hang either
    let junk = format!("```\n{}\n```", "QUJD".repeat(500_000));
    let mut app = chat_with(&junk);
    let t3 = std::time::Instant::now();
    shot(&mut app, &cx, 1100);
    assert!(t3.elapsed().as_secs() < 20, "{:?}", t3.elapsed());
}

#[test]
fn the_show_svg_tool_puts_the_picture_under_the_tool_block() {
    let (cx, home) = agent_cx("showsvg1", |_| vec![turn("", vec![("s1", "show_svg", json!({"svg": HELLO_SVG, "title": "Greeting"}))]), turn("There it is.", vec![])]);
    let mut app = ChatApp::new();
    app.save_home = Some(home.clone());
    app.handle(&Event::Key(Key::Ctrl('g')), &cx);
    ask(&mut app, &cx, "draw me a hello banner");
    run_until(&mut app, &cx, |a| a.pending_approval().is_some() || !a.is_running());
    assert!(app.pending_approval().is_none(), "showing a picture needs no approval");
    run_until(&mut app, &cx, |a| !a.is_running());
    let tool = app.messages().iter().find(|m| m.role == Role::Tool).expect("the tool result").clone();
    assert!(tool.content.contains("IMAGE: ") && tool.content.contains(".svg"), "{}", tool.content);
    assert_eq!(app.messages().last().unwrap().content, "There it is.");
    let g = render(&mut app, &cx);
    assert!(count(&g, is_blue) > 12_000, "the drawing is on screen: {}", tool.content);
    assert!(white_inside_blue(&g) > 60, "with its text");
    // it sits right under the tool block, before the closing answer
    let ls = app.view.layouts();
    let tool_i = ls.iter().position(|l| l.texts().first().is_some_and(|t| t == "show_svg")).expect("the tool block");
    assert!(ls[tool_i + 1].right_edge() > 150.0, "the next block is the picture");
    assert!(ls[tool_i + 2].texts().join(" ").contains("There it is."));
    assert!(!all_text(&app).contains("IMAGE:"));
}

#[test]
fn the_show_image_tool_draws_a_png_from_a_data_address() {
    let png = png(120, 80, [200, 0, 200]);
    let url = format!("data:image/png;base64,{}", b64(&png));
    let (cx, _home) = agent_cx("showimg1", |_| vec![turn("", vec![("i1", "show_image", json!({"data": url}))]), turn("Done.", vec![])]);
    let mut app = ChatApp::new();
    app.handle(&Event::Key(Key::Ctrl('g')), &cx);
    ask(&mut app, &cx, "show me a purple box");
    run_until(&mut app, &cx, |a| !a.is_running());
    let g = render(&mut app, &cx);
    assert!(count(&g, |p| p[0] > 180 && p[1] < 30 && p[2] > 180) > 8000, "{:?}", app.messages().iter().map(|m| m.content.chars().take(300).collect::<String>()).collect::<Vec<_>>());
    assert!(all_text(&app).len() < 2000 && !all_text(&app).contains(&url[..200]), "the long address is not in the transcript text either");
}

/// `IYAKKI_SHOT=/path.png cargo test -p iyakki-shell --lib svg_chat_picture -- --ignored`
#[test]
#[ignore]
fn svg_chat_picture() {
    let Ok(out) = std::env::var("IYAKKI_SHOT") else { return };
    let (cx, _d) = cx_with_backend("svgshot", iyakki_agent::chat::Unconfigured);
    let chart = "<svg xmlns='http://www.w3.org/2000/svg' width='360' height='170' viewBox='0 0 360 170'><rect width='360' height='170' rx='10' fill='#10243f'/><text x='180' y='28' font-size='16' font-weight='bold' text-anchor='middle' fill='#ffffff'>Weekly steps</text><g fill='#4f8cff'><rect x='30' y='90' width='30' height='60'/><rect x='80' y='60' width='30' height='90'/><rect x='130' y='75' width='30' height='75'/><rect x='180' y='40' width='30' height='110'/><rect x='230' y='70' width='30' height='80'/><rect x='280' y='50' width='30' height='100'/></g><g font-size='11' fill='#c8d4f0' text-anchor='middle'><text x='45' y='164'>Mon</text><text x='95' y='164'>Tue</text><text x='145' y='164'>Wed</text><text x='195' y='164'>Thu</text><text x='245' y='164'>Fri</text><text x='295' y='164'>Sat</text></g></svg>";
    let url = format!("data:image/png;base64,{}", b64(&noise_png(90, 60)));
    let mut app = ChatApp::new();
    app.msgs.push(Message::user("can you send me a hello svg, a chart and a base64 image?"));
    app.msgs.push(Message::assistant(format!("Sure. Here is the greeting:\n\n```svg\n{}\n```\n\nA small chart:\n\n{chart}\n\nAnd a base64 image:\n\n{url}\n\nThat is all.", HELLO_SVG.trim())));
    let mut g = Gfx::new(1100, 1250);
    g.clear(rgb(9, 11, 20));
    app.render(&mut g, R::new(0.0, 46.0, 1100.0, 1190.0), 1.0, &cx, 0);
    g.save_png(std::path::Path::new(&out)).unwrap();
}
