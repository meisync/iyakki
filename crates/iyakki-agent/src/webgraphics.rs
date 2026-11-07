//! Safe graphics mode for the browser engine: which GL the engine draws with, and the automatic fall back to software after a
//! freeze.
//!
//! Background: on a Lenovo 82WM (AMD Radeon 610M) the whole machine froze about 20 seconds after the engine printed
//! `graphics: hardware AMD Radeon 610M (radeonsi ...)`; a MacBook with Intel graphics ran the same steps fine. The cause is not
//! known, so the browser must not keep taking a machine down every time.
//!
//! * The setting (`<data>/web-profile/web-graphics.json`, next to the `web-tuning` file): **Automatic** (default), **Software**
//!   (safe: `LIBGL_ALWAYS_SOFTWARE=1` and the llvmpipe thread count) or **Hardware** (never fall back).
//! * Automatic fall back: when the last finished session (`sessions.tsv`, [`iyakki_storage::crashlog`]) did not end cleanly AND the
//!   engine log (`engine.log`) shows that the engine started on the graphics card in that session and nothing else was logged for
//!   more than [`BLAME_WINDOW_S`] seconds after it, the next engine start draws in software. The fall back is remembered per graphics
//!   card (`vendor:device`) in `web-graphics-state.json` until the person changes the setting or presses "Try hardware again", or
//!   until a later start on the card ends cleanly. After a clean session in software the person is OFFERED another try; nothing
//!   forces it.
//! * A clean shutdown, a power cut in a session where the engine never started, and an unclean session whose engine was started
//!   long before it ended never trigger the fall back.
//! * [`decide`] is pure; [`apply`] reads the real files, writes the memory, the engine log and the notices the shell shows.
//!
//! Both files are versioned (`iyakki_schema`): a damaged file is moved aside, a newer one is left as it is and cannot be changed.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

pub const SETTING_FILE: &str = "web-graphics.json";
pub const STATE_FILE: &str = "web-graphics-state.json";

/// The session counts as stopped "right after" the engine started when its last sign of life is at most this long after the start.
pub const BLAME_WINDOW_S: i64 = 600;

/// The sentence the person reads when the fall back happens.
pub const FREEZE_NOTICE: &str = "The last session stopped right after the browser started on this graphics card, so the browser now draws in software (slower). Settings, Browser, Graphics changes this.";
/// The sentence when the setting is Hardware and the same thing happened.
pub const FREEZE_NOTICE_HARDWARE: &str = "The last session stopped right after the browser started on this graphics card. Browser graphics is set to Hardware, so the browser still uses the card. Settings, Browser, Graphics changes this.";
/// The offer after a clean session in software.
pub const OFFER_NOTICE: &str = "The browser has been drawing in software since the last freeze. If you want, Settings, Browser, Graphics, Try hardware again lets it use the graphics card once more.";

pub const SETTING_SCHEMA: iyakki_schema::Spec = iyakki_schema::Spec { name: "browser graphics setting", current: 1, migrations: &[iyakki_schema::Migration { to: 1, what: "first layout", run: |_| {} }] };
pub const STATE_SCHEMA: iyakki_schema::Spec = iyakki_schema::Spec { name: "browser graphics memory", current: 1, migrations: &[iyakki_schema::Migration { to: 1, what: "first layout", run: |_| {} }] };

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Automatic,
    Software,
    Hardware,
}

impl Mode {
    pub fn id(self) -> &'static str {
        match self {
            Mode::Automatic => "automatic",
            Mode::Software => "software",
            Mode::Hardware => "hardware",
        }
    }
    pub fn from_id(s: &str) -> Option<Mode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "automatic" | "auto" => Some(Mode::Automatic),
            "software" | "software (safe)" | "safe" => Some(Mode::Software),
            "hardware" => Some(Mode::Hardware),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Mode::Automatic => "Automatic",
            Mode::Software => "Software (safe)",
            Mode::Hardware => "Hardware",
        }
    }
    /// The next choice in the order the Settings key cycles through.
    pub fn next(self) -> Mode {
        match self {
            Mode::Automatic => Mode::Software,
            Mode::Software => Mode::Hardware,
            Mode::Hardware => Mode::Automatic,
        }
    }
}

