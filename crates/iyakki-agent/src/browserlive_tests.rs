// #[cfg(test)] -- this whole file is test code (it is included under #[cfg(test)] from browsertools.rs); the marker keeps the BOUNDARY check from counting it.
//! Live tests (ignored by default): the real WebKitGTK helper of `dist/webview/host-helper.py` drives real pages.
//! Needs python3-gi, gir1.2-webkit2-4.1 and (for a hidden window) Xvfb. Run with
//! `cargo test -p iyakki-agent --lib browserlive -- --ignored --nocapture --test-threads=1`.

use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use iyakki_sec::policy::{BrowserAgent, Role};

use super::*;
use crate::browserjobs::{HeadlessOptions, HeadlessSession};

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

struct Server(Child);
impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn serve_fixture() -> Server {
    let dir = repo().join("dist/samples/browser-test-fixture");
    let child = Command::new("python3").args(["-m", "http.server", "8765", "--bind", "127.0.0.1", "--directory"]).arg(&dir).stdout(Stdio::null()).stderr(Stdio::null()).spawn().expect("python3 http.server");
    std::thread::sleep(Duration::from_millis(700));
    Server(child)
}

fn svc(tag: &str) -> Arc<Services> {
    let d = std::env::temp_dir().join(format!("iyakki-live-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    struct Dead;
    impl iyakki_proxy::Upstream for Dead {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }
    Services::open(&d, Arc::new(Dead), Arc::new(Dead)).unwrap()
}

fn start(svc: &Arc<Services>, hosts: Vec<String>) -> HeadlessSession {
    crate::webengine::set_host_mode(true);
    let helper = crate::webengine::embedded_helper().expect("dist/webview/host-helper.py");
    assume_engine(true);
    HeadlessSession::start(svc.browser.jobs.clone(), "live", HeadlessOptions { helper, profile: None, env: None, size: (1100, 800), allow_hosts: hosts, user_agent: String::new() }).expect("the helper starts")
}

#[test]
#[ignore = "needs WebKitGTK (python3-gi, gir1.2-webkit2-4.1) and python3"]
fn live_the_runner_passes_the_sample_spec_against_a_local_server_with_real_clicks_and_typing() {
    let _server = serve_fixture();
    let svc = svc("spec");
    let spec = crate::browserrun::parse_spec(&std::fs::read_to_string(repo().join("dist/samples/browser-test-login-form.json")).unwrap()).unwrap();
    let _s = start(&svc, spec.allow_domains.clone());
    let mut policy = Role::Admin.template();
    policy.browser_agent = BrowserAgent::Interactive;
    let run = svc.browser.begin_run("live", 500, 60);
    svc.browser.preapprove("live", run, &spec.allow_domains);
    let out = std::env::temp_dir().join(format!("iyakki-live-out-{}", std::process::id()));
    let mut a = Actor::new(&svc, &policy, "live", run, "", &out);
    a.who = Who::Spec;
    let rep = crate::browserrun::run_spec(&a, &spec, &out);
    println!("{}", rep.text());
    assert!(rep.passed(), "{}", rep.text());
    // a failing expectation gives a report and a real screenshot
    let bad = crate::browserrun::parse_spec(r#"{"name":"fails","allow_domains":["127.0.0.1"],"base_url":"http://127.0.0.1:8765","timeout_ms":1500,"steps":[{"goto":"/form.html"},{"expect":{"text":"This text is not on the page"}}]}"#).unwrap();
    let rep = crate::browserrun::run_spec(&a, &bad, &out);
    println!("{}", rep.text());
    assert!(!rep.passed());
    let shot = rep.steps[1].screenshot.clone().expect("a screenshot");
    assert!(std::fs::metadata(shot).unwrap().len() > 2000);
}

#[test]
#[ignore = "needs WebKitGTK and the internet"]
fn live_gemini_read_only_shows_what_a_signed_out_visitor_sees() {
    let svc = svc("gemini");
    let _s = start(&svc, vec!["*".into()]);
    let mut policy = Role::User.template();
    policy.browser_agent = BrowserAgent::ReadOnly;
    policy.net_hosts = vec!["*".into()];
    let run = svc.browser.begin_run("live", 60, 20);
    svc.browser.allow_look("live", "gemini.google.com");
    svc.browser.allow_look("live", "accounts.google.com");
    let a = Actor::new(&svc, &policy, "live", run, "", &std::env::temp_dir());
    let r = a.open("https://gemini.google.com/app");
    match r {
        Ok(text) => {
            println!("----- first snapshot lines -----");
            for l in text.lines().take(40) {
                println!("{l}");
            }
        }
        Err(e) => println!("open failed: {e}"),
    }
    // clicking is refused for a read-only account
    let e = a.click("e1");
    println!("click: {e:?}");
}

fn count_procs(name: &str) -> usize {
    std::fs::read_dir("/proc").map(|d| d.flatten().filter(|e| std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim() == name)).count()).unwrap_or(0)
}

fn my_shm_files() -> usize {
    let me = format!("iyakki-webview-{}-", std::process::id());
    std::fs::read_dir("/dev/shm").map(|d| d.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&me)).count()).unwrap_or(0)
}

