//! Detached jobs at the shell level: real mouse clicks on the strip, the redraw rule (frames only while a job runs), and the
//! inbox note for a job that ends while the person is somewhere else.

use super::tests::*;
use super::*;
use iyakki_agent::jobs::{Cmd, Spec};
use std::time::{Duration, Instant};

fn start(sh: &Shell, user: &str, label: &str, line: &str) -> String {
    let dir = std::env::temp_dir();
    sh.services_for_demo().jobs.start(user, &Spec { label: label.into(), cmd: Cmd::Shell(line.into()), cwd: dir, workspace: String::new() }, &|_: &std::path::Path| Ok(())).unwrap().id
}

/// Heartbeats at about 30 a second; how many of them asked for a new frame.
fn frames(sh: &mut Shell, beats: usize) -> usize {
    let mut n = 0;
    for _ in 0..beats {
        sh.handle(&Event::Tick);
        std::thread::sleep(Duration::from_millis(33));
        if sh.take_stale() {
            n += 1;
        }
    }
    n
}

fn stop_all(sh: &Shell, user: &str) {
    let jobs = &sh.services_for_demo().jobs;
    for j in jobs.list(user) {
        let _ = jobs.stop(user, &j.id, Duration::from_millis(300));
    }
}

#[test]
fn the_shell_draws_extra_frames_only_while_a_job_runs_in_the_open_chat() {
    let (mut sh, _) = shell_with_users("jobsredraw");
    login(&mut sh, "root", "rootpass12");
    assert!(sh.open_for_demo("chat"), "{}", sh.screen_name());
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    frames(&mut sh, 10);
    let idle = frames(&mut sh, 90);
    assert!(idle <= 6, "an idle chat with no job asks for next to nothing: {idle} frames in 3 s");
    let id = start(&sh, "root", "nap", "sleep 120");
    // the chat notices a job started by someone else within a few seconds
    let t0 = Instant::now();
    while !has(&mut sh, &mut g, "Stop") {
        frames(&mut sh, 3);
        assert!(t0.elapsed() < Duration::from_secs(10), "the strip never showed the job");
    }
    let running = frames(&mut sh, 90);
    assert!(running >= idle + 20, "about ten frames a second while a job runs: {running} (idle {idle})");
    sh.render(&mut g);
    assert!(sh.hits().iter().any(|h| h.label == "Stop"), "the strip is on screen");
    sh.services_for_demo().jobs.stop("root", &id, Duration::from_secs(1)).unwrap();
    frames(&mut sh, 60);
    let after = frames(&mut sh, 90);
    assert!(after <= idle + 2, "back to idle once the job ended: {after} (idle {idle})");
    stop_all(&sh, "root");
}

#[test]
fn clicking_stop_and_log_on_the_strip_with_the_mouse_works_through_the_shell() {
    let (mut sh, _) = shell_with_users("jobsclick");
    login(&mut sh, "root", "rootpass12");
    assert!(sh.open_for_demo("chat"));
    let id = start(&sh, "root", "downloader", "echo 'Downloading 20%'; sleep 120");
    let mut g = Gfx::new(1280, 720);
    let t0 = Instant::now();
    while !has(&mut sh, &mut g, "Stop") {
        sh.handle(&Event::Tick);
        assert!(t0.elapsed() < Duration::from_secs(10), "no strip");
        std::thread::sleep(Duration::from_millis(50));
    }
    click(&mut sh, &mut g, "Log");
    assert!(has(&mut sh, &mut g, "Close"), "the log view opened");
    click(&mut sh, &mut g, "Close");
    assert!(!has(&mut sh, &mut g, "Close"));
    click(&mut sh, &mut g, "Stop");
    let jobs = sh.services_for_demo().jobs.clone();
    let end = Instant::now() + Duration::from_secs(10);
    while jobs.get("root", &id).unwrap().running() {
        assert!(Instant::now() < end, "the click did not stop the job");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(jobs.get("root", &id).unwrap().state, iyakki_agent::jobs::State::Stopped);
    assert!(sh.services_for_demo().jobs.list("root").len() == 1);
    stop_all(&sh, "root");
}

#[test]
fn a_job_that_ends_while_the_person_is_elsewhere_leaves_one_inbox_note_that_opens_the_jobs_list() {
    let (mut sh, _) = shell_with_users("jobsinbox");
    login(&mut sh, "root", "rootpass12");
    let id = start(&sh, "root", "emulator install", "echo installed");
    sh.services_for_demo().jobs.wait("root", &id, Duration::from_secs(20)).unwrap();
    for _ in 0..100 {
        sh.handle(&Event::Tick);
    }
    let notes: Vec<_> = sh.services_for_demo().inbox.list("root").into_iter().filter(|n| n.title.contains("emulator install")).collect();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert!(notes[0].title.contains("finished") && notes[0].body.contains("succeeded"), "{:?}", notes[0]);
    let link = notes[0].link.clone().expect("a link");
    assert_eq!((link.app.as_str(), link.id.as_str()), ("runs", "jobs"));
    for _ in 0..100 {
        sh.handle(&Event::Tick);
    }
    assert_eq!(sh.services_for_demo().inbox.list("root").iter().filter(|n| n.title.contains("emulator install")).count(), 1, "once");
    // a failed one is a warning
    let bad = start(&sh, "root", "broken build", "exit 4");
    sh.services_for_demo().jobs.wait("root", &bad, Duration::from_secs(20)).unwrap();
    for _ in 0..100 {
        sh.handle(&Event::Tick);
    }
    let n = sh.services_for_demo().inbox.list("root").into_iter().find(|n| n.title.contains("broken build")).unwrap();
    assert_eq!(n.kind, iyakki_agent::inbox::Kind::Warning);
    assert!(n.title.contains("did not finish") && n.body.contains("failed (exit 4)"));
    stop_all(&sh, "root");
}
