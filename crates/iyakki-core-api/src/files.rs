//! The files facet an app may embed (docs/development/UI-SDK.md, "Embedding the file browser"): a person's own home folder, listed and
//! read through the core. The same policy as `markdown.openFile` (`HomeFiles`): the root is the person's home and nothing outside it is
//! ever listed or read; a link is never followed or listed; hidden names are skipped; names are relative (`a/b.txt`, `""` is the home
//! itself) and an app never sees where the home really is. Version 1 reads: listing a folder and reading a text file. Version 2 changes files
//! (a new folder, a rename, a delete to the trash, a save) and every change goes through the core's approval: [`Files::change`] never changes
//! anything, it asks; the person's answer comes through [`Files::decide`], and only an approved change runs.
//!
//! Every implementation (the real one, the fake of the tests, a remote client) uses the checks below, so a request that breaks a limit is
//! refused with the same sentence everywhere.

use serde::{Deserialize, Serialize};

pub const FILES_FACET_VERSION: u32 = 2;

/// Most entries of one folder a listing carries (the rest is said with `truncated`), the biggest text read, and the limits of a path.
pub const MAX_LIST: usize = 500;
pub const MAX_READ: usize = 1 << 20;
pub const MAX_PATH: usize = 1024;
pub const MAX_NAME: usize = 255;
pub const MAX_DEPTH: usize = 24;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Seconds since 1970 when the file was last changed, when the core knows.
    pub modified: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Listing {
    /// The folder listed, relative to the home folder (`""` is the home itself).
    pub dir: String,
    /// Folders first, then files, each sorted by name without regard to case.
    pub entries: Vec<FileEntry>,
    /// More entries were there than [`MAX_LIST`].
    pub truncated: bool,
}

/// The sentence for a core that has no files.
pub const NO_FILES: &str = "this core has no file browser";

/// A relative path made of plain names: `""` or `a/b/c`. No `..`, no `.`, no leading or doubled `/`, no control characters, no name
/// longer than [`MAX_NAME`], at most [`MAX_DEPTH`] names and [`MAX_PATH`] characters. Returns the names.
pub fn check_rel(path: &str) -> Result<Vec<&str>, String> {
    if path.is_empty() {
        return Ok(Vec::new());
    }
    if path.len() > MAX_PATH || path.chars().any(char::is_control) {
        return Err(format!("a path is at most {MAX_PATH} characters without control characters"));
    }
    let names: Vec<&str> = path.split('/').collect();
    if names.len() > MAX_DEPTH {
        return Err(format!("that path is more than {MAX_DEPTH} folders deep"));
    }
    for n in &names {
        if n.is_empty() || *n == "." || *n == ".." || n.len() > MAX_NAME {
            return Err("that is not a path inside your home folder".into());
        }
    }
    Ok(names)
}

/// `dir` and `name` together (`dir` may be empty).
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

/// The folder above `dir` (`""` for the home or a folder directly in it).
pub fn parent(dir: &str) -> String {
    dir.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_default()
}

/// Sort the way a listing promises: folders first, then files, by name without regard to case (ties by the exact name).
pub fn sort_entries(v: &mut [FileEntry]) {
    v.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())).then_with(|| a.name.cmp(&b.name)));
}

/// A name that is shown and skipped when it starts with a dot (hidden files stay hidden).
pub fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

/// One change an app asks for. Paths are relative to the home like everywhere else; a `name` is one plain name (no `/`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileChange {
    /// A new folder `name` inside `dir`.
    MakeDir { dir: String, name: String },
    /// The file or folder `path` gets the name `new_name` (in the same folder).
    Rename { path: String, new_name: String },
    /// The file or folder `path` goes to the person's trash (it is moved, not erased; an administrator can get it back).
    Trash { path: String },
    /// The text file `path` gets `text` (a new file, or the old one replaced).
    Save { path: String, text: String },
    /// The file `path` gets `bytes` (a file a connected client device sent, at most [`MAX_UPLOAD`]; a new file, or the old one replaced). Added for the
    /// client page's file transfer (F-CLIENT-01); it asks like every other change.
    SaveBytes { path: String, bytes: Vec<u8> },
}

