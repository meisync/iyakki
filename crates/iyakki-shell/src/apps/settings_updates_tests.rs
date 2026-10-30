//! Settings -> System update: who sees the page, finding a pack, the plain preview, install with a yes, no install without one, a wrong
//! signature, rollback, the note after a start, the Overview line, and the page at four sizes with every state. The boot partition is a
//! folder with a size limit and the packs are made in a temp folder: no drive, no mount, no restart.

use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};
use iyakki_agent::devmode::centre as uc;
use iyakki_agent::devmode::update as up;

const MENU: &str = "set default=\"0\"\nmenuentry \"IYAKKI\" {\n\tlinux /boot/bzImage quiet console=tty0\n\tinitrd /boot/initrd\n}\n";

fn bytes(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed).wrapping_add((i >> 9) as u8)).collect()
}

fn put(root: &std::path::Path, rel: &str, data: &[u8]) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, data).unwrap();
}

/// A throwaway signing key made from a fixed seed (never a real key).
macro_rules! key {
    () => {
        iyakki_agent::pkg::signing_key_from_hex(&"05".repeat(32)).unwrap()
    };
}

struct Rig {
    boot: std::path::PathBuf,
    data: std::path::PathBuf,
    stick: std::path::PathBuf,
}

fn rig(tag: &str) -> Rig {
    let dir = temp_dir(&format!("upd-{tag}"));
    let boot = dir.join("bootpart");
    put(&boot, "boot/bzImage", &bytes(1 << 20, 1));
    put(&boot, "boot/initrd", &bytes(3 << 20, 2));
    put(&boot, "boot/grub/grub.cfg", MENU.as_bytes());
    let data = dir.join("data");
    put(&data, "home/amy/a.txt", b"my file");
    Rig { stick: dir.join("stick"), boot, data }
}

/// A pack of `version` on the "stick", signed or not.
fn pack(r: &Rig, name: &str, version: &str, variant: &str, initrd_mb: usize, signed: bool) -> std::path::PathBuf {
    let d = r.stick.join(name);
    put(&d, "boot/bzImage", &bytes((1 << 20) + 700, 11));
    put(&d, "boot/initrd", &bytes(initrd_mb << 20, 12));
    put(&d, "boot/grub/grub.cfg", MENU.replace("quiet", "quiet splash").as_bytes());
    put(&d, "boot/iyakki-release", format!("VERSION={version}\nVARIANT={variant}\nBUILD_DATE=2026-10-22\n").as_bytes());
    let list: Vec<(String, u64, String)> = up::list_boot(&d).into_iter().map(|(p, s)| (p.clone(), s, iyakki_agent::devmode::files::sha256_file(&d.join(&p)).unwrap())).collect();
    let m = up::build_manifest(&d, &list, None, None, "test").unwrap();
    up::write_manifest(&d, &m).unwrap();
    if signed {
        uc::sign_pack(&d, &key!()).unwrap();
    }
    d
}

fn app(r: &Rig, cx: &AppCx) -> SettingsApp {
    let c = uc::Centre::new(&r.data, Box::new(up::DirBoot { root: r.boot.clone(), capacity: 14 << 20, cluster: 4096 }), "VERSION=0.1.0-alpha.1\nVARIANT=pc\nBUILD_DATE=2026-10-20\n".into(), false, Some(key!().verifying_key())).with_data_free(400 << 20);
    let mut a = SettingsApp::new(cx).with_updates(c, vec![r.stick.clone()], "boot-1");
    a.go_to(Tab::Updates);
    a
}

fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    a.handle(&Event::Key(Key::Char(c)), cx);
}

