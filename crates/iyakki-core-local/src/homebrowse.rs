//! Listing and reading the person's home folder for an embedded file browser (the `Files` facet of `iyakki-core-api`, F-SDK-100). Same
//! policy as `homefiles` (the Markdown list): the root is the home folder and nothing outside it is listed or read; no step of a path may be
//! a link; hidden names are skipped; names are relative and the app never sees the real place. `root` is given by the caller (the agent
//! computes it from the account), so this file only walks a folder.

use std::path::{Path, PathBuf};

use iyakki_core_api::files::{check_name, check_rel, is_hidden, sort_entries, FileChange, FileEntry, Listing, MAX_LIST, MAX_READ, MAX_SAVE};

/// The real folder for the relative `rel` under `root`, with every step checked: present, not a link.
fn walk(root: &Path, rel: &str) -> Result<PathBuf, String> {
    let names = check_rel(rel)?;
    let mut p = PathBuf::from(root);
    for n in names {
        if is_hidden(n) {
            return Err("that is not there".into());
        }
        p.push(n);
        match std::fs::symlink_metadata(&p) {
            Ok(m) if !m.file_type().is_symlink() => {}
            Ok(_) => return Err("that is a link, which is not followed".into()),
            Err(_) => return Err("that is not there".into()),
        }
    }
    Ok(p)
}

pub fn list(root: &Path, dir: &str) -> Result<Listing, String> {
    let p = walk(root, dir)?;
    let rd = std::fs::read_dir(&p).map_err(|_| "that is not a folder you can open".to_string())?;
    let mut entries = Vec::new();
    let mut truncated = false;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if is_hidden(&name) {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() || !(ft.is_dir() || ft.is_file()) {
            continue;
        }
        if entries.len() >= MAX_LIST * 2 {
            truncated = true;
            break;
        }
        let meta = e.metadata().ok();
        entries.push(FileEntry {
            name,
            is_dir: ft.is_dir(),
            size: if ft.is_file() { meta.as_ref().map_or(0, |m| m.len()) } else { 0 },
            modified: meta.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs()),
        });
    }
    sort_entries(&mut entries);
    if entries.len() > MAX_LIST {
        entries.truncate(MAX_LIST);
        truncated = true;
    }
    Ok(Listing { dir: dir.to_string(), entries, truncated })
}

pub fn read_text(root: &Path, path: &str, max: usize) -> Result<String, String> {
    if path.is_empty() {
        return Err("name a file".into());
    }
    let max = max.min(MAX_READ);
    let p = walk(root, path)?;
    let meta = std::fs::metadata(&p).map_err(|_| "that file cannot be read".to_string())?;
    if !meta.is_file() {
        return Err("that is a folder, not a file".into());
    }
    if meta.len() > max as u64 {
        return Err(format!("that file is over {} KB, which is as much as can be shown", max / 1024));
    }
    String::from_utf8(std::fs::read(&p).map_err(|_| "that file cannot be read".to_string())?).map_err(|_| "that file is not text".into())
}

/// The real place of a folder that must exist (every step checked, no link, nothing hidden) and be a folder.
fn folder(root: &Path, dir: &str) -> Result<PathBuf, String> {
    let p = walk(root, dir)?;
    match std::fs::symlink_metadata(&p) {
        Ok(m) if m.is_dir() => Ok(p),
        _ => Err("that is not a folder you can use".into()),
    }
}

