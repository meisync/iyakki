use super::models::*;
use iyakki_proxy::{StreamHead, Streamer};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-indic-{tag}-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().subsec_nanos()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b).iter().map(|x| format!("{x:02x}")).collect()
}

/// Three fake files with their pins written as the administrator's catalog.json.
fn bundle(root: &Path, lang: &str) -> HashMap<String, Vec<u8>> {
    let files: HashMap<String, Vec<u8>> = FILES.iter().enumerate().map(|(i, f)| (f.to_string(), (0..(3000 + i * 1000)).map(|k| ((k * 7 + i) % 251) as u8).collect())).collect();
    let mut cat = serde_json::Map::new();
    let mut l = serde_json::Map::new();
    for (n, b) in &files {
        l.insert(n.clone(), serde_json::json!({"sha256": sha(b), "bytes": b.len()}));
    }
    cat.insert(lang.into(), l.into());
    std::fs::write(root.join("catalog.json"), serde_json::Value::Object(cat).to_string()).unwrap();
    files
}

struct Stop(std::io::Cursor<Vec<u8>>, usize);
impl Read for Stop {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        if self.1 == 0 {
            return Err(std::io::Error::other("connection reset"));
        }
        let n = b.len().min(self.1);
        let k = self.0.read(&mut b[..n])?;
        self.1 -= k;
        Ok(k)
    }
}

#[derive(Default)]
struct Fake {
    /// url -> content
    files: HashMap<String, Vec<u8>>,
    /// url -> redirect target
    redirects: HashMap<String, String>,
    ignore_range: bool,
    /// cut the connection after this many bytes (once)
    cut: Mutex<Option<usize>>,
    requests: Mutex<Vec<(String, Option<String>)>>,
}

impl Streamer for Fake {
    fn open(&self, url: &str, headers: &[(String, String)]) -> Result<StreamHead, String> {
        let range = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("range")).map(|(_, v)| v.clone());
        self.requests.lock().unwrap().push((url.to_string(), range.clone()));
        if let Some(t) = self.redirects.get(url) {
            return Ok(StreamHead { status: 302, headers: vec![("location".into(), t.clone())], body: Box::new(std::io::empty()) });
        }
        let Some(body) = self.files.get(url) else { return Ok(StreamHead { status: 404, headers: vec![], body: Box::new(std::io::empty()) }) };
        let start = if self.ignore_range { 0 } else { range.as_deref().and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.trim_end_matches('-').parse::<usize>().ok()).unwrap_or(0) };
        let status = if start > 0 { 206 } else { 200 };
        let data = body[start..].to_vec();
        let limit = self.cut.lock().unwrap().take().unwrap_or(usize::MAX);
        Ok(StreamHead { status, headers: vec![], body: Box::new(Stop(std::io::Cursor::new(data), limit)) })
    }
}

fn serve_all(files: &HashMap<String, Vec<u8>>, base: &str, lang: &str) -> HashMap<String, Vec<u8>> {
    files.iter().map(|(n, b)| (format!("{base}/{lang}/{n}"), b.clone())).collect()
}

#[test]
fn nothing_is_installed_until_all_files_match() {
    let root = tmp("status");
    let files = bundle(&root, "ta");
    assert_eq!(status(&root, "ta"), Status::NotInstalled);
    assert_eq!(status(&root, "gu"), Status::NoBundle, "no pin, no bundle");
    assert_eq!(status(&root, "zz"), Status::NoBundle);
    std::fs::create_dir_all(root.join("ta")).unwrap();
    std::fs::write(root.join("ta/vocab.json"), &files["vocab.json"]).unwrap();
    assert!(matches!(status(&root, "ta"), Status::Partial { .. }));
    for (n, b) in &files {
        std::fs::write(root.join("ta").join(n), b).unwrap();
    }
    assert!(matches!(status(&root, "ta"), Status::Installed { .. }));
    assert_eq!(installed(&root), vec!["ta"]);
    verify(&root, "ta").unwrap();
    // same size, other content: only verify sees it
    let mut bad = files["hifigan.onnx"].clone();
    bad[10] ^= 1;
    std::fs::write(root.join("ta/hifigan.onnx"), bad).unwrap();
    assert!(verify(&root, "ta").unwrap_err().contains("hifigan.onnx"));
    // a wrong size is damage
    std::fs::write(root.join("ta/hifigan.onnx"), b"short").unwrap();
    assert!(matches!(status(&root, "ta"), Status::Damaged(_)));
    assert!(installed(&root).is_empty());
}

