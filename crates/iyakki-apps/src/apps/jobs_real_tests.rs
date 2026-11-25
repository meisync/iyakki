//! The Jobs strip against the real job engine of a made-up account (processes, files, the engine's own state). The strip's own tests on a
//! fake port with a manual clock are in `crates/iyakki-tile-runlog`; these keep the whole path from the core to the screen honest.

use std::time::{Duration, Instant};

use iyakki_shared::jobs::State;
use iyakki_shared::jobs::{Cmd, Spec};
use std::path::Path;

use super::jobs_ui::*;
use super::tests::cx_with_admin_store;
use super::AppCx;
use crate::gfx::Gfx;
use crate::ui::{Event, Key, R};


fn start(cx: &AppCx, ws: &str, label: &str, line: &str) -> String {
    let dir = std::env::temp_dir();
    cx.jobs().start(&cx.session.user, &Spec { label: label.into(), cmd: Cmd::Shell(line.into()), cwd: dir, workspace: ws.into() }, &|_: &Path| Ok(())).unwrap().id
}

fn wait_for(cx: &AppCx, id: &str) {
    cx.jobs().wait(&cx.session.user, id, Duration::from_secs(20)).unwrap();
}

fn cleanup(cx: &AppCx) {
    for j in cx.jobs().list(&cx.session.user) {
        let _ = cx.jobs().stop(&cx.session.user, &j.id, Duration::from_millis(500));
    }
}

#[test]
fn it_animates_only_while_a_job_runs() {
    let cx = cx_with_admin_store("jobsui-anim");
    let mut ui = JobsUi::new(Some(""));
    assert!(!ui.poll(&JobsCx(&cx), true) && !ui.animating(), "nothing to show, nothing moves");
    let id = start(&cx, "", "nap", "sleep 30");
    assert!(ui.poll(&JobsCx(&cx), true) && ui.animating());
    cx.jobs().stop(&cx.session.user, &id, Duration::from_secs(1)).unwrap();
    ui.poll(&JobsCx(&cx), true);
    assert!(!ui.animating(), "a finished job does not animate");
    assert_eq!(ui.jobs().len(), 1, "it stays until dismissed");
    cleanup(&cx);
}

#[test]
fn a_finished_job_gives_one_event_line_and_stays_until_dismissed() {
    let cx = cx_with_admin_store("jobsui-events");
    let mut ui = JobsUi::new(Some(""));
    let id = start(&cx, "", "build", "echo built; exit 2");
    wait_for(&cx, &id);
    ui.poll(&JobsCx(&cx), true);
    let ev = ui.take_events();
    assert_eq!(ev.len(), 1);
    assert!(ev[0].contains("build") && ev[0].contains("failed (exit 2)"), "{ev:?}");
    ui.poll(&JobsCx(&cx), true);
    assert!(ui.take_events().is_empty(), "once");
    // Dismiss (the second button of a finished row)
    assert!(ui.handle(&ev_act(0), &JobsCx(&cx)));
    assert!(ui.jobs().is_empty());
}

