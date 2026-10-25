//! Measurements for `docs/REMOTE-PROTOCOL.md`: real shell screens (home, chat typing), a synthetic video region over the home
//! screen, and scrolling text drawn with the shell's own text engine, at 1280x720 and 1920x1080. Each is coded the old way (the
//! bounding rectangle of what changed, as one PNG) and the new way (tiles, at every level). Ignored because it is slow in a
//! debug build; the numbers in the document come from a release build:
//!
//! `cargo test --release -p iyakki-remote --test measure -- --ignored --nocapture`
//!
//! `IYAKKI_MEASURE_OUT=file` also writes the table there. Time is the encoder's CPU time on this machine.

use iyakki_agent::iyakki_proxy::{Upstream, UpstreamResponse};
use iyakki_agent::HostApi;
use iyakki_remote::codec::{Caps, Level, Opts, Streamer};
use iyakki_remote::screen::{changed_rect, encode_region};
use iyakki_sec::policy::Policy;
use iyakki_shell::accounts::{AuditLog, UserStore};
use iyakki_shell::gfx::{Face, Gfx};
use iyakki_shell::shell::{Shell, ShellEnv};
use iyakki_shell::ui::{Event, Key};
use std::sync::{Arc, Mutex};
use std::time::Instant;

struct Offline;
impl Upstream for Offline {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<UpstreamResponse, String> {
        Err("offline".into())
    }
}

struct Host;
impl HostApi for Host {
    fn detect_env(&self) -> String {
        "test".into()
    }
    fn net_status(&self) -> String {
        "none".into()
    }
    fn run_wasm(&self, _: &str, _: &Policy, _: &[u8], _: &str) -> Result<String, String> {
        Ok(String::new())
    }
    fn power_off(&self) {}
    fn reboot(&self) {}
}