/// What the core says to a change: it waits for the person.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeAsk {
    pub call: String,
    /// Plain words for the card: what will happen.
    pub summary: String,
}

/// The most changes one person may have waiting at once, and the biggest text a save carries.
pub const MAX_PENDING: usize = 4;
pub const MAX_SAVE: usize = MAX_READ;
/// The biggest file a client device may send, and the biggest the core hands to one (the client page's file transfer).
pub const MAX_UPLOAD: usize = 8 * 1024 * 1024;

/// A plain name for a new or renamed entry: 1 to [`MAX_NAME`] characters, no `/`, no control characters, not `.` or `..`, not hidden.
pub fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > MAX_NAME || name.contains('/') || name.chars().any(char::is_control) || name == "." || name == ".." {
        return Err(format!("a name is 1 to {MAX_NAME} characters without `/` or control characters"));
    }
    if is_hidden(name) {
        return Err("a name that starts with a dot is hidden: choose another".into());
    }
    Ok(())
}

/// The checks every implementation makes before it asks: paths and names are plain and the save fits. Err is the sentence.
pub fn check_change(c: &FileChange) -> Result<(), String> {
    match c {
        FileChange::MakeDir { dir, name } => {
            check_rel(dir)?;
            check_name(name)
        }
        FileChange::Rename { path, new_name } => {
            if check_rel(path)?.is_empty() {
                return Err("the home folder itself cannot be renamed".into());
            }
            check_name(new_name)
        }
        FileChange::Trash { path } => {
            if check_rel(path)?.is_empty() {
                return Err("the home folder itself cannot be deleted".into());
            }
            Ok(())
        }
        FileChange::Save { path, text } => {
            let names = check_rel(path)?;
            match names.last() {
                None => return Err("name a file to save".into()),
                Some(n) => check_name(n)?,
            }
            if text.len() > MAX_SAVE {
                return Err(format!("a saved file is at most {} KB", MAX_SAVE / 1024));
            }
            Ok(())
        }
        FileChange::SaveBytes { path, bytes } => {
            let names = check_rel(path)?;
            match names.last() {
                None => return Err("name a file to save".into()),
                Some(n) => check_name(n)?,
            }
            if bytes.len() > MAX_UPLOAD {
                return Err(format!("a file from a client is at most {} MB", MAX_UPLOAD / (1024 * 1024)));
            }
            Ok(())
        }
    }
}

/// The card's words for a change (names only: a saved file's text is never in them).
pub fn describe(c: &FileChange) -> String {
    let at = |d: &str| if d.is_empty() { "your home folder".to_string() } else { format!("`{d}`") };
    match c {
        FileChange::MakeDir { dir, name } => format!("Make the folder `{name}` in {}", at(dir)),
        FileChange::Rename { path, new_name } => format!("Rename `{path}` to `{new_name}`"),
        FileChange::Trash { path } => format!("Move `{path}` to the trash"),
        FileChange::Save { path, text } => format!("Save `{path}` ({} bytes), replacing it if it is there", text.len()),
        FileChange::SaveBytes { path, bytes } => format!("Save `{path}` ({} bytes sent from a connected device), replacing it if it is there", bytes.len()),
    }
}

/// The changes waiting for a person's answer, kept by a core that implements [`Files::change`]. One book serves every person; a call answers
/// only for the person it was asked of.
#[derive(Default)]
pub struct ChangeBook {
    inner: std::sync::Mutex<(u64, Vec<(String, String, FileChange)>)>,
}

impl ChangeBook {
    /// Keep `c` for `user` and make the question. Err when the person already has [`MAX_PENDING`] waiting.
    pub fn ask(&self, user: &str, c: &FileChange) -> Result<ChangeAsk, String> {
        let mut g = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if g.1.iter().filter(|(u, _, _)| u == user).count() >= MAX_PENDING {
            return Err(format!("{MAX_PENDING} changes are already waiting for your answer"));
        }
        g.0 += 1;
        let call = format!("f{}", g.0);
        g.1.push((user.to_string(), call.clone(), c.clone()));
        Ok(ChangeAsk { call, summary: describe(c) })
    }

