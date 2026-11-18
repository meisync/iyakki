//! The login screens' top bar and power card (`loginbar.rs`): where the pieces sit at five screen sizes, the keys and clicks
//! of the card, the remote session that gets neither, and a host that cannot power off.

use super::tests::*;
use super::*;
use crate::devpop::{Item, Jump, Level, Snapshot};
use crate::loginbar::{self, Chip};

const SIZES: [(u32, u32); 5] = [(1280, 720), (1920, 1080), (1024, 600), (800, 480), (360, 740)];

fn item(title: &str, detail: &str, level: Level) -> Item {
    Item { title: title.into(), detail: detail.into(), level, jump: Jump::None, acts: vec![] }
}

fn rendered(sh: &mut Shell, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    sh.render(&mut g);
    g
}

fn inside(r: R, w: u32, h: u32) -> bool {
    r.x >= 0.0 && r.y >= 0.0 && r.x + r.w <= w as f32 && r.y + r.h <= h as f32 && r.w > 0.0 && r.h > 0.0
}

fn apart(a: R, b: R) -> bool {
    a.x + a.w <= b.x || b.x + b.w <= a.x || a.y + a.h <= b.y || b.y + b.h <= a.y
}

fn card_of(w: u32, h: u32, fields: usize) -> R {
    auth_card(w, h, scale_for(w, h), fields)
}

#[test]
fn the_bar_shows_on_login_and_setup_and_never_touches_the_form_at_any_size() {
    for (w, h) in SIZES {
        for setup in [false, true] {
            let (mut sh, _) = if setup { (Shell::new(env("lb-setup").0), None) } else { let (s, h) = shell_with_users("lb-login"); (s, Some(h)) };
            assert_eq!(matches!(sh.screen, Screen::Setup(_)), setup);
            let fields = if let Screen::Setup(f) | Screen::Login(f) = &sh.screen { f.fields.len() } else { 0 };
            sh.handle(&Event::Tick);
            rendered(&mut sh, w, h);
            let (chips, power) = sh.login_bar_layout();
            let power = power.unwrap_or_else(|| panic!("{w}x{h} setup={setup}: no Power button"));
            assert!(inside(power, w, h), "{w}x{h}: Power hit region {power:?}");
            assert!(chips.iter().any(|(c, _)| *c == Chip::Time), "{w}x{h}: the clock is shown");
            let card = card_of(w, h, fields);
            for (c, r) in chips {
                assert!(inside(*r, w, h), "{w}x{h} {c:?} {r:?}");
                assert!(apart(*r, card), "{w}x{h} setup={setup}: {c:?} {r:?} overlaps the form {card:?}");
            }
            for (i, (c, a)) in chips.iter().enumerate() {
                for (d, b) in &chips[i + 1..] {
                    assert!(apart(*a, *b), "{w}x{h}: {c:?} overlaps {d:?}");
                }
            }
            // the Power button is the rightmost piece, and a click on it is wired
            assert!(chips.iter().all(|(_, r)| r.x <= power.x), "{w}x{h}: Power is rightmost");
            assert!(sh.hits().iter().any(|h| h.label == "power" && h.rect.x == power.x), "{w}x{h}");
            // the form's own fields are all below the bar
            for i in 0..fields {
                assert!(apart(auth_field_rect(w, h, scale_for(w, h), i, fields), power), "{w}x{h} field {i}");
            }
        }
    }
}

#[test]
fn the_chips_say_what_the_machine_has() {
    let snap = |items| Snapshot { items, driver_help: vec![], at: 1 };
    assert_eq!(loginbar::network_chip(None).0, "checking network");
    let wifi = snap(vec![item("Wi-Fi", "host name box · wlan0 · HomeNet · signal -50 dBm (excellent) · 10.0.0.5", Level::Ok)]);
    assert_eq!(loginbar::network_chip(Some(&wifi)), ("Wi-Fi: HomeNet".to_string(), true));
    let eth = snap(vec![item("Ethernet", "host name box · eth0 · connected · 10.0.0.2", Level::Ok), item("Wi-Fi", "wlan0 · on, not joined to a network", Level::Warn)]);
    assert_eq!(loginbar::network_chip(Some(&eth)), ("Ethernet connected".to_string(), true));
    let none = snap(vec![item("Ethernet", "eth0 · no cable plugged in", Level::Warn)]);
    assert_eq!(loginbar::network_chip(Some(&none)), ("no network".to_string(), false));
    let bat = snap(vec![item("Battery", "85% · charging · mains power connected", Level::Ok)]);
    assert_eq!(loginbar::battery_chip(Some(&bat)), Some(("Battery 85% charging".to_string(), true)));
    assert_eq!(loginbar::battery_chip(Some(&none)), None, "no battery, no chip");
}

