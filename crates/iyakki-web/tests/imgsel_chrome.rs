//! Picture choice against a real browser (F-PICT-01, F-BROW-05): for a set of `<img srcset sizes>`, `<picture><source media type>` pages and
//! four window and density settings, the address the reader picks (`imgsel::image_source`) is compared with the one headless Chrome picks
//! (`currentSrc`). Skipped, with a message, when Chrome is missing. The images do not exist: Chrome decides on the address before it loads.

use iyakki_web::{html, imgsel};
use std::process::Command;

fn chrome() -> Option<String> {
    let mut names: Vec<String> = std::env::var("IYAKKI_CHROME").into_iter().collect();
    names.extend(["google-chrome", "google-chrome-stable", "chromium", "chromium-browser"].map(String::from));
    names.into_iter().find(|n| Command::new(n).arg("--version").output().map(|o| o.status.success()).unwrap_or(false))
}

/// (name, html of one case; the first `<img>` is the one looked at)
const CASES: &[(&str, &str)] = &[
    ("plain src", r#"<img src="a.png">"#),
    ("density list", r#"<img src="a.png" srcset="a.png 1x, b.png 2x, c.png 3x">"#),
    ("density without the first", r#"<img src="a.png" srcset="b.png 2x, c.png 3x">"#),
    ("density that skips", r#"<img src="a.png" srcset="a.png, c.png 3x">"#),
    ("fractions", r#"<img src="a.png" srcset="a.png 0.5x, b.png 1.5x">"#),
    ("widths and 100vw", r#"<img src="a.jpg" srcset="s.jpg 400w, m.jpg 800w, l.jpg 1600w" sizes="100vw">"#),
    ("widths and a slot in pixels", r#"<img src="a.jpg" srcset="s.jpg 400w, m.jpg 800w, l.jpg 1600w" sizes="400px">"#),
    ("widths and a half window", r#"<img src="a.jpg" srcset="s.jpg 400w, m.jpg 800w, l.jpg 1600w" sizes="50vw">"#),
    ("widths and a media condition", r#"<img src="a.jpg" srcset="s.jpg 400w, m.jpg 800w, l.jpg 1600w" sizes="(min-width: 700px) 400px, 100vw">"#),
    ("widths and no sizes", r#"<img src="a.jpg" srcset="s.jpg 400w, m.jpg 800w, l.jpg 1600w">"#),
    ("picture with a media source", r#"<picture><source media="(min-width: 700px)" srcset="wide.png"><img src="narrow.png"></picture>"#),
    ("picture with a max-width source", r#"<picture><source media="(max-width: 600px)" srcset="small.png"><img src="big.png"></picture>"#),
    ("picture with a density source", r#"<picture><source media="(min-resolution: 2dppx)" srcset="hd.png"><img src="sd.png"></picture>"#),
    ("picture with types the reader draws", r#"<picture><source type="image/webp" srcset="x.webp"><source type="image/png" srcset="x.png"><img src="x.jpg"></picture>"#),
    ("picture with a type nobody draws", r#"<picture><source type="image/x-nothing" srcset="x.nothing"><img src="x.jpg"></picture>"#),
    ("picture first source wins", r#"<picture><source srcset="one.png"><source srcset="two.png"><img src="x.jpg"></picture>"#),
    ("picture with widths", r#"<picture><source srcset="s.jpg 400w, l.jpg 1600w" sizes="100vw"><img src="x.jpg"></picture>"#),
    ("picture source after the image is ignored", r#"<picture><img src="x.jpg"><source srcset="late.png"></picture>"#),
    ("srcset beats src", r#"<img src="old.png" srcset="new.png">"#),
    ("empty srcset keeps src", r#"<img src="keep.png" srcset="">"#),
];

/// (window width, density)
const SETTINGS: &[(u32, f32)] = &[(800, 1.0), (800, 2.0), (500, 1.0), (1200, 3.0)];

fn chrome_picks(width: u32, dppx: f32) -> (u32, Vec<String>) {
    let dir = std::env::temp_dir().join(format!("iyakki-imgsel-{}-{width}-{}", std::process::id(), dppx as u32));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut body = String::new();
    for (i, (_, h)) in CASES.iter().enumerate() {
        body.push_str(&format!("<div id=\"c{i}\">{h}</div>\n"));
    }
    let page = format!("<!doctype html><meta charset=utf-8><body style=\"margin:0\">{body}<pre id=out></pre><script>window.addEventListener('load', () => {{ const o = []; for (let i = 0; i < {n}; i++) {{ const im = document.querySelector('#c' + i + ' img'); o.push(im.currentSrc); }} document.getElementById('out').textContent = ['RESULT', innerWidth].concat(o).join('|'); }});</script>", n = CASES.len());
    let f = dir.join("page.html");
    std::fs::write(&f, page).unwrap();
    let o = Command::new(chrome().unwrap())
        .args(["--headless=new", "--no-sandbox", "--disable-gpu", "--hide-scrollbars", "--virtual-time-budget=4000", "--dump-dom"])
        .arg(format!("--force-device-scale-factor={dppx}"))
        .arg(format!("--user-data-dir={}", dir.join("profile").display()))
        .arg(format!("--window-size={width},800"))
        .arg(format!("file://{}", f.display()))
        .output()
        .expect("chrome runs");
    let dom = String::from_utf8_lossy(&o.stdout).into_owned();
    let at = dom.find("RESULT|").unwrap_or_else(|| panic!("no result from chrome: {dom:.400} {}", String::from_utf8_lossy(&o.stderr)));
    let end = dom[at..].find("</pre>").map(|e| at + e).unwrap_or(dom.len());
    let parts: Vec<&str> = dom[at + 7..end].split('|').collect();
    let w: u32 = parts[0].parse().unwrap();
    let base = format!("file://{}/", dir.display());
    let picks: Vec<String> = parts[1..].iter().map(|s| s.replacen(&base, "", 1)).collect();
    let _ = std::fs::remove_dir_all(&dir);
    (w, picks)
}

#[test]
fn the_picture_the_reader_chooses_is_the_picture_a_real_chrome_chooses() {
    if chrome().is_none() {
        eprintln!("SKIPPED: Chrome is not installed (the picture choice was not compared with a browser)");
        return;
    }
    let mut checked = 0;
    let mut differ = Vec::new();
    for &(width, dppx) in SETTINGS {
        let (inner, theirs) = chrome_picks(width, dppx);
        assert_eq!(theirs.len(), CASES.len());
        for (i, (name, h)) in CASES.iter().enumerate() {
            let t = html::parse(h);
            let n = (0..t.nodes.len()).find(|&n| t.tag(n) == Some("img")).unwrap();
            let ours = imgsel::with_window_dppx(inner as f32, dppx, || imgsel::image_source(&t, n));
            checked += 1;
            if ours != theirs[i] {
                differ.push(format!("{name} at {inner}px x{dppx}: reader {ours:?}, Chrome {:?}", theirs[i]));
            }
        }
    }
    eprintln!("picture choice: {checked} choices compared with Chrome, {} differ", differ.len());
    for d in &differ {
        eprintln!("  DIFF {d}");
    }
    assert!(differ.is_empty(), "{differ:#?}");
}
