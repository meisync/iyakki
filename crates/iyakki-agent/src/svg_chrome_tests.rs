//! The SVG renderer against a second renderer: the same drawings in a real headless Chrome (F-BROX-05). Each drawing is rendered by `svg::render` and
//! by Chrome (a screenshot of the same file), both laid over white, and compared pixel by pixel: a pixel counts as different when a channel differs by
//! more than 24 of 255, and a drawing passes when no more than its own share of pixels differ (edges are anti-aliased differently, so a few percent is
//! normal). Skipped, with a printed line, where there is no Chrome. A failing drawing leaves both pictures in the temporary folder named in the message.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use iyakki_web::svg;

const SIZE: u32 = 200;

fn find_chrome() -> Option<String> {
    ["google-chrome", "chromium", "chromium-browser", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/Applications/Chromium.app/Contents/MacOS/Chromium"]
        .iter()
        .find(|c| Command::new(c).arg("--version").output().map(|o| o.status.success()).unwrap_or(false))
        .map(|c| c.to_string())
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-svgc-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Chrome's picture of `svg` (inline in a page with no margin), `SIZE` x `SIZE` from the top left corner, as RGBA over white.
fn chrome_picture(chrome: &str, dir: &Path, name: &str, svg_text: &str) -> Result<Vec<u8>, String> {
    let page = dir.join(format!("{name}.html"));
    std::fs::write(&page, format!("<!doctype html><html><body style=\"margin:0;background:#fff\">{svg_text}</body></html>")).unwrap();
    let shot = dir.join(format!("{name}-chrome.png"));
    let mut child = Command::new(chrome)
        .args(["--headless=new", "--disable-gpu", "--no-sandbox", "--no-first-run", "--hide-scrollbars", "--force-device-scale-factor=1", "--window-size=400,400", "--default-background-color=ffffffff"])
        .arg(format!("--user-data-dir={}", dir.join("profile").display()))
        .arg(format!("--screenshot={}", shot.display()))
        .arg(format!("file://{}", page.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let t = Instant::now();
    while child.try_wait().map_err(|e| e.to_string())?.is_none() {
        if t.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Chrome did not finish in a minute".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let png = std::fs::read(&shot).map_err(|e| format!("no screenshot: {e}"))?;
    let b = crate::qaimage::decode_png(&png)?;
    let mut out = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let i = ((y * b.w + x) * 4) as usize;
            out.extend_from_slice(&b.rgba[i..i + 4]);
        }
    }
    Ok(out)
}

fn over_white(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks(4)
        .flat_map(|p| {
            let a = p[3] as u32;
            let m = |c: u8| ((c as u32 * a + 255 * (255 - a) + 127) / 255) as u8;
            [m(p[0]), m(p[1]), m(p[2]), 255]
        })
        .collect()
}

/// The mean absolute difference over all channels, 0 to 255.
fn mean_difference(a: &[u8], b: &[u8]) -> f32 {
    let n = a.len() / 4 * 3;
    (0..a.len() / 4).map(|i| (0..3).map(|c| (a[i * 4 + c] as i32 - b[i * 4 + c] as i32).unsigned_abs() as f32).sum::<f32>()).sum::<f32>() / n as f32
}

/// (share of pixels that differ by more than 24 in any channel, in percent)
fn difference(a: &[u8], b: &[u8]) -> f32 {
    let n = a.len() / 4;
    let bad = (0..n).filter(|i| (0..3).any(|c| (a[i * 4 + c] as i32 - b[i * 4 + c] as i32).abs() > 24)).count();
    bad as f32 * 100.0 / n as f32
}

fn drawing(body: &str) -> String {
    format!("<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{SIZE}\" height=\"{SIZE}\" viewBox=\"0 0 {SIZE} {SIZE}\">{body}</svg>")
}

/// (name, drawing, the share of pixels allowed to differ, in percent)
fn drawings() -> Vec<(&'static str, String, f32)> {
    vec![
        ("shapes and strokes", drawing(r##"<rect x="20" y="20" width="80" height="60" fill="#d33" stroke="#222" stroke-width="6"/><circle cx="140" cy="60" r="35" fill="#39c"/><ellipse cx="60" cy="150" rx="40" ry="20" fill="none" stroke="#2a2" stroke-width="5" stroke-dasharray="10 6"/><path d="M110 120 L180 120 L145 185 Z" fill="#fc3" stroke="#000" stroke-width="3" stroke-linejoin="round"/>"##), 1.5),
        ("gradients", drawing(r##"<defs><linearGradient id="l" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#f00"/><stop offset="1" stop-color="#00f"/></linearGradient><radialGradient id="r" cx="0.5" cy="0.5" r="0.5" fx="0.3" fy="0.3"><stop offset="0" stop-color="#fff"/><stop offset="1" stop-color="#080"/></radialGradient></defs><rect x="10" y="10" width="180" height="80" fill="url(#l)"/><circle cx="100" cy="145" r="45" fill="url(#r)"/>"##), 1.5),
        ("clip path", drawing(r##"<defs><clipPath id="c"><circle cx="100" cy="100" r="60"/></clipPath></defs><rect x="0" y="0" width="200" height="200" fill="#eee"/><rect x="30" y="30" width="140" height="140" fill="#c36" clip-path="url(#c)"/>"##), 1.5),
        ("mask", drawing(r##"<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#000"/><stop offset="1" stop-color="#fff"/></linearGradient><mask id="m"><rect x="0" y="0" width="200" height="200" fill="url(#g)"/></mask></defs><rect x="20" y="40" width="160" height="120" fill="#06c" mask="url(#m)"/>"##), 1.5),
        ("group opacity and transform", drawing(r##"<g opacity="0.5" transform="translate(100 100) rotate(30) scale(1.2)"><rect x="-50" y="-30" width="100" height="60" fill="#f60"/><rect x="-20" y="-60" width="40" height="120" fill="#06f"/></g>"##), 1.5),
        ("gaussian blur", drawing(r##"<defs><filter id="b"><feGaussianBlur stdDeviation="6"/></filter></defs><rect x="50" y="50" width="100" height="100" fill="#c00" filter="url(#b)"/>"##), 2.0),
        ("drop shadow", drawing(r##"<defs><filter id="s" x="-20%" y="-20%" width="150%" height="150%"><feDropShadow dx="8" dy="8" stdDeviation="4" flood-color="#000" flood-opacity="0.6"/></filter></defs><rect x="40" y="40" width="90" height="90" fill="#3a3" filter="url(#s)"/>"##), 1.5),
        ("colour matrix", drawing(r##"<defs><filter id="f"><feColorMatrix type="saturate" values="0.2"/></filter></defs><rect x="20" y="20" width="160" height="70" fill="#e22" filter="url(#f)"/><rect x="20" y="110" width="160" height="70" fill="#22e"/>"##), 1.5),
        ("pattern", drawing(r##"<defs><pattern id="p" width="20" height="20" patternUnits="userSpaceOnUse"><rect width="10" height="10" fill="#333"/><rect x="10" y="10" width="10" height="10" fill="#c33"/></pattern></defs><rect x="20" y="20" width="160" height="160" fill="url(#p)"/>"##), 1.5),
        ("markers", drawing(r##"<defs><marker id="a" markerWidth="10" markerHeight="10" refX="5" refY="5" orient="auto"><path d="M0 0 L10 5 L0 10 Z" fill="#c00"/></marker></defs><polyline points="30,160 90,60 150,140 180,40" fill="none" stroke="#000" stroke-width="3" marker-start="url(#a)" marker-mid="url(#a)" marker-end="url(#a)"/>"##), 1.5),
        ("curves arcs and fill rules", drawing(r##"<path d="M20 100 C 20 20, 100 20, 100 100 S 180 180, 180 100" fill="none" stroke="#a0a" stroke-width="4"/><path d="M30 190 A 40 25 20 1 0 90 150" fill="none" stroke="#080" stroke-width="3" stroke-linecap="round"/><path d="M120 120 h60 v60 h-60 z M135 135 h30 v30 h-30 z" fill="#c80" fill-rule="evenodd"/>"##), 1.5),
        ("caps joins and miter", drawing(r##"<polyline points="20,60 60,20 100,60" fill="none" stroke="#000" stroke-width="14" stroke-linejoin="miter" stroke-miterlimit="10"/><polyline points="120,60 150,20 180,60" fill="none" stroke="#000" stroke-width="14" stroke-linejoin="bevel"/><line x1="30" y1="120" x2="170" y2="120" stroke="#06c" stroke-width="16" stroke-linecap="square"/><line x1="30" y1="160" x2="170" y2="160" stroke="#c60" stroke-width="16" stroke-linecap="round"/>"##), 1.5),
        ("rounded rectangles and opacity", drawing(r##"<rect x="20" y="20" width="160" height="100" rx="30" ry="20" fill="#36c" fill-opacity="0.6" stroke="#000" stroke-opacity="0.5" stroke-width="10"/><rect x="60" y="80" width="120" height="100" rx="10" fill="#c36" opacity="0.7"/>"##), 1.5),
        ("gradient spread and transform", drawing(r##"<defs><linearGradient id="g" x1="0.3" y1="0" x2="0.6" y2="0" spreadMethod="reflect" gradientTransform="rotate(20 0.5 0.5)"><stop offset="0" stop-color="#ff0"/><stop offset="1" stop-color="#00f"/></linearGradient><linearGradient id="h" x1="0.4" y1="0" x2="0.6" y2="0" spreadMethod="repeat"><stop offset="0" stop-color="#f0f"/><stop offset="1" stop-color="#0ff"/></linearGradient></defs><rect x="10" y="10" width="180" height="85" fill="url(#g)"/><rect x="10" y="105" width="180" height="85" fill="url(#h)"/>"##), 1.5),
        ("use and symbol", drawing(r##"<defs><g id="s"><circle cx="0" cy="0" r="14" fill="#c33"/><rect x="-6" y="-6" width="12" height="12" fill="#fff"/></g><symbol id="y" viewBox="0 0 10 10"><path d="M0 0 L10 0 L5 10 Z" fill="#363"/></symbol></defs><use href="#s" x="40" y="40"/><use href="#s" x="100" y="40" transform="scale(1.5)"/><use href="#y" x="20" y="100" width="60" height="60"/><use href="#y" x="110" y="100" width="70" height="40"/>"##), 1.5),
        ("preserve aspect ratio", format!("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{SIZE}\" height=\"{SIZE}\"><rect width=\"200\" height=\"200\" fill=\"#eee\"/><svg x=\"0\" y=\"0\" width=\"200\" height=\"100\" viewBox=\"0 0 100 100\" preserveAspectRatio=\"xMinYMid meet\"><circle cx=\"50\" cy=\"50\" r=\"45\" fill=\"#c00\"/></svg><svg x=\"0\" y=\"100\" width=\"200\" height=\"100\" viewBox=\"0 0 100 100\" preserveAspectRatio=\"none\"><circle cx=\"50\" cy=\"50\" r=\"45\" fill=\"#00c\"/></svg></svg>"), 1.5),
        ("composite blend and merge", drawing(r##"<defs><filter id="a" x="0" y="0" width="100%" height="100%"><feFlood flood-color="#0a0" result="f"/><feComposite in="f" in2="SourceGraphic" operator="in" result="c"/><feOffset in="c" dx="12" dy="12" result="o"/><feMerge><feMergeNode in="o"/><feMergeNode in="SourceGraphic"/></feMerge></filter><filter id="b"><feFlood flood-color="#08f" result="f"/><feBlend in="SourceGraphic" in2="f" mode="multiply"/></filter></defs><rect x="20" y="20" width="70" height="70" fill="#e50" filter="url(#a)"/><rect x="110" y="110" width="70" height="70" fill="#fc0" filter="url(#b)"/>"##), 1.5),
        ("hue rotate and luminance", drawing(r##"<defs><filter id="h"><feColorMatrix type="hueRotate" values="90"/></filter><filter id="l"><feColorMatrix type="luminanceToAlpha"/></filter></defs><rect x="10" y="10" width="85" height="85" fill="#f33" filter="url(#h)"/><rect x="105" y="10" width="85" height="85" fill="#3c3" filter="url(#h)"/><rect x="10" y="105" width="180" height="85" fill="#44e" filter="url(#l)"/>"##), 1.5),
        ("mask with colours and clip nesting", drawing(r##"<defs><mask id="m" maskContentUnits="userSpaceOnUse"><rect width="200" height="200" fill="#fff"/><circle cx="100" cy="100" r="50" fill="#444"/><circle cx="100" cy="100" r="20" fill="#000"/></mask><clipPath id="c"><rect x="20" y="20" width="160" height="160" rx="30"/></clipPath></defs><g clip-path="url(#c)"><rect width="200" height="200" fill="#6a3" mask="url(#m)"/></g>"##), 1.5),
        ("pattern with a view box and transform", drawing(r##"<defs><pattern id="p" width="0.25" height="0.25" patternContentUnits="userSpaceOnUse" patternTransform="rotate(30)"><circle cx="12" cy="12" r="9" fill="#d33"/><rect width="8" height="8" fill="#33d"/></pattern></defs><rect x="20" y="20" width="160" height="160" fill="url(#p)" stroke="#000"/>"##), 4.0),
        ("nested svg with a viewBox", drawing(r##"<rect width="200" height="200" fill="#fe9"/><svg x="40" y="40" width="120" height="120" viewBox="0 0 10 10"><rect x="0" y="0" width="10" height="10" fill="#369"/><circle cx="5" cy="5" r="3" fill="#fc0"/></svg>"##), 1.5),
        ("component transfer", drawing(r##"<defs><filter id="t"><feComponentTransfer><feFuncR type="table" tableValues="1 0.2 0.8"/><feFuncG type="linear" slope="0.5" intercept="0.25"/><feFuncB type="discrete" tableValues="0 1 0.5"/></feComponentTransfer></filter><linearGradient id="g" x1="0" x2="1"><stop offset="0" stop-color="#f33"/><stop offset="1" stop-color="#38f"/></linearGradient></defs><rect x="20" y="20" width="160" height="70" fill="url(#g)" filter="url(#t)"/><rect x="20" y="110" width="160" height="70" fill="url(#g)"/>"##), 2.5),
        ("gamma transfer", drawing(r##"<defs><filter id="t"><feComponentTransfer><feFuncR type="gamma" amplitude="1.2" exponent="2" offset="0"/><feFuncA type="linear" slope="0.6"/></feComponentTransfer></filter></defs><rect x="20" y="20" width="160" height="160" fill="#c63" filter="url(#t)"/>"##), 2.0),
        ("erode and dilate", drawing(r##"<defs><filter id="e"><feMorphology operator="erode" radius="6"/></filter><filter id="d"><feMorphology operator="dilate" radius="5 3"/></filter></defs><rect x="15" y="15" width="70" height="70" fill="#2a6" filter="url(#e)"/><rect x="115" y="15" width="40" height="40" fill="#a26" filter="url(#d)"/><circle cx="100" cy="140" r="35" fill="#36c" filter="url(#d)"/>"##), 3.0),
        ("convolve matrix", drawing(r##"<defs><filter id="c" x="0" y="0" width="100%" height="100%"><feConvolveMatrix order="3" kernelMatrix="0 -1 0 -1 5 -1 0 -1 0" preserveAlpha="true"/></filter></defs><rect x="30" y="30" width="140" height="140" fill="#ccc"/><circle cx="100" cy="100" r="40" fill="#e44" filter="url(#c)"/><rect x="60" y="60" width="80" height="80" fill="#468" filter="url(#c)"/>"##), 4.0),
        ("fractal noise", drawing(r##"<defs><filter id="n" x="0" y="0" width="100%" height="100%" color-interpolation-filters="sRGB"><feTurbulence type="fractalNoise" baseFrequency="0.03" numOctaves="3" seed="5"/></filter></defs><rect x="10" y="10" width="180" height="180" fill="#000" filter="url(#n)"/>"##), 6.0),
        ("turbulence in linear light", drawing(r##"<defs><filter id="n" x="0" y="0" width="100%" height="100%"><feTurbulence type="turbulence" baseFrequency="0.05 0.02" numOctaves="2" seed="11"/></filter></defs><rect x="10" y="10" width="180" height="180" fill="#000" filter="url(#n)"/>"##), 6.0),
    ]
}

#[test]
fn the_renderer_agrees_with_a_real_chrome_on_the_same_drawings() {
    let Some(chrome) = find_chrome() else {
        eprintln!("no Chrome or Chromium here: this test did not run");
        return;
    };
    let dir = tmp("cmp");
    let mut failures = Vec::new();
    let all = drawings();
    for (i, (name, text, allowed)) in all.clone().into_iter().enumerate() {
        let ours = svg::render(text.as_bytes(), Some((SIZE, SIZE))).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!((ours.width, ours.height), (SIZE, SIZE), "{name}");
        let ours = over_white(&ours.rgba);
        let theirs = match chrome_picture(&chrome, &dir, &format!("d{i}"), &text) {
            Ok(t) => t,
            Err(e) => panic!("{name}: {e}"),
        };
        let d = difference(&ours, &theirs);
        let mean = mean_difference(&ours, &theirs);
        eprintln!("svg vs Chrome: {name}: {d:.2} % of the pixels differ (allowed {allowed} %), mean difference {mean:.2} of 255");
        // the picture is not blank, and the comparison tells a different drawing apart (it would be no test otherwise)
        let ink = theirs.chunks(4).filter(|p| p[0] < 200 || p[1] < 200 || p[2] < 200).count();
        assert!(ink > 500, "{name}: Chrome drew almost nothing");
        let other = &all[(i + 1) % all.len()];
        let other_ours = over_white(&svg::render(other.1.as_bytes(), Some((SIZE, SIZE))).unwrap().rgba);
        assert!(difference(&other_ours, &theirs) > 2.0 * allowed.max(1.0), "{name}: a different drawing is not told apart from it");
        if mean > allowed / 2.0 {
            failures.push(format!("{name}: mean difference {mean:.2} of 255 is over {}", allowed / 2.0));
        }
        if d > allowed {
            let png_path = dir.join(format!("d{i}-ours.ppm"));
            let mut ppm = format!("P6\n{SIZE} {SIZE}\n255\n").into_bytes();
            ppm.extend(ours.chunks(4).flat_map(|p| [p[0], p[1], p[2]]));
            let _ = std::fs::write(&png_path, ppm);
            failures.push(format!("{name}: {d:.2} % differ, allowed {allowed} % (pictures in {})", dir.display()));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
    let _ = std::fs::remove_dir_all(dir);
}