#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
struct SettingFile {
    #[serde(default)]
    schema: u32,
    #[serde(default)]
    mode: String,
    #[serde(flatten)]
    extra: iyakki_schema::Extra,
}

/// A graphics card the browser fell back from.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct Fallback {
    /// `vendor:device`, lower case hex (for example `1002:1681`).
    pub gpu: String,
    pub since: i64,
    pub reason: String,
}

/// What is remembered between sessions.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct State {
    #[serde(default)]
    pub schema: u32,
    #[serde(default)]
    pub fallbacks: Vec<Fallback>,
    /// The boot id of the finished session whose end was already judged (a session is judged once).
    #[serde(default)]
    pub judged_boot: String,
    /// The boot id of the session after which the offer to try hardware again was made.
    #[serde(default)]
    pub offered_boot: String,
    /// A software session ended cleanly after the fall back: the person may try hardware again.
    #[serde(default)]
    pub offer: bool,
    #[serde(flatten)]
    pub extra: iyakki_schema::Extra,
}

impl State {
    pub fn fallback_for(&self, gpu: &str) -> Option<&Fallback> {
        self.fallbacks.iter().find(|f| f.gpu == gpu)
    }
}

/// The previous finished session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prev {
    pub boot_id: String,
    pub started: i64,
    pub unclean: bool,
    /// Uptime of the last kernel log line it kept.
    pub uptime_s: u64,
}

impl From<iyakki_storage::crashlog::Finished> for Prev {
    fn from(f: iyakki_storage::crashlog::Finished) -> Prev {
        Prev { unclean: f.unclean(), boot_id: f.boot_id, started: f.started, uptime_s: f.uptime_s }
    }
}

pub struct Inputs<'a> {
    pub mode: Mode,
    /// `vendor:device` of the machine's graphics (empty: none, a VM without a GPU).
    pub gpu: &'a str,
    /// An Intel card whose driver is not in this image: software is the only way (see `webengine::force_software_gl`).
    pub no_driver: bool,
    pub prev: Option<&'a Prev>,
    /// When the running session started (the previous session ended before it).
    pub cur_started: i64,
    /// `engine.log` lines, oldest first: `(unix time, text)`.
    pub log: &'a [(i64, String)],
    pub state: &'a State,
    pub now: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    /// Automatic, nothing wrong: the card.
    Normal,
    /// No graphics card at all.
    NoGpu,
    /// The person chose Software.
    ChosenSoftware,
    /// The person chose Hardware.
    ChosenHardware,
    /// No driver for the Intel card in this image.
    NoDriver,
    /// The last session stopped right after the engine started on the card: software from now on.
    NewFreeze,
    /// An earlier freeze is remembered for this card.
    Remembered,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decision {
    pub software: bool,
    pub why: Why,
    pub state: State,
    /// A sentence for the person (a toast).
    pub notice: Option<String>,
    /// The line for the engine log (always).
    pub log_line: String,
    /// A line for the audit log, only when something was decided or learnt.
    pub audit: Option<String>,
}

/// What the previous session's engine log says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Evidence {
    /// The engine did not start on the card in that session (or there is nothing to judge by).
    None,
    /// It started on the card and the session ended cleanly.
    CleanEnd,
    /// It started on the card (at this time) and the session stopped right after, without a clean end.
    StoppedAfterStart(i64),
}

