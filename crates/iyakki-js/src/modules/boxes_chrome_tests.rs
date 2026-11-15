//! The page boxes the script engine reports (`getBoundingClientRect`) against a real headless Chrome's, for the same pages (F-BROW-28). Each page is
//! loaded by `run_page_with` with an 800 x 600 window and by Chrome with the same window size, a script lists the box of every element that has an
//! `id`, and the two lists are compared: boxes made only from sizes, margins, floats, flex, grid, tables and positions must agree to within a pixel;
//! boxes whose size comes from text (the fonts differ) to within a share of their height. Skipped, with a printed line, where there is no Chrome.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_web::css::Viewport;
use iyakki_web::fetch::Response;
use iyakki_web::{Fetch, Url};

use crate::{run_page_with, Limits, SiteData};

struct NoFiles;
impl Fetch for NoFiles {
    fn get(&self, _: &Url) -> Result<Response, String> {
        Err("offline".into())
    }
}

const LIST: &str = "JSON.stringify(Array.from(document.querySelectorAll('[id]')).map(function(e){var r=e.getBoundingClientRect();return [e.id,Math.round(r.left*100)/100,Math.round(r.top*100)/100,Math.round(r.width*100)/100,Math.round(r.height*100)/100]}))";
/// Chrome's own window, as a last entry (its headless window is not the size asked for: the browser's own bars take some of it).
const WINDOW: &str = "JSON.stringify([['__window',window.innerWidth,window.innerHeight,0,0]])";

fn parse(json: &str) -> Vec<(String, [f64; 4])> {
    // [["id",x,y,w,h],...]: small and regular, read by hand
    let mut out = Vec::new();
    for item in json.trim().trim_start_matches('[').trim_end_matches(']').split("],[") {
        let item = item.trim_start_matches('[').trim_end_matches(']');
        let mut parts = item.splitn(2, ',');
        let id = parts.next().unwrap_or("").trim_matches('"').to_string();
        let nums: Vec<f64> = parts.next().unwrap_or("").split(',').filter_map(|n| n.trim().parse().ok()).collect();
        if id.is_empty() || nums.len() != 4 {
            continue;
        }
        out.push((id, [nums[0], nums[1], nums[2], nums[3]]));
    }
    out
}

fn ours(html: &str, win: (f32, f32)) -> Vec<(String, [f64; 4])> {
    let web: Arc<dyn Fetch> = Arc::new(NoFiles);
    let data = SiteData { viewport: Some(Viewport { width: win.0, height: win.1, ..Viewport::default() }), ..SiteData::default() };
    let mut out = String::new();
    let res = run_page_with(html, &Url::parse("https://a.org/").unwrap(), web, |_| true, &data, Limits::default(), |e| {
        out = e.eval("boxes", LIST).unwrap_or_default();
    });
    assert_eq!(res.report.failed, 0, "{:?}", res.report.notes);
    parse(&out)
}

