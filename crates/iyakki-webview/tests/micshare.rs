//! The microphone plugin of the web helper (dist/webview/wpe-helper/micshare.c, scan logic in micscan.h). Runs the plugin's own C
//! self check (`make check`): fake /proc/asound trees (a T2-like layout with playback-only devices, an HDMI card, a webcam with a USB
//! microphone, the T2 capture switch on and off; a PC with an IEC958 capture and a loopback card; nothing at all) and the device
//! provider with a test microphone that refuses a second open: three pages asking for 44100 Hz stereo, 16000 Hz mono and 48000 Hz
//! stereo share one open, the card closes after the last page, a later page opens it again. Needs the GStreamer and alsa-lib
//! development files and audiotestsrc; without them the test says so and passes.

use std::path::PathBuf;
use std::process::Command;

fn have(pkg: &str) -> bool {
    Command::new("pkg-config").args(["--exists", pkg]).status().map(|s| s.success()).unwrap_or(false)
}

#[test]
fn the_microphone_plugin_lists_real_capture_streams_and_shares_one_open() {
    if !have("gstreamer-1.0") || !have("gstreamer-app-1.0") || !have("alsa") {
        eprintln!("skipped: no GStreamer or alsa-lib development files (pkg-config gstreamer-1.0 gstreamer-app-1.0 alsa)");
        return;
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dist/webview/wpe-helper");
    let out = std::env::temp_dir().join(format!("iyakki-mic-check-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    let exe = out.join("mic-check");
    let flags = String::from_utf8(Command::new("pkg-config").args(["--cflags", "--libs", "gstreamer-1.0", "gstreamer-app-1.0", "alsa"]).output().unwrap().stdout).unwrap();
    let build = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()))
        .args(["-O1", "-Wall", "-I"]).arg(&dir).arg("-o").arg(&exe).arg(dir.join("mic-check.c")).args(flags.split_whitespace()).output().unwrap();
    assert!(build.status.success(), "build failed:\n{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&exe).output().unwrap();
    let _ = std::fs::remove_dir_all(&out);
    let text = format!("{}{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    assert!(run.status.success() && text.contains("mic-check: ok"), "self check failed:\n{text}");
}
