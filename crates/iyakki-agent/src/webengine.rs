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

/// Sites that are web apps the built-in reader cannot run (fast JavaScript, video, Media Source, WebGL): (domain, name).
/// A host matches when it is the domain or a sub-domain of it. Used only to say so plainly when no full engine is installed.
const HEAVY_SITES: &[(&str, &str)] = &[
    ("youtube.com", "YouTube"),
    ("youtu.be", "YouTube"),
    ("youtube-nocookie.com", "YouTube"),
    ("netflix.com", "Netflix"),
    ("mail.google.com", "Gmail"),
    ("gmail.com", "Gmail"),
    ("accounts.google.com", "Google sign-in"),
    ("outlook.live.com", "Outlook"),
    ("outlook.office.com", "Outlook"),
    ("outlook.office365.com", "Outlook"),
    ("docs.google.com", "Google Docs"),
    ("sheets.google.com", "Google Sheets"),
    ("slides.google.com", "Google Slides"),
    ("drive.google.com", "Google Drive"),
    ("slack.com", "Slack"),
    ("notion.so", "Notion"),
    ("twitter.com", "Twitter"),
    ("x.com", "X"),
    ("facebook.com", "Facebook"),
    ("instagram.com", "Instagram"),
    ("maps.google.com", "Google Maps"),
    ("web.whatsapp.com", "WhatsApp Web"),
    ("teams.microsoft.com", "Microsoft Teams"),
    ("figma.com", "Figma"),
    ("spotify.com", "Spotify"),
];

/// The name of a known heavy site when `host` belongs to it (`music.youtube.com` and `www.youtube.com` are YouTube).
pub fn heavy_site(host: &str) -> Option<&'static str> {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    HEAVY_SITES.iter().find(|(d, _)| h == *d || h.ends_with(&format!(".{d}"))).map(|(_, n)| *n)
}

/// Sites the BROWSER tile opens straight in the full engine when the image has one, without trying the built-in reader first: the
/// heavy sites, and Google itself (its pages load in the reader but come out empty or stripped, which looks like "not loading").
pub fn full_by_default(host: &str) -> bool {
    let h = host.trim().trim_end_matches('.').to_ascii_lowercase();
    heavy_site(&h).is_some() || h == "google.com" || h.ends_with(".google.com") || h == "youtu.be"
}

/// What the BROWSER tile says above a page when the full engine is absent (`offered` false) and the page is one the built-in
/// reader cannot run: a known heavy site, or a page the reader judged it could not show (`reader_gave_up`).
/// `(headline, explanation)`; `None` when the engine is there or the page is fine.
pub fn missing_engine_banner(host: &str, reader_gave_up: bool, offered: bool) -> Option<(String, String)> {
    if offered {
        return None;
    }
    let site = heavy_site(host);
    if site.is_none() && !reader_gave_up {
        return None;
    }
    let what = site.map_or_else(|| "This page".to_string(), |n| n.to_string());
    Some((
        "This site needs the full web engine.".into(),
        format!("{what} will not load properly in the built-in reader. Boot the \"web\" or \"macweb\" image (macweb is the Mac image with the web engine), or use window mode. Web searches, Wikipedia and most articles work here."),
    ))
}

