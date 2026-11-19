//! Settings -> System update: who sees the page, finding a pack, the plain preview, install with a yes, no install without one, a wrong
//! signature, rollback, the note after a start, the Overview line, and the page at four sizes with every state. The boot partition is a
//! folder with a size limit and the packs are made in a temp folder: no drive, no mount, no restart.
use iyakki_agent::facets_h::RealServices as _;

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
    let mut a = SettingsApp::new(cx).with_updates(c.into_facet(), vec![r.stick.clone()], "boot-1");
    a.go_to(Tab::Updates);
    a
}

fn press(a: &mut SettingsApp, cx: &AppCx, c: char) {
    a.handle(&Event::Key(Key::Char(c)), cx);
}

fn settle(a: &mut SettingsApp, cx: &AppCx) {
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(60) {
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
    let s = crate::ui::scale_for(w, h);
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

// ---------------------------------------------------------------------------------------- the update check (F-UPD-04)

use iyakki_agent::store::updatecheck as chk;

fn check_cfg(cx: &AppCx) -> chk::CheckSettings {
    cx.settings().lock().unwrap().get().update_check.clone()
}

fn report() -> chk::Report {
    chk::Report {
        checked_at: iyakki_clock::now_unix() - 3 * 3600,
        updates: vec![
            chk::Update { kind: chk::Kind::System, id: "system".into(), title: "IYAKKI".into(), from: "0.1.0-alpha.1".into(), to: "0.1.0-alpha.3".into(), source: "System update".into(), new_permissions: vec![], note: "Fixes the sound.".into(), driver: false },
            chk::Update { kind: chk::Kind::App, id: "skill:notes".into(), title: "Meeting notes".into(), from: "1.0.0".into(), to: "1.2.0".into(), source: "Stick".into(), new_permissions: vec!["reach example.com".into()], note: "asks for permissions the installed version did not".into(), driver: false },
        ],
        errors: vec![("Mirror".into(), "could not reach mirror.example: offline".into())],
        asked: vec![],
    }
}

#[test]
fn the_page_shows_the_last_check_what_it_found_and_that_nothing_is_downloaded() {
    let r = rig("chk-show");
    let cx = cx_with_admin_store("upd-chk-show");
    let mut a = app(&r, &cx);
    let none = a.upd_check_lines(&cx).into_iter().map(|l| l.0).collect::<Vec<_>>().join("\n");
    assert!(none.contains("every day") && none.contains("Not checked yet") && none.contains("cannot tell a metered connection"), "{none}");
    report().save(cx.data_dir()).unwrap();
    a.upd.check = None;
    let text = a.upd_check_lines(&cx).into_iter().map(|l| l.0).collect::<Vec<_>>().join("\n");
    for want in ["3 hours ago", "2 update(s) available", "Nothing is downloaded or installed until you do it yourself", "System: IYAKKI 0.1.0-alpha.1 -> 0.1.0-alpha.3", "Fixes the sound.", "App: Meeting notes 1.0.0 -> 1.2.0 (Stick)", "Asks for new permissions: reach example.com", "Mirror: could not reach"] {
        assert!(text.contains(want), "{want} in:\n{text}");
    }
    // the buttons are on the page for mouse and touch
    let g = draw(&mut a, &cx, 1280, 720);
    for k in ['c', 'm', 'u'] {
        assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char(k))), "button {k}");
    }
}

#[test]
fn how_often_cycles_and_is_saved_and_audited_and_off_refuses_to_check() {
    let r = rig("chk-mode");
    let cx = cx_with_admin_store("upd-chk-mode");
    let mut a = app(&r, &cx);
    assert_eq!(check_cfg(&cx).mode, chk::Mode::Daily);
    press(&mut a, &cx, 'm');
    assert_eq!(check_cfg(&cx).mode, chk::Mode::Manual);
    press(&mut a, &cx, 'm');
    assert_eq!(check_cfg(&cx).mode, chk::Mode::Off);
    press(&mut a, &cx, 'c');
    assert!(a.status().is_some_and(|s| !s.1 && s.0.contains("is off")), "{:?}", a.status());
    assert!(!cx.data_dir().join(chk::FILE).exists(), "nothing was checked");
    press(&mut a, &cx, 'm');
    assert_eq!(check_cfg(&cx).mode, chk::Mode::Daily);
}

