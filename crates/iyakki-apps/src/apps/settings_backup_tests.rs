//! Settings -> Backup and restore: what the page says, the folder that is never chosen for the person, a backup made and opened and
//! restored through the page's own forms (with fake folders), the confirmations, and the page at four sizes in both of its modes.

use iyakki_shared::backup::{self as bk, Layout};

use super::backup_page::{BackupForm, BackupOut};
use super::*;
use crate::apps::tests::{cx_with_admin_store, temp_dir};

fn put(p: &std::path::Path, t: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, t).unwrap();
}

fn fake(tag: &str, user: &str) -> (AppCx, SettingsApp, Layout, std::path::PathBuf) {
    let mut cx = cx_with_admin_store(tag);
    cx.session.user = user.into();
    let root = temp_dir(&format!("{tag}-bk"));
    let l = Layout { data: root.join("data"), home: root.join("home"), user: user.into() };
    std::fs::create_dir_all(&l.data).unwrap();
    std::fs::create_dir_all(&l.home).unwrap();
    put(&l.home.join("Dashboards/cpu.json"), "{\"t\":1}");
    put(&l.home.join("Flows/a/flow.json"), "{}");
    put(&l.home.join("Recordings/x.wav"), "RIFFxxxx");
    put(&l.data.join(format!("vault/users/{user}.afv")), "SEALED");
    let a = SettingsApp::new(&cx).with_backup_layout(l.clone());
    let stick = root.join("stick");
    std::fs::create_dir_all(&stick).unwrap();
    (cx, a, l, stick)
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
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

fn draw(a: &mut SettingsApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
    let mut g = Gfx::new(w, h);
    g.clear(rgb(0, 0, 0));
    let s = crate::ui::scale_for(w, h);
    a.render(&mut g, R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s), s, cx, 0);
    g
}

fn alltext(g: &Gfx) -> String {
    g.layout_log.texts.iter().map(|t| t.3.clone()).collect::<Vec<_>>().join("\n")
}

fn status(a: &SettingsApp) -> String {
    a.status.as_ref().map(|s| s.0.clone()).unwrap_or_default()
}

fn wait(a: &mut SettingsApp, cx: &AppCx) {
    let __wall = std::time::Instant::now();
    while __wall.elapsed() < std::time::Duration::from_secs(30) {
        a.poll(cx);
        if a.job.is_none() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    panic!("the job did not finish");
}

fn make(a: &mut SettingsApp, cx: &AppCx, dest: &std::path::Path, pass: &str, again: &str) -> Result<String, String> {
    let mut f = Form::new("x").path("f", "", "", crate::filepicker::PathKind::Folder, &[]).secret("p", "").secret("r", "");
    f.set_value(0, &dest.to_string_lossy());
    f.set_value(1, pass);
    f.set_value(2, again);
    let r = a.backup_submit(cx, &BackupForm::Make, &mut f);
    wait(a, cx);
    r
}

fn open(a: &mut SettingsApp, cx: &AppCx, file: &std::path::Path, pass: &str) {
    let mut f = Form::new("x").path("f", "", "", crate::filepicker::PathKind::File, &[]).secret("p", "");
    f.set_value(0, &file.to_string_lossy());
    f.set_value(1, pass);
    a.backup_submit(cx, &BackupForm::Open, &mut f).unwrap();
    wait(a, cx);
}

fn newest(dir: &std::path::Path) -> std::path::PathBuf {
    std::fs::read_dir(dir).unwrap().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|e| e == bk::EXTENSION)).expect("a backup file")
}

#[test]
fn the_page_says_what_is_included_how_big_and_that_the_file_is_not_encrypted() {
    let (cx, mut a, _l, _s) = fake("bk-page", "root");
    goto(&mut a, &cx, Tab::Backup);
    let g = draw(&mut a, &cx, 1280, 900);
    let text = alltext(&g);
    assert!(text.contains("NOT") && text.contains("passphrase"), "{text}");
    assert!(text.contains("Flows") && text.contains("Dashboards") && text.contains("Recordings") && text.contains("vault"), "{text}");
    assert!(text.contains("file(s)") && text.contains(" B"), "sizes are shown");
    assert!(text.contains("in the backup"));
}

