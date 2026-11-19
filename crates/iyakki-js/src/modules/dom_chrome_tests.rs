//! Script behaviour against a real headless Chrome (F-BROW-18, F-BJS-01): one battery of small expressions about the page's objects (the tree, classes,
//! attributes, serialization, selectors, events, URLs, storage, forms and the language itself) is run in the script engine of the reader and in
//! Chrome, each on a fresh copy of the same small page, and every answer is compared as text. Answers that differ on purpose are listed in
//! `KNOWN` with the reason (a number that must not grow); any other difference fails the test and prints both answers. Skipped, with a printed line,
//! where there is no Chrome.

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

const FIXTURE: &str = "<div id=a class=\"x y\" data-k=v title=t><span>one</span><span>two</span></div><p id=p>text <b>bold</b> tail</p><ul id=ul><li>1</li><li>2</li></ul><input id=in value=val><a id=lnk href=\"/path?q=1#h\">l</a>";

/// The battery: `name | expression` per line.
const BATTERY: &str = include_str!("dom_chrome_battery.txt");

/// Differences that are known and accepted: (case name, why). The list only shrinks.
const KNOWN: &[(&str, &str)] = &[
    ("style cssText", "a shorthand written into a style declaration (`margin:1px`) is not split into its four sides"),
    ("input value set", "the `value` property writes the attribute too: the reader's own forms read the attribute, so a script's edit has to reach it"),
    ("checkbox", "`checked` is the attribute too, for the same reason"),
    ("document readyState", "scripts run after the page was read, so it says `complete`, not `loading`"),
    ("location parts", "the test page is a file in Chrome and an https address here"),
    ("cookie", "a file page has no cookies in Chrome; the test page here is an https address"),
    ("constructor names", "`document` is a `Document`, not an `HTMLDocument`"),
    ("Object.prototype.toString", "`document` and `attributes` are not told apart by `Object.prototype.toString`"),
    ("element ids global", "an element with an id is not a property of `window`"),
    ("url 42 'https:'", "`https:` alone, a scheme with no authority, is read as a path here; the standard keeps the base for it"),
    ("parse adoption agency", "misnested formatting tags (`<b><i>x</b>y</i>`) are not rebuilt by the adoption agency algorithm"),
    ("parse bad nesting", "the same: a `<b>` that is closed inside a paragraph is not reopened"),
    ("parse comment", "comments are dropped by the parser, so a script never sees one"),
    ("sort stability and locale", "`localeCompare` compares code points: QuickJS has no collation tables, a browser puts `C` after `b`"),
];

fn cases() -> Vec<(String, String)> {
    BATTERY
        .lines()
        .filter(|l| !l.trim().is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once(" | "))
        .map(|(n, e)| (n.trim().to_string(), e.trim().to_string()))
        .collect()
}

/// The script both sides run: it leaves the answers, as a JSON array of strings, in `window.__results`.
fn script() -> String {
    let cases: Vec<(String, String)> = cases();
    let js_cases = serde_json::to_string(&cases.iter().map(|(_, e)| e.as_str()).collect::<Vec<_>>()).unwrap();
    let fixture = serde_json::to_string(FIXTURE).unwrap();
    format!(
        "(function(){{var CASES={js_cases};var FIX={fixture};var out=[],pend=[];\
         function fresh(){{var nb=document.createElement('body');nb.innerHTML=FIX;document.documentElement.replaceChild(nb,document.body)}}\
         CASES.forEach(function(code,i){{fresh();var a=document.getElementById('a'),p=document.getElementById('p'),ul=document.getElementById('ul');\
         var r;try{{r=(new Function('a','p','ul','return ('+code+')'))(a,p,ul)}}catch(e){{r='THROWS '+(e&&e.name)}}\
         if(r&&typeof r.then==='function'){{pend.push(r.then(function(v){{out[i]=String(v)}},function(e){{out[i]='REJECT'}}))}}else{{out[i]=String(r)}}}});\
         Promise.all(pend).then(function(){{window.__results=JSON.stringify(out)}})}})()"
    )
}

fn tidy(s: &str) -> String {
    s.to_string()
}

