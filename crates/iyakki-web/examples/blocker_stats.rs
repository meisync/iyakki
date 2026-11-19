//! What a real filter list does to the blocker: `cargo run --release -p iyakki-web --example blocker_stats -- LIST.txt [LIST.txt...]`
//! Prints rule counts by kind, lines skipped and dropped, compile time, memory estimate, microseconds per request over a sample of
//! typical addresses, and the size of the WebKit rule list the full engine would get.

use std::time::Instant;

use iyakki_web::blocker::{Compiler, ReqInfo, ResType};

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    if files.is_empty() {
        eprintln!("usage: blocker_stats LIST.txt [LIST.txt...]");
        std::process::exit(2);
    }
    let texts: Vec<String> = files.iter().map(|f| std::fs::read_to_string(f).unwrap_or_default()).collect();
    let t = Instant::now();
    let mut c = Compiler::new();
    for (i, t) in texts.iter().enumerate() {
        c.add_list(t, i.min(255) as u8);
    }
    let b = c.finish();
    let compile = t.elapsed();
    let st = b.stats();
    println!("lists {}  lines used {}  blocking {}  exceptions {}  hiding {}  page allows {}  skipped {}  dropped {}", st.lists, st.lines, st.blocking, st.exceptions, st.cosmetic, st.page_allows, st.skipped, st.dropped);
    println!("compile {:.0} ms  memory about {:.1} MB", compile.as_secs_f64() * 1000.0, b.memory_estimate() as f64 / 1048576.0);
    let sample: Vec<(&str, &str, ResType)> = vec![
        ("https://www.youtube.com/s/player/abc/player_ias.vflset/en_US/base.js", "https://www.youtube.com/watch?v=x", ResType::Script),
        ("https://i.ytimg.com/vi/x/hqdefault.jpg", "https://www.youtube.com/", ResType::Image),
        ("https://googleads.g.doubleclick.net/pagead/id", "https://www.youtube.com/", ResType::Xhr),
        ("https://www.google-analytics.com/analytics.js", "https://www.bbc.com/news", ResType::Script),
        ("https://static.files.bbci.co.uk/orbit/x/js/main.js", "https://www.bbc.com/news", ResType::Script),
        ("https://www.gstatic.com/og/_/js/k=og.og2.en_US.x/rt=j/m=q_dnp.js", "https://www.google.com/search?q=rust", ResType::Script),
        ("https://upload.wikimedia.org/wikipedia/commons/a/a9/x.png", "https://en.wikipedia.org/wiki/Rust", ResType::Image),
        ("https://securepubads.g.doubleclick.net/tag/js/gpt.js", "https://www.cnn.com/", ResType::Script),
        ("https://cdn.example.org/app.chunk.1234.js", "https://example.org/", ResType::Script),
        ("https://pixel.example.net/track?id=1&ref=2", "https://example.org/", ResType::Image),
    ];
    let mut blocked = 0;
    let n = 20000;
    let t = Instant::now();
    for i in 0..n {
        let (u, p, k) = sample[i % sample.len()];
        if b.check(&ReqInfo { url: u, initiator: p, kind: k }).is_block() {
            blocked += 1;
        }
    }
    let per = t.elapsed().as_secs_f64() * 1e6 / n as f64;
    println!("{per:.2} us per request over {} sample addresses ({} of {n} blocked)", sample.len(), blocked);
    for (u, p, k) in &sample {
        let v = b.check(&ReqInfo { url: u, initiator: p, kind: *k });
        println!("  {:<8} {}  {}", if v.is_block() { "BLOCKED" } else { "allowed" }, u, if let iyakki_web::blocker::Verdict::Block { rule } = v { b.describe(rule) } else { String::new() });
    }
    let t = Instant::now();
    let j = iyakki_web::blocker_json::export(&b, iyakki_web::blocker_json::DEFAULT_LIMIT, true);
    println!("WebKit rule list: {} rules, {:.1} MB of JSON, {} not expressible, {} over the limit, written in {:.0} ms", j.rules, j.json.len() as f64 / 1048576.0, j.unsupported, j.over_limit, t.elapsed().as_secs_f64() * 1000.0);
}