#[test]
fn nothing_is_written_until_the_person_chooses_a_folder_and_a_wrong_passphrase_is_refused() {
    let (cx, mut a, l, stick) = fake("bk-choose", "root");
    goto(&mut a, &cx, Tab::Backup);
    key(&mut a, &cx, Key::Char('b'));
    let Some(Modal::Form(FormKind::Backup(BackupForm::Make), f)) = &a.modal else { panic!("form not open") };
    assert_eq!(f.value(0), "", "no drive or folder is pre-chosen");
    a.modal = None;
    assert!(make(&mut a, &cx, std::path::Path::new(""), "", "").unwrap_err().contains("Choose the folder"));
    assert!(make(&mut a, &cx, &stick, "longenough1", "different22").unwrap_err().contains("differ"));
    assert!(make(&mut a, &cx, &stick, "short", "short").unwrap_err().contains("at least 8"));
    assert_eq!(std::fs::read_dir(&stick).unwrap().count(), 0);
    assert_eq!(std::fs::read_dir(&l.home).unwrap().count() > 0, true);
}

#[test]
fn back_up_open_and_restore_through_the_page() {
    let (cx, mut a, l, stick) = fake("bk-flow", "root");
    goto(&mut a, &cx, Tab::Backup);
    // leave recordings out
    for _ in 0..7 {
        key(&mut a, &cx, Key::Down);
    }
    assert_eq!(a.backup_rows(&cx)[a.sel[a.tab]].id, "recordings");
    key(&mut a, &cx, Key::Char(' '));
    make(&mut a, &cx, &stick, "", "").unwrap();
    assert!(status(&a).contains("NOT encrypted"), "{}", status(&a));
    let file = newest(&stick);
    open(&mut a, &cx, &file, "");
    assert!(status(&a).contains("Verified"), "{}", status(&a));
    let rows = a.backup_rows(&cx);
    assert!(rows.iter().all(|r| r.id != "recordings"), "left out means not in the file");
    // change data, choose only dashboards
    put(&l.home.join("Dashboards/cpu.json"), "CHANGED");
    key(&mut a, &cx, Key::Char('r'));
    assert!(status(&a).contains("Choose what to restore"), "nothing is chosen for the person");
    let idx = rows.iter().position(|r| r.id == "dashboards").unwrap();
    a.sel[a.tab] = idx;
    key(&mut a, &cx, Key::Char(' '));
    let g = draw(&mut a, &cx, 1280, 900);
    assert!(alltext(&g).contains("checksum"));
    key(&mut a, &cx, Key::Char('r'));
    let Some(Modal::Form(FormKind::Backup(BackupForm::Restore), mut f)) = a.modal.take() else { panic!("no confirm form") };
    assert!(a.backup_submit(&cx, &BackupForm::Restore, &mut f).unwrap_err().contains("RESTORE"));
    f.set_value(0, "RESTORE");
    a.backup_submit(&cx, &BackupForm::Restore, &mut f).unwrap();
    wait(&mut a, &cx);
    assert!(status(&a).contains("Restored 1"), "{}", status(&a));
    assert_eq!(std::fs::read_to_string(l.home.join("Dashboards/cpu.json")).unwrap(), "{\"t\":1}");
    assert!(std::fs::read_dir(&l.home).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("Dashboards.before-restore-")), "the current data is kept");
    key(&mut a, &cx, Key::Char('c'));
    assert!(a.backup.loaded.is_none());
}

