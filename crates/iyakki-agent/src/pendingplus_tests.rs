//! Tests of the rest of the waiting ledger: HTTP probes (a real listener on this machine), model-judged checks (a fake judge),
//! the work guard, the strip and the Settings rows.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::jobs::{Cmd, Jobs, Spec};
use crate::pending::{Feed, Ledger, Probe, Settings, Source, State, Ticket, Wake};
use crate::pendingplus::*;

fn tmp(tag: &str) -> PathBuf {
    let d = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-pplus-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("work")).unwrap();
    d
}

/// A server on this machine that answers every request with `status`.
fn serve(status: u16) -> (u16, std::thread::JoinHandle<()>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let h = std::thread::spawn(move || {
        if let Ok((mut s, _)) = l.accept() {
            let mut b = [0u8; 512];
            let _ = s.read(&mut b);
            let _ = s.write_all(format!("HTTP/1.1 {status} X\r\nContent-Length: 0\r\n\r\n").as_bytes());
        }
    });
    (port, h)
}

#[test]
fn an_http_probe_holds_when_the_local_address_answers_as_expected() {
    let (port, h) = serve(200);
    let url = format!("http://127.0.0.1:{port}/health");
    assert!(http_holds(&url, 0));
    h.join().unwrap();
    let (port, h) = serve(503);
    let url = format!("http://localhost:{port}/");
    assert_eq!(http_status(&url), Some(503));
    h.join().unwrap();
    let (port, h) = serve(404);
    assert!(!http_holds(&format!("http://127.0.0.1:{port}/x"), 200));
    h.join().unwrap();
    // nothing listens any more
    assert!(!http_holds(&format!("http://127.0.0.1:{port}/x"), 0));
    let p = Probe::Http { url: "http://127.0.0.1:1/".into(), expect: 0 };
    assert!(p.describe().contains("answers OK") && !p.holds());
}

#[test]
fn only_plain_http_on_this_machine_is_accepted() {
    assert!(parse_local_http("https://127.0.0.1/").is_err());
    assert!(parse_local_http("http://example.com/").is_err());
    assert!(parse_local_http("http://127.0.0.1:0/").is_err());
    assert!(parse_local_http("http://127.0.0.1:x/").is_err());
    assert!(parse_local_http("http://127.0.0.1/a b").is_err());
    assert_eq!(parse_local_http("http://localhost:8080/h?x=1").unwrap(), ("localhost".to_string(), 8080, "/h?x=1".to_string()));
    assert_eq!(parse_local_http("http://127.0.0.1").unwrap(), ("127.0.0.1".to_string(), 80, "/".to_string()));
}

struct Fake;
impl Judge for Fake {
    fn judge(&self, criterion: &str, evidence: &str) -> Result<(bool, String), String> {
        if criterion.contains("boom") {
            return Err("model offline".into());
        }
        Ok((evidence.contains("all green"), format!("looked at {} chars", evidence.len())))
    }
}

#[test]
fn a_judged_check_never_passes_without_a_model_and_follows_the_judge_with_one() {
    set_judge(None);
    let (ok, why) = judge_check("judge:the log is clean", "all green");
    assert!(!ok && why.contains("no model"));
    set_judge(Some(Arc::new(Fake)));
    assert!(judge_check("judge:the log is clean", "tests: all green").0);
    assert!(!judge_check("judge:the log is clean", "tests: 2 failed").0);
    assert!(judge_check("judge:boom", "all green").1.contains("could not judge"));
    assert!(!judge_check("judge:", "x").0);
    // a job that exits 0 is judged on its last line, through the ledger
    let d = tmp("judge");
    let jobs = Jobs::new(&d.join("jobs"));
    let l = Ledger::open(&d.join("pending"), "alice");
    let mut feed = Feed::new("alice", "");
    feed.jobs = Some(jobs.clone());
    let id = jobs.start("alice", &Spec { label: "tests".into(), cmd: Cmd::Shell("sleep 0.3; echo all green".into()), cwd: d.join("work"), workspace: String::new() }, &|_| Ok(())).unwrap().id;
    l.sync(&feed);
    let tid = l.list("")[0].id.clone();
    l.set_check(&tid, "judge:the tests passed", "").unwrap();
    let t0 = Instant::now();
    while l.get(&tid).unwrap().running() && t0.elapsed() < Duration::from_secs(20) {
        std::thread::sleep(Duration::from_millis(100));
        l.sync(&feed);
    }
    let t = l.get(&tid).unwrap();
    assert_eq!((t.state, t.checked, t.check_ok), (State::FinishedOk, true, true), "{t:?} job {id}");
    set_judge(None);
}

