//! Speed benchmarks for the script engine (all ignored; run them one at a time in a release build):
//!   cargo test -q --release -p iyakki-js --test bench -- --ignored --nocapture --test-threads=1 [name]
//! `bench_page` replays a real page (`IYAKKI_BENCH_URL`, default https://www.youtube.com/) from a disk cache
//! (`IYAKKI_BENCH_CACHE`, default target/bench-cache) so that repeated runs measure the engine, not the network.
use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_js::{run_page, JsEngine, Limits, SiteData};
use iyakki_web::fetch::Response;
use iyakki_web::{Fetch, Url};

struct CachedWeb(std::path::PathBuf);

impl Fetch for CachedWeb {
    fn get(&self, url: &Url) -> Result<Response, String> {
        let key: String = url.to_string().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        let key = if key.len() > 150 { format!("{}_{:x}", &key[..100], key.bytes().fold(0u64, |h, b| h.wrapping_mul(1099511628211) ^ b as u64)) } else { key };
        let path = self.0.join(key);
        let miss = path.with_extension("miss");
        if miss.exists() {
            return Err("not found (cached)".into());
        }
        if !path.exists() {
            std::fs::create_dir_all(&self.0).map_err(|e| e.to_string())?;
            let st = std::process::Command::new("curl")
                .args(["-sfL", "--max-time", "60", "-A", &std::env::var("IYAKKI_BENCH_UA").unwrap_or_else(|_| "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36 Iyakki/0.1".into()), "-o"])
                .arg(&path)
                .arg(url.to_string())
                .status()
                .map_err(|e| e.to_string())?;
            if !st.success() {
                let _ = std::fs::write(&miss, b"");
                return Err("curl failed".into());
            }
        }
        let body = std::fs::read(&path).map_err(|e| e.to_string())?;
        Ok(Response { status: 200, content_type: "text/html".into(), body, ..Response::default() })
    }
}

fn big() -> Limits {
    Limits { fuel: u64::MAX / 4, wall: Duration::from_secs(3000), ..Limits::default() }
}

fn run_inline(name: &str, script: &str) {
    let html = format!("<body><div id=root></div><script>{script}</script></body>");
    let web: Arc<dyn Fetch> = Arc::new(iyakki_web::fetch::NoNetwork);
    let t = Instant::now();
    let r = run_page(&html, &Url::parse("https://a.org/").unwrap(), web, |_| true, &SiteData::default(), big());
    let el = t.elapsed();
    let out: Vec<String> = r.report.lines().into_iter().take(4).collect();
    eprintln!("BENCH {name}: {:.3} s  ran {} failed {} {:?}", el.as_secs_f64(), r.report.ran, r.report.failed, out);
}

#[test]
#[ignore]
fn bench_startup() {
    let t = Instant::now();
    let _ = JsEngine::new(big(), vec![]).unwrap();
    eprintln!("BENCH startup-first (compile+instantiate): {:.3} s", t.elapsed().as_secs_f64());
    let t = Instant::now();
    let _ = JsEngine::new(big(), vec![]).unwrap();
    eprintln!("BENCH startup-second (instantiate): {:.3} s", t.elapsed().as_secs_f64());
}

#[test]
#[ignore]
fn bench_loops() {
    run_inline("pure-loop 30M iterations", "var s=0; for (var i=0;i<30000000;i++){ s=(s+i*3)%1000003; } console.log(s);");
    run_inline("fib(30) recursion", "function f(n){return n<2?n:f(n-1)+f(n-2)} console.log(f(30));");
}

#[test]
#[ignore]
fn bench_strings_arrays_json() {
    run_inline(
        "string/array work",
        "var a=[]; for (var i=0;i<300000;i++){ a.push('item'+i); } var m=a.map(x=>x.toUpperCase()).filter(x=>x.length>6).join(',').split(',').length; var o={}; for (var i=0;i<200000;i++){o['k'+i]=i;} var t=0; for (var k in o) t+=o[k]; a.sort(); console.log(m,t);",
    );
    run_inline(
        "JSON 20x roundtrip",
        "var d=[]; for (var i=0;i<20000;i++){ d.push({id:i,name:'n'+i,tags:['a','b','c'],nested:{x:i,y:[1,2,3]}}); } var n=0; for (var r=0;r<20;r++){ n+=JSON.parse(JSON.stringify(d)).length; } console.log(n);",
    );
    run_inline("regex+replace", "var s='the quick brown fox '.repeat(50000); var c=0; for (var r=0;r<5;r++){ c+=s.replace(/o(\\w)/g,'0$1').match(/\\w+/g).length; } console.log(c);");
}

