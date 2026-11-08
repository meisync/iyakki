//! Settings -> Shared folders: the form and its checks, start/stop/stop-all with a real server on 127.0.0.1, the folder rules, the
//! actions of the IDE and EDIT, and the page drawn at four sizes with a QR code. Servers listen on loopback only.

use std::io::{Read, Write};

use super::shares_page::{draft_of_form, share_form, ShareForm};
use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_agent::sharedfolder::{Access, Mode};

struct Offline;
impl iyakki_agent::iyakki_proxy::Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

/// These tests start real servers and read the process-wide count of running shares, so they take turns.
static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn serial() -> std::sync::MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|e| e.into_inner())
}

fn admin(tag: &str) -> AppCx {
    let mut cx = cx_with_admin_store(tag);
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, Arc::new(Offline));
    cx.services.on_login("root", "rootpass12", true);
    // the account's shareable place is a folder of the test, never the real home folder
    let home = cx.services.dir().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let home = std::fs::canonicalize(home).unwrap();
    let allow: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(move |p| std::path::Path::new(p).starts_with(&home));
    iyakki_agent::sharedfolder::for_user(cx.services.dir(), "root", Arc::new(|_, _| {}), allow, iyakki_agent::sharedfolder::Scope::Admin);
    cx
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::shell::scale_for(w, h);
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
    g
}

fn goto(a: &mut SettingsApp, cx: &AppCx, t: Tab) {
    for _ in 0..60 {
        if a.tab() == t {
            return;
        }
        key(a, cx, Key::Right);
    }
    panic!("no {t:?} page");
}

fn status(a: &SettingsApp) -> String {
    a.status.as_ref().map(|s| s.0.clone()).unwrap_or_default()
}

/// A folder inside the account's home, with a file in it.
fn folder(cx: &AppCx, name: &str) -> String {
    let d = cx.services.dir().join("home").join(name);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.join("hello.txt"), "hi there").unwrap();
    d.to_string_lossy().into_owned()
}

