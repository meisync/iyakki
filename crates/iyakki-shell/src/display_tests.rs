//! `visible_size` and its sysfs readers: the MacBook case (buffer larger than the panel's mode) and the odd ones.

use super::*;

fn conn(name: &str, connected: bool, modes: &[(u32, u32)]) -> Connector {
    Connector { name: name.into(), connected, enabled: None, modes: modes.to_vec() }
}

#[test]
fn a_buffer_the_size_of_the_mode_is_unchanged() {
    let v = visible_size((2560, 1600), (2560, 1600), &[conn("eDP-1", true, &[(2560, 1600), (1920, 1200)])]);
    assert_eq!((v.width, v.height), (2560, 1600));
    assert!(!v.smaller_than((2560, 1600)));
}

#[test]
fn the_macbook_buffer_larger_than_the_edp_mode_shows_only_the_mode() {
    let v = visible_size((2880, 1800), (2880, 1800), &[conn("eDP-1", true, &[(2560, 1600)])]);
    assert_eq!((v.width, v.height), (2560, 1600));
    assert_eq!(v.source, "eDP-1 preferred mode");
    assert!(v.smaller_than((2880, 1800)));
}

#[test]
fn with_several_connectors_the_built_in_panel_wins_and_else_the_smallest() {
    let both = [conn("HDMI-A-1", true, &[(3840, 2160)]), conn("eDP-1", true, &[(2560, 1600)])];
    let v = visible_size((2880, 1800), (2880, 1800), &both);
    assert_eq!((v.width, v.height), (2560, 1600), "HDMI 4K does not fit and eDP is primary");
    let two_ext = [conn("HDMI-A-1", true, &[(1920, 1080)]), conn("DP-1", true, &[(1280, 720)])];
    assert_eq!(visible_size((2880, 1800), (2880, 1800), &two_ext).width, 1280, "the smallest external");
    let off = [conn("eDP-1", false, &[(2560, 1600)]), conn("HDMI-A-1", true, &[(1920, 1080)])];
    assert_eq!(visible_size((2880, 1800), (2880, 1800), &off).width, 1920, "a disconnected panel is ignored");
    let mut disabled = both.to_vec();
    disabled[0].enabled = Some(true);
    disabled[1].enabled = Some(false);
    // only the enabled one scans out
    assert_eq!(visible_size((3840, 2160), (3840, 2160), &disabled).width, 3840);
}

#[test]
fn a_mode_larger_than_the_buffer_uses_the_buffer() {
    let v = visible_size((1920, 1080), (1920, 1080), &[conn("eDP-1", true, &[(2560, 1600)])]);
    assert_eq!((v.width, v.height), (1920, 1080));
    assert!(v.source.contains("does not fit"));
    // a rotated panel (1600x2560) is not rotated and does not fit either
    let r = visible_size((2880, 1800), (2880, 1800), &[conn("DSI-1", true, &[(1600, 2560)])]);
    assert_eq!((r.width, r.height), (2880, 1800));
    // a mode that fits in both dimensions in portrait is just a size
    let p = visible_size((1800, 2880), (1800, 2880), &[conn("DSI-1", true, &[(1600, 2560)])]);
    assert_eq!((p.width, p.height), (1600, 2560));
}

#[test]
fn nothing_readable_or_zero_modes_use_the_buffer() {
    for c in [vec![], vec![conn("eDP-1", true, &[])], vec![conn("eDP-1", true, &[(0, 0)])], vec![conn("eDP-1", false, &[(1280, 800)])]] {
        let v = visible_size((2880, 1800), (2880, 1800), &c);
        assert_eq!((v.width, v.height), (2880, 1800), "{c:?}");
    }
    assert_eq!(visible_size((0, 0), (0, 0), &[]), Visible { width: 0, height: 0, source: "framebuffer size; no connected display mode readable".into() });
}

#[test]
fn odd_modes_and_an_honest_framebuffer() {
    let v = visible_size((1920, 1080), (1920, 1080), &[conn("eDP-1", true, &[(1366, 768)])]);
    assert_eq!((v.width, v.height), (1366, 768));
    // the driver itself reports a smaller visible size: that is the base, and a smaller mode still wins
    assert_eq!(visible_size((2880, 1800), (2560, 1600), &[]).width, 2560);
    assert_eq!(visible_size((2880, 1800), (2560, 1600), &[conn("eDP-1", true, &[(2880, 1800)])]).width, 2560);
    // an impossible fb_var (larger than the buffer, or zero) is ignored
    assert_eq!(visible_size((2880, 1800), (4000, 4000), &[]).width, 2880);
    assert_eq!(visible_size((2880, 1800), (0, 0), &[]).width, 2880);
}

#[test]
fn modes_parse_from_sysfs_lines() {
    assert_eq!(parse_mode("2560x1600"), Some((2560, 1600)));
    assert_eq!(parse_mode("U:2880x1800p-0"), Some((2880, 1800)));
    assert_eq!(parse_mode("1920x1080i"), Some((1920, 1080)));
    assert_eq!(parse_mode("0x0"), None);
    assert_eq!(parse_mode("garbage"), None);
    assert_eq!(parse_mode(""), None);
}

fn fake_drm(tag: &str, entries: &[(&str, &str, &str, Option<&str>)]) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-drm-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    for (name, status, modes, enabled) in entries {
        let p = d.join(name);
        std::fs::create_dir_all(&p).unwrap();
        std::fs::write(p.join("status"), format!("{status}\n")).unwrap();
        std::fs::write(p.join("modes"), modes).unwrap();
        if let Some(e) = enabled {
            std::fs::write(p.join("enabled"), format!("{e}\n")).unwrap();
        }
    }
    std::fs::create_dir_all(d.join("version")).unwrap(); // not a connector
    d
}

#[test]
fn fake_sysfs_directories_are_read() {
    let d = fake_drm("read", &[("card1-eDP-1", "connected", "2560x1600\n1920x1200\n", Some("enabled")), ("card1-HDMI-A-1", "disconnected", "", Some("disabled")), ("card1-DP-1", "connected", "3840x2160\n", None), ("card0-Unknown-1", "connected", "2880x1800\n", None)]);
    let c = read_connectors(&d);
    assert_eq!(c.len(), 4);
    let v = visible_size((2880, 1800), (2880, 1800), &c);
    assert_eq!((v.width, v.height, v.source.as_str()), (2560, 1600, "eDP-1 preferred mode"));
    assert_eq!(signature(&d), "Unknown-1:2880x1800,DP-1:3840x2160,eDP-1:2560x1600");
    // unreadable / missing directory: nothing, never an error
    assert!(read_connectors(&d.join("nope")).is_empty());
    assert_eq!(visible_size((2880, 1800), (2880, 1800), &read_connectors(&d.join("nope"))).width, 2880);
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn clear_outside_blanks_only_the_hidden_part() {
    let (pitch, rows) = (16usize, 6usize); // 4 px per row, 4 bytes each
    let mut mem = vec![7u8; pitch * rows];
    clear_outside(&mut mem, pitch, rows, 3, 4, 4);
    for y in 0..rows {
        for x in 0..4 {
            let v = mem[y * pitch + x * 4];
            assert_eq!(v == 7, y < 4 && x < 3, "({x},{y})");
        }
    }
    let mut short = vec![7u8; 20]; // memory shorter than pitch * rows must not panic
    clear_outside(&mut short, pitch, rows, 1, 1, 4);
    assert_eq!(short[0], 7);
    assert_eq!(short[19], 0);
}
