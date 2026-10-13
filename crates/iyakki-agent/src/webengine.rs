//! The optional full web engine.
//!
//! Two ways to run it, both separate programs (*nothing is linked into the binary*):
//!
//! * **Embedded (the way)**: a *helper program* renders the page offscreen and the shell draws its frames in the browser
//!   tile's page area (`docs/WEBVIEW-PROTOCOL.md`, client in the `iyakki-webview` crate). The helper is WPE WebKit on the
//!   `web` image variant ([`HELPER_IMAGE`], `dist/webview/wpe-helper`, built by the Buildroot package `iyakki-webview-helper`) and WebKitGTK on a development desktop (`dist/webview/host-helper.py`). This
//!   module finds the helper ([`embedded_helper`]), makes the per-person profile folder and says who may use it.
//! * **Legacy hand-over**: `cage` (kiosk Wayland compositor) running `cog`, with the shell letting go of the screen and
//!   keyboard (`iyakki-shell`'s `external` and `backend` modules). The `web` image no longer ships cage and cog; this is
//!   only for a custom image that has them and no embedded helper ([`available`] and [`run`]).
//!
//! Every program is started without a shell, in its own process group, with a small environment. The browser's
//! cookies and storage live in a private per-person folder under the data directory (`web-profile/<user>`, mode 0700).

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

pub const CAGE: &str = "/usr/bin/cage";
pub const COG: &str = "/usr/bin/cog";
/// Where the compositor's and the engine's messages go (for diagnosis: `cat /run/iyakki-web.log`).
pub const LOG: &str = "/run/iyakki-web.log";
/// The Wayland socket and friends live here while the full browser runs.
pub const RUNTIME_DIR: &str = "/run/iyakki-web";

pub const NOT_INSTALLED: &str = "The full web engine is not installed in this image (see docs/FULL-BROWSER.md)";

/// Both programs are present: this image is the `web` variant.
pub fn available() -> bool {
    Path::new(CAGE).is_file() && Path::new(COG).is_file()
}

/// A folder name made only of letters, digits, `-`, `_` and `.` (never `.` alone or `..`), so a user name
/// cannot reach outside the profile directory.
pub fn safe_name(user: &str) -> String {
    let s: String = user.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' }).take(64).collect();
    if s.is_empty() { "_".into() } else { s }
}

/// `<data>/web-profile/<user>`
pub fn profile_dir(data: &Path, user: &str) -> PathBuf {
    data.join("web-profile").join(safe_name(user))
}

/// Make the profile folder (mode 0700, also its parent) and the XDG folders inside it.
pub fn prepare_profile(data: &Path, user: &str) -> Result<PathBuf, String> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    let dir = profile_dir(data, user);
    for d in [dir.parent().map(Path::to_path_buf).unwrap_or_default(), dir.clone(), dir.join("data"), dir.join("config"), dir.join("cache")] {
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700)).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    Ok(dir)
}

/// The address the engine is started with: only `http` and `https`, no control characters, a host, at most 4 KB.
pub fn check_url(url: &str) -> Result<String, String> {
    let u = url.trim();
    if u.len() > 4096 || u.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err("that address cannot be opened".into());
    }
    iyakki_sec::url_host(u).ok_or_else(|| "only http and https addresses can be opened in the full browser".to_string())?;
    Ok(u.to_string())
}

/// May this account use the full browser for this address? It is refused when the account may not open the
/// address's host, and also for an account limited to a list of sites: the engine follows links anywhere, and
/// the host list can only be enforced on the first address.
pub fn allowed(policy: &iyakki_sec::policy::Policy, url: &str) -> Result<String, String> {
    let url = check_url(url)?;
    let host = iyakki_sec::url_host(&url).unwrap_or_default();
    if !policy.allows_host(&host) {
        return Err(format!("Your account may not open pages from {host}."));
    }
    if !policy.net_hosts.iter().any(|h| h == "*") {
        return Err("Your account is limited to some sites; the full browser can follow links to any site, so it is not available to it.".into());
    }
    Ok(url)
}

/// What `cage` is started with: `cage -s -- cog --platform=wl <url>`. `-s` lets the person switch to another
/// text console. Alt+Esc leaves the compositor; Ctrl+Q leaves the browser (and cage exits with it).
pub fn cage_argv(url: &str) -> Vec<String> {
    [CAGE, "-s", "--", COG, "--platform=wl", url].iter().map(|s| s.to_string()).collect()
}

