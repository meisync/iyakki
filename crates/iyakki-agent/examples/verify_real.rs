//! Checks the pieces that are otherwise only tested against fakes, against the real services:
//!
//!   cargo run -p iyakki-agent --example verify_real -- ollama [http://localhost:11434]
//!   cargo run -p iyakki-agent --example verify_real -- clis [id ...]
//!   cargo run -p iyakki-agent --example verify_real -- ollama-server
//!
//! `ollama` lists models, asks one for a short answer, tries to download a name that does not exist and a very small
//! real model, and asks for a picture when no picture model exists. `clis` installs each tool from its real vendor into a
//! temporary folder, prints what was found and removes it again. Nothing outside the temporary folders is changed, except
//! that the small Ollama model is downloaded (and is deleted again with `ollama rm`).

use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_agent::chat::Message;
use iyakki_agent::providers::{Kind, ProviderCfg};
use iyakki_agent::services::Services;
use iyakki_proxy::https::{UreqStreamer, UreqUpstream};

fn services(tag: &str) -> Arc<Services> {
    let dir = std::env::temp_dir().join(format!("iyakki-verify-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let quick: Arc<dyn iyakki_proxy::Upstream> = Arc::new(UreqUpstream::with_timeout(Duration::from_secs(10)));
    let slow: Arc<dyn iyakki_proxy::Upstream> = Arc::new(UreqUpstream::with_timeout(Duration::from_secs(300)));
    let s = Services::open(&dir, quick, slow).expect("services");
    s.set_streamer(Arc::new(UreqStreamer::default()));
    s
}

fn ollama(base: &str) {
    let s = services("ollama");
    let mut p = ProviderCfg::new("ollama", Kind::Ollama);
    p.base_url = Some(base.to_string());
    s.settings.lock().unwrap().upsert_provider(p).unwrap();
    println!("== models");
    let models = s.all_models();
    for (p, m) in &models {
        println!("   {p}: {m}{}", if iyakki_agent::models::is_image_model(m) { "  [draws pictures]" } else { "" });
    }
    if let Some((_, m)) = models.iter().find(|(_, m)| !iyakki_agent::models::is_image_model(m)) {
        println!("== a short answer from {m}");
        let t = Instant::now();
        let r = s.client_for("ollama", false).unwrap().chat(m, &[Message::user("Reply with exactly the word: ready")]);
        match r {
            Ok(a) => println!("   ok in {:.1}s: {:?}", t.elapsed().as_secs_f32(), a.chars().take(80).collect::<String>()),
            Err(e) => println!("   FAILED: {e}"),
        }
    }
    println!("== download of a name that does not exist");
    s.pull_model("ollama", "qwen-image-2.1").unwrap();
    wait_pull(&s, "qwen-image-2.1");
    println!("== download of a very small real model");
    s.pull_model("ollama", "smollm2:135m").unwrap();
    wait_pull(&s, "smollm2:135m");
    println!("== models after the download");
    for (p, m) in s.all_models() {
        println!("   {p}: {m}");
    }
    println!("== a picture without a picture model");
    match s.generate_image(None, "a red kite over a beach", "512x512") {
        Ok((p, m, b, ext)) => println!("   drew with {p}/{m}: {} bytes .{ext}", b.len()),
        Err(e) => println!("   {e}"),
    }
    println!("(remove the small model again with: ollama rm smollm2:135m)");
}

fn wait_pull(s: &Services, model: &str) {
    let t = Instant::now();
    let mut last = String::new();
    while t.elapsed() < Duration::from_secs(300) {
        if let Some(st) = s.pull_status().into_iter().find(|p| p.model == model) {
            let d = st.describe();
            if d != last && (st.done || st.error.is_some() || t.elapsed().as_secs() % 3 == 0) {
                println!("   {d}");
                last = d;
            }
            if st.done || st.error.is_some() {
                return;
            }
        }
        std::thread::sleep(Duration::from_millis(300));
    }
    println!("   timed out");
}

fn clis(ids: &[String]) {
    let s = services("clis");
    let all: Vec<&str> = iyakki_agent::clis::CATALOG.iter().map(|d| d.id).collect();
    let ids: Vec<String> = if ids.is_empty() { all.iter().map(|s| s.to_string()).collect() } else { ids.to_vec() };
    for id in ids {
        let t = Instant::now();
        println!("== {id}");
        if let Err(e) = s.clis.install(&id, "verify") {
            println!("   cannot start: {e}");
            continue;
        }
        let mut last = String::new();
        loop {
            let Some(j) = s.clis.job(&id) else { break };
            if j.error.is_some() || j.finished {
                break;
            }
            let d = j.describe();
            if d != last && t.elapsed().as_secs() % 5 == 0 {
                println!("   {d}");
                last = d;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        match s.clis.job(&id) {
            Some(j) if j.error.is_some() => println!("   FAILED after {:.0}s: {}", t.elapsed().as_secs_f32(), j.error.unwrap_or_default()),
            _ => match s.clis.is_installed(&id) {
                Some(i) => println!("   installed {} in {:.0}s, checksum verified: {}, program says: {}", i.version, t.elapsed().as_secs_f32(), i.verified, i.reported),
                None => println!("   not installed"),
            },
        }
        let _ = s.clis.remove(&id);
    }
}

/// Installs the real Ollama (a download of over a gigabyte) from its vendor, starts it as a local server on a free port
/// with its models in a temporary folder, asks it for its version and its model list through the Models provider that is added
/// when it answers, and stops it again. Put TMPDIR on a real disk: the install unpacks several gigabytes.
fn ollama_server() {
    let s = services("ollama-server");
    let t = Instant::now();
    println!("== installing Ollama (a big download)");
    s.clis.install("ollama", "verify").expect("start install");
    let mut last = String::new();
    loop {
        let j = s.clis.job("ollama").unwrap();
        let d = j.describe();
        if d != last && t.elapsed().as_secs() % 10 == 0 {
            println!("   {d}");
            last = d;
        }
        if j.finished || j.error.is_some() {
            println!("   {} (after {:.0}s)", j.describe(), t.elapsed().as_secs_f32());
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let Some(i) = s.clis.is_installed("ollama") else { return println!("   not installed, stopping here") };
    println!("   installed {}, checksum verified: {}, program says: {}", i.version, i.verified, i.reported);
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    s.ollama.set_port(port);
    println!("== starting the server on port {port}");
    s.ollama.start().expect("start");
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(120) {
        println!("   {:?}", s.ollama.state());
        if !matches!(s.ollama.state(), iyakki_agent::ollama::OllamaState::Starting) {
            break;
        }
        std::thread::sleep(Duration::from_secs(2));
    }
    println!("   state after {:.0}s: {:?}", t.elapsed().as_secs_f32(), s.ollama.state());
    let providers: Vec<String> = s.settings.lock().unwrap().get().providers.iter().map(|p| format!("{} ({:?})", p.id, p.base_url)).collect();
    println!("== providers now: {providers:?}");
    println!("== models (a fresh server has none)");
    for (p, m) in s.all_models() {
        println!("   {p}: {m}");
    }
    println!("== log tail\n{}", s.ollama.log_tail(6));
    println!("== stopping");
    println!("   {:?}", s.ollama.stop());
    let _ = s.clis.remove("ollama");
}

/// Needs root and cgroup v2: run it inside a privileged container (see the notes in FEATURES.md F-CLI-03). Installs the real k3s,
/// starts the cluster, runs real code in containers and checks the limits that are promised.
fn cluster() {
    let s = services("cluster");
    println!("== installing k3s");
    s.clis.install("k3s", "verify").expect("start install");
    loop {
        let j = s.clis.job("k3s").unwrap();
        if j.finished || j.error.is_some() {
            println!("   {}", j.describe());
            break;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    let missing = s.cluster.requirements();
    println!("== requirements missing: {missing:?}");
    println!("== starting the cluster");
    let t = Instant::now();
    s.cluster.start().expect("start");
    while t.elapsed() < Duration::from_secs(300) {
        match s.cluster.state() {
            iyakki_agent::cluster::ClusterState::Running => break,
            iyakki_agent::cluster::ClusterState::Failed(e) => {
                println!("   FAILED: {e}");
                println!("{}", s.cluster.log_tail(30));
                return;
            }
            _ => std::thread::sleep(Duration::from_secs(2)),
        }
    }
    println!("   state: {} after {:.0}s", s.cluster.state().label(), t.elapsed().as_secs_f32());
    let run = |label: &str, lang: &str, code: &str, secs: u64, net: bool| {
        println!("== {label}");
        let t = Instant::now();
        match s.cluster.sandbox_run(lang, code, secs, net) {
            Ok(r) => println!("   ok={} in {:.0}s: {}", r.ok, t.elapsed().as_secs_f32(), r.output.trim().replace('\n', " | ")),
            Err(e) => println!("   ERROR: {e}"),
        }
    };
    run("hello from python (the first run fetches the image)", "python", "print('hello', 2 + 2)", 120, false);
    run("who am I (should not be root)", "python", "import os; print('uid', os.getuid(), 'gid', os.getgid())", 60, false);
    run("a file the code writes in /work works, / is not writable by it", "python", "open('/work/a.txt','w').write('x'); print('work ok')\ntry:\n    open('/etc/x','w')\nexcept Exception as e:\n    print('etc blocked:', type(e).__name__)", 60, false);
    run("network OFF: a connection must fail", "python", "import socket\ntry:\n    socket.create_connection(('1.1.1.1', 53), 5)\n    print('REACHED THE NETWORK')\nexcept Exception as e:\n    print('no network:', type(e).__name__)", 60, false);
    run("network ON: a connection should work", "python", "import socket\ntry:\n    socket.create_connection(('1.1.1.1', 53), 5)\n    print('reached the network')\nexcept Exception as e:\n    print('no network:', type(e).__name__)", 60, true);
    run("memory limit (512Mi): allocating 1 GB must be killed", "python", "b = bytearray(1024*1024*1024)\nprint('ALLOCATED 1 GB')", 60, false);
    run("time limit: an endless loop must be stopped", "python", "while True: pass", 8, false);
    run("node", "node", "console.log('node', process.version)", 120, false);
    run("shell", "shell", "echo hi from sh; id -u", 60, false);
    run("a failing program reports its error", "python", "raise SystemExit('boom')", 30, false);
    println!("== kubectl through the cluster");
    let pods = iyakki_agent::exec::run_with_env(&[iyakki_agent::exec::Extra { name: "k3s".into(), host: None }], "k3s", &["kubectl".into(), "get".into(), "pods".into(), "-A".into()], std::path::Path::new("/tmp"), None, 30, Some(&s.clis.bin_dir()), &[("KUBECONFIG".into(), s.cluster.kubeconfig().display().to_string())]);
    println!("{}", pods.unwrap_or_else(|e| e));
    println!("== stopping");
    println!("   {:?}", s.cluster.stop());
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ollama") => ollama(args.get(1).map_or("http://localhost:11434", String::as_str)),
        Some("clis") => clis(&args[1..]),
        Some("cluster") => cluster(),
        Some("ollama-server") => ollama_server(),
        _ => println!("usage: verify_real ollama [url] | clis [id ...] | cluster | ollama-server"),
    }
}