fn find_chrome() -> Option<String> {
    ["google-chrome", "chromium", "chromium-browser", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/Applications/Chromium.app/Contents/MacOS/Chromium"]
        .iter()
        .find(|c| Command::new(c).arg("--version").output().map(|o| o.status.success()).unwrap_or(false))
        .map(|c| c.to_string())
}

fn chrome(chrome: &str, dir: &Path, name: &str, html: &str) -> Result<Vec<(String, [f64; 4])>, String> {
    let page = dir.join(format!("{name}.html"));
    let doc = html.replace("</body>", &format!("<script>var __j={LIST};var __w={WINDOW};var p=document.createElement('pre');p.id='__out';p.textContent=__j;var q=document.createElement('pre');q.id='__win';q.textContent=__w;document.documentElement.appendChild(q);document.documentElement.appendChild(p)</script></body>"));
    std::fs::write(&page, doc).unwrap();
    let mut child = Command::new(chrome)
        .args(["--headless=new", "--disable-gpu", "--no-sandbox", "--no-first-run", "--hide-scrollbars", "--force-device-scale-factor=1", "--font-render-hinting=none", "--window-size=800,600", "--virtual-time-budget=2000", "--dump-dom"])
        .arg(format!("--user-data-dir={}", dir.join("profile").display()))
        .arg(format!("file://{}", page.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut so = child.stdout.take().unwrap();
    let reader = std::thread::spawn(move || {
        let mut b = Vec::new();
        let _ = std::io::Read::read_to_end(&mut so, &mut b);
        b
    });
    let t = Instant::now();
    while child.try_wait().map_err(|e| e.to_string())?.is_none() {
        if t.elapsed() > Duration::from_secs(60) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Chrome did not finish in a minute".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let dom = String::from_utf8_lossy(&reader.join().unwrap()).into_owned();
    let a = dom.find("<pre id=\"__out\">").ok_or("no list in the page Chrome made")? + "<pre id=\"__out\">".len();
    let b = a + dom[a..].find("</pre>").ok_or("the list is cut")?;
    let wa = dom.find("<pre id=\"__win\">").ok_or("no window size")? + "<pre id=\"__win\">".len();
    let wb = wa + dom[wa..].find("</pre>").ok_or("the window size is cut")?;
    let mut all = parse(&dom[a..b]);
    all.extend(parse(&dom[wa..wb]));
    Ok(all)
}

fn tmp() -> PathBuf {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("iyakki-boxes-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// (name, the page, the pixels the two may differ by for each side of a box; text-driven pages say so with a share of the height too)
fn pages() -> Vec<(&'static str, String, f64)> {
    let head = "<!doctype html><html><head><style>html,body{margin:0;padding:0} *{box-sizing:content-box}";
    let page = |css: &str, body: &str| format!("{head}{css}</style></head><body>{body}</body></html>");
    vec![
        ("sizes margins and collapsing", page("#a{margin:10px 0 0 15px;width:200px;height:30px} #b{margin:20px 0;width:100px;height:40px;padding:5px;border:3px solid #000} #o{margin:30px;height:10px} #i{margin:15px 0;height:20px}", "<div id=a></div><div id=b></div><div id=o><div id=i></div></div>"), 1.0),
        ("auto margins and max-width", page("#c{width:50%;max-width:300px;margin:0 auto;height:20px} #d{width:70%;margin:10px auto 0 20px;height:20px} #e{margin:10px;padding:0 20px;height:20px}", "<div id=c></div><div id=d></div><div id=e></div>"), 1.0),
        ("border box", page("#a{box-sizing:border-box;width:200px;height:100px;padding:20px;border:5px solid #000;margin:10px} #b{box-sizing:border-box;width:50%;padding:10px;height:60px}", "<div id=a></div><div id=b></div>"), 1.0),
        ("flex row grow shrink and gap", page(".row{display:flex;width:600px;gap:10px;margin-top:5px} .i{width:100px;height:30px} #g{flex-grow:1;height:20px} #g2{flex-grow:2;height:25px} #s{flex-shrink:1;width:300px;height:10px}", "<div class=row id=r1><div class=i id=i1></div><div class=i id=i2></div><div id=g></div><div id=g2></div></div><div class=row id=r2><div id=s></div><div class=i id=i3 style=width:400px></div></div>"), 1.0),
        ("flex justify and wrap", page(".row{display:flex;width:500px;height:100px;margin-top:10px} .i{width:100px;height:30px;flex:none} #c{justify-content:center} #sb{justify-content:space-between} #w{flex-wrap:wrap;height:auto;width:250px}", "<div class=row id=c><div class=i id=c1></div><div class=i id=c2></div></div><div class=row id=sb><div class=i id=s1></div><div class=i id=s2></div><div class=i id=s3></div></div><div class=row id=w><div class=i id=w1></div><div class=i id=w2></div><div class=i id=w3></div></div>"), 1.0),
        ("floats and clear", page("#l{float:left;width:150px;height:80px;margin:0 10px 10px 0} #r{float:right;width:100px;height:50px} #m{clear:both;height:20px} #box{overflow:hidden;width:400px} #in{float:left;width:50px;height:60px}", "<div id=l></div><div id=r></div><div id=m></div><div id=box><div id=in></div></div>"), 1.0),
        ("positions", page("#rel{position:relative;left:20px;top:10px;width:100px;height:40px} #p{position:relative;width:300px;height:200px;margin:20px} #abs{position:absolute;right:10px;bottom:10px;width:50px;height:30px} #abs2{position:absolute;left:25%;top:10px;width:40px;height:40px} #fx{position:fixed;left:5px;top:5px;width:30px;height:30px}", "<div id=rel></div><div id=p><div id=abs></div><div id=abs2></div></div><div id=fx></div>"), 1.0),
        ("inline blocks", page(".b{display:inline-block;width:120px;height:40px;margin:5px;vertical-align:top}", "<div id=row><div class=b id=b1></div><div class=b id=b2></div><div class=b id=b3></div><div class=b id=b4></div><div class=b id=b5></div><div class=b id=b6></div><div class=b id=b7></div></div>"), 1.0),
        ("grid equal columns", page("#g{display:grid;grid-template-columns:repeat(3,1fr);gap:10px;width:620px;margin:10px} .c{height:30px}", "<div id=g><div class=c id=g1></div><div class=c id=g2></div><div class=c id=g3></div><div class=c id=g4></div></div>"), 1.0),
        ("table cells", page("#t{border-collapse:collapse;width:400px} td{padding:0;height:30px} #t1{width:100px} ", "<table id=t><tr id=r1><td id=t1></td><td id=t2></td><td id=t3></td></tr><tr id=r2><td id=u1></td><td id=u2></td><td id=u3></td></tr></table>"), 2.0),
        ("margins through and between", page("#p1{margin:20px 0;border-top:1px solid #000;height:30px} #c1{margin:25px 0;height:10px} #e1{margin:15px 0} #e2{margin:30px 0;height:0} #n1{margin:10px 0;height:20px} #w{padding-top:1px;margin:5px 0} #c2{margin:40px 0 0 0;height:10px} #last{margin-bottom:50px;height:10px} #after{height:5px}", "<div id=p1><div id=c1></div></div><div id=e1></div><div id=e2></div><div id=n1></div><div id=w><div id=c2></div></div><div id=last></div><div id=after></div>"), 1.0),
        ("relative offsets in percent", page("#p{width:400px;height:200px;margin:10px} #r{position:relative;left:25%;top:10px;width:50px;height:30px} #r2{position:relative;right:10%;width:60px;height:20px}", "<div id=p><div id=r></div><div id=r2></div></div>"), 1.0),
        ("min and max sizes", page("#a{width:500px;max-width:200px;min-height:80px;height:10px} #b{width:50px;min-width:120px;max-height:30px;height:100px} #c{width:50%;min-width:500px;height:10px}", "<div id=a></div><div id=b></div><div id=c></div>"), 1.0),
        ("nested blocks with padding", page("#o{padding:10px 20px;border:2px solid #000;margin:5px} #m{margin:8px;padding:4px;border:1px solid #000} #i{height:20px;margin:3px}", "<div id=o><div id=m><div id=i></div></div></div>"), 1.0),
        ("absolute with both offsets", page("#p{position:relative;width:300px;height:200px;margin:10px;padding:10px;border:5px solid #000} #a{position:absolute;left:10px;right:20px;top:15px;bottom:25px} #b{position:absolute;right:0;bottom:0;width:20px;height:20px} #c{position:absolute;top:50%;left:50%;width:40px;height:40px}", "<div id=p><div id=a></div><div id=b></div><div id=c></div></div>"), 1.0),
        ("percentage heights", page("html,body{height:100%} #a{height:50%} #w{height:300px} #h{height:50%} #x{height:20%}", "<div id=a></div><div id=w><div id=h></div></div><div id=x></div>"), 1.0),
    ]
}

fn compare(name: &str, a: &[(String, [f64; 4])], b: &[(String, [f64; 4])], tol: f64) -> Vec<String> {
    let theirs: HashMap<&str, [f64; 4]> = b.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let mut bad = Vec::new();
    for (id, mine) in a {
        let Some(chrome) = theirs.get(id.as_str()) else {
            bad.push(format!("{name}: #{id} is not in Chrome's list"));
            continue;
        };
        let names = ["x", "y", "width", "height"];
        for i in 0..4 {
            if (mine[i] - chrome[i]).abs() > tol {
                bad.push(format!("{name}: #{id} {}: ours {} Chrome {}", names[i], mine[i], chrome[i]));
            }
        }
    }
    if a.len() != b.len() {
        bad.push(format!("{name}: {} boxes ours, {} Chrome's", a.len(), b.len()));
    }
    bad
}

#[test]
fn the_boxes_of_the_script_engine_agree_with_a_real_chrome_on_pages_without_text() {
    let Some(c) = find_chrome() else {
        eprintln!("no Chrome or Chromium here: this test did not run");
        return;
    };
    let dir = tmp();
    let mut bad = Vec::new();
    let mut checked = 0;
    for (i, (name, html, tol)) in pages().into_iter().enumerate() {
        let theirs = chrome(&c, &dir, &format!("p{i}"), &html).unwrap_or_else(|e| panic!("{name}: {e}"));
        let win = theirs.iter().find(|(k, _)| k == "__window").map(|(_, r)| (r[0] as f32, r[1] as f32)).expect("Chrome says how big its window is");
        let theirs: Vec<_> = theirs.into_iter().filter(|(k, _)| k != "__window").collect();
        let mine = ours(&html, win);
        assert!(!mine.is_empty() && mine.iter().any(|(_, r)| r[2] > 0.0), "{name}: the engine reported no boxes at all");
        let found = compare(name, &mine, &theirs, tol);
        eprintln!("boxes vs Chrome: {name}: {} boxes, {} differences", mine.len(), found.len());
        checked += mine.len();
        bad.extend(found);
    }
    assert!(checked > 40, "{checked} boxes compared");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    let _ = std::fs::remove_dir_all(dir);
}

const SAMPLE: &str = "The quick brown fox jumps over the lazy dog while seven wizards quietly mix a dozen jugs of liquid, and Jim's TV won't wake up before half past noon; 1,234.56 items cost $78.90 (about 100% more).";

/// Pages where the size of a box comes from its text: the width table of `glyphs` against the face a real Chrome draws for the same name.
/// Arial is drawn with Liberation Sans, whose widths are Arial's; Times New Roman with Liberation Serif, whose widths are Times'.
fn text_pages() -> Vec<(String, String, f64)> {
    let head = "<!doctype html><html><head><style>html,body{margin:0;padding:0} p{margin:0} span{display:inline-block;white-space:nowrap}";
    let mut v = Vec::new();
    for (face, css) in [("arial", "Arial"), ("times", "'Times New Roman'")] {
        for w in [180, 260, 340, 480] {
            let body = format!("<p id=p style=\"font:16px {css};width:{w}px\">{SAMPLE}</p>");
            v.push((format!("{face} paragraph {w}px"), format!("{head}</style></head><body>{body}</body></html>"), 1.5));
        }
        let body = format!("<span id=a style=\"font:16px {css}\">Hello, world</span><br><span id=b style=\"font:20px {css}\">{}</span><br><span id=c style=\"font:12px {css}\">iiiiiiiiii WWWWWWWWWW</span>", &SAMPLE[..44]);
        v.push((format!("{face} whole sample"), format!("{head}</style></head><body><span id=s style=\"font:16px {css}\">{SAMPLE}</span></body></html>"), 3.0));
        v.push((format!("{face} nowrap spans"), format!("{head}</style></head><body>{body}</body></html>"), 6.0));
    }
    v
}

#[test]
fn text_driven_sizes_agree_with_a_real_chrome_for_arial_and_times() {
    let Some(c) = find_chrome() else {
        eprintln!("no Chrome or Chromium here: this test did not run");
        return;
    };
    let dir = tmp();
    let mut bad = Vec::new();
    for (i, (name, html, tol)) in text_pages().into_iter().enumerate() {
        let theirs = chrome(&c, &dir, &format!("t{i}"), &html).unwrap_or_else(|e| panic!("{name}: {e}"));
        let win = theirs.iter().find(|(k, _)| k == "__window").map(|(_, r)| (r[0] as f32, r[1] as f32)).expect("window");
        let theirs: Vec<_> = theirs.into_iter().filter(|(k, _)| k != "__window").collect();
        let mine = ours(&html, win);
        let found = compare(&name, &mine, &theirs, tol);
        eprintln!("text vs Chrome: {name}: {} boxes, {} differences {:?}", mine.len(), found.len(), found);
        bad.extend(found);
    }
    let _ = std::fs::remove_dir_all(dir);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
