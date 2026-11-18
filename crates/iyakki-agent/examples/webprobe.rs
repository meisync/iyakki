//! Try the assistant's web tools against the real internet:
//!   cargo run -p iyakki-agent --example webprobe -- fetch URL | links URL [filter] | feed URL | research TOPIC | search WORDS | raw URL
use iyakki_agent::webresearch::{Budget, Nav, Research};
use iyakki_agent::webtools::{Engine, WebClient};
use iyakki_sec::policy::Role;

fn main() {
    let mut p = Role::User.template();
    p.net_hosts = vec!["*".into()];
    let c = WebClient::new();
    let a: Vec<String> = std::env::args().skip(1).collect();
    let b = Budget::unlimited();
    let t = std::time::Instant::now();
    let out = match (a.first().map(String::as_str), a.get(1)) {
        (Some("fetch"), Some(u)) => c.fetch_page_ex(&p, u, 4000, Nav { next: a.get(2).is_some_and(|x| x == "next"), page: None }, &b),
        (Some("links"), Some(u)) => c.links_of(&p, u, a.get(2).map(String::as_str), 30, &b),
        (Some("feed"), Some(u)) => c.feed_of(&p, u, 10, &b),
        (Some("search"), Some(q)) => c.search(&p, &Engine::order(None), q, 8),
        (Some("research"), Some(q)) => c.research(&p, &Engine::order(None), &Research { topic: q.clone(), max_pages: 5, sources: vec![], fresh: true }, &b),
        (Some("raw"), Some(u)) => c.get(&p, u).map(|(f, r)| format!("{f} status {} ct {:?} bytes {} loc {:?}", r.status, r.content_type, r.body.len(), r.location)),
        _ => Err("usage".into()),
    };
    match out {
        Ok(s) => println!("{s}"),
        Err(e) => println!("ERR {e}"),
    }
    eprintln!("[{} ms]", t.elapsed().as_millis());
}
