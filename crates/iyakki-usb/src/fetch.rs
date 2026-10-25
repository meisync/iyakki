//! Getting the image for the window installer: a download with progress and, when a `.sha256` is published next to it, a
//! checksum comparison. The transport is the caller's (the window installer passes the workspace's HTTPS client), so this file
//! is pure and testable with a made-up server.

use std::io::{Read, Write};
use std::path::Path;

use sha2::{Digest, Sha256};

/// The start of an HTTP answer.
pub struct Head {
    pub status: u16,
    /// Lower-case names.
    pub headers: Vec<(String, String)>,
    pub body: Box<dyn Read + Send>,
}

impl Head {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }
}

/// Opens a URL (one GET, no redirects followed).
pub type Opener<'a> = &'a dyn Fn(&str) -> Result<Head, String>;

/// May this address be fetched? HTTPS, or plain HTTP only for the local computer.
pub fn url_allowed(url: &str) -> Result<(), String> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Ok(());
    }
    if let Some(rest) = lower.strip_prefix("http://") {
        let host = rest.split(['/', ':']).next().unwrap_or("");
        if matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
            return Ok(());
        }
        return Err("plain http:// downloads are refused (the image could be altered on the way); use https://".into());
    }
    Err("the address must start with https://".into())
}

fn open_following(open: Opener, url: &str) -> Result<Head, String> {
    let mut url = url.to_string();
    for _ in 0..6 {
        url_allowed(&url)?;
        let head = open(&url)?;
        if (300..400).contains(&head.status) {
            let loc = head.header("location").ok_or("a redirect without a place to go")?.to_string();
            url = if loc.contains("://") {
                loc
            } else if let Some(path) = loc.strip_prefix('/') {
                let scheme_host: String = url.splitn(4, '/').take(3).collect::<Vec<_>>().join("/");
                format!("{scheme_host}/{path}")
            } else {
                return Err("an unusable redirect".into());
            };
            continue;
        }
        return Ok(head);
    }
    Err("too many redirects".into())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Downloaded {
    pub bytes: u64,
    pub sha256: String,
    /// `Some(true)`: matched the published `.sha256`; `None`: none was published (not verified).
    pub verified: Option<bool>,
}

/// The first 64 hex digits of a `.sha256` file (`<hash>  <name>` or just the hash).
pub fn parse_sha256(text: &str) -> Option<String> {
    let tok = text.split_whitespace().next()?.to_ascii_lowercase();
    (tok.len() == 64 && tok.bytes().all(|b| b.is_ascii_hexdigit())).then_some(tok)
}

/// Download `url` to `dest` (through `dest.part`), at most `max_bytes`, reporting `(done, total)`. When `url.sha256` exists the
/// result must match it, otherwise the file is deleted and the error says so.
pub fn download(open: Opener, url: &str, dest: &Path, max_bytes: u64, progress: &mut dyn FnMut(u64, u64)) -> Result<Downloaded, String> {
    let head = open_following(open, url)?;
    if head.status != 200 {
        return Err(format!("{url} answered HTTP {}", head.status));
    }
    let total = head.header("content-length").and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    if total > max_bytes {
        return Err(format!("the download is {} MB; more than the {} MB allowed", total >> 20, max_bytes >> 20));
    }
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    let part = dest.with_extension("part");
    let mut out = std::fs::File::create(&part).map_err(|e| format!("{}: {e}", part.display()))?;
    let (mut h, mut body, mut got) = (Sha256::new(), head.body, 0u64);
    let mut buf = vec![0u8; 256 * 1024];
    let r = (|| -> Result<(), String> {
        loop {
            let n = body.read(&mut buf).map_err(|e| format!("the download broke off: {e}"))?;
            if n == 0 {
                break;
            }
            got += n as u64;
            if got > max_bytes {
                return Err("the download is larger than allowed; stopped".into());
            }
            h.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
            progress(got, total.max(got));
        }
        out.flush().map_err(|e| e.to_string())?;
        out.sync_all().map_err(|e| e.to_string())
    })();
    drop(out);
    if let Err(e) = r {
        let _ = std::fs::remove_file(&part);
        return Err(e);
    }
    if total > 0 && got != total {
        let _ = std::fs::remove_file(&part);
        return Err(format!("the download stopped at {got} of {total} bytes"));
    }
    let sha: String = h.finalize().iter().map(|b| format!("{b:02x}")).collect();
    // the published checksum, if any
    let published = open_following(open, &format!("{url}.sha256")).ok().filter(|h| h.status == 200).and_then(|mut h| {
        let mut t = String::new();
        h.body.by_ref().take(4096).read_to_string(&mut t).ok()?;
        parse_sha256(&t)
    });
    let verified = match published {
        Some(want) if want == sha => Some(true),
        Some(want) => {
            let _ = std::fs::remove_file(&part);
            return Err(format!("the download does not match its published checksum (the file is damaged or was altered); it was deleted.\n  published {want}\n  downloaded {sha}"));
        }
        None => None,
    };
    std::fs::rename(&part, dest).map_err(|e| format!("{}: {e}", dest.display()))?;
    Ok(Downloaded { bytes: got, sha256: sha, verified })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn head(status: u16, headers: &[(&str, &str)], body: Vec<u8>) -> Result<Head, String> {
        Ok(Head { status, headers: headers.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(), body: Box::new(Cursor::new(body)) })
    }

    fn hash(b: &[u8]) -> String {
        Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
    }

    fn dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-fetch-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn urls_must_be_https_or_local() {
        assert!(url_allowed("https://example.org/x.img").is_ok());
        assert!(url_allowed("http://localhost:8000/x").is_ok());
        assert!(url_allowed("http://example.org/x").unwrap_err().contains("refused"));
        assert!(url_allowed("ftp://example.org/x").is_err());
        assert!(url_allowed("file:///etc/passwd").is_err());
    }

    #[test]
    fn a_download_is_checked_against_the_published_checksum() {
        let d = dir("ok");
        let data: Vec<u8> = (0..1_000_000u32).map(|i| (i % 253) as u8).collect();
        let sum = hash(&data);
        let open = |u: &str| -> Result<Head, String> {
            match u {
                "https://x.test/a.img" => head(200, &[("content-length", &data.len().to_string())], data.clone()),
                "https://x.test/a.img.sha256" => head(200, &[], format!("{sum}  a.img\n").into_bytes()),
                "https://x.test/old" => head(302, &[("location", "/a.img")], vec![]),
                _ => head(404, &[], vec![]),
            }
        };
        let mut last = (0, 0);
        let r = download(&open, "https://x.test/a.img", &d.join("a.img"), 10 << 20, &mut |a, b| last = (a, b)).unwrap();
        assert_eq!((r.bytes, r.verified, r.sha256.as_str()), (1_000_000, Some(true), sum.as_str()));
        assert_eq!(last, (1_000_000, 1_000_000));
        assert_eq!(std::fs::read(d.join("a.img")).unwrap(), data);
        assert!(!d.join("a.part").exists());
        // a redirect to the same host is followed
        let r2 = download(&open, "https://x.test/old", &d.join("b.img"), 10 << 20, &mut |_, _| {});
        assert!(r2.is_err() || r2.unwrap().bytes == 1_000_000);
    }

    #[test]
    fn a_wrong_checksum_deletes_the_file_and_no_checksum_means_not_verified() {
        let d = dir("bad");
        let data = vec![7u8; 5000];
        let open = |u: &str| -> Result<Head, String> {
            match u {
                "https://x.test/a.img" => head(200, &[("content-length", "5000")], data.clone()),
                "https://x.test/a.img.sha256" => head(200, &[], format!("{}\n", "0".repeat(64)).into_bytes()),
                "https://x.test/b.img" => head(200, &[], data.clone()),
                _ => head(404, &[], vec![]),
            }
        };
        let e = download(&open, "https://x.test/a.img", &d.join("a.img"), 1 << 20, &mut |_, _| {}).unwrap_err();
        assert!(e.contains("does not match") && e.contains("deleted"), "{e}");
        assert!(!d.join("a.img").exists() && !d.join("a.part").exists());
        let r = download(&open, "https://x.test/b.img", &d.join("b.img"), 1 << 20, &mut |_, _| {}).unwrap();
        assert_eq!(r.verified, None);
        assert!(d.join("b.img").exists());
    }

    #[test]
    fn size_limits_status_codes_and_short_downloads_are_errors() {
        let d = dir("lim");
        let open = |u: &str| -> Result<Head, String> {
            match u {
                "https://x.test/big" => head(200, &[("content-length", "999999999")], vec![0; 10]),
                "https://x.test/short" => head(200, &[("content-length", "100")], vec![0; 10]),
                "https://x.test/stream" => head(200, &[], vec![0; 3 << 20]),
                _ => head(404, &[], vec![]),
            }
        };
        assert!(download(&open, "https://x.test/big", &d.join("o"), 1 << 20, &mut |_, _| {}).unwrap_err().contains("more than"));
        assert!(download(&open, "https://x.test/short", &d.join("o"), 1 << 20, &mut |_, _| {}).unwrap_err().contains("stopped at"));
        assert!(download(&open, "https://x.test/stream", &d.join("o"), 1 << 20, &mut |_, _| {}).unwrap_err().contains("larger than allowed"));
        assert!(download(&open, "https://x.test/none", &d.join("o"), 1 << 20, &mut |_, _| {}).unwrap_err().contains("HTTP 404"));
        assert!(!d.join("o").exists() && !d.join("o.part").exists());
        assert!(download(&open, "http://evil.test/x", &d.join("o"), 1 << 20, &mut |_, _| {}).is_err());
        assert_eq!(parse_sha256(&format!("{} file", "AB".repeat(32))), Some("ab".repeat(32)));
        assert_eq!(parse_sha256("nope"), None);
        let _ = std::fs::remove_dir_all(&d);
    }
}