/// What the previous session shows. The last `graphics:` line the engine wrote during that session says what it started on; only
/// `graphics: hardware ...` counts. An unclean session counts only when its last sign of life (the last kernel log line, the last
/// engine log line) is within [`BLAME_WINDOW_S`] of that start.
pub fn evidence(prev: Option<&Prev>, cur_started: i64, log: &[(i64, String)]) -> Evidence {
    let Some(p) = prev else { return Evidence::None };
    let sane = iyakki_storage::crashlog::EARLIEST_SANE;
    if p.started < sane {
        return Evidence::None;
    }
    let hi = if cur_started > p.started { cur_started } else { i64::MAX };
    let inside: Vec<&(i64, String)> = log.iter().filter(|(t, _)| *t >= p.started && *t < hi && *t >= sane).collect();
    let Some((hw_ts, last)) = inside.iter().rev().find(|(_, l)| l.starts_with("graphics:")).map(|(t, l)| (*t, l.as_str())) else { return Evidence::None };
    if !last.starts_with("graphics: hardware") {
        return Evidence::None;
    }
    if !p.unclean {
        return Evidence::CleanEnd;
    }
    let last_sign = inside.iter().map(|(t, _)| *t).max().unwrap_or(hw_ts).max(p.started + p.uptime_s as i64);
    if last_sign - hw_ts <= BLAME_WINDOW_S {
        Evidence::StoppedAfterStart(hw_ts)
    } else {
        Evidence::None
    }
}

/// The decision for one engine start. Pure: the same inputs give the same answer, and feeding `state` back in gives the same
/// `software` (the memory makes a decision stable for the rest of the session).
pub fn decide(i: &Inputs) -> Decision {
    let mut st = i.state.clone();
    let ev = evidence(i.prev, i.cur_started, i.log);
    let prev_boot = i.prev.map(|p| p.boot_id.clone()).unwrap_or_default();
    let new_freeze = matches!(ev, Evidence::StoppedAfterStart(_)) && !prev_boot.is_empty() && prev_boot != st.judged_boot;
    // a later start on the card that ended cleanly: the old fall back is no longer needed
    let mut cleared = false;
    if ev == Evidence::CleanEnd && !i.gpu.is_empty() && st.fallback_for(i.gpu).is_some() {
        st.fallbacks.retain(|f| f.gpu != i.gpu);
        st.offer = false;
        cleared = true;
    }
    let (software, why, notice, audit);
    match i.mode {
        Mode::Software => {
            if new_freeze {
                st.judged_boot = prev_boot.clone();
            }
            (software, why, notice, audit) = (true, Why::ChosenSoftware, None, None);
        }
        Mode::Hardware => {
            let n = new_freeze.then(|| FREEZE_NOTICE_HARDWARE.to_string());
            if new_freeze {
                st.judged_boot = prev_boot.clone();
            }
            let a = new_freeze.then(|| format!("The last session stopped right after the browser started on graphics card {}; Browser graphics is Hardware, so it is used again", gpu_name(i.gpu)));
            (software, why, notice, audit) = (i.no_driver, Why::ChosenHardware, n, a);
        }
        Mode::Automatic => {
            if i.no_driver {
                (software, why, notice, audit) = (true, Why::NoDriver, None, None);
            } else if i.gpu.is_empty() {
                (software, why, notice, audit) = (false, Why::NoGpu, None, None);
            } else if new_freeze {
                let reason = format!("session {prev_boot} (started {}) stopped without a clean shutdown right after the engine started on the card", i.prev.map(|p| p.started).unwrap_or(0));
                st.fallbacks.retain(|f| f.gpu != i.gpu);
                st.fallbacks.push(Fallback { gpu: i.gpu.to_string(), since: i.now, reason: reason.clone() });
                let keep = st.fallbacks.len().saturating_sub(8);
                st.fallbacks.drain(..keep);
                st.judged_boot = prev_boot.clone();
                st.offer = false;
                (software, why, notice, audit) = (true, Why::NewFreeze, Some(FREEZE_NOTICE.to_string()), Some(format!("Browser graphics: software from now on for card {}, because {reason}", i.gpu)));
            } else if st.fallback_for(i.gpu).is_some() {
                let clean_prev = i.prev.is_some_and(|p| !p.unclean);
                let mut n = None;
                let mut a = None;
                if clean_prev && st.offered_boot != prev_boot {
                    st.offer = true;
                    st.offered_boot = prev_boot.clone();
                    n = Some(OFFER_NOTICE.to_string());
                    a = Some(format!("Browser graphics: the last session in software ended cleanly; trying hardware again on card {} is offered", i.gpu));
                }
                (software, why, notice, audit) = (true, Why::Remembered, n, a);
            } else {
                (software, why, notice, audit) = (false, Why::Normal, None, None);
            }
        }
    }
    let audit = audit.or_else(|| cleared.then(|| format!("Browser graphics: the card {} ended a session cleanly after the engine started on it; the software fall back is cleared", i.gpu)));
    let log_line = format!(
        "graphics mode: {} ({}; setting {}; card {}; previous session {})",
        if software { "software" } else { "hardware" },
        match why {
            Why::Normal => "automatic, no trouble remembered".to_string(),
            Why::NoGpu => "no graphics card found".to_string(),
            Why::ChosenSoftware => "chosen in Settings".to_string(),
            Why::ChosenHardware if i.no_driver => "chosen in Settings, but this image has no driver for the Intel card".to_string(),
            Why::ChosenHardware if new_freeze => "chosen in Settings; the last session stopped right after the engine started on the card, the choice is kept".to_string(),
            Why::ChosenHardware => "chosen in Settings".to_string(),
            Why::NoDriver => "no driver for the Intel card in this image".to_string(),
            Why::NewFreeze => "the last session stopped right after the engine started on the card".to_string(),
            Why::Remembered => st.fallback_for(i.gpu).map(|f| format!("fell back earlier: {}", f.reason)).unwrap_or_default(),
        },
        i.mode.id(),
        gpu_name(i.gpu),
        match (i.prev, ev) {
            (None, _) => "none",
            (Some(p), _) if p.unclean => "unclean",
            _ => "clean",
        }
    );
    Decision { software, why, state: st, notice, log_line, audit }
}