#[test]
fn a_damaged_file_and_a_wrong_passphrase_are_reported_and_nothing_opens() {
    let (cx, mut a, _l, stick) = fake("bk-bad", "root");
    make(&mut a, &cx, &stick, "correct horse battery", "correct horse battery").unwrap();
    assert!(status(&a).contains("Encrypted"), "{}", status(&a));
    let file = newest(&stick);
    let mut f = Form::new("x").path("f", "", "", crate::filepicker::PathKind::File, &[]).secret("p", "");
    f.set_value(0, &file.to_string_lossy());
    a.backup_submit(&cx, &BackupForm::Open, &mut f).unwrap();
    wait(&mut a, &cx);
    assert!(status(&a).contains("encrypted"), "{}", status(&a));
    assert!(a.backup.loaded.is_none());
    f.set_value(1, "not the passphrase");
    a.backup_submit(&cx, &BackupForm::Open, &mut f).unwrap();
    wait(&mut a, &cx);
    assert!(status(&a).contains("Wrong passphrase"), "{}", status(&a));
    let mut bytes = std::fs::read(&file).unwrap();
    let n = bytes.len();
    bytes[n - 5] ^= 0xff;
    std::fs::write(&file, bytes).unwrap();
    open(&mut a, &cx, &file, "correct horse battery");
    assert!(status(&a).contains("damaged") || status(&a).contains("Wrong passphrase"), "{}", status(&a));
    assert!(a.backup.loaded.is_none());
}

#[test]
fn another_accounts_backup_needs_the_name_typed() {
    let (_cx, mut amy, _l, stick) = fake("bk-amy", "amy");
    let cx_amy = {
        let mut c = cx_with_admin_store("bk-amy2");
        c.session.user = "amy".into();
        c
    };
    make(&mut amy, &cx_amy, &stick, "", "").unwrap();
    let file = newest(&stick);
    let (cx, mut bo, _l2, _s2) = fake("bk-bo", "root");
    goto(&mut bo, &cx, Tab::Backup);
    open(&mut bo, &cx, &file, "");
    let g = draw(&mut bo, &cx, 1280, 900);
    assert!(alltext(&g).contains("amy"));
    key(&mut bo, &cx, Key::Char(' '));
    key(&mut bo, &cx, Key::Char('r'));
    let Some(Modal::Form(FormKind::Backup(BackupForm::Restore), mut f)) = bo.modal.take() else { panic!("no confirm form") };
    f.set_value(0, "RESTORE");
    assert!(bo.backup_submit(&cx, &BackupForm::Restore, &mut f).unwrap_err().contains("amy"));
    f.set_value(1, "amy");
    bo.backup_submit(&cx, &BackupForm::Restore, &mut f).unwrap();
    wait(&mut bo, &cx);
    assert!(status(&bo).contains("Restored"), "{}", status(&bo));
}

#[test]
fn the_page_fits_from_a_phone_to_4k_in_both_modes() {
    for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080), (2560, 1600), (3840, 2160), (480, 900)] {
        let (cx, mut a, _l, stick) = fake(&format!("bk-geo-{w}"), "root");
        goto(&mut a, &cx, Tab::Backup);
        for loaded in [false, true] {
            if loaded {
                make(&mut a, &cx, &stick, "", "").unwrap();
                let f = newest(&stick);
                open(&mut a, &cx, &f, "");
                assert!(a.backup.loaded.is_some());
            }
            let g = draw(&mut a, &cx, w, h);
            let buttons: Vec<_> = g.hits.iter().filter(|x| matches!(x.ev, Event::Key(Key::Char(_))) && x.rect.h < 90.0).collect();
            assert!(buttons.len() >= 4, "{w}x{h} loaded={loaded}: buttons {}", buttons.len());
            for (i, b) in buttons.iter().enumerate() {
                assert!(b.rect.x >= 0.0 && b.rect.x + b.rect.w <= w as f32 + 0.5 && b.rect.y >= 0.0 && b.rect.y + b.rect.h <= h as f32 + 0.5, "{w}x{h} {} {:?}", b.label, b.rect);
                let o = |p: R, q: R| p.x < q.x + q.w - 0.5 && q.x < p.x + p.w - 0.5 && p.y < q.y + q.h - 0.5 && q.y < p.y + p.h - 0.5;
                assert!(buttons[i + 1..].iter().all(|n| !o(b.rect, n.rect)), "{w}x{h}: {} overlaps another button", b.label);
            }
            let rows: Vec<_> = g.hits.iter().filter(|x| matches!(x.ev, Event::Row(_))).collect();
            assert!(!rows.is_empty(), "{w}x{h} loaded={loaded}: at least one row");
            for r in rows {
                assert!(r.rect.x >= 0.0 && r.rect.x + r.rect.w <= w as f32 + 0.5 && r.rect.y + r.rect.h <= h as f32 + 0.5, "{w}x{h} row {:?}", r.rect);
            }
        }
    }
}

