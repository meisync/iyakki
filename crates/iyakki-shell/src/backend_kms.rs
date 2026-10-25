//! The display loop for several displays: KMS dumb buffers, one per output (two, flipped), driven through [`DrmBackend`].
//!
//! [`Kms`] is everything except the devices: it plans, builds buffers, shows frames, notices hot-plug and runs the "Keep these
//! settings?" trial; with the fake card it is tested completely. [`maybe_run`] is the loop on a real machine; it is only started
//! when two or more displays are connected, the memory gate allows it, the shell does not run in a window and the card lets the
//! shell become its master. In every other case the framebuffer loop (`backend::run_fb`) goes on exactly as before and one line
//! says why. See `docs/MULTI-DISPLAY.md` section 4.

use crate::display::kms::{Buffer, Connector, DrmBackend, ModeInfo, Resources};
use crate::display::outputs::{self, hub, now_ms, Change, DispSpec, DisplayConfig, OutView, Plan, Request, Status, Trial};

/// One output with its two buffers.
struct Output {
    connector: u32,
    crtc: u32,
    mode: ModeInfo,
    bufs: [Buffer; 2],
    /// The buffer the CRTC shows now.
    front: usize,
    /// Whether a mode has been set on the CRTC (the first frame sets it; later ones flip).
    live: bool,
}

pub struct Kms<D: DrmBackend> {
    drm: D,
    res: Resources,
    conns: Vec<Connector>,
    cfg: DisplayConfig,
    plan: Plan,
    outs: Vec<Output>,
    max_displays: usize,
    use_flip: bool,
    trial: Option<Trial>,
    kept: Vec<DisplayConfig>,
    /// Bumped whenever the plan (the list of displays) changed.
    pub generation: u64,
    notes: Vec<String>,
}

/// Turn the shell's RGBA frame into XRGB8888 (bytes B, G, R, X) in a buffer with `pitch` bytes per row.
pub fn rgba_to_xrgb(src: &[u8], w: usize, h: usize, dst: &mut [u8], pitch: usize) {
    for y in 0..h {
        let (s, d) = (&src[y * w * 4..(y + 1) * w * 4], &mut dst[y * pitch..y * pitch + w * 4]);
        for (sp, dp) in s.chunks_exact(4).zip(d.chunks_exact_mut(4)) {
            dp[0] = sp[2];
            dp[1] = sp[1];
            dp[2] = sp[0];
            dp[3] = 0xFF;
        }
    }
}

impl<D: DrmBackend> Kms<D> {
    /// Look at the card, plan and show the first (black) frame on every planned output. Errors when nothing can be shown;
    /// the caller then keeps the framebuffer.
    pub fn open(mut drm: D, cfg: DisplayConfig, max_displays: usize) -> Result<Kms<D>, String> {
        let res = drm.resources()?;
        let conns = Self::read_conns(&mut drm, &res)?;
        let plan = outputs::plan(&res, &conns, &cfg, max_displays)?;
        let mut k = Kms { drm, res, conns, cfg, plan: Plan::default(), outs: Vec::new(), max_displays, use_flip: true, trial: None, kept: Vec::new(), generation: 0, notes: Vec::new() };
        k.build(plan)?;
        Ok(k)
    }

    fn read_conns(drm: &mut D, res: &Resources) -> Result<Vec<Connector>, String> {
        res.connectors.iter().map(|id| drm.connector(*id)).collect()
    }

    /// Make the buffers for `plan` and show black on every output. On failure nothing is left half done.
    fn build(&mut self, plan: Plan) -> Result<(), String> {
        self.teardown();
        let mut outs: Vec<Output> = Vec::new();
        for p in &plan.outputs {
            let (w, h) = (p.w() as u32, p.h() as u32);
            let made = (|| -> Result<[Buffer; 2], String> {
                let a = self.drm.create_buffer(w, h)?;
                let b = match self.drm.create_buffer(w, h) {
                    Ok(b) => b,
                    Err(e) => {
                        self.drm.destroy_buffer(a);
                        return Err(e);
                    }
                };
                Ok([a, b])
            })();
            let bufs = match made {
                Ok(b) => b,
                Err(e) => {
                    for o in outs.drain(..) {
                        for b in o.bufs {
                            self.drm.destroy_buffer(b);
                        }
                    }
                    return Err(format!("{}: {e}", p.name));
                }
            };
            for b in &bufs {
                if let Ok(px) = self.drm.pixels(b) {
                    px.fill(0);
                }
            }
            outs.push(Output { connector: p.connector, crtc: p.crtc, mode: p.mode, bufs, front: 0, live: false });
        }
        // the first picture: black, with the mode set; the shell's first frame follows at once
        for (o, p) in outs.iter_mut().zip(&plan.outputs) {
            if let Err(e) = self.drm.set_crtc(o.crtc, o.bufs[0].fb_id, &[o.connector], Some(&o.mode)) {
                for o2 in outs.drain(..) {
                    for b in o2.bufs {
                        self.drm.destroy_buffer(b);
                    }
                }
                return Err(format!("{}: {e}", p.name));
            }
            o.live = true;
        }
        self.notes = plan.notes.clone();
        self.plan = plan;
        self.outs = outs;
        self.generation += 1;
        Ok(())
    }