fn parse_strings(json: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = json.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '"' {
            continue;
        }
        let mut s = String::new();
        while let Some(d) = chars.next() {
            match d {
                '"' => break,
                '\\' => match chars.next() {
                    Some('n') => s.push('\n'),
                    Some('t') => s.push('\t'),
                    Some('r') => s.push('\r'),
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
        out.push(s);
    }
    out
}

fn ours() -> Vec<String> {
    let page = format!("<!doctype html><html><head></head><body>{FIXTURE}</body></html>");
    let web: Arc<dyn Fetch> = Arc::new(NoFiles);
    let data = SiteData { viewport: Some(Viewport { width: 800.0, height: 600.0, ..Viewport::default() }), ..SiteData::default() };
    let js = script();
    let mut out = String::new();
    let res = run_page_with(&page, &Url::parse("https://a.org/").unwrap(), web, |_| true, &data, Limits::default(), |e| {
        let _ = e.eval("battery", &js);
        let _ = e.run_jobs();
        let _ = crate::modules::run_timers(e, 1_000, 100);
        out = e.eval("read", "typeof window.__results === 'undefined' ? '' : window.__results").unwrap_or_default();
    });
    let _ = res;
    parse_strings(&out)
}

fn find_chrome() -> Option<String> {
    ["google-chrome", "chromium", "chromium-browser", "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/Applications/Chromium.app/Contents/MacOS/Chromium"]
        .iter()
        .find(|c| Command::new(c).arg("--version").output().map(|o| o.status.success()).unwrap_or(false))
        .map(|c| c.to_string())
}

fn chrome(chrome: &str, dir: &Path) -> Result<Vec<String>, String> {
    let page = dir.join("battery.html");
    let doc = format!(
        "<!doctype html><html><head><script>setTimeout(function(){{var p=document.createElement('pre');p.id='__out';p.textContent=window.__results||'';document.documentElement.appendChild(p)}},300)</script></head><body>{FIXTURE}<script>{}</script></body></html>",
        script().replace("</script", "<\\/script")
    );
    std::fs::write(&page, doc).unwrap();
    let mut child = Command::new(chrome)
        .args(["--headless=new", "--disable-gpu", "--no-sandbox", "--no-first-run", "--hide-scrollbars", "--window-size=800,600", "--virtual-time-budget=5000", "--dump-dom"])
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
        if t.elapsed() > Duration::from_secs(90) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Chrome did not finish in 90 seconds".into());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let dom = String::from_utf8_lossy(&reader.join().unwrap()).into_owned();
    let open = "<pre id=\"__out\">";
    let a = dom.find(open).ok_or("no answers in the page Chrome made")? + open.len();
    let b = a + dom[a..].find("</pre>").ok_or("the answers are cut")?;
    Ok(parse_strings(&dom[a..b].replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&amp;", "&")))
}

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-dombat-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn the_script_engine_answers_a_battery_of_page_questions_as_a_real_chrome_does() {
    let Some(c) = find_chrome() else {
        eprintln!("no Chrome or Chromium here: this test did not run");
        return;
    };
    let dir = tmp();
    let cases = cases();
    let theirs = chrome(&c, &dir).unwrap_or_else(|e| panic!("{e}"));
    let mine = ours();
    assert_eq!(theirs.len(), cases.len(), "Chrome answered {} of {}", theirs.len(), cases.len());
    assert_eq!(mine.len(), cases.len(), "the engine answered {} of {}", mine.len(), cases.len());
    let mut bad: Vec<String> = Vec::new();
    let mut known_seen = 0;
    for (i, (name, expr)) in cases.iter().enumerate() {
        let (a, b) = (tidy(&mine[i]), tidy(&theirs[i]));
        if a == b {
            continue;
        }
        if KNOWN.iter().any(|(n, _)| n == name) {
            known_seen += 1;
            continue;
        }
        bad.push(format!("{name}: {expr}\n    ours:   {a:?}\n    Chrome: {b:?}"));
    }
    eprintln!("page questions vs Chrome: {} asked, {} differ ({} known)", cases.len(), bad.len() + known_seen, known_seen);
    assert!(cases.len() >= 100, "{} cases", cases.len());
    assert!(bad.is_empty(), "{} answers differ from Chrome's:\n{}", bad.len(), bad.join("\n"));
    let _ = std::fs::remove_dir_all(dir);
}
