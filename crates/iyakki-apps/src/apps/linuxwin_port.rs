//! What the LINUX WINDOW tile asks of the rest of the system (the app standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12).
//!
//! The tile (the `iyakki-tile-linuxwin` crate) names no `Services`, no session and no audit log: it asks a [`LinuxWinPort`] for who is signed in and what the
//! person's policy allows, whether the base-system install job is running, and to start that install. The sandbox manager, the spawner and the
//! process table stay the tile's `Rig` (tests replace it); the real rig comes from the port. The real port ([`CxPort`]) is the only place that
//! reads the session, the jobs and the quick-install service and writes the audit line.

use iyakki_rfb::Link;
use iyakki_sec::policy::CliLevel;
use iyakki_shared::linuxapps::LinuxApps;
use iyakki_shared::linuxwin as lw;
use iyakki_shared::quickinstall as qi;
use iyakki_tile_api::linuxwin as t;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::AppCx;

/// What the tile uses that tests replace: the sandbox manager of the person, the spawner and the process table.
pub struct Rig {
    pub la: Arc<dyn Fn() -> Result<LinuxApps, String> + Send + Sync>,
    pub spawn: Arc<dyn lw::Spawn>,
    pub proc_root: std::path::PathBuf,
}

/// How long a start waits for the display's socket.
const START_WAIT: Duration = Duration::from_secs(40);

pub trait LinuxWinPort {
    /// The signed-in person's name.
    fn user(&self) -> String;
    /// The person may install programs for the sandbox (administrators).
    fn can_install(&self) -> bool;
    /// The person has the full command-line permission (Linux windows run programs of the person's choosing).
    fn full_cli(&self) -> bool;
    /// The person's network policy is "any host".
    fn all_hosts(&self) -> bool;
    /// The "Install Linux windows" job of this person is running now.
    fn base_install_running(&self) -> bool;
    /// Start that install as a detached job (administrators); the audit line is written here.
    fn start_base_install(&self) -> Result<(), String>;
}

/// The real port, over the shell's context.
pub struct CxPort<'a>(pub &'a AppCx);

impl CxPort<'_> {
    /// The real rig: the sandbox manager of the signed-in person, the real spawner and this machine's process table.
    pub fn rig(&self) -> Rig {
        let data = self.0.data_dir().to_path_buf();
        let user = self.0.session.user.clone();
        Rig {
            la: Arc::new(move || {
                let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
                Ok(LinuxApps::new(&data, &user, &exe, crate::agent().exec().system()))
            }),
            spawn: Arc::new(lw::System),
            proc_root: self.0.core.host().proc_root(),
        }
    }
}

