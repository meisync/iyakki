//! Streams one plain question through the provider code and prints what arrives and when:
//!   cargo run --release -p iyakki-agent --example stream_probe -- MODEL "question" [none|low]
use std::sync::Arc;
use std::time::{Duration, Instant};
use iyakki_agent::chat::{ChatBackend, Message};
use iyakki_agent::providers::{ProviderCfg, ProviderChat, ProviderClient};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let model = a.first().cloned().unwrap_or("qwen3:8b".into());
    let q = a.get(1).cloned().unwrap_or("What is 17*23? Answer briefly.".into());
    let cfg: ProviderCfg = serde_json::from_value(serde_json::json!({"id":"p","kind":"ollama","base_url":"http://192.168.1.68:31434"})).unwrap();
    let up: Arc<dyn iyakki_proxy::Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(300)));
    let st: Arc<dyn iyakki_proxy::Streamer> = Arc::new(iyakki_proxy::https::UreqStreamer::with_response_timeout(Duration::from_secs(600)));
    let c = ProviderChat { client: ProviderClient::new(cfg, iyakki_agent::vault::Secrets::new(None, up.clone()), up), model, streamer: Some(st) };
    if let Some(e) = a.get(2) { iyakki_agent::providers::set_thread_reasoning(Some(Box::leak(e.clone().into_boxed_str()))); }
    let t = Instant::now();
    let mut n = 0;
    let r = c.complete_tools_stream(&[Message::user(q)], &[], &mut |p| { n += 1; if n < 6 || !p.is_empty() && n % 40 == 0 { eprintln!("[{:>6} ms] piece {:?}", t.elapsed().as_millis(), p); } true });
    println!("{:?} after {} ms, {} callbacks", r.map(|t| (t.text, t.calls.len())), t.elapsed().as_millis(), n);
}
