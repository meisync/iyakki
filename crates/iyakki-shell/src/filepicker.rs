//! The file picker: one reusable way to ask for a file or a folder, so nobody has to type a path.
//!
//! Two parts. [`FilePicker`] is a modal browser: a breadcrumb, Up and Home, a location line that can be edited or pasted into,
//! a filter box, a hidden-files switch, a New folder button, a list of folders and files with an icon, name, size and
//! modified date, columns that sort when clicked, the wheel and the trackpad (through [`ScrollList`]) and the keyboard.
//! The path field of a form (`Form::path` in `apps/widgets.rs`) is the other part: the ordinary text box (paste and correct
//! still work), a Browse button that opens the picker, and a live chip that says whether what is in the box is a file, a
//! folder, missing, the wrong kind or not yours to read ([`check`]).
//!
//! What it shows follows the account: a folder is listed only when the signed-in account's policy may read it (or it is a
//! shared host folder), so a guest sees nothing and a user sees their own folders. Files are read through `std::fs` in this
//! process only (no program is started). The only change it ever makes is a new folder, and only where the policy allows
//! writing.
//!
//! Drops: files the window receives are turned into a [`drop_event`] that fills the path field on screen; inside the picker a
//! row can also be dragged onto the location line.

use crate::safeclamp::SafeClamp;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use iyakki_agent::scope::FileRoots;
use iyakki_sec::policy::Policy;

use crate::gfx::{rgb, Face, Gfx};
use crate::scrolllist::ScrollList;
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

/// Clicks of the picker are `Event::Tab(TAB_BASE + n)`; the list's scroll bar is `Area(AREA_LIST)`; its text boxes are
/// `Focus(FOCUS_BASE + n)`; the Browse button of field `i` of a form is `Tab(BROWSE_TAB + i)`.
pub const TAB_BASE: usize = 3_500_000;
pub const BROWSE_TAB: usize = TAB_BASE + 800;
pub const AREA_LIST: usize = 3_501_900;
pub const FOCUS_BASE: usize = 3_502_000;
const T_UP: usize = 0;
const T_HOME: usize = 1;
const T_NEWDIR: usize = 2;
const T_HIDDEN: usize = 3;
const T_ACCEPT: usize = 4;
const T_CANCEL: usize = 5;
const T_CREATE: usize = 6;
const T_DRIVES: usize = 7;
const T_SORT: usize = 10;
const T_CRUMB: usize = 100;
const F_FILTER: usize = 0;
const F_LOC: usize = 1;
const F_NAME: usize = 2;

/// What a dropped path text starts with (see [`drop_event`]).
pub const DROP_PREFIX: &str = "\u{1}iyakki-drop:";

pub fn is_picker_tab(n: usize) -> bool {
    (TAB_BASE..TAB_BASE + 1000).contains(&n)
}

// ------------------------------------------------------------------------------------------------ who is asking

/// What the signed-in account may read and write.
#[derive(Clone, Debug)]
pub struct Scope {
    pub user: String,
    pub policy: Policy,
    pub roots: FileRoots,
}

impl Scope {
    pub fn new(user: &str, policy: &Policy, roots: FileRoots) -> Scope {
        Scope { user: user.into(), policy: policy.clone(), roots }
    }

    pub fn home(&self) -> &Path {
        &self.roots.home
    }

    /// May this (real) path be read?
    pub fn can_read(&self, real: &Path) -> bool {
        self.policy.allows_read(&self.user, &real.to_string_lossy()) || self.roots.mount_of(real).is_some()
    }

    pub fn can_write(&self, real: &Path) -> bool {
        self.roots.write_ok(&self.policy, &self.user, real)
    }

    /// `~`, `~/x` and relative paths (from home) made absolute; nothing is touched on disk.
    pub fn expand(&self, text: &str) -> PathBuf {
        let t = self.roots.expand(text);
        let p = PathBuf::from(&t);
        if p.is_absolute() { p } else { self.roots.home.join(p) }
    }
}

struct Session {
    user: String,
    policy: Policy,
    data_dir: PathBuf,
}

thread_local! {
    static SESSION: RefCell<Option<Session>> = const { RefCell::new(None) };
    static OVERRIDE: RefCell<Option<Scope>> = const { RefCell::new(None) };
    static ON_SCREEN: Cell<bool> = const { Cell::new(false) };
}

/// The shell tells the pickers who is signed in (and clears it at sign-out).
pub fn set_session(who: Option<(&str, &Policy, &Path)>) {
    SESSION.with(|s| *s.borrow_mut() = who.map(|(u, p, d)| Session { user: u.into(), policy: p.clone(), data_dir: d.to_path_buf() }));
}

/// Tests (and anything with its own rules) fix the scope directly.
pub fn set_scope_override(s: Option<Scope>) {
    OVERRIDE.with(|o| *o.borrow_mut() = s);
}

/// The scope of whoever is signed in; with nobody signed in, a scope that reads nothing.
pub fn current_scope() -> Scope {
    if let Some(s) = OVERRIDE.with(|o| o.borrow().clone()) {
        return s;
    }
    SESSION.with(|s| match &*s.borrow() {
        Some(se) => Scope::new(&se.user, &se.policy, FileRoots::new(&se.policy, &se.user, &se.data_dir)),
        None => {
            let none = Policy { fs_read: vec![], fs_write: vec![], ..iyakki_sec::policy::Role::Guest.template() };
            Scope::new("", &none, FileRoots { home: PathBuf::from("/"), roots: vec![], mounts: vec![] })
        }
    })
}