/// The port refuses connections (within 3 s: a child process forked while the listener was open keeps it alive until its `exec`).
fn refused(port: u16) -> bool {
    let addr: std::net::SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
    loop {
        if std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_err() {
            return true;
        }
        if std::time::Instant::now() >= end {
            return false;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
}

fn free_port() -> u16 {
    iyakki_agent::testdir::free_port()
}

fn form_for(cx: &AppCx, name: &str, access: usize) -> Form {
    let d = iyakki_agent::sharedfolder::Draft { name: name.into(), folder: folder(cx, name), access: Access::Local, expiry_secs: 3600, max_upload_mb: 25, bind: "all".into(), port: free_port(), ..Default::default() };
    let mut f = share_form(&d, false);
    f.set_choice(3, access);
    f
}

fn get(port: u16, path: &str) -> String {
    let mut c = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
    c.set_read_timeout(Some(std::time::Duration::from_secs(10))).unwrap();
    c.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n").as_bytes()).unwrap();
    let mut s = String::new();
    let _ = c.read_to_string(&mut s);
    s
}

fn white(g: &Gfx) -> usize {
    g.rgba().chunks(4).filter(|p| p[0] == 255 && p[1] == 255 && p[2] == 255 && p[3] == 255).count()
}

#[test]
fn the_form_is_checked_and_read_back_and_nothing_is_shared_by_default() {
    let _g = serial();
    let cx = admin("shr-form");
    let mut f = form_for(&cx, "docs", 3);
    let d = draft_of_form(&f).unwrap();
    assert!(!d.upload && !d.mkdir && !d.overwrite && !d.show_hidden, "read only by default");
    assert_eq!((d.mode, d.access), (Mode::Files, Access::Local));
    assert_eq!(d.expiry_secs, 3600);
    // uploads, website, secret link
    f.set_choice(6, 2);
    f.set_choice(2, 1);
    f.set_choice(3, 0);
    let d = draft_of_form(&f).unwrap();
    assert!(d.upload && d.mkdir && !d.overwrite && d.mode == Mode::Website && d.access == Access::Token);
    // bad numbers
    f.set_value(7, "0");
    assert!(draft_of_form(&f).is_err());
    f.set_value(7, "x");
    assert!(draft_of_form(&f).is_err());
    f.set_value(7, "25");
    f.set_value(11, "99999");
    assert!(draft_of_form(&f).is_err());
    f.set_value(11, "");
    assert!(draft_of_form(&f).is_ok());
    // the open mode needs the typed confirmation
    let mut f = form_for(&cx, "open", 2);
    let mut a = SettingsApp::new(&cx);
    let e = a.shares_submit(&cx, &ShareForm::New, &mut f).unwrap_err();
    assert!(e.contains("everyone on the network"), "{e}");
    f.set_value(5, "OPEN");
    assert!(a.shares_submit(&cx, &ShareForm::New, &mut f).is_ok());
    // creating a share starts nothing
    // (the process-wide count is not asserted here: another test of this module may have a server running at the same time)
    assert!(SettingsApp::shares_rows(&cx).iter().all(|r| !r.running));
}

#[test]
fn folders_outside_the_account_are_refused() {
    let _g = serial();
    let cx = admin("shr-forbid");
    let mut a = SettingsApp::new(&cx);
    // even an administrator cannot map the whole filesystem or the system's interfaces
    for p in ["/", "/proc", "/sys", "/dev"] {
        let mut f = form_for(&cx, "x", 0);
        f.set_value(1, p);
        let e = a.shares_submit(&cx, &ShareForm::New, &mut f).unwrap_err();
        assert!(e.contains("system"), "{p}: {e}");
    }
    // an administrator may map any other folder of the disks (owner rule 2026-11-06)
    let other = temp_dir("shr-other");
    let mut f = form_for(&cx, "y", 0);
    f.set_value(1, &other.to_string_lossy());
    assert!(a.shares_submit(&cx, &ShareForm::New, &mut f).is_ok(), "an administrator maps a folder outside the home");
    // a person who is not an administrator may map only folders inside their own home
    let mut user = cx.clone();
    user.session.user = "ursula".into();
    user.session.role = iyakki_sec::policy::Role::User;
    user.session.policy = iyakki_sec::policy::Role::User.template();
    let mut ua = SettingsApp::new(&user);
    for p in ["/etc", "/boot", "/home", "/root"] {
        let mut f = form_for(&user, "u", 0);
        f.set_value(1, p);
        assert!(ua.shares_submit(&user, &ShareForm::New, &mut f).is_err(), "{p}");
    }
    let mut f = form_for(&user, "v", 0);
    f.set_value(1, &other.to_string_lossy());
    let e = ua.shares_submit(&user, &ShareForm::New, &mut f).unwrap_err();
    assert!(e.contains("inside your own home folder"), "{e}");
    // the system's private data folder is never shared, by anyone
    let mut f = form_for(&cx, "z", 0);
    f.set_value(1, &cx.services.dir().to_string_lossy());
    assert!(a.shares_submit(&cx, &ShareForm::New, &mut f).is_err());
}

#[test]
fn start_stop_and_stop_all_with_a_real_server() {
    let _g = serial();
    let cx = admin("shr-run");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Shares);
    let mut f = form_for(&cx, "pics", 3);
    a.shares_submit(&cx, &ShareForm::New, &mut f).unwrap();
    let r = SettingsApp::shares_rows(&cx);
    assert_eq!(r.len(), 1);
    let port = r[0].cfg.port;
    assert!(!r[0].running);
    key(&mut a, &cx, Key::Char('s'));
    assert!(status(&a).contains("is shared now"), "{}", status(&a));
    let body = get(port, "/hello.txt");
    assert!(body.starts_with("HTTP/1.1 200") && body.ends_with("hi there"), "{body}");
    assert!(get(port, "/").contains("hello.txt"));
    assert!(get(port, "/../../etc/passwd").contains(" 40"), "traversal answers 4xx");
    let rows = SettingsApp::shares_rows(&cx);
    assert!(rows[0].running && rows[0].clients == 1 && rows[0].left.is_some());
    key(&mut a, &cx, Key::Char('s'));
    assert!(status(&a).contains("Stopped sharing"));
    assert!(refused(port));
    // start again, then stop all
    key(&mut a, &cx, Key::Char('s'));
    assert_eq!(iyakki_agent::sharedfolder::running_total(), 1);
    // the top bar's Devices list carries a Stop action while something is shared
    assert!(iyakki_agent::sharedfolder::running_total() > 0);
    key(&mut a, &cx, Key::Char('x'));
    assert!(matches!(&a.modal, Some(Modal::Confirm(ConfirmKind::ShareStopAll, _))), "stop all asks first");
    assert_eq!(iyakki_agent::sharedfolder::running_total(), 1, "nothing stopped before the answer");
    key(&mut a, &cx, Key::Char('n'));
    assert!(a.modal.is_none() && iyakki_agent::sharedfolder::running_total() == 1, "any other key keeps sharing");
    key(&mut a, &cx, Key::Char('x'));
    key(&mut a, &cx, Key::Char('y'));
    assert!(status(&a).contains("Stopped 1 share"), "{}", status(&a));
    assert_eq!(iyakki_agent::sharedfolder::running_total(), 0);
    assert!(refused(port));
    // sign-out stops them too
    key(&mut a, &cx, Key::Char('s'));
    assert_eq!(iyakki_agent::sharedfolder::running_total(), 1);
    iyakki_agent::sharedfolder::on_logout("root");
    assert_eq!(iyakki_agent::sharedfolder::running_total(), 0);
    // expiry: the share records when it ends; a stopped one shows its setting
    key(&mut a, &cx, Key::Char('s'));
    assert_eq!(SettingsApp::shares_rows(&cx)[0].left.map(|l| l > 3000 && l <= 3600), Some(true));
    iyakki_agent::sharedfolder::stop_everything();
    // delete asks for the word
    key(&mut a, &cx, Key::Char('d'));
    assert!(matches!(&a.modal, Some(Modal::Form(FormKind::Share(ShareForm::Delete(_)), _))));
    key(&mut a, &cx, Key::Esc);
    assert_eq!(SettingsApp::shares_len(&cx), 1);
}

#[test]
fn the_secret_link_can_be_renewed_and_copied() {
    let _g = serial();
    let cx = admin("shr-token");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Shares);
    let mut f = form_for(&cx, "tok", 0);
    a.shares_submit(&cx, &ShareForm::New, &mut f).unwrap();
    let before = SettingsApp::shares_rows(&cx)[0].cfg.token.clone();
    assert_eq!(before.len(), 32);
    assert!(SettingsApp::shares_rows(&cx)[0].urls[0].contains(&format!("/s/{before}/")));
    key(&mut a, &cx, Key::Char('t'));
    let after = SettingsApp::shares_rows(&cx)[0].cfg.token.clone();
    assert_ne!(before, after);
    key(&mut a, &cx, Key::Char('c'));
    assert!(status(&a).contains("Copied the link"), "{}", status(&a));
    let mut f = form_for(&cx, "loc", 3);
    a.shares_submit(&cx, &ShareForm::New, &mut f).unwrap();
    key(&mut a, &cx, Key::Down);
    key(&mut a, &cx, Key::Char('t'));
    assert!(status(&a).contains("Only a share with a secret link"));
}