#[test]
fn check_now_runs_in_the_background_keeps_the_answer_and_installs_nothing() {
    let r = rig("chk-now");
    let cx = cx_with_admin_store("upd-chk-now");
    let mut a = app(&r, &cx);
    let boot_before = boot_initrd(&r);
    press(&mut a, &cx, 'c');
    assert!(a.is_busy(), "a background job, the page stays usable");
    settle(&mut a, &cx);
    assert!(status(&a).contains("up to date") || status(&a).contains("could not be reached"), "{}", status(&a));
    let rep = chk::Report::load(cx.data_dir()).unwrap();
    assert!(rep.checked_at > 0 && rep.updates.is_empty());
    assert_eq!(boot_initrd(&r), boot_before, "the boot files are untouched");
}

#[test]
fn the_update_list_address_is_checked_before_it_is_saved_and_only_administrators_set_it() {
    let r = rig("chk-url");
    let cx = cx_with_admin_store("upd-chk-url");
    let mut a = app(&r, &cx);
    press(&mut a, &cx, 'u');
    assert!(a.modal.is_some(), "the form opens");
    let mut f = Form::new("x").text("Address", "", "");
    f.set_value(0, "ftp://nope");
    assert!(a.upd_source_submit(&cx, &f).unwrap_err().contains("https://"));
    assert_eq!(check_cfg(&cx).system_url, "");
    f.set_value(0, " https://updates.example/iyakki/index.json ");
    assert!(a.upd_source_submit(&cx, &f).unwrap().contains("saved"));
    assert_eq!(check_cfg(&cx).system_url, "https://updates.example/iyakki/index.json");
    f.set_value(0, "");
    assert!(a.upd_source_submit(&cx, &f).unwrap().contains("only apps"));
    // an ordinary user changes nothing
    let mut ucx = cx_with_admin_store("upd-chk-url-user");
    ucx.session.user = "amy".into();
    ucx.session.role = iyakki_sec::policy::Role::User;
    ucx.session.policy = iyakki_sec::policy::Role::User.template();
    let mut b = app(&r, &ucx);
    b.upd_check_key(&Key::Char('m'), &ucx);
    assert_eq!(check_cfg(&ucx).mode, chk::Mode::Daily);
}

#[test]
fn the_overview_says_how_many_updates_the_last_check_found() {
    let r = rig("chk-over");
    let cx = cx_with_admin_store("upd-chk-over");
    let a = app(&r, &cx);
    assert!(!a.attention(&cx).iter().any(|x| x.title.contains("available")));
    report().save(cx.data_dir()).unwrap();
    let n = a.attention(&cx).into_iter().find(|x| x.title.contains("available")).expect("an Overview line");
    assert_eq!((n.tab, n.title.as_str()), (Tab::Updates, "2 updates available"));
}

#[test]
fn use_a_store_sets_the_update_list_to_that_stores_announcement_without_typing_a_url() {
    let r = rig("chk-store");
    let cx = cx_with_admin_store("upd-chk-store");
    let mut a = app(&r, &cx);
    // no store with a checked key yet
    a.upd_check_key(&Key::Char('t'), &cx);
    assert_eq!(check_cfg(&cx).system_url, "");
    // two stores with their keys pinned, one without
    let svc = cx.services.clone();
    let store = iyakki_agent::store::Store::for_services(svc.real());
    let mut c = store.config();
    let key = "ab".repeat(32);
    c.add_pinned("Home store", "http://192.0.2.7:30580/catalog.json", &key).unwrap();
    c.add_pinned("Work store", "https://work.example/store/catalog.json", &key).unwrap();
    c.add("Loose", iyakki_shared::store_catalog::SourceKind::Url, "https://loose.example/catalog.json").unwrap();
    c.save(store.dir()).unwrap();
    a.upd_check_key(&Key::Char('t'), &cx);
    assert_eq!(check_cfg(&cx).system_url, "http://192.0.2.7:30580/v1/system.json");
    a.upd_check_key(&Key::Char('t'), &cx);
    assert_eq!(check_cfg(&cx).system_url, "https://work.example/store/v1/system.json");
    a.upd_check_key(&Key::Char('t'), &cx);
    assert_eq!(check_cfg(&cx).system_url, "http://192.0.2.7:30580/v1/system.json", "it goes round, and the store without a checked key is never offered");
    assert!(a.upd_check_lines(&cx).iter().any(|l| l.0.contains("announcement of Home store")));
    assert!(a.updates_buttons().iter().any(|b| b.0 == "Use a store"));
    // an ordinary user changes nothing
    let mut ucx = cx_with_admin_store("upd-chk-store-user");
    ucx.session.user = "amy".into();
    ucx.session.role = iyakki_sec::policy::Role::User;
    ucx.session.policy = iyakki_sec::policy::Role::User.template();
    let mut b = app(&r, &ucx);
    b.upd_check_key(&Key::Char('t'), &ucx);
    assert_eq!(check_cfg(&ucx).system_url, "");
}