impl LinuxWinPort for CxPort<'_> {
    fn user(&self) -> String {
        self.0.session.user.clone()
    }
    fn can_install(&self) -> bool {
        self.0.session.policy.manage_settings
    }
    fn full_cli(&self) -> bool {
        self.0.session.policy.cli == CliLevel::Full
    }
    fn all_hosts(&self) -> bool {
        self.0.session.policy.net_hosts.iter().any(|h| h == "*")
    }
    fn base_install_running(&self) -> bool {
        self.0.jobs().visible(&self.0.session.user, qi::WORKSPACE).iter().any(|j| j.running() && qi::item_of(j) == Some(qi::Item::LinuxWindows))
    }
    fn start_base_install(&self) -> Result<(), String> {
        let l = qi::Launch::this_program(self.0.data_dir())?;
        self.0.b0().qi_start(&self.0.session.user, qi::Item::LinuxWindows, &|_| false, &l)?;
        self.0.audit.record(&self.0.session.user, "quickinstall.start", "linux-windows");
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------ the tile crate's port (D-S1-E4-01, pattern b)

fn program_to_tile(p: &lw::Program) -> t::Program {
    t::Program { id: p.id.clone(), name: p.name.clone(), exec: p.exec.clone() }
}

fn program_from_tile(p: &t::Program) -> lw::Program {
    lw::Program { id: p.id.clone(), name: p.name.clone(), exec: p.exec.clone() }
}

fn usage_to_tile(u: lw::Usage) -> t::Usage {
    t::Usage { procs: u.procs, rss_kb: u.rss_kb, ticks: u.ticks }
}

/// The remote screen of a window: the RFB link as the tile's screen. The picture is sent to the tile whole when it is new or has another size,
/// and by the changed part otherwise.
pub struct ShellScreen {
    link: Link,
    /// The size of the picture the tile was last given whole.
    sent: Mutex<(u32, u32)>,
}

impl ShellScreen {
    pub fn new(link: Link) -> ShellScreen {
        ShellScreen { link, sent: Mutex::new((0, 0)) }
    }
}

/// RGBA bytes (alpha 255) of a part of the framebuffer, cut to its edge: the width, the height and the bytes.
fn rgba_of(fb: &iyakki_rfb::Framebuffer, x: u32, y: u32, w: u32, h: u32) -> (u32, u32, Vec<u8>) {
    let x0 = x.min(fb.w);
    let y0 = y.min(fb.h);
    let x1 = x.saturating_add(w).min(fb.w);
    let y1 = y.saturating_add(h).min(fb.h);
    let (cw, ch) = (x1 - x0, y1 - y0);
    let mut out = Vec::with_capacity(cw as usize * ch as usize * 4);
    for yy in y0..y1 {
        for xx in x0..x1 {
            let p = fb.px.get((yy * fb.w + xx) as usize).copied().unwrap_or(0);
            out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8, 255]);
        }
    }
    (cw, ch, out)
}

impl t::Screen for ShellScreen {
    fn ready(&self) -> bool {
        self.link.with(|s| s.ready())
    }
    fn ended(&self) -> Option<String> {
        self.link.ended()
    }
    fn take_updates(&self) -> Vec<t::ScreenUpdate> {
        use iyakki_rfb::Event as E;
        let mut sent = self.sent.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut out = Vec::new();
        self.link.with(|s| {
            let events = s.take_events();
            let fb = s.fb();
            let full = |sent: &mut (u32, u32), out: &mut Vec<t::ScreenUpdate>| {
                let (w, h, rgba) = rgba_of(fb, 0, 0, fb.w, fb.h);
                *sent = (fb.w, fb.h);
                out.push(t::ScreenUpdate::Full { w, h, rgba });
            };
            for e in events {
                match e {
                    E::Ready { .. } | E::Resized { .. } => full(&mut sent, &mut out),
                    E::Updated(r) => {
                        if *sent != (fb.w, fb.h) {
                            full(&mut sent, &mut out);
                        } else {
                            let (w, h, rgba) = rgba_of(fb, r.x, r.y, r.w, r.h);
                            out.push(t::ScreenUpdate::Rect { x: r.x, y: r.y, w, h, rgba });
                        }
                    }
                    E::CutText(text) => out.push(t::ScreenUpdate::Clip(text)),
                    E::Bell | E::ResizeRefused(_) => {}
                }
            }
        });
        out
    }
    fn pointer(&self, x: i32, y: i32, buttons: u8) {
        self.link.with(|s| s.pointer_event(x, y, buttons));
    }
    fn key(&self, down: bool, keysym: u32) {
        self.link.with(|s| s.key_event(down, keysym));
    }
    fn cut_text(&self, text: &str) {
        self.link.with(|s| s.cut_text(text));
    }
    fn request_resize(&self, w: u32, h: u32) -> bool {
        self.link.with(|s| s.request_resize(w, h))
    }
    fn close(&mut self) {
        self.link.close();
    }
}

/// The display process of a window.
pub struct ShellWindow(pub lw::Window);