/// The help the banner's button opens: an HTML page (kept short; the long version is docs/FULL-BROWSER.md).
pub const FULL_ENGINE_HELP_HTML: &str = "<html><head><title>The full web engine</title></head><body>\
<h1>The full web engine</h1>\
<p>The built-in reader shows documents and runs small scripts. YouTube, Gmail, Google Docs, Netflix, Slack, Notion, X, Facebook, Instagram and Maps need a full web engine (fast JavaScript, video, WebGL). This image does not have one.</p>\
<h2>Ways to get it</h2>\
<ul>\
<li>Boot the <b>web</b> image (any PC) or the <b>macweb</b> image (Intel Macs such as the Mac mini 2014): the engine is WPE WebKit in a helper program, and the BROWSER tile uses it by itself or when you press F.</li>\
<li>Use <b>window mode</b> on a desktop (./run.sh window): the host's WebKitGTK is the engine.</li>\
</ul>\
<h2>Good to know</h2>\
<ul>\
<li>YouTube plays VP9 and Opus on the web images without H.264. Build with WEB_H264=1 for H.264 and AAC.</li>\
<li>Sign-in and video need the clock to be right: a wrong date breaks HTTPS.</li>\
<li>Details: docs/FULL-BROWSER.md and docs/YOUTUBE-ON-IYAKKI.md in the source tree.</li>\
</ul></body></html>";

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
    let vendors: Vec<String> = (0..4).filter_map(|i| std::fs::read_to_string(format!("/sys/class/drm/card{i}/device/vendor")).ok()).collect();
    let marker = std::fs::read_to_string(GL_MARKER).unwrap_or_default();
    let software = force_software_gl(&vendors, &marker);
    if software {
        v.push(("LIBGL_ALWAYS_SOFTWARE".into(), "1".into()));
    }
    // rendering experiments an administrator switches on by writing a word into <data>/web-profile/web-tuning (docs/BROWSER-SPEED.md)
    let mode = profile.parent().and_then(|d| std::fs::read_to_string(d.join("web-tuning")).ok()).unwrap_or_default();
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(2);
    let tuned = tuning_env(&mode, cores);
    // Mesa's software renderer starts one thread per logical CPU by default: half of them unless the web-tuning file says `lp:N`
    if software && !tuned.iter().any(|(k, _)| k == "LP_NUM_THREADS") {
        v.push(("LP_NUM_THREADS".into(), llvmpipe_threads(cores).to_string()));
    }
    v.extend(tuned);
    // sound: where the sound output is a T2 Mac's, the engine plays through its own PCM (softvol.rs), see web_audio_env
    let asound = std::fs::read_to_string(ASOUND_CONF).unwrap_or_default();
    if let Some(dir) = profile.parent() {
        v.extend(web_audio_env(&asound, &dir.join("web-alsa.conf")));
    }
    v.extend(web_autoplay_env(&asound));
    Some(v)
}

/// The system's ALSA configuration, written by the sound watcher ([`crate::softvol`]).
const ASOUND_CONF: &str = "/etc/asound.conf";

/// `ALSA_CONFIG_PATH` for the engine's processes when the system configuration has the web PCM ([`crate::softvol::WEB_PCM`]):
/// the file (written here) loads the system's ALSA configuration and makes that PCM the engine's `default`, so GStreamer's
/// `alsasink` never goes through the rate conversion that is broken on the T2 sound device. Empty on every other machine.
pub fn web_audio_env(asound_conf: &str, conf_path: &Path) -> Vec<(String, String)> {
    let defined = asound_conf.contains(&format!("pcm.{} {{", crate::softvol::WEB_PCM));
    if !defined {
        return Vec::new();
    }
    if std::fs::read_to_string(conf_path).ok().as_deref() != Some(crate::softvol::WEB_ALSA_CONF) && std::fs::write(conf_path, crate::softvol::WEB_ALSA_CONF).is_err() {
        return Vec::new();
    }
    vec![("ALSA_CONFIG_PATH".to_string(), conf_path.to_string_lossy().into_owned())]
}

/// `IYAKKI_WEBVIEW_AUTOPLAY=deny` where the sound output cannot be shared (a configuration without dmix: the T2 Mac's speaker, a card
/// with a hardware volume). The first media element that opens the device holds it, and pages such as YouTube open several muted
/// previews on their own, so the video that is meant to be heard stays silent. With deny a click starts the media instead.
/// Nothing is set where the system configuration is unknown (empty) or mixes (dmix).
pub fn web_autoplay_env(asound_conf: &str) -> Vec<(String, String)> {
    if asound_conf.trim().is_empty() || asound_conf.contains("type dmix") {
        return Vec::new();
    }
    vec![("IYAKKI_WEBVIEW_AUTOPLAY".to_string(), "deny".to_string())]
}

/// How many threads Mesa's software renderer (llvmpipe) may use for the engine: half the logical CPUs, at least 2. By default it
/// starts one per logical CPU, and on a 2020 MacBook Pro (8 threads, no Intel driver in the image) that left the video decoder no
/// core of its own: a 720p60 YouTube video dropped 428 to 564 of about 700 frames in four runs, and none in three runs with 4 threads
/// (docs/BROWSER-SPEED.md, "Interactive performance"). Pages that only scroll used 30 to 45 % less CPU for the same frame rate.
pub fn llvmpipe_threads(cores: usize) -> usize {
    (cores / 2).clamp(2, 16).min(cores.max(1))
}

