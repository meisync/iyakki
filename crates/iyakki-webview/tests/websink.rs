//! The web engine's sound output (dist/webview/wpe-helper/websink.c): runs its C self check (`make check`: the `ts-offset` property set
//! and read back without a GLib warning, forwarded to the inner sink, audio to the end of a stream through ALSA's `null` device, the
//! log lines for start / first buffer / stop, an error and a warning logged once per second). Needs GStreamer development files
//! (gstreamer-1.0, gstreamer-audio-1.0, alsa, audiotestsrc); without them the test says so and passes.

use std::path::PathBuf;
use std::process::Command;

fn have(pkg: &str) -> bool {
    Command::new("pkg-config").args(["--exists", pkg]).status().map(|s| s.success()).unwrap_or(false)
}

#[test]
fn the_web_sound_sink_takes_ts_offset_and_logs_its_errors() {
    if !have("gstreamer-1.0") || !have("gstreamer-audio-1.0") || !have("alsa") {
        eprintln!("skipped: no GStreamer or ALSA development files (pkg-config gstreamer-1.0 gstreamer-audio-1.0 alsa)");
        return;
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dist/webview/wpe-helper");
    let out = std::env::temp_dir().join(format!("iyakki-websink-check-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    let exe = out.join("websink-check");
    let flags = String::from_utf8(Command::new("pkg-config").args(["--cflags", "--libs", "gstreamer-1.0", "gstreamer-audio-1.0", "alsa"]).output().unwrap().stdout).unwrap();
    let build = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()))
        .args(["-O1", "-Wall", "-I"]).arg(&dir).arg("-o").arg(&exe).arg(dir.join("websink-check.c"))
        .args(flags.split_whitespace())
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed:\n{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&exe).output().unwrap();
    let _ = std::fs::remove_dir_all(&out);
    let text = format!("{}{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    assert!(run.status.success() && text.contains("websink-check: ok"), "self check failed:\n{text}");
}