#[test]
fn review_and_apply_shows_the_components_line_and_the_review_and_installs_nothing_until_the_yes() {
    use crate::apps::settings::updates_page::UpdAsk;
    use iyakki_agent::store::compapply::{review, save_check, StoreLine};
    let r = rig("comp");
    let cx = cx_with_admin_store("upd-comp");
    let mut a = app(&r, &cx);
    assert!(a.updates_buttons().iter().any(|b| b.0 == "Review and apply"));
    // the line from the last check
    assert!(!a.upd_check_lines(&cx).iter().any(|l| l.0.starts_with("Components from")));
    save_check(cx.data_dir(), &[StoreLine { store: "Lab store".into(), newer: 2, checked_at: 1, error: String::new() }]);
    a.upd.comp_lines = None;
    assert!(a.upd_check_lines(&cx).iter().any(|l| l.0 == "Components from Lab store: 2 newer."));
    // a review is on screen: the same text the chat card shows, and a plain instruction
    let plan = iyakki_agent::store::components::Plan { store: "Lab store".into(), channel: "stable".into(), ..Default::default() };
    let rev = review(&plan, None);
    *a.upd.comp_review.lock().unwrap() = Some(iyakki_agent_api::ComponentReview { store: "Lab store".into(), channel: "stable".into(), review: rev });
    a.upd.ask = UpdAsk::Components;
    let lines = a.upd_check_lines(&cx);
    assert!(lines.iter().any(|l| l.0.contains("Install system components from Lab store")) && lines.iter().any(|l| l.0.contains("Press y to approve")));
    // n cancels; nothing was recorded or written
    press(&mut a, &cx, 'n');
    assert_eq!(a.upd.ask, UpdAsk::None);
    assert!(!cx.data_dir().join("components-installed.json").exists() && !cx.data_dir().join("component-work").exists());
}

#[test]
fn the_last_component_install_result_stays_on_the_page_with_its_time_until_the_next_action() {
    use iyakki_agent::store::compapply::{classify, save_result};
    let r = rig("comp-result");
    let cx = cx_with_admin_store("upd-comp-result");
    let mut a = app(&r, &cx);
    let now = iyakki_clock::now_unix();
    assert!(!a.upd_check_lines(&cx).iter().any(|l| l.0.starts_with("Last system component install")));
    // a refusal is on the page, with the reason, and says nothing was changed
    save_result(cx.data_dir(), &classify("Lab store", "stable", &Err("not applied: not enough room to prepare the update: 900 MB needed in the work folder, 100 MB free".into()), false, now - 125));
    a.upd.comp_result = None;
    let lines = a.upd_check_lines(&cx);
    let head = lines.iter().position(|l| l.0.starts_with("Last system component install (2 minutes ago): Refused, nothing was changed.")).expect("the headline");
    assert!(lines[head + 1].0.contains("not enough room to prepare the update"), "{:?}", lines[head + 1]);
    assert_eq!(lines[head].1, crate::fx::AMBER);
    // it is still there on a later look (not a message that fades), and a failure that was rolled back is in the error colour
    a.upd.comp_result = None;
    assert!(a.upd_check_lines(&cx).iter().any(|l| l.0.contains("Refused, nothing was changed")));
    save_result(cx.data_dir(), &classify("Lab store", "stable", &Err("a check after writing failed (x); the previous version was put back: ok Nothing is recorded as installed.".into()), false, now));
    a.upd.comp_result = None;
    let lines = a.upd_check_lines(&cx);
    let head = lines.iter().find(|l| l.0.starts_with("Last system component install (just now): Failed and rolled back")).expect("rolled back");
    assert_eq!(head.1, ERR);
    // success is shown as installed
    save_result(cx.data_dir(), &classify("Lab store", "stable", &Ok(iyakki_agent::store::compapply::Applied { lines: vec!["Installed: kernel 6.12.5.".into()], restart_machine: true, restart_core: false, installed: Default::default(), boot_message: None }), false, now));
    a.upd.comp_result = None;
    let lines = a.upd_check_lines(&cx);
    assert!(lines.iter().any(|l| l.0.contains("Installed.")) && lines.iter().any(|l| l.0 == "Installed: kernel 6.12.5."));
}