#[test]
fn when_room_runs_out_the_date_goes_first_and_power_never() {
    let widths = [(Chip::Host, 100.0), (Chip::Network, 150.0), (Chip::Battery, 120.0), (Chip::Date, 80.0), (Chip::Time, 60.0), (Chip::Power, 80.0)];
    let all = loginbar::plan(1000.0, 1.0, &widths);
    assert_eq!(all.len(), 6);
    let tight = loginbar::plan(230.0, 1.0, &widths);
    let kinds: Vec<Chip> = tight.iter().map(|(c, _)| *c).collect();
    assert!(kinds.contains(&Chip::Power) && kinds.contains(&Chip::Time) && !kinds.contains(&Chip::Date), "{kinds:?}");
    for (_, r) in &tight {
        assert!(r.x >= 0.0 && r.x + r.w <= 230.0);
    }
}

#[test]
fn the_power_button_and_ctrl_p_open_a_card_that_never_acts_by_itself() {
    for (w, h) in SIZES {
        let (mut sh, host) = shell_with_users("lb-card");
        sh.handle(&Event::Tick);
        rendered(&mut sh, w, h);
        let p = sh.login_bar_layout().1.unwrap();
        sh.handle(&Event::MouseDown((p.x + p.w / 2.0) as i32, (p.y + p.h / 2.0) as i32));
        assert!(sh.lbar.card.is_some(), "{w}x{h}: the click opens the card");
        rendered(&mut sh, w, h);
        for label in ["Power off", "Restart", "Cancel"] {
            let hit = sh.hits().iter().find(|h| h.label == label).unwrap_or_else(|| panic!("{w}x{h}: {label} button"));
            assert!(inside(hit.rect, w, h), "{w}x{h}: {label} {:?}", hit.rect);
        }
        assert!(host.powered_off.lock().unwrap().is_none(), "opening does nothing");
        // a stray key closes it without acting
        sh.handle(&Event::Key(Key::Char('x')));
        assert!(sh.lbar.card.is_none() && host.powered_off.lock().unwrap().is_none());
        // Ctrl+P opens it again; a click on Cancel closes it
        sh.handle(&Event::Key(Key::Ctrl('p')));
        rendered(&mut sh, w, h);
        let c = sh.hits().iter().find(|h| h.label == "Cancel").unwrap().rect;
        sh.handle(&Event::MouseDown((c.x + c.w / 2.0) as i32, (c.y + c.h / 2.0) as i32));
        assert!(sh.lbar.card.is_none() && host.powered_off.lock().unwrap().is_none());
    }
}

#[test]
fn keyboard_choices_are_audited_as_the_login_screen() {
    let (mut sh, host) = shell_with_users("lb-keys");
    // Enter takes the highlighted choice (Power off first), Right moves it
    sh.handle(&Event::Key(Key::Ctrl('p')));
    sh.handle(&Event::Key(Key::Esc));
    assert!(sh.lbar.card.is_none() && host.powered_off.lock().unwrap().is_none(), "Esc cancels");
    sh.handle(&Event::Key(Key::Ctrl('p')));
    sh.handle(&Event::Key(Key::Right));
    assert_eq!(sh.lbar.card, Some(1));
    sh.handle(&Event::Key(Key::Enter));
    assert_eq!(*host.powered_off.lock().unwrap(), Some("reboot".into()));
    let (mut sh, host) = shell_with_users("lb-keys2");
    sh.handle(&Event::Key(Key::Ctrl('p')));
    sh.handle(&Event::Key(Key::Char('y')));
    assert_eq!(*host.powered_off.lock().unwrap(), Some("off".into()));
    let log = std::fs::read_to_string(&sh.env.audit.path()).unwrap_or_default();
    assert!(log.contains("login screen") && log.contains("power.off"), "{log}");
    // Cancel highlighted (Left from the first) + Enter closes without acting
    let (mut sh, host) = shell_with_users("lb-keys3");
    sh.handle(&Event::Key(Key::Ctrl('p')));
    sh.handle(&Event::Key(Key::Left));
    assert_eq!(sh.lbar.card, Some(2));
    sh.handle(&Event::Key(Key::Enter));
    assert!(sh.lbar.card.is_none() && host.powered_off.lock().unwrap().is_none());
    // the power key of the keyboard opens it too
    let (mut sh, _) = shell_with_users("lb-fn");
    sh.handle(&Event::Tick);
    sh.handle(&Event::Tick);
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::Power, true));
    assert!(sh.lbar.card.is_some());
}

