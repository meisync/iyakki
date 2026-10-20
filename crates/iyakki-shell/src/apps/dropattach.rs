//! Files dropped on the CHAT (window mode): what goes with the next message. A text file is quoted (up to
//! [`MAX_TEXT`]), a picture is copied into the Uploads folder and named so the assistant can show it, anything else is copied
//! into Uploads and listed by name. Folders are not attached. Nothing on the host is changed and no file is replaced (a
//! number is added to a name that is taken).

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

use iyakki_agent::hostfiles as hf;

/// The most of a text file that is quoted into a message.
pub const MAX_TEXT: u64 = 32 * 1024;
/// The largest file copied into Uploads from a chat drop.
pub const MAX_COPY: u64 = 64 << 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    pub name: String,
    pub size: u64,
    /// The words that go with the next message.
    pub block: String,
}

fn is_picture(name: &str) -> bool {
    matches!(Path::new(name).extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).as_deref(), Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "svg"))
}

fn kb(b: u64) -> String {
    if b >= 1 << 20 { format!("{:.1} MB", b as f64 / (1 << 20) as f64) } else { format!("{} KB", b.div_ceil(1024)) }
}

/// Prepare the attachments of a drop. `uploads` is where pictures and other files are copied. Returns the attachments and
/// plain notes about what was left out.
pub fn prepare(paths: &[PathBuf], uploads: &Path) -> (Vec<Attachment>, Vec<String>) {
    let mut out = Vec::new();
    let mut notes = Vec::new();
    for p in paths {
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let Ok(meta) = std::fs::metadata(p) else {
            notes.push(format!("{name} could not be read"));
            continue;
        };
        if meta.is_dir() {
            notes.push(format!("{name} is a folder: drop it on DATA or the AI IDE, or use Host files"));
            continue;
        }
        let size = meta.len();
        // text, within the cap: quoted
        if size <= MAX_TEXT && !is_picture(&name) {
            if let Ok(bytes) = std::fs::read(p) {
                if !bytes.contains(&0) {
                    if let Ok(text) = String::from_utf8(bytes) {
                        out.push(Attachment { name: name.clone(), size, block: format!("[Attached file: {name} ({})]\n```\n{}\n```", kb(size), text.trim_end()) });
                        continue;
                    }
                }
            }
        }
        if size > MAX_COPY {
            notes.push(format!("{name} is {} MB, more than the {} MB limit for an attachment", size >> 20, MAX_COPY >> 20));
            continue;
        }
        // pictures and everything else: copied into Uploads, named in the message
        let plan = hf::plan_import(std::slice::from_ref(p), uploads);
        match hf::run_import(&plan, uploads, hf::OnExists::KeepBoth, &AtomicU64::new(0)) {
            Ok(files) => {
                let at = files.first().cloned().unwrap_or_default();
                let block = if is_picture(&name) {
                    format!("[Attached picture: {name} ({}), saved as {}; show it with show_image]", kb(size), at.display())
                } else if size > MAX_TEXT && std::fs::read(p).ok().is_some_and(|b| !b.contains(&0) && String::from_utf8(b).is_ok()) {
                    format!("[Attached file: {name} ({}) is too long to quote (limit {} KB); saved as {}; read it with read_file]", kb(size), MAX_TEXT / 1024, at.display())
                } else {
                    format!("[Attached file: {name} ({}), saved as {}]", kb(size), at.display())
                };
                out.push(Attachment { name, size, block });
            }
            Err(e) => notes.push(format!("{name}: {e}")),
        }
    }
    (out, notes)
}

