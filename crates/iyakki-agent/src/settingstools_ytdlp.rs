//! Chat tools for the optional video-site extractor (F-MEDIA-12, `ytdlp.rs`): `ytdlp_status` (free), `ytdlp_install` (asks; fetches the
//! pinned program from the vendor at the person's approval, refused while the pin in `legal/downloads.txt` is missing) and
//! `media_download_site` (asks every time; one video from a public https page into the person's own download folder).

use serde_json::{json};

use crate::settingstools::{arg_str, no_args, Entry, Env};
use crate::ytdlp::{self, Fetch, Refusal};

const TAB: &str = "Browser";
pub const TOOLS: [&str; 3] = ["ytdlp_status", "ytdlp_install", "media_download_site"];

const DOWNLOADS: &str = include_str!("../../../legal/downloads.txt");

pub use iyakki_shared::settingstools_ytdlp::PROMPT_RULE;

/// The vendor fetch: https only, redirects followed (each one must be https), at most 200 MB.
pub struct VendorFetch;

impl Fetch for VendorFetch {
    fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        use iyakki_proxy::Streamer;
        use std::io::Read;
        let s = iyakki_proxy::https::UreqStreamer::default();
        let mut cur = url.to_string();
        for _ in 0..6 {
            if !cur.starts_with("https://") {
                return Err("the vendor sent the download somewhere that is not https".into());
            }
            let head = s.open(&cur, &[])?;
            if (300..400).contains(&head.status) {
                cur = head.header("location").ok_or("a redirect without a place")?.to_string();
                continue;
            }
            if head.status != 200 {
                return Err(format!("the vendor answered {}", head.status));
            }
            let mut b = Vec::new();
            head.body.take(200 << 20).read_to_end(&mut b).map_err(|e| e.to_string())?;
            return Ok(b);
        }
        Err("too many redirects".into())
    }
}

/// The real library: the person's own writable video folder and the services' media index.
pub struct RealLibrary {
    pub svc: std::sync::Arc<crate::services::Services>,
    pub admin: bool,
}

impl ytdlp::Library for RealLibrary {
    fn video_folder(&self, user: &str) -> Option<std::path::PathBuf> {
        let folders = crate::media::MediaStore::new(self.svc.dir(), user).folders();
        let folders = crate::mediaindex::picture_folders(&folders, user);
        let policy = if self.admin { iyakki_sec::policy::Role::Admin.template() } else { iyakki_sec::policy::Role::User.template() };
        crate::mediadl::default_folder(&folders, &policy, user, crate::mediaindex::ItemKind::Video).map(std::path::PathBuf::from)
    }

    fn register(&self, user: &str, file: &std::path::Path, source_url: &str) -> Result<(), String> {
        let extra = crate::mediaindex::Extra { source_url: source_url.to_string(), ..Default::default() };
        let ix = self.svc.media_index(user);
        let mut g = ix.lock().unwrap_or_else(|e| e.into_inner());
        g.add_file(&file.to_string_lossy(), &extra, crate::clock::now_unix())?;
        let _ = g.save();
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let _ = self.svc.notify(user, crate::inbox::Kind::Download, &format!("Downloaded {name}"), &format!("from {}", source_url.split('/').nth(2).unwrap_or("a video site")), Some(crate::inbox::Link::new("media", "")));
        Ok(())
    }
}

pub fn entries(env: &Env) -> Vec<Entry> {
    entries_with(env, || Box::new(VendorFetch), || Box::new(ytdlp::ProcessRunner), DOWNLOADS.to_string(), None)
}

/// The tools with the person's media library behind them: a finished file goes to their video folder and into the media index.
pub fn entries_for(env: &Env, library: std::sync::Arc<dyn ytdlp::Library>) -> Vec<Entry> {
    entries_with(env, || Box::new(VendorFetch), || Box::new(ytdlp::ProcessRunner), DOWNLOADS.to_string(), Some(library))
}

