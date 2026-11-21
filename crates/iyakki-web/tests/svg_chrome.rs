//! SVG pictures against a real browser: shapes drawn by our SVG renderer (alone, linked, and written inline in a page) are compared
//! pixel by pixel with what headless Chrome draws from the same text (ImageMagick's own SVG drawing is not used: it fills one pixel too far). Edges are anti-aliased differently, so a small share of
//! pixels may differ a little; wrong colours, wrong places or a missing shape fail. Skipped (with a message) when Chrome is missing.

use iyakki_web::image::decode;
use std::process::Command;

fn chrome() -> Option<String> {
    let mut names: Vec<String> = std::env::var("IYAKKI_CHROME").into_iter().collect();
    names.extend(["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"].map(String::from));
    names.into_iter().find(|n| Command::new(n).arg("--version").output().map(|o| o.status.success()).unwrap_or(false))
}

fn have_convert() -> bool {
    chrome().is_some()
}

/// What a real browser (headless Chrome) draws for the SVG file on a transparent background; `convert` only turns its PNG into raw RGBA.
fn reference(svg: &str, w: usize, h: usize) -> Vec<u8> {
    let d = std::env::temp_dir().join(format!("iyakki-svg-chrome-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let key = format!("{:x}", svg.len() * 31 + svg.bytes().map(|b| b as usize).sum::<usize>());
    static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let (f, png) = (d.join(format!("{key}-{n}.svg")), d.join(format!("{key}-{n}.png")));
    std::fs::write(&f, svg).unwrap();
    let o = Command::new(chrome().unwrap())
        .args(["--headless=new", "--no-sandbox", "--disable-gpu", "--hide-scrollbars", "--default-background-color=00000000", "--force-device-scale-factor=1"])
        .arg(format!("--user-data-dir={}", d.join(format!("profile-{n}")).display()))
        .arg("--window-size=400,400")
        .arg(format!("--screenshot={}", png.display()))
        .arg(format!("file://{}", f.display()))
        .output()
        .expect("chrome runs");
    assert!(png.is_file(), "chrome made no picture: {}", String::from_utf8_lossy(&o.stderr));
    let b = iyakki_web::png::decode(&std::fs::read(&png).expect("the screenshot")).expect("Chrome's PNG decodes");
    // the window is bigger than the drawing (headless Chrome's window includes its own frame): the drawing is the top left corner
    assert!(b.width as usize >= w && b.height as usize >= h, "reference size");
    let mut out = Vec::with_capacity(w * h * 4);
    for y in 0..h {
        let i = y * b.width as usize * 4;
        out.extend_from_slice(&b.rgba[i..i + w * 4]);
    }
    out
}

/// (share of pixels that differ by more than 40 in some channel, mean difference)
fn compare(ours: &[u8], want: &[u8]) -> (f64, f64) {
    let (mut big, mut sum, mut n) = (0usize, 0u64, 0usize);
    for (a, b) in ours.chunks(4).zip(want.chunks(4)) {
        // compare over white so that a transparent pixel's colour does not matter
        let flat = |p: &[u8]| -> [i32; 3] {
            let al = p[3] as i32;
            [0, 1, 2].map(|i| (p[i] as i32 * al + 255 * (255 - al)) / 255)
        };
        let (x, y) = (flat(a), flat(b));
        let m = (0..3).map(|i| (x[i] - y[i]).abs()).max().unwrap();
        if m > 40 && std::env::var("SVG_DEBUG").is_ok() {
            eprintln!("far pixel {} at ({}, {}): ours {:?} reference {:?}", n, n % 80, n / 80, a, b);
        }
        n += 1;
        sum += m as u64;
        big += (m > 40) as usize;
    }
    (big as f64 / (ours.len() / 4) as f64, sum as f64 / (ours.len() / 4) as f64)
}

const SHAPES: &[(&str, &str)] = &[
    ("two filled shapes", r##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="60" viewBox="0 0 80 60"><rect x="5" y="5" width="30" height="20" fill="#ff0000"/><circle cx="55" cy="35" r="15" fill="#0000ff"/></svg>"##),
    ("a path with a stroke", r##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="60" viewBox="0 0 80 60"><path d="M10 50 L40 10 L70 50 Z" fill="#00aa00" stroke="#000000" stroke-width="4"/></svg>"##),
    ("a gradient scaled by the view box", r##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="60" viewBox="0 0 8 6"><defs><linearGradient id="g" x1="0" x2="1" y1="0" y2="0"><stop offset="0" stop-color="#ff0000"/><stop offset="1" stop-color="#0000ff"/></linearGradient></defs><rect width="8" height="6" fill="url(#g)"/></svg>"##),
    ("opacity and an ellipse", r##"<svg xmlns="http://www.w3.org/2000/svg" width="80" height="60" viewBox="0 0 80 60"><rect width="80" height="60" fill="#ffff00"/><ellipse cx="40" cy="30" rx="30" ry="15" fill="#ff00ff" fill-opacity="0.5"/></svg>"##),
];

#[test]
fn shapes_look_like_what_a_real_renderer_draws() {
    if !have_convert() {
        eprintln!("SKIPPED: Chrome is not installed");
        return;
    }
    for (what, svg) in SHAPES {
        let ours = iyakki_web::svg::render(svg.as_bytes(), None).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!((ours.width, ours.height), (80, 60), "{what}");
        let (big, mean) = compare(&ours.rgba, &reference(svg, 80, 60));
        assert!(big < 0.04 && mean < 4.0, "{what}: {:.1}% of pixels are far off, mean difference {mean:.2}", big * 100.0);
        // the same file through the picture decoder the browser uses (it sniffs SVG from the first bytes)
        assert_eq!(decode(svg.as_bytes()).unwrap().rgba, ours.rgba, "{what}: the picture decoder and the renderer agree");
    }
}

#[test]
fn a_wrong_picture_is_caught_by_the_comparison() {
    if !have_convert() {
        return;
    }
    let (_, svg) = SHAPES[0];
    let other = svg.replace("#ff0000", "#00ff00");
    let ours = iyakki_web::svg::render(other.as_bytes(), None).unwrap();
    let (big, _) = compare(&ours.rgba, &reference(svg, 80, 60));
    assert!(big > 0.1, "a red box drawn green must fail the check");
}

#[test]
fn an_svg_written_inside_a_page_is_drawn_like_the_real_renderer_draws_it() {
    if !have_convert() {
        eprintln!("SKIPPED: Chrome is not installed");
        return;
    }
    for (what, svg) in SHAPES {
        // inside a page the xmlns is not needed and the parser lower-cases names: the page reader must put everything back
        let inline = svg.replace(r#" xmlns="http://www.w3.org/2000/svg""#, "");
        let d = iyakki_web::doc::from_html(&format!("<p>before {inline} after</p>"));
        assert_eq!(d.images.len(), 1, "{what}");
        let im = &d.images[0];
        assert_eq!((im.width, im.height), (Some(80), Some(60)), "{what}");
        let (_, bytes) = iyakki_web::image::parse_data_url(&im.src).expect("a data address");
        let ours = decode(&bytes).unwrap_or_else(|e| panic!("{what}: {e}"));
        let (big, mean) = compare(&ours.rgba, &reference(svg, 80, 60));
        assert!(big < 0.04 && mean < 4.0, "{what}: {:.1}% of pixels are far off, mean {mean:.2}", big * 100.0);
    }
}

/// A 16 x 12 PNG (red and blue squares, green with a fading alpha) used by the `image` and `feImage` drawings.
const PNG16: &str = "iVBORw0KGgoAAAANSUhEUgAAABAAAAAMCAYAAABr5z2BAAAAT0lEQVR4nGN4pqHxHxlr2NxBwYTkGYaBARonAs4jY1INBBlwGBmTY8B2ZEyOAauRMckGYGg4ETAfGZMeCycCJiNjcgxoR8bkGFCNjAkZAADXzKBQzEQKdAAAAABJRU5ErkJggg==";

fn drawing100(inner: &str) -> String {
    format!(r##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="100" height="100" viewBox="0 0 100 100">{inner}</svg>"##)
}

/// Pictures and light, drawn by us and by Chrome (F-BROX-05, KI-714): (name, drawing, share of far pixels allowed in percent, mean difference allowed).
fn picture_and_light_drawings() -> Vec<(&'static str, String, f64, f64)> {
    let img = format!("data:image/png;base64,{PNG16}");
    vec![
        ("image in a box", drawing100(&format!(r##"<rect width="100" height="100" fill="#eee"/><image href="{img}" x="10" y="10" width="80" height="60"/>"##)), 2.0, 2.0),
        ("image keeps its aspect ratio", drawing100(&format!(r##"<rect width="100" height="100" fill="#eee"/><image href="{img}" x="5" y="5" width="40" height="90" preserveAspectRatio="xMinYMin meet"/><image xlink:href="{img}" x="55" y="5" width="40" height="40" preserveAspectRatio="xMaxYMax slice"/><image href="{img}" x="55" y="55" width="40" height="30" preserveAspectRatio="none" opacity="0.5"/>"##)), 2.0, 2.0),
        ("image moved and turned by a transform", drawing100(&format!(r##"<rect width="100" height="100" fill="#fe9"/><image href="{img}" x="0" y="0" width="48" height="36" transform="translate(30 20) rotate(20)"/>"##)), 2.0, 2.0),
        ("feImage fills the filter region", drawing100(&format!(r##"<defs><filter id="f" x="0" y="0" width="100%" height="100%"><feImage href="{img}" preserveAspectRatio="none"/></filter></defs><rect x="10" y="10" width="80" height="60" fill="#000" filter="url(#f)"/>"##)), 2.0, 4.0),
        ("feImage in its own sub-region over the shape", drawing100(&format!(r##"<defs><filter id="f" x="0" y="0" width="100%" height="100%"><feImage href="{img}" x="30" y="20" width="32" height="24" result="i"/><feMerge><feMergeNode in="SourceGraphic"/><feMergeNode in="i"/></feMerge></filter></defs><rect x="10" y="10" width="80" height="80" fill="#6a3" filter="url(#f)"/>"##)), 2.0, 2.0),
        ("feTile repeats a sub-region", drawing100(r##"<defs><filter id="t" filterUnits="userSpaceOnUse" x="0" y="0" width="100" height="100"><feFlood flood-color="#e33" x="0" y="0" width="10" height="10" result="a"/><feFlood flood-color="#33e" x="10" y="0" width="10" height="10" result="b"/><feMerge x="0" y="0" width="20" height="10" result="m"><feMergeNode in="a"/><feMergeNode in="b"/></feMerge><feTile in="m"/></filter></defs><rect x="0" y="0" width="100" height="100" fill="#000" filter="url(#t)"/>"##), 2.0, 2.0),
        ("feTile of an offset picture", drawing100(r##"<defs><filter id="t" filterUnits="userSpaceOnUse" x="0" y="0" width="100" height="100"><feOffset dx="3" dy="3" x="0" y="0" width="24" height="24" result="o"/><feTile in="o"/></filter></defs><circle cx="12" cy="12" r="9" fill="#d60" filter="url(#t)"/>"##), 3.0, 3.0),
        ("distant diffuse light on a bump", drawing100(r##"<defs><filter id="l" x="0" y="0" width="100%" height="100%"><feGaussianBlur in="SourceAlpha" stdDeviation="4" result="b"/><feDiffuseLighting in="b" surfaceScale="6" diffuseConstant="1.1" lighting-color="#fff4cc"><feDistantLight azimuth="225" elevation="45"/></feDiffuseLighting></filter></defs><circle cx="50" cy="50" r="30" fill="#c33" filter="url(#l)"/>"##), 6.0, 6.0),
        ("point diffuse light", drawing100(r##"<defs><filter id="l" x="0" y="0" width="100%" height="100%"><feGaussianBlur in="SourceAlpha" stdDeviation="3" result="b"/><feDiffuseLighting in="b" surfaceScale="5" diffuseConstant="1" lighting-color="#cfe"><fePointLight x="20" y="20" z="30"/></feDiffuseLighting></filter></defs><rect x="15" y="15" width="70" height="70" rx="12" fill="#369" filter="url(#l)"/>"##), 6.0, 6.0),
        ("specular light added to the picture", drawing100(r##"<defs><filter id="l" x="0" y="0" width="100%" height="100%"><feGaussianBlur in="SourceAlpha" stdDeviation="4" result="b"/><feSpecularLighting in="b" surfaceScale="5" specularConstant="1" specularExponent="20" lighting-color="#fff" result="s"><fePointLight x="30" y="25" z="40"/></feSpecularLighting><feComposite in="s" in2="SourceAlpha" operator="in" result="c"/><feMerge><feMergeNode in="SourceGraphic"/><feMergeNode in="c"/></feMerge></filter></defs><circle cx="50" cy="50" r="32" fill="#3a6" filter="url(#l)"/>"##), 6.0, 6.0),
        ("spot light with a cone", drawing100(r##"<defs><filter id="l" x="0" y="0" width="100%" height="100%"><feGaussianBlur in="SourceAlpha" stdDeviation="2" result="b"/><feDiffuseLighting in="b" surfaceScale="3" diffuseConstant="1" lighting-color="#fd8"><feSpotLight x="20" y="10" z="60" pointsAtX="60" pointsAtY="60" pointsAtZ="0" specularExponent="4" limitingConeAngle="35"/></feDiffuseLighting></filter></defs><rect x="10" y="10" width="80" height="80" fill="#555" filter="url(#l)"/>"##), 8.0, 8.0),
    ]
}

#[test]
fn pictures_tiles_and_light_look_like_what_a_real_renderer_draws() {
    if !have_convert() {
        eprintln!("SKIPPED: Chrome is not installed");
        return;
    }
    let mut failures = vec![];
    for (what, svg, allowed_big, allowed_mean) in picture_and_light_drawings() {
        let ours = iyakki_web::svg::render(svg.as_bytes(), None).unwrap_or_else(|e| panic!("{what}: {e}"));
        assert_eq!((ours.width, ours.height), (100, 100), "{what}");
        let (big, mean) = compare(&ours.rgba, &reference(&svg, 100, 100));
        eprintln!("svg vs Chrome: {what}: {:.2} % of pixels far off, mean {mean:.2}", big * 100.0);
        if let Some(dir) = std::env::var("SVG_DUMP").ok().or_else(|| std::path::Path::new("/mnt/iyakki-build/svgdump").is_dir().then(|| "/mnt/iyakki-build/svgdump".to_string())) {
            // both pictures over white as raw files, to look at when a drawing differs (RGB, 100 x 100)
            let flat = |p: &[u8]| -> Vec<u8> { p.chunks(4).flat_map(|q| (0..3).map(move |i| ((q[i] as u32 * q[3] as u32 + 255 * (255 - q[3] as u32)) / 255) as u8)).collect() };
            let name: String = what.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
            let _ = std::fs::write(format!("{dir}/{name}.ours.rgb"), flat(&ours.rgba));
            let _ = std::fs::write(format!("{dir}/{name}.chrome.rgb"), flat(&reference(&svg, 100, 100)));
        }
        if big * 100.0 > allowed_big || mean > allowed_mean {
            failures.push(format!("{what}: {:.2} % far off (allowed {allowed_big}), mean {mean:.2} (allowed {allowed_mean})", big * 100.0));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