#[test]
fn the_channel_button_cycles_stable_beta_nightly_is_kept_and_clears_a_review_of_the_other_channel() {
    use crate::apps::settings::updates_page::UpdAsk;
    let r = rig("comp-channel");
    let cx = cx_with_admin_store("upd-comp-channel");
    let mut a = app(&r, &cx);
    assert!(a.updates_buttons().iter().any(|b| b.0 == "Channel"));
    assert!(a.upd_check_lines(&cx).iter().any(|l| l.0.starts_with("Component channel on this computer: stable")));
    let plan = iyakki_agent::store::components::Plan { store: "Lab store".into(), channel: "stable".into(), ..Default::default() };
    *a.upd.comp_review.lock().unwrap() = Some(iyakki_agent_api::ComponentReview { store: "Lab store".into(), channel: "stable".into(), review: iyakki_agent::store::compapply::review(&plan, None) });
    a.upd.ask = UpdAsk::Components;
    press(&mut a, &cx, 'g');
    assert_eq!(iyakki_agent::store::compapply::channel(cx.data_dir()), "beta");
    assert_eq!(a.upd.ask, UpdAsk::None);
    assert!(a.upd.comp_review.lock().unwrap().is_none(), "a review made for stable is not approved for beta");
    assert!(a.upd_check_lines(&cx).iter().any(|l| l.0.starts_with("Component channel on this computer: beta")));
    press(&mut a, &cx, 'g');
    assert_eq!(iyakki_agent::store::compapply::channel(cx.data_dir()), "nightly");
    press(&mut a, &cx, 'g');
    assert_eq!(iyakki_agent::store::compapply::channel(cx.data_dir()), "stable");
    // a person who is not an administrator cannot change it
    let mut ucx = cx_with_admin_store("upd-comp-channel-user");
    ucx.session.user = "amy".into();
    ucx.session.role = iyakki_sec::policy::Role::User;
    ucx.session.policy = iyakki_sec::policy::Role::User.template();
    let mut b = app(&r, &ucx);
    press(&mut b, &ucx, 'g');
    assert_eq!(iyakki_agent::store::compapply::channel(ucx.data_dir()), "stable");
}

// ---- F-CORE-24: the pending setup wizards on the System update page

fn queue_wizard(cx: &AppCx, user: &str) {
    let dir = cx.data_dir().join("wizards");
    std::fs::create_dir_all(&dir).unwrap();
    let body = serde_json::json!({"schema": 1, "pending": [{"app": "notes", "wizard": "setup", "title": "Set up Notes", "trigger": "on_install", "from_version": null, "queued_at": 5}]});
    std::fs::write(dir.join(format!("{user}.json")), body.to_string()).unwrap();
}