#[test]
fn the_card_names_the_host_and_the_size_and_a_download_needs_an_address() {
    let root = tmp("plan");
    let files = bundle(&root, "ta");
    let total: u64 = files.values().map(|b| b.len() as u64).sum();
    assert!(plan(&root, "ta", "").unwrap_err().contains("no bundle address"));
    assert!(plan(&root, "ta", "ftp://x").is_err());
    assert!(plan(&root, "ta", "http://tts.example.com/x").unwrap_err().contains("https"), "plain http only on the local network");
    for ok in ["https://tts.example.com/b", "http://192.168.1.5:8080/b", "http://localhost/b", "http://nas.local/b", "http://127.0.0.1:9/b"] {
        assert!(plan(&root, "ta", ok).is_ok(), "{ok}");
    }
    let p = plan(&root, "ta", "https://models.example.org/indic/").unwrap();
    assert_eq!((p.host.as_str(), p.total_bytes, p.resume_bytes), ("models.example.org", total, 0));
    assert_eq!(p.files[0].0, "https://models.example.org/indic/ta/vocab.json");
    let card = p.card();
    assert!(card.contains("Tamil") && card.contains("models.example.org") && card.contains("checksums"), "{card}");
    assert!(plan(&root, "gu", "https://x.org").unwrap_err().contains("no bundle"));
    set_source(&root, "https://models.example.org/indic/").unwrap();
    assert_eq!(source(&root), "https://models.example.org/indic");
    assert!(set_source(&root, "http://evil.example.com").is_err());
    set_source(&root, "").unwrap();
    assert_eq!(source(&root), "");
}

#[test]
fn a_download_is_verified_and_lands_in_place() {
    let root = tmp("dl");
    let files = bundle(&root, "ta");
    let base = "https://models.example.org/indic";
    let f = Fake { files: serve_all(&files, base, "ta"), ..Fake::default() };
    let p = plan(&root, "ta", base).unwrap();
    let prog = Progress::default();
    download(&root, &p, &f, &prog).unwrap();
    assert_eq!(prog.percent(), 100);
    assert!(matches!(status(&root, "ta"), Status::Installed { .. }));
    for (n, b) in &files {
        assert_eq!(&std::fs::read(root.join("ta").join(n)).unwrap(), b);
    }
    assert!(!root.join("ta").read_dir().unwrap().flatten().any(|e| e.file_name().to_string_lossy().ends_with(".part")), "no part files left");
    // a second run fetches nothing
    let again = Fake { files: HashMap::new(), ..Fake::default() };
    download(&root, &plan(&root, "ta", base).unwrap(), &again, &Progress::default()).unwrap();
    assert!(again.requests.lock().unwrap().is_empty());
}