#[test]
fn the_page_shows_a_bar_while_a_backup_runs_and_the_s_key_stops_it() {
    let (cx, mut a, _l, _stick) = fake("bk-stop", "amy");
    goto(&mut a, &cx, Tab::Backup);
    // a running job that holds a progress record and ends when the stop flag is set (the real job does the same with `create_with_progress`)
    let pr = std::sync::Arc::new(bk::Progress::default());
    a.backup.progress = Some(pr.clone());
    let p2 = pr.clone();
    a.start("Backing up", move || {
        let __wall = std::time::Instant::now();
        while __wall.elapsed() < std::time::Duration::from_secs(30) {
            if p2.cancel.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        JobOut::Backup(BackupOut::Made(Err(bk::BackupError::Cancelled.to_string())))
    });
    assert!(a.backup_buttons().iter().any(|b| b.2 == 's' && b.0 == "Stop the backup"), "the stop button is there while it runs");
    let counting = alltext(&draw(&mut a, &cx, 1280, 720));
    assert!(counting.contains("Counting the files"), "{counting}");
    pr.total.store(1000, std::sync::atomic::Ordering::Relaxed);
    pr.done.store(250, std::sync::atomic::Ordering::Relaxed);
    assert!(alltext(&draw(&mut a, &cx, 1280, 720)).contains("25%"), "the bar says how far it has come");
    key(&mut a, &cx, Key::Char('s'));
    assert!(pr.cancel.load(std::sync::atomic::Ordering::Relaxed), "{}", status(&a));
    wait(&mut a, &cx);
    assert!(status(&a).contains("stopped") && a.backup.progress.is_none(), "{}", status(&a));
    assert!(!a.backup_buttons().iter().any(|b| b.2 == 's'), "no stop button when nothing runs");
    key(&mut a, &cx, Key::Char('s'));
    assert!(status(&a).contains("No backup is running"));
}

#[test]
fn a_real_backup_made_through_the_page_leaves_no_progress_behind() {
    let (cx, mut a, _l, stick) = fake("bk-prog", "amy");
    goto(&mut a, &cx, Tab::Backup);
    make(&mut a, &cx, &stick, "", "").unwrap();
    assert!(a.backup.progress.is_none() && !stick.read_dir().unwrap().flatten().all(|e| e.path().extension().is_none_or(|x| x != bk::EXTENSION)), "{}", status(&a));
}

#[test]
fn the_schedule_form_saves_checks_and_shows_the_schedule_and_u_turns_it_off() {
    let (cx, mut a, _l, stick) = fake("bk-sched", "amy");
    goto(&mut a, &cx, Tab::Backup);
    let dir = cx.data_dir().to_path_buf();
    assert!(!iyakki_shared::backup_schedule::Schedule::load(&dir, "amy").enabled);
    let form = |days: &str, hour: &str, folder: &str, keep: &str, pf: &str| {
        let mut f = Form::new("x").text("d", "", "").text("h", "", "").path("f", "", "", crate::filepicker::PathKind::Folder, &[]).text("k", "", "").path("p", "", "", crate::filepicker::PathKind::File, &[]).text("s", "", "").path("c", "", "", crate::filepicker::PathKind::File, &[]).text("n", "", "");
        for (i, v) in [days, hour, folder, keep, pf, "", "", "0"].into_iter().enumerate() {
            f.set_value(i, v);
        }
        f
    };
    let stick_s = stick.to_string_lossy().into_owned();
    for bad in [form("0", "3", &stick_s, "7", ""), form("2", "24", &stick_s, "7", ""), form("2", "3", &stick_s, "0", ""), form("2", "3", "relative", "7", ""), form("2", "3", &format!("{stick_s}/not-there"), "7", ""), form("x", "3", &stick_s, "7", "")] {
        let mut b = bad;
        assert!(a.backup_submit(&cx, &BackupForm::Schedule, &mut b).is_err());
    }
    assert!(!iyakki_shared::backup_schedule::Schedule::load(&dir, "amy").enabled, "nothing was saved by a bad form");
    let mut ok = form("2", "5", &stick_s, "4", "");
    let said = a.backup_submit(&cx, &BackupForm::Schedule, &mut ok).unwrap();
    assert!(said.contains("Every 2 day(s) from 05:00") && said.contains("NOT encrypted"), "{said}");
    let s = iyakki_shared::backup_schedule::Schedule::load(&dir, "amy");
    assert!(s.enabled && (s.every_days, s.hour, s.keep) == (2, 5, 4) && s.dest == stick_s);
    let t = alltext(&draw(&mut a, &cx, 1280, 720));
    assert!(t.contains("Every 2 day(s) from 05:00"), "the page shows it: {t}");
    // a share instead of the folder needs no folder, only an address that parses; a chain length of one is refused
    let mut sh = form("2", "5", "", "4", "");
    sh.set_value(5, "smb://nas/backups");
    assert!(a.backup_submit(&cx, &BackupForm::Schedule, &mut sh).is_ok());
    let s = iyakki_shared::backup_schedule::Schedule::load(&dir, "amy");
    assert!(s.share == "smb://nas/backups" && s.full_every == 0, "{s:?}");
    let mut one = form("2", "5", &stick_s, "4", "");
    one.set_value(7, "1");
    assert!(a.backup_submit(&cx, &BackupForm::Schedule, &mut one).is_err());
    let mut ch = form("2", "5", &stick_s, "4", "");
    ch.set_value(7, "7");
    assert!(a.backup_submit(&cx, &BackupForm::Schedule, &mut ch).unwrap().contains("after 7 files"));
    key(&mut a, &cx, Key::Char('u'));
    assert!(!iyakki_shared::backup_schedule::Schedule::load(&dir, "amy").enabled && status(&a).contains("is off"));
    let audit = std::fs::read_to_string(cx.audit.path_buf()).unwrap_or_default();
    assert!(audit.contains("backup.schedule"), "{audit}");
}

#[test]
fn a_share_a_chain_link_and_the_machines_own_data_are_backed_up_through_the_page() {
    use iyakki_shared::backup_share::FakeMounter;
    let (cx, a, l, stick) = fake("bk-more", "root");
    let fm = std::sync::Arc::new(FakeMounter::default());
    let mut a = a.with_backup_mounter(fm.clone());
    goto(&mut a, &cx, Tab::Backup);
    // the buttons are there
    let names: Vec<&str> = a.backup_buttons().iter().map(|b| b.0).collect();
    assert!(names.contains(&"To a network share…") && names.contains(&"Add a link…") && names.contains(&"Machine data…"), "{names:?}");
    // a share: the address is read strictly, the passwords must match, the share is let go again
    let share = |a: &mut SettingsApp, addr: &str, creds: &str, p1: &str, p2: &str| {
        let mut f = Form::new("x").text("a", "", "").path("c", "", "", crate::filepicker::PathKind::File, &[]).secret("p", "").secret("r", "");
        f.set_value(0, addr);
        f.set_value(1, creds);
        f.set_value(2, p1);
        f.set_value(3, p2);
        let r = a.backup_submit(&cx, &BackupForm::Share, &mut f);
        wait(a, &cx);
        r
    };
    assert!(share(&mut a, "http://nas/x", "", "", "").is_err());
    assert!(share(&mut a, "nfs://nas/export", "", "longenough1", "different22").unwrap_err().contains("differ"));
    assert!(fm.calls.lock().unwrap().is_empty(), "nothing is mounted for a refused form");
    let creds = stick.join("creds.txt");
    put(&creds, "amy\nsecret-pw\n");
    share(&mut a, "smb://nas/backups/me", &creds.to_string_lossy(), "", "").unwrap();
    assert!(status(&a).contains("Wrote iyakki-backup-root-") && status(&a).contains("not encrypted") && !status(&a).contains("secret-pw"), "{}", status(&a));
    let calls = fm.calls.lock().unwrap().clone();
    assert!(calls.len() == 2 && calls[0].starts_with("mount ") && calls[1].starts_with("umount"), "{calls:?}");
    assert!(!calls[0].contains("secret-pw"));
    // the machine's own data: an administrator, a passphrase, nothing else
    let sys = |a: &mut SettingsApp, dest: &std::path::Path, p1: &str, p2: &str| {
        let mut f = Form::new("x").path("f", "", "", crate::filepicker::PathKind::Folder, &[]).secret("p", "").secret("r", "");
        f.set_value(0, &dest.to_string_lossy());
        f.set_value(1, p1);
        f.set_value(2, p2);
        let r = a.backup_submit(&cx, &BackupForm::System, &mut f);
        wait(a, &cx);
        r
    };
    put(&l.data.join("settings.json"), "{}");
    assert!(sys(&mut a, &stick, "", "").unwrap_err().contains("at least 8"));
    sys(&mut a, &stick, "longenough1", "longenough1").unwrap();
    assert!(status(&a).contains("encrypted"), "{}", status(&a));
    // a chain: a full file, then a link naming it
    let full = stick.join("full");
    std::fs::create_dir_all(&full).unwrap();
    make(&mut a, &cx, &full, "", "").unwrap();
    let first = newest(&full);
    put(&l.home.join("Flows/b/flow.json"), "{\"new\":1}");
    let link = |a: &mut SettingsApp, chain: &str| {
        let mut f = Form::new("x").path("f", "", "", crate::filepicker::PathKind::Folder, &[]).text("c", "", "").secret("p", "");
        f.set_value(0, &full.to_string_lossy());
        f.set_value(1, chain);
        let r = a.backup_submit(&cx, &BackupForm::Link, &mut f);
        wait(a, &cx);
        r
    };
    assert!(link(&mut a, "").is_err());
    link(&mut a, &first.to_string_lossy()).unwrap();
    assert!(status(&a).contains("Wrote iyakki-incr-root-") && status(&a).contains("1 changed files"), "{}", status(&a));
}

#[test]
fn the_machines_own_data_and_a_chain_are_restored_through_the_page_with_every_check() {
    let (cx, a, l, stick) = fake("bk-restore", "root");
    let mut a = a;
    goto(&mut a, &cx, Tab::Backup);
    let names: Vec<&str> = a.backup_buttons().iter().map(|b| b.0).collect();
    assert!(names.contains(&"Restore machine data…") && names.contains(&"Restore a chain…"), "{names:?}");
    // machine data: made, changed, then put back; the passphrase is typed twice and RESTORE is typed
    put(&l.data.join("settings.json"), "{\"v\":1}");
    let mk = |a: &mut SettingsApp| {
        let mut f = Form::new("x").path("f", "", "", crate::filepicker::PathKind::Folder, &[]).secret("p", "").secret("r", "");
        f.set_value(0, &stick.to_string_lossy());
        f.set_value(1, "longenough1");
        f.set_value(2, "longenough1");
        a.backup_submit(&cx, &BackupForm::System, &mut f).unwrap();
        wait(a, &cx);
    };
    mk(&mut a);
    let file = newest(&stick);
    put(&l.data.join("settings.json"), "{\"v\":2}");
    let sysr = |a: &mut SettingsApp, p1: &str, p2: &str, word: &str| {
        let mut f = Form::new("x").path("f", "", "", crate::filepicker::PathKind::File, &[]).secret("p", "").secret("r", "").text("w", "", "");
        f.set_value(0, &file.to_string_lossy());
        f.set_value(1, p1);
        f.set_value(2, p2);
        f.set_value(3, word);
        let r = a.backup_submit(&cx, &BackupForm::SystemRestore, &mut f);
        wait(a, &cx);
        r
    };
    assert!(sysr(&mut a, "longenough1", "other-pass-22", "RESTORE").unwrap_err().contains("differ"));
    assert!(sysr(&mut a, "longenough1", "longenough1", "yes").unwrap_err().contains("RESTORE"));
    assert!(sysr(&mut a, "", "", "RESTORE").is_err());
    assert_eq!(std::fs::read_to_string(l.data.join("settings.json")).unwrap(), "{\"v\":2}", "a refused form changes nothing");
    sysr(&mut a, "longenough1", "longenough1", "RESTORE").unwrap();
    assert!(status(&a).contains("Restored"), "{}", status(&a));
    assert_eq!(std::fs::read_to_string(l.data.join("settings.json")).unwrap(), "{\"v\":1}");
    // a chain: full file, a link, then the areas back through the page
    let full = stick.join("chain");
    std::fs::create_dir_all(&full).unwrap();
    make(&mut a, &cx, &full, "", "").unwrap();
    let first = newest(&full);
    put(&l.home.join("Flows/b/flow.json"), "{\"new\":1}");
    let mut f = Form::new("x").path("f", "", "", crate::filepicker::PathKind::Folder, &[]).text("c", "", "").secret("p", "");
    f.set_value(0, &full.to_string_lossy());
    f.set_value(1, &first.to_string_lossy());
    a.backup_submit(&cx, &BackupForm::Link, &mut f).unwrap();
    wait(&mut a, &cx);
    let second = std::fs::read_dir(&full).unwrap().filter_map(Result::ok).map(|e| e.path()).find(|p| p != &first && p.to_string_lossy().contains("incr")).unwrap();
    std::fs::remove_file(l.home.join("Flows/b/flow.json")).unwrap();
    let chain = format!("{};{}", first.display(), second.display());
    let cr = |a: &mut SettingsApp, chain: &str, areas: &str, word: &str| {
        let mut f = Form::new("x").text("c", "", "").text("a", "", "").secret("p", "").text("w", "", "");
        f.set_value(0, chain);
        f.set_value(1, areas);
        f.set_value(3, word);
        let r = a.backup_submit(&cx, &BackupForm::ChainRestore, &mut f);
        wait(a, &cx);
        r
    };
    assert!(cr(&mut a, "", "flows", "RESTORE").is_err());
    assert!(cr(&mut a, &chain, "", "RESTORE").is_err());
    assert!(cr(&mut a, &chain, "nonsense", "RESTORE").unwrap_err().contains("not a backup area"));
    assert!(cr(&mut a, &chain, "flows", "no").unwrap_err().contains("RESTORE"));
    assert!(!l.home.join("Flows/b/flow.json").exists(), "a refused form restores nothing");
    cr(&mut a, &chain, "flows", "RESTORE").unwrap();
    assert!(l.home.join("Flows/b/flow.json").exists(), "{}", status(&a));
}