fn gpu_name(g: &str) -> &str {
    if g.is_empty() { "none" } else { g }
}

/// `1002:1681` from the files of the first card; several cards give `1002:1681+10de:2560` (sorted, no repeats).
pub fn gpu_id(gpus: &[(String, String)]) -> String {
    let hex = |s: &str| s.trim().trim_start_matches("0x").trim_start_matches("0X").to_ascii_lowercase();
    let mut v: Vec<String> = gpus.iter().filter(|(v, _)| !hex(v).is_empty()).map(|(v, d)| format!("{}:{}", hex(v), hex(d))).collect();
    v.sort();
    v.dedup();
    v.join("+")
}

/// The environment of the software path: Mesa's software renderer with half the logical CPUs (the number measured in
/// docs/BROWSER-SPEED.md), unless the `web-tuning` words already set `LP_NUM_THREADS`.
pub fn software_env(cores: usize, tuned: &[(String, String)]) -> Vec<(String, String)> {
    let mut v = vec![("LIBGL_ALWAYS_SOFTWARE".to_string(), "1".to_string())];
    if !tuned.iter().any(|(k, _)| k == "LP_NUM_THREADS") {
        v.push(("LP_NUM_THREADS".to_string(), crate::webengine::llvmpipe_threads(cores).to_string()));
    }
    v
}

// ------------------------------------------------------------------------------------------------ the files

/// The folder of the setting files (next to `web-tuning`).
pub fn root(data: &Path) -> PathBuf {
    data.join("web-profile")
}

/// The setting; the second value is why it cannot be changed (a newer release wrote it, or it could not be moved aside).
pub fn load_mode(data: &Path) -> (Mode, Option<String>) {
    let p = root(data).join(SETTING_FILE);
    let (out, f, _) = iyakki_schema::load_typed::<SettingFile>(&SETTING_SCHEMA, &p, iyakki_schema::now_secs());
    let ro = match &out {
        iyakki_schema::Outcome::Newer { found, .. } => Some(iyakki_schema::newer_message(&SETTING_SCHEMA, *found)),
        iyakki_schema::Outcome::Corrupt { moved_to: None, reason } => Some(format!("The browser graphics file cannot be read ({reason}); it is left as it is.")),
        _ => None,
    };
    (Mode::from_id(&f.mode).unwrap_or_default(), ro)
}