impl t::Window for ShellWindow {
    fn alive(&mut self) -> bool {
        self.0.alive()
    }
    fn pid(&self) -> Option<u32> {
        self.0.pid()
    }
    fn stop(&mut self) {
        self.0.stop()
    }
}

/// The real rig as the tile crate's rig: the sandbox manager of the person, the spawner and this machine's process table. The sandbox's safety
/// rules (the command, the folders, the namespaces, the network switch) are those of `iyakki_shared::linuxwin`, untouched.
pub struct ShellRig(pub Rig);

impl t::WinRig for ShellRig {
    fn sandbox(&self) -> Result<t::SandboxInfo, String> {
        let la = (self.0.la)()?;
        if !lw::ready(&la) {
            return Ok(t::SandboxInfo { ready: false, programs: vec![], network: false });
        }
        let programs = lw::programs(&la.rootfs_of(lw::SANDBOX)).iter().map(program_to_tile).collect();
        Ok(t::SandboxInfo { ready: true, programs, network: la.get(lw::SANDBOX).map(|s| s.network).unwrap_or(false) })
    }
    fn network(&self) -> Result<bool, String> {
        let la = (self.0.la)()?;
        la.get(lw::SANDBOX).map(|s| s.network)
    }
    fn start(&self, program: &t::Program, w: u32, h: u32, slot: u32, cancel: &AtomicBool) -> Result<t::Live, String> {
        let la = (self.0.la)()?;
        let mut win = lw::Window::start_slot(&la, &*self.0.spawn, &program_from_tile(program), w, h, slot)?;
        win.wait_socket_or(START_WAIT, &std::thread::sleep, &|| cancel.load(Ordering::SeqCst))?;
        let link = Link::connect_unix(&win.socket(), Duration::from_secs(10))?;
        let t0 = Instant::now();
        loop {
            if link.with(|s| s.ready()) {
                break;
            }
            if let Some(e) = link.ended() {
                return Err(format!("the display ended while connecting: {e}"));
            }
            if cancel.load(Ordering::SeqCst) || t0.elapsed() > Duration::from_secs(15) {
                return Err("the display did not answer".into());
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        Ok((Box::new(ShellWindow(win)), Box::new(ShellScreen::new(link))))
    }
    fn usage(&self, pid: u32) -> t::Usage {
        usage_to_tile(lw::usage_of(&self.0.proc_root, pid))
    }
    fn install_program(&self, package: &str) -> Result<(), String> {
        let la = (self.0.la)()?;
        lw::install_program(&la, package, None)
    }
}

/// A steady clock in milliseconds since the first call.
fn now_ms() -> u64 {
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// The shell's port as the tile crate's port, with the rig the tile uses.
pub struct Bridge<'a> {
    pub port: &'a dyn LinuxWinPort,
    pub rig: Arc<dyn t::WinRig>,
}

impl t::LinuxWinPort for Bridge<'_> {
    fn rig(&self) -> Arc<dyn t::WinRig> {
        self.rig.clone()
    }
    fn can_install(&self) -> bool {
        self.port.can_install()
    }
    fn full_cli(&self) -> bool {
        self.port.full_cli()
    }
    fn all_hosts(&self) -> bool {
        self.port.all_hosts()
    }
    fn base_install_running(&self) -> bool {
        self.port.base_install_running()
    }
    fn start_base_install(&self) -> Result<(), String> {
        self.port.start_base_install()
    }
    fn last_press_was_command(&self) -> bool {
        crate::inputprofile::last_press_was_command()
    }
    fn now_ms(&self) -> u64 {
        now_ms()
    }
}

#[cfg(test)]
mod mirror_tests {
    use super::*;

    #[test]
    fn the_constants_and_the_text_are_those_of_the_engine() {
        assert_eq!((t::MAX_SLOTS, t::MIN_SIZE, t::MAX_SIZE), (lw::MAX_SLOTS, lw::MIN_SIZE, lw::MAX_SIZE));
        assert_eq!(t::TEST_PROGRAMS, lw::TEST_PROGRAMS);
        assert_eq!(t::NOTE, lw::NOTE);
    }

