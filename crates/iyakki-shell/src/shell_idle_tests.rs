//! KI-092: an idle Home screen must not be drawn again and again. The loop below plays the backend's decision (a frame when the
//! shell asked for one, or when the safety-net interval ran out) at the 33 ms heartbeat and counts the frames by who asked.

use super::tests::*;
use super::*;

/// Plays `ticks` idle heartbeats. Returns the number of frames the backend would draw and the table of who asked for them.
fn idle_frames(sh: &mut Shell, ticks: u32) -> (u32, String) {
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    sh.take_stale();
    let _ = crate::stale_stats::take_table();
    let (mut frames, mut since_ms) = (0u32, 0u64);
    for _ in 0..ticks {
        sh.handle(&Event::Tick);
        since_ms += 33;
        let dirty = sh.take_stale();
        let timed = since_ms >= sh.redraw_interval_ms();
        if dirty || timed {
            if !dirty {
                crate::stale_stats::note("timed");
            }
            frames += 1;
            since_ms = 0;
            sh.render(&mut g);
        }
    }
    (frames, crate::stale_stats::take_table())
}

#[test]
fn an_idle_home_screen_is_drawn_about_once_a_second_not_ten_times() {
    let (mut sh, _) = shell_with_users("idle1");
    login(&mut sh, "alice", "alicepass1");
    assert_eq!(sh.screen_name(), "launcher");
    // 300 heartbeats are about 10 seconds
    let (frames, table) = idle_frames(&mut sh, 300);
    eprintln!("idle Home: {frames} frames in 300 heartbeats; by reason: {table}");
    assert!(frames <= 25, "an idle Home drew {frames} frames in 300 heartbeats; by reason: {table}");
}

#[test]
fn a_start_up_strip_that_is_still_busy_keeps_its_own_faster_frames() {
    let (mut sh, _) = shell_with_users("idle2");
    login(&mut sh, "alice", "alicepass1");
    sh.startup.phase = iyakki_agent::startup::Phase::Busy;
    assert!(!sh.home_is_calm());
    assert_eq!(sh.redraw_interval_ms(), 100, "a moving strip keeps the 100 ms safety net");
}
