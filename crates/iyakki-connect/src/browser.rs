//! The installed browser for `iyakki-connect window`: found by well-known paths (never downloaded, never bundled), started in application
//! mode (no tabs, no address bar) with a private temporary profile and no extensions. Chrome, Chromium and Edge do this with `--app=`;
//! Firefox and Safari have no such mode, so they get an ordinary window and the program says so.
//!
//! Discovery takes its surroundings as arguments (the system, a way to ask "does this file exist", environment variables), so the lists
//! for macOS and Windows are tested on any system with fake paths. Only the Linux path has been used for real.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Os {
    Linux,
    Mac,
    Windows,
}

impl Os {
    pub fn here() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Chrome, Chromium, Edge: `--app=URL`.
    Chromium,
    Firefox,
    Safari,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Browser {
    pub kind: Kind,
    pub name: &'static str,
    pub path: PathBuf,
}

/// Where to look.
pub struct Surroundings<'a> {
    pub os: Os,
    pub exists: &'a dyn Fn(&Path) -> bool,
    pub var: &'a dyn Fn(&str) -> Option<String>,
}

fn path_dirs(var: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
    let sep = |c: char| c == ':' || c == ';';
    var("PATH").map(|p| p.split(sep).filter(|d| !d.is_empty()).map(PathBuf::from).collect()).unwrap_or_default()
}