    fn teardown(&mut self) {
        for o in self.outs.drain(..) {
            let _ = self.drm.set_crtc(o.crtc, 0, &[], None);
            for b in o.bufs {
                self.drm.destroy_buffer(b);
            }
        }
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    pub fn config(&self) -> &DisplayConfig {
        &self.cfg
    }

    /// The displays the shell lays itself out for: the primary only when mirroring.
    pub fn specs(&self) -> Vec<DispSpec> {
        let all = self.plan.specs();
        if self.plan.mirror {
            all.into_iter().take(1).collect()
        } else {
            all
        }
    }

    /// Show a rendered frame (RGBA, `w` x `h`) on display `k`, with the pointer drawn on it when it is there. When mirroring,
    /// display 0 is on every output.
    pub fn present(&mut self, k: usize, rgba: &[u8], w: usize, h: usize, pointer: Option<(i32, i32, usize)>) -> Result<(), String> {
        self.present_shaped(k, rgba, w, h, pointer, crate::ui::Pointer::Default)
    }

    /// Like [`Kms::present`], with the pointer drawn as `shape` (a hand over a link, an I-beam over a field, ...).
    pub fn present_shaped(&mut self, k: usize, rgba: &[u8], w: usize, h: usize, pointer: Option<(i32, i32, usize)>, shape: crate::ui::Pointer) -> Result<(), String> {
        let targets: Vec<usize> = if self.plan.mirror { (0..self.outs.len()).collect() } else { vec![k] };
        for t in targets {
            let Some(o) = self.outs.get_mut(t) else { return Err("no such output".into()) };
            let back = 1 - o.front;
            let buf = o.bufs[back];
            if (buf.w as usize, buf.h as usize) != (w, h) {
                return Err(format!("the frame is {w}x{h}, the output {}x{}", buf.w, buf.h));
            }
            {
                let px = self.drm.pixels(&buf)?;
                rgba_to_xrgb(rgba, w, h, px, buf.pitch as usize);
                if let Some((x, y, scale)) = pointer {
                    if let Some(((rx, ry, rw, rh), patch)) = crate::backend::cursor_patch(rgba, w, h, shape, x, y, scale) {
                        for row in 0..rh {
                            let d = (ry + row) * buf.pitch as usize + rx * 4;
                            rgba_to_xrgb(&patch[row * rw * 4..(row + 1) * rw * 4], rw, 1, &mut px[d..d + rw * 4], rw * 4);
                        }
                    }
                }
            }
            let flipped = self.use_flip && self.drm.page_flip(o.crtc, buf.fb_id).is_ok();
            if !flipped {
                // the card cannot flip (or is still busy with the last flip): set the CRTC again, which always works
                self.drm.set_crtc(o.crtc, buf.fb_id, &[o.connector], Some(&o.mode))?;
            }
            o.front = back;
        }
        Ok(())
    }

    /// A flip is refused for good by this card (not just busy): use SETCRTC for every frame from now on.
    pub fn stop_flipping(&mut self) {
        self.use_flip = false;
    }

    /// Look at the connectors again; when something was plugged or unplugged (or a connector's modes changed) plan again.
    /// Returns what changed. On failure to show the new plan the old one is kept.
    pub fn rescan(&mut self) -> Result<Vec<Change>, String> {
        self.res = self.drm.resources()?;
        let now = Self::read_conns(&mut self.drm, &self.res)?;
        let changes = outputs::diff(&self.conns, &now);
        if changes.is_empty() {
            self.conns = now;
            return Ok(changes);
        }
        let old = std::mem::replace(&mut self.conns, now);
        match outputs::plan(&self.res, &self.conns, &self.cfg, self.max_displays) {
            Ok(plan) => {
                if let Err(e) = self.build(plan) {
                    self.conns = old;
                    let _ = outputs::plan(&self.res, &self.conns, &self.cfg, self.max_displays).map(|p| self.build(p));
                    return Err(e);
                }
                Ok(changes)
            }
            Err(e) => {
                self.conns = old;
                Err(e)
            }
        }
    }

    /// Try a new setting: it stays for [`outputs::KEEP_SECS`] seconds unless kept; one that cannot be shown is undone at once.
    pub fn apply(&mut self, cfg: DisplayConfig, now: u64) -> Result<(), String> {
        let plan = outputs::plan(&self.res, &self.conns, &cfg, self.max_displays)?;
        let previous = self.cfg.clone();
        let old_plan = self.plan.clone();
        match self.build(plan) {
            Ok(()) => {
                self.cfg = cfg;
                if self.trial.is_none() {
                    self.trial = Some(Trial::start(previous, now));
                } else if let Some(t) = self.trial.as_mut() {
                    t.until_ms = now + outputs::KEEP_SECS * 1000;
                }
                Ok(())
            }
            Err(e) => {
                // the old arrangement comes back at once
                let _ = self.build(old_plan);
                Err(e)
            }
        }
    }

    /// The person kept the new setting.
    pub fn keep(&mut self) {
        if self.trial.take().is_some() {
            self.kept.push(self.cfg.clone());
        }
    }

    /// Go back to what was before the trial. Returns whether there was one.
    pub fn revert(&mut self) -> Result<bool, String> {
        let Some(t) = self.trial.take() else { return Ok(false) };
        let plan = outputs::plan(&self.res, &self.conns, &t.previous, self.max_displays)?;
        self.cfg = t.previous;
        self.build(plan)?;
        Ok(true)
    }

    /// Called often: reverts when the time is up. Returns whether it did.
    pub fn tick_trial(&mut self, now: u64) -> Result<bool, String> {
        if self.trial.as_ref().is_some_and(|t| t.expired(now)) {
            return self.revert();
        }
        Ok(false)
    }

    pub fn trial_secs(&self, now: u64) -> Option<u64> {
        self.trial.as_ref().map(|t| t.secs_left(now))
    }

    /// Settings kept since the last call (the caller writes them to the person's file).
    pub fn take_kept(&mut self) -> Vec<DisplayConfig> {
        std::mem::take(&mut self.kept)
    }

    /// What Settings -> Displays shows.
    pub fn status(&self, now: u64) -> Status {
        let outputs = self
            .conns
            .iter()
            .map(|c| {
                let planned = self.plan.outputs.iter().find(|p| p.name == c.name);
                let mut modes: Vec<(u32, u32, u32)> = Vec::new();
                for m in &c.modes {
                    let e = (m.hdisplay as u32, m.vdisplay as u32, m.vrefresh);
                    if !modes.contains(&e) {
                        modes.push(e);
                    }
                }
                OutView { name: c.name.clone(), connected: c.connected, modes, current: planned.map(|p| (p.w() as u32, p.h() as u32)), pos: planned.map_or((0, 0), |p| (p.x, p.y)), primary: self.plan.outputs.first().is_some_and(|p| p.name == c.name) }
            })
            .collect();
        Status { kms: true, why_not: String::new(), outputs, config: self.cfg.clone(), trial_secs: self.trial_secs(now), notes: self.notes.clone() }
    }

    /// Carry out what Settings asked for. Returns whether the plan changed (the shell must be told the new displays).
    pub fn process_requests(&mut self, reqs: Vec<Request>, now: u64) -> bool {
        let g = self.generation;
        for r in reqs {
            match r {
                Request::Apply(cfg) => {
                    if let Err(e) = self.apply(cfg, now) {
                        self.notes = vec![format!("Could not use that arrangement ({e}); the old one is back")];
                    }
                }
                Request::Keep => self.keep(),
                Request::Revert => {
                    if let Err(e) = self.revert() {
                        self.notes = vec![format!("Could not go back: {e}")];
                    }
                }
                Request::Identify => hub().identify(now + 3000),
            }
        }
        self.generation != g
    }

    #[cfg(test)]
    pub fn drm(&mut self) -> &mut D {
        &mut self.drm
    }
}

impl<D: DrmBackend> Drop for Kms<D> {
    fn drop(&mut self) {
        self.teardown();
    }
}

// ------------------------------------------------------------------------------------------------ deciding

/// Whether the displays should be driven by KMS now, and if not why not (one line for the log and Settings).
/// `connected` is the number of connected connectors (sysfs), `windowed` whether a desktop owns the screen.
pub fn decide(connected: usize, windowed: bool, features: &crate::memgate::Allowed) -> Result<(), String> {
    if windowed {
        return Err("the shell runs in a window (a desktop owns the screen)".to_string());
    }
    if connected < 2 {
        return Err("one display is connected".into());
    }
    if features.switch == crate::memgate::Switch::Off {
        return Err("screen features are switched off".into());
    }
    if !features.multi_display {
        return Err(if features.reason.is_empty() { "this computer does not have the memory for several displays".to_string() } else { format!("several displays are not used: {}", features.reason) });
    }
    Ok(())
}

pub enum Outcome {
    /// The loop ran and ended (the shell stops, or the display is to be started again).
    Ran(Result<(), String>),
    /// Not used: the framebuffer loop goes on.
    NotUsed,
}

fn is_windowed() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var_os("DISPLAY").is_some()
}

