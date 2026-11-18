//! The Markdown files in a person's own home folder, for the viewer's file list (F-SDK-31). Policy (owner decision 2026-11-16: only the
//! person's own folders, asked per open): the root is the person's home and nothing outside it is ever listed or read; a link that leads
//! out of it is not followed; hidden folders and files are skipped; at most [`MAX_FILES`] names, [`MAX_DEPTH`] folders deep; a file is
//! read only when its name ends in `.md` or `.markdown`, is at most [`MAX_BYTES`] and is valid text. The caller (the bridge) has already
//! asked the person on a card; the app that asked only ever gets names relative to the home folder, never a path.

use std::path::{Component, Path, PathBuf};

pub const MAX_FILES: usize = 200;
pub const MAX_DEPTH: usize = 4;
pub const MAX_BYTES: u64 = 1 << 20;

fn is_markdown(name: &str) -> bool {
    let l = name.to_ascii_lowercase();
    l.ends_with(".md") || l.ends_with(".markdown")
}

/// Names (relative, `/`-separated, sorted) of the Markdown files under `root`.
pub fn list_markdown(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, root, 0, &mut out);
    out.sort();
    out.truncate(MAX_FILES);
    out
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES * 2 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        // a link is never followed or listed: it could lead anywhere
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let p = e.path();
        if ft.is_dir() {
            walk(root, &p, depth + 1, out);
        } else if ft.is_file() && is_markdown(&name) {
            if let Ok(rel) = p.strip_prefix(root) {
                out.push(rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"));
            }
        }
    }
}

/// The text of one file of the list, by the name the list gave.
pub fn read_markdown(root: &Path, name: &str) -> Result<String, String> {
    let rel = Path::new(name);
    if name.is_empty() || rel.is_absolute() || rel.components().any(|c| !matches!(c, Component::Normal(_))) || !is_markdown(name) {
        return Err("that is not a Markdown file of your home folder".into());
    }
    let mut p = PathBuf::from(root);
    for c in rel.components() {
        p.push(c);
        // no step of the way may be a link
        if std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(true) {
            return Err("that file is not there (or is a link, which is not followed)".into());
        }
    }
    let meta = std::fs::metadata(&p).map_err(|_| "that file cannot be read".to_string())?;
    if !meta.is_file() || meta.len() > MAX_BYTES {
        return Err("that file is not a plain file or is over 1 MB".into());
    }
    String::from_utf8(std::fs::read(&p).map_err(|_| "that file cannot be read".to_string())?).map_err(|_| "that file is not text".into())
}

/// A media file of the home (the embedded player, F-SDK-140): the same policy as the Markdown list (own folder only, no link followed,
/// hidden entries skipped, bounded depth) for the names the core knows as sound or video.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaFile {
    pub name: String,
    pub size: u64,
    pub video: bool,
}

/// The media files under `root`, sorted by name, at most [`MAX_FILES`].
pub fn list_media(root: &Path) -> Vec<MediaFile> {
    let mut out = Vec::new();
    walk_media(root, root, 0, &mut out);
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out.truncate(MAX_FILES);
    out
}