#[test]
fn the_page_lists_pending_setup_wizards_the_overview_mentions_them_and_a_opens_the_assistant() {
    let r = rig("wiz-page");
    let cx = cx_with_admin_store("wiz-page");
    let mut a = app(&r, &cx);
    a.go_to(Tab::Updates);
    let g = draw(&mut a, &cx, 1280, 800);
    assert!(!g.text_log.iter().any(|(_, _, t)| t.contains("Setup steps waiting")), "nothing queued: no section");
    assert!(!a.attention(&cx).iter().any(|x| x.title.contains("setup wizard")));
    queue_wizard(&cx, &cx.session.user);
    let g = draw(&mut a, &cx, 1280, 800);
    let all: Vec<String> = g.text_log.iter().map(|(_, _, t)| t.clone()).collect();
    assert!(all.iter().any(|t| t == "Setup steps waiting") && all.iter().any(|t| t.contains("notes - Set up Notes (on_install)")), "{all:?}");
    assert!(a.attention(&cx).iter().any(|x| x.tab == Tab::Updates && x.title == "1 setup wizard waiting"));
    a.handle(&Event::Key(Key::Char('a')), &cx);
    assert_eq!(a.take_request(), Some(("chat".into(), String::new())), "the assistant opens; it holds the steps and asks before it runs them");
}

/// A signed system update file for `version`/`variant` on the "stick": the folder of files, `release.json`, the packed `.tgz` and its `.sig`.
fn bundle_file(r: &Rig, version: &str, variant: &str, sign_with_the_release_key: bool) -> std::path::PathBuf {
    let src = r.stick.join(format!("src-{version}"));
    put(&src, "boot/bzImage", &bytes((1 << 20) + 300, 21));
    put(&src, "boot/initrd", &bytes(2 << 20, 22));
    put(&src, "boot/grub/grub.cfg", MENU.replace("quiet", "quiet splash").as_bytes());
    let files: Vec<serde_json::Value> = ["boot/bzImage", "boot/initrd", "boot/grub/grub.cfg"].iter().map(|p| serde_json::json!({"path": p, "size": std::fs::metadata(src.join(p)).unwrap().len(), "sha256": iyakki_agent::devmode::files::sha256_file(&src.join(p)).unwrap()})).collect();
    put(&src, "release.json", serde_json::json!({"schema": 1, "version": version, "variant": variant, "min_base": "", "notes": "Sound fixes.", "files": files}).to_string().as_bytes());
    let out = r.stick.join(format!("iyakki-{version}.tgz"));
    iyakki_agent::sysupdate::pack(&src, &out).unwrap();
    let signer = if sign_with_the_release_key { key!() } else { iyakki_agent::pkg::signing_key_from_hex(&"07".repeat(32)).unwrap() };
    std::fs::write(format!("{}.sig", out.display()), iyakki_agent::sysupdate::sign(&out, &signer).unwrap()).unwrap();
    out
}

fn open_file(a: &mut SettingsApp, cx: &AppCx, path: &std::path::Path) -> Result<String, String> {
    let mut f = Form::new("x").text("File", "", "");
    f.set_value(0, &path.display().to_string());
    a.upd_bundle_submit(&f, cx)
}

#[test]
fn a_signed_update_file_is_opened_from_the_page_listed_and_selected_and_nothing_is_installed() {
    let r = rig("bundle-ok");
    let cx = cx_with_admin_store("upd-bundle-ok");
    let mut a = app(&r, &cx);
    let before = boot_initrd(&r);
    press(&mut a, &cx, 'b');
    assert!(a.modal.is_some(), "the key opens the form");
    a.modal = None;
    let file = bundle_file(&r, "0.1.0-alpha.2", "pc", true);
    let said = open_file(&mut a, &cx, &file).unwrap();
    assert!(said.contains("signature is good") && said.contains("0.1.0-alpha.2") && said.contains("nothing is installed"), "{said}");
    assert_eq!(a.upd.found.len(), 1, "the page lists the opened update");
    assert!(a.upd.found[0].dir.starts_with(iyakki_agent::sysupdate::staged_root(&r.data)));
    let pv = a.upd.preview.clone().unwrap().unwrap();
    assert_eq!((pv.from.as_str(), pv.to.as_str()), ("0.1.0-alpha.1", "0.1.0-alpha.2"));
    assert_eq!(boot_initrd(&r), before, "opening writes nothing to the boot partition");
    assert!(cx.audit.recent(10).iter().any(|l| l.contains("system.update.bundle") && l.contains("0.1.0-alpha.2")));
    // the usual install flow takes it from here: Install asks first
    press(&mut a, &cx, 'i');
    assert_eq!(a.upd.ask, crate::apps::settings::updates_page::UpdAsk::Install);
    press(&mut a, &cx, 'n');
    assert_eq!(boot_initrd(&r), before);
    // the button is on the page at the five sizes
    for (w, h) in [(360, 740), (800, 480), (1280, 720), (1920, 1080), (2560, 1600)] {
        let g = draw(&mut a, &cx, w, h);
        assert!(g.hits.iter().any(|x| x.ev == Event::Key(Key::Char('b'))), "{w}x{h}: no Open update file button");
    }
}