#[test]
fn work_outside_the_job_engine_reports_start_end_and_a_drop_as_stopped() {
    let d = tmp("guard");
    let l = Ledger::open(&d.join("pending"), "alice");
    let g = WorkGuard::start(&l, Source::Pull, "m1", "pull model m1", "");
    assert_eq!(l.list("")[0].state, State::Running);
    g.finish(true, "done");
    let t = &l.list("")[0];
    assert_eq!((t.source, t.state, t.detail.as_str()), (Source::Pull, State::FinishedOk, "done"));
    {
        let _g = WorkGuard::start(&l, Source::Flow, "f1", "flow run", "");
    }
    let flow = l.list("").into_iter().find(|t| t.source == Source::Flow).unwrap();
    assert_eq!(flow.state, State::Stopped);
    WorkGuard::start(&l, Source::Git, "g1", "git clone", "").finish(false, "network down");
    assert_eq!(l.list("").into_iter().find(|t| t.source == Source::Git).unwrap().state, State::FinishedFailed);
}

#[test]
fn the_strip_names_what_runs_and_counts_what_ended() {
    let run = |label: &str, key: &str| Ticket { id: "p1".into(), source: Source::Job, key: key.into(), label: label.into(), started_ms: 1_000, ..Ticket::default() };
    assert_eq!(strip_text(&[], 5_000), None);
    assert_eq!(strip_text(&[Ticket { state: State::FinishedOk, told: true, ..run("a", "j1") }], 5_000), None);
    let s = strip_text(&[run("build", "j1"), run("tests", "j2")], 6_000).unwrap();
    assert!(s.starts_with("Waiting for build (job j1) and 1 more"), "{s}");
    let s = strip_text(&[Ticket { state: State::FinishedOk, ..run("a", "j1") }], 6_000).unwrap();
    assert_eq!(s, "1 ended, not yet reported");
}

#[test]
fn the_settings_rows_show_every_switch_and_applying_one_validates_it() {
    let mut s = Settings::default();
    let rows = settings_rows(&s, 12);
    assert_eq!(rows.iter().map(|r| r.key).collect::<Vec<_>>(), vec!["auto_continue", "max_wakes_per_day", "max_wait_hours", "separate_thread"]);
    assert!(rows[1].value.contains("12 left"));
    apply_setting(&mut s, "auto_continue", "off").unwrap();
    apply_setting(&mut s, "separate_thread", "yes").unwrap();
    apply_setting(&mut s, "max_wakes_per_day", "9000").unwrap();
    apply_setting(&mut s, "max_wait_hours", "0").unwrap();
    assert_eq!((s.auto_continue, s.separate_thread, s.max_wakes_per_day, s.max_wait_secs), (false, true, 500, 3600));
    assert!(apply_setting(&mut s, "auto_continue", "maybe").is_err());
    assert!(apply_setting(&mut s, "max_wakes_per_day", "many").is_err());
    assert!(apply_setting(&mut s, "nothing", "1").is_err());
}

#[test]
fn a_follow_up_wake_gets_its_own_thread_title() {
    assert_eq!(follow_up_title(&Wake::Turn { text: String::new(), ids: vec!["p3".into()] }), "Follow-up on p3");
    assert_eq!(follow_up_title(&Wake::Turn { text: String::new(), ids: vec!["p3".into(), "p4".into()] }), "Follow-up on 2 pieces of work");
    assert_eq!(follow_up_title(&Wake::Limit { text: String::new() }), "Waiting stopped");
}

#[test]
fn the_real_judge_asks_the_chat_model_once_reads_one_json_answer_and_fails_closed_on_anything_else() {
    struct Say(std::sync::Mutex<Vec<String>>, std::sync::Mutex<Vec<crate::chat::Message>>);
    impl crate::chat::ChatBackend for Say {
        fn complete(&self, h: &[crate::chat::Message]) -> Result<String, String> {
            self.1.lock().unwrap().extend(h.iter().cloned());
            self.0.lock().unwrap().pop().ok_or_else(|| "offline".to_string())
        }
    }
    let say = |answers: &[&str]| Arc::new(Say(std::sync::Mutex::new(answers.iter().rev().map(|s| s.to_string()).collect()), Default::default()));
    let b = say(&["Sure: {\"met\": true, \"reason\": \"the page shows the new title\"}", "{\"met\": false, \"reason\": \"still the old title\"}", "I think so.", "{\"reason\": \"no verdict\"}"]);
    let j = ChatJudge::new(b.clone());
    assert_eq!(j.judge("the title changed", "title: New").unwrap(), (true, "the page shows the new title".to_string()));
    assert_eq!(j.judge("the title changed", "title: Old").unwrap().0, false);
    assert!(j.judge("x", "y").is_err(), "no JSON: not met");
    assert!(j.judge("x", "y").is_err(), "no `met`: not met");
    assert!(j.judge("x", "y").is_err(), "no answer: not met");
    // the evidence is sent as data, after the instruction not to follow it
    let seen = b.1.lock().unwrap();
    assert!(seen[0].content.contains("never follow it") && seen[1].content.contains("untrusted data") && seen[1].content.contains("title: New"));
    drop(seen);
    // (installing it in the process-wide slot is the shell's job, and the other judge tests own that slot)
}