/// The card to try: the first `/dev/dri/cardN` whose sysfs directory has two or more connected connectors.
fn pick_card(drm_sys: &std::path::Path) -> Option<String> {
    let conns = crate::display::read_connectors(drm_sys);
    let mut by_card: std::collections::BTreeMap<String, usize> = Default::default();
    if let Ok(rd) = std::fs::read_dir(drm_sys) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if let Some((card, _)) = n.split_once('-') {
                if card.starts_with("card") && card[4..].chars().all(|c| c.is_ascii_digit()) {
                    let name = n.split_once('-').map(|x| x.1.to_string()).unwrap_or_default();
                    if conns.iter().any(|c| c.name == name && c.connected) {
                        *by_card.entry(card.to_string()).or_default() += 1;
                    }
                }
            }
        }
    }
    by_card.into_iter().find(|(_, n)| *n >= 2).map(|(c, _)| format!("/dev/dri/{c}"))
}

/// Called at the start of the framebuffer loop: when this machine has two displays and may use them, run the display loop
/// here. Never on a development computer, in a window or in a test (see [`decide`]).
pub fn maybe_run(shell: &mut crate::shell::Shell, stop: &std::sync::atomic::AtomicBool) -> Outcome {
    use crate::memgate::DisplayMode;
    let sys = std::path::Path::new("/sys/class/drm");
    let connected = crate::display::read_connectors(sys).iter().filter(|c| c.connected).count();
    publish_fallback(sys, "one display is connected");
    if connected < 2 {
        return Outcome::NotUsed;
    }
    let modes: Vec<DisplayMode> = crate::display::read_connectors(sys).iter().filter(|c| c.connected).filter_map(|c| c.modes.first().map(|m| DisplayMode { w: m.0, h: m.1 })).collect();
    let features = shell.display_features(&modes);
    if let Err(why) = decide(connected, is_windowed(), &features) {
        log::info!("shell: several displays are not driven by the shell: {why}; using the framebuffer");
        publish_fallback(sys, &why);
        return Outcome::NotUsed;
    }
    let Some(card) = pick_card(sys) else {
        log::info!("shell: several displays are connected but no single graphics card has two of them; using the framebuffer");
        publish_fallback(sys, "no single graphics card has two connected displays");
        return Outcome::NotUsed;
    };
    let drm = match crate::display::kms::RealDrm::open(&card, is_windowed()) {
        Ok(d) => d,
        Err(e) => {
            log::info!("shell: not taking over the displays ({e}); using the framebuffer");
            publish_fallback(sys, &e);
            return Outcome::NotUsed;
        }
    };
    let kms = match Kms::open(drm, DisplayConfig::default(), features.max_displays) {
        Ok(k) => k,
        Err(e) => {
            log::info!("shell: could not set up the displays ({e}); using the framebuffer");
            publish_fallback(sys, &e);
            return Outcome::NotUsed;
        }
    };
    log::info!("shell: {} displays driven by the shell through {card}: {}", kms.plan().outputs.len(), kms.plan().outputs.iter().map(|o| format!("{} {}x{}+{}+{}", o.name, o.w(), o.h(), o.x, o.y)).collect::<Vec<_>>().join(", "));
    Outcome::Ran(run_loop(shell, kms, stop, features.max_displays))
}

