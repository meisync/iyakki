//! The computed styles the script engine reports (`getComputedStyle`) against a real headless Chrome's, for the same pages (F-BROW-06): the cascade (type,
//! class, id, inline, `!important`, order), inheritance, selectors, units, colours, media queries and custom properties. Each page is loaded by
//! `run_page_with` and by Chrome with the same window; for every element with an `id` a list of properties is read and the two lists are compared as
//! text, colours as red, green, blue and alpha numbers. Skipped, with a printed line, where there is no Chrome.

use std::collections::BTreeMap;
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

const PROPS: &[&str] = &[
    "display", "color", "background-color", "font-size", "font-weight", "font-style", "margin-top", "margin-right", "margin-bottom", "margin-left", "padding-top", "padding-right",
    "padding-bottom", "padding-left", "border-top-width", "border-left-width", "position", "float", "box-sizing", "line-height", "text-align", "visibility",
];

fn list_js() -> String {
    let props = PROPS.iter().map(|p| format!("'{p}'")).collect::<Vec<_>>().join(",");
    format!("JSON.stringify(Array.from(document.querySelectorAll('[id]')).map(function(e){{var s=getComputedStyle(e);return [e.id].concat([{props}].map(function(p){{return String(s.getPropertyValue(p))}}))}}))")
}

/// `[["id","v1","v2",...],...]` read back: JSON of strings, written by `JSON.stringify` on both sides.
fn parse(json: &str) -> Vec<(String, Vec<String>)> {
    // a tiny reader of an array of arrays of strings (escapes: \" \\ \/ \n and \uXXXX are all this data can hold)
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut depth = 0;
    let mut chars = json.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '[' => {
                depth += 1;
                if depth == 2 {
                    cur.clear();
                }
            }
            ']' => {
                if depth == 2 && !cur.is_empty() {
                    let id = cur.remove(0);
                    out.push((id, cur.clone()));
                }
                depth -= 1;
            }
            '"' if depth == 2 => {
                let mut s = String::new();
                while let Some(d) = chars.next() {
                    match d {
                        '"' => break,
                        '\\' => match chars.next() {
                            Some('n') => s.push('\n'),
                            Some('u') => {
                                let hex: String = chars.by_ref().take(4).collect();
                                s.push(u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32).unwrap_or('?'));
                            }
                            Some(x) => s.push(x),
                            None => break,
                        },
                        d => s.push(d),
                    }
                }
                cur.push(s);
            }
            _ => {}
        }
    }
    out
}

