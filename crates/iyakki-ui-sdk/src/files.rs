//! The files port: what an SDK app is handed so that it can embed a file browser (see [`crate::filesview::FilesView`]). The port is the
//! core's `Files` facet seen from the app: the person's own home folder, listed and read as text, nothing else. Paths are relative to the
//! home folder (`""` is the home itself) and the app never learns where the home really is. Like the other ports it carries no pixels and
//! opens no socket; [`MemFiles`] is the in-memory port for tests and the browser preview, following the same rules as the core (no `..`,
//! no hidden names, text only, a size limit).

use std::collections::BTreeMap;

/// The longest text a preview reads.
pub const MAX_PREVIEW: usize = 256 * 1024;
/// Most entries a listing carries (the core's own limit is the same).
pub const MAX_ENTRIES: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EntryInfo {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirListing {
    pub dir: String,
    pub entries: Vec<EntryInfo>,
    /// More were there than the port carries.
    pub truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FilesError {
    /// The core said no, in a sentence for the person.
    Refused(String),
    /// This host has no file browser (a preview, a core without one).
    Unavailable,
}

impl FilesError {
    pub fn sentence(&self) -> String {
        match self {
            FilesError::Refused(s) => s.clone(),
            FilesError::Unavailable => "The file browser is not available here.".into(),
        }
    }
}

/// A change the app asks the core for. Nothing changes until the person answers the question the core gives back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Change {
    MakeDir { dir: String, name: String },
    Rename { path: String, new_name: String },
    Trash { path: String },
    Save { path: String, text: String },
}

/// The core's question for a change: `summary` says in plain words what will happen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ask {
    pub call: String,
    pub summary: String,
}

pub trait FilesPort {
    /// The folder `dir` of the home (folders first, sorted; no hidden names).
    fn list(&mut self, dir: &str) -> Result<DirListing, FilesError>;
    /// The text of the file `path`, at most `max` bytes.
    fn read_text(&mut self, path: &str, max: usize) -> Result<String, FilesError>;
    /// Ask for a change: the answer is the question the person must decide. A host that changes no files says [`FilesError::Unavailable`].
    fn change(&mut self, _c: &Change) -> Result<Ask, FilesError> {
        Err(FilesError::Unavailable)
    }
    /// The person's answer to a waiting question: yes does it and says what happened, no drops it.
    fn decide(&mut self, _call: &str, _approve: bool) -> Result<String, FilesError> {
        Err(FilesError::Unavailable)
    }
}

/// The parent of `dir` (`""` for the home or a folder in it).
pub fn parent(dir: &str) -> String {
    dir.rsplit_once('/').map(|(p, _)| p.to_string()).unwrap_or_default()
}

pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_string() } else { format!("{dir}/{name}") }
}

fn plain(path: &str) -> bool {
    path.is_empty() || path.split('/').all(|n| !n.is_empty() && n != "." && n != ".." && !n.starts_with('.') && !n.chars().any(char::is_control))
}

/// An in-memory home folder: `put` a file (the folders above it appear). A port with no files at all is `MemFiles::unavailable()`.
#[derive(Default)]
pub struct MemFiles {
    nodes: BTreeMap<String, Option<String>>,
    off: bool,
    n: u32,
    waiting: Vec<(String, Change)>,
    /// The paths moved to the trash.
    pub trashed: Vec<String>,
}

impl MemFiles {
    pub fn new() -> MemFiles {
        MemFiles::default()
    }

    pub fn unavailable() -> MemFiles {
        MemFiles { off: true, ..MemFiles::default() }
    }

    pub fn text_of(&self, path: &str) -> Option<String> {
        self.nodes.get(path).and_then(|n| n.clone())
    }

    pub fn put(&mut self, path: &str, text: &str) {
        let mut p = parent(path);
        while !p.is_empty() {
            self.nodes.entry(p.clone()).or_insert(None);
            p = parent(&p);
        }
        self.nodes.insert(path.to_string(), Some(text.to_string()));
    }
}

impl FilesPort for MemFiles {
    fn list(&mut self, dir: &str) -> Result<DirListing, FilesError> {
        if self.off {
            return Err(FilesError::Unavailable);
        }
        if !plain(dir) || (!dir.is_empty() && !matches!(self.nodes.get(dir), Some(None))) {
            return Err(FilesError::Refused("that is not there".into()));
        }
        let mut entries: Vec<EntryInfo> = self
            .nodes
            .iter()
            .filter(|(p, _)| parent(p) == dir && !p.is_empty())
            .map(|(p, n)| EntryInfo { name: p.rsplit('/').next().unwrap_or(p).to_string(), is_dir: n.is_none(), size: n.as_ref().map_or(0, |t| t.len() as u64) })
            .filter(|e| !e.name.starts_with('.'))
            .collect();
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
        let truncated = entries.len() > MAX_ENTRIES;
        entries.truncate(MAX_ENTRIES);
        Ok(DirListing { dir: dir.into(), entries, truncated })
    }