/// The environment of the compositor and the engine (nothing else is passed on).
pub fn session_env(profile: &Path) -> Vec<(String, String)> {
    let p = |sub: &str| profile.join(sub).to_string_lossy().into_owned();
    let mut v: Vec<(String, String)> = vec![
        ("PATH".into(), "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin".into()),
        ("HOME".into(), profile.to_string_lossy().into_owned()),
        ("XDG_RUNTIME_DIR".into(), RUNTIME_DIR.into()),
        ("XDG_DATA_HOME".into(), p("data")),
        ("XDG_CONFIG_HOME".into(), p("config")),
        ("XDG_CACHE_HOME".into(), p("cache")),
        ("LANG".into(), "C.UTF-8".into()),
        // no login manager: wlroots opens the display and input devices itself, as root, through libseat's built-in backend
        ("LIBSEAT_BACKEND".into(), "builtin".into()),
        ("SEATD_VTBOUND".into(), "0".into()),
        // no GPU-specific setup: Mesa picks the hardware driver or falls back to software (llvmpipe)
        ("WLR_RENDERER".into(), "gles2".into()),
        ("XKB_DEFAULT_LAYOUT".into(), std::env::var("IYAKKI_KEYMAP").unwrap_or_else(|_| "us".into())),
        ("GST_REGISTRY_1_0".into(), p("cache/gst-registry.bin")),
    ];
    for k in ["IYAKKI_WEB_ENV_EXTRA"] {
        if let Ok(extra) = std::env::var(k) {
            // test hook: "KEY=VALUE KEY2=VALUE2" (never set on a real system)
            for kv in extra.split_whitespace() {
                if let Some((a, b)) = kv.split_once('=') {
                    v.push((a.to_string(), b.to_string()));
                }
            }
        }
    }
    v
}

fn first_existing(candidates: &[&str]) -> Option<String> {
    candidates.iter().find(|p| Path::new(p).is_file()).map(|p| p.to_string())
}

/// Start the device manager the compositor's input library needs and let it describe every device that is
/// already there. Safe to call again: it does nothing when `udevd` is already running.
pub fn prepare_devices() -> Result<(), String> {
    if Path::new("/run/udev/control").exists() {
        return Ok(());
    }
    let udevd = first_existing(&["/sbin/udevd", "/usr/sbin/udevd", "/usr/bin/udevd", "/bin/udevd", "/lib/systemd/systemd-udevd"]);
    let udevadm = first_existing(&["/sbin/udevadm", "/usr/sbin/udevadm", "/usr/bin/udevadm", "/bin/udevadm"]);
    let (Some(udevd), Some(udevadm)) = (udevd, udevadm) else {
        return Err("udevd is missing from this image".into());
    };
    let _ = std::fs::create_dir_all("/run/udev");
    let log = std::fs::OpenOptions::new().create(true).append(true).open(LOG).ok();
    let quiet = |c: &mut Command| {
        c.stdin(Stdio::null());
        match log.as_ref().and_then(|f| f.try_clone().ok()) {
            Some(f) => c.stdout(f),
            None => c.stdout(Stdio::null()),
        };
        match log.as_ref().and_then(|f| f.try_clone().ok()) {
            Some(f) => c.stderr(f),
            None => c.stderr(Stdio::null()),
        };
    };
    let mut c = Command::new(&udevd);
    c.arg("--daemon");
    quiet(&mut c);
    c.status().map_err(|e| format!("could not start udevd: {e}"))?;
    for args in [&["trigger", "--type=subsystems", "--action=add"][..], &["trigger", "--type=devices", "--action=add"][..], &["settle", "--timeout=10"][..]] {
        let mut c = Command::new(&udevadm);
        c.args(args);
        quiet(&mut c);
        let _ = c.status();
    }
    Ok(())
}

