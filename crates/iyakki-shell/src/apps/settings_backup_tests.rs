//! Settings -> Backup and restore: what the page says, the folder that is never chosen for the person, a backup made and opened and
//! restored through the page's own forms (with fake folders), the confirmations, and the page at four sizes in both of its modes.

use iyakki_agent::backup::{self as bk, Layout};

use super::backup_page::BackupForm;
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
    let s = crate::shell::scale_for(w, h);
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
    for _ in 0..400 {
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