/// Settings -> Displays when the shell does not drive the displays itself: the connected outputs from sysfs and the reason.
pub fn publish_fallback(sys: &std::path::Path, why: &str) {
    let conns = crate::display::read_connectors(sys);
    let mut st = Status { kms: false, why_not: why.to_string(), ..Default::default() };
    for c in conns.iter().filter(|c| c.connected) {
        st.outputs.push(OutView { name: c.name.clone(), connected: true, modes: c.modes.iter().map(|m| (m.0, m.1, 0)).collect(), current: c.modes.first().copied(), pos: (0, 0), primary: false });
    }
    hub().set_status(st);
}

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant};

use crate::gfx::Gfx;
use crate::input::{poll_devices, scan_devices, Device, InputState};
use crate::ui::Event;

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// The loop on a real machine. Not run in tests (it needs a real card and real input devices).
fn run_loop(shell: &mut crate::shell::Shell, mut kms: Kms<crate::display::kms::RealDrm>, stop: &AtomicBool, max_displays: usize) -> Result<(), String> {
    let _ = max_displays;
    // the text console stays off while the shell owns the screens
    std::mem::forget(crate::backend::GraphicsMode::enter());
    let plan_shared = Mutex::new(kms.plan().clone());
    let (tx, rx) = mpsc::channel::<(Option<usize>, Event)>();
    let pointer: Mutex<(i32, i32)> = Mutex::new((0, 0));
    let quit = AtomicBool::new(false);
    let mut user_loaded: Option<String> = None;
    std::thread::scope(|sc| {
        let (b0, b1) = kms.plan().bounds();
        let (plan_shared, pointer, quit) = (&plan_shared, &pointer, &quit);
        let tx = tx.clone();
        let input = std::thread::Builder::new().name("iyakki-input".into()).stack_size(8 * 1024 * 1024).spawn_scoped(sc, move || {
            let mut st = InputState::new(b0.max(1) as u32, b1.max(1) as u32);
            let mut devs: Vec<Device> = Vec::new();
            let mut last_scan = Instant::now().checked_sub(Duration::from_secs(10)).unwrap_or_else(Instant::now);
            while !stop.load(Ordering::Relaxed) && !quit.load(Ordering::Relaxed) {
                let plan = lock(&plan_shared).clone();
                let (bw, bh) = plan.bounds();
                st.w = bw.max(1);
                st.h = bh.max(1);
                for (raw, abs, pad) in poll_devices(&mut devs, 10) {
                    for ev in (match pad { Some(range) => st.feed_pad(raw, &range), None => st.feed(raw, abs) }) {
                        // the pointer lives on the arrangement: pull it back onto an output and tell the display it is on
                        let (cx, cy) = plan.clamp(st.x, st.y);
                        st.x = cx;
                        st.y = cy;
                        *lock(&pointer) = (cx, cy);
                        let (d, local) = match plan.locate(cx, cy) {
                            Some((d, lx, ly)) => (Some(d), (lx, ly)),
                            None => (None, (cx, cy)),
                        };
                        let ev = match ev {
                            Event::MouseMove(..) => Event::MouseMove(local.0, local.1),
                            Event::MouseDown(..) => Event::MouseDown(local.0, local.1),
                            Event::MouseUp(..) => Event::MouseUp(local.0, local.1),
                            Event::MouseRight(..) => Event::MouseRight(local.0, local.1),
                            other => other,
                        };
                        let _ = tx.send((d, ev));
                    }
                }
                // the smooth scroll steps due now go to the display the pointer is on
                for ev in st.scroll_tick() {
                    let d = plan.locate(st.x, st.y).map(|(d, _, _)| d);
                    let _ = tx.send((d, ev));
                }
                if last_scan.elapsed() > Duration::from_secs(2) {
                    scan_devices(&mut devs);
                    last_scan = Instant::now();
                }
            }
        });
        if input.is_err() {
            quit.store(true, Ordering::Relaxed);
        }
        let mut frames: Vec<Gfx> = Vec::new();
        let mut last_render = Instant::now();
        let mut last_tick = Instant::now();
        let mut last_req = Instant::now();
        let mut last_look = Instant::now();
        let mut seen_gen = u64::MAX;
        let mut first = true;
        let mut ptr_shown: Option<(usize, i32, i32)> = None;
        while !stop.load(Ordering::Relaxed) && !quit.load(Ordering::Relaxed) {
            let now = now_ms();
            // a new set of displays: tell the shell, make the drawing surfaces
            if seen_gen != kms.generation {
                seen_gen = kms.generation;
                let specs = kms.specs();
                *lock(&plan_shared) = kms.plan().clone();
                frames = specs.iter().map(|s| Gfx::new(s.w, s.h)).collect();
                let cfg = match shell.current_user() {
                    Some(u) => outputs::load(&shell.display_dir(), &u),
                    None => DisplayConfig::default(),
                };
                shell.set_displays(&specs, &cfg);
                first = true;
            }
            let mut dirty = false;
            let wait = Duration::from_millis(33).saturating_sub(last_tick.elapsed());
            let mut next = rx.recv_timeout(wait).ok();
            while let Some((d, ev)) = next {
                if let Some(d) = d {
                    shell.set_pointer_display(if kms.plan().mirror { 0 } else { d });
                }
                shell.handle(&ev);
                dirty |= shell.take_stale();
                next = rx.try_recv().ok();
            }
            if last_tick.elapsed() >= Duration::from_millis(33) {
                last_tick = Instant::now();
                shell.handle(&Event::Tick);
                dirty |= shell.take_stale();
            }
            // the person's own arrangement is applied once they sign in (and again for the next person)
            if shell.current_user() != user_loaded {
                user_loaded = shell.current_user();
                if let Some(u) = &user_loaded {
                    let cfg = outputs::load(&shell.display_dir(), u);
                    if cfg != *kms.config() {
                        let _ = kms.apply(cfg, now);
                        kms.keep();
                        let _ = kms.take_kept();
                    }
                }
            }
            if last_req.elapsed() >= Duration::from_millis(200) {
                last_req = Instant::now();
                let reqs = hub().take_requests();
                kms.process_requests(reqs, now);
                match kms.tick_trial(now) {
                    Ok(true) => log::info!("shell: the new display settings were not kept; the old ones are back"),
                    Ok(false) => {}
                    Err(e) => log::warn!("shell: going back to the old display settings failed: {e}"),
                }
                for cfg in kms.take_kept() {
                    if let Some(u) = shell.current_user() {
                        let _ = outputs::save(&shell.display_dir(), &u, &cfg);
                    }
                }
                let mut st = kms.status(now);
                st.notes.extend(Vec::<String>::new());
                hub().set_status(st);
                dirty = true;
            }
            if last_look.elapsed() >= Duration::from_secs(1) {
                last_look = Instant::now();
                match kms.rescan() {
                    Ok(ch) if !ch.is_empty() => log::info!("shell: displays changed: {ch:?}"),
                    Ok(_) => {}
                    Err(e) => log::warn!("shell: a display change could not be shown: {e}"),
                }
            }
            let (px, py) = *lock(&pointer);
            let loc = kms.plan().locate(px, py);
            let ptr_now = loc.map(|(d, lx, ly)| (d, lx, ly));
            let ptr_moved = ptr_now != ptr_shown;
            let every = Duration::from_millis(shell.redraw_interval_ms());
            if dirty || first || ptr_moved || last_render.elapsed() >= every {
                let must_draw = dirty || first || last_render.elapsed() >= every;
                first = false;
                if must_draw {
                    // the other displays first: the working display's frame leaves the shell's idea of its own size right
                    let active = shell.displays_active();
                    let order: Vec<usize> = (0..frames.len()).filter(|k| *k != active).chain(std::iter::once(active).filter(|k| *k < frames.len())).collect();
                    for k in order {
                        shell.render_display(k, &mut frames[k]);
                    }
                    last_render = Instant::now();
                }
                for k in 0..frames.len() {
                    let ptr = ptr_now.filter(|(d, _, _)| *d == k || kms.plan().mirror).map(|(_, lx, ly)| (lx, ly, crate::backend::cursor_scale(frames[k].width(), frames[k].height())));
                    let (w, h) = (frames[k].width() as usize, frames[k].height() as usize);
                    if let Err(e) = kms.present_shaped(k, frames[k].rgba(), w, h, ptr, shell.pointer()) {
                        log::warn!("shell: showing a frame failed ({e}); flipping off");
                        kms.stop_flipping();
                    }
                }
                ptr_shown = ptr_now;
            }
        }
        quit.store(true, Ordering::Relaxed);
    });
    Ok(())
}

#[cfg(test)]
#[path = "backend_kms_tests.rs"]
mod tests;
