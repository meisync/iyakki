//! The optional yt-dlp extractor (F-MEDIA-12): downloads a video from a page address (a site the plain `media_download` cannot read).
//! A third-party program: never shipped, never installed on its own. The person presses Install (a card with the licence and the
//! pinned version), the program is fetched from the vendor, its SHA-256 is checked against the pin in `legal/downloads.txt`, and only
//! then is it written to the data folder. FAIL CLOSED: while that row is missing, or its pin is empty or not a SHA-256, nothing is
//! fetched and nothing is run. Each download is asked about by the chat tool; the program runs with an empty environment, with no
//! config file, no plug-ins and no `--exec`, one video at a time, into one folder.

use std::path::{Path, PathBuf};

/// The id of the row in `legal/downloads.txt`.
pub const ROW_ID: &str = "yt-dlp";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pin {
    pub version: String,
    pub sha256: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The row is not in the file, or its pin is empty or not `sha256:<64 hex>`: a human must add the vendor's checksum first.
    NoPin(String),
    Mismatch,
    NotApproved,
    BadAddress(String),
    NotInstalled,
    Failed(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NoPin(why) => write!(f, "the video-site extractor is not available yet: {why}. Nothing was downloaded."),
            Refusal::Mismatch => write!(f, "the downloaded extractor does not match the pinned checksum, so it was thrown away"),
            Refusal::NotApproved => write!(f, "the person has not approved this"),
            Refusal::BadAddress(w) => write!(f, "{w}"),
            Refusal::NotInstalled => write!(f, "the extractor is not installed; install it from Store, Setup first"),
            Refusal::Failed(w) => write!(f, "the extractor failed: {w}"),
        }
    }
}

fn hex64(s: &str) -> bool {
    s.len() == 64 && s.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f'))
}

/// Read the pin from the text of `legal/downloads.txt`: `yt-dlp | licence | download-only | digest | sha256:<hex> | yt-dlp <version> ... | date`.
pub fn pin_from(downloads_txt: &str) -> Result<Pin, Refusal> {
    let row = downloads_txt.lines().map(str::trim).filter(|l| !l.starts_with('#')).map(|l| l.split('|').map(str::trim).collect::<Vec<_>>()).find(|f| f.first() == Some(&ROW_ID));
    let f = row.ok_or_else(|| Refusal::NoPin("legal/downloads.txt has no yt-dlp row".into()))?;
    if f.len() != 7 {
        return Err(Refusal::NoPin("the yt-dlp row is malformed".into()));
    }
    let sha = f[4].strip_prefix("sha256:").filter(|h| hex64(h)).ok_or_else(|| Refusal::NoPin("the yt-dlp row has no sha256 pin".into()))?;
    let version = f[5].strip_prefix("yt-dlp ").and_then(|r| r.split_whitespace().next()).filter(|v| !v.is_empty() && v.chars().all(|c| c.is_ascii_alphanumeric() || c == '.')).ok_or_else(|| Refusal::NoPin("the yt-dlp row does not start its reason with `yt-dlp <version>`".into()))?;
    Ok(Pin { version: version.into(), sha256: sha.into() })
}

/// The vendor address of the pinned release (https, the vendor's own release host).
pub fn release_url(p: &Pin) -> String {
    format!("https://github.com/yt-dlp/yt-dlp/releases/download/{}/yt-dlp", p.version)
}

pub trait Fetch {
    fn get(&self, url: &str) -> Result<Vec<u8>, String>;
}

pub trait Runner {
    /// Run the program with these arguments and return the files it wrote into `out`.
    fn run(&self, exe: &Path, args: &[String], out: &Path) -> Result<Vec<PathBuf>, String>;
}

pub fn exe_path(data: &Path) -> PathBuf {
    data.join("tools").join("yt-dlp")
}

pub fn installed(data: &Path) -> bool {
    exe_path(data).is_file()
}