#[test]
fn an_update_file_with_a_wrong_signature_no_sig_file_the_wrong_edition_or_an_ordinary_user_is_refused_and_nothing_is_listed() {
    let r = rig("bundle-no");
    let cx = cx_with_admin_store("upd-bundle-no");
    let mut a = app(&r, &cx);
    let wrong_key = bundle_file(&r, "0.1.0-alpha.3", "pc", false);
    assert!(open_file(&mut a, &cx, &wrong_key).unwrap_err().contains("Nothing was unpacked"));
    let no_sig = bundle_file(&r, "0.1.0-alpha.4", "pc", true);
    std::fs::remove_file(format!("{}.sig", no_sig.display())).unwrap();
    assert!(open_file(&mut a, &cx, &no_sig).unwrap_err().contains(".sig"));
    let other_edition = bundle_file(&r, "0.1.0-alpha.5", "mac", true);
    assert!(open_file(&mut a, &cx, &other_edition).unwrap_err().contains("edition"));
    assert!(open_file(&mut a, &cx, &r.stick.join("missing.tgz")).is_err());
    assert!(a.upd.found.is_empty(), "nothing was listed: {:?}", a.upd.found.len());
    // an ordinary user cannot open one
    let mut ucx = cx_with_admin_store("upd-bundle-user");
    ucx.session.user = "amy".into();
    ucx.session.role = iyakki_sec::policy::Role::User;
    ucx.session.policy = iyakki_sec::policy::Role::User.template();
    let good = bundle_file(&r, "0.1.0-alpha.6", "pc", true);
    let mut b = app(&r, &ucx);
    assert!(open_file(&mut b, &ucx, &good).unwrap_err().contains("administrator"));
    assert!(!iyakki_agent::sysupdate::staged_root(&r.data).join("x").exists());
}

// ---- F-CORE-24: the setup wizard of an app on a screen

mod wizard_screen {
    use super::*;
    use iyakki_agent::wizardrun::{Env, Run};
    use iyakki_agent::wizardtools::Backend;
    use iyakki_core_api::wizard::WizardSpec;
    use serde_json::{json, Value};
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    struct FakeEnv {
        log: Mutex<Vec<String>>,
        fail_check: bool,
    }
    impl Env for FakeEnv {
        fn check(&self, check: &str, param: &str) -> Result<(), String> {
            self.log.lock().unwrap().push(format!("check {check} {param}"));
            if self.fail_check {
                Err("cannot reach the server".into())
            } else {
                Ok(())
            }
        }
        fn migrate(&self, r: &str, _: Option<&str>, _: Option<&str>, a: &BTreeMap<String, Value>) -> Result<String, String> {
            self.log.lock().unwrap().push(format!("migrate {r} keys={}", a.keys().cloned().collect::<Vec<_>>().join(",")));
            Ok("moved".into())
        }
    }

    struct Fake {
        spec: WizardSpec,
        pending: Mutex<Vec<(String, String, String, String)>>,
        run: Arc<Mutex<Option<(String, Run)>>>,
        env: Arc<FakeEnv>,
        audit: Mutex<Vec<String>>,
    }
    impl Backend for Fake {
        fn installed(&self) -> Vec<(String, String, WizardSpec)> {
            vec![("notes".into(), "1.0.0".into(), self.spec.clone())]
        }
        fn pending(&self) -> Vec<(String, String, String, String)> {
            self.pending.lock().unwrap().clone()
        }
        fn clear_pending(&self, app: &str, wizard: &str) {
            self.pending.lock().unwrap().retain(|p| !(p.0 == app && p.1 == wizard));
        }
        fn env(&self) -> Arc<dyn Env> {
            self.env.clone()
        }
        fn run(&self) -> Arc<Mutex<Option<(String, Run)>>> {
            self.run.clone()
        }
        fn audit(&self, what: &str, detail: &str) {
            self.audit.lock().unwrap().push(format!("{what} {detail}"));
        }
    }