/// Do an approved change under `root`; `trash` is the folder trashed things are moved into (made if missing, outside the home). Every
/// check of the request is made again here: the app and the card are not trusted. Returns the sentence for the person.
pub fn apply(root: &Path, trash: &Path, c: &FileChange) -> Result<String, String> {
    iyakki_core_api::files::check_change(c)?;
    match c {
        FileChange::MakeDir { dir, name } => {
            let p = folder(root, dir)?.join(name);
            if std::fs::symlink_metadata(&p).is_ok() {
                return Err(format!("`{name}` is already there"));
            }
            std::fs::create_dir(&p).map_err(|_| "the folder could not be made".to_string())?;
            Ok(format!("Made the folder `{name}`."))
        }
        FileChange::Rename { path, new_name } => {
            let from = walk(root, path)?;
            let to = from.parent().ok_or("that cannot be renamed")?.join(new_name);
            if std::fs::symlink_metadata(&to).is_ok() {
                return Err(format!("`{new_name}` is already there"));
            }
            std::fs::rename(&from, &to).map_err(|_| "it could not be renamed".to_string())?;
            Ok(format!("Renamed to `{new_name}`."))
        }
        FileChange::Trash { path } => {
            let from = walk(root, path)?;
            std::fs::create_dir_all(trash).map_err(|_| "the trash could not be opened".to_string())?;
            let name = path.rsplit('/').next().unwrap_or(path);
            let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let mut to = trash.join(format!("{stamp}-{name}"));
            let mut n = 1;
            while std::fs::symlink_metadata(&to).is_ok() {
                to = trash.join(format!("{stamp}-{n}-{name}"));
                n += 1;
            }
            std::fs::rename(&from, &to).map_err(|_| "it could not be moved to the trash".to_string())?;
            Ok(format!("Moved `{name}` to the trash."))
        }
        FileChange::Save { path, text } => {
            if text.len() > MAX_SAVE {
                return Err("that is too big to save".into());
            }
            let (dir, name) = path.rsplit_once('/').map_or(("", path.as_str()), |(d, n)| (d, n));
            check_name(name)?;
            let target = folder(root, dir)?.join(name);
            match std::fs::symlink_metadata(&target) {
                Ok(m) if m.file_type().is_symlink() => return Err("that is a link, which is not followed".into()),
                Ok(m) if !m.is_file() => return Err("that is a folder, not a file".into()),
                _ => {}
            }
            // written beside it and moved over it, so a failed write leaves the old file whole
            let tmp = target.with_file_name(format!(".{name}.saving"));
            std::fs::write(&tmp, text).map_err(|_| "the file could not be written".to_string())?;
            std::fs::rename(&tmp, &target).map_err(|_| {
                let _ = std::fs::remove_file(&tmp);
                "the file could not be written".to_string()
            })?;
            Ok(format!("Saved `{name}`."))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-homebrowse-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("Docs/deep")).unwrap();
        std::fs::create_dir_all(d.join(".hidden")).unwrap();
        std::fs::write(d.join("b.txt"), "bee").unwrap();
        std::fs::write(d.join("A.txt"), "ay").unwrap();
        std::fs::write(d.join(".dot"), "x").unwrap();
        std::fs::write(d.join("Docs/note.md"), "# n").unwrap();
        d
    }

    #[test]
    fn a_folder_lists_folders_first_without_hidden_names_and_a_file_reads_as_text() {
        let d = tree("list");
        let l = list(&d, "").unwrap();
        assert_eq!(l.entries.iter().map(|e| (e.name.as_str(), e.is_dir)).collect::<Vec<_>>(), [("Docs", true), ("A.txt", false), ("b.txt", false)]);
        assert_eq!(l.entries[2].size, 3);
        assert!(l.entries[2].modified.is_some() && !l.truncated);
        assert_eq!(list(&d, "Docs").unwrap().entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["deep", "note.md"]);
        assert_eq!(read_text(&d, "Docs/note.md", 1000).unwrap(), "# n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn nothing_outside_the_home_is_reached_by_a_dot_dot_a_link_a_hidden_name_a_big_file_or_binary() {
        let d = tree("evil");
        let outside = std::env::temp_dir().join(format!("iyakki-homebrowse-out-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("s.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(&outside, d.join("out")).unwrap();
        std::os::unix::fs::symlink(outside.join("s.txt"), d.join("s.txt")).unwrap();
        assert!(!list(&d, "").unwrap().entries.iter().any(|e| e.name == "out" || e.name == "s.txt"), "links are not listed");
        for bad in ["..", "../x", "/etc", "out", "out/s.txt", "s.txt", "Docs/../b.txt", "nope", ".hidden"] {
            assert!(list(&d, bad).is_err(), "{bad}");
            assert!(read_text(&d, bad, 1000).is_err(), "{bad}");
        }
        assert!(read_text(&d, ".dot", 1000).is_err(), "hidden files are not read");
        assert!(read_text(&d, "Docs", 1000).unwrap_err().contains("folder"));
        std::fs::write(d.join("big.txt"), vec![b'x'; 5000]).unwrap();
        std::fs::write(d.join("bin.dat"), [0xff, 0xfe, 0x00]).unwrap();
        assert!(read_text(&d, "big.txt", 4096).unwrap_err().contains("over 4 KB"));
        assert!(read_text(&d, "bin.dat", 4096).unwrap_err().contains("not text"));
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn a_big_folder_is_cut_and_says_so() {
        let d = tree("big");
        for i in 0..(MAX_LIST + 20) {
            std::fs::write(d.join(format!("f{i:04}.txt")), "").unwrap();
        }
        let l = list(&d, "").unwrap();
        assert_eq!(l.entries.len(), MAX_LIST);
        assert!(l.truncated);
        let _ = std::fs::remove_dir_all(&d);
    }

    fn change_tree(n: &str) -> (PathBuf, PathBuf) {
        let d = tree(n);
        let t = std::env::temp_dir().join(format!("iyakki-homebrowse-trash-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        (d, t)
    }

    #[test]
    fn approved_changes_make_a_folder_rename_move_to_the_trash_and_save_inside_the_home() {
        let (d, t) = change_tree("apply");
        let mk = |dir: &str, name: &str| FileChange::MakeDir { dir: dir.into(), name: name.into() };
        assert!(apply(&d, &t, &mk("Docs", "new")).unwrap().contains("Made"));
        assert!(d.join("Docs/new").is_dir());
        assert!(apply(&d, &t, &mk("Docs", "new")).unwrap_err().contains("already there"));
        assert!(apply(&d, &t, &FileChange::Rename { path: "Docs/new".into(), new_name: "newer".into() }).is_ok());
        assert!(d.join("Docs/newer").is_dir() && !d.join("Docs/new").exists());
        assert!(apply(&d, &t, &FileChange::Rename { path: "b.txt".into(), new_name: "A.txt".into() }).unwrap_err().contains("already there"));
        assert!(apply(&d, &t, &FileChange::Trash { path: "b.txt".into() }).unwrap().contains("trash"));
        assert!(!d.join("b.txt").exists());
        let moved: Vec<String> = std::fs::read_dir(&t).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert!(moved.len() == 1 && moved[0].ends_with("-b.txt"), "{moved:?}");
        assert_eq!(std::fs::read_to_string(t.join(&moved[0])).unwrap(), "bee", "moved whole, not erased");
        // a save makes a new file, replaces an old one whole, and leaves no half-written file behind
        let save = |p: &str, x: &str| FileChange::Save { path: p.into(), text: x.into() };
        assert!(apply(&d, &t, &save("Docs/fresh.txt", "one")).is_ok());
        assert!(apply(&d, &t, &save("Docs/fresh.txt", "two")).is_ok());
        assert_eq!(std::fs::read_to_string(d.join("Docs/fresh.txt")).unwrap(), "two");
        assert!(!d.join("Docs/.fresh.txt.saving").exists());
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&t);
    }

    #[test]
    fn a_change_that_leaves_the_home_goes_through_a_link_or_a_hidden_name_or_hits_a_folder_is_refused() {
        let (d, t) = change_tree("evil-apply");
        let outside = std::env::temp_dir().join(format!("iyakki-homebrowse-out2-{}", std::process::id()));
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("s.txt"), "secret").unwrap();
        std::os::unix::fs::symlink(&outside, d.join("out")).unwrap();
        std::os::unix::fs::symlink(outside.join("s.txt"), d.join("lnk.txt")).unwrap();
        let save = |p: &str| FileChange::Save { path: p.into(), text: "x".into() };
        for bad in ["../x.txt", "/etc/x.txt", "out/x.txt", "lnk.txt", ".dot.txt", "Docs", "nope/x.txt", ".hidden/x.txt"] {
            assert!(apply(&d, &t, &save(bad)).is_err(), "{bad}");
        }
        assert_eq!(std::fs::read_to_string(outside.join("s.txt")).unwrap(), "secret", "the file outside is untouched");
        assert!(!outside.join("x.txt").exists());
        for c in [
            FileChange::Trash { path: "out".into() },
            FileChange::Trash { path: "lnk.txt".into() },
            FileChange::Trash { path: ".hidden".into() },
            FileChange::Trash { path: "".into() },
            FileChange::Rename { path: "out".into(), new_name: "x".into() },
            FileChange::Rename { path: "A.txt".into(), new_name: "../x".into() },
            FileChange::Rename { path: "A.txt".into(), new_name: ".h".into() },
            FileChange::MakeDir { dir: "out".into(), name: "x".into() },
            FileChange::MakeDir { dir: "A.txt".into(), name: "x".into() },
            FileChange::MakeDir { dir: "".into(), name: "a/b".into() },
        ] {
            assert!(apply(&d, &t, &c).is_err(), "{c:?}");
        }
        assert!(outside.join("s.txt").exists() && d.join("lnk.txt").exists() && d.join("out").exists());
        assert!(!t.exists() || std::fs::read_dir(&t).unwrap().next().is_none(), "nothing was moved to the trash");
        let _ = std::fs::remove_dir_all(&d);
        let _ = std::fs::remove_dir_all(&outside);
        let _ = std::fs::remove_dir_all(&t);
    }
}
