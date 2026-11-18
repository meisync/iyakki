//! SVG pictures against a real browser: shapes drawn by our SVG renderer (alone, linked, and written inline in a page) are compared
//! pixel by pixel with what headless Chrome draws from the same text (ImageMagick's own SVG drawing is not used: it fills one pixel too far). Edges are anti-aliased differently, so a small share of
//! pixels may differ a little; wrong colours, wrong places or a missing shape fail. Skipped (with a message) when Chrome or `convert` (only used to turn Chrome's PNG into raw pixels) is missing.

use iyakki_web::image::decode;
use std::process::Command;

fn chrome() -> Option<String> {
    let mut names: Vec<String> = std::env::var("IYAKKI_CHROME").into_iter().collect();
    names.extend(["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"].map(String::from));
    names.into_iter().find(|n| Command::new(n).arg("--version").output().map(|o| o.status.success()).unwrap_or(false))
}

fn have_convert() -> bool {
    chrome().is_some() && Command::new("convert").arg("-version").output().map(|o| o.status.success()).unwrap_or(false)
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
        .arg(format!("--window-size={w},{h}"))
        .arg(format!("--screenshot={}", png.display()))
        .arg(format!("file://{}", f.display()))
        .output()
        .expect("chrome runs");
    assert!(png.is_file(), "chrome made no picture: {}", String::from_utf8_lossy(&o.stderr));
    let o = Command::new("convert").arg(&png).args(["-depth", "8", "rgba:-"]).output().expect("convert runs");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(o.stdout.len(), w * h * 4, "reference size");
    o.stdout
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
        eprintln!("SKIPPED: Chrome or convert is not installed");
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
        eprintln!("SKIPPED: Chrome or convert is not installed");
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