#[test]
#[ignore = "needs WebKitGTK; 30 helper start-ups take a few minutes"]
fn live_thirty_cycles_of_open_snapshot_screenshot_close_leave_nothing_behind() {
    let _server = serve_fixture();
    let xvfb_before = count_procs("Xvfb");
    let mut failures: Vec<String> = Vec::new();
    for cycle in 0..30 {
        let svc = svc(&format!("cycle{cycle}"));
        let s = start(&svc, vec!["127.0.0.1".into()]);
        let mut policy = Role::User.template();
        policy.browser_agent = BrowserAgent::Interactive;
        policy.fs_write = vec![std::env::temp_dir().display().to_string()];
        let run = svc.browser.begin_run("live", 60, 20);
        svc.browser.allow_look("live", "127.0.0.1");
        let a = Actor::new(&svc, &policy, "live", run, "", &std::env::temp_dir());
        let step = |what: &str, r: Result<String, String>, failures: &mut Vec<String>| {
            if let Err(e) = r {
                failures.push(format!("cycle {cycle}: {what}: {e}"));
            }
        };
        let page = if cycle % 2 == 0 { "form.html" } else { "chat.html" };
        step("open", a.open(&format!("http://127.0.0.1:8765/{page}")), &mut failures);
        step("snapshot", a.snapshot(), &mut failures);
        step("screenshot", a.screenshot(), &mut failures);
        if cycle % 5 == 0 {
            // a quick interaction too
            let _ = svc.browser.mode("live", "127.0.0.1", run);
        }
        step("close", a.close(), &mut failures);
        drop(s);
        println!("cycle {cycle} done");
    }
    std::thread::sleep(Duration::from_secs(3));
    let xvfb_after = count_procs("Xvfb");
    println!("Xvfb before {xvfb_before}, after {xvfb_after}; shm files left {}", my_shm_files());
    assert!(failures.is_empty(), "{failures:#?}");
    assert_eq!(my_shm_files(), 0, "shared files left behind");
    assert!(xvfb_after <= xvfb_before, "Xvfb processes left behind: before {xvfb_before}, after {xvfb_after}");
}

#[test]
#[ignore = "needs WebKitGTK; kills the helper on purpose; run alone (it also kills any other host-helper.py of this user)"]
fn live_a_killed_helper_gives_the_tool_an_error_and_takes_its_xvfb_with_it() {
    let _server = serve_fixture();
    let xvfb_before = count_procs("Xvfb");
    let svc = svc("killed");
    let s = start(&svc, vec!["127.0.0.1".into()]);
    let mut policy = Role::User.template();
    policy.browser_agent = BrowserAgent::Interactive;
    policy.fs_write = vec![std::env::temp_dir().display().to_string()];
    let run = svc.browser.begin_run("live", 60, 20);
    svc.browser.allow_look("live", "127.0.0.1");
    let a = Actor::new(&svc, &policy, "live", run, "", &std::env::temp_dir());
    a.open("http://127.0.0.1:8765/form.html").unwrap();
    assert!(count_procs("Xvfb") > xvfb_before, "the helper started its own Xvfb");
    let _ = Command::new("pkill").args(["-9", "-f", "host-helper.py"]).status();
    let t = std::time::Instant::now();
    let e = a.snapshot().unwrap_err();
    println!("after the kill: {e} (in {:?})", t.elapsed());
    assert!(t.elapsed() < Duration::from_secs(30));
    std::thread::sleep(Duration::from_secs(3));
    assert_eq!(count_procs("Xvfb"), xvfb_before, "Xvfb left behind");
    drop(s);
}
