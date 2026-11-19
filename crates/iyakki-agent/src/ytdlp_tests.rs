use super::*;
use std::cell::RefCell;

const BODY: &[u8] = b"#!/bin/sh\necho hi\n";

fn row(sha: &str) -> String {
    format!("# c\nyt-dlp | Unlicense | download-only | digest | {sha} | yt-dlp 2026.11.01 from the vendor's release page | 2026-11-18\n")
}

fn good() -> String {
    row(&format!("sha256:{}", crate::ocipush::sha256_hex(BODY)))
}

struct Fake(Vec<u8>, RefCell<Vec<String>>);
impl Fetch for Fake {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        self.1.borrow_mut().push(url.into());
        Ok(self.0.clone())
    }
}

struct Run(RefCell<Vec<Vec<String>>>, String);
impl Runner for Run {
    fn run(&self, _: &Path, args: &[String], out: &Path) -> Result<Vec<PathBuf>, String> {
        self.0.borrow_mut().push(args.to_vec());
        let p = if self.1.starts_with('/') { PathBuf::from(&self.1) } else { out.join(&self.1) };
        if !self.1.starts_with('/') {
            std::fs::write(&p, b"v").unwrap();
        }
        Ok(vec![p])
    }
}

fn dir(tag: &str) -> PathBuf {
    crate::testdir::outside_project("ytdlp", tag)
}

#[test]
fn an_empty_or_missing_pin_fails_closed_and_nothing_is_fetched() {
    let d = dir("ytdlp-closed");
    let f = Fake(BODY.to_vec(), RefCell::new(vec![]));
    for text in ["", "# nothing\n", &row(""), &row("sha256:abc"), &row("floating"), "yt-dlp | MIT | download-only | digest | broken\n"] {
        let e = install(&d, text, &f).unwrap_err();
        assert!(matches!(e, Refusal::NoPin(_)), "{text:?}: {e:?}");
        assert!(e.to_string().contains("Nothing was downloaded"));
    }
    assert!(f.1.borrow().is_empty(), "the vendor was never contacted");
    assert!(!installed(&d));
}

#[test]
fn a_matching_download_is_installed_and_a_different_one_is_thrown_away() {
    let d = dir("ytdlp-install");
    let bad = Fake(b"other".to_vec(), RefCell::new(vec![]));
    assert_eq!(install(&d, &good(), &bad).unwrap_err(), Refusal::Mismatch);
    assert!(!installed(&d) && !exe_path(&d).with_extension("part").exists());
    let ok = Fake(BODY.to_vec(), RefCell::new(vec![]));
    let p = install(&d, &good(), &ok).unwrap();
    assert!(installed(&d) && p == exe_path(&d));
    assert_eq!(ok.1.borrow()[0], "https://github.com/yt-dlp/yt-dlp/releases/download/2026.11.01/yt-dlp");
    assert_eq!(pin_from(&good()).unwrap().version, "2026.11.01");
}

#[test]
fn a_download_needs_approval_an_install_a_public_https_address_and_stays_in_its_folder() {
    let d = dir("ytdlp-run");
    install(&d, &good(), &Fake(BODY.to_vec(), RefCell::new(vec![]))).unwrap();
    let out = d.join("Videos");
    let r = Run(RefCell::new(vec![]), "clip.mp4".into());
    assert_eq!(download(&d, &out, "https://video.example/v/1", false, &r).unwrap_err(), Refusal::NotApproved);
    for bad in ["http://video.example/v", "https://127.0.0.1/x", "https://localhost/x", "https://u:p@video.example/x", "https://printer.local/x", "https://10.0.0.1/x", "https://a.example/x y", "file:///etc/passwd"] {
        assert!(matches!(download(&d, &out, bad, true, &r).unwrap_err(), Refusal::BadAddress(_)), "{bad}");
    }
    assert!(r.0.borrow().is_empty(), "the program never ran for a refused address");
    let files = download(&d, &out, "https://video.example/v/1", true, &r).unwrap();
    assert_eq!(files.len(), 1);
    let a = r.0.borrow()[0].join(" ");
    assert!(a.contains("--no-exec") && a.contains("--ignore-config") && a.contains("--no-plugin-dirs") && a.ends_with("-- https://video.example/v/1"), "{a}");
    let escape = Run(RefCell::new(vec![]), d.join("tools").join("yt-dlp").display().to_string());
    assert!(matches!(download(&d, &out, "https://video.example/v/2", true, &escape).unwrap_err(), Refusal::Failed(_)));
    let d2 = dir("ytdlp-none");
    assert_eq!(download(&d2, &d2.join("o"), "https://video.example/v", true, &r).unwrap_err(), Refusal::NotInstalled);
}