pub fn load_state(data: &Path) -> State {
    iyakki_schema::load_typed::<State>(&STATE_SCHEMA, &root(data).join(STATE_FILE), iyakki_schema::now_secs()).1
}

fn save_state(data: &Path, st: &State) {
    let mut s = st.clone();
    s.schema = STATE_SCHEMA.current;
    let _ = std::fs::create_dir_all(root(data));
    let _ = iyakki_schema::save_typed(&STATE_SCHEMA, &root(data).join(STATE_FILE), &s, 0o600);
}

/// Change the setting. Any change forgets the remembered fall backs: the person has decided, so the next start follows the setting.
pub fn set_mode(data: &Path, mode: Mode) -> Result<(), String> {
    let (_, ro) = load_mode(data);
    if let Some(why) = ro {
        return Err(why);
    }
    let _ = std::fs::create_dir_all(root(data));
    let f = SettingFile { schema: SETTING_SCHEMA.current, mode: mode.id().to_string(), extra: Default::default() };
    iyakki_schema::save_typed(&SETTING_SCHEMA, &root(data).join(SETTING_FILE), &f, 0o600)?;
    let mut st = load_state(data);
    st.fallbacks.clear();
    st.offer = false;
    save_state(data, &st);
    Ok(())
}

/// "Try hardware again": forget the fall back for this card (the setting stays Automatic). The judged session stays judged, so the
/// same old freeze does not trigger it again; a new freeze will.
pub fn try_hardware_again(data: &Path, gpu: &str) -> Result<(), String> {
    let mut st = load_state(data);
    if st.fallback_for(gpu).is_none() {
        return Err("no fall back to software is remembered for this graphics card".into());
    }
    st.fallbacks.retain(|f| f.gpu != gpu);
    st.offer = false;
    save_state(data, &st);
    Ok(())
}

// ------------------------------------------------------------------------------------------------ the running machine

/// What the real machine says (a fake in tests).
pub struct World {
    /// `(vendor, device)` file contents of the graphics cards.
    pub gpus: Vec<(String, String)>,
    pub marker: String,
    pub prev: Option<Prev>,
    pub cur_started: i64,
    pub log: Vec<(i64, String)>,
    pub log_dir: PathBuf,
    pub now: i64,
}

impl World {
    pub fn real() -> World {
        let dir = crate::enginelog::dir();
        let now = crate::clock::now_unix();
        let cur = iyakki_storage::crashlog::current_session();
        let prev = iyakki_storage::crashlog::last_finished(&dir).filter(|f| cur.as_ref().is_none_or(|c| c.boot_id != f.boot_id)).map(Prev::from);
        World {
            gpus: crate::webengine::drm_gpus(),
            marker: std::fs::read_to_string(crate::webengine::GL_MARKER).unwrap_or_default(),
            prev,
            cur_started: cur.map(|c| c.started).unwrap_or(now),
            log: crate::enginelog::read_in(&dir),
            log_dir: dir,
            now,
        }
    }
    fn vendors(&self) -> Vec<String> {
        self.gpus.iter().map(|g| g.0.clone()).collect()
    }
}

/// A sentence for the shell to show and record, queued by [`apply`].
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Pending {
    /// The full sentence for the browser's status line.
    pub status: Option<String>,
    /// A short sentence for the toast (the toast is one line).
    pub toast: Option<String>,
    pub audit: Option<String>,
}

static PENDING: Mutex<Vec<Pending>> = Mutex::new(Vec::new());