/// Ask the whole process group to stop: SIGTERM, then SIGKILL after `grace`.
fn stop_group(child: &mut Child, grace: Duration) {
    let pgid = child.id() as i32;
    // SAFETY: signalling a process group we created with `process_group(0)`.
    unsafe { libc::kill(-pgid, libc::SIGTERM) };
    let t = Instant::now();
    while t.elapsed() < grace {
        if matches!(child.try_wait(), Ok(Some(_))) {
            // the compositor is gone; make sure nothing it started is left holding the display
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    unsafe { libc::kill(-pgid, libc::SIGKILL) };
    let _ = child.kill();
    let _ = child.wait();
}

/// Run the full browser on `url` and wait until the person leaves it. The caller must already have let go of the
/// display and the input devices. Returns the exit status (0 = left normally). `stop` ends it early.
pub fn run(data: &Path, user: &str, url: &str, stop: &std::sync::atomic::AtomicBool) -> Result<i32, String> {
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::process::CommandExt;
    if !available() {
        return Err(NOT_INSTALLED.into());
    }
    let url = check_url(url)?;
    let profile = prepare_profile(data, user)?;
    let _ = std::fs::DirBuilder::new().recursive(true).mode(0o700).create(RUNTIME_DIR);
    let _ = std::fs::set_permissions(RUNTIME_DIR, std::os::unix::fs::PermissionsExt::from_mode(0o700));
    if let Err(e) = prepare_devices() {
        log::warn!("webengine: {e}");
    }
    let log = std::fs::OpenOptions::new().create(true).append(true).open(LOG).map_err(|e| format!("{LOG}: {e}"))?;
    let argv = cage_argv(&url);
    let mut cmd = Command::new(&argv[0]);
    cmd.args(&argv[1..]).env_clear().envs(session_env(&profile)).current_dir(&profile);
    cmd.process_group(0).stdin(Stdio::null()).stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
    let mut child = cmd.spawn().map_err(|e| format!("could not start cage: {e}"))?;
    log::info!("webengine: started cage (pid {}) for {}", child.id(), iyakki_sec::url_host(&url).unwrap_or_default());
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                // cage exits when cog does; anything still alive in the group is stopped
                unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
                return Ok(status.code().unwrap_or(-1));
            }
            Ok(None) => {}
            Err(e) => return Err(format!("waiting for cage: {e}")),
        }
        if stop.load(std::sync::atomic::Ordering::Relaxed) {
            stop_group(&mut child, Duration::from_secs(3));
            return Ok(0);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

// ------------------------------------------------------------------------------------- the embedded engine

/// Where the image puts the helper program (WPE WebKit, speaking `docs/WEBVIEW-PROTOCOL.md`).
pub const HELPER_IMAGE: &str = "/usr/libexec/iyakki/webview-helper";
/// Names a helper program to use instead of the ones below (tests, or a person's own build).
pub const HELPER_ENV: &str = "IYAKKI_WEBVIEW_HELPER";

static HOST_MODE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The desktop window (`iyakki-desktop`) says it runs on a development machine's desktop, so the helper may be the
/// WebKitGTK one of this repository and runs with the desktop's own environment.
pub fn set_host_mode(on: bool) {
    HOST_MODE.store(on, std::sync::atomic::Ordering::Relaxed);
}

/// A desktop session (window mode on a development machine, not the `web` image).
pub fn host_session() -> bool {
    HOST_MODE.load(std::sync::atomic::Ordering::Relaxed) && !available()
}

/// `dist/webview/host-helper.py` of the checkout this program runs from: next to the executable (`target/debug/..`), up the
/// directory tree from it, or where this source was compiled.
fn host_helper_in_checkout() -> Option<PathBuf> {
    const REL: &str = "dist/webview/host-helper.py";
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        candidates.extend(exe.ancestors().skip(1).map(|d| d.join(REL)));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.extend(cwd.ancestors().map(|d| d.join(REL)));
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").join(REL));
    candidates.into_iter().find(|p| p.is_file())
}

/// The program that renders pages for the shell: `IYAKKI_WEBVIEW_HELPER`, else the image's WPE helper, else (window mode on
/// a desktop) the WebKitGTK helper script of the checkout.
pub fn embedded_helper() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os(HELPER_ENV).map(PathBuf::from).filter(|p| p.is_file()) {
        return Some(p);
    }
    let img = PathBuf::from(HELPER_IMAGE);
    if img.is_file() {
        return Some(img);
    }
    if HOST_MODE.load(std::sync::atomic::Ordering::Relaxed) {
        return host_helper_in_checkout();
    }
    None
}

/// The embedded engine can be used.
pub fn embedded_available() -> bool {
    embedded_helper().is_some()
}

/// The full browser can be offered: the embedded engine, or an image with cage and cog.
pub fn offered() -> bool {
    embedded_available() || available()
}

/// The environment the helper runs in: on a desktop its own (it needs the display, D-Bus, audio), on the image a small one
/// with the person's folders (`None` means "inherit").
pub fn helper_env(profile: &Path) -> Option<Vec<(String, String)>> {
    if host_session() || !embedded_helper().is_some_and(|p| p == Path::new(HELPER_IMAGE)) {
        return None;
    }
    let mut v = session_env(profile);
    v.retain(|(k, _)| !k.starts_with("WLR_") && !k.starts_with("LIBSEAT") && k != "SEATD_VTBOUND");
    Some(v)
}

/// Added to the engine's own user agent so sites see a current browser name followed by ours.
pub fn user_agent_suffix() -> String {
    format!("IYAKKI/{}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_sec::policy::Role;

    #[test]
    fn user_names_cannot_leave_the_profile_folder() {
        assert_eq!(safe_name("alice"), "alice");
        assert_eq!(safe_name("../etc"), "___etc");
        assert_eq!(safe_name(""), "_");
        assert_eq!(safe_name("a/b c"), "a_b_c");
        assert_eq!(profile_dir(Path::new("/var/lib/iyakki"), "bob"), PathBuf::from("/var/lib/iyakki/web-profile/bob"));
    }

    #[test]
    fn only_web_addresses_are_accepted() {
        assert!(check_url("https://www.youtube.com/watch?v=x").is_ok());
        assert!(check_url("http://10.0.2.2:8000/").is_ok());
        for bad in ["file:///etc/passwd", "javascript:alert(1)", "https://", "https://a b", "https://u@h/", "--help", "https://x/\u{7}", ""] {
            assert!(check_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn the_account_policy_is_applied_first() {
        let mut p = Role::User.template();
        p.net_hosts = vec!["*".into()];
        assert!(allowed(&p, "https://www.youtube.com/").is_ok());
        p.net_hosts = vec!["example.org".into()];
        assert!(allowed(&p, "https://www.youtube.com/").unwrap_err().contains("may not open pages from www.youtube.com"));
        let limited = allowed(&p, "https://example.org/").unwrap_err();
        assert!(limited.contains("limited to some sites"), "{limited}");
        p.net_hosts.clear();
        assert!(allowed(&p, "https://example.org/").is_err());
    }

    #[test]
    fn the_command_line_has_no_shell_and_the_address_is_one_argument() {
        let a = cage_argv("https://a.org/?q=1&r=2");
        assert_eq!(a, vec![CAGE, "-s", "--", COG, "--platform=wl", "https://a.org/?q=1&r=2"]);
    }

    #[test]
    fn the_profile_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("iyakki-webprofile-{}", std::process::id()));
        let p = prepare_profile(&dir, "carol").unwrap();
        for d in [p.clone(), p.join("data"), dir.join("web-profile")] {
            assert_eq!(std::fs::metadata(&d).unwrap().permissions().mode() & 0o777, 0o700, "{}", d.display());
        }
        let env = session_env(&p);
        assert!(env.iter().any(|(k, v)| k == "XDG_DATA_HOME" && v.ends_with("carol/data")));
        assert!(env.iter().any(|(k, v)| k == "XDG_CACHE_HOME" && v.ends_with("carol/cache")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_helper_is_found_by_override_image_path_or_checkout() {
        let dir = std::env::temp_dir().join(format!("iyakki-helper-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let prog = dir.join("helper");
        std::fs::write(&prog, "#!/bin/sh\n").unwrap();
        std::env::set_var(HELPER_ENV, &prog);
        assert_eq!(embedded_helper(), Some(prog.clone()));
        assert!(embedded_available() && offered());
        std::env::set_var(HELPER_ENV, dir.join("missing"));
        // a bad override falls through to the other places; the checkout's script is only used in window mode
        set_host_mode(false);
        if !Path::new(HELPER_IMAGE).is_file() {
            assert_eq!(embedded_helper(), None);
        }
        set_host_mode(true);
        assert!(embedded_helper().is_none_or(|p| p.ends_with("dist/webview/host-helper.py") || p == Path::new(HELPER_IMAGE)));
        set_host_mode(false);
        std::env::remove_var(HELPER_ENV);
        let _ = std::fs::remove_dir_all(dir);
        assert!(host_helper_in_checkout().is_some_and(|p| p.ends_with("dist/webview/host-helper.py")), "the helper script is part of the repository");
        assert!(user_agent_suffix().starts_with("IYAKKI/"));
    }
}