    /// Take the change `call` of `user` out (it is answered once).
    pub fn take(&self, user: &str, call: &str) -> Option<FileChange> {
        let mut g = self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let i = g.1.iter().position(|(u, c, _)| u == user && c == call)?;
        Some(g.1.remove(i).2)
    }

    pub fn waiting(&self, user: &str) -> usize {
        self.inner.lock().unwrap_or_else(std::sync::PoisonError::into_inner).1.iter().filter(|(u, _, _)| u == user).count()
    }
}

/// The person's files as an app may reach them. Every call names the signed-in person. The defaults say there is no file browser, so a core
/// that has none (a preview, a remote client without it) refuses with a sentence instead of failing.
pub trait Files: Send + Sync {
    /// The folder `dir` of the person's home (see [`check_rel`]); at most [`MAX_LIST`] entries, no hidden names, no links.
    fn list(&self, _user: &str, _dir: &str) -> Result<Listing, String> {
        Err(NO_FILES.into())
    }
    /// The text of the file `path` (at most `max` bytes, never more than [`MAX_READ`]; valid UTF-8 only). A larger file is refused with a
    /// sentence, never cut.
    fn read_text(&self, _user: &str, _path: &str, _max: usize) -> Result<String, String> {
        Err(NO_FILES.into())
    }
    /// The bytes of the file `path` (at most `max`, never more than [`MAX_UPLOAD`]), by the same walk as [`Files::read_text`]: no link, nothing hidden,
    /// inside the home. A larger file is refused with a sentence, never cut.
    fn read_bytes(&self, _user: &str, _path: &str, _max: usize) -> Result<Vec<u8>, String> {
        Err(NO_FILES.into())
    }
    /// The book of waiting changes of this core (`None`: this core changes no files).
    fn book(&self) -> Option<&ChangeBook> {
        None
    }
    /// Do an approved change. Only [`Files::decide`] calls it, after the person said yes; an implementation checks again (paths, links, hidden
    /// names, sizes) because the app is not trusted. Returns the sentence for the person.
    fn apply(&self, _user: &str, _change: &FileChange) -> Result<String, String> {
        Err(NO_FILES.into())
    }
    /// Ask for a change. Nothing changes: the reply is the question the person must answer, and [`Files::decide`] answers it.
    fn change(&self, user: &str, change: &FileChange) -> Result<ChangeAsk, String> {
        check_change(change)?;
        self.book().ok_or_else(|| NO_FILES.to_string())?.ask(user, change)
    }
    /// The person's answer to a waiting change: yes runs it (and says what happened), no drops it. A call that is not waiting for this person
    /// is refused.
    fn decide(&self, user: &str, call: &str, approve: bool) -> Result<String, String> {
        let book = self.book().ok_or_else(|| NO_FILES.to_string())?;
        let c = book.take(user, call).ok_or("there is no such question waiting")?;
        if !approve {
            return Ok("Not changed: you said no.".into());
        }
        check_change(&c)?;
        self.apply(user, &c)
    }
}

/// The files of a core that keeps none.
pub struct NoFiles;

impl Files for NoFiles {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_plain_names_inside_the_home() {
        assert_eq!(check_rel("").unwrap(), Vec::<&str>::new());
        assert_eq!(check_rel("a/b.txt").unwrap(), vec!["a", "b.txt"]);
        for bad in ["/a", "a/", "a//b", "..", "a/../b", ".", "./a", "a\u{7}b", &"x/".repeat(30), &"y".repeat(256)] {
            assert!(check_rel(bad).is_err(), "{bad:?}");
        }
        assert!(check_rel(&"z".repeat(MAX_PATH + 1)).is_err());
    }

