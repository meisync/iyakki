//! The shared camera plugin of the web helper (dist/webview/wpe-helper/camshare.c): one open of a camera serves every page request.
//! Runs the plugin's own C self check (`make check`: mode choice, three pages with different sizes on one open of a test camera that
//! refuses a second open like a UVC node, a late page, close after the last page, reopen; raw and MJPEG). Needs the GStreamer
//! development files (gstreamer-1.0, gstreamer-app-1.0, videotestsrc, jpeg); without them the test says so and passes.

use std::path::PathBuf;
use std::process::Command;

fn have(pkg: &str) -> bool {
    Command::new("pkg-config").args(["--exists", pkg]).status().map(|s| s.success()).unwrap_or(false)
}

#[test]
fn the_shared_camera_opens_a_camera_once_for_every_page_request() {
    if !have("gstreamer-1.0") || !have("gstreamer-app-1.0") {
        eprintln!("skipped: no GStreamer development files (pkg-config gstreamer-1.0 gstreamer-app-1.0)");
        return;
    }
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../dist/webview/wpe-helper");
    let out = std::env::temp_dir().join(format!("iyakki-camshare-check-{}", std::process::id()));
    std::fs::create_dir_all(&out).unwrap();
    let exe = out.join("camshare-check");
    let build = Command::new(std::env::var("CC").unwrap_or_else(|_| "cc".into()))
        .args(["-O1", "-Wall", "-I"]).arg(&dir).arg("-o").arg(&exe).arg(dir.join("camshare-check.c"))
        .args(String::from_utf8(Command::new("pkg-config").args(["--cflags", "--libs", "gstreamer-1.0", "gstreamer-app-1.0"]).output().unwrap().stdout).unwrap().split_whitespace())
        .arg("-lm")
        .output()
        .unwrap();
    assert!(build.status.success(), "build failed:\n{}", String::from_utf8_lossy(&build.stderr));
    let run = Command::new(&exe).output().unwrap();
    let _ = std::fs::remove_dir_all(&out);
    let text = format!("{}{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
    assert!(run.status.success() && text.contains("camshare-check: ok"), "self check failed:\n{text}");
}