    fn fake(fail_check: bool) -> Arc<Fake> {
        let spec: WizardSpec = serde_json::from_value(json!({
            "id": "setup", "title": "Setup", "trigger": {"on": "on_install"},
            "steps": [
                {"kind": "form", "id": "who", "title": "Who", "fields": [
                    {"id": "name", "label": "Name", "kind": "text", "required": true, "rules": [{"rule": "min_len", "value": 3}]},
                    {"id": "token", "label": "Token", "kind": "secret"},
                    {"id": "mode", "label": "Mode", "kind": "choice", "required": true, "choices": [{"value": "fast", "label": "Fast"}, {"value": "safe", "label": "Safe"}], "default": "fast"},
                    {"id": "sync", "label": "Sync", "kind": "toggle"}]},
                {"kind": "check", "id": "net", "title": "Server reachable", "check": "net.reach", "param": "example.com:443"},
                {"kind": "migration", "id": "mig", "title": "Move data", "migration": "m1"},
                {"kind": "summary", "id": "done", "title": "Ready", "text": "All set."}]
        }))
        .unwrap();
        Arc::new(Fake { spec, pending: Mutex::new(vec![("notes".into(), "setup".into(), "on_install".into(), "Set up Notes".into())]), run: Arc::new(Mutex::new(None)), env: Arc::new(FakeEnv { log: Mutex::new(vec![]), fail_check }), audit: Mutex::new(vec![]) })
    }

    fn with(tag: &str, be: &Arc<Fake>) -> (SettingsApp, AppCx) {
        let r = rig(tag);
        let cx = cx_with_admin_store(tag);
        let mut a = app(&r, &cx);
        a.go_to(Tab::Updates);
        queue_wizard(&cx, &cx.session.user);
        a.upd.wiz_be = Some(be.clone());
        (a, cx)
    }

    fn form_title(a: &SettingsApp) -> Option<String> {
        match &a.modal {
            Some(Modal::Form(FormKind::BundleWizard, f)) => Some(f.title.clone()),
            _ => None,
        }
    }

    fn type_in(a: &mut SettingsApp, cx: &AppCx, s: &str) {
        for c in s.chars() {
            a.handle(&Event::Key(Key::Char(c)), cx);
        }
    }

    #[test]
    fn w_opens_the_first_question_as_a_form_a_refused_answer_stays_with_the_reason_and_the_secret_is_typed_here() {
        let be = fake(false);
        let (mut a, cx) = with("wiz-s1", &be);
        press(&mut a, &cx, 'w');
        assert_eq!(form_title(&a).as_deref(), Some("Setup of notes: Who"));
        // too short a name: the form stays, with the field and the reason
        type_in(&mut a, &cx, "ab");
        a.handle(&Event::Key(Key::Ctrl('s')), &cx);
        match &a.modal {
            Some(Modal::Form(FormKind::BundleWizard, f)) => assert_eq!(f.error.as_deref(), Some("Name: at least 3 characters"), "{:?}", f.error),
            other => panic!("the form should stay open: {}", other.is_some()),
        }
        assert!(be.env.log.lock().unwrap().is_empty(), "nothing ran");
        // fix the name, type the secret, leave the choice and the toggle as they are
        type_in(&mut a, &cx, "y");
        a.handle(&Event::Key(Key::Enter), &cx);
        type_in(&mut a, &cx, "s3cret-token");
        a.handle(&Event::Key(Key::Ctrl('s')), &cx);
        // the next step runs checks: the card lists them and nothing has run yet
        match &a.modal {
            Some(Modal::Confirm(ConfirmKind::WizardRun, t)) => assert!(t.contains("check net.reach example.com:443") && t.contains("migration m1") && t.contains("Press y"), "{t}"),
            _ => panic!("the card of the steps that will run was expected"),
        }
        assert!(be.env.log.lock().unwrap().is_empty(), "nothing ran before y");
        assert_eq!(be.pending.lock().unwrap().len(), 1, "it stays waiting until it is over");
        // y runs them on the worker thread, then the summary asks for y
        press(&mut a, &cx, 'y');
        settle(&mut a, &cx);
        assert_eq!(*be.env.log.lock().unwrap(), vec!["check net.reach example.com:443".to_string(), "migrate m1 keys=mode,name,sync".to_string()], "the secret never reached the migration");
        match &a.modal {
            Some(Modal::Confirm(ConfirmKind::WizardFinish, t)) => assert!(t.contains("All set.")),
            _ => panic!("the summary was expected"),
        }
        press(&mut a, &cx, 'y');
        assert!(a.modal.is_none());
        assert!(status(&a).starts_with("The setup of notes is finished."), "{}", status(&a));
        assert!(be.pending.lock().unwrap().is_empty(), "it is off the waiting list now");
        let audit = be.audit.lock().unwrap().join("\n");
        assert!(audit.contains("wizard.start notes/setup") && audit.contains("wizard.answer") && audit.contains("wizard.run") && audit.contains("wizard.finish"), "{audit}");
        assert!(!audit.contains("s3cret-token") && !status(&a).contains("s3cret-token"), "the secret is in no line");
    }