/// Fetch, verify and write the program. Pressing Install is the approval; the licence card is shown by the caller.
pub fn install(data: &Path, downloads_txt: &str, fetch: &dyn Fetch) -> Result<PathBuf, Refusal> {
    let pin = pin_from(downloads_txt)?;
    let bytes = fetch.get(&release_url(&pin)).map_err(Refusal::Failed)?;
    if crate::ocipush::sha256_hex(&bytes) != pin.sha256 {
        return Err(Refusal::Mismatch);
    }
    let p = exe_path(data);
    std::fs::create_dir_all(p.parent().unwrap_or(data)).map_err(|e| Refusal::Failed(e.to_string()))?;
    let tmp = p.with_extension("part");
    std::fs::write(&tmp, &bytes).map_err(|e| Refusal::Failed(e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o700));
    }
    std::fs::rename(&tmp, &p).map_err(|e| Refusal::Failed(e.to_string()))?;
    Ok(p)
}

/// A page address the extractor may be given: https only, a host name (never an address of this machine or its network), no credentials.
pub fn check_address(url: &str) -> Result<String, Refusal> {
    let u = url.trim();
    let rest = u.strip_prefix("https://").ok_or_else(|| Refusal::BadAddress("only https addresses are accepted".into()))?;
    let auth = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = auth.rsplit_once(':').map(|x| x.0).unwrap_or(auth).to_ascii_lowercase();
    let numeric = host.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ':' || c == '[' || c == ']');
    if auth.contains('@') || host.is_empty() || !host.contains('.') || numeric || host.ends_with(".local") || host.ends_with(".internal") || host == "localhost" || host.chars().any(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '.')) || u.len() > 2000 || u.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(Refusal::BadAddress("that address is not a public https site".into()));
    }
    Ok(u.to_string())
}

/// The fixed argument list: one video, no config, no plug-ins, no command execution, a bounded file name, into `out`.
pub fn args_for(url: &str, out: &Path) -> Vec<String> {
    let mut a: Vec<String> = ["--ignore-config", "--no-plugin-dirs", "--no-playlist", "--no-exec", "--no-cache-dir", "--no-mtime", "--restrict-filenames", "--max-filesize", "4G", "--newline", "-o"].iter().map(|s| s.to_string()).collect();
    a.push(format!("{}/%(title).80B.%(ext)s", out.display()));
    a.push("--".into());
    a.push(url.into());
    a
}

/// Download one video. `approved` is the person's answer to the card; the file stays inside `out`.
pub fn download(data: &Path, out: &Path, url: &str, approved: bool, runner: &dyn Runner) -> Result<Vec<PathBuf>, Refusal> {
    if !approved {
        return Err(Refusal::NotApproved);
    }
    let url = check_address(url)?;
    if !installed(data) {
        return Err(Refusal::NotInstalled);
    }
    std::fs::create_dir_all(out).map_err(|e| Refusal::Failed(e.to_string()))?;
    let files = runner.run(&exe_path(data), &args_for(&url, out), out).map_err(Refusal::Failed)?;
    let root = std::fs::canonicalize(out).map_err(|e| Refusal::Failed(e.to_string()))?;
    let mut kept = Vec::new();
    for f in files {
        match std::fs::canonicalize(&f) {
            Ok(c) if c.starts_with(&root) && c.is_file() => kept.push(c),
            _ => return Err(Refusal::Failed("the extractor named a file outside the download folder".into())),
        }
    }
    Ok(kept)
}

/// The real runner: an empty environment (only a HOME inside the output folder), the output folder as the working directory.
pub struct ProcessRunner;

impl Runner for ProcessRunner {
    fn run(&self, exe: &Path, args: &[String], out: &Path) -> Result<Vec<PathBuf>, String> {
        let before: std::collections::BTreeSet<PathBuf> = list(out);
        let o = std::process::Command::new(exe).args(args).env_clear().env("HOME", out).env("PATH", "/usr/bin:/bin").current_dir(out).output().map_err(|e| e.to_string())?;
        if !o.status.success() {
            return Err(String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("it stopped without a message").chars().take(200).collect());
        }
        Ok(list(out).difference(&before).cloned().collect())
    }
}

fn list(d: &Path) -> std::collections::BTreeSet<PathBuf> {
    std::fs::read_dir(d).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_file()).collect()
}

#[cfg(test)]
#[path = "ytdlp_tests.rs"]
mod tests;
