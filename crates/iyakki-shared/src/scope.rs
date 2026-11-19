pub use iyakki_filescope::{resolve, FileRoots, HostMount};
use std::path::Path;
use std::path::PathBuf;

/// Like [`resolve`], but for a file about to be written: the folders above it need not exist yet. The nearest folder that
/// does exist is resolved (symlinks followed), so the result can be checked against the write scope before anything is made.
/// A file in the middle of the path is an error that names it.
pub fn resolve_for_write(cwd: &Path, input: &str) -> Result<PathBuf, String> {
    let joined = if input.starts_with('/') { input.to_string() } else { format!("{}/{}", cwd.display(), input) };
    let norm = iyakki_vfs::normalize(&joined).map_err(|_| format!("invalid path: {input}"))?;
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    let mut cur = PathBuf::from(&norm);
    loop {
        match std::fs::canonicalize(&cur) {
            Ok(real) => {
                if !tail.is_empty() && !real.is_dir() {
                    return Err(format!("cannot write {input}: {}", blocking_file_text(&real)));
                }
                return Ok(tail.iter().rev().fold(real, |p, c| p.join(c)));
            }
            Err(_) => {
                let Some(name) = cur.file_name().map(|n| n.to_os_string()) else { return Err(format!("invalid path: {input}")) };
                tail.push(name);
                if !cur.pop() {
                    return Err(format!("invalid path: {input}"));
                }
            }
        }
    }
}

/// "<f> is a file, not a folder" plus what to do; an empty file (a leftover of a mistaken write) is said to be safe to delete.
pub fn blocking_file_text(f: &Path) -> String {
    let empty = std::fs::metadata(f).is_ok_and(|m| m.is_file() && m.len() == 0);
    if empty {
        format!("{0} is a file, not a folder, so nothing can be put inside it. It is EMPTY (probably made by mistake), so deleting it is safe: ask the person to delete {0} (or delete it with a command if you may), then retry. Or choose a different folder name.", f.display())
    } else {
        format!("{0} is a file, not a folder, so nothing can be put inside it. It has content, so do not delete it: {SUGGEST_OTHER_NAME}", f.display())
    }
}

pub const SUGGEST_OTHER_NAME: &str = "Next: choose a different file or folder name, or call list_dir on the parent folder to see what is there.";