    #[test]
    fn esc_cancels_the_run_and_it_stays_waiting_and_a_failed_check_says_why_and_can_be_tried_again() {
        let be = fake(false);
        let (mut a, cx) = with("wiz-s2", &be);
        press(&mut a, &cx, 'w');
        a.handle(&Event::Key(Key::Esc), &cx);
        assert!(a.modal.is_none() && status(&a).contains("cancelled") && status(&a).contains("stays waiting"), "{}", status(&a));
        assert_eq!(be.pending.lock().unwrap().len(), 1);
        // a check that fails
        let bad = fake(true);
        let (mut b, cx2) = with("wiz-s3", &bad);
        press(&mut b, &cx2, 'w');
        type_in(&mut b, &cx2, "amy");
        b.handle(&Event::Key(Key::Ctrl('s')), &cx2);
        press(&mut b, &cx2, 'y');
        settle(&mut b, &cx2);
        assert!(b.modal.is_none(), "no summary after a failure");
        assert!(status(&b).contains("stopped") && status(&b).contains("cannot reach the server") && status(&b).contains("press w"), "{}", status(&b));
        assert_eq!(bad.pending.lock().unwrap().len(), 1, "a failed setup stays waiting");
        assert_eq!(*bad.env.log.lock().unwrap(), vec!["check net.reach example.com:443".to_string()], "the move did not run after the failed check");
        // w tries again from the first question
        press(&mut b, &cx2, 'w');
        assert_eq!(form_title(&b).as_deref(), Some("Setup of notes: Who"));
    }

    #[test]
    fn a_wizard_started_in_the_chat_is_picked_up_by_w_and_the_other_way_round_and_a_is_the_assistant() {
        use iyakki_agent::wizardtools::screen;
        let be = fake(false);
        let (mut a, cx) = with("wiz-s4", &be);
        // started by the assistant's tool (the same call it makes), the page's waiting list is not what `w` needs now
        screen::open(&*be, "notes", "setup", None).unwrap();
        press(&mut a, &cx, 'w');
        assert_eq!(form_title(&a).as_deref(), Some("Setup of notes: Who"), "the open run is shown, not a second one started");
        type_in(&mut a, &cx, "amy");
        a.handle(&Event::Key(Key::Ctrl('s')), &cx);
        // the assistant sees the same run at the same step
        let seen = screen::shown(&*be).unwrap();
        assert!(matches!(seen.view, iyakki_shared::wizardrun::View::Working { .. }), "{:?}", seen.view);
        // `a` still goes to the assistant
        a.handle(&Event::Key(Key::Esc), &cx);
        press(&mut a, &cx, 'a');
        assert_eq!(a.take_request(), Some(("chat".into(), String::new())));
    }
}