#[test]
fn an_interrupted_download_resumes_with_a_range_request() {
    let root = tmp("resume");
    let files = bundle(&root, "ta");
    let base = "https://m.example.org/b";
    let f = Fake { files: serve_all(&files, base, "ta"), ..Fake::default() };
    *f.cut.lock().unwrap() = Some(1500);
    let p = plan(&root, "ta", base).unwrap();
    let e = download(&root, &p, &f, &Progress::default()).unwrap_err();
    assert!(e.contains("interrupted") && e.contains("again"), "{e}");
    assert!(matches!(status(&root, "ta"), Status::Partial { have: 1500, .. }), "{:?}", status(&root, "ta"));
    let p2 = plan(&root, "ta", base).unwrap();
    assert_eq!(p2.resume_bytes, 1500);
    download(&root, &p2, &f, &Progress::default()).unwrap();
    let reqs = f.requests.lock().unwrap().clone();
    assert!(reqs.iter().any(|(_, r)| r.as_deref() == Some("bytes=1500-")), "{reqs:?}");
    assert!(matches!(status(&root, "ta"), Status::Installed { .. }));
    verify(&root, "ta").unwrap();
    // a server that ignores Range sends everything again: still correct
    let root2 = tmp("resume2");
    let files2 = bundle(&root2, "ta");
    let g = Fake { files: serve_all(&files2, base, "ta"), ignore_range: true, ..Fake::default() };
    *g.cut.lock().unwrap() = Some(700);
    let p = plan(&root2, "ta", base).unwrap();
    assert!(download(&root2, &p, &g, &Progress::default()).is_err());
    download(&root2, &plan(&root2, "ta", base).unwrap(), &g, &Progress::default()).unwrap();
    verify(&root2, "ta").unwrap();
}

#[test]
fn a_file_that_does_not_match_its_checksum_is_refused_and_deleted() {
    let root = tmp("hash");
    let mut files = bundle(&root, "ta");
    let base = "https://m.example.org/b";
    files.get_mut("hifigan.onnx").unwrap()[5] ^= 0xff; // same size, other content
    let f = Fake { files: serve_all(&files, base, "ta"), ..Fake::default() };
    let e = download(&root, &plan(&root, "ta", base).unwrap(), &f, &Progress::default()).unwrap_err();
    assert!(e.contains("hifigan.onnx") && e.contains("checksum") && e.contains("deleted"), "{e}");
    assert!(!root.join("ta/hifigan.onnx").exists() && !root.join("ta/hifigan.onnx.part").exists());
    assert!(!matches!(status(&root, "ta"), Status::Installed { .. }));
    // a file larger than pinned is cut off and deleted
    let root3 = tmp("big");
    let mut files3 = bundle(&root3, "ta");
    files3.get_mut("vocab.json").unwrap().extend(vec![1u8; 500]);
    let f3 = Fake { files: serve_all(&files3, base, "ta"), ..Fake::default() };
    let e = download(&root3, &plan(&root3, "ta", base).unwrap(), &f3, &Progress::default()).unwrap_err();
    assert!(e.contains("larger than its pinned size"), "{e}");
    assert!(!root3.join("ta/vocab.json.part").exists());
    // not found
    let root4 = tmp("404");
    bundle(&root4, "ta");
    let e = download(&root4, &plan(&root4, "ta", base).unwrap(), &Fake::default(), &Progress::default()).unwrap_err();
    assert!(e.contains("HTTP 404"), "{e}");
}

#[test]
fn redirects_are_followed_with_the_same_rules_and_cancel_keeps_what_was_fetched() {
    let root = tmp("redir");
    let files = bundle(&root, "ta");
    let base = "https://github.example.org/rel";
    let mut f = Fake::default();
    for (n, b) in &files {
        f.redirects.insert(format!("{base}/ta/{n}"), format!("https://objects.example.org/blob/{n}"));
        f.files.insert(format!("https://objects.example.org/blob/{n}"), b.clone());
    }
    download(&root, &plan(&root, "ta", base).unwrap(), &f, &Progress::default()).unwrap();
    assert!(matches!(status(&root, "ta"), Status::Installed { .. }));
    // a redirect to plain http on a public host is refused
    let root2 = tmp("redir2");
    let files2 = bundle(&root2, "ta");
    let mut g = Fake::default();
    g.redirects.insert(format!("{base}/ta/vocab.json"), "http://evil.example.com/v".into());
    g.files.insert("http://evil.example.com/v".into(), files2["vocab.json"].clone());
    let e = download(&root2, &plan(&root2, "ta", base).unwrap(), &g, &Progress::default()).unwrap_err();
    assert!(e.contains("https"), "{e}");
    // cancel
    let root3 = tmp("cancel");
    let files3 = bundle(&root3, "ta");
    let h = Fake { files: serve_all(&files3, base, "ta"), ..Fake::default() };
    let prog = Progress::default();
    prog.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
    let e = download(&root3, &plan(&root3, "ta", base).unwrap(), &h, &prog).unwrap_err();
    assert!(e.contains("cancelled"), "{e}");
}

