use super::*;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-enginelog-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn page_addresses_and_console_messages_are_never_kept() {
    let d = tmp("pages");
    keep_in(&d, 100, "WebKit: load failed for https://shop.example.org/cart?user=ann&session=abc123 after 3 s");
    keep_in(&d, 101, "image from data:image/png;base64,AAAA and blob:null/1234 ignored");
    keep_in(&d, 102, "CONSOLE ERROR Uncaught TypeError: x is not a function at https://a.example/app.js:1");
    keep_in(&d, 103, "(Web Content:7) GStreamer warning: alsasink busy");
    let all = std::fs::read_to_string(d.join(FILE)).unwrap();
    for bad in ["shop.example.org", "session=abc123", "https://", "AAAA", "1234", "app.js", "TypeError"] {
        assert!(!all.contains(bad), "{bad:?} must not be kept:\n{all}");
    }
    assert!(all.contains("[address]"));
    assert!(all.contains("GStreamer warning: alsasink busy"), "the helper's own words stay");
    assert!(all.contains("100 WebKit: load failed for [address] after 3 s"));
}

#[test]
fn credentials_in_a_helper_line_are_removed() {
    let d = tmp("creds");
    keep_in(&d, 5, "helper env dump: token=ghp_0123456789abcdefghij0123456789abcdef and password: hunter2hunter2");
    keep_in(&d, 6, "key sk-abcdefghijklmnopqrstuvwxyz0123456789 seen");
    let all = std::fs::read_to_string(d.join(FILE)).unwrap();
    for bad in ["ghp_0123", "hunter2", "sk-abcdef"] {
        assert!(!all.contains(bad), "{bad:?} survived:\n{all}");
    }
}

#[test]
fn lines_are_cut_cleaned_and_empty_ones_dropped() {
    let d = tmp("shape");
    keep_in(&d, 1, "   ");
    keep_in(&d, 2, "\u{1b}[31mred\u{1b}[0m text\u{7}");
    keep_in(&d, 3, &"x".repeat(5000));
    let got = read_in(&d);
    assert_eq!(got.len(), 2);
    assert!(!got[0].1.contains('\u{1b}') && !got[0].1.contains('\u{7}'));
    assert_eq!(got[1].1.chars().count(), LINE_MAX);
}

#[test]
fn the_files_rotate_and_stay_near_200_kb() {
    let d = tmp("rotate");
    let line = "y".repeat(300);
    for i in 0..2000 {
        keep_in(&d, 1000 + i, &format!("{line} {i}"));
    }
    let sz = |n: &str| std::fs::metadata(d.join(n)).map(|m| m.len()).unwrap_or(0);
    assert!(sz(FILE) > 0 && sz(PREVIOUS) > 0, "both files exist");
    assert!(sz(FILE) < HALF_BYTES + 1024, "the current file stops near its half: {}", sz(FILE));
    assert!(sz(FILE) + sz(PREVIOUS) < 2 * HALF_BYTES + 2048);
    let all = read_in(&d);
    assert_eq!(all.last().unwrap().0, 2999, "the newest line is kept");
    assert!(all.first().unwrap().0 > 1000, "the oldest were dropped");
    assert!(all.windows(2).all(|w| w[0].0 < w[1].0), "oldest first");
    let t = tail_text(&d, 3);
    assert_eq!(t.lines().count(), 3);
}

#[test]
fn an_unwritable_folder_is_ignored() {
    keep_in(Path::new("/proc/definitely/not/here"), 1, "hello");
    assert!(read_in(Path::new("/proc/definitely/not/here")).is_empty());
}

fn l(t: i64, s: &str) -> (i64, String) {
    (t, s.to_string())
}

#[test]
fn a_crashed_helper_and_a_failing_camera_or_microphone_are_counted_in_the_window() {
    let lines = vec![
        l(10, "the helper closed the connection: segmentation fault"),
        l(500, "v4l2src0: Could not open device /dev/video0: Device or resource busy"),
        l(510, "alsasrc: Could not open audio device for recording"),
        l(520, "GStreamer-WARNING: a harmless note"),
        l(530, "camera ready"),
    ];
    let h = health(&lines, 400);
    assert_eq!((h.crashed, h.camera_trouble, h.mic_trouble), (0, 1, 1), "the old crash is outside the window");
    assert!(h.last.contains("alsasrc"));
    let all = health(&lines, 0);
    assert_eq!(all.crashed, 1);
    assert_eq!(health(&[], 0), Health::default());
}

#[test]
fn the_default_folder_is_the_crash_log_folder() {
    assert!(DEFAULT_DIR.ends_with("/log"));
    assert!(!DEFAULT_DIR.contains("home"), "no personal paths");
}

#[test]
fn developer_mode_reads_the_kept_lines_through_the_engine_source() {
    let d = tmp("devmode");
    keep_in(&d, 7, "helper: alsasink busy");
    keep_in(&d, 8, "login password: hunter2hunter2 was typed");
    let v = crate::devmode::debug::engine_lines(&d, 10);
    assert_eq!(v.len(), 2);
    assert_eq!(v[0], "[7] helper: alsasink busy");
    assert!(!v.join("\n").contains("hunter2"));
    assert_eq!(crate::devmode::debug::engine_lines(&d, 1).len(), 1);
}

#[test]
fn the_kept_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let d = tmp("mode");
    keep_in(&d, 1, "hello");
    assert_eq!(std::fs::metadata(d.join(FILE)).unwrap().permissions().mode() & 0o777, 0o600);
}