    #[test]
    fn join_parent_and_order() {
        assert_eq!((join("", "a"), join("a", "b"), parent("a/b"), parent("a"), parent("")), ("a".into(), "a/b".into(), "a".into(), String::new(), String::new()));
        let f = |n: &str, d: bool| FileEntry { name: n.into(), is_dir: d, size: 0, modified: None };
        let mut v = vec![f("b.txt", false), f("Zed", true), f("a.txt", false), f("alpha", true)];
        sort_entries(&mut v);
        assert_eq!(v.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["alpha", "Zed", "a.txt", "b.txt"]);
        assert!(NoFiles.list("u", "").unwrap_err().contains("no file browser"));
        assert!(NoFiles.change("u", &FileChange::Trash { path: "a".into() }).is_err());
        assert!(NoFiles.read_text("u", "a", 10).is_err());
    }

    #[test]
    fn changes_are_checked_described_by_name_and_kept_until_answered_once() {
        let mk = |d: &str, n: &str| FileChange::MakeDir { dir: d.into(), name: n.into() };
        assert!(check_change(&mk("", "new")).is_ok() && check_change(&mk("a/b", "x y")).is_ok());
        for bad in [mk("", ""), mk("", "a/b"), mk("", ".hidden"), mk("", ".."), mk("../x", "n"), mk("", &"n".repeat(300))] {
            assert!(check_change(&bad).is_err(), "{bad:?}");
        }
        assert!(check_change(&FileChange::Rename { path: String::new(), new_name: "x".into() }).is_err());
        assert!(check_change(&FileChange::Trash { path: String::new() }).is_err());
        assert!(check_change(&FileChange::Save { path: "a.txt".into(), text: "x".repeat(MAX_SAVE + 1) }).is_err());
        assert!(check_change(&FileChange::Save { path: "d/.k".into(), text: String::new() }).is_err());
        let save = FileChange::Save { path: "notes/a.txt".into(), text: "secret words".into() };
        assert_eq!(describe(&save), "Save `notes/a.txt` (12 bytes), replacing it if it is there");
        assert!(!describe(&save).contains("secret"));
        let book = ChangeBook::default();
        let a = book.ask("ann", &save).unwrap();
        assert_eq!(book.waiting("ann"), 1);
        assert_eq!(book.take("bob", &a.call), None, "another person cannot answer it");
        assert_eq!(book.take("ann", &a.call), Some(save.clone()));
        assert_eq!(book.take("ann", &a.call), None, "answered once");
        for _ in 0..MAX_PENDING {
            book.ask("ann", &save).unwrap();
        }
        assert!(book.ask("ann", &save).unwrap_err().contains("already waiting"));
        assert!(book.ask("bob", &save).is_ok());
    }

    #[test]
    fn a_file_from_a_client_is_a_change_like_any_other_with_its_own_limit_and_a_card_that_names_only_the_name_and_the_size() {
        let ok = FileChange::SaveBytes { path: "in/photo.jpg".into(), bytes: vec![0xFF; 100] };
        assert!(check_change(&ok).is_ok());
        assert_eq!(describe(&ok), "Save `in/photo.jpg` (100 bytes sent from a connected device), replacing it if it is there");
        assert!(check_change(&FileChange::SaveBytes { path: "a".into(), bytes: vec![0; MAX_UPLOAD + 1] }).is_err());
        assert!(check_change(&FileChange::SaveBytes { path: "a".into(), bytes: vec![0; MAX_UPLOAD] }).is_ok());
        for bad in ["", "../x", "/etc/x", "d/.hidden", "a//b"] {
            assert!(check_change(&FileChange::SaveBytes { path: bad.into(), bytes: vec![1] }).is_err(), "{bad}");
        }
        // the old text save keeps its smaller limit
        assert!(check_change(&FileChange::Save { path: "a.txt".into(), text: "x".repeat(MAX_SAVE + 1) }).is_err());
        // a core that has no files hands out no bytes
        assert!(NoFiles.read_bytes("u", "a", 10).is_err());
        assert_eq!(serde_json::from_str::<FileChange>(&serde_json::to_string(&ok).unwrap()).unwrap(), ok);
    }
}