fn settle(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..4000 {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("the job did not finish");
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::shell::scale_for(w, h);
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
    g
}

fn boot_initrd(r: &Rig) -> String {
    iyakki_agent::devmode::files::sha256_file(&r.boot.join("boot/initrd")).unwrap()
}

#[test]
fn only_an_administrator_has_the_page() {
    let r = rig("admin");
    let cx = cx_with_admin_store("upd-admin");
    assert!(app(&r, &cx).tabs().contains(&Tab::Updates));
    let mut cx = cx_with_admin_store("upd-user");
    cx.session.user = "amy".into();
    cx.session.role = iyakki_sec::policy::Role::User;
    cx.session.policy = iyakki_sec::policy::Role::User.template();
    assert!(!SettingsApp::new(&cx).tabs().contains(&Tab::Updates));
}

#[test]
fn the_page_shows_versions_finds_a_pack_and_says_what_will_change() {
    let r = rig("show");
    pack(&r, "iyakki-0.1.0-alpha.2", "0.1.0-alpha.2", "pc", 4, true);
    let cx = cx_with_admin_store("upd-show");
    let mut a = app(&r, &cx);
    press(&mut a, &cx, 's');
    assert_eq!(a.upd.found.len(), 1);
    let pv = a.upd.preview.clone().unwrap().unwrap();
    assert_eq!((pv.from.as_str(), pv.to.as_str()), ("0.1.0-alpha.1", "0.1.0-alpha.2"));
    assert!(pv.confirm.is_empty());
    let g = draw(&mut a, &cx, 1280, 720);
    assert!(g.hits.iter().any(|h| h.label == "Install"));
    assert_eq!(a.upd.versions.as_ref().unwrap().on_disk, "0.1.0-alpha.1");
}

#[test]
fn install_asks_first_then_writes_only_the_boot_files_keeps_the_old_version_and_does_not_restart() {
    let r = rig("install");
    pack(&r, "p2", "0.1.0-alpha.2", "pc", 4, true);
    let cx = cx_with_admin_store("upd-install");
    let mut a = app(&r, &cx);
    let (initrd_before, data_before) = (boot_initrd(&r), std::fs::read(r.data.join("home/amy/a.txt")).unwrap());
    press(&mut a, &cx, 'y');
    assert!(a.status().is_some_and(|s| !s.1) && status(&a).contains("nothing to confirm"), "{}", status(&a));
    press(&mut a, &cx, 'i');
    assert!(status(&a).contains("Press y"), "{}", status(&a));
    assert_eq!(boot_initrd(&r), initrd_before, "asking writes nothing");
    press(&mut a, &cx, 'n');
    assert_eq!(boot_initrd(&r), initrd_before);
    press(&mut a, &cx, 'i');
    press(&mut a, &cx, 'y');
    settle(&mut a, &cx);
    assert!(a.status().is_some_and(|s| s.1) && status(&a).contains("Restart when you are ready"), "{}", status(&a));
    assert_ne!(boot_initrd(&r), initrd_before);
    assert_eq!(std::fs::read(r.data.join("home/amy/a.txt")).unwrap(), data_before);
    let g = draw(&mut a, &cx, 1280, 720);
    let _ = g;
    let v = a.upd.versions.clone().unwrap();
    assert_eq!((v.on_disk.as_str(), v.previous.as_deref()), ("0.1.0-alpha.2", Some("0.1.0-alpha.1")));
    assert!(cx.audit.recent(20).iter().any(|l| l.contains("system.update.install")));
}

#[test]
fn another_variant_an_older_version_or_no_signature_show_the_reason_before_the_yes_and_a_bad_signature_is_refused() {
    let r = rig("confirm");
    pack(&r, "other", "0.1.0-alpha.2", "mac", 4, true);
    pack(&r, "old", "0.0.5", "pc", 4, true);
    pack(&r, "plain", "0.1.0-alpha.3", "pc", 4, false);
    let bad = pack(&r, "bad", "0.1.0-alpha.4", "pc", 4, true);
    std::fs::write(bad.join(uc::SIG_FILE), "ab".repeat(64)).unwrap();
    let cx = cx_with_admin_store("upd-confirm");
    let mut a = app(&r, &cx);
    press(&mut a, &cx, 's');
    assert_eq!(a.upd.found.len(), 4);
    let before = boot_initrd(&r);
    let mut seen = Vec::new();
    for i in 0..4 {
        a.upd.sel = i;
        a.upd_preview();
        let name = a.upd.found[i].dir.file_name().unwrap().to_string_lossy().into_owned();
        press(&mut a, &cx, 'i');
        seen.push((name.clone(), status(&a)));
        press(&mut a, &cx, 'n');
    }
    let get = |n: &str| seen.iter().find(|(k, _)| k == n).unwrap().1.clone();
    assert!(get("other").contains("needs your yes") && get("other").contains("mac"), "{}", get("other"));
    assert!(get("old").contains("older"), "{}", get("old"));
    assert!(get("plain").contains("no signature"), "{}", get("plain"));
    assert!(get("bad").contains("cannot be installed") && get("bad").contains("signature"), "{}", get("bad"));
    assert_eq!(boot_initrd(&r), before);
}

#[test]
fn rollback_asks_first_and_puts_the_kept_version_back() {
    let r = rig("rollback");
    pack(&r, "p2", "0.1.0-alpha.2", "pc", 4, true);
    let cx = cx_with_admin_store("upd-rollback");
    let mut a = app(&r, &cx);
    let original = boot_initrd(&r);
    press(&mut a, &cx, 'r');
    assert!(a.status().is_some_and(|s| !s.1) && status(&a).contains("No previous version"), "{}", status(&a));
    press(&mut a, &cx, 'i');
    press(&mut a, &cx, 'y');
    settle(&mut a, &cx);
    assert_ne!(boot_initrd(&r), original);
    press(&mut a, &cx, 'r');
    assert!(status(&a).contains("Roll back to IYAKKI 0.1.0-alpha.1") && status(&a).contains("nothing restarts by itself"), "{}", status(&a));
    press(&mut a, &cx, 'n');
    assert_ne!(boot_initrd(&r), original, "asking changes nothing");
    press(&mut a, &cx, 'r');
    press(&mut a, &cx, 'y');
    settle(&mut a, &cx);
    assert!(a.status().is_some_and(|s| s.1), "{}", status(&a));
    assert_eq!(boot_initrd(&r), original);
    let _ = draw(&mut a, &cx, 1280, 720);
    assert!(cx.audit.recent(30).iter().any(|l| l.contains("system.update.rollback")));
    assert_eq!(a.upd.versions.as_ref().unwrap().previous.as_deref(), Some("0.1.0-alpha.2"));
}

#[test]
fn the_first_start_after_an_update_is_checked_and_a_fall_back_is_reported_on_the_overview() {
    let r = rig("boot");
    pack(&r, "p2", "0.1.0-alpha.2", "pc", 4, true);
    let cx = cx_with_admin_store("upd-boot");
    let mut a = app(&r, &cx);
    press(&mut a, &cx, 'i');
    press(&mut a, &cx, 'y');
    settle(&mut a, &cx);
    // the next start still runs the old version: the boot menu fell back
    let mut t = uc::read_trial(&r.data).unwrap();
    t.boot_id = "boot-0".into();
    std::fs::write(r.data.join(uc::TRIAL_FILE), serde_json::to_vec(&t).unwrap()).unwrap();
    assert!(!a.attention(&cx).iter().any(|x| x.tab == Tab::Updates));
    a.upd.loaded = false;
    press(&mut a, &cx, 's');
    assert!(a.upd.notice.as_deref().is_some_and(|n| n.contains("fell back")), "{:?}", a.upd.notice);
    let at = a.attention(&cx).into_iter().find(|x| x.tab == Tab::Updates).expect("listed");
    assert!(at.title.contains("did not start"), "{}", at.title);
    // an update that did not finish is listed too
    std::fs::create_dir_all(r.data.join("boot-next")).unwrap();
    let mk = up::Marker { from: "a".into(), to: "b".into(), started: 1, prev_id: "x".into(), files: vec![] };
    std::fs::write(r.data.join("boot-next/UPDATING.json"), serde_json::to_vec(&mk).unwrap()).unwrap();
    assert!(a.attention(&cx).iter().any(|x| x.tab == Tab::Updates && x.title.contains("did not finish")));
}

#[test]
fn the_page_fits_at_four_sizes_in_every_state() {
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
        let r = rig(&format!("geo-{w}"));
        let cx = cx_with_admin_store(&format!("upd-geo-{w}"));
        let mut a = app(&r, &cx);
        let check = |g: &Gfx, what: &str| {
            let buttons: Vec<_> = g.hits.iter().filter(|x| matches!(x.ev, Event::Key(Key::Char(_))) && x.rect.h < 90.0).collect();
            assert!(buttons.len() >= 4, "{w}x{h} {what}: buttons {}", buttons.len());
            for (i, b) in buttons.iter().enumerate() {
                assert!(b.rect.x >= 0.0 && b.rect.x + b.rect.w <= w as f32 + 0.5 && b.rect.y >= 0.0 && b.rect.y + b.rect.h <= h as f32 + 0.5, "{w}x{h} {what}: {} {:?}", b.label, b.rect);
                let o = |p: R, q: R| p.x < q.x + q.w - 0.5 && q.x < p.x + p.w - 0.5 && p.y < q.y + q.h - 0.5 && q.y < p.y + p.h - 0.5;
                assert!(buttons[i + 1..].iter().all(|n| !o(b.rect, n.rect)), "{w}x{h} {what}: {} overlaps another button", b.label);
            }
        };
        check(&draw(&mut a, &cx, w, h), "nothing found");
        pack(&r, "p2", "0.1.0-alpha.2", "mac", 4, false);
        pack(&r, "p3", "0.1.0-alpha.3", "pc", 4, true);
        press(&mut a, &cx, 's');
        check(&draw(&mut a, &cx, w, h), "two packs");
        press(&mut a, &cx, 'i');
        check(&draw(&mut a, &cx, w, h), "asking");
        press(&mut a, &cx, 'y');
        check(&draw(&mut a, &cx, w, h), "running");
        settle(&mut a, &cx);
        check(&draw(&mut a, &cx, w, h), "installed");
        press(&mut a, &cx, 'r');
        check(&draw(&mut a, &cx, w, h), "asking to roll back");
    }
}
