//! Journeys through the real shell: real mouse and key events go in, the frame comes out as pixels, and the checks read the
//! pixels (where the picture changed, how much, and that it went back) as well as the screen the shell says it is on. These are
//! the top bar (Settings icon, Home icon, speaker dropdown, power menu) and the panes (Ctrl+Alt layouts), at three screen sizes.
use iyakki_agent::facets_h::RealServices as _;

use super::tests::*;
use super::*;
use crate::panes::LayoutKey;

const SIZES: [(u32, u32); 3] = [(1280, 720), (1024, 600), (800, 480)];

fn signed(tag: &str, w: u32, h: u32) -> (Shell, Gfx) {
    let (mut sh, _) = shell_with_users(tag);
    login(&mut sh, "root", "rootpass12");
    sh.fnk.sound_probe = Some(true);
    sh.set_mem_for_test(Some(crate::memgate::MemFacts { total: 16 * crate::memgate::GIB, available: 12 * crate::memgate::GIB }));
    let mut g = Gfx::new(w, h);
    sh.handle(&Event::MouseMove(1, h as i32 - 1));
    sh.render(&mut g);
    (sh, g)
}

fn px(g: &Gfx) -> Vec<u8> {
    g.pm.data().to_vec()
}

/// The box (x0, y0, x1, y1) around every pixel that differs between two frames of the same size, and how many differ.
fn diff(a: &[u8], b: &[u8], w: u32) -> Option<(i32, i32, i32, i32, usize)> {
    assert_eq!(a.len(), b.len());
    let (mut x0, mut y0, mut x1, mut y1, mut n) = (i32::MAX, i32::MAX, -1, -1, 0usize);
    for (i, (p, q)) in a.chunks_exact(4).zip(b.chunks_exact(4)).enumerate() {
        if p != q {
            let (x, y) = ((i as u32 % w) as i32, (i as u32 / w) as i32);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
            n += 1;
        }
    }
    (n > 0).then_some((x0, y0, x1, y1, n))
}

/// How many pixels inside the rectangle differ between two frames.
fn diff_in(a: &[u8], b: &[u8], w: u32, r: R) -> usize {
    let mut n = 0;
    for y in r.y.max(0.0) as u32..(r.y + r.h) as u32 {
        for x in r.x.max(0.0) as u32..(r.x + r.w) as u32 {
            let i = ((y * w + x) * 4) as usize;
            if i + 4 <= a.len() && a[i..i + 4] != b[i..i + 4] {
                n += 1;
            }
        }
    }
    n
}

fn label(sh: &Shell, l: &str) -> Option<R> {
    sh.hits().iter().rev().find(|h| h.label == l).map(|h| h.rect)
}

fn click_label(sh: &mut Shell, g: &mut Gfx, l: &str) {
    sh.render(g);
    let r = label(sh, l).unwrap_or_else(|| panic!("a hit region named {l:?}"));
    click_rect(sh, g, r);
}

fn click_rect(sh: &mut Shell, g: &mut Gfx, r: R) {
    let (x, y) = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
    sh.handle(&Event::MouseMove(x, y));
    sh.handle(&Event::MouseDown(x, y));
    sh.handle(&Event::MouseUp(x, y));
    sh.handle(&Event::MouseMove(1, g.pm.height() as i32 - 1));
    sh.render(g);
}

fn inside(b: (i32, i32, i32, i32, usize), r: R) -> bool {
    b.0 as f32 <= r.x + 1.0 && b.1 as f32 <= r.y + 1.0 && b.2 as f32 >= r.x + r.w - 2.0 && b.3 as f32 >= r.y + r.h - 2.0
}

fn vol(sh: &Shell) -> u32 {
    sh.env.services.real().master().percent
}