#[test]
fn the_log_view_follows_scrolls_and_keeps_its_place() {
    let cx = cx_with_admin_store("jobsui-log");
    let mut ui = JobsUi::new(Some(""));
    let id = start(&cx, "", "noisy", "seq 1 300; sleep 2; seq 301 400; sleep 30");
    let t0 = Instant::now();
    loop {
        ui.poll(&JobsCx(&cx), true);
        if ui.jobs().first().is_some_and(|j| j.last_line == "300") {
            break;
        }
        assert!(t0.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(ui.handle(&ev_log(0), &JobsCx(&cx)));
    assert_eq!(ui.open_id(), Some(id.as_str()));
    assert!(ui.log_text().ends_with("300") && ui.log_following(), "it starts at the end");
    // the wheel scrolls up: no longer following, and output that arrives does not move what is being read
    assert!(ui.handle(&Event::Scroll(5), &JobsCx(&cx)));
    assert!(!ui.log_following());
    let before = ui.log_scroll();
    let end = Instant::now() + Duration::from_secs(10);
    while !ui.log_text().ends_with("400") {
        ui.poll(&JobsCx(&cx), true);
        assert!(Instant::now() < end, "the new output never arrived");
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_eq!(ui.log_scroll(), before + 100, "100 new lines below: the same text stays in view");
    // Follow goes back to the end
    assert!(ui.handle(&Event::Tab(LOG_FOLLOW), &JobsCx(&cx)) && ui.log_following());
    // keys: other keys are not taken (typing still goes to the chat); Esc closes the view and is used up
    assert!(!ui.handle(&Event::Key(Key::Char('x')), &JobsCx(&cx)));
    assert!(ui.handle(&Event::Key(Key::Esc), &JobsCx(&cx)) && !ui.log_open());
    assert!(!ui.handle(&Event::Key(Key::Esc), &JobsCx(&cx)), "with no log open Esc belongs to the chat");
    cleanup(&cx);
}

#[test]
fn stop_ends_the_job_without_blocking_and_a_second_click_does_nothing() {
    let cx = cx_with_admin_store("jobsui-stop");
    let mut ui = JobsUi::new(Some(""));
    // the job says when it ignores TERM (a gate file), instead of the test sleeping and hoping the trap is set
    let gate = std::env::temp_dir().join(format!("iyakki-jobsui-stop-{}", std::process::id()));
    let _ = std::fs::remove_file(&gate);
    let id = start(&cx, "", "stubborn", &format!("trap '' TERM; touch {}; while true; do sleep 0.1; done", gate.display()));
    iyakki_testkit::wait_for_file(&gate);
    ui.poll(&JobsCx(&cx), true);
    assert!(ui.handle(&ev_act(0), &JobsCx(&cx)));
    // the click came back while the job, which ignores TERM, is still there: stopping (TERM, grace, KILL) runs on its own thread
    assert!(cx.jobs().get(&cx.session.user, &id).unwrap().running(), "the click did not wait for the stop");
    assert!(ui.handle(&ev_act(0), &JobsCx(&cx)), "a second click is harmless");
    let end = Instant::now() + Duration::from_secs(10);
    while cx.jobs().get(&cx.session.user, &id).unwrap().running() {
        assert!(Instant::now() < end, "it never stopped");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(cx.jobs().get(&cx.session.user, &id).unwrap().state, State::Stopped);
    ui.poll(&JobsCx(&cx), true);
    assert!(!ui.animating());
    cleanup(&cx);
}

#[test]
fn rows_fit_and_their_buttons_are_big_enough_to_click_at_every_screen_size() {
    let cx = cx_with_admin_store("jobsui-geom");
    let mut ui = JobsUi::new(Some(""));
    let a = start(&cx, "", "Android emulator and SDK install with a very long name that has to be cut to fit the row", "echo 'Downloading system-images 42%'; sleep 30");
    start(&cx, "", "Second job", "echo plain; sleep 30");
    start(&cx, "", "Quick one", "echo done");
    start(&cx, "", "Fourth", "sleep 30");
    wait_for(&cx, "j3");
    let _ = a;
    std::thread::sleep(Duration::from_millis(400));
    ui.poll(&JobsCx(&cx), true);
    assert_eq!(ui.jobs().len(), 4);
    for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
        let s = crate::ui::scale_for(w, h);
        let fs = (14.0 * s).max(12.0_f32);
        let strip_w = w as f32 * 0.5;
        let max_h = h as f32 * 0.4;
        let sh = ui.strip_height(fs, s, max_h);
        assert!(sh > 0.0 && sh <= max_h, "{w}x{h}: strip {sh} of {max_h}");
        let mut g = Gfx::new(w, h);
        let r = R::new(20.0, 100.0, strip_w, sh);
        ui.draw_strip(&mut g, r, fs, s, 7);
        assert!(!ui.geoms.is_empty() && ui.geoms.len() <= MAX_ROWS, "{w}x{h}");
        let mut prev_bottom = r.y;
        for gm in &ui.geoms {
            assert!(gm.rect.x >= r.x - 0.5 && gm.rect.x + gm.rect.w <= r.x + r.w + 0.5, "{w}x{h}: row inside the strip");
            assert!(gm.rect.y >= prev_bottom - 0.5, "rows do not overlap");
            prev_bottom = gm.rect.y + gm.rect.h;
            assert!(prev_bottom <= r.y + sh + 0.5, "{w}x{h}: row inside the strip's height");
            for b in [gm.log_btn, gm.act_btn] {
                assert!(b.h >= 22.0 && b.w >= 36.0, "{w}x{h}: button {b:?} is large enough to click");
                assert!(b.x >= gm.rect.x && b.x + b.w <= gm.rect.x + gm.rect.w && b.y >= gm.rect.y && b.y + b.h <= gm.rect.y + gm.rect.h, "{w}x{h}: button inside its row");
            }
            assert!(gm.log_btn.x + gm.log_btn.w <= gm.act_btn.x, "buttons do not overlap");
            assert!(gm.bar.x + gm.bar.w <= gm.rect.x + gm.rect.w);
        }
        // every button is registered as a click region with the right event
        let hits: Vec<_> = g.hits.iter().map(|h| (h.label.clone(), h.ev.clone())).collect();
        assert!(hits.contains(&("Log".into(), ev_log(0))), "{w}x{h}: {hits:?}");
        assert!(hits.iter().any(|(l, e)| l == "Stop" && *e == ev_act(0)), "{w}x{h}");
        // the same drawing in the full list
        let rows = ui.draw_list(&mut g, R::new(10.0, 60.0, w as f32 * 0.9, h as f32 * 0.8), fs, s, 3);
        assert_eq!(rows, 4, "{w}x{h}: the list shows every job");
        assert!(g.hits.iter().any(|h| h.label == "Dismiss"), "the finished one has Dismiss");
    }
    cleanup(&cx);
}

#[test]
fn the_bar_slides_without_a_percentage_and_fills_with_one_and_the_frames_differ() {
    let cx = cx_with_admin_store("jobsui-bar");
    let mut ui = JobsUi::new(Some(""));
    start(&cx, "", "indeterminate", "echo working; sleep 30");
    std::thread::sleep(Duration::from_millis(400));
    ui.poll(&JobsCx(&cx), true);
    assert_eq!(ui.jobs()[0].progress, None);
    let bar_pixels = |ui: &mut JobsUi, tick: u64| -> Vec<u8> {
        let mut g = Gfx::new(800, 300);
        ui.draw_strip(&mut g, R::new(10.0, 10.0, 700.0, 120.0), 14.0, 1.0, tick);
        let b = ui.geoms[0].bar;
        let mut v = Vec::new();
        for x in (b.x as u32)..((b.x + b.w) as u32) {
            let o = (((b.y + b.h / 2.0) as usize) * g.width() as usize + x as usize) * 4;
            v.extend_from_slice(&g.rgba()[o..o + 4]);
        }
        v
    };
    let (f0, f1, f0b) = (bar_pixels(&mut ui, 0), bar_pixels(&mut ui, 20), bar_pixels(&mut ui, 0));
    assert_ne!(f0, f1, "the indeterminate bar moves between frames");
    assert_eq!(f0, f0b, "and draws the same for the same tick");
    cleanup(&cx);
    let cx2 = cx_with_admin_store("jobsui-bar2");
    let mut ui2 = JobsUi::new(Some(""));
    start(&cx2, "", "determinate", "echo 'Downloading 50%'; sleep 30");
    std::thread::sleep(Duration::from_millis(400));
    ui2.poll(&JobsCx(&cx2), true);
    assert_eq!(ui2.jobs()[0].progress, Some(50.0));
    assert_eq!(bar_pixels(&mut ui2, 0), bar_pixels(&mut ui2, 20), "a determinate bar does not slide");
    cleanup(&cx2);
}