fn ours(html: &str, win: (f32, f32)) -> Vec<(String, Vec<String>)> {
    let web: Arc<dyn Fetch> = Arc::new(NoFiles);
    let data = SiteData { viewport: Some(Viewport { width: win.0, height: win.1, ..Viewport::default() }), ..SiteData::default() };
    let mut out = String::new();
    let js = list_js();
    let res = run_page_with(html, &Url::parse("https://a.org/").unwrap(), web, |_| true, &data, Limits::default(), |e| {
        out = e.eval("styles", &js).unwrap_or_default();
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

fn chrome(chrome: &str, dir: &Path, name: &str, html: &str) -> Result<(Vec<(String, Vec<String>)>, (f32, f32)), String> {
    let page = dir.join(format!("{name}.html"));
    let doc = html.replace("</body>", &format!("<script>var __j={};var p=document.createElement('pre');p.id='__out';p.textContent=__j;var q=document.createElement('pre');q.id='__win';q.textContent=window.innerWidth+','+window.innerHeight;document.documentElement.appendChild(p);document.documentElement.appendChild(q)</script></body>", list_js()));
    std::fs::write(&page, doc).unwrap();
    let mut child = Command::new(chrome)
        .args(["--headless=new", "--disable-gpu", "--no-sandbox", "--no-first-run", "--hide-scrollbars", "--force-device-scale-factor=1", "--window-size=800,600", "--virtual-time-budget=2000", "--dump-dom"])
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
    let between = |open: &str| -> Result<String, String> {
        let a = dom.find(open).ok_or("a list is missing in the page Chrome made")? + open.len();
        let b = a + dom[a..].find("</pre>").ok_or("a list is cut")?;
        // the page's text is HTML: the quotes arrive as they are, `&` and `<` escaped
        Ok(dom[a..b].replace("&lt;", "<").replace("&gt;", ">").replace("&amp;", "&"))
    };
    let list = parse(&between("<pre id=\"__out\">")?);
    let win = between("<pre id=\"__win\">")?;
    let (w, h) = win.split_once(',').ok_or("no window size")?;
    Ok((list, (w.parse().map_err(|_| "bad width")?, h.parse().map_err(|_| "bad height")?)))
}

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-styles-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Colours as numbers, so `rgb(0 0 0 / 50%)`, `rgba(0, 0, 0, 0.5)` and a different spacing agree; everything else is compared as text with the spaces tidied.
fn norm(v: &str) -> String {
    let t = v.trim();
    if let Some(inner) = t.strip_prefix("rgba(").or_else(|| t.strip_prefix("rgb(")).and_then(|r| r.strip_suffix(')')) {
        let n: Vec<f64> = inner.replace('/', ",").replace(',', " ").split_whitespace().filter_map(|x| x.trim_end_matches('%').parse().ok()).collect();
        if n.len() >= 3 {
            let a = n.get(3).copied().unwrap_or(1.0);
            let a = if inner.contains('%') && n.len() > 3 { a / 100.0 } else { a };
            return format!("rgb({} {} {} / {:.2})", n[0].round(), n[1].round(), n[2].round(), a);
        }
    }
    if let Some(n) = t.strip_suffix("px").and_then(|n| n.parse::<f64>().ok()) {
        return format!("{n:.2}px");
    }
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Differences that are known and accepted: (page, id, property, why). The list only shrinks.
const KNOWN: &[(&str, &str, &str, &str)] = &[
    ("inheritance and font sizes", "g", "font-weight", "`bolder` is bold here: the weights between 100 and 900 are not kept, only bold or not"),
    ("selectors", "a1", "border-top-width", "a border width without a border style counts here; a browser counts it as 0"),
];

fn pages() -> Vec<(&'static str, String)> {
    let page = |css: &str, body: &str| format!("<!doctype html><html><head><style>html,body{{margin:0;padding:0}} {css}</style></head><body>{body}</body></html>");
    vec![
        ("specificity order and important", page("p{color:#111} .a{color:#222} #i{color:#333} p.a{color:#444} .b{color:#555 !important} #j{color:#666}", "<p id=p1>x</p><p id=i class=a>x</p><p id=j class='a b'>x</p><p id=k class=a style='color:#777'>x</p><p id=l class=b style='color:#888'>x</p>")),
        ("inheritance and font sizes", page("html{font-size:20px} #a{font-size:2em} #b{font-size:80%} #c{font-size:1.5rem} #d{font-size:larger} #e{font-weight:bold} #f{font-style:italic;color:#0a0a0a} #n{font-size:10px}", "<div id=a>x<div id=b>x<span id=c>x</span></div></div><div id=n><div id=d>x</div></div><div id=e>x<b id=g>x</b><span id=h>x</span></div><div id=f>x<span id=f2>x</span></div>")),
        ("selectors", page("ul>li{color:#010101} li+li{color:#020202} li:first-child{color:#030303} li:last-child{color:#040404} li:nth-child(odd){background-color:#050505} li:nth-child(2n){background-color:#060606} li:not(.x){margin-left:3px} [data-k]{padding-left:7px} [data-k=\"v\"]{padding-top:8px} a[href^=\"http\"]{border-top-width:2px} :is(h1,h2){font-weight:700} div ~ p{margin-top:9px}", "<ul><li id=l1>1</li><li id=l2 class=x>2</li><li id=l3 data-k=v>3</li><li id=l4 data-k=w>4</li></ul><h1 id=h1>t</h1><h2 id=h2>t</h2><a id=a1 href='http://x.org'>a</a><a id=a2 href='/rel'>a</a><div></div><p id=p1>x</p><p id=p2>x</p>")),
        ("units and shorthands", page("#a{margin:10px 20px 30px 40px;padding:1em 2em} #b{margin:5px 6px;padding:3px} #c{margin:2vw 1vh;padding:5%} #d{margin:calc(10px + 5px);padding:calc(2 * 4px)} #e{border:3px solid #000;margin:0 auto} #f{font-size:12px;margin:2em .5em;padding:0 1.5em} #g{margin:1rem;padding:.25rem}", "<div id=a></div><div id=b></div><div id=c></div><div id=d></div><div id=e></div><div id=f></div><div id=g></div>")),
        ("colours", page("#a{color:#f00;background-color:#00ff0080} #b{color:rgb(10,20,30);background-color:rgba(1,2,3,.5)} #c{color:hsl(120,100%,25%);background-color:hsla(240,50%,50%,.25)} #d{color:red;background-color:transparent} #e{color:#abc;background-color:#abcd} #f{color:currentColor;background-color:rgb(10 20 30 / 40%)} #g{color:Gold;background-color:rebeccapurple}", "<div id=a></div><div id=b></div><div id=c></div><div id=d></div><div id=e></div><div id=f></div><div id=g></div>")),
        ("media queries and custom properties", page(":root{--c:#123456;--m:12px} #a{color:var(--c);margin-left:var(--m)} #b{color:var(--missing,#654321)} #c{margin-top:calc(var(--m) * 2)} @media (min-width:700px){#d{color:#0b0b0b}} @media (max-width:700px){#d{color:#0c0c0c}} @media (min-width:700px) and (max-width:900px){#e{margin-top:11px}} @media not all and (min-width:2000px){#f{margin-top:13px}}", "<div id=a></div><div id=b></div><div id=c></div><div id=d></div><div id=e></div><div id=f></div>")),
        ("display position float and visibility", page("#a{display:flex} #b{display:inline-block} #c{display:none} #d{visibility:hidden} #e{position:absolute} #f{position:relative;float:left} #g{display:grid} #h{box-sizing:border-box;text-align:center} #i{line-height:30px} #j{line-height:1.5;font-size:10px} #k{display:inline} #l{display:table} #m{display:list-item}", "<div id=a></div><span id=b></span><div id=c></div><div id=d></div><div id=e></div><div id=f></div><div id=g></div><div id=h><p id=h2>x</p></div><div id=i></div><div id=j></div><span id=k></span><div id=l></div><div id=m></div>")),
        ("default styles of elements", "<!doctype html><html><head></head><body id=bd><h1 id=h1>t</h1><h2 id=h2>t</h2><h3 id=h3>t</h3><p id=p>x</p><ul id=ul><li id=li>i</li></ul><a id=a href=#>l</a><b id=b>b</b><i id=i>i</i><strong id=s>s</strong><em id=e>e</em><pre id=pre>p</pre><blockquote id=bq>q</blockquote><table id=tb><tr id=tr><td id=td>c</td></tr></table><button id=bt>b</button><span id=sp>s</span><div id=dv>d</div><img id=im><input id=in><small id=sm>s</small><code id=cd>c</code><hr id=hr></body></html>".to_string()),
    ]
}

#[test]
fn the_computed_styles_of_the_script_engine_agree_with_a_real_chrome() {
    let Some(c) = find_chrome() else {
        eprintln!("no Chrome or Chromium here: this test did not run");
        return;
    };
    let dir = tmp();
    let mut bad: Vec<String> = Vec::new();
    let mut compared = 0usize;
    let mut table: BTreeMap<String, usize> = BTreeMap::new();
    for (i, (name, html)) in pages().into_iter().enumerate() {
        let (theirs, win) = chrome(&c, &dir, &format!("p{i}"), &html).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mine = ours(&html, win);
        assert!(!mine.is_empty() && !theirs.is_empty(), "{name}: no elements");
        for (id, vals) in &mine {
            let Some((_, cv)) = theirs.iter().find(|(k, _)| k == id) else {
                bad.push(format!("{name}: #{id} is not in Chrome's list"));
                continue;
            };
            for (j, p) in PROPS.iter().enumerate() {
                compared += 1;
                let (mut a, mut b) = (norm(vals.get(j).map(String::as_str).unwrap_or("")), norm(cv.get(j).map(String::as_str).unwrap_or("")));
                // `start` is `left` in a page that reads left to right
                if *p == "text-align" {
                    a = a.replace("start", "left");
                    b = b.replace("start", "left");
                }
                if a != b && KNOWN.iter().any(|(pg, i, pr, _)| *pg == name && i == id && pr == p) {
                    continue;
                }
                if a != b {
                    *table.entry((*p).into()).or_default() += 1;
                    bad.push(format!("{name}: #{id} {p}: ours {a:?} Chrome {b:?}"));
                }
            }
        }
    }
    eprintln!("computed styles vs Chrome: {compared} values compared, {} differ; by property: {table:?}", bad.len());
    assert!(compared > 800, "{compared} values compared");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
    let _ = std::fs::remove_dir_all(dir);
}