#[test]
fn the_top_bar_journey_changes_the_pixels_where_each_control_is_and_goes_back() {
    for (w, h) in SIZES {
        let (mut sh, mut g) = signed("journey-bar", w, h);
        sh.env.services.update_sound(|s| {
            s.volume_pct = 50;
            s.muted = false;
        })
        .unwrap();
        sh.render(&mut g);
        let home = px(&g);
        assert!(home.chunks_exact(4).any(|p| p[..3] != home[..3]), "{w}x{h}: the home screen is not one flat colour");

        // the speaker: a click paints a dropdown over the tiles (the Mute button's rectangle was plain before) and nothing else
        click_label(&mut sh, &mut g, "volume");
        let open = px(&g);
        let b = diff(&home, &open, w).unwrap_or_else(|| panic!("{w}x{h}: the click painted nothing"));
        let mute = label(&sh, "volume mute").expect("a Mute button");
        assert!(inside(b, mute), "{w}x{h}: the changed box {b:?} covers the Mute button {mute:?}");
        assert!(b.4 < (w * h) as usize / 3, "{w}x{h}: only the dropdown changed, not the whole screen ({} px)", b.4);
        assert!(sh.volume_slider_open());

        // Louder: the level moves by five and the dropdown's own pixels (the slider fill and the percent) change
        click_label(&mut sh, &mut g, "volume louder");
        assert_eq!(vol(&sh), 55, "{w}x{h}: Louder is +5");
        let louder = px(&g);
        let b2 = diff(&open, &louder, w).unwrap_or_else(|| panic!("{w}x{h}: Louder repainted nothing"));
        assert!(b2.4 < b.4, "{w}x{h}: a level change repaints less than the whole dropdown");
        click_label(&mut sh, &mut g, "volume quieter");
        click_label(&mut sh, &mut g, "volume quieter");
        assert_eq!(vol(&sh), 45);

        // Esc closes it: where the dropdown was, the pixels are the home screen's again (the tiles may breathe, so a share is allowed)
        key(&mut sh, Key::Esc);
        assert!(!sh.volume_slider_open() && sh.screen_name() == "launcher", "{w}x{h}: Esc closes the dropdown and stays on Home");
        sh.render(&mut g);
        let shut = px(&g);
        let was = diff_in(&home, &open, w, mute);
        let now = diff_in(&home, &shut, w, mute);
        assert!(was > 50, "{w}x{h}: the dropdown really covered the Mute button's place ({was} px)");
        let from_open = diff_in(&open, &shut, w, mute);
        assert!(now < from_open, "{w}x{h}: after Esc the Mute button's place looks like the home screen ({now} px differ) and not like the open dropdown ({from_open} px differ; was {was})");

        // the gear opens Settings (the whole screen changes), the Home icon brings the tiles back
        let gear = sh.settings_rect().expect("the gear icon is in the home screen's bar");
        click_rect(&mut sh, &mut g, gear);
        assert_eq!(sh.screen_name(), "app:settings", "{w}x{h}");
        let settings = px(&g);
        let sb = diff(&home, &settings, w).unwrap();
        assert!(sb.4 > (w * h) as usize / 4, "{w}x{h}: Settings is a different screen, not a nudge ({} px)", sb.4);
        assert!(label(&sh, "home").is_some() && label(&sh, "settings").is_some() && label(&sh, "volume").is_some(), "{w}x{h}: the same three icons are in the app's header");
        click_label(&mut sh, &mut g, "home");
        assert_eq!(sh.screen_name(), "launcher", "{w}x{h}: the Home icon returns to the tiles");
        let again = px(&g);
        assert!(diff_in(&settings, &again, w, R::new(0.0, 0.0, w as f32, h as f32)) > (w * h) as usize / 4, "{w}x{h}: Home is a different picture from Settings");

        // the power menu hangs under the top bar's right end; Esc closes it
        key(&mut sh, Key::Char('p'));
        sh.render(&mut g);
        assert_eq!(sh.screen_name(), "power", "{w}x{h}");
        let pw = px(&g);
        let pb = diff(&again, &pw, w).unwrap();
        let lock = label(&sh, "Lock").expect("a Lock button for a signed-in person");
        assert!(lock.x > w as f32 / 2.0 && lock.y < h as f32 / 2.0, "{w}x{h}: the Lock button is in the top right: {lock:?}");
        assert!(inside(pb, lock), "{w}x{h}: the menu's pixels cover the Lock button");
        key(&mut sh, Key::Esc);
        assert_eq!(sh.screen_name(), "launcher");
    }
}

#[test]
fn the_panes_journey_two_panes_split_the_pixels_and_one_pane_brings_the_home_screen_back() {
    for (w, h) in SIZES {
        let (mut sh, mut g) = signed("journey-panes", w, h);
        let one = px(&g);
        sh.handle(&Event::Layout(LayoutKey::Apply(2)));
        sh.render(&mut g);
        let two = px(&g);
        let b = diff(&one, &two, w).unwrap_or_else(|| panic!("{w}x{h}: Ctrl+Alt+2 changed nothing"));
        assert!(b.4 > (w * h) as usize / 10, "{w}x{h}: two panes repaint a good part of the screen ({} px)", b.4);
        // a vertical line near the middle where the two panes meet: the columns just left and right of the middle differ in the
        // body rows more than two columns inside one pane do
        let col = |x: u32, y: u32| -> [u8; 4] {
            let i = ((y * w + x) * 4) as usize;
            [two[i], two[i + 1], two[i + 2], two[i + 3]]
        };
        let y = h / 2;
        let mut changes = 0;
        for x in 1..w {
            if col(x, y) != col(x - 1, y) {
                changes += 1;
            }
        }
        assert!(changes > 4, "{w}x{h}: the middle row of two panes has edges (a divider and two apps)");
        sh.handle(&Event::Layout(LayoutKey::Apply(1)));
        sh.render(&mut g);
        let back = px(&g);
        let whole = R::new(0.0, 0.0, w as f32, h as f32);
        let (was, now) = (diff_in(&one, &two, w, whole), diff_in(&one, &back, w, whole));
        assert!(now * 5 < was, "{w}x{h}: the single layout brings the home screen back ({now} px differ, two panes had {was})");
    }
}

// ---- reading the real frame with an OCR program (a test tool only: nothing of it is shipped) ----

fn tesseract_here() -> bool {
    std::process::Command::new("tesseract").arg("--version").output().is_ok()
}