    fn read_text(&mut self, path: &str, max: usize) -> Result<String, FilesError> {
        if self.off {
            return Err(FilesError::Unavailable);
        }
        if path.is_empty() || !plain(path) {
            return Err(FilesError::Refused("that is not there".into()));
        }
        match self.nodes.get(path) {
            Some(Some(t)) if t.len() > max => Err(FilesError::Refused(format!("that file is over {} KB, which is as much as can be shown", max / 1024))),
            Some(Some(t)) => Ok(t.clone()),
            Some(None) => Err(FilesError::Refused("that is a folder, not a file".into())),
            None => Err(FilesError::Refused("that file cannot be read".into())),
        }
    }

    fn change(&mut self, c: &Change) -> Result<Ask, FilesError> {
        if self.off {
            return Err(FilesError::Unavailable);
        }
        let bad = |m: &str| Err(FilesError::Refused(m.to_string()));
        let name_ok = |n: &str| !n.is_empty() && n.len() <= 255 && !n.contains('/') && !n.starts_with('.') && !n.chars().any(char::is_control);
        let summary = match c {
            Change::MakeDir { dir, name } if plain(dir) && name_ok(name) => format!("Make the folder `{name}` in {}", if dir.is_empty() { "your home folder".to_string() } else { format!("`{dir}`") }),
            Change::Rename { path, new_name } if !path.is_empty() && plain(path) && name_ok(new_name) => format!("Rename `{path}` to `{new_name}`"),
            Change::Trash { path } if !path.is_empty() && plain(path) => format!("Move `{path}` to the trash"),
            Change::Save { path, text } if !path.is_empty() && plain(path) && text.len() <= MAX_PREVIEW * 4 => format!("Save `{path}` ({} bytes), replacing it if it is there", text.len()),
            _ => return bad("that change is not allowed"),
        };
        if self.waiting.len() >= 4 {
            return bad("4 changes are already waiting for your answer");
        }
        self.n += 1;
        let call = format!("f{}", self.n);
        self.waiting.push((call.clone(), c.clone()));
        Ok(Ask { call, summary })
    }

    fn decide(&mut self, call: &str, approve: bool) -> Result<String, FilesError> {
        let Some(i) = self.waiting.iter().position(|(c, _)| c == call) else { return Err(FilesError::Refused("there is no such question waiting".into())) };
        let (_, c) = self.waiting.remove(i);
        if !approve {
            return Ok("Not changed: you said no.".into());
        }
        match c {
            Change::MakeDir { dir, name } => {
                let p = join(&dir, &name);
                if (!dir.is_empty() && !matches!(self.nodes.get(&dir), Some(None))) || self.nodes.contains_key(&p) {
                    return Err(FilesError::Refused("that cannot be made there".into()));
                }
                self.nodes.insert(p, None);
                Ok(format!("Made the folder `{name}`."))
            }
            Change::Rename { path, new_name } => {
                let to = join(&parent(&path), &new_name);
                if !self.nodes.contains_key(&path) || self.nodes.contains_key(&to) {
                    return Err(FilesError::Refused("that cannot be renamed".into()));
                }
                let prefix = format!("{path}/");
                let moved: Vec<(String, Option<String>)> = self.nodes.iter().filter(|(p, _)| **p == path || p.starts_with(&prefix)).map(|(p, n)| (p.clone(), n.clone())).collect();
                for (p, n) in moved {
                    self.nodes.remove(&p);
                    self.nodes.insert(format!("{to}{}", &p[path.len()..]), n);
                }
                Ok(format!("Renamed to `{new_name}`."))
            }
            Change::Trash { path } => {
                if !self.nodes.contains_key(&path) {
                    return Err(FilesError::Refused("that is not there".into()));
                }
                let prefix = format!("{path}/");
                self.nodes.retain(|p, _| *p != path && !p.starts_with(&prefix));
                self.trashed.push(path.clone());
                Ok(format!("Moved `{}` to the trash.", path.rsplit('/').next().unwrap_or(&path)))
            }
            Change::Save { path, text } => {
                let dir = parent(&path);
                if (!dir.is_empty() && !matches!(self.nodes.get(&dir), Some(None))) || matches!(self.nodes.get(&path), Some(None)) {
                    return Err(FilesError::Refused("that is not a place a file can be saved".into()));
                }
                self.nodes.insert(path.clone(), Some(text));
                Ok(format!("Saved `{}`.", path.rsplit('/').next().unwrap_or(&path)))
            }
        }
    }
}