/// What to save from an assistant answer: its first table as `result.csv`, else its first code block (named by its language),
/// else the whole answer as `answer.md`.
pub fn result_file(answer: &str) -> (String, Vec<u8>) {
    // a Markdown table: rows that start with | , the separator row dropped
    let rows: Vec<&str> = answer.lines().map(str::trim).filter(|l| l.starts_with('|') && l.matches('|').count() >= 2).collect();
    if rows.len() >= 2 {
        let mut csv = String::new();
        for r in rows {
            let masked = r.replace("\\|", "\u{1}");
            let cells: Vec<String> = masked.trim_matches('|').split('|').map(|c| c.trim().replace('\u{1}', "|")).collect();
            if cells.iter().all(|c| !c.is_empty() && c.chars().all(|ch| ch == '-' || ch == ':')) {
                continue;
            }
            let line: Vec<String> = cells.iter().map(|c| iyakki_data::csv::field(c, ',')).collect();
            csv.push_str(&line.join(","));
            csv.push('\n');
        }
        return ("result.csv".to_string(), csv.into_bytes());
    }
    // the first fenced code block
    if let Some(start) = answer.find("```") {
        let after = &answer[start + 3..];
        let (lang, body) = after.split_once('\n').unwrap_or(("", after));
        if let Some(end) = body.find("```") {
            let ext = match lang.trim().to_ascii_lowercase().as_str() {
                "rust" | "rs" => "rs",
                "python" | "py" => "py",
                "javascript" | "js" => "js",
                "typescript" | "ts" => "ts",
                "json" => "json",
                "yaml" | "yml" => "yaml",
                "toml" => "toml",
                "html" => "html",
                "css" => "css",
                "sh" | "bash" | "shell" => "sh",
                "csv" => "csv",
                "sql" => "sql",
                "" => "txt",
                other if other.len() <= 5 && other.chars().all(|c| c.is_ascii_alphanumeric()) => "txt",
                _ => "txt",
            };
            return (format!("snippet.{ext}"), body[..end].as_bytes().to_vec());
        }
    }
    ("answer.md".to_string(), answer.as_bytes().to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_saved_as_its_table_its_code_or_itself() {
        let (n, b) = result_file("Here you go:\n| name | note |\n| --- | --- |\n| Ann | a, b |\n| Bob | x \\| y |\n");
        assert_eq!(n, "result.csv");
        assert_eq!(String::from_utf8(b).unwrap(), "name,note\nAnn,\"a, b\"\nBob,x | y\n");
        let (n, b) = result_file("Use this:\n```rust\nfn main() {}\n```\nDone.");
        assert_eq!((n.as_str(), String::from_utf8(b).unwrap().as_str()), ("snippet.rs", "fn main() {}\n"));
        let (n, b) = result_file("Just words.");
        assert_eq!((n.as_str(), b.as_slice()), ("answer.md", "Just words.".as_bytes()));
    }

    #[test]
    fn text_is_quoted_pictures_and_others_are_copied_and_named_folders_and_big_files_are_left_out() {
        let base = std::fs::canonicalize(std::env::temp_dir()).unwrap().join(format!("iyakki-attach-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (host, up) = (base.join("host"), base.join("Uploads"));
        std::fs::create_dir_all(host.join("dir")).unwrap();
        std::fs::write(host.join("notes.txt"), "line one\nline two\n").unwrap();
        std::fs::write(host.join("pic.png"), [137u8, 80, 78, 71, 0, 1]).unwrap();
        std::fs::write(host.join("data.bin"), [0u8, 1, 2, 3]).unwrap();
        std::fs::write(host.join("long.txt"), "x".repeat(40 * 1024)).unwrap();
        let (a, notes) = prepare(&[host.join("notes.txt"), host.join("pic.png"), host.join("data.bin"), host.join("long.txt"), host.join("dir"), host.join("missing")], &up);
        assert_eq!(a.len(), 4, "{a:?} {notes:?}");
        assert!(a[0].block.starts_with("[Attached file: notes.txt (1 KB)]\n```\nline one\nline two\n```"), "{}", a[0].block);
        assert!(a[1].block.contains("Attached picture: pic.png") && a[1].block.contains("show_image") && up.join("pic.png").exists(), "{}", a[1].block);
        assert!(a[2].block.contains("data.bin") && up.join("data.bin").exists());
        assert!(a[3].block.contains("too long to quote") && a[3].block.contains("read_file") && up.join("long.txt").exists(), "{}", a[3].block);
        assert_eq!(notes.len(), 2);
        assert!(notes[0].contains("dir is a folder") && notes[1].contains("missing could not be read"), "{notes:?}");
        // again: nothing is replaced, a number is added
        let (b, _) = prepare(&[host.join("pic.png")], &up);
        assert!(b[0].block.contains("pic (2).png"), "{}", b[0].block);
        assert!(std::fs::read(host.join("pic.png")).is_ok(), "the host file is untouched");
    }
}
