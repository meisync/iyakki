// The speed report of the reader's own style and layout code (F-BROX-15): the same synthetic pages on every machine, the times of
// parsing, style, the first drawing of the page (layout and paint) and the drawings after it. Run by `dist/scripts/browser-speed-report.sh`
// (release-like build, a quiet machine); not a pass or fail test. Included (include!) into the browser's test module.

fn speed_page(blocks: usize) -> String {
    let mut s = String::from("<html><head><title>Speed</title><style>body{margin:8px;font-family:sans-serif}.c0{color:#223}.c1{color:#322;background:#eef}.c2{margin:4px 0;padding:2px}.c3{border:1px solid #ccc}.c4 b{color:#a00}.c5 a{color:#06c}.c6{background:#fafafa;padding:4px}h3{margin:6px 0 2px}p{margin:2px 0;line-height:1.4}@media (max-width:600px){.c1{display:none}}</style></head><body>");
    for i in 0..blocks {
        s.push_str(&format!("<div class=\"c{} c{}\"><h3>Item {i}: a heading of ordinary length</h3><p>The quick brown fox jumps over the lazy dog, then <b>reads</b> a <a href=\"/p/{i}\">link to page {i}</a> and keeps going for another line or two so that wrapping happens.</p></div>", i % 7, (i * 3) % 7));
    }
    s.push_str("</body></html>");
    s
}

fn median_ms(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[v.len() / 2]
}

fn frame_on(a: &mut BrowserApp, cx: &AppCx, g: &mut Gfx, w: u32, h: u32) {
    g.clear(rgb(9, 11, 20));
    a.render(g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), (w as f32 / 1280.0).min(h as f32 / 720.0).clamp(0.6, 3.0), cx, 0);
}

#[test]
#[ignore = "a measurement, run by dist/scripts/browser-speed-report.sh"]
fn speed_report_style_and_layout_of_synthetic_pages() {
    use std::time::Instant;
    let runs: usize = std::env::var("IYAKKI_SPEED_RUNS").ok().and_then(|v| v.parse().ok()).unwrap_or(5).clamp(1, 50);
    // the shell keeps one frame buffer (and its glyph cache) per pane for as long as the pane lives, so a drawing in use is a "warm" one;
    // the first drawing on a new buffer also pays for every glyph it has not drawn before ("cold")
    println!("SPEEDREPORT\tpage\tblocks\tparse_ms\tstyle_ms\tcold_first_frame_ms\twarm_frame_ms\twarm_scroll_frame_ms");
    for (name, n) in [("small", 100usize), ("medium", 1000), ("large", 4000)] {
        let page = speed_page(n);
        let url = Url::parse("https://speed.test/").unwrap();
        let vp = iyakki_web::css::Viewport::default();
        let (mut parse, mut style) = (vec![], vec![]);
        for _ in 0..runs {
            let t = Instant::now();
            let tree = iyakki_web::html::parse(&page);
            parse.push(t.elapsed().as_secs_f64() * 1000.0);
            let t = Instant::now();
            let d = iyakki_web::style::styled_doc(&tree, &url, &vp, &|_: &Url| None);
            style.push(t.elapsed().as_secs_f64() * 1000.0);
            assert!(!d.blocks.is_empty(), "the page has content");
        }
        let (mut cold, mut warm, mut scroll) = (vec![], vec![], vec![]);
        for r in 0..runs {
            let web = site(vec![("https://speed.test/", html(&page))]);
            let cx = cx_with(&format!("speed{name}{r}"), web);
            browser_config::save(&cx, &BrowserConfig { js_all: false, images: false, ..BrowserConfig::default() });
            let mut a = BrowserApp::new(&cx);
            a.open_url("speed.test", &cx);
            settle(&mut a, &cx);
            let mut g = Gfx::new(1280, 720);
            let t = Instant::now();
            frame_on(&mut a, &cx, &mut g, 1280, 720);
            cold.push(t.elapsed().as_secs_f64() * 1000.0);
            frame_on(&mut a, &cx, &mut g, 1280, 720);
            let t = Instant::now();
            for _ in 0..5 {
                frame_on(&mut a, &cx, &mut g, 1280, 720);
            }
            warm.push(t.elapsed().as_secs_f64() * 1000.0 / 5.0);
            let t = Instant::now();
            for _ in 0..5 {
                a.handle(&Event::Key(Key::PageDown), &cx);
                frame_on(&mut a, &cx, &mut g, 1280, 720);
            }
            scroll.push(t.elapsed().as_secs_f64() * 1000.0 / 5.0);
        }
        println!("SPEEDREPORT\t{name}\t{n}\t{:.1}\t{:.1}\t{:.1}\t{:.1}\t{:.1}", median_ms(parse), median_ms(style), median_ms(cold), median_ms(warm), median_ms(scroll));
    }
    // what a drawing costs with nothing in it: the frame buffer and the browser's own bars (no page)
    {
        let t = Instant::now();
        for _ in 0..20 {
            let mut g = Gfx::new(1280, 720);
            g.clear(rgb(9, 11, 20));
        }
        println!("SPEEDREPORT\tnew-buffer\t-\t-\t-\t{:.2}\t-\t-", t.elapsed().as_secs_f64() * 1000.0 / 20.0);
        let cx = cx_with("speedstart", site(vec![]));
        let mut a = BrowserApp::new(&cx);
        let mut g = Gfx::new(1280, 720);
        let t = Instant::now();
        frame_on(&mut a, &cx, &mut g, 1280, 720);
        let cold = t.elapsed().as_secs_f64() * 1000.0;
        let t = Instant::now();
        for _ in 0..10 {
            frame_on(&mut a, &cx, &mut g, 1280, 720);
        }
        println!("SPEEDREPORT\tstart-page\t-\t-\t-\t{cold:.2}\t{:.2}\t-", t.elapsed().as_secs_f64() * 1000.0 / 10.0);
    }
}