fn env(tag: &str) -> ShellEnv {
    let d = std::env::temp_dir().join(format!("iyakki-remote-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    let mut store = UserStore::open(&d.join("users.json")).unwrap();
    store.create_first_admin("root", "rootpass12").unwrap();
    let up: Arc<dyn Upstream> = Arc::new(Offline);
    ShellEnv {
        store: Arc::new(Mutex::new(store)),
        audit: Arc::new(AuditLog::new(&d.join("audit.log"))),
        host: Arc::new(Host),
        cfg: iyakki_agent::config::AiConfig::parse("").unwrap(),
        chat: Arc::new(iyakki_agent::chat::Unconfigured),
        mcp_servers: vec![],
        services: iyakki_agent::services::Services::open(&d.join("svc"), up.clone(), up).unwrap(),
        web: Arc::new(iyakki_web::fetch::NoNetwork),
        hostname: "iyakki-test".into(),
        data_volatile: false,
        hwcheck: false,
    }
}

fn signed_in(tag: &str) -> Shell {
    let env = env(tag);
    let session = env.store.lock().unwrap().authenticate("root", "rootpass12").unwrap();
    let mut shell = Shell::new(env);
    shell.sign_in_checked(session, "rootpass12");
    shell
}

struct Scenario {
    name: &'static str,
    /// Frames per second the scenario runs at.
    fps: u64,
    frames: Vec<Vec<u8>>,
}

fn home(w: u32, h: u32) -> Scenario {
    let mut sh = signed_in("m-home");
    let mut g = Gfx::new(w, h);
    let mut frames = Vec::new();
    for _ in 0..12 {
        sh.handle(&Event::Tick);
        sh.render(&mut g);
        frames.push(g.rgba().to_vec());
    }
    Scenario { name: "home screen (sign-in, then idle)", fps: 2, frames }
}

fn chat(w: u32, h: u32) -> Scenario {
    let mut sh = signed_in("m-chat");
    let mut g = Gfx::new(w, h);
    sh.render(&mut g);
    assert!(sh.open_for_demo("chat"), "the chat app opens");
    sh.render(&mut g);
    let mut frames = vec![g.rgba().to_vec()];
    for c in "Hello, can you summarise the notes from yesterday's meeting? Thanks".chars().take(48) {
        sh.handle(&Event::Key(Key::Char(c)));
        sh.render(&mut g);
        frames.push(g.rgba().to_vec());
    }
    Scenario { name: "chat typing (8 keys/s)", fps: 8, frames }
}

fn video(w: u32, h: u32) -> Scenario {
    let mut sh = signed_in("m-video");
    let mut g = Gfx::new(w, h);
    sh.render(&mut g);
    let base = g.rgba().to_vec();
    let (vw, vh) = (640usize, 360usize);
    let (vx, vy) = ((w as usize - vw) / 2, (h as usize - vh) / 2);
    let mut seed = 12345u32;
    let mut frames = Vec::new();
    for t in 0..60usize {
        let mut f = base.clone();
        for y in 0..vh {
            for x in 0..vw {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = (seed >> 24) as i32 % 7 - 3; // camera grain
                let (dx, dy) = (x as f32 - (100.0 + t as f32 * 6.0), y as f32 - 180.0);
                let blob = (-(dx * dx + dy * dy) / 4000.0).exp();
                let c = |v: f32| (v as i32 + noise).clamp(0, 255) as u8;
                let o = ((vy + y) * w as usize + vx + x) * 4;
                f[o] = c(40.0 + x as f32 * 0.3 + blob * 180.0);
                f[o + 1] = c(60.0 + y as f32 * 0.4 + blob * 120.0);
                f[o + 2] = c(120.0 + (x + y + t * 3) as f32 * 0.1);
                f[o + 3] = 255;
            }
        }
        frames.push(f);
    }
    Scenario { name: "video-like (640x360 moving picture with grain, 30 fps)", fps: 30, frames }
}

fn scroll(w: u32, h: u32) -> Scenario {
    let mut g = Gfx::new(w, h);
    let mut frames = Vec::new();
    let lines: Vec<String> = (0..400).map(|i| format!("{i:03}  The quick brown fox jumps over the lazy dog; pack my box with five dozen liquor jugs — line {} of the listing", i * 7)).collect();
    for t in 0..60usize {
        g.clear([20, 24, 38, 255]);
        let off = t * 3;
        let mut y = 20.0 - (off % 20) as f32;
        let first = off / 20;
        let mut i = first;
        while y < h as f32 && i < lines.len() {
            g.text(24.0, y, 15.0, [225, 228, 240, 255], &lines[i], Face::Mono);
            y += 20.0;
            i += 1;
        }
        frames.push(g.rgba().to_vec());
    }
    Scenario { name: "scrolling text (3 px per frame, 30 fps)", fps: 30, frames }
}

struct Row {
    bytes: usize,
    sent_frames: usize,
    ms: f64,
}

fn old_way(s: &Scenario, w: usize, h: usize) -> Row {
    let (mut bytes, mut n, mut ms) = (0, 0, 0.0);
    let mut last: Vec<u8> = Vec::new();
    for f in &s.frames {
        let t = Instant::now();
        let rect = if last.is_empty() { Some((0, 0, w, h)) } else { changed_rect(&last, f, w, h) };
        if let Some(r) = rect {
            if let Some(fr) = encode_region(f, w, r) {
                bytes += 8 + fr.png.len();
                n += 1;
            }
        }
        ms += t.elapsed().as_secs_f64() * 1000.0;
        last = f.clone();
    }
    Row { bytes, sent_frames: n, ms }
}

fn new_way(s: &Scenario, w: usize, h: usize, level: Level) -> (Row, usize) {
    let mut st = Streamer::new();
    let (mut bytes, mut n, mut ms) = (0, 0, 0.0);
    let step = 1000 / s.fps;
    let mut now = 0u64;
    for f in &s.frames {
        st.set_source(w, h, f.clone());
        let t = Instant::now();
        while let Some(e) = st.encode(&Opts { level, caps: Caps { deflate: true }, budget: usize::MAX, now_ms: now }) {
            bytes += e.msg.len();
            n += 1;
        }
        ms += t.elapsed().as_secs_f64() * 1000.0;
        now += step;
    }
    // then the picture goes still: lossy tiles are sent again exactly (counted separately)
    let mut refine = 0;
    for _ in 0..20 {
        now += 100;
        if st.wants_encode(now) {
            if let Some(e) = st.encode(&Opts { level, caps: Caps { deflate: true }, budget: usize::MAX, now_ms: now }) {
                refine += e.msg.len();
            }
        }
    }
    (Row { bytes, sent_frames: n, ms }, refine)
}

#[test]
#[ignore]
fn measure_the_old_and_the_new_protocol_on_real_screens() {
    let mut out = String::new();
    out.push_str("| screen | size | scenario | frames | way | sent msgs | bytes/frame | bytes/s | ms/frame | after still |\n|---|---|---|---|---|---|---|---|---|---|\n");
    for (w, h) in [(1280u32, 720u32), (1920, 1080)] {
        let scenarios = [home(w, h), chat(w, h), video(w, h), scroll(w, h)];
        for s in &scenarios {
            let n = s.frames.len() as f64;
            let secs = n / s.fps as f64;
            let fmt = |label: &str, r: &Row, refine: Option<usize>| {
                format!(
                    "| {} | {}x{} | {} | {} | {} | {} | {:.0} | {:.0} | {:.1} | {} |\n",
                    "", w, h, s.name, s.frames.len(), label, r.sent_frames, r.bytes as f64 / n, r.bytes as f64 / secs, r.ms / n, refine.map_or("-".to_string(), |b| format!("+{b}"))
                )
            };
            out.push_str(&fmt("old: one PNG of the changed rectangle", &old_way(s, w as usize, h as usize), None));
            for level in Level::ALL {
                let (r, refine) = new_way(s, w as usize, h as usize, level);
                out.push_str(&fmt(&format!("new: {}", level.label()), &r, Some(refine)));
            }
        }
    }
    println!("{out}");
    if let Ok(p) = std::env::var("IYAKKI_MEASURE_OUT") {
        std::fs::write(p, &out).unwrap();
    }
}
