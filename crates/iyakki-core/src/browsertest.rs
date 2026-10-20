//! `iyakki browser-test SPEC.json`: the deterministic browser test runner from the command line (docs/BROWSER-AGENT.md).
//!
//! It starts the same embedded web engine the BROWSER app uses (WebKitGTK helper on a desktop, WPE on the image, or whatever
//! `IYAKKI_WEBVIEW_HELPER` names) headless in this process, and runs the spec's steps through the same code as the assistant's
//! tools. Running a spec yourself is your approval for the domains in its `allow_domains` (and only those): a step that leaves
//! them stops the run. Exit status: 0 all steps passed, 1 a step failed, 2 the spec or the engine could not be used.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use iyakki_agent::browserjobs::{HeadlessOptions, HeadlessSession};
use iyakki_agent::browserrun::{parse_spec, run_spec};
use iyakki_agent::browsertools::{assume_engine, Actor, Who};
use iyakki_sec::policy::{BrowserAgent, Role};

const USAGE: &str = "usage: iyakki browser-test SPEC.json [--out DIR] [--json] [--profile DIR] [--size WxH]";

fn say(s: &str) {
    crate::console::out_write(&format!("{s}\n"));
}

fn fail(s: &str, code: u8) -> ExitCode {
    crate::console::err_write(&format!("iyakki: {s}\n"));
    ExitCode::from(code)
}

pub fn run(args: &[String]) -> ExitCode {
    let mut spec_path = None;
    let mut out = None;
    let mut profile = None;
    let mut json = false;
    let mut size = (1280u32, 800u32);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = it.next().map(PathBuf::from),
            "--profile" => profile = it.next().map(PathBuf::from),
            "--json" => json = true,
            "--size" => {
                if let Some((w, h)) = it.next().and_then(|v| v.split_once('x')) {
                    size = (w.parse().unwrap_or(1280), h.parse().unwrap_or(800));
                }
            }
            s if !s.starts_with('-') && spec_path.is_none() => spec_path = Some(PathBuf::from(s)),
            _ => return fail(USAGE, 2),
        }
    }
    let Some(spec_path) = spec_path else { return fail(USAGE, 2) };
    let text = match std::fs::read_to_string(&spec_path) {
        Ok(t) => t,
        Err(e) => return fail(&format!("{}: {e}", spec_path.display()), 2),
    };
    let spec = match parse_spec(&text) {
        Ok(s) => s,
        Err(e) => return fail(&format!("{}: {e}", spec_path.display()), 2),
    };
    // a desktop session may use the WebKitGTK helper of this checkout
    iyakki_agent::webengine::set_host_mode(true);
    let Some(helper) = iyakki_agent::webengine::embedded_helper() else {
        return fail("no web engine helper found: set IYAKKI_WEBVIEW_HELPER, run from a checkout with python3-gi and gir1.2-webkit2-4.1, or use the web image (docs/FULL-BROWSER.md)", 2);
    };
    let out_dir = out.unwrap_or_else(|| std::env::temp_dir().join(format!("iyakki-browser-test-{}", std::process::id())));
    let work = std::env::temp_dir().join(format!("iyakki-browser-test-svc-{}", std::process::id()));
    let _ = std::fs::create_dir_all(&work);
    let quick: Arc<dyn iyakki_agent::iyakki_proxy::Upstream> = Arc::new(iyakki_agent::iyakki_proxy::https::UreqUpstream::with_timeout(std::time::Duration::from_secs(10)));
    let svc = match iyakki_agent::services::Services::open(&work, quick.clone(), quick) {
        Ok(s) => s,
        Err(e) => return fail(&format!("{e}"), 2),
    };
    assume_engine(true);
    let user = "browser-test";
    let opts = HeadlessOptions { helper, profile: profile.clone(), env: None, size, allow_hosts: spec.allow_domains.clone(), user_agent: iyakki_agent::webengine::user_agent_suffix() };
    let session = match HeadlessSession::start(svc.browser.jobs.clone(), user, opts) {
        Ok(s) => s,
        Err(e) => return fail(&format!("the web engine could not be started: {e}"), 2),
    };
    let mut policy = Role::Admin.template();
    policy.browser_agent = BrowserAgent::Interactive;
    policy.browser_max_actions = 500;
    policy.browser_max_minutes = 120;
    let run = svc.browser.begin_run(user, 500, 120);
    // the person ran the spec: its domains are approved for the run, nothing else is
    svc.browser.preapprove(user, run, &spec.allow_domains);
    let mut actor = Actor::new(&svc, &policy, user, run, "", &out_dir);
    actor.who = Who::Spec;
    actor.allow_payment = spec.allow_payment;
    actor.allow_destructive = spec.allow_destructive;
    let report = run_spec(&actor, &spec, &out_dir);
    drop(session);
    let _ = std::fs::remove_dir_all(&work);
    if json {
        say(&serde_json_pretty(&report.json()));
    } else {
        say(report.text().trim_end());
        if report.steps.iter().any(|s| s.screenshot.is_some()) {
            say(&format!("Screenshots are in {}", out_dir.display()));
        }
    }
    if report.passed() { ExitCode::SUCCESS } else { ExitCode::from(1) }
}

fn serde_json_pretty(v: &serde_json::Value) -> String {
    serde_json::to_string_pretty(v).unwrap_or_default()
}