    #[test]
    fn the_keysyms_are_those_of_the_rfb_crate() {
        use iyakki_rfb::keysyms as r;
        use t::keysyms as m;
        for (a, b) in [(m::BACKSPACE, r::BACKSPACE), (m::TAB, r::TAB), (m::RETURN, r::RETURN), (m::ESCAPE, r::ESCAPE), (m::HOME, r::HOME), (m::LEFT, r::LEFT), (m::UP, r::UP), (m::RIGHT, r::RIGHT), (m::DOWN, r::DOWN), (m::PAGE_UP, r::PAGE_UP), (m::PAGE_DOWN, r::PAGE_DOWN), (m::END, r::END), (m::INSERT, r::INSERT), (m::DELETE, r::DELETE), (m::SHIFT_L, r::SHIFT_L), (m::CONTROL_L, r::CONTROL_L), (m::ALT_L, r::ALT_L), (m::ISO_LEFT_TAB, r::ISO_LEFT_TAB), (m::F1, r::F1)] {
            assert_eq!(a, b);
        }
        for n in 0..=14u8 {
            assert_eq!(m::function_key(n), r::function_key(n));
        }
        for c in ['a', 'Z', ' ', '~', '\u{a0}', '\u{ff}', 'ä', '€', '\u{1F600}', '\u{7f}'] {
            assert_eq!(m::from_char(c), r::from_char(c), "{c:?}");
        }
    }

    #[test]
    fn programs_and_usage_round_trip_and_the_cpu_figure_is_the_engines() {
        let p = lw::Program { id: "xterm".into(), name: "XTerm".into(), exec: vec!["xterm".into(), "-e".into(), "sh".into()] };
        assert_eq!(program_from_tile(&program_to_tile(&p)), p);
        let (a, b) = (lw::Usage { procs: 1, rss_kb: 10, ticks: 100 }, lw::Usage { procs: 2, rss_kb: 20, ticks: 250 });
        let (ta, tb) = (usage_to_tile(a), usage_to_tile(b));
        assert_eq!((tb.procs, tb.rss_kb, tb.ticks), (2, 20, 250));
        for secs in [-1.0, 0.0, 0.5, 1.0, 3.0] {
            assert_eq!(t::cpu_percent(ta, tb, secs), lw::cpu_percent(a, b, secs));
            assert_eq!(t::cpu_percent(tb, ta, secs), lw::cpu_percent(b, a, secs), "a counter that went back gives 0");
        }
    }

    #[test]
    fn a_part_of_the_framebuffer_is_cut_to_its_edge_and_turned_into_rgba() {
        let fb = iyakki_rfb::Framebuffer { w: 3, h: 2, px: vec![0x00ff0000, 0x0000ff00, 0x000000ff, 0x00010203, 0, 0x00ffffff] };
        let (w, h, d) = rgba_of(&fb, 0, 0, 3, 2);
        assert_eq!((w, h, d.len()), (3, 2, 24));
        assert_eq!(&d[0..4], &[255, 0, 0, 255]);
        assert_eq!(&d[4..8], &[0, 255, 0, 255]);
        assert_eq!(&d[12..16], &[1, 2, 3, 255]);
        let (w, h, d) = rgba_of(&fb, 2, 1, 10, 10);
        assert_eq!((w, h), (1, 1));
        assert_eq!(d, vec![255, 255, 255, 255]);
        let (w, h, d) = rgba_of(&fb, 5, 5, 2, 2);
        assert_eq!((w, h, d.len()), (0, 0, 0), "a rectangle outside the picture is nothing");
    }

    #[test]
    fn the_clock_never_goes_back() {
        let a = now_ms();
        let b = now_ms();
        assert!(b >= a);
    }
}