#[test]
#[ignore]
fn bench_dom() {
    run_inline(
        "DOM create/append/setAttribute 10k",
        "var root=document.getElementById('root'); for (var i=0;i<10000;i++){ var e=document.createElement('div'); e.setAttribute('class','c'+(i%10)); e.setAttribute('data-i',''+i); e.id='n'+i; e.textContent='x'+i; root.appendChild(e);} console.log(root.children.length);",
    );
    run_inline(
        "DOM querySelectorAll 200x over 10k",
        "var root=document.getElementById('root'); for (var i=0;i<10000;i++){ var e=document.createElement('div'); e.className='c'+(i%10); e.id='n'+i; root.appendChild(e);} var t=0; for (var r=0;r<200;r++){ t+=document.querySelectorAll('div.c'+(r%10)).length; t+=root.querySelectorAll('#n'+r).length; } console.log(t);",
    );
    run_inline(
        "DOM reads (getAttribute/tagName/getElementById) 10k x 10",
        "var root=document.getElementById('root'); for (var i=0;i<10000;i++){ var e=document.createElement('span'); e.setAttribute('k',''+i); e.id='n'+i; root.appendChild(e);} var t=0; var kids=root.children; for (var r=0;r<10;r++){ for (var i=0;i<kids.length;i++){ var e=kids[i]; t+=e.getAttribute('k').length+e.tagName.length; } for (var i=0;i<10000;i+=7) t+=document.getElementById('n'+i)?1:0; } console.log(t);",
    );
}

#[test]
#[ignore]
fn bench_page() {
    let url = std::env::var("IYAKKI_BENCH_URL").unwrap_or_else(|_| "https://www.youtube.com/".into());
    let dir = std::env::var("IYAKKI_BENCH_CACHE").map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/bench-cache"));
    let web: Arc<dyn Fetch> = Arc::new(CachedWeb(dir));
    let u = Url::parse(&url).unwrap();
    let mut html = String::from_utf8_lossy(&web.get(&u).unwrap().body).into_owned();
    // IYAKKI_BENCH_PRE=file.js: a script to run before the page's own (for probing)
    if let Ok(f) = std::env::var("IYAKKI_BENCH_PRE") {
        let pre = std::fs::read_to_string(f).unwrap();
        html = html.replacen("<head>", &format!("<head><script>{pre}</script>"), 1);
    }
    let t = Instant::now();
    let r = run_page(&html, &u, web, |_| true, &SiteData::default(), big());
    eprintln!("BENCH page {url}: {:.2} s  ran {} failed {} skipped {}", t.elapsed().as_secs_f64(), r.report.ran, r.report.failed, r.report.skipped);
    for l in r.report.lines().iter().take(30) {
        eprintln!("  {}", l.chars().take(4000).collect::<String>());
    }
}

#[test]
#[ignore]
fn probe_getter_only() {
    run_inline(
        "getter-only accessors",
        "var out=[]; for (var P of [Node.prototype, Element.prototype, HTMLElement.prototype, Document.prototype]) { for (var k of Object.getOwnPropertyNames(P)) { var d=Object.getOwnPropertyDescriptor(P,k); if (d.get && !d.set) out.push(P.constructor.name+'.'+k); } } console.log(out.join(' '));",
    );
}

#[test]
#[ignore]
fn bench_hostcall() {
    run_inline("200k raw __host calls (connected)", "var t=0; for (var i=0;i<200000;i++){ t+=__host('dom','connected','{\"n\":0}').length; } console.log(t);");
    run_inline("200k JSON.parse+stringify small", "var t=0; for (var i=0;i<200000;i++){ t+=JSON.parse(JSON.stringify({n:i,k:'abc'})).n; } console.log(t);");
    run_inline("200k empty loop", "var t=0; for (var i=0;i<200000;i++){ t+=i; } console.log(t);");
}