fn walk_media(root: &Path, dir: &Path, depth: usize, out: &mut Vec<MediaFile>) {
    if depth > MAX_DEPTH || out.len() >= MAX_FILES * 2 {
        return;
    }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let Ok(ft) = e.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let p = e.path();
        if ft.is_dir() {
            walk_media(root, &p, depth + 1, out);
        } else if ft.is_file() {
            if let (Some(video), Ok(rel)) = (iyakki_core_api::media::media_kind_of_name(&name), p.strip_prefix(root)) {
                let size = e.metadata().map(|m| m.len()).unwrap_or(0);
                out.push(MediaFile { name: rel.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/"), size, video });
            }
        }
    }
}

/// The path of one media file by the name the list gave, after the checks: a relative name, no step of the way a link, a plain file
/// of at most `max` bytes with a media extension. The caller reads it; the app that asked never sees the path.
pub fn resolve_media(root: &Path, name: &str, max: u64) -> Result<PathBuf, String> {
    let rel = Path::new(name);
    if name.is_empty() || rel.is_absolute() || rel.components().any(|c| !matches!(c, Component::Normal(_))) || iyakki_core_api::media::media_kind_of_name(name).is_none() {
        return Err("that is not a sound or video file of your home folder".into());
    }
    let mut p = PathBuf::from(root);
    for c in rel.components() {
        p.push(c);
        if std::fs::symlink_metadata(&p).map(|m| m.file_type().is_symlink()).unwrap_or(true) {
            return Err("that file is not there (or is a link, which is not followed)".into());
        }
    }
    let meta = std::fs::metadata(&p).map_err(|_| "that file cannot be read".to_string())?;
    if !meta.is_file() {
        return Err("that is not a plain file".into());
    }
    if meta.len() > max {
        return Err(format!("that file is over {} MB, which the player does not open", max >> 20));
    }
    Ok(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(n: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-homefiles-{n}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("notes/deep")).unwrap();
        std::fs::create_dir_all(d.join(".hidden")).unwrap();
        std::fs::write(d.join("a.md"), "# A").unwrap();
        std::fs::write(d.join("notes/b.markdown"), "# B").unwrap();
        std::fs::write(d.join("notes/deep/c.MD"), "# C").unwrap();
        std::fs::write(d.join("notes/skip.txt"), "x").unwrap();
        std::fs::write(d.join(".hidden/h.md"), "x").unwrap();
        std::fs::write(d.join(".dot.md"), "x").unwrap();
        d
    }

    #[test]
    fn only_markdown_files_under_the_home_are_listed_sorted_without_hidden_ones_or_paths_above() {
        let d = tree("list");
        assert_eq!(list_markdown(&d), vec!["a.md".to_string(), "notes/b.markdown".into(), "notes/deep/c.MD".into()]);
        assert_eq!(read_markdown(&d, "notes/b.markdown").unwrap(), "# B");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_name_that_leaves_the_home_or_goes_through_a_link_or_is_not_markdown_is_refused() {
        let d = tree("evil");
        let outside = std::env::temp_dir().join(format!("iyakki-homefiles-outside-{}.md", std::process::id()));
        std::fs::write(&outside, "secret").unwrap();
        std::os::unix::fs::symlink(&outside, d.join("link.md")).unwrap();
        std::os::unix::fs::symlink(std::env::temp_dir(), d.join("dirlink")).unwrap();
        assert!(!list_markdown(&d).iter().any(|n| n.contains("link")), "links are not listed");
        for bad in ["../x.md", "/etc/passwd.md", "link.md", "dirlink/y.md", "notes/skip.txt", "", "a.md/../a.md", "notes//b.markdown/.."] {
            assert!(read_markdown(&d, bad).is_err(), "{bad}");
        }
        std::fs::write(d.join("big.md"), vec![b'x'; (MAX_BYTES + 1) as usize]).unwrap();
        std::fs::write(d.join("bin.md"), [0xff, 0xfe, 0x00]).unwrap();
        assert!(read_markdown(&d, "big.md").unwrap_err().contains("1 MB"));
        assert!(read_markdown(&d, "bin.md").unwrap_err().contains("not text"));
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_list_is_bounded_in_size_and_depth() {
        let d = std::env::temp_dir().join(format!("iyakki-homefiles-big-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for i in 0..250 {
            std::fs::write(d.join(format!("f{i:03}.md")), "x").unwrap();
        }
        let deep = d.join("a/b/c/d/e/f");
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("too-deep.md"), "x").unwrap();
        let l = list_markdown(&d);
        assert_eq!(l.len(), MAX_FILES);
        assert!(!l.iter().any(|n| n.contains("too-deep")));
        let _ = std::fs::remove_dir_all(&d);
    }
    #[test]
    fn media_files_follow_the_same_home_policy() {
        let d = tree("media");
        std::fs::write(d.join("song.mp3"), "x").unwrap();
        std::fs::write(d.join("notes/clip.WEBM"), "xx").unwrap();
        std::fs::write(d.join(".hidden/h.mp3"), "x").unwrap();
        std::os::unix::fs::symlink(std::env::temp_dir(), d.join("dirlink")).unwrap();
        let l = list_media(&d);
        assert_eq!(l, vec![MediaFile { name: "notes/clip.WEBM".into(), size: 2, video: true }, MediaFile { name: "song.mp3".into(), size: 1, video: false }]);
        assert!(resolve_media(&d, "song.mp3", 10).is_ok());
        for bad in ["../song.mp3", "/etc/x.mp3", "dirlink/a.mp3", "a.md", "", "nothere.mp3"] {
            assert!(resolve_media(&d, bad, 10).is_err(), "{bad}");
        }
        assert!(resolve_media(&d, "notes/clip.WEBM", 1).unwrap_err().contains("over"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