/// Engine settings for the experiment words in `<data>/web-profile/web-tuning` (separated by spaces; `docs/BROWSER-SPEED.md`):
/// `cpu` paints on the processor (WebKit's default for painting is the GPU through GL, which on a machine whose only GL is llvmpipe
/// means software GL plus a copy), `cpu:N` the same with N painting threads (WebKit's default is half the cores), `gpu:N` GL
/// painting with N threads (default 1, 0 paints on the main thread), `lp:N` N llvmpipe threads (default [`llvmpipe_threads`]),
/// `gstgl:off` GStreamer's own video sink instead of the GL one, `nice:N` (1 to 19) the engine N steps below the shell, `fps:N` (5 to 60) the most pictures a second the helper sends (default 30). Anything else, including an empty file, changes nothing. The names
/// were read from the WPE WebKit 2.50 sources shipped in the image build (`SkiaPaintingEngine.cpp`, `WebProcessGLib.cpp`) and from
/// the shipped binary; which one is faster on a given Mac has to be measured there.
pub fn tuning_env(mode: &str, cores: usize) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = Vec::new();
    let mut push = |v: &mut Vec<(String, String)>, k: &str, val: String| {
        v.retain(|(name, _)| name != k);
        v.push((k.to_string(), val));
    };
    for word in mode.split_whitespace().take(8) {
        let (kind, arg) = match word.split_once(':') {
            Some((k, a)) => (k, Some(a.trim())),
            None => (word, None),
        };
        let n = arg.and_then(|a| a.parse::<usize>().ok()).map(|n| n.min(cores.max(1)));
        match kind {
            "cpu" => {
                push(&mut v, "WEBKIT_SKIA_ENABLE_CPU_RENDERING", "1".to_string());
                if let Some(n) = n {
                    push(&mut v, "WEBKIT_SKIA_CPU_PAINTING_THREADS", n.to_string());
                }
            }
            "gpu" => {
                if let Some(n) = n {
                    push(&mut v, "WEBKIT_SKIA_GPU_PAINTING_THREADS", n.to_string());
                }
            }
            "lp" => {
                if let Some(n) = n.filter(|n| *n >= 1) {
                    push(&mut v, "LP_NUM_THREADS", n.to_string());
                }
            }
            "gstgl" if arg == Some("off") => push(&mut v, "WEBKIT_GST_DISABLE_GL_SINK", "1".to_string()),
            // the helper's own switches (helper.c): run the engine N steps below the shell; the most frames a second it sends
            "nice" => {
                if let Some(n) = arg.and_then(|a| a.parse::<u32>().ok()).filter(|n| (1..=19).contains(n)) {
                    push(&mut v, "IYAKKI_WEBVIEW_NICE", n.to_string());
                }
            }
            "fps" => {
                if let Some(n) = arg.and_then(|a| a.parse::<u32>().ok()).filter(|n| (5..=60).contains(n)) {
                    push(&mut v, "IYAKKI_WEBVIEW_FPS", n.to_string());
                }
            }
            _ => {}
        }
    }
    v
}

/// The two variables that make the helper load the person's content-filter rule list (written by the browser next to the
/// profile when the lists change): empty when there is none or blocking is off.
pub fn filter_env(profile: &Path) -> Vec<(String, String)> {
    let json = profile.join("content-filter.json");
    let id = std::fs::read_to_string(profile.join("content-filter.id")).unwrap_or_default();
    let id = id.trim();
    if json.is_file() && !id.is_empty() && id.len() <= 64 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_') {
        vec![("IYAKKI_WEBVIEW_FILTER".into(), json.to_string_lossy().into_owned()), ("IYAKKI_WEBVIEW_FILTER_ID".into(), id.to_string())]
    } else {
        Vec::new()
    }
}

/// Written by `web-post-build.sh`: `intel` when Mesa was built with an Intel GPU driver (iris or crocus), `software` when not.
pub const GL_MARKER: &str = "/usr/share/iyakki/web-gl-drivers";

