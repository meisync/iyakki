//! File uploads from forms (F-BROW-94): `<input type=file>` is a field where the person types the path of a file they may read;
//! when the form is sent the file travels as `multipart/form-data`. The file rules of the account apply (the path must be readable
//! for the person), the size is capped, and nothing is sent that the person did not name.

use std::path::{Path, PathBuf};

/// Largest file that is sent (the whole form is held in memory).
pub const MAX_UPLOAD: u64 = 16 << 20;

/// One field of a multipart form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Part {
    pub name: String,
    /// `Some` for a file (may be empty: no file chosen).
    pub filename: Option<String>,
    pub content_type: String,
    pub data: Vec<u8>,
}

impl Part {
    pub fn text(name: &str, value: &str) -> Part {
        Part { name: name.to_string(), filename: None, content_type: String::new(), data: value.as_bytes().to_vec() }
    }
}

/// A name inside the quoted header parameters: quotes, line breaks and backslashes are written as the browsers write them.
fn quote(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            '"' => "%22".to_string(),
            '\r' => "%0D".to_string(),
            '\n' => "%0A".to_string(),
            '\\' => "%5C".to_string(),
            c if (c as u32) < 0x20 => String::new(),
            c => c.to_string(),
        })
        .collect()
}

/// A boundary that does not occur in any part (the caller gives random bytes; if it still occurs the next try is used).
pub fn boundary_for(parts: &[Part], random: &mut dyn FnMut() -> u64) -> String {
    for _ in 0..16 {
        let b = format!("----IyakkiForm{:016x}{:016x}", random(), random());
        if !parts.iter().any(|p| contains(&p.data, b.as_bytes())) {
            return b;
        }
    }
    format!("----IyakkiForm{}", "0".repeat(40))
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && hay.windows(needle.len()).any(|w| w == needle)
}