/// What [`apply`] queued since the last call (the shell shows the toast and writes the audit line).
pub fn take_pending() -> Vec<Pending> {
    std::mem::take(&mut *PENDING.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The one-line form of a notice, for the toast.
pub fn short_of(notice: &str) -> String {
    match notice {
        FREEZE_NOTICE => "The browser now draws in software after the last freeze. Settings, Browser, Graphics changes this.",
        FREEZE_NOTICE_HARDWARE => "The last session stopped after the browser started; Hardware is kept. See Settings, Browser, Graphics.",
        OFFER_NOTICE => "You can let the browser use the graphics card again: Settings, Browser, Graphics.",
        other => other,
    }
    .to_string()
}

/// One engine start: decide, remember, log. Returns whether the engine draws in software.
pub fn apply_with(data: &Path, w: &World) -> Decision {
    let (mode, _) = load_mode(data);
    let st = load_state(data);
    let gpu = gpu_id(&w.gpus);
    let no_driver = crate::webengine::force_software_gl(&w.vendors(), &w.marker);
    let d = decide(&Inputs { mode, gpu: &gpu, no_driver, prev: w.prev.as_ref(), cur_started: w.cur_started, log: &w.log, state: &st, now: w.now });
    if d.state != st {
        save_state(data, &d.state);
    }
    crate::enginelog::keep_in(&w.log_dir, w.now, &d.log_line);
    if d.notice.is_some() || d.audit.is_some() {
        if let Some(a) = &d.audit {
            crate::enginelog::keep_in(&w.log_dir, w.now, a);
        }
        let mut q = PENDING.lock().unwrap_or_else(|e| e.into_inner());
        if q.len() < 8 {
            q.push(Pending { status: d.notice.clone(), toast: d.notice.as_deref().map(short_of), audit: d.audit.clone() });
        }
    }
    d
}

/// [`apply_with`] on the real machine.
pub fn apply(data: &Path) -> Decision {
    apply_with(data, &World::real())
}

/// What Settings and the chat tool show (reads only; nothing is judged or written).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Status {
    pub mode: Mode,
    pub software: bool,
    pub gpu: String,
    /// `Automatic: hardware`, `Automatic: software because ...`, `Software (safe)`, `Hardware`.
    pub line: String,
    pub detail: String,
    pub fallback: bool,
    pub offer: bool,
    pub read_only: Option<String>,
}

pub fn status_with(data: &Path, gpus: &[(String, String)], marker: &str) -> Status {
    let (mode, read_only) = load_mode(data);
    let st = load_state(data);
    let gpu = gpu_id(gpus);
    let vendors: Vec<String> = gpus.iter().map(|g| g.0.clone()).collect();
    let no_driver = crate::webengine::force_software_gl(&vendors, marker);
    let fb = !gpu.is_empty() && st.fallback_for(&gpu).is_some();
    let (software, line, detail) = match mode {
        Mode::Software => (true, "Software (safe)".to_string(), "The browser always draws with the processor. Slower, but it does not use the graphics card.".to_string()),
        Mode::Hardware if no_driver => (true, "Hardware, but software here".to_string(), "This image has no driver for this Intel graphics card, so the browser draws in software whatever is chosen.".to_string()),
        Mode::Hardware => (false, "Hardware (never falls back)".to_string(), "The browser always uses the graphics card, even after a freeze. If the computer freezes when the browser starts, choose Automatic or Software.".to_string()),
        Mode::Automatic if no_driver => (true, "Automatic: software because this image has no driver for this Intel graphics card".to_string(), "Nothing to do: the card cannot be used by this image.".to_string()),
        Mode::Automatic if fb => (
            true,
            "Automatic: software because the last session stopped right after the browser started on this graphics card".to_string(),
            if st.offer { "The last session in software ended cleanly. Try hardware again lets the browser use the card once more.".to_string() } else { "Slower, but safe. Choosing a setting, or Try hardware again, makes the browser use the card once more.".to_string() },
        ),
        Mode::Automatic => (false, "Automatic: hardware".to_string(), "The browser uses the graphics card. If the computer stops right after the browser starts, the next start draws in software by itself and tells you.".to_string()),
    };
    Status { mode, software, gpu, line, detail, fallback: fb && mode == Mode::Automatic, offer: st.offer && fb, read_only }
}

pub fn status(data: &Path) -> Status {
    status_with(data, &crate::webengine::drm_gpus(), &std::fs::read_to_string(crate::webengine::GL_MARKER).unwrap_or_default())
}