/// Write the frame as a grey, upscaled, dark-on-light picture, run tesseract on it and return what it read (and the word boxes
/// as `(text, height)` pairs, heights in the picture's own pixels).
fn ocr(g: &Gfx, tag: &str) -> (String, Vec<(String, u32)>) {
    let (w, h) = (g.width() as usize, g.height() as usize);
    let d = g.pm.data();
    let mut grey: Vec<u8> = d.chunks_exact(4).map(|p| ((p[0] as u32 * 30 + p[1] as u32 * 59 + p[2] as u32 * 11) / 100) as u8).collect();
    if grey.iter().map(|&v| v as u64).sum::<u64>() / (grey.len() as u64) < 128 {
        for v in &mut grey {
            *v = 255 - *v;
        }
    }
    let up = 2;
    let mut out = format!("P5\n{} {}\n255\n", w * up, h * up).into_bytes();
    for y in 0..h * up {
        for x in 0..w * up {
            out.push(grey[(y / up) * w + x / up]);
        }
    }
    let dir = std::env::temp_dir().join(format!("iyakki-ocr-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pic = dir.join("frame.pgm");
    std::fs::write(&pic, out).unwrap();
    let run = |args: &[&str]| -> String {
        let o = std::process::Command::new("tesseract").arg(&pic).arg("stdout").args(args).output().expect("tesseract runs");
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    let text = run(&["--psm", "11"]);
    let tsv = run(&["--psm", "11", "tsv"]);
    let words = tsv
        .lines()
        .skip(1)
        .filter_map(|l| {
            let c: Vec<&str> = l.split('\t').collect();
            (c.len() >= 12 && !c[11].trim().is_empty()).then(|| (c[11].trim().to_string(), c[9].parse::<u32>().unwrap_or(0) / up as u32))
        })
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    (text, words)
}

fn has(text: &str, what: &str) -> bool {
    let n = |t: &str| t.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    n(text).contains(&n(what))
}

#[test]
fn what_a_person_would_read_is_in_the_pixels_sign_in_power_menu_and_the_time_zone_question() {
    if !tesseract_here() {
        eprintln!("skipped: tesseract (a test tool) is not installed");
        return;
    }
    // the sign-in screen: the button, the host and the form's words are legible in the drawn frame
    let (mut sh, _) = shell_with_users("ocr-login");
    let mut g = Gfx::new(1280, 720);
    sh.handle(&Event::MouseMove(1, 719));
    sh.render(&mut g);
    let (login, _) = ocr(&g, "login");
    assert!(has(&login, "Sign in") && has(&login, "Password"), "the sign-in button and the password field are readable: {login:?}");
    assert!(has(&login, "iyakki-test") && has(&login, "network"), "the top bar of the sign-in screen names the host and the network: {login:?}");

    // the power menu of a signed-in administrator
    login_as_root(&mut sh);
    sh.render(&mut g);
    key(&mut sh, Key::Char('p'));
    sh.render(&mut g);
    assert_eq!(sh.screen_name(), "power");
    let (power, _) = ocr(&g, "power");
    for w in ["Lock", "Restart", "Power off", "Cancel"] {
        assert!(has(&power, w), "{w} is readable in the power menu: {power:?}");
    }
    key(&mut sh, Key::Esc);

    // the one-time time zone question
    sh.tzask.open = true;
    sh.render(&mut g);
    let (tz, _) = ocr(&g, "tz");
    for w in ["Detect automatically", "Use UTC", "Not now"] {
        assert!(has(&tz, w), "{w} is readable on the time zone card: {tz:?}");
    }
}

fn login_as_root(sh: &mut Shell) {
    login(sh, "root", "rootpass12");
    sh.fnk.sound_probe = Some(true);
}

#[test]
fn a_larger_text_size_makes_the_words_on_the_home_screen_taller_in_the_drawn_frame() {
    if !tesseract_here() {
        eprintln!("skipped: tesseract (a test tool) is not installed");
        return;
    }
    let (mut sh, _) = shell_with_users("ocr-size");
    let dir = sh.services_for_demo().dir().to_path_buf();
    crate::uisize::save(&dir, "alice", 3); // Larger, 150%
    let mut g = Gfx::new(1280, 720);
    login(&mut sh, "root", "rootpass12");
    sh.render(&mut g);
    let (_, normal) = ocr(&g, "n");
    key(&mut sh, Key::Char('o'));
    login(&mut sh, "alice", "alicepass1");
    sh.render(&mut g);
    let (_, large) = ocr(&g, "l");
    let median = |v: &[(String, u32)]| {
        let mut h: Vec<u32> = v.iter().filter(|w| w.0.len() >= 3 && w.1 > 4).map(|w| w.1).collect();
        h.sort_unstable();
        h.get(h.len() / 2).copied().unwrap_or(0) as f32
    };
    let (a, b) = (median(&normal), median(&large));
    assert!(a > 0.0 && b > 0.0, "words were read in both pictures ({} and {})", normal.len(), large.len());
    assert!(b > a * 1.2, "150% text reads taller than normal text: median word height {a} -> {b}");
}