/// The body of a `multipart/form-data` request.
pub fn encode(parts: &[Part], boundary: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for p in parts {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match &p.filename {
            Some(f) => {
                out.extend_from_slice(format!("Content-Disposition: form-data; name=\"{}\"; filename=\"{}\"\r\n", quote(&p.name), quote(f)).as_bytes());
                let ct = if p.content_type.is_empty() { "application/octet-stream" } else { p.content_type.as_str() };
                out.extend_from_slice(format!("Content-Type: {}\r\n", ct.replace(['\r', '\n'], "")).as_bytes());
            }
            None => out.extend_from_slice(format!("Content-Disposition: form-data; name=\"{}\"\r\n", quote(&p.name)).as_bytes()),
        }
        out.extend_from_slice(b"\r\n");
        out.extend_from_slice(&p.data);
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

/// The media type for a file name, by its ending (a short list; anything else is `application/octet-stream`).
pub fn content_type_for(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "txt" | "log" | "md" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "xml" => "application/xml",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "mp4" => "video/mp4",
        _ => "application/octet-stream",
    }
}

/// Read the file the person named. `may_read` is the account's rule (the real path is checked). Returns the file name to send
/// (the last part of the path, never a folder) and the bytes. An empty path means "no file chosen".
pub fn read_upload(typed: &str, home: &Path, may_read: &dyn Fn(&Path) -> Result<(), String>) -> Result<Option<(String, Vec<u8>)>, String> {
    let typed = typed.trim();
    if typed.is_empty() {
        return Ok(None);
    }
    let path: PathBuf = match typed.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if typed == "~" => home.to_path_buf(),
        None => PathBuf::from(typed),
    };
    let path = if path.is_absolute() { path } else { home.join(path) };
    let real = std::fs::canonicalize(&path).map_err(|_| format!("{typed}: no such file"))?;
    may_read(&real)?;
    let meta = std::fs::metadata(&real).map_err(|e| format!("{typed}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{typed} is not a file"));
    }
    if meta.len() > MAX_UPLOAD {
        return Err(format!("{typed} is larger than {} MB; files that big are not sent from a form here", MAX_UPLOAD >> 20));
    }
    let data = std::fs::read(&real).map_err(|e| format!("{typed}: {e}"))?;
    let name = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
    Ok(Some((name, data)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let base = std::env::var("TMPDIR").unwrap_or_else(|_| std::env::temp_dir().to_string_lossy().into_owned());
        let d = PathBuf::from(base).join(format!("iyakki-upload-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_form_with_text_and_a_file_is_encoded_the_way_servers_read_it() {
        let parts = vec![Part::text("title", "Hello"), Part { name: "doc".into(), filename: Some("a \"b\".txt".into()), content_type: "text/plain".into(), data: b"line1\r\nline2".to_vec() }, Part { name: "empty".into(), filename: Some(String::new()), content_type: String::new(), data: vec![] }];
        let body = String::from_utf8(encode(&parts, "XX")).unwrap();
        assert_eq!(
            body,
            "--XX\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nHello\r\n--XX\r\nContent-Disposition: form-data; name=\"doc\"; filename=\"a %22b%22.txt\"\r\nContent-Type: text/plain\r\n\r\nline1\r\nline2\r\n--XX\r\nContent-Disposition: form-data; name=\"empty\"; filename=\"\"\r\nContent-Type: application/octet-stream\r\n\r\n\r\n--XX--\r\n"
        );
    }

    #[test]
    fn names_cannot_break_out_of_their_header_and_the_boundary_avoids_the_data() {
        let evil = Part { name: "x\"\r\nContent-Type: evil".into(), filename: Some("f\r\n.txt".into()), content_type: "text/plain\r\nX: y".into(), data: vec![] };
        let body = String::from_utf8(encode(&[evil], "B")).unwrap();
        assert_eq!(body.matches("\r\n").count(), 6, "{body:?}");
        assert!(body.contains("name=\"x%22%0D%0AContent-Type: evil\"") && body.contains("filename=\"f%0D%0A.txt\""));
        // a boundary that happens to be inside the data is not used
        let mut calls = 0u64;
        let first = format!("----IyakkiForm{:016x}{:016x}", 1, 2);
        let parts = vec![Part::text("t", &format!("xx{first}yy"))];
        let b = boundary_for(&parts, &mut || {
            calls += 1;
            calls
        });
        assert_ne!(b, first);
        assert!(b.starts_with("----IyakkiForm") && !parts[0].data.windows(b.len()).any(|w| w == b.as_bytes()));
    }

    #[test]
    fn the_file_named_is_read_under_the_account_rules_and_with_a_size_limit() {
        let d = tmp("read");
        std::fs::write(d.join("note.txt"), b"hello").unwrap();
        std::fs::create_dir_all(d.join("sub")).unwrap();
        let allow = |_: &Path| Ok(());
        assert_eq!(read_upload("", &d, &allow).unwrap(), None, "no file chosen");
        assert_eq!(read_upload("note.txt", &d, &allow).unwrap(), Some(("note.txt".into(), b"hello".to_vec())), "relative to the home folder");
        assert_eq!(read_upload(&format!("{}/note.txt", d.display()), &d, &allow).unwrap().unwrap().0, "note.txt");
        assert!(read_upload("sub", &d, &allow).unwrap_err().contains("not a file"));
        assert!(read_upload("missing.txt", &d, &allow).unwrap_err().contains("no such file"));
        let deny = |p: &Path| Err(format!("permission denied: {}", p.display()));
        assert!(read_upload("note.txt", &d, &deny).unwrap_err().contains("permission denied"), "the account's rule decides");
        let big = std::fs::File::create(d.join("big.bin")).unwrap();
        big.set_len(MAX_UPLOAD + 1).unwrap();
        assert!(read_upload("big.bin", &d, &allow).unwrap_err().contains("larger than"));
        assert_eq!(content_type_for("Photo.JPG"), "image/jpeg");
        assert_eq!(content_type_for("noext"), "application/octet-stream");
    }
}