#[test]
fn share_this_folder_from_the_ide_or_edit_opens_the_form_filled_in() {
    let _g = serial();
    let cx = admin("shr-ide");
    let site = folder(&cx, "mysite");
    std::fs::write(std::path::Path::new(&site).join("index.html"), "<h1>hi</h1>").unwrap();
    let mut a = SettingsApp::new(&cx);
    a.open_item(&format!("share:{site}"), &cx);
    assert_eq!(a.tab(), Tab::Shares);
    let _ = draw(&mut a, &cx, 1280, 800);
    let Some(Modal::Form(FormKind::Share(ShareForm::New), f)) = &a.modal else { panic!("form not open") };
    assert_eq!(f.value(1), site);
    assert_eq!(f.value(0), "mysite");
    assert_eq!(f.choice_idx(2), 1, "a folder with index.html starts as a website");
    assert_eq!(f.choice_idx(3), 0, "the secret link is the default");
    // EDIT's menu entry asks through the shared hint
    iyakki_agent::sharedfolder::ask_share(&site);
    assert_eq!(iyakki_agent::sharedfolder::take_share_ask().as_deref(), Some(site.as_str()));
    assert_eq!(iyakki_agent::sharedfolder::take_share_ask(), None);
    // the page is there for accounts that have the IDE or EDIT, and for nobody else
    let mut guest = cx.clone();
    guest.session.user = "gus".into();
    guest.session.policy = iyakki_sec::policy::Role::Guest.template();
    let has = SettingsApp::new(&guest).tabs().contains(&Tab::Shares);
    assert_eq!(has, guest.session.policy.allows_app(iyakki_sec::policy::App::Ide) || guest.session.policy.allows_app(iyakki_sec::policy::App::Edit));
}

#[test]
fn the_page_draws_inside_the_window_at_four_sizes_with_a_qr_code() {
    let _g = serial();
    let cx = admin("shr-draw");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Shares);
    let empty = draw(&mut a, &cx, 1280, 720);
    assert!(empty.hits.iter().any(|h| h.label == "New share…"));
    for i in 0..6 {
        let mut f = form_for(&cx, &format!("share{i}"), i % 4);
        f.set_value(5, "OPEN");
        f.set_value(4, "a long password");
        a.shares_submit(&cx, &ShareForm::New, &mut f).unwrap();
    }
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
        for sel in [0usize, 3, 5] {
            a.sel[a.tab] = sel;
            let plain = draw(&mut a, &cx, w, h);
            for hit in plain.hits.iter().filter(|x| x.rect.w > 0.0) {
                let r = hit.rect;
                assert!(r.x >= -0.5 && r.y >= -0.5 && r.x + r.w <= w as f32 + 0.5 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: {:?} {:?}", hit.label, r);
            }
            assert!(plain.hits.iter().any(|x| x.label == "New share…"), "{w}x{h}");
            key(&mut a, &cx, Key::Char('q'));
            let with_qr = draw(&mut a, &cx, w, h);
            assert!(white(&with_qr) > white(&plain) + 500, "{w}x{h}: the QR code is drawn");
            key(&mut a, &cx, Key::Char('q'));
        }
    }
    // the form itself fits too
    let f = share_form(&iyakki_agent::sharedfolder::Draft::default(), false);
    a.modal = Some(Modal::Form(FormKind::Share(ShareForm::New), f));
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
        let g = draw(&mut a, &cx, w, h);
        assert!(!g.hits.is_empty(), "{w}x{h}");
    }
}