/// Every place a supported browser may be, best first: Chromium family before Firefox before Safari.
pub fn candidates(s: &Surroundings) -> Vec<Browser> {
    let mut v = Vec::new();
    let mut add = |kind, name, path: String| v.push(Browser { kind, name, path: PathBuf::from(path) });
    match s.os {
        Os::Linux => {
            for (kind, name, files) in [
                (Kind::Chromium, "Google Chrome", &["google-chrome", "google-chrome-stable"][..]),
                (Kind::Chromium, "Chromium", &["chromium", "chromium-browser"][..]),
                (Kind::Chromium, "Microsoft Edge", &["microsoft-edge", "microsoft-edge-stable"][..]),
                (Kind::Firefox, "Firefox", &["firefox"][..]),
            ] {
                for f in files {
                    for d in path_dirs(s.var).into_iter().chain([PathBuf::from("/usr/bin"), PathBuf::from("/snap/bin")]) {
                        add(kind, name, d.join(f).to_string_lossy().into_owned());
                    }
                }
            }
        }
        Os::Mac => {
            let home = (s.var)("HOME").unwrap_or_default();
            for (kind, name, rel) in [
                (Kind::Chromium, "Google Chrome", "Google Chrome.app/Contents/MacOS/Google Chrome"),
                (Kind::Chromium, "Chromium", "Chromium.app/Contents/MacOS/Chromium"),
                (Kind::Chromium, "Microsoft Edge", "Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
                (Kind::Firefox, "Firefox", "Firefox.app/Contents/MacOS/firefox"),
                (Kind::Safari, "Safari", "Safari.app"),
            ] {
                add(kind, name, format!("/Applications/{rel}"));
                if !home.is_empty() && kind != Kind::Safari {
                    add(kind, name, format!("{home}/Applications/{rel}"));
                }
            }
            add(Kind::Safari, "Safari", "/System/Volumes/Preboot/Cryptexes/App/System/Applications/Safari.app".into());
        }
        Os::Windows => {
            let roots: Vec<String> = ["ProgramFiles", "ProgramFiles(x86)", "LocalAppData"].iter().filter_map(|k| (s.var)(k)).filter(|v| !v.is_empty()).collect();
            for (kind, name, rel) in [
                (Kind::Chromium, "Google Chrome", "Google\\Chrome\\Application\\chrome.exe"),
                (Kind::Chromium, "Chromium", "Chromium\\Application\\chrome.exe"),
                (Kind::Chromium, "Microsoft Edge", "Microsoft\\Edge\\Application\\msedge.exe"),
                (Kind::Firefox, "Firefox", "Mozilla Firefox\\firefox.exe"),
            ] {
                for r in &roots {
                    add(kind, name, format!("{}\\{rel}", r.trim_end_matches('\\')));
                }
            }
        }
    }
    v
}

/// The first browser that is there.
pub fn find(s: &Surroundings) -> Option<Browser> {
    candidates(s).into_iter().find(|b| (s.exists)(&b.path))
}

/// The browser on this computer, or the one the person named (`--browser PATH` or `IYAKKI_CONNECT_BROWSER`; its kind is read from its file name).
pub fn find_here(named: Option<&str>) -> Result<Browser, String> {
    if let Some(p) = named.filter(|p| !p.is_empty()) {
        let path = PathBuf::from(p);
        if !path.is_file() && !path.is_dir() {
            return Err(format!("{p}: no such browser file"));
        }
        let lower = p.to_ascii_lowercase();
        let (kind, name) = if lower.contains("firefox") {
            (Kind::Firefox, "Firefox")
        } else if lower.contains("safari") {
            (Kind::Safari, "Safari")
        } else {
            (Kind::Chromium, "the browser you named")
        };
        return Ok(Browser { kind, name, path });
    }
    let exists = |p: &Path| p.exists();
    let var = |k: &str| std::env::var(k).ok();
    find(&Surroundings { os: Os::here(), exists: &exists, var: &var })
        .ok_or_else(|| "no browser was found (looked for Chrome, Chromium and Edge, then Firefox, then Safari). Install one, or name it with --browser PATH. IYAKKI never downloads a browser.".to_string())
}

/// `file:///...` for a local path (what the browser opens first).
pub fn file_url(path: &Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from(if p.starts_with('/') { "file://" } else { "file:///" });
    for b in p.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'.' | b'_' | b'~' | b'-' | b':') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The program, its arguments and a note for the person (when the window is not an application window).
#[derive(Debug, PartialEq, Eq)]
pub struct Command {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub note: Option<&'static str>,
}

/// How to start `b` on `start` (a local page that sends the browser on to the loopback address, so no secret is in the argument list),
/// with `profile` as its private folder. `extra` arguments are appended unchanged (the person's own `--browser-arg`).
pub fn command(b: &Browser, profile: &Path, start: &Path, extra: &[String]) -> Command {
    let url = file_url(start);
    let mut args: Vec<OsString> = Vec::new();
    let (program, note) = match b.kind {
        Kind::Chromium => {
            args.push(format!("--app={url}").into());
            args.push(format!("--user-data-dir={}", profile.display()).into());
            for a in ["--no-first-run", "--no-default-browser-check", "--disable-extensions", "--disable-sync", "--disable-background-networking", "--disable-component-update", "--password-store=basic", "--window-size=1280,800"] {
                args.push(a.into());
            }
            (b.path.clone(), None)
        }
        Kind::Firefox => {
            for a in ["--no-remote", "--new-instance", "--profile"] {
                args.push(a.into());
            }
            args.push(profile.into());
            args.push(url.into());
            (b.path.clone(), Some("Firefox has no application window mode: this opens an ordinary Firefox window with its own temporary profile. Chrome, Chromium or Edge give a window without tabs."))
        }
        Kind::Safari => {
            // Safari cannot take a profile or an application window; it is opened by name on the local start page
            for a in ["-a", "Safari"] {
                args.push(a.into());
            }
            args.push(start.into());
            (PathBuf::from("open"), Some("Safari has no application window mode and cannot use a private profile folder: this opens an ordinary Safari window. Chrome, Chromium or Edge give a window without tabs."))
        }
    };
    args.extend(extra.iter().map(OsString::from));
    Command { program, args, note }
}

/// Make the private folder (mode 700 on Unix) with a start page that sends the browser to `url`, and, for the Chromium family, settings that
/// switch the browser's password manager and sign-in offers off in it. Returns (profile folder, start page).
pub fn prepare_profile(base: &Path, url: &str, kind: Kind) -> Result<(PathBuf, PathBuf), String> {
    use std::io::Write;
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
    let mut r = [0u8; 8];
    iyakki_sec::random_fill(&mut r).map_err(|_| "no random numbers".to_string())?;
    let root = base.join(format!("iyakki-window-{}", iyakki_link::sigscheme::hex(&r)));
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    let profile = root.join("profile");
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(profile.join("Default")).map_err(|e| format!("{}: {e}", profile.display()))?;
    let write = |p: &Path, body: &str| -> Result<(), String> {
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(p).map_err(|e| format!("{}: {e}", p.display()))?;
        f.write_all(body.as_bytes()).map_err(|e| e.to_string())
    };
    if kind == Kind::Chromium {
        write(&profile.join("Default").join("Preferences"), r#"{"credentials_enable_service":false,"profile":{"password_manager_enabled":false},"signin":{"allowed":false},"browser":{"check_default_browser":false}}"#)?;
    }
    let start = root.join("start.html");
    write(&start, &format!("<!doctype html><meta charset=utf-8><meta http-equiv=refresh content=\"0;url={url}\"><title>IYAKKI</title><p>Opening IYAKKI..."))?;
    Ok((profile, start))
}

/// The folder for the private profile: the runtime folder when there is one (memory-backed, per user), else the temporary folder.
pub fn profile_base() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_dir()).unwrap_or_else(std::env::temp_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};

    fn found(os: Os, vars: &[(&str, &str)], present: &[&str]) -> Option<Browser> {
        let vars: HashMap<String, String> = vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let present: HashSet<PathBuf> = present.iter().map(PathBuf::from).collect();
        let exists = |p: &Path| present.contains(p);
        let var = |k: &str| vars.get(k).cloned();
        find(&Surroundings { os, exists: &exists, var: &var })
    }

    #[test]
    fn linux_looks_through_the_path_chrome_before_chromium_before_edge_before_firefox() {
        let vars = [("PATH", "/opt/x/bin:/home/u/bin")];
        let b = found(Os::Linux, &vars, &["/usr/bin/firefox", "/usr/bin/chromium", "/home/u/bin/google-chrome"]).unwrap();
        assert_eq!((b.name, b.kind), ("Google Chrome", Kind::Chromium));
        let b = found(Os::Linux, &vars, &["/usr/bin/firefox", "/snap/bin/chromium"]).unwrap();
        assert_eq!(b.path, PathBuf::from("/snap/bin/chromium"));
        let b = found(Os::Linux, &vars, &["/usr/bin/firefox", "/usr/bin/microsoft-edge-stable"]).unwrap();
        assert_eq!(b.name, "Microsoft Edge");
        assert_eq!(found(Os::Linux, &vars, &["/usr/bin/firefox"]).unwrap().kind, Kind::Firefox);
        assert_eq!(found(Os::Linux, &vars, &[]), None);
    }

    #[test]
    fn macos_looks_in_applications_and_the_users_applications_and_falls_back_to_safari() {
        let chrome = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";
        assert_eq!(found(Os::Mac, &[("HOME", "/Users/u")], &[chrome, "/Applications/Safari.app"]).unwrap().path, PathBuf::from(chrome));
        let mine = "/Users/u/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge";
        assert_eq!(found(Os::Mac, &[("HOME", "/Users/u")], &[mine]).unwrap().name, "Microsoft Edge");
        let b = found(Os::Mac, &[("HOME", "/Users/u")], &["/Applications/Safari.app"]).unwrap();
        assert_eq!((b.kind, b.name), (Kind::Safari, "Safari"));
        assert_eq!(found(Os::Mac, &[], &[]), None);
    }

    #[test]
    fn windows_looks_in_program_files_both_flavours_and_the_users_local_folder() {
        let vars = [("ProgramFiles", "C:\\Program Files"), ("ProgramFiles(x86)", "C:\\Program Files (x86)"), ("LocalAppData", "C:\\Users\\u\\AppData\\Local")];
        let edge = "C:\\Program Files (x86)\\Microsoft\\Edge\\Application\\msedge.exe";
        assert_eq!(found(Os::Windows, &vars, &[edge, "C:\\Program Files\\Mozilla Firefox\\firefox.exe"]).unwrap().path, PathBuf::from(edge));
        let local = "C:\\Users\\u\\AppData\\Local\\Google\\Chrome\\Application\\chrome.exe";
        assert_eq!(found(Os::Windows, &vars, &[local, edge]).unwrap().name, "Google Chrome");
        assert_eq!(found(Os::Windows, &vars, &[]), None);
        assert_eq!(found(Os::Windows, &[], &[edge]), None, "without the environment nothing is guessed");
    }

    #[test]
    fn chromium_gets_an_application_window_with_a_private_profile_and_no_extensions_and_no_secret_in_the_arguments() {
        let b = Browser { kind: Kind::Chromium, name: "Google Chrome", path: "/usr/bin/google-chrome".into() };
        let c = command(&b, Path::new("/run/user/1/w/profile"), Path::new("/run/user/1/w/start page.html"), &["--headless=new".into()]);
        let args: Vec<String> = c.args.iter().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.contains(&"--app=file:///run/user/1/w/start%20page.html".to_string()), "{args:?}");
        assert!(args.contains(&"--user-data-dir=/run/user/1/w/profile".to_string()));
        for want in ["--disable-extensions", "--no-first-run", "--disable-sync", "--headless=new"] {
            assert!(args.contains(&want.to_string()), "{want}");
        }
        assert_eq!(c.note, None);
        assert!(!args.iter().any(|a| a.contains("127.0.0.1") || a.contains("token")), "the arguments carry no address and no token: {args:?}");
    }

    #[test]
    fn firefox_and_safari_get_an_ordinary_window_and_a_clear_note() {
        let ff = command(&Browser { kind: Kind::Firefox, name: "Firefox", path: "/usr/bin/firefox".into() }, Path::new("/t/p"), Path::new("/t/start.html"), &[]);
        assert!(ff.note.unwrap().contains("no application window"));
        assert!(ff.args.iter().any(|a| a == "--profile") && ff.args.iter().any(|a| a == "--no-remote"));
        let sf = command(&Browser { kind: Kind::Safari, name: "Safari", path: "/Applications/Safari.app".into() }, Path::new("/t/p"), Path::new("/t/start.html"), &[]);
        assert_eq!(sf.program, PathBuf::from("open"));
        assert!(sf.note.unwrap().contains("ordinary Safari window"));
    }

    #[test]
    fn file_urls_escape_what_must_be_escaped() {
        assert_eq!(file_url(Path::new("/a b/c#d.html")), "file:///a%20b/c%23d.html");
        assert_eq!(file_url(Path::new("C:\\Users\\u\\x.html")), "file:///C:/Users/u/x.html");
    }

    #[test]
    fn the_profile_folder_is_private_and_holds_a_start_page_that_redirects() {
        let base = std::env::temp_dir().join(format!("iyakki-browser-test-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let (profile, start) = prepare_profile(&base, "http://127.0.0.1:1/launch?t=abc", Kind::Chromium).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&profile).unwrap().permissions().mode() & 0o077, 0);
        assert_eq!(std::fs::metadata(&start).unwrap().permissions().mode() & 0o077, 0);
        assert!(std::fs::read_to_string(&start).unwrap().contains("url=http://127.0.0.1:1/launch?t=abc"));
        assert!(std::fs::read_to_string(profile.join("Default/Preferences")).unwrap().contains("password_manager_enabled\":false"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