/// Mesa must be told to use llvmpipe when an Intel GPU is present but this image has no Intel driver in it: otherwise EGL finds the
/// render node, finds no driver for it and may give up instead of falling back (the Haswell Mac mini on the default web image).
/// `vendors` are the contents of `/sys/class/drm/card*/device/vendor`.
pub fn force_software_gl(vendors: &[String], marker: &str) -> bool {
    vendors.iter().any(|v| v.trim().eq_ignore_ascii_case("0x8086")) && marker.trim() != "intel"
}

/// Added to the engine's own user agent so sites see a current browser name followed by ours.
pub fn user_agent_suffix() -> String {
    format!("IYAKKI/{}", env!("CARGO_PKG_VERSION"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_engine_gets_its_own_alsa_default_only_where_the_system_configuration_has_the_web_pcm() {
        let d = std::env::temp_dir().join(format!("iyakki-webalsa-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("web-alsa.conf");
        assert!(super::web_audio_env("pcm.iyakkivol {\n}\n", &f).is_empty());
        assert!(!f.exists());
        let env = super::web_audio_env("pcm.iyakkiweb {\n type linear\n}\n", &f);
        assert_eq!(env, vec![("ALSA_CONFIG_PATH".to_string(), f.to_string_lossy().into_owned())]);
        assert_eq!(std::fs::read_to_string(&f).unwrap(), crate::softvol::WEB_ALSA_CONF);
        assert!(super::web_audio_env("pcm.iyakkiweb {\n}\n", &d.join("no/such/dir/x.conf")).is_empty());
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn autoplay_is_denied_only_where_the_sound_output_cannot_be_shared() {
        assert!(super::web_autoplay_env("").is_empty(), "unknown configuration: unchanged");
        assert!(super::web_autoplay_env("pcm.iyakkimix {\n    type dmix\n}\n").is_empty(), "a mixing output shares the device");
        let env = super::web_autoplay_env("pcm.iyakkivol {\n    type softvol\n    slave.pcm \"plughw:0,0\"\n}\n");
        assert_eq!(env, vec![("IYAKKI_WEBVIEW_AUTOPLAY".to_string(), "deny".to_string())]);
    }

    use super::*;
    use iyakki_sec::policy::Role;

    #[test]
    fn known_heavy_sites_are_recognised_by_domain_and_subdomain_only() {
        for (h, n) in [("youtube.com", "YouTube"), ("www.youtube.com", "YouTube"), ("music.youtube.com", "YouTube"), ("youtu.be", "YouTube"), ("WWW.NETFLIX.COM", "Netflix"),
            ("mail.google.com", "Gmail"), ("docs.google.com", "Google Docs"), ("app.slack.com", "Slack"), ("www.notion.so", "Notion"), ("x.com", "X"), ("twitter.com", "Twitter"),
            ("m.facebook.com", "Facebook"), ("www.instagram.com", "Instagram"), ("maps.google.com", "Google Maps"), ("youtube.com.", "YouTube")] {
            assert_eq!(heavy_site(h), Some(n), "{h}");
        }
        for h in ["example.org", "notyoutube.com", "box.com", "youtube.com.evil.org", "wikipedia.org", "google.com", ""] {
            assert_eq!(heavy_site(h), None, "{h}");
        }
    }

    #[test]
    fn google_and_the_heavy_sites_go_straight_to_the_full_engine() {
        for h in ["google.com", "www.google.com", "accounts.google.com", "WWW.YOUTUBE.COM", "youtu.be", "mail.google.com", "google.com."] {
            assert!(full_by_default(h), "{h}");
        }
        for h in ["example.org", "notgoogle.com", "google.com.evil.org", "wikipedia.org", "www.bing.com", ""] {
            assert!(!full_by_default(h), "{h}");
        }
    }

    #[test]
    fn an_intel_gpu_without_an_intel_mesa_driver_is_told_to_use_software_rendering() {
        let intel = vec!["0x8086\n".to_string()];
        assert!(force_software_gl(&intel, "software\n"));
        assert!(force_software_gl(&intel, ""), "no marker: a plain build");
        assert!(!force_software_gl(&intel, "intel\n"), "iris or crocus is there");
        assert!(!force_software_gl(&["0x1002\n".into()], "software"), "radeonsi is built");
        assert!(!force_software_gl(&[], "software"), "no GPU (a VM without one)");
    }

    #[test]
    fn the_missing_engine_banner_shows_only_without_the_engine() {
        let (head, body) = missing_engine_banner("www.youtube.com", false, false).unwrap();
        assert_eq!(head, "This site needs the full web engine.");
        assert!(body.starts_with("YouTube") && body.contains("\"web\" or \"macweb\" image") && body.contains("window mode"), "{body}");
        assert!(missing_engine_banner("www.youtube.com", false, true).is_none(), "the engine is here: no banner");
        assert!(missing_engine_banner("example.org", false, false).is_none(), "an ordinary page gets none");
        let (_, generic) = missing_engine_banner("example.org", true, false).unwrap();
        assert!(generic.starts_with("This page"), "{generic}");
        assert!(missing_engine_banner("example.org", true, true).is_none());
        assert!(FULL_ENGINE_HELP_HTML.contains("macweb") && FULL_ENGINE_HELP_HTML.contains("window mode"));
    }

    #[test]
    fn rendering_experiments_are_off_unless_asked_for() {
        assert!(tuning_env("", 8).is_empty() && tuning_env("gpu", 8).is_empty() && tuning_env("nonsense", 8).is_empty() && tuning_env("cpu:x", 8).len() == 1);
        assert_eq!(tuning_env("cpu\n", 8), vec![("WEBKIT_SKIA_ENABLE_CPU_RENDERING".to_string(), "1".to_string())]);
        let v = tuning_env("cpu:4", 8);
        assert_eq!(v[1], ("WEBKIT_SKIA_CPU_PAINTING_THREADS".to_string(), "4".to_string()));
        assert_eq!(tuning_env("cpu:64", 2)[1].1, "2", "never more threads than cores");
        assert_eq!(tuning_env("gpu:0", 8), vec![("WEBKIT_SKIA_GPU_PAINTING_THREADS".to_string(), "0".to_string())]);
        // several words, llvmpipe threads, the video sink; a later word for the same variable wins; junk is ignored
        let v = tuning_env("cpu:4 lp:3 gstgl:off lp:2 zzz lp:0 gstgl:on", 8);
        let get = |k: &str| v.iter().find(|(n, _)| n == k).map(|(_, x)| x.as_str());
        assert_eq!((get("LP_NUM_THREADS"), get("WEBKIT_GST_DISABLE_GL_SINK"), get("WEBKIT_SKIA_CPU_PAINTING_THREADS")), (Some("2"), Some("1"), Some("4")));
        assert_eq!(v.iter().filter(|(n, _)| n == "LP_NUM_THREADS").count(), 1, "one entry per variable");
        assert_eq!(tuning_env("nice:5 fps:60", 8), vec![("IYAKKI_WEBVIEW_NICE".to_string(), "5".to_string()), ("IYAKKI_WEBVIEW_FPS".to_string(), "60".to_string())]);
        assert!(tuning_env("nice:0 nice:20 nice:-1 nice fps:4 fps:61 fps", 8).is_empty(), "out of range values change nothing");
        assert!(tuning_env("lp:0", 8).is_empty() && tuning_env("lp", 8).is_empty() && tuning_env("gstgl", 8).is_empty());
        assert_eq!(tuning_env("lp:64", 4)[0].1, "4", "never more threads than cores");
        assert_eq!(tuning_env(&"cpu ".repeat(100), 8).len(), 1, "a long file is bounded");
        // llvmpipe gets half the logical CPUs, at least two (one on a single-CPU machine)
        assert_eq!([llvmpipe_threads(1), llvmpipe_threads(2), llvmpipe_threads(4), llvmpipe_threads(8), llvmpipe_threads(16), llvmpipe_threads(64)], [1, 2, 2, 4, 8, 16]);
    }

    #[test]
    fn the_filter_variables_need_both_files_and_a_safe_id() {
        let d = std::env::temp_dir().join(format!("iyakki-fenv-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert!(filter_env(&d).is_empty());
        std::fs::write(d.join("content-filter.json"), "[]").unwrap();
        assert!(filter_env(&d).is_empty(), "no id yet");
        std::fs::write(d.join("content-filter.id"), "ab12-CD_3\n").unwrap();
        let v = filter_env(&d);
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].1, "ab12-CD_3");
        std::fs::write(d.join("content-filter.id"), "../x").unwrap();
        assert!(filter_env(&d).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

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