pub fn entries_with(env: &Env, fetch: fn() -> Box<dyn Fetch>, runner: fn() -> Box<dyn ytdlp::Runner>, downloads: String, library: Option<std::sync::Arc<dyn ytdlp::Library>>) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let (e, d) = (env.clone(), downloads.clone());
        v.push(Entry::read(TAB, "ytdlp_status", "Say whether the optional video-site extractor is installed, its licence and where it comes from, and whether its pinned checksum exists (without a pin it cannot be installed).", no_args(), move |_| {
            let pin = match ytdlp::pin_from(&d) {
                Ok(p) => format!("pinned version {}", p.version),
                Err(r) => r.to_string(),
            };
            Ok(format!("Extractor: {}. {pin}.\n{}", if ytdlp::installed(&e.dir) { "installed" } else { "not installed" }, ytdlp::card(&e.dir, &d).text()))
        }));
    }
    {
        let (e, d, d2) = (env.clone(), downloads.clone(), downloads.clone());
        let edir = env.dir.clone();
        v.push(Entry::change(TAB, "ytdlp_install", "Download the optional video-site extractor from its vendor, check it against the pinned checksum and keep it. The person approves a card first (it shows the licence, the vendor's address and the pinned version); refused while no checksum is pinned.", json!({"type": "object", "properties": {}}), move |_| format!("Download the video-site extractor from its vendor and check it.\n{}", ytdlp::card(&edir, &d2).text()), move |_| {
            let f = fetch();
            match ytdlp::install(&e.dir, &d, f.as_ref()) {
                Ok(_) => {
                    e.audit("media.ytdlp", "installed");
                    Ok("The extractor is installed.".into())
                }
                Err(r) => Err(r.to_string()),
            }
        }));
    }
    {
        let e = env.clone();
        let lib = library.clone();
        let schema = json!({"type": "object", "properties": {"url": {"type": "string", "description": "A public https page with one video."}}, "required": ["url"]});
        v.push(Entry::change(TAB, "media_download_site", "Download one video from a public https page with the optional extractor into the person's own folder. The person approves every download.", schema, |a| format!("Download the video at {}", a["url"].as_str().unwrap_or("a page")), move |a| {
            let url = arg_str(a, "url")?;
            // the person's own video folder when there is a library, else a folder of their own under the data folder
            let out = lib.as_ref().and_then(|l| l.video_folder(&e.user)).unwrap_or_else(|| e.dir.join("site-downloads").join(&e.user));
            let r = runner();
            match ytdlp::download(&e.dir, &out, &url, true, r.as_ref()) {
                Ok(files) => {
                    e.audit("media.ytdlp", &format!("downloaded {} file(s) from {}", files.len(), url.split('/').nth(2).unwrap_or("")));
                    // every finished file is entered in the media index (a file the library does not know stays where it is, and is said so)
                    let mut added = 0;
                    let mut left = Vec::new();
                    if let Some(l) = &lib {
                        for f in &files {
                            match l.register(&e.user, f, &url) {
                                Ok(()) => added += 1,
                                Err(why) => left.push(format!("{} ({why})", f.file_name().and_then(|n| n.to_str()).unwrap_or("a file"))),
                            }
                        }
                    }
                    let lib_note = if lib.is_none() { String::new() } else if left.is_empty() { format!(" {added} added to your media library.") } else { format!(" {added} added to your media library; not added: {}.", left.join(", ")) };
                    Ok(format!("Saved {} file(s) in {}.{lib_note}", files.len(), out.display()))
                }
                Err(Refusal::NotInstalled) => Err(Refusal::NotInstalled.to_string()),
                Err(r) => Err(r.to_string()),
            }
        }));
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use crate::agent::Tools;
    use crate::settingstools_tests::{assert_policy, rig, tools};

    struct Good;
    impl Fetch for Good {
        fn get(&self, _: &str) -> Result<Vec<u8>, String> {
            Ok(b"prog".to_vec())
        }
    }
    struct Ran;
    impl ytdlp::Runner for Ran {
        fn run(&self, _: &std::path::Path, _: &[String], out: &std::path::Path) -> Result<Vec<std::path::PathBuf>, String> {
            std::fs::write(out.join("v.mp4"), b"x").unwrap();
            Ok(vec![out.join("v.mp4")])
        }
    }

    fn pinned() -> String {
        format!("ytdlp | Unlicense | download-only | digest | sha256:{} | yt-dlp 1.0 test | 2026-11-18\n", crate::ocipush::sha256_hex(b"prog"))
    }

    #[test]
    fn the_tools_ask_refuse_without_a_pin_and_install_then_download_with_one() {
        let r = rig("ytdlp", false);
        let none = tools(&r, entries_with(&r.env, || Box::new(Good), || Box::new(Ran), String::new(), None));
        assert_policy(&none, &["ytdlp_status"], &["ytdlp_install", "media_download_site"]);
        assert!(none.call("ytdlp_status", &json!({})).unwrap().contains("not installed"));
        assert!(none.call("ytdlp_install", &json!({})).unwrap_err().contains("Nothing was downloaded"));
        let t = tools(&r, entries_with(&r.env, || Box::new(Good), || Box::new(Ran), pinned(), None));
        assert!(t.call("media_download_site", &json!({"url": "https://video.example/a"})).unwrap_err().contains("not installed"));
        t.call("ytdlp_install", &json!({})).unwrap();
        assert!(t.call("ytdlp_status", &json!({})).unwrap().starts_with("Extractor: installed"));
        assert!(t.call("media_download_site", &json!({"url": "http://video.example/a"})).is_err());
        assert!(t.call("media_download_site", &json!({"url": "https://video.example/a"})).unwrap().contains("Saved 1 file"));
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("media.ytdlp downloaded 1 file(s) from video.example")));
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    struct Lib {
        folder: std::path::PathBuf,
        got: std::sync::Mutex<Vec<(String, std::path::PathBuf, String)>>,
        refuse: bool,
    }
    impl ytdlp::Library for Lib {
        fn video_folder(&self, _: &str) -> Option<std::path::PathBuf> {
            Some(self.folder.clone())
        }
        fn register(&self, user: &str, file: &std::path::Path, url: &str) -> Result<(), String> {
            self.got.lock().unwrap().push((user.into(), file.into(), url.into()));
            if self.refuse { Err("the library does not know this kind of file".into()) } else { Ok(()) }
        }
    }

    #[test]
    fn a_finished_download_goes_to_the_video_folder_and_into_the_media_index_and_the_card_says_what_it_is() {
        let r = rig("ytdlp-lib", false);
        let lib = Arc::new(Lib { folder: r.dir.join("Videos"), got: Default::default(), refuse: false });
        let t = tools(&r, entries_with(&r.env, || Box::new(Good), || Box::new(Ran), pinned(), Some(lib.clone())));
        // the card on approval and in the status: licence, vendor, pinned version
        let status = t.call("ytdlp_status", &json!({})).unwrap();
        assert!(status.contains("Unlicense") && status.contains("github.com/yt-dlp/yt-dlp/releases/download/1.0/yt-dlp") && status.contains("Pinned version 1.0") && status.contains("Not installed. Install downloads"), "{status}");
        t.call("ytdlp_install", &json!({})).unwrap();
        let out = t.call("media_download_site", &json!({"url": "https://video.example/a"})).unwrap();
        assert!(out.contains(&r.dir.join("Videos").display().to_string()) && out.contains("1 added to your media library"), "{out}");
        let got = lib.got.lock().unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].0.as_str(), got[0].2.as_str()), (r.env.user.as_str(), "https://video.example/a"));
        assert!(got[0].1.starts_with(std::fs::canonicalize(r.dir.join("Videos")).unwrap()));
        drop(got);
        // a file the library will not take stays where it is and the answer says so
        let lib2 = Arc::new(Lib { folder: r.dir.join("Videos"), got: Default::default(), refuse: true });
        let t2 = tools(&r, entries_with(&r.env, || Box::new(Good), || Box::new(Ran), pinned(), Some(lib2)));
        let out = t2.call("media_download_site", &json!({"url": "https://video.example/b"})).unwrap();
        assert!(out.contains("0 added") && out.contains("not added: v.mp4 (the library does not know"), "{out}");
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn the_card_offers_no_button_without_a_pin_and_one_with_it() {
        let d = crate::testdir::outside_project("ytdlp", "card");
        let none = ytdlp::card(&d, "");
        assert!(!none.can_install() && matches!(none.state, ytdlp::CardState::Unavailable(_)));
        assert!(none.text().contains("Not available yet") && none.text().contains("Unlicense") && none.text().contains("Nothing was downloaded"), "{}", none.text());
        assert!(none.vendor.starts_with("https://github.com/yt-dlp") && none.version.is_empty());
        let ok = ytdlp::card(&d, &pinned());
        assert!(ok.can_install() && ok.version == "1.0" && ok.checksum_short.len() == 12);
        std::fs::create_dir_all(d.join("tools")).unwrap();
        std::fs::write(ytdlp::exe_path(&d), b"x").unwrap();
        let inst = ytdlp::card(&d, &pinned());
        assert!(!inst.can_install() && inst.text().contains("Installed. Each download still asks"));
    }

    #[test]
    fn the_shipped_downloads_file_carries_the_vendor_pin_and_the_code_constant_matches_it() {
        let p = ytdlp::pin_from(DOWNLOADS).expect("the owner-directed pin of 2026-11-19 is in the file");
        assert_eq!(p.sha256, ytdlp::PINNED_SHA256);
        assert!(ytdlp::release_url(&p).starts_with("https://github.com/yt-dlp/yt-dlp/releases/download/"));
    }
}
