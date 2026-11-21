//! The writer window's own file picker (F-PKG-08): a plain folder browser, no picker library. It lists one folder (folders first, then
//! the image files: `.iso`, `.img`, `.raw`, `.bin`, `.iyakki`), goes up, enters a folder and returns the file chosen. All the logic is
//! here and tested on temporary folders; the window only draws [`Picker::entries`] and sends clicks.
// SPDX-License-Identifier: Apache-2.0

use std::path::{Path, PathBuf};

/// File name endings the picker shows (lower case).
pub const IMAGE_EXTENSIONS: [&str; 5] = ["iso", "img", "raw", "bin", "iyakki"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub is_dir: bool,
    /// Bytes of a file (0 for a folder).
    pub size: u64,
}

#[derive(Debug)]
pub struct Picker {
    dir: PathBuf,
    entries: Vec<Entry>,
    /// A plain sentence when the folder could not be read.
    pub error: Option<String>,
}

fn is_image(name: &str) -> bool {
    Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| IMAGE_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

impl Picker {
    /// Start in `dir` (the person's home or downloads folder, or a drive root).
    pub fn new(dir: &Path) -> Picker {
        let mut p = Picker { dir: dir.to_path_buf(), entries: Vec::new(), error: None };
        p.refresh();
        p
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Read the folder again: hidden names are left out, folders come first, each group is sorted without regard to case.
    pub fn refresh(&mut self) {
        self.error = None;
        self.entries.clear();
        let rd = match std::fs::read_dir(&self.dir) {
            Ok(r) => r,
            Err(e) => {
                self.error = Some(format!("{} cannot be opened: {e}", self.dir.display()));
                return;
            }
        };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let Ok(meta) = std::fs::metadata(e.path()) else { continue };
            if meta.is_dir() {
                self.entries.push(Entry { name, is_dir: true, size: 0 });
            } else if meta.is_file() && is_image(&name) {
                self.entries.push(Entry { name, is_dir: false, size: meta.len() });
            }
        }
        self.entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase())));
    }

    /// Go to the parent folder. False at the top.
    pub fn up(&mut self) -> bool {
        match self.dir.parent().map(Path::to_path_buf) {
            Some(p) if !p.as_os_str().is_empty() => {
                self.dir = p;
                self.refresh();
                true
            }
            _ => false,
        }
    }

    /// Open entry `i`: a folder is entered (None), a file is returned (its full path).
    pub fn open(&mut self, i: usize) -> Option<PathBuf> {
        let e = self.entries.get(i)?.clone();
        let path = self.dir.join(&e.name);
        if e.is_dir {
            self.dir = path;
            self.refresh();
            None
        } else {
            Some(path)
        }
    }

    /// Jump to a given folder (a drive root or a typed path).
    pub fn go(&mut self, dir: &Path) {
        self.dir = dir.to_path_buf();
        self.refresh();
    }
}

/// The places to start from: the home folder (and its Downloads) when known, then the roots (`/` or the existing drive letters).
pub fn start_places() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(h) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from) {
        let d = h.join("Downloads");
        if d.is_dir() {
            v.push(d);
        }
        v.push(h);
    }
    #[cfg(windows)]
    for l in b'A'..=b'Z' {
        let r = PathBuf::from(format!("{}:\\", l as char));
        if r.is_dir() {
            v.push(r);
        }
    }
    #[cfg(not(windows))]
    v.push(PathBuf::from("/"));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_picker_lists_folders_first_then_images_goes_into_a_folder_and_up_and_returns_a_file() {
        let d = crate::install::tests::tmp("filepick");
        std::fs::create_dir_all(d.join("Sub")).unwrap();
        std::fs::create_dir_all(d.join("alpha")).unwrap();
        std::fs::write(d.join("b.ISO"), vec![0u8; 10]).unwrap();
        std::fs::write(d.join("a.img"), vec![0u8; 5]).unwrap();
        std::fs::write(d.join("notes.txt"), b"x").unwrap();
        std::fs::write(d.join(".hidden.iso"), b"x").unwrap();
        std::fs::write(d.join("Sub").join("deep.iso"), b"y").unwrap();
        let mut p = Picker::new(&d);
        let names: Vec<(&str, bool)> = p.entries().iter().map(|e| (e.name.as_str(), e.is_dir)).collect();
        assert_eq!(names, [("alpha", true), ("Sub", true), ("a.img", false), ("b.ISO", false)]);
        assert_eq!(p.entries()[3].size, 10);
        // a folder is entered, a file is returned
        assert_eq!(p.open(1), None);
        assert_eq!(p.dir(), d.join("Sub"));
        assert_eq!(p.entries().len(), 1);
        assert_eq!(p.open(0), Some(d.join("Sub").join("deep.iso")));
        assert!(p.up() && p.dir() == d);
        assert_eq!(p.open(99), None);
        // an unreadable folder says so in words and lists nothing
        p.go(&d.join("missing"));
        assert!(p.error.as_deref().unwrap().contains("cannot be opened") && p.entries().is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn start_places_always_end_with_a_root() {
        assert!(!start_places().is_empty());
    }
}
