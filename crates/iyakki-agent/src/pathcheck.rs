//! The smallest real call of every runtime path that used to start an external program (or needs the system's CA bundle), for the boot
//! self-test (`iyakki.selftest=1`, `dist/buildroot/qemu-test.sh`). Fake runners cannot tell that an image lacks `curl` or
//! `ca-certificates.crt`; these calls can, because each one goes through the production code with no fake: the in-house HTTP client, the
//! registry probe, the registry client, the cluster tools' runner, the Git client and DOCTOR's probes.
//!
//! * Every call goes to a loopback listener the self-test starts itself ([`Loopback`]): never the internet, so the check needs no network.
//! * A path is `ok` only when its call really worked; a missing program or a missing CA bundle is a `FAILED` line that says which.
//! * The names are the contract with `qemu-test.sh`, which requires each `IYAKKI-SELFTEST path <name> ok`.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// The paths, in the order they are checked (`qemu-test.sh` requires every one).
pub const PATHS: [&str; 10] = ["ca-bundle", "programs", "platform-connect", "registry-probe", "registry-setup", "cluster-tools", "git-client", "doctor-probes", "mp3-decoder", "web-recorder"];

/// Programs the paths above rely on that the image ships (`dist/buildroot/image-programs.txt`, kind `image`).
pub const IMAGE_PROGRAMS: [&str; 7] = ["sh", "ip", "git", "ssh", "aplay", "arecord", "amixer"];

/// A tiny HTTP server on 127.0.0.1: every request is answered `200` with a small JSON body, until it is dropped.
pub struct Loopback {
    pub port: u16,
    stop: Arc<AtomicBool>,
}