/// A form with a path field was drawn (the shell asks before it offers a drop to a field).
pub fn note_path_field() {
    ON_SCREEN.with(|c| c.set(true));
}

pub fn clear_path_field_note() {
    ON_SCREEN.with(|c| c.set(false));
}

pub fn path_field_on_screen() -> bool {
    ON_SCREEN.with(|c| c.get())
}

/// The event that carries dropped paths to the path field on screen (only the first path is used).
pub fn drop_event(paths: &[PathBuf]) -> Option<Event> {
    paths.first().map(|p| Event::Paste(format!("{DROP_PREFIX}{}", p.display())))
}

pub fn dropped_text(ev: &Event) -> Option<&str> {
    match ev {
        Event::Paste(t) => t.strip_prefix(DROP_PREFIX),
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------------ validating a path

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PathKind {
    File,
    Folder,
    /// A file to be written: it may not exist yet.
    Save,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Check {
    Empty,
    IsFile,
    IsFolder,
    Missing,
    /// The account may not read it (or, for a file to save, write there).
    NotAllowed,
    /// A file to save that does not exist yet, in a folder that does.
    NewFile,
    /// Saving over a file that exists.
    Replaces,
    WrongKind(&'static str),
    BadExtension(String),
}

/// What is in the box: exists, a file or a folder, allowed, the right kind and extension.
pub fn check(scope: &Scope, kind: PathKind, exts: &[String], text: &str) -> Check {
    let t = text.trim();
    if t.is_empty() {
        return Check::Empty;
    }
    let p = scope.expand(t);
    let real = std::fs::canonicalize(&p).ok();
    if kind == PathKind::Save {
        let parent_ok = |p: &Path| p.parent().and_then(|d| std::fs::canonicalize(d).ok()).filter(|d| d.is_dir());
        return match real {
            Some(r) if r.is_dir() => Check::WrongKind("a folder"),
            Some(r) if !scope.can_write(&r) => Check::NotAllowed,
            Some(_) => Check::Replaces,
            None => match parent_ok(&p) {
                Some(d) if scope.can_write(&d.join(p.file_name().unwrap_or_default())) => Check::NewFile,
                Some(_) => Check::NotAllowed,
                None => Check::Missing,
            },
        };
    }
    let Some(real) = real else { return Check::Missing };
    if !scope.can_read(&real) {
        return Check::NotAllowed;
    }
    match (kind, real.is_dir()) {
        (PathKind::Folder, true) => Check::IsFolder,
        (PathKind::Folder, false) => Check::WrongKind("a file"),
        (_, true) => Check::WrongKind("a folder"),
        (_, false) => {
            let name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if ext_ok(exts, &name) { Check::IsFile } else { Check::BadExtension(exts.join(", ")) }
        }
    }
}

/// The chip's words and whether they are good news (true), a warning (None) or a problem (false).
pub fn chip(c: &Check, kind: PathKind) -> (String, Option<bool>) {
    match c {
        Check::Empty => (String::new(), None),
        Check::IsFile => ("file found".into(), Some(true)),
        Check::IsFolder => ("folder found".into(), Some(true)),
        Check::Missing => (if kind == PathKind::Save { "folder does not exist".into() } else { "does not exist".into() }, Some(false)),
        Check::NotAllowed => ("your account may not use it".into(), Some(false)),
        Check::NewFile => ("new file".into(), Some(true)),
        Check::Replaces => ("replaces an existing file".into(), None),
        Check::WrongKind(what) => (format!("that is {what}, not {}", if kind == PathKind::Folder { "a folder" } else { "a file" }), Some(false)),
        Check::BadExtension(e) => (format!("needs a {e} file"), Some(false)),
    }
}

fn ext_ok(exts: &[String], name: &str) -> bool {
    if exts.is_empty() {
        return true;
    }
    let low = name.to_lowercase();
    exts.iter().any(|e| low.ends_with(&format!(".{}", e.trim_start_matches('.').to_lowercase())))
}

// ------------------------------------------------------------------------------------------------ listing

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified: Option<SystemTime>,
}

/// The entries of a folder that the account may read (a link is followed and judged by where it leads).
pub fn list_dir(scope: &Scope, dir: &Path) -> Result<Vec<Entry>, String> {
    if !scope.can_read(dir) {
        return Err(format!("{} is not a folder your account may read.", dir.display()));
    }
    let rd = std::fs::read_dir(dir).map_err(|e| iyakki_agent::scope::explain_io(&e, dir, "list"))?;
    let mut out = Vec::new();
    for e in rd.flatten().take(20_000) {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(meta) = std::fs::metadata(e.path()) else { continue };
        if e.file_type().map(|t| t.is_symlink()).unwrap_or(false) {
            match std::fs::canonicalize(e.path()) {
                Ok(real) if scope.can_read(&real) => {}
                _ => continue,
            }
        }
        out.push(Entry { name, is_dir: meta.is_dir(), size: if meta.is_dir() { 0 } else { meta.len() }, modified: meta.modified().ok() });
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortBy {
    Name,
    Size,
    Modified,
}

/// Folders first, then by the chosen column; ties by name.
pub fn sort_entries(v: &mut [Entry], by: SortBy, ascending: bool) {
    v.sort_by(|a, b| {
        let dirs = b.is_dir.cmp(&a.is_dir);
        let by_name = a.name.to_lowercase().cmp(&b.name.to_lowercase());
        let main = match by {
            SortBy::Name => by_name,
            SortBy::Size if a.is_dir && b.is_dir => by_name,
            SortBy::Size => a.size.cmp(&b.size).then(by_name),
            SortBy::Modified => a.modified.cmp(&b.modified).then(by_name),
        };
        dirs.then(if ascending { main } else { main.reverse() })
    });
}

pub fn human_size(b: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let (mut v, mut i) = (b as f64, 0);
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 { format!("{b} B") } else { format!("{v:.1} {}", U[i]) }
}

/// `2026-10-22 14:05` (UTC).
pub fn date_text(t: Option<SystemTime>) -> String {
    let Some(t) = t else { return String::new() };
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

// ------------------------------------------------------------------------------------------------ the picker

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    List,
    Filter,
    Location,
    NewName,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickOut {
    Stay,
    Cancel,
    Chosen(PathBuf),
}

#[derive(Clone, Debug)]
pub struct FilePicker {
    pub kind: PathKind,
    pub exts: Vec<String>,
    pub title: String,
    pub dir: PathBuf,
    entries: Vec<Entry>,
    pub error: Option<String>,
    pub sort: SortBy,
    pub ascending: bool,
    pub show_hidden: bool,
    pub filter: TextField,
    pub loc: TextField,
    pub name: TextField,
    pub creating: bool,
    focus: Focus,
    pub sel: Option<String>,
    pub list: ScrollList,
    scope: Scope,
    drag: Option<(String, (i32, i32), bool)>,
    loc_rect: R,
    reveal: bool,
}

/// The rectangles of the picker, for drawing and for tests.
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub panel: R,
    pub buttons: Vec<(usize, String, R)>,
    pub filter: R,
    pub crumb: R,
    pub loc: R,
    pub header: R,
    pub list: R,
    pub newrow: R,
    pub status: R,
}

const ROW_H: f32 = 28.0;

impl FilePicker {
    pub fn new(kind: PathKind, exts: &[String], title: &str, start: &str, scope: Scope) -> FilePicker {
        let want = scope.expand(start);
        // start in the folder named, or the folder of the file named, or the nearest folder above that exists and is readable
        let mut probe = Some(want.clone());
        let mut dir = None;
        let mut pre = None;
        while let Some(p) = probe {
            if let Ok(real) = std::fs::canonicalize(&p) {
                if real.is_dir() && scope.can_read(&real) {
                    dir = Some(real);
                    break;
                } else if real.is_file() {
                    pre = real.file_name().map(|n| n.to_string_lossy().into_owned());
                    probe = real.parent().map(Path::to_path_buf);
                    continue;
                }
            }
            probe = p.parent().map(Path::to_path_buf);
        }
        let dir = dir.unwrap_or_else(|| scope.home().to_path_buf());
        let mut name = TextField::new("file name", false);
        name.max = 255;
        if kind == PathKind::Save {
            let n = pre.clone().or_else(|| want.file_name().filter(|_| !want.is_dir()).map(|n| n.to_string_lossy().into_owned()));
            name.set(&n.unwrap_or_default());
        }
        let mut p = FilePicker {
            kind,
            exts: exts.to_vec(),
            title: title.into(),
            dir: dir.clone(),
            entries: vec![],
            error: None,
            sort: SortBy::Name,
            ascending: true,
            show_hidden: false,
            filter: TextField::new("type to filter", false),
            loc: TextField::new("", false),
            name,
            creating: false,
            focus: Focus::List,
            sel: pre,
            list: ScrollList::default(),
            scope,
            drag: None,
            loc_rect: R::default(),
            reveal: true,
        };
        p.loc.max = 4096;
        p.filter.max = 200;
        p.go(dir);
        p
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Go to a folder (when the account may read it); a refusal is said in plain words and the picker stays where it was.
    pub fn go(&mut self, to: PathBuf) -> bool {
        let real = std::fs::canonicalize(&to).unwrap_or(to);
        match list_dir(&self.scope, &real) {
            Ok(mut v) => {
                sort_entries(&mut v, self.sort, self.ascending);
                self.entries = v;
                self.dir = real;
                self.error = None;
                self.sel = None;
                self.filter.set("");
                self.loc.set(&self.dir.to_string_lossy());
                self.list.to_top();
                self.reveal = true;
                true
            }
            Err(e) => {
                self.error = Some(e);
                false
            }
        }
    }

    fn reload(&mut self) {
        if let Ok(mut v) = list_dir(&self.scope, &self.dir) {
            sort_entries(&mut v, self.sort, self.ascending);
            self.entries = v;
        }
    }

    pub fn parent(&self) -> Option<PathBuf> {
        self.dir.parent().map(Path::to_path_buf).filter(|p| self.scope.can_read(p))
    }

    /// The rows shown: hidden names, the filter and (for files) the extension decide; folders always stay.
    pub fn visible(&self) -> Vec<&Entry> {
        let f = self.filter.value.trim().to_lowercase();
        self.entries
            .iter()
            .filter(|e| (self.show_hidden || !e.name.starts_with('.')) && (f.is_empty() || e.name.to_lowercase().contains(&f)))
            .collect()
    }

    /// Can this row be chosen with Open? (Files that do not fit the filter, or any file when a folder is wanted, cannot.)
    fn pickable(&self, e: &Entry) -> bool {
        match self.kind {
            PathKind::Folder => e.is_dir,
            _ => e.is_dir || ext_ok(&self.exts, &e.name),
        }
    }

    fn sel_index(&self) -> Option<usize> {
        let name = self.sel.as_ref()?;
        self.visible().iter().position(|e| &e.name == name)
    }

    fn select_index(&mut self, i: usize) {
        let name = self.visible().get(i).map(|e| e.name.clone());
        if let Some(n) = name {
            if self.kind == PathKind::Save && !self.entries.iter().any(|e| e.name == n && e.is_dir) {
                self.name.set(&n);
            }
            self.sel = Some(n);
            self.reveal = true;
        }
    }

    fn move_sel(&mut self, by: isize) {
        let n = self.visible().len();
        if n == 0 {
            return;
        }
        let cur = self.sel_index().map_or(if by >= 0 { -1 } else { n as isize }, |i| i as isize);
        let next = (cur + by).clamp_safe(0, n as isize - 1) as usize;
        self.select_index(next);
    }

    fn sort_by(&mut self, by: SortBy) {
        if self.sort == by {
            self.ascending = !self.ascending;
        } else {
            self.sort = by;
            self.ascending = true;
        }
        self.reload();
    }

    /// Open the chosen row: a folder is entered, a file is answered.
    fn open_sel(&mut self) -> PickOut {
        let Some(name) = self.sel.clone() else { return self.accept() };
        let Some(e) = self.entries.iter().find(|e| e.name == name).cloned() else { return PickOut::Stay };
        if e.is_dir {
            self.go(self.dir.join(&e.name));
            return PickOut::Stay;
        }
        self.accept()
    }

    /// The Open / Select / Save button.
    pub fn accept(&mut self) -> PickOut {
        let sel = self.sel.as_ref().and_then(|n| self.entries.iter().find(|e| &e.name == n)).cloned();
        let out = match self.kind {
            PathKind::Folder => match sel {
                Some(e) if e.is_dir => self.dir.join(&e.name),
                _ => self.dir.clone(),
            },
            PathKind::File => match sel {
                Some(e) if !e.is_dir && ext_ok(&self.exts, &e.name) => self.dir.join(&e.name),
                Some(e) if e.is_dir => {
                    self.go(self.dir.join(&e.name));
                    return PickOut::Stay;
                }
                Some(_) => {
                    self.error = Some(format!("Choose a {} file.", self.exts.join(" / ")));
                    return PickOut::Stay;
                }
                None => {
                    self.error = Some("Choose a file first.".into());
                    return PickOut::Stay;
                }
            },
            PathKind::Save => {
                let n = self.name.value.trim().to_string();
                if n.is_empty() || n.contains('/') || n == "." || n == ".." {
                    self.error = Some("Type a file name (no slashes).".into());
                    return PickOut::Stay;
                }
                self.dir.join(n)
            }
        };
        if self.kind == PathKind::Save && !self.scope.can_write(&out) {
            self.error = Some("Your account may not write there.".into());
            return PickOut::Stay;
        }
        PickOut::Chosen(out)
    }

    fn make_folder(&mut self) {
        let n = self.name.value.trim().to_string();
        let target = self.dir.join(&n);
        if n.is_empty() || n.contains('/') || n == "." || n == ".." {
            self.error = Some("Type a folder name (no slashes).".into());
        } else if !self.scope.can_write(&target) {
            self.error = Some("Your account may not make folders here.".into());
        } else if target.exists() {
            self.error = Some(format!("{n} exists already."));
        } else {
            match std::fs::create_dir(&target) {
                Ok(()) => {
                    self.creating = false;
                    self.name.set("");
                    self.reload();
                    self.error = None;
                    self.sel = Some(n);
                    self.reveal = true;
                    self.focus = Focus::List;
                }
                Err(e) => self.error = Some(iyakki_agent::scope::explain_io(&e, &target, "write")),
            }
        }
    }

    /// The location line was confirmed (or text was pasted/dropped into it): a folder is entered, a file is selected.
    fn go_to_text(&mut self, text: &str) -> PickOut {
        let p = self.scope.expand(text);
        let Ok(real) = std::fs::canonicalize(&p) else {
            self.error = Some(format!("{} does not exist.", p.display()));
            return PickOut::Stay;
        };
        if real.is_dir() {
            self.go(real);
            self.focus = Focus::List;
            return PickOut::Stay;
        }
        let name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(parent) = real.parent().map(Path::to_path_buf) {
            if self.go(parent) {
                self.sel = Some(name);
                self.focus = Focus::List;
                self.reveal = true;
                if self.kind == PathKind::File {
                    return self.accept();
                }
            }
        }
        PickOut::Stay
    }

    fn type_into_filter(&mut self, k: &Key) {
        self.filter.handle(k);
        self.list.to_top();
        // keep the chosen row only when it is still shown
        if self.sel_index().is_none() {
            self.sel = None;
        }
    }

    pub fn handle_event(&mut self, ev: &Event) -> PickOut {
        // wheel, trackpad and scroll bar
        if self.list.event(ev, AREA_LIST) {
            return PickOut::Stay;
        }
        match ev {
            Event::Key(k) => return self.key(k),
            Event::Paste(t) => {
                if let Some(d) = dropped_text(ev) {
                    self.loc.set(d);
                    return self.go_to_text(&d.to_string());
                }
                match self.focus {
                    Focus::Filter => {
                        self.filter.paste(t);
                        self.list.to_top();
                    }
                    Focus::NewName => {
                        self.name.paste(t);
                    }
                    Focus::Location | Focus::List => {
                        // a pasted path goes to the location line, whatever had the keyboard
                        self.focus = Focus::Location;
                        self.loc.set("");
                        self.loc.paste(t);
                    }
                }
            }
            Event::FocusAt(id, x) if *id >= FOCUS_BASE && *id < FOCUS_BASE + 10 => match id - FOCUS_BASE {
                F_FILTER => {
                    self.focus = Focus::Filter;
                    self.filter.click(*x);
                }
                F_LOC => {
                    self.focus = Focus::Location;
                    self.loc.click(*x);
                }
                _ => {
                    self.focus = Focus::NewName;
                    self.name.click(*x);
                }
            },
            Event::Row(i) => {
                self.focus = Focus::List;
                self.select_index(*i);
                let at = self.list.pointer.unwrap_or((0, 0));
                if let Some(n) = self.sel.clone() {
                    self.drag = Some((n, at, false));
                }
            }
            Event::MouseMove(x, y) => {
                if let Some((_, from, moved)) = self.drag.as_mut() {
                    if (*x - from.0).abs() > 8 || (*y - from.1).abs() > 8 {
                        *moved = true;
                    }
                }
            }
            Event::MouseUp(x, y) => {
                if let Some((name, _, true)) = self.drag.take() {
                    if self.loc_rect.contains(*x, *y) {
                        let p = self.dir.join(name);
                        self.loc.set(&p.to_string_lossy());
                        self.focus = Focus::Location;
                    }
                }
            }
            Event::Tab(n) if is_picker_tab(*n) => return self.tab(*n - TAB_BASE),
            _ => {}
        }
        PickOut::Stay
    }

    fn tab(&mut self, n: usize) -> PickOut {
        match n {
            T_UP => match self.parent() {
                Some(p) => {
                    let from = self.dir.file_name().map(|n| n.to_string_lossy().into_owned());
                    self.go(p);
                    self.sel = from;
                    self.reveal = true;
                }
                None => self.error = Some("There is nothing above this that your account may read.".into()),
            },
            T_HOME => {
                self.go(self.scope.home().to_path_buf());
            }
            T_NEWDIR => {
                self.creating = !self.creating;
                self.focus = if self.creating { Focus::NewName } else { Focus::List };
                self.name.set("");
            }
            T_CREATE => self.make_folder(),
            // the drives that are open: DISKS opens them in the person's own home, in `media`
            T_DRIVES => {
                let media = self.scope.home().join("media");
                if !media.is_dir() || std::fs::read_dir(&media).map_or(true, |mut d| d.next().is_none()) {
                    self.error = Some("No USB drive or card is open. Open one in the DISKS tile.".into());
                } else if self.scope.can_read(&media) {
                    self.go(media);
                } else {
                    self.error = Some("Your account may not read the drives folder.".into());
                }
            }
            T_HIDDEN => {
                self.show_hidden = !self.show_hidden;
                self.list.to_top();
            }
            T_ACCEPT => return self.accept(),
            T_CANCEL => return PickOut::Cancel,
            T_SORT => self.sort_by(SortBy::Name),
            11 => self.sort_by(SortBy::Size),
            12 => self.sort_by(SortBy::Modified),
            n if n >= T_CRUMB => {
                let comps: Vec<_> = self.dir.components().collect();
                let upto = (n - T_CRUMB + 1).min(comps.len());
                let p: PathBuf = comps[..upto].iter().collect();
                if self.scope.can_read(&p) {
                    self.go(p);
                } else {
                    self.error = Some("Your account may not read that folder.".into());
                }
            }
            _ => {}
        }
        PickOut::Stay
    }

    fn key(&mut self, k: &Key) -> PickOut {
        match k {
            Key::Esc => {
                if self.creating {
                    self.creating = false;
                    self.focus = Focus::List;
                    return PickOut::Stay;
                }
                return PickOut::Cancel;
            }
            Key::Tab | Key::BackTab => {
                let order = if self.creating { [Focus::List, Focus::Filter, Focus::Location, Focus::NewName] } else { [Focus::List, Focus::Filter, Focus::Location, Focus::List] };
                let at = order.iter().position(|f| *f == self.focus).unwrap_or(0);
                let n = if self.creating { 4 } else { 3 };
                self.focus = order[if matches!(k, Key::Tab) { (at + 1) % n } else { (at + n - 1) % n }];
                return PickOut::Stay;
            }
            Key::Ctrl('l') => {
                self.focus = Focus::Location;
                return PickOut::Stay;
            }
            Key::Ctrl('f') => {
                self.focus = Focus::Filter;
                return PickOut::Stay;
            }
            Key::Ctrl('h') => {
                self.show_hidden = !self.show_hidden;
                return PickOut::Stay;
            }
            Key::Ctrl('n') => return self.tab(T_NEWDIR),
            _ => {}
        }
        match self.focus {
            Focus::Location => match k {
                Key::Enter => {
                    let t = self.loc.value.clone();
                    return self.go_to_text(&t);
                }
                Key::Down => self.focus = Focus::List,
                other => {
                    self.loc.handle(other);
                }
            },
            Focus::NewName => match k {
                Key::Enter if self.creating => self.make_folder(),
                Key::Enter => return self.accept(),
                other => {
                    self.name.handle(other);
                }
            },
            Focus::Filter => match k {
                Key::Enter | Key::Down => self.focus = Focus::List,
                other => self.type_into_filter(other),
            },
            Focus::List => {
                let page = ((self.list.view.h / self.list.row.max(1.0)) as isize - 1).max(1);
                match k {
                    Key::Up => self.move_sel(-1),
                    Key::Down => self.move_sel(1),
                    Key::PageUp => self.move_sel(-page),
                    Key::PageDown => self.move_sel(page),
                    Key::Home => self.move_sel(isize::MIN / 2),
                    Key::End => self.move_sel(isize::MAX / 2),
                    Key::Enter => return self.open_sel(),
                    Key::Backspace | Key::Left => {
                        return self.tab(T_UP);
                    }
                    Key::Char(c) if !c.is_control() => {
                        self.focus = Focus::Filter;
                        self.type_into_filter(k);
                    }
                    _ => {}
                }
            }
        }
        PickOut::Stay
    }

    // -------------------------------------------------------------------------------------------- layout and drawing

    /// Where everything goes in `area` at scale `s`. Never panics at any size (sizes only shrink to zero).
    pub fn layout(&self, g: &mut Gfx, area: R, s: f32) -> Layout {
        let pw = (900.0 * s).min(area.w - 16.0 * s).max(0.0);
        let ph = (640.0 * s).min(area.h - 16.0 * s).max(0.0);
        let panel = R::new(area.x + (area.w - pw) / 2.0, area.y + (area.h - ph) / 2.0, pw, ph);
        let pad = 16.0 * s;
        let ix = panel.x + pad;
        let iw = (pw - 2.0 * pad).max(0.0);
        let right = ix + iw;
        let mut y = panel.y + 54.0 * s;
        let size = 12.0 * s;
        let bh = size * 2.1;
        let mut buttons = Vec::new();
        let mut x = ix;
        let mut line_h = 30.0 * s;
        let labels = vec![(T_UP, "Up".to_string()), (T_HOME, "Home".to_string()), (T_NEWDIR, "New folder".to_string()), (T_DRIVES, "Drives".to_string()), (T_HIDDEN, if self.show_hidden { "Hidden: shown".to_string() } else { "Hidden: off".to_string() })];
        for (id, label) in labels {
            let w = g.text_width(size, &label, Face::Bold) + size * 1.7;
            if x + w > right && x > ix {
                x = ix;
                y += line_h;
            }
            buttons.push((id, label, R::new(x, y + (line_h - bh) / 2.0, w, bh)));
            x += w + 6.0 * s;
        }
        let room = right - x;
        let filter = if room >= 130.0 * s {
            R::new(x, y, room, 30.0 * s)
        } else {
            y += line_h;
            R::new(ix, y, iw, 30.0 * s)
        };
        line_h = 34.0 * s;
        y += line_h;
        let crumb = R::new(ix, y, iw, 24.0 * s);
        y += 28.0 * s;
        let loc = R::new(ix, y, iw, 32.0 * s);
        y += 36.0 * s;
        let header = R::new(ix, y, iw, 24.0 * s);
        y += 24.0 * s;
        let foot = 44.0 * s;
        let foot_top = panel.y + panel.h - foot - 6.0 * s;
        let want_row = self.creating || self.kind == PathKind::Save;
        let new_h = if want_row { 36.0 * s } else { 0.0 };
        let list = R::new(ix, y.min(foot_top), iw, (foot_top - new_h - y).max(0.0));
        let newrow = if want_row { R::new(ix, list.y + list.h + 4.0 * s, iw, 30.0 * s) } else { R::new(ix, list.y + list.h, iw, 0.0) };
        let status = R::new(ix, foot_top, iw, foot);
        Layout { panel, buttons, filter, crumb, loc, header, list, newrow, status }
    }

    fn accept_label(&self) -> &'static str {
        match self.kind {
            PathKind::File => "Open",
            PathKind::Folder => "Select folder",
            PathKind::Save => "Save",
        }
    }

    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32, accent: [u8; 4], blink: bool) {
        let l = self.layout(g, area, s);
        // the dimmed screen behind can't be clicked; a click on it does nothing
        g.clear_hits();
        g.rect(area.x, area.y, area.w, area.h, [3, 5, 14, 200]);
        g.hit(area, Event::Tick, "backdrop");
        crate::fx::modal_panel(g, l.panel, s, accent, false);
        g.hit(l.panel, Event::Tick, "backdrop");
        crate::fx::modal_title(g, l.panel, s, &self.title, accent);
        let size = 12.0 * s;
        // toolbar
        for (id, label, r) in &l.buttons {
            let on = *id == T_HIDDEN && self.show_hidden || *id == T_NEWDIR && self.creating;
            let hot = g.hovered(*r);
            crate::fx::pill(g, *r, size / 14.0, if on { accent } else { MUTED }, hot, true);
            g.text(r.x + size * 0.85, r.y + (r.h - g.line_height(size, Face::Bold)) / 2.0, size, TEXT, label, Face::Bold);
            g.hit(*r, Event::Tab(TAB_BASE + id), label);
        }
        self.filter.render(g, l.filter, self.focus == Focus::Filter, s, blink, Face::Sans, FOCUS_BASE + F_FILTER);
        self.draw_crumbs(g, l.crumb, s, accent);
        // the location line is also where a dragged row can be dropped
        self.loc_rect = l.loc;
        let dragging = self.drag.as_ref().is_some_and(|d| d.2);
        self.loc.render(g, l.loc, self.focus == Focus::Location, s, blink, Face::Sans, FOCUS_BASE + F_LOC);
        if dragging {
            g.rrect_outline(l.loc.x, l.loc.y, l.loc.w, l.loc.h, 6.0 * s, 2.0 * s, accent);
        }
        self.draw_list(g, &l, s, accent);
        if l.newrow.h > 0.0 {
            let label = if self.creating { "Folder name" } else { "File name" };
            let lw = g.text_width(12.0 * s, label, Face::Bold) + 10.0 * s;
            g.text(l.newrow.x, l.newrow.y + 8.0 * s, 12.0 * s, MUTED, label, Face::Bold);
            let bw = if self.creating { g.text_width(size, "Create", Face::Bold) + size * 1.7 } else { 0.0 };
            let bx = (l.newrow.x + l.newrow.w - bw * 1.0).max(l.newrow.x);
            let fx = (l.newrow.x + lw).min(bx);
            let fr = R::new(fx, l.newrow.y, (bx - fx - if self.creating { 6.0 * s } else { 0.0 }).max(0.0), l.newrow.h);
            if !self.creating {
                let fr2 = R::new(fx, l.newrow.y, (l.newrow.x + l.newrow.w - fx).max(0.0), l.newrow.h);
                self.name.render(g, fr2, self.focus == Focus::NewName, s, blink, Face::Sans, FOCUS_BASE + F_NAME);
            } else {
                self.name.render(g, fr, self.focus == Focus::NewName, s, blink, Face::Sans, FOCUS_BASE + F_NAME);
                g.button(bx, l.newrow.y + 2.0 * s, size, "Create", accent, Event::Tab(TAB_BASE + T_CREATE));
            }
        }
        // footer: what is chosen or what went wrong, then the buttons
        // the two buttons: the biggest size and the fullest words that fit side by side
        let full = self.accept_label();
        let short = match self.kind {
            PathKind::Folder => "Select",
            _ => full,
        };
        let mut pick = (11.0f32, short);
        'find: for sz in [14.0f32, 12.0, 11.0] {
            for lab in [full, short] {
                let w = g.text_width(sz * s, lab, Face::Bold) + sz * 1.7 * s + g.text_width(sz * s, "Cancel", Face::Bold) + sz * 1.7 * s + 8.0 * s;
                if w <= l.status.w {
                    pick = (sz, lab);
                    break 'find;
                }
            }
        }
        let (bs, lab) = pick;
        let aw = g.text_width(bs * s, lab, Face::Bold) + bs * 1.7 * s;
        let cw = g.text_width(bs * s, "Cancel", Face::Bold) + bs * 1.7 * s;
        let by = l.status.y + (l.status.h - bs * 2.1 * s) / 2.0;
        let ax = (l.status.x + l.status.w - aw).max(l.status.x);
        let cx = (ax - cw - 8.0 * s).max(l.status.x);
        g.button(ax, by, bs * s, lab, accent, Event::Tab(TAB_BASE + T_ACCEPT));
        g.button(cx, by, bs * s, "Cancel", MUTED, Event::Tab(TAB_BASE + T_CANCEL));
        let (msg, col) = match (&self.error, self.sel.as_ref()) {
            (Some(e), _) => (e.clone(), ERR),
            (None, Some(n)) => (n.clone(), MUTED),
            _ => (format!("{} item{}", self.visible().len(), if self.visible().len() == 1 { "" } else { "s" }), DIM),
        };
        let msg = g.fit(13.0 * s, &msg, Face::Sans, (cx - l.status.x - 8.0 * s).max(0.0));
        g.text(l.status.x, by + 6.0 * s, 13.0 * s, col, &msg, Face::Sans);
        // a dragged row follows the pointer
        if let (Some((name, _, true)), Some((px, py))) = (&self.drag, self.list.pointer) {
            let w = g.text_width(13.0 * s, name, Face::Bold) + 16.0 * s;
            let x = (px as f32 + 12.0 * s).min(l.panel.x + l.panel.w - w).max(l.panel.x);
            let y = (py as f32 + 6.0 * s).min(l.panel.y + l.panel.h - 24.0 * s).max(l.panel.y);
            g.rrect(x, y, w, 22.0 * s, 6.0 * s, crate::fx::with_alpha(accent, 200));
            g.text(x + 8.0 * s, y + 3.0 * s, 13.0 * s, rgb(10, 12, 20), name, Face::Bold);
        }
    }

    fn draw_crumbs(&self, g: &mut Gfx, r: R, s: f32, accent: [u8; 4]) {
        let comps: Vec<String> = self.dir.components().map(|c| match c {
            std::path::Component::RootDir => "/".to_string(),
            other => other.as_os_str().to_string_lossy().into_owned(),
        }).collect();
        let size = 13.0 * s;
        let widths: Vec<f32> = comps.iter().map(|c| g.text_width(size, c, Face::Bold) + 14.0 * s).collect();
        // drop the leading parts that do not fit; the last ones are the ones that matter
        let mut first = 0;
        while first + 1 < comps.len() && widths[first..].iter().sum::<f32>() + if first > 0 { 20.0 * s } else { 0.0 } > r.w {
            first += 1;
        }
        let mut x = r.x;
        if first > 0 {
            g.text(x, r.y + 3.0 * s, size, DIM, "…", Face::Bold);
            x += 20.0 * s;
        }
        for (i, c) in comps.iter().enumerate().skip(first) {
            let w = widths[i].min((r.x + r.w - x).max(0.0));
            if w <= 0.0 {
                break;
            }
            let cell = R::new(x, r.y, w, r.h);
            let last = i + 1 == comps.len();
            if g.hovered(cell) {
                g.rrect(cell.x, cell.y, cell.w, cell.h, 5.0 * s, rgb(40, 50, 86));
            }
            let t = g.fit(size, c, Face::Bold, (w - 6.0 * s).max(0.0));
            g.text(x + 4.0 * s, r.y + 3.0 * s, size, if last { accent } else { TEXT }, &t, Face::Bold);
            g.hit(cell, Event::Tab(TAB_BASE + T_CRUMB + i), &format!("crumb {c}"));
            x += w;
        }
    }

    fn draw_list(&mut self, g: &mut Gfx, l: &Layout, s: f32, accent: [u8; 4]) {
        let (list, head) = (l.list, l.header);
        let mod_w = if list.w >= 520.0 * s { 150.0 * s } else { 0.0 };
        let size_w = if list.w >= 300.0 * s { 84.0 * s } else { 0.0 };
        let bar = 16.0 * s;
        let name_x = list.x + 30.0 * s;
        let size_x = list.x + (list.w - bar - mod_w - size_w).max(0.0);
        let mod_x = list.x + (list.w - bar - mod_w).max(0.0);
        // header cells sort
        g.rect(head.x, head.y, head.w, head.h, rgb(22, 28, 50));
        let arrow = if self.ascending { " ▲" } else { " ▼" };
        let cells = [(T_SORT, SortBy::Name, "Name", name_x - 4.0 * s, (size_x - name_x).max(0.0)), (11, SortBy::Size, "Size", size_x, size_w), (12, SortBy::Modified, "Modified", mod_x, mod_w)];
        for (id, by, label, x, w) in cells {
            if w <= 0.0 {
                continue;
            }
            let r = R::new(x, head.y, w.min((head.x + head.w - x).max(0.0)), head.h);
            let t = format!("{label}{}", if self.sort == by { arrow } else { "" });
            let t = g.fit(12.0 * s, &t, Face::Bold, (r.w - 6.0 * s).max(0.0));
            g.text(r.x + 4.0 * s, r.y + 5.0 * s, 12.0 * s, if self.sort == by { accent } else { MUTED }, &t, Face::Bold);
            g.hit(r, Event::Tab(TAB_BASE + id), &format!("sort {label}"));
        }
        // rows
        let row = ROW_H * s;
        let rows: Vec<Entry> = self.visible().into_iter().cloned().collect();
        self.list.layout(list, rows.len() as f32 * row, row, s);
        if self.reveal {
            if let Some(i) = self.sel_index() {
                self.list.show(i as f32 * row, (i + 1) as f32 * row);
            }
            self.reveal = false;
        }
        g.rect(list.x, list.y, list.w, list.h, rgb(14, 18, 34));
        let off = self.list.offset;
        let sel = self.sel.clone();
        let strip = if self.list.scrolls() { bar } else { 0.0 };
        g.clipped(list, |g| {
            if rows.is_empty() {
                let msg = if self.error.is_some() { "This folder cannot be shown." } else if self.entries.is_empty() { "This folder is empty." } else { "Nothing matches the filter." };
                g.text(list.x + 12.0 * s, list.y + 10.0 * s, 14.0 * s, DIM, msg, Face::Sans);
            }
            let first = (off / row).floor().max(0.0) as usize;
            let last = (((off + list.h) / row).ceil().max(0.0) as usize).min(rows.len());
            for i in first..last {
                let e = &rows[i];
                let y = list.y + i as f32 * row - off;
                let r = R::new(list.x, y, (list.w - strip).max(0.0), row);
                let on = sel.as_deref() == Some(e.name.as_str());
                let pick = self.pickable(e);
                if on {
                    g.rrect(r.x, r.y + 1.0, r.w, r.h - 2.0, 5.0 * s, crate::fx::with_alpha(accent, 70));
                } else if g.hovered(r) {
                    g.rrect(r.x, r.y + 1.0, r.w, r.h - 2.0, 5.0 * s, rgb(30, 38, 66));
                }
                draw_icon(g, list.x + 8.0 * s, y + 5.0 * s, s, e.is_dir);
                let col = if pick { TEXT } else { DIM };
                let nw = (size_x - name_x - 8.0 * s).max(0.0);
                let t = g.fit(14.0 * s, &e.name, Face::Sans, nw);
                g.text(name_x, y + 5.0 * s, 14.0 * s, col, &t, Face::Sans);
                if size_w > 0.0 && !e.is_dir {
                    let t = human_size(e.size);
                    let w = g.text_width(12.0 * s, &t, Face::Sans);
                    g.text((size_x + size_w - 8.0 * s - w).max(size_x), y + 7.0 * s, 12.0 * s, MUTED, &t, Face::Sans);
                }
                if mod_w > 0.0 {
                    g.text(mod_x + 4.0 * s, y + 7.0 * s, 12.0 * s, MUTED, &date_text(e.modified), Face::Sans);
                }
                g.hit(r, Event::Row(i), &format!("row {}", e.name));
            }
        });
        self.list.draw_bar(g, s, AREA_LIST, accent);
    }
}

fn draw_icon(g: &mut Gfx, x: f32, y: f32, s: f32, dir: bool) {
    if dir {
        g.rrect(x, y, 8.0 * s, 4.0 * s, 1.5 * s, rgb(214, 168, 70));
        g.rrect(x, y + 3.0 * s, 16.0 * s, 12.0 * s, 2.0 * s, rgb(240, 192, 84));
    } else {
        g.rrect(x + 2.0 * s, y, 12.0 * s, 15.0 * s, 2.0 * s, rgb(190, 200, 228));
        g.rect(x + 4.0 * s, y + 5.0 * s, 8.0 * s, 1.2 * s, rgb(110, 120, 150));
        g.rect(x + 4.0 * s, y + 8.0 * s, 8.0 * s, 1.2 * s, rgb(110, 120, 150));
    }
}

/// The chip beside a path field's label: a dot and a few words.
pub fn draw_chip(g: &mut Gfx, right: f32, y: f32, s: f32, c: &Check, kind: PathKind) {
    let (text, good) = chip(c, kind);
    if text.is_empty() {
        return;
    }
    let col = match good {
        Some(true) => OK,
        Some(false) => ERR,
        None => rgb(238, 200, 100),
    };
    let w = g.text_width(12.0 * s, &text, Face::Bold);
    let x = (right - w).max(0.0);
    g.circle(x - 8.0 * s, y + 7.0 * s, 3.5 * s, col);
    g.text(x, y, 12.0 * s, col, &text, Face::Bold);
}