#[test]
fn a_remote_session_has_no_power_button_and_cannot_power_off() {
    let (mut sh, host) = shell_with_users("lb-remote");
    sh.set_remote(true);
    sh.handle(&Event::Tick);
    for (w, h) in SIZES {
        rendered(&mut sh, w, h);
        assert!(sh.login_bar_layout().1.is_none() && sh.login_bar_layout().0.is_empty(), "{w}x{h}");
        assert!(sh.hits().iter().all(|h| h.label != "power"));
    }
    sh.handle(&Event::Key(Key::Ctrl('p')));
    assert!(sh.lbar.card.is_none(), "Ctrl+P does nothing");
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::Power, true));
    assert!(sh.lbar.card.is_none() && !sh.fnk.power_card);
    sh.lbar.card = Some(0); // even if a card were forced open, the action is refused
    sh.handle(&Event::Key(Key::Char('y')));
    assert!(host.powered_off.lock().unwrap().is_none());
    // a signed-in person keeps the power policy rules (no bar after sign-in; the Fn card works)
    login(&mut sh, "root", "rootpass12");
    sh.handle(&Event::Tick);
    sh.handle(&Event::Tick);
    sh.handle(&Event::Fn(crate::fnkeys::FnKey::Power, true));
    assert!(sh.fnk.power_card);
}

struct NoPower;
impl HostApi for NoPower {
    fn detect_env(&self) -> String {
        "dev".into()
    }
    fn net_status(&self) -> String {
        String::new()
    }
    fn run_wasm(&self, _: &str, _: &iyakki_sec::policy::Policy, _: &[u8], _: &str) -> Result<String, String> {
        Ok(String::new())
    }
    fn power_off(&self) {
        panic!("must not be called");
    }
    fn reboot(&self) {
        panic!("must not be called");
    }
    fn can_power(&self) -> bool {
        false
    }
}

#[test]
fn a_host_that_cannot_power_off_says_so() {
    let (mut e, _) = env("lb-nopower");
    e.host = Arc::new(NoPower);
    let mut sh = Shell::new(e);
    sh.handle(&Event::Key(Key::Ctrl('p')));
    rendered(&mut sh, 800, 480);
    assert!(sh.hits().iter().any(|h| h.label == "Power off"), "the card still shows");
    sh.handle(&Event::Key(Key::Char('y')));
    assert!(sh.toast_text().is_some_and(|t| t.contains("cannot power off from here")), "{:?}", sh.toast_text());
    assert!(sh.lbar.card.is_none());
}

#[test]
fn cloud_mode_hides_nothing_here() {
    let (mut sh, _) = shell_with_users("lb-cloud");
    sh.cloud.fn_keys_off = true;
    sh.handle(&Event::Tick);
    rendered(&mut sh, 1280, 720);
    assert!(sh.login_bar_layout().1.is_some());
    sh.handle(&Event::Key(Key::Ctrl('p')));
    assert!(sh.lbar.card.is_some());
}

#[test]
fn the_login_card_shows_sleep_only_when_the_machine_can_sleep_and_s_sleeps_once() {
    for (w, h) in SIZES {
        // not possible: no Sleep button and `s` is just a stray key that closes the card
        let (mut sh, host) = shell_with_users("lb-nosleep");
        sh.handle(&Event::Key(Key::Ctrl('p')));
        rendered(&mut sh, w, h);
        assert!(!sh.hits().iter().any(|x| x.label == "Sleep"), "{w}x{h}: no dead button");
        sh.handle(&Event::Key(Key::Char('s')));
        assert!(sh.lbar.card.is_none() && *host.slept.lock().unwrap() == 0);
        // possible: the button shows inside the screen, `s` sleeps once, Esc cancels
        let (mut sh, host) = shell_with_users("lb-sleep");
        *host.sleepable.lock().unwrap() = true;
        sh.handle(&Event::Key(Key::Ctrl('p')));
        rendered(&mut sh, w, h);
        for label in ["Power off", "Restart", "Sleep", "Cancel"] {
            let hit = sh.hits().iter().find(|x| x.label == label).unwrap_or_else(|| panic!("{w}x{h}: {label}"));
            assert!(inside(hit.rect, w, h), "{w}x{h}: {label} {:?}", hit.rect);
        }
        sh.handle(&Event::Key(Key::Esc));
        assert!(sh.lbar.card.is_none() && *host.slept.lock().unwrap() == 0, "Esc cancels");
        sh.handle(&Event::Key(Key::Ctrl('p')));
        sh.handle(&Event::Key(Key::Char('s')));
        assert!(sh.lbar.card.is_none());
        assert_eq!(*host.slept.lock().unwrap(), 1);
        assert!(host.powered_off.lock().unwrap().is_none());
        // the highlighted Sleep (two steps right) with Enter does the same
        let (mut sh, host) = shell_with_users("lb-sleep2");
        *host.sleepable.lock().unwrap() = true;
        sh.handle(&Event::Key(Key::Ctrl('p')));
        sh.handle(&Event::Key(Key::Right));
        sh.handle(&Event::Key(Key::Right));
        sh.handle(&Event::Key(Key::Enter));
        assert_eq!(*host.slept.lock().unwrap(), 1);
    }
}