#[test]
fn receive_a_package_makes_a_ten_minute_password_drop_in_the_packages_folder_and_says_how_to_send_to_it() {
    let _g = serial();
    let cx = admin("sh-drop");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Shares);
    key(&mut a, &cx, Key::Char('r'));
    let said = status(&a);
    assert!(said.contains("Ready for 10 minutes") && said.contains("curl -T ffmpeg-libs-") && said.contains("-u iyakki:"), "{said}");
    let m = SettingsApp::shares(&cx);
    let drops: Vec<_> = m.list().into_iter().filter(|i| i.cfg.package_drop).collect();
    assert_eq!(drops.len(), 1);
    assert!(drops[0].running && drops[0].cfg.folder.ends_with("local/packages") && drops[0].cfg.expiry_secs == 600, "{:?}", drops[0].cfg);
    assert!(std::fs::metadata(cx.services.dir().join("local/packages")).unwrap().is_dir());
    // the drop has a real port from the shared-folder range, and the curl line ends in it (never ':0')
    let port = drops[0].cfg.port;
    assert!((8081..=8180).contains(&port), "port {port}");
    assert!(said.contains(&format!(":{port}")) && !said.contains(":0 ") && !said.contains(":0)"), "{said}");
    assert!(drops[0].urls.iter().all(|u| u.contains(&format!(":{port}"))), "{:?}", drops[0].urls);
    // the password is in the message but never in the saved share list
    let pw = said.split("-u iyakki:").nth(1).unwrap().split(' ').next().unwrap().to_string();
    assert_eq!(pw.len(), 12);
    assert!(!std::fs::read_to_string(m.file()).unwrap().contains(&pw));
    m.stop(&drops[0].cfg.id);
}


#[test]
fn the_password_hint_says_in_plain_words_that_it_is_sent_in_clear_and_the_page_is_not_encrypted() {
    let _g = serial();
    let top = shares_page::PASSWORD_HINT_TOP;
    assert!(top.contains("password is sent in clear over plain HTTP") && top.contains("secret link is the safest"), "{top}");
    assert!(top.contains("needs no link or password"), "{top}");
    let row = shares_page::PASSWORD_HINT_ROW;
    assert!(row.contains("not encrypted") && row.contains("network you trust"), "{row}");
    // plain words: no jargon beyond HTTP, and short enough to read at a glance
    for t in [top, row] {
        assert!(!t.contains("TLS") && !t.contains("hash") && t.len() < 300, "{t}");
    }
}

#[test]
fn the_form_says_in_one_line_who_may_map_which_folder_and_the_line_fits_at_four_sizes() {
    let _g = serial();
    let cx = admin("shr-hint");
    let mut a = SettingsApp::new(&cx);
    goto(&mut a, &cx, Tab::Shares);
    for edit in [false, true] {
        let f = share_form(&iyakki_agent::sharedfolder::Draft::default(), edit);
        assert_eq!(f.note, "Administrators can map any folder; everyone else only folders in their own home.");
        a.modal = Some(Modal::Form(FormKind::Share(ShareForm::New), f));
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            let g = draw(&mut a, &cx, w, h);
            let line = g.layout_log.texts.iter().filter(|(_, _, _, t)| "Administrators can map any folder; everyone else only folders in their own home.".contains(t.as_str()) && !t.is_empty());
            let rects: Vec<_> = line.map(|x| x.1).collect();
            assert!(!rects.is_empty(), "{w}x{h}: the hint is drawn");
            let joined: usize = g.layout_log.texts.iter().filter(|(_, _, _, t)| "Administrators can map any folder; everyone else only folders in their own home.".contains(t.as_str()) && t.len() > 3).map(|x| x.3.len()).sum();
            assert!(joined >= 60, "{w}x{h}: the whole sentence is drawn ({joined} chars)");
            for r in rects {
                assert!(r.x >= 0.0 && r.x + r.w <= w as f32 && r.y >= 0.0 && r.y + r.h <= h as f32, "{w}x{h}: hint {r:?} leaves the window");
            }
        }
    }
}
