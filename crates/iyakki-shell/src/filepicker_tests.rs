//! File picker tests: a fake filesystem under a short temp folder, the account's limits, drawing at every size.

use super::filepicker::*;
use crate::apps::widgets::{Form, FormEvent};
use crate::gfx::Gfx;
use crate::ui::{Event, Key, R};
use iyakki_agent::scope::FileRoots;
use iyakki_sec::policy::Role;
use std::path::{Path, PathBuf};

struct Fs {
    base: PathBuf,
    home: PathBuf,
}

impl Drop for Fs {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.base);
    }
}

fn fs(name: &str) -> Fs {
    let tmp = std::env::var("TMPDIR").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("/tmp/iyakki-test/rt-picker"));
    let base = std::fs::canonicalize({
        let _ = std::fs::create_dir_all(&tmp);
        &tmp
    })
    .unwrap()
    .join(format!("pk-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let home = base.join("home/amy");
    for d in ["docs", "zdir", ".cfg"] {
        std::fs::create_dir_all(home.join(d)).unwrap();
    }
    std::fs::create_dir_all(base.join("outside")).unwrap();
    std::fs::write(home.join("a.txt"), "abc").unwrap();
    std::fs::write(home.join("b.csv"), "x,y\n1,2\n").unwrap();
    std::fs::write(home.join("big.bin"), vec![0u8; 2000]).unwrap();
    std::fs::write(home.join(".hidden"), "h").unwrap();
    std::fs::write(home.join("docs/note.md"), "n").unwrap();
    std::fs::write(base.join("outside/secret.txt"), "s").unwrap();
    std::os::unix::fs::symlink(base.join("outside"), home.join("link-out")).unwrap();
    Fs { base, home }
}

fn account_scope(f: &Fs, role: Role, write: bool) -> Scope {
    let mut p = role.template();
    p.fs_read = if role == Role::Guest { vec![] } else { vec![f.home.display().to_string()] };
    p.fs_write = if write { vec![f.home.display().to_string()] } else { vec![] };
    let data = f.base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    scope_for("amy", &p, FileRoots::with_home(&p, "amy", &data, f.home.clone()))
}

fn picker(f: &Fs, kind: PathKind, exts: &[&str]) -> FilePicker {
    let e: Vec<String> = exts.iter().map(|s| s.to_string()).collect();
    FilePicker::new(kind, &e, "Choose", &f.home.to_string_lossy(), account_scope(f, Role::User, true))
}

fn names(p: &FilePicker) -> Vec<String> {
    p.visible().iter().map(|e| e.name.clone()).collect()
}

fn key(p: &mut FilePicker, k: Key) -> PickOut {
    p.handle_event(&Event::Key(k))
}

#[test]
fn listing_shows_folders_first_hides_dotfiles_and_links_that_leave_the_account() {
    let f = fs("list");
    let p = picker(&f, PathKind::File, &[]);
    assert_eq!(names(&p), ["docs", "zdir", "a.txt", "b.csv", "big.bin"], "folders first, no .hidden, no link to /outside");
    let big = p.entries().iter().find(|e| e.name == "big.bin").unwrap();
    assert_eq!(big.size, 2000);
    assert_eq!(human_size(2000), "2.0 KB");
    assert_eq!(human_size(12), "12 B");
    assert!(date_text(big.modified).starts_with("20"));
    assert_eq!(date_text(Some(std::time::UNIX_EPOCH)), "1970-01-01 00:00");
}

#[test]
fn clicking_the_columns_sorts_and_clicking_again_reverses() {
    let f = fs("sort");
    let mut p = picker(&f, PathKind::File, &[]);
    p.handle_event(&Event::Tab(TAB_BASE + 11)); // size
    assert_eq!(names(&p)[2..], ["a.txt", "b.csv", "big.bin"], "size ascending after the folders");
    p.handle_event(&Event::Tab(TAB_BASE + 11));
    assert_eq!(names(&p)[2], "big.bin", "descending");
    p.handle_event(&Event::Tab(TAB_BASE + 10)); // name
    p.handle_event(&Event::Tab(TAB_BASE + 10));
    assert_eq!(names(&p), ["zdir", "docs", "big.bin", "b.csv", "a.txt"], "name descending, folders still first");
}

#[test]
fn filter_hidden_switch_and_extension_filter() {
    let f = fs("filter");
    let mut p = picker(&f, PathKind::File, &["csv"]);
    for c in "B.C".chars() {
        key(&mut p, Key::Char(c));
    }
    assert_eq!(names(&p), ["b.csv"], "typing filters, case-insensitively");
    p.filter.set("");
    p.handle_event(&Event::Tab(TAB_BASE + 3));
    assert!(names(&p).contains(&".hidden".to_string()) && names(&p).contains(&".cfg".to_string()), "hidden shown");
    // a file that does not fit the extension cannot be opened
    let i = names(&p).iter().position(|n| n == "a.txt").unwrap();
    p.handle_event(&Event::Row(i));
    assert_eq!(key(&mut p, Key::Enter), PickOut::Stay);
    assert!(p.error.as_deref().unwrap().contains("csv"));
    let i = names(&p).iter().position(|n| n == "b.csv").unwrap();
    p.handle_event(&Event::Row(i));
    assert_eq!(key(&mut p, Key::Enter), PickOut::Chosen(f.home.join("b.csv")));
}

#[test]
fn navigation_by_row_keyboard_up_home_and_breadcrumb() {
    let f = fs("nav");
    let mut p = picker(&f, PathKind::File, &[]);
    p.handle_event(&Event::Row(0)); // docs
    assert_eq!(key(&mut p, Key::Enter), PickOut::Stay);
    assert_eq!(p.dir, f.home.join("docs"));
    assert_eq!(names(&p), ["note.md"]);
    p.handle_event(&Event::Tab(TAB_BASE)); // Up
    assert_eq!(p.dir, f.home);
    assert_eq!(p.sel.as_deref(), Some("docs"), "the folder we came from is chosen");
    key(&mut p, Key::Down);
    assert_eq!(p.sel.as_deref(), Some("zdir"));
    key(&mut p, Key::End);
    assert_eq!(p.sel.as_deref(), Some("big.bin"));
    key(&mut p, Key::Home);
    assert_eq!(p.sel.as_deref(), Some("docs"));
    // above home is not the account's to read: Up says so and stays
    p.handle_event(&Event::Tab(TAB_BASE));
    assert_eq!(p.dir, f.home);
    assert!(p.error.as_deref().unwrap().contains("nothing above"));
    // Backspace goes up from a sub folder; Home returns; a breadcrumb jumps back
    p.handle_event(&Event::Row(0));
    key(&mut p, Key::Enter);
    key(&mut p, Key::Backspace);
    assert_eq!(p.dir, f.home);
    p.handle_event(&Event::Row(0));
    key(&mut p, Key::Enter);
    p.handle_event(&Event::Tab(TAB_BASE + 1));
    assert_eq!(p.dir, f.home);
    p.handle_event(&Event::Row(0));
    key(&mut p, Key::Enter);
    let depth = f.home.components().count();
    p.handle_event(&Event::Tab(TAB_BASE + 100 + depth - 1));
    assert_eq!(p.dir, f.home, "the breadcrumb of the home folder");
}

#[test]
fn selecting_a_file_a_folder_or_a_save_name() {
    let f = fs("select");
    let mut p = picker(&f, PathKind::Folder, &[]);
    assert_eq!(p.accept(), PickOut::Chosen(f.home.clone()), "no row chosen: the folder shown");
    p.handle_event(&Event::Row(1));
    assert_eq!(p.accept(), PickOut::Chosen(f.home.join("zdir")));
    let mut p = picker(&f, PathKind::File, &[]);
    assert_eq!(p.accept(), PickOut::Stay);
    assert!(p.error.is_some());
    p.handle_event(&Event::Row(2));
    assert_eq!(p.accept(), PickOut::Chosen(f.home.join("a.txt")));
    assert_eq!(p.handle_event(&Event::Tab(TAB_BASE + 5)), PickOut::Cancel);
    assert_eq!(key(&mut p, Key::Esc), PickOut::Cancel);
    let mut p = picker(&f, PathKind::Save, &[]);
    p.handle_event(&Event::Row(2));
    assert_eq!(p.name.value, "a.txt", "clicking a file fills the name");
    p.name.set("out.json");
    assert_eq!(p.accept(), PickOut::Chosen(f.home.join("out.json")));
    p.name.set("a/b");
    assert_eq!(p.accept(), PickOut::Stay);
    // a file path in the location line is chosen at once in file mode
    let mut p = picker(&f, PathKind::File, &[]);
    let t = f.home.join("docs/note.md").to_string_lossy().into_owned();
    assert_eq!(p.handle_event(&Event::Key(Key::Ctrl('l'))), PickOut::Stay);
    p.handle_event(&Event::Paste(t));
    assert_eq!(key(&mut p, Key::Enter), PickOut::Chosen(f.home.join("docs/note.md")));
}

#[test]
fn a_new_folder_is_made_only_where_the_account_may_write() {
    let f = fs("mkdir");
    let mut p = picker(&f, PathKind::Folder, &[]);
    p.handle_event(&Event::Tab(TAB_BASE + 2));
    for c in "fresh".chars() {
        key(&mut p, Key::Char(c));
    }
    key(&mut p, Key::Enter);
    assert!(f.home.join("fresh").is_dir() && p.sel.as_deref() == Some("fresh") && !p.creating);
    let mut ro = FilePicker::new(PathKind::Folder, &[], "x", &f.home.to_string_lossy(), account_scope(&f, Role::User, false));
    ro.handle_event(&Event::Tab(TAB_BASE + 2));
    ro.name.set("nope");
    ro.handle_event(&Event::Tab(TAB_BASE + 6));
    assert!(!f.home.join("nope").exists() && ro.error.as_deref().unwrap().contains("may not"));
}

#[test]
fn the_picker_shows_only_what_the_account_may_read() {
    let f = fs("roles");
    let guest = FilePicker::new(PathKind::File, &[], "x", &f.home.to_string_lossy(), account_scope(&f, Role::Guest, false));
    assert!(guest.entries().is_empty(), "a guest reads nothing here");
    let mut user = picker(&f, PathKind::File, &[]);
    assert!(!user.go(f.base.join("outside")), "outside the scope");
    assert!(user.error.as_deref().unwrap().contains("may read"));
    assert_eq!(user.dir, f.home);
    let t = f.base.join("outside/secret.txt").to_string_lossy().into_owned();
    assert_eq!(check(&account_scope(&f, Role::User, false), PathKind::File, &[], &t), Check::NotAllowed);
}

#[test]
fn pasted_paths_are_checked_live() {
    let f = fs("check");
    let sc = account_scope(&f, Role::User, true);
    let h = f.home.to_string_lossy().into_owned();
    assert_eq!(check(&sc, PathKind::File, &[], ""), Check::Empty);
    assert_eq!(check(&sc, PathKind::File, &[], &format!("{h}/a.txt")), Check::IsFile);
    assert_eq!(check(&sc, PathKind::File, &[], "~/a.txt"), Check::IsFile, "~ is home");
    assert_eq!(check(&sc, PathKind::File, &[], "a.txt"), Check::IsFile, "relative to home");
    assert_eq!(check(&sc, PathKind::Folder, &[], &format!("{h}/docs")), Check::IsFolder);
    assert_eq!(check(&sc, PathKind::File, &[], &format!("{h}/docs")), Check::WrongKind("a folder"));
    assert_eq!(check(&sc, PathKind::Folder, &[], &format!("{h}/a.txt")), Check::WrongKind("a file"));
    assert_eq!(check(&sc, PathKind::File, &[], &format!("{h}/nope")), Check::Missing);
    assert_eq!(check(&sc, PathKind::File, &["csv".into()], &format!("{h}/a.txt")), Check::BadExtension("csv".into()));
    assert_eq!(check(&sc, PathKind::Save, &[], &format!("{h}/new.txt")), Check::NewFile);
    assert_eq!(check(&sc, PathKind::Save, &[], &format!("{h}/a.txt")), Check::Replaces);
    assert_eq!(check(&sc, PathKind::Save, &[], &format!("{h}/nodir/x")), Check::Missing);
    assert_eq!(check(&sc, PathKind::Save, &[], &format!("{h}/docs")), Check::WrongKind("a folder"));
    assert_eq!(chip(&Check::IsFile, PathKind::File), ("file found".into(), Some(true)));
    // in a form: Ctrl+V (a Paste event) fills the box and the chip follows
    set_scope_override(Some(sc));
    let mut form = Form::new("t").path("File", "", "hint", PathKind::File, &[]);
    assert_eq!(form.path_check(0), Some(Check::Empty));
    form.handle_event(&Event::Paste(format!("{h}/a.txt")));
    assert_eq!(form.path_check(0), Some(Check::IsFile));
    form.set_value(0, &format!("{h}/docs"));
    assert_eq!(form.path_check(0), Some(Check::WrongKind("a folder")));
    set_scope_override(None);
}

#[test]
fn a_dropped_path_fills_the_field_and_a_file_dropped_on_a_folder_field_uses_its_folder() {
    let f = fs("drop");
    set_scope_override(Some(account_scope(&f, Role::User, true)));
    let mut form = Form::new("t").text("Name", "", "").path("Folder", "", "", PathKind::Folder, &[]).path("File", "", "", PathKind::File, &[]);
    let ev = drop_event(&[f.home.join("a.txt")]).unwrap();
    form.focus = 0; // a plain field has the keyboard: the first path field takes it
    assert_eq!(form.handle_event(&ev), FormEvent::Changed(1));
    assert_eq!(form.value(1), f.home.to_string_lossy(), "the file's folder");
    assert_eq!(form.value(0), "", "the plain field is untouched");
    form.focus = 2;
    form.handle_event(&drop_event(&[f.home.join("b.csv")]).unwrap());
    assert_eq!(form.value(2), f.home.join("b.csv").to_string_lossy());
    assert!(drop_event(&[]).is_none());
    set_scope_override(None);
}

#[test]
fn browse_opens_the_picker_over_the_form_and_its_answer_fills_the_field() {
    let f = fs("browse");
    set_scope_override(Some(account_scope(&f, Role::User, true)));
    let mut form = Form::new("t").text("Name", "", "").path("File", "", "", PathKind::File, &["csv"]);
    let mut g = Gfx::new(1280, 720);
    form.render(&mut g, R::new(0.0, 100.0, 1280.0, 620.0), 1.0, [100, 200, 240, 255], true);
    let browse = g.hits.iter().find(|h| h.label == "Browse").cloned().expect("a Browse button");
    assert_eq!(browse.ev, Event::Tab(BROWSE_TAB + 1));
    form.handle_event(&browse.ev);
    assert!(form.picker.borrow().is_some());
    // Esc closes the picker, not the form
    assert_eq!(form.handle_event(&Event::Key(Key::Esc)), FormEvent::None);
    assert!(form.picker.borrow().is_none());
    form.handle_event(&Event::Key(Key::Ctrl('o')));
    assert!(form.picker.borrow().is_some(), "Ctrl+O browses too");
    let i = form.picker.borrow().as_ref().unwrap().visible().iter().position(|e| e.name == "b.csv").unwrap();
    form.handle_event(&Event::Row(i));
    assert_eq!(form.handle_event(&Event::Key(Key::Enter)), FormEvent::Changed(1));
    assert_eq!(form.value(1), f.home.join("b.csv").to_string_lossy());
    assert!(form.picker.borrow().is_none());
    set_scope_override(None);
}

#[test]
fn a_row_dragged_onto_the_location_line_fills_it() {
    let f = fs("drag");
    let mut p = picker(&f, PathKind::File, &[]);
    let mut g = Gfx::new(1280, 720);
    p.render(&mut g, R::new(0.0, 0.0, 1280.0, 720.0), 1.0, [100, 200, 240, 255], false);
    let row = g.hits.iter().find(|h| h.label == "row a.txt").cloned().unwrap();
    let loc = p.layout(&mut g, R::new(0.0, 0.0, 1280.0, 720.0), 1.0).loc;
    let (rx, ry) = ((row.rect.x + 20.0) as i32, (row.rect.y + 5.0) as i32);
    p.handle_event(&Event::MouseMove(rx, ry));
    p.handle_event(&row.ev);
    p.handle_event(&Event::MouseMove(rx + 100, ry - 200));
    p.handle_event(&Event::MouseUp((loc.x + 40.0) as i32, (loc.y + 8.0) as i32));
    assert_eq!(p.loc.value, f.home.join("a.txt").to_string_lossy());
    // a plain click (no movement) drops nothing
    p.loc.set("x");
    p.handle_event(&row.ev);
    p.handle_event(&Event::MouseUp((loc.x + 40.0) as i32, (loc.y + 8.0) as i32));
    assert_eq!(p.loc.value, "x");
}

fn many(f: &Fs, n: usize) {
    for i in 0..n {
        std::fs::write(f.home.join(format!("file{i:03}.txt")), "x").unwrap();
    }
}

#[test]
fn it_draws_at_every_size_and_in_split_panes_with_every_hit_inside_its_rect() {
    let f = fs("draw");
    many(&f, 60);
    let areas = [(360u32, 740u32, 1.0f32), (800, 480, 1.0), (1280, 720, 1.0), (1920, 1080, 1.5), (2560, 1600, 2.0)];
    for (w, h, s) in areas {
        for (label, area) in [("full", R::new(0.0, 0.0, w as f32, h as f32)), ("left half", R::new(0.0, 0.0, w as f32 / 2.0, h as f32)), ("right half", R::new(w as f32 / 2.0, 0.0, w as f32 / 2.0, h as f32))] {
            for kind in [PathKind::File, PathKind::Folder, PathKind::Save] {
                let mut p = picker(&f, kind, &[]);
                p.handle_event(&Event::Tab(TAB_BASE + 2));
                p.handle_event(&Event::Row(3));
                let mut g = Gfx::new(w, h);
                p.render(&mut g, area, s, [100, 200, 240, 255], true);
                assert!(!g.hits.is_empty());
                let l = p.layout(&mut g, area, s);
                for hit in &g.hits {
                    let r = hit.rect;
                    assert!(r.x >= area.x - 0.5 && r.y >= area.y - 0.5 && r.x + r.w <= area.x + area.w + 0.5 && r.y + r.h <= area.y + area.h + 0.5, "{w}x{h} {label} {kind:?}: `{}` {:?} outside {:?}", hit.label, r, area);
                    if hit.label != "backdrop" {
                        let q = l.panel;
                        assert!(r.x >= q.x - 0.5 && r.y >= q.y - 0.5 && r.x + r.w <= q.x + q.w + 0.5 && r.y + r.h <= q.y + q.h + 0.5, "{w}x{h} {label} {kind:?}: `{}` {:?} outside the dialog {:?}", hit.label, r, q);
                    }
                    if hit.label.starts_with("row ") {
                        assert!(r.y + 0.5 >= l.list.y && r.y + r.h <= l.list.y + l.list.h + 0.5, "{w}x{h} {label}: row `{}` outside the list", hit.label);
                    }
                }
                // a click in the middle of every control lands on that control (topmost hit)
                for hit in g.hits.iter().filter(|h| h.label != "backdrop" && h.rect.w > 0.0 && h.rect.h > 0.0) {
                    let (cx, cy) = ((hit.rect.x + hit.rect.w / 2.0) as i32, (hit.rect.y + hit.rect.h / 2.0) as i32);
                    let top = g.hits.iter().rev().find(|h| h.rect.contains(cx, cy)).unwrap();
                    assert_eq!(top.rect, hit.rect, "{w}x{h} {label}: `{}` is covered by `{}`", hit.label, top.label);
                }
                // and everything stays panic free at absurd sizes
            }
        }
    }
    for (w, h) in [(0.0f32, 0.0f32), (10.0, 10.0), (50.0, 3000.0), (3000.0, 20.0)] {
        let mut p = picker(&f, PathKind::File, &[]);
        let mut g = Gfx::new(64, 64);
        p.render(&mut g, R::new(0.0, 0.0, w, h), 1.0, [100, 200, 240, 255], false);
        p.handle_event(&Event::Scroll(-3));
        key(&mut p, Key::PageDown);
    }
}

#[test]
fn wheel_trackpad_scroll_bar_and_keys_move_a_long_list() {
    let f = fs("scroll");
    many(&f, 120);
    let mut p = picker(&f, PathKind::File, &[]);
    let area = R::new(0.0, 0.0, 1280.0, 720.0);
    let mut g = Gfx::new(1280, 720);
    p.render(&mut g, area, 1.0, [100, 200, 240, 255], false);
    assert!(p.list.scrolls(), "125 rows do not fit");
    let l = p.layout(&mut g, area, 1.0);
    p.handle_event(&Event::MouseMove((l.list.x + 50.0) as i32, (l.list.y + 50.0) as i32));
    p.handle_event(&Event::Scroll(-1));
    assert!(p.list.offset > 0.0, "the wheel scrolls over the list");
    let before = p.list.offset;
    p.handle_event(&Event::Scroll(1));
    assert!(p.list.offset < before);
    // the arrow keys carry the chosen row into view
    for _ in 0..40 {
        key(&mut p, Key::Down);
    }
    p.render(&mut g, area, 1.0, [100, 200, 240, 255], false);
    let i = p.visible().iter().position(|e| Some(&e.name) == p.sel.as_ref()).unwrap();
    assert!(i as f32 * 28.0 + 1.0 >= p.list.offset && (i + 1) as f32 * 28.0 <= p.list.offset + p.list.view.h + 1.0, "row {i} in view at {}", p.list.offset);
    key(&mut p, Key::End);
    p.render(&mut g, area, 1.0, [100, 200, 240, 255], false);
    assert!((p.list.offset - p.list.max).abs() < 1.0);
    // dragging the bar
    p.handle_event(&Event::AreaAt(AREA_LIST, 5.0, 5.0));
    p.handle_event(&Event::MouseMove(10, (l.list.y + 5.0) as i32));
    p.handle_event(&Event::MouseUp(10, 10));
    assert!(p.list.offset < p.list.max);
    // only the rows in view have regions (the list is virtual)
    let rows = g.hits.iter().filter(|h| h.label.starts_with("row ")).count();
    assert!(rows < 40, "{rows}");
}

#[test]
fn the_location_line_takes_typing_pasting_and_enter() {
    let f = fs("loc");
    let mut p = picker(&f, PathKind::File, &[]);
    key(&mut p, Key::Ctrl('l'));
    p.loc.set("");
    p.handle_event(&Event::Paste(format!("{}/docs", f.home.display())));
    key(&mut p, Key::Enter);
    assert_eq!(p.dir, f.home.join("docs"));
    key(&mut p, Key::Ctrl('l'));
    p.loc.set("/definitely/not/here");
    key(&mut p, Key::Enter);
    assert!(p.error.as_deref().unwrap().contains("does not exist"));
    assert_eq!(p.dir, f.home.join("docs"));
    let _ = Path::new("");
}
