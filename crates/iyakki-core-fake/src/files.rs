//! Files of made-up homes for tests (the `Files` facet): `(user, path) -> entry`, with the same checks as the real one.

use std::collections::BTreeMap;
use std::sync::Mutex;

use iyakki_core_api::files::{check_change, check_rel, is_hidden, join, parent, sort_entries, ChangeBook, FileChange, FileEntry, Files, Listing, MAX_LIST, MAX_READ};

#[derive(Clone)]
enum Node {
    Dir,
    File(String),
}

#[derive(Default)]
pub struct FakeFileTree {
    nodes: Mutex<BTreeMap<(String, String), Node>>,
    book: ChangeBook,
    /// What was moved to the trash, `(user, old path)`.
    trash: Mutex<Vec<(String, String)>>,
}

fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl FakeFileTree {
    /// A file with text; the folders above it are made.
    pub fn put_file(&self, user: &str, path: &str, text: &str) {
        let mut g = lk(&self.nodes);
        let mut p = parent(path);
        while !p.is_empty() {
            g.entry((user.into(), p.clone())).or_insert(Node::Dir);
            p = parent(&p);
        }
        g.insert((user.into(), path.into()), Node::File(text.into()));
    }

    pub fn trashed(&self) -> Vec<(String, String)> {
        lk(&self.trash).clone()
    }

    pub fn text_of(&self, user: &str, path: &str) -> Option<String> {
        match lk(&self.nodes).get(&(user.to_string(), path.to_string())) {
            Some(Node::File(t)) => Some(t.clone()),
            _ => None,
        }
    }

    pub fn put_dir(&self, user: &str, path: &str) {
        lk(&self.nodes).insert((user.into(), path.into()), Node::Dir);
    }
}

impl Files for FakeFileTree {
    fn list(&self, user: &str, dir: &str) -> Result<Listing, String> {
        let names = check_rel(dir)?;
        if names.iter().any(|n| is_hidden(n)) {
            return Err("that is not there".into());
        }
        let g = lk(&self.nodes);
        if !dir.is_empty() && !matches!(g.get(&(user.to_string(), dir.to_string())), Some(Node::Dir)) {
            return Err("that is not there".into());
        }
        let mut entries: Vec<FileEntry> = g
            .iter()
            .filter(|((u, p), _)| u == user && parent(p) == dir && !p.is_empty() && p != dir)
            .filter_map(|((_, p), n)| {
                let name = p.rsplit('/').next().unwrap_or(p).to_string();
                (!is_hidden(&name)).then(|| match n {
                    Node::Dir => FileEntry { name, is_dir: true, size: 0, modified: Some(1) },
                    Node::File(t) => FileEntry { name, is_dir: false, size: t.len() as u64, modified: Some(1) },
                })
            })
            .collect();
        sort_entries(&mut entries);
        let truncated = entries.len() > MAX_LIST;
        entries.truncate(MAX_LIST);
        Ok(Listing { dir: dir.into(), entries, truncated })
    }

    fn read_text(&self, user: &str, path: &str, max: usize) -> Result<String, String> {
        let names = check_rel(path)?;
        if names.is_empty() || names.iter().any(|n| is_hidden(n)) {
            return Err("that is not there".into());
        }
        match lk(&self.nodes).get(&(user.to_string(), path.to_string())) {
            Some(Node::File(t)) if t.len() > max.min(MAX_READ) => Err(format!("that file is over {} KB, which is as much as can be shown", max.min(MAX_READ) / 1024)),
            Some(Node::File(t)) => Ok(t.clone()),
            Some(Node::Dir) => Err("that is a folder, not a file".into()),
            None => Err("that file cannot be read".into()),
        }
    }

    fn book(&self) -> Option<&ChangeBook> {
        Some(&self.book)
    }

    fn apply(&self, user: &str, c: &FileChange) -> Result<String, String> {
        check_change(c)?;
        let mut g = lk(&self.nodes);
        let key = |p: &str| (user.to_string(), p.to_string());
        let hidden = |p: &str| p.split('/').any(is_hidden);
        let dir_ok = |g: &BTreeMap<(String, String), Node>, d: &str| d.is_empty() || matches!(g.get(&(user.to_string(), d.to_string())), Some(Node::Dir));
        match c {
            FileChange::MakeDir { dir, name } => {
                if hidden(dir) || !dir_ok(&g, dir) {
                    return Err("that is not a folder you can use".into());
                }
                let p = join(dir, name);
                if g.contains_key(&key(&p)) {
                    return Err(format!("`{name}` is already there"));
                }
                g.insert(key(&p), Node::Dir);
                Ok(format!("Made the folder `{name}`."))
            }
            FileChange::Rename { path, new_name } => {
                if hidden(path) || !g.contains_key(&key(path)) {
                    return Err("that is not there".into());
                }
                let to = join(&parent(path), new_name);
                if g.contains_key(&key(&to)) {
                    return Err(format!("`{new_name}` is already there"));
                }
                let prefix = format!("{path}/");
                let moved: Vec<(String, Node)> = g.iter().filter(|((u, p), _)| u == user && (p == path || p.starts_with(&prefix))).map(|((_, p), n)| (p.clone(), n.clone())).collect();
                for (p, n) in moved {
                    g.remove(&key(&p));
                    g.insert(key(&format!("{to}{}", &p[path.len()..])), n);
                }
                Ok(format!("Renamed to `{new_name}`."))
            }
            FileChange::Trash { path } => {
                if hidden(path) || !g.contains_key(&key(path)) {
                    return Err("that is not there".into());
                }
                let prefix = format!("{path}/");
                g.retain(|(u, p), _| !(u == user && (p == path || p.starts_with(&prefix))));
                lk(&self.trash).push((user.to_string(), path.clone()));
                Ok(format!("Moved `{}` to the trash.", path.rsplit('/').next().unwrap_or(path)))
            }
            FileChange::Save { path, text } => {
                let dir = parent(path);
                if hidden(path) || !dir_ok(&g, &dir) || matches!(g.get(&key(path)), Some(Node::Dir)) {
                    return Err("that is not a place a file can be saved".into());
                }
                g.insert(key(path), Node::File(text.clone()));
                Ok(format!("Saved `{}`.", path.rsplit('/').next().unwrap_or(path)))
            }
        }
    }
}