impl Loopback {
    pub fn start() -> Result<Loopback, String> {
        let l = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("cannot listen on 127.0.0.1 (is the loopback interface up?): {e}"))?;
        let port = l.local_addr().map_err(|e| e.to_string())?.port();
        l.set_nonblocking(true).map_err(|e| e.to_string())?;
        let stop = Arc::new(AtomicBool::new(false));
        let s2 = stop.clone();
        std::thread::Builder::new()
            .name("iyakki-selftest-loopback".into())
            .spawn(move || {
                while !s2.load(Ordering::Relaxed) {
                    match l.accept() {
                        Ok((c, _)) => {
                            let _ = c.set_nonblocking(false);
                            std::thread::spawn(move || answer(c));
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(10)),
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(Loopback { port, stop })
    }

    pub fn host(&self) -> String {
        format!("127.0.0.1:{}", self.port)
    }

    pub fn url(&self, path: &str) -> String {
        format!("http://127.0.0.1:{}{path}", self.port)
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn answer(mut c: TcpStream) {
    let _ = c.set_read_timeout(Some(Duration::from_secs(5)));
    let mut got = Vec::new();
    let mut buf = [0u8; 2048];
    // read to the end of the head; the requests here carry no body that matters
    while !got.windows(4).any(|w| w == b"\r\n\r\n") && got.len() < 16 * 1024 {
        match c.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => got.extend_from_slice(&buf[..n]),
        }
    }
    let body = r#"{"version":"selftest"}"#;
    let _ = c.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nDocker-Distribution-Api-Version: registry/2.0\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes());
}

/// One path's verdict: what it proved, or why it failed (one line).
pub type Verdict = Result<String, String>;

/// Run one path by name. Unknown names fail.
pub fn check(name: &str, lo: &Loopback) -> Verdict {
    match name {
        "ca-bundle" => crate::gitnet::ca_bundle_certs().map(|n| format!("{n} trusted authorities")),
        "programs" => programs(),
        "platform-connect" => platform_connect(lo),
        "registry-probe" => registry_probe(lo),
        "registry-setup" => registry_setup(lo),
        "cluster-tools" => cluster_tools(),
        "git-client" => git_client(lo),
        "doctor-probes" => doctor_probes(lo),
        "mp3-decoder" => mp3_decoder(),
        "web-recorder" => web_recorder(),
        other => Err(format!("no such path: {other}")),
    }
}

/// Every program the image should ship for these paths is on `PATH`.
fn programs() -> Verdict {
    let missing: Vec<&str> = IMAGE_PROGRAMS.iter().copied().filter(|p| !crate::wsexec::on_path(p)).collect();
    if missing.is_empty() { Ok(format!("{} programs", IMAGE_PROGRAMS.len())) } else { Err(format!("the image lacks: {}", missing.join(", "))) }
}

/// Platform connect: the Gitea / platform client, a token in memory, `GET /api/v1/version`.
fn platform_connect(lo: &Loopback) -> Verdict {
    use crate::platform::connect::{Http, NetHttp};
    let r = NetHttp::default().request("GET", &lo.url("/api/v1/version"), Some("selftest-token"), None)?;
    if r.status == 200 && r.body.contains("selftest") { Ok("answered 200".into()) } else { Err(format!("unexpected answer {}", r.status)) }
}

/// Registry probe: "is the registry answering" (`netprobe`, the call the cluster tools and Settings make).
fn registry_probe(lo: &Loopback) -> Verdict {
    if !crate::netprobe::get_ok(&lo.url("/v2/"), 4) {
        return Err("the probe got no 2xx answer from the loopback registry".into());
    }
    let body = crate::netprobe::get_body(&lo.url("/v2/_catalog"), 4, 4096)?;
    if body.contains("selftest") { Ok("answered".into()) } else { Err("unexpected body".into()) }
}

/// Registry setup: the registry client that pushes and checks images (`ocipush::TcpHttp`) and the reachability check that accepts any
/// certificate (`regsetup`'s `/v2/` test).
fn registry_setup(lo: &Loopback) -> Verdict {
    use crate::ocipush::{RegistryHttp, TcpHttp};
    let r = TcpHttp::default().request("GET", &lo.host(), "/v2/", &[], &[])?;
    if r.status != 200 {
        return Err(format!("the registry client got {}", r.status));
    }
    let a = crate::netprobe::call("GET", &lo.url("/v2/"), None, 3, 4096, true)?;
    if a.success() { Ok("answered".into()) } else { Err(format!("the reachability test got {}", a.status)) }
}

/// Cluster tools: `kubectl` is downloaded with k3s at the person's click, so on a fresh image its absence must be answered in words, not by
/// a crash; when it is there it must run.
fn cluster_tools() -> Verdict {
    use crate::wsexec::{Runner, System};
    let argv: Vec<String> = ["kubectl", "version", "--client"].iter().map(|s| s.to_string()).collect();
    let out = System.run(std::path::Path::new("/"), &argv, &[], Duration::from_secs(20), 16 * 1024, None);
    if System.has("kubectl") {
        return if out.code == Some(0) { Ok("kubectl runs".into()) } else { Err(format!("kubectl is installed but failed: {}", out.stderr.lines().next().unwrap_or("").trim())) };
    }
    if !out.missing {
        return Err("a missing kubectl was not reported as missing".into());
    }
    let words = crate::localreg::explain_kubectl("kubectl is not installed");
    if words.contains("no kubectl") { Ok("kubectl is not installed yet; the tools say so in words".into()) } else { Err("the missing-kubectl explanation is gone".into()) }
}

/// The Git client: the in-house HTTP client with a token, and the `git` program the image ships.
fn git_client(lo: &Loopback) -> Verdict {
    use crate::gitconn::{GitHttp, Req, Style};
    let req = Req { method: "GET".into(), url: lo.url("/api/v1/user"), token: Some("selftest-token".into()), style: Style::Token, body: None, cert: None, accept: None, max_bytes: 0, timeout_s: 5 };
    let r = crate::gitnet::NetGit::default().send(&req).map_err(|e| format!("{e:?}"))?;
    if r.status != 200 {
        return Err(format!("the Git client got {}", r.status));
    }
    use crate::wsexec::{Runner, System};
    let argv: Vec<String> = ["git", "--version"].iter().map(|s| s.to_string()).collect();
    let out = System.run(std::path::Path::new("/"), &argv, &[], Duration::from_secs(10), 4096, None);
    if out.missing {
        return Err("git is not on this system".into());
    }
    if out.code != Some(0) || !out.stdout.contains("git version") {
        return Err(format!("git --version failed: {}", out.stderr.lines().next().unwrap_or("").trim()));
    }
    Ok(out.stdout.lines().next().unwrap_or("").trim().to_string())
}

/// DOCTOR's probes: a connection, the interface list, the uptime and the hardware report.
fn doctor_probes(lo: &Loopback) -> Verdict {
    if !crate::doctor::answers(&lo.host(), Duration::from_millis(600)) {
        return Err("the connection probe does not see the loopback listener".into());
    }
    if !crate::doctor::has_address("127.0.0.1") {
        return Err("the interface probe does not list 127.0.0.1 (the loopback interface is down or missing)".into());
    }
    if crate::doctor::uptime_secs() > 10 * 365 * 24 * 3600 {
        return Err("the uptime cannot be read (/proc)".into());
    }
    if crate::doctor::hardware_text().trim().is_empty() {
        return Err("the hardware report is empty".into());
    }
    Ok("connection, interfaces, uptime, hardware".into())
}

/// The MP3 plugin the web engine plays a page's MP3 through (`dist/webview/wpe-helper/mp3dec.c`); only the web images carry it.
pub const MP3_PLUGIN: &str = "/usr/lib/gstreamer-1.0/libgstiyakkimp3.so";
/// A 0.6 s stereo sine, MP3, 5 KB (also the fixture of `tests/mp3dec_match.rs`).
const MP3_SAMPLE: &[u8] = include_bytes!("../tests/fixtures/sine440-stereo-44k1.mp3");

/// The browser's MP3 decoder: the plugin loads and decodes a real MP3 file through GStreamer. Images without the web engine have no such
/// plugin and say so; an image that ships the plugin but cannot run it is a `FAILED` line.
fn mp3_decoder() -> Verdict {
    mp3_decoder_with(Path::new(MP3_PLUGIN))
}

fn mp3_decoder_with(plugin: &Path) -> Verdict {
    if !plugin.exists() {
        return Ok("no web engine in this image, nothing to decode".into());
    }
    let dir = std::env::temp_dir().join(format!("iyakki-mp3check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make a work folder: {e}"))?;
    let file = dir.join("sine.mp3");
    let result = std::fs::write(&file, MP3_SAMPLE).map_err(|e| format!("cannot write the sample: {e}")).and_then(|_| {
        let launch = "gst-launch-1.0";
        if !crate::wsexec::on_path(launch) {
            return Err("the image ships the MP3 plugin but not gst-launch-1.0 to run it".to_string());
        }
        let out = std::process::Command::new(launch)
            .args(["-q", "filesrc", &format!("location={}", file.display()), "!", "mpegaudioparse", "!", "iyakkimp3dec", "!", "fakesink"])
            .env("GST_REGISTRY_1_0", dir.join("registry.bin"))
            .output()
            .map_err(|e| format!("cannot run gst-launch-1.0: {e}"))?;
        if out.status.success() {
            Ok("decoded a 44.1 kHz stereo MP3 through the plugin".to_string())
        } else {
            let text = String::from_utf8_lossy(&out.stderr).trim().to_string() + &String::from_utf8_lossy(&out.stdout);
            Err(format!("the MP3 plugin did not decode the sample: {}", text.lines().find(|l| l.contains("rror") || l.contains("no element")).unwrap_or("gst-launch-1.0 failed").trim()))
        }
    });
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// The elements the web engine's recorder and sound output need (`encodebin` and `uritranscodebin` from GStreamer, `iyakkiwebaudiosink` from
/// `dist/webview/wpe-helper/websink.c`). Without them every recorder page fails with "The MediaRecorder is unsupported on this platform".
fn web_recorder() -> Verdict {
    web_recorder_with(Path::new(MP3_PLUGIN), &["encodebin", "uritranscodebin", "iyakkiwebaudiosink"])
}

/// `marker` is a file only the web images carry (the MP3 plugin): without it there is no web engine and nothing to check.
fn web_recorder_with(marker: &Path, elements: &[&str]) -> Verdict {
    if !marker.exists() {
        return Ok("no web engine in this image, nothing to record with".into());
    }
    let inspect = "gst-inspect-1.0";
    if !crate::wsexec::on_path(inspect) {
        return Err("the image has the web engine but not gst-inspect-1.0 to check its recorder".to_string());
    }
    let dir = std::env::temp_dir().join(format!("iyakki-reccheck-{}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make a work folder: {e}"))?;
    let mut missing: Vec<&str> = Vec::new();
    for e in elements {
        let ok = std::process::Command::new(inspect).arg(e).env("GST_REGISTRY_1_0", dir.join("registry.bin")).output().map(|o| o.status.success()).unwrap_or(false);
        if !ok {
            missing.push(e);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    if missing.is_empty() {
        Ok(format!("{} found", elements.join(", ")))
    } else {
        Err(format!("the web engine's recorder is missing: {}", missing.join(", ")))
    }
}

/// Run every path; `(name, verdict)` in [`PATHS`] order. Starts the loopback listener itself.
pub fn run_all() -> Vec<(&'static str, Verdict)> {
    match Loopback::start() {
        Ok(lo) => PATHS.iter().map(|n| (*n, check(n, &lo))).collect(),
        Err(e) => PATHS.iter().map(|n| (*n, Err(e.clone()))).collect(),
    }
}

/// The self-test's lines: `IYAKKI-SELFTEST path <name> ok` (plus what it proved after a dash) or `... FAILED <why>`.
pub fn lines() -> Vec<String> {
    run_all()
        .into_iter()
        .map(|(n, v)| match v {
            Ok(m) => format!("IYAKKI-SELFTEST path {n} ok - {m}"),
            Err(e) => format!("IYAKKI-SELFTEST path {n} FAILED {}", e.lines().next().unwrap_or("").trim()),
        })
        .collect()
}

#[cfg(test)]
#[path = "pathcheck_tests.rs"]
mod tests;