#[test]
fn delete_removes_the_language_and_is_safe_to_repeat() {
    let root = tmp("del");
    let files = bundle(&root, "ta");
    std::fs::create_dir_all(root.join("ta")).unwrap();
    for (n, b) in &files {
        std::fs::write(root.join("ta").join(n), b).unwrap();
    }
    std::fs::create_dir_all(root.join("hi")).unwrap();
    std::fs::write(root.join("hi/keep"), b"x").unwrap();
    assert!(disk_used(&root) > 6000);
    delete(&root, "ta").unwrap();
    assert_eq!(status(&root, "ta"), Status::NotInstalled);
    delete(&root, "ta").unwrap();
    assert!(root.join("hi/keep").exists(), "other languages are untouched");
    assert!(delete(&root, "../x").is_err() && delete(&root, "").is_err(), "no path tricks");
    assert!(root.join("catalog.json").exists());
}

#[test]
fn a_folder_or_usb_stick_is_checked_before_anything_is_installed() {
    let root = tmp("usb-root");
    let files = bundle(&root, "ta");
    let stick = tmp("usb-stick");
    std::fs::create_dir_all(stick.join("ta")).unwrap();
    for (n, b) in &files {
        std::fs::write(stick.join("ta").join(n), b).unwrap();
    }
    import_folder(&root, "ta", &stick).unwrap();
    assert!(matches!(status(&root, "ta"), Status::Installed { .. }));
    // the files directly in the folder work too
    let root2 = tmp("usb-root2");
    bundle(&root2, "ta");
    import_folder(&root2, "ta", &stick.join("ta")).unwrap();
    assert!(matches!(status(&root2, "ta"), Status::Installed { .. }));
    // one wrong file installs nothing
    let root3 = tmp("usb-root3");
    bundle(&root3, "ta");
    let mut bad = files["fastpitch.onnx"].clone();
    bad[3] ^= 1;
    std::fs::write(stick.join("ta/fastpitch.onnx"), bad).unwrap();
    let e = import_folder(&root3, "ta", &stick).unwrap_err();
    assert!(e.contains("fastpitch.onnx") && e.contains("checksum"), "{e}");
    assert_eq!(status(&root3, "ta"), Status::NotInstalled);
    assert!(!root3.join("ta").exists());
    // missing file
    std::fs::remove_file(stick.join("ta/vocab.json")).unwrap();
    assert!(import_folder(&root3, "ta", &stick).unwrap_err().contains("not in"));
    assert!(import_folder(&root3, "xx", &stick).is_err());
}

#[test]
fn the_built_in_table_pins_every_exported_language_with_real_hashes() {
    for (l, files) in BUILTIN {
        assert!(super::lang_info(l).is_some(), "{l}");
        for (n, h, b) in files {
            assert!(FILES.contains(n) && h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()) && *b > 100, "{l} {n}");
        }
    }
    let root = tmp("builtin");
    for l in ["ta", "te", "ml", "kn", "hi", "bn", "mr", "en"] {
        assert!(pins(&root, l).is_some(), "{l}");
    }
    // the administrator's catalog.json wins; a broken entry of it means no bundle (it never falls back silently), other languages keep theirs
    std::fs::write(root.join("catalog.json"), "{\"ta\":{\"vocab.json\":{\"sha256\":\"zz\",\"bytes\":1}}}").unwrap();
    assert!(pins(&root, "ta").is_none() && pins(&root, "hi").is_some());
    assert!(human(1_500_000_000).contains("GB") && human(260 << 20).contains("MB"));
}
