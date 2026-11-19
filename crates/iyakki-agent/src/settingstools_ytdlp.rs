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

pub fn entries(env: &Env) -> Vec<Entry> {
    entries_with(env, || Box::new(VendorFetch), || Box::new(ytdlp::ProcessRunner), DOWNLOADS.to_string())
}

pub fn entries_with(env: &Env, fetch: fn() -> Box<dyn Fetch>, runner: fn() -> Box<dyn ytdlp::Runner>, downloads: String) -> Vec<Entry> {
    let mut v = Vec::new();
    {
        let (e, d) = (env.clone(), downloads.clone());
        v.push(Entry::read(TAB, "ytdlp_status", "Say whether the optional video-site extractor is installed and whether its pinned checksum exists (without a pin it cannot be installed).", no_args(), move |_| {
            let pin = match ytdlp::pin_from(&d) {
                Ok(p) => format!("pinned version {}", p.version),
                Err(r) => r.to_string(),
            };
            Ok(format!("Extractor: {}. {pin}.", if ytdlp::installed(&e.dir) { "installed" } else { "not installed" }))
        }));
    }
    {
        let (e, d) = (env.clone(), downloads.clone());
        v.push(Entry::change(TAB, "ytdlp_install", "Download the optional video-site extractor from its vendor, check it against the pinned checksum and keep it. The person approves a card first; refused while no checksum is pinned.", json!({"type": "object", "properties": {}}), |_| "Download the video-site extractor from its vendor and check it".to_string(), move |_| {
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
        let schema = json!({"type": "object", "properties": {"url": {"type": "string", "description": "A public https page with one video."}}, "required": ["url"]});
        v.push(Entry::change(TAB, "media_download_site", "Download one video from a public https page with the optional extractor into the person's own folder. The person approves every download.", schema, |a| format!("Download the video at {}", a["url"].as_str().unwrap_or("a page")), move |a| {
            let url = arg_str(a, "url")?;
            let out = e.dir.join("site-downloads").join(&e.user);
            let r = runner();
            match ytdlp::download(&e.dir, &out, &url, true, r.as_ref()) {
                Ok(files) => {
                    e.audit("media.ytdlp", &format!("downloaded {} file(s) from {}", files.len(), url.split('/').nth(2).unwrap_or("")));
                    Ok(format!("Saved {} file(s) in {}.", files.len(), out.display()))
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
        format!("yt-dlp | Unlicense | download-only | digest | sha256:{} | yt-dlp 1.0 test | 2026-11-18\n", crate::ocipush::sha256_hex(b"prog"))
    }

    #[test]
    fn the_tools_ask_refuse_without_a_pin_and_install_then_download_with_one() {
        let r = rig("ytdlp", false);
        let none = tools(&r, entries_with(&r.env, || Box::new(Good), || Box::new(Ran), String::new()));
        assert_policy(&none, &["ytdlp_status"], &["ytdlp_install", "media_download_site"]);
        assert!(none.call("ytdlp_status", &json!({})).unwrap().contains("not installed"));
        assert!(none.call("ytdlp_install", &json!({})).unwrap_err().contains("Nothing was downloaded"));
        let t = tools(&r, entries_with(&r.env, || Box::new(Good), || Box::new(Ran), pinned()));
        assert!(t.call("media_download_site", &json!({"url": "https://video.example/a"})).unwrap_err().contains("not installed"));
        t.call("ytdlp_install", &json!({})).unwrap();
        assert!(t.call("ytdlp_status", &json!({})).unwrap().starts_with("Extractor: installed"));
        assert!(t.call("media_download_site", &json!({"url": "http://video.example/a"})).is_err());
        assert!(t.call("media_download_site", &json!({"url": "https://video.example/a"})).unwrap().contains("Saved 1 file"));
        assert!(r.audits.lock().unwrap().iter().any(|l| l.contains("media.ytdlp downloaded 1 file(s) from video.example")));
        let _ = std::fs::remove_dir_all(&r.dir);
    }

    #[test]
    fn the_shipped_downloads_file_has_no_yt_dlp_pin_yet_so_the_real_tools_refuse() {
        assert!(matches!(ytdlp::pin_from(DOWNLOADS), Err(Refusal::NoPin(_))), "a human adds the row with the vendor checksum; until then the flow is closed");
    }
}
