//! The assistant's own browser tab: the web engine behind the `browser_*` tools (docs/BROWSER-AGENT.md).
//!
//! The tools run on worker threads and put their steps in `Services::browser.jobs`; the engine lives here, on the shell's side of
//! the fence, and is serviced on every tick ([`tick`]). It is one embedded-engine view with the person's own profile folder (their
//! signed-in sessions), started when the first job arrives, hidden while the BROWSER app is not showing it, and shut down after
//! ten idle minutes. The BROWSER app can show it at any time ([`render`], [`handle`]): while the assistant acts the tab says so,
//! with Stop and Take over buttons; the person's own mouse and keys always work.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use iyakki_agent::browserjobs::{Frame, PageEngine, PageState, Servicer};
use iyakki_agent::services::Services;
use iyakki_webview::proto::ToHelper;

use super::browser_full::{Full, Note, Start, AREA_FULL};
use crate::accounts::{AuditLog, Session};
use crate::gfx::{Face, Gfx};
use crate::ui::{Event, Key, R, DIM, ERR, MUTED, OK, TEXT};

/// The engine is closed after this long without a job.
pub const IDLE_CLOSE: Duration = Duration::from_secs(10 * 60);
/// The click region of the assistant's page area (the browser uses 901 and 902).
pub const AREA_AGENT: usize = AREA_FULL + 1;
/// Toolbar events of the assistant view, as `Event::Tab` numbers.
pub const TAB_OPEN: usize = 3000;
pub const TAB_STOP: usize = 3001;
pub const TAB_TAKE_OVER: usize = 3002;
pub const TAB_RESUME: usize = 3003;
pub const TAB_LEAVE: usize = 3004;

const ACCENT: [u8; 4] = [255, 150, 60, 255];

pub struct AgentTab {
    full: Option<Full>,
    servicer: Servicer,
    last_use: Instant,
    /// A helper program to use instead of the installed one (tests).
    helper_override: Option<PathBuf>,
    /// Why the engine could not be started, for the person.
    pub problem: Option<String>,
    /// The audit lines of navigations the engine refused, drained by the next tick.
    size: (u32, u32),
}

static TABS: OnceLock<Mutex<HashMap<String, Arc<Mutex<AgentTab>>>>> = OnceLock::new();

/// The assistant's tab of one person (made on first use, empty until a job arrives).
pub fn tab_of(user: &str) -> Arc<Mutex<AgentTab>> {
    let m = TABS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    g.entry(user.to_string()).or_insert_with(|| Arc::new(Mutex::new(AgentTab { full: None, servicer: Servicer::new(), last_use: Instant::now(), helper_override: None, problem: None, size: (1280, 800) }))).clone()
}

/// The person's tab exists (the engine is running).
pub fn alive(user: &str) -> bool {
    tab_of(user).lock().map(|t| t.full.is_some()).unwrap_or(false)
}

/// An adapter: the shell's view of the engine as a [`PageEngine`].
struct FullEngine<'a> {
    f: &'a mut Full,
    allow: &'a dyn Fn(&str) -> bool,
    refused: &'a mut Vec<String>,
}

impl PageEngine for FullEngine<'_> {
    fn send(&mut self, m: ToHelper) -> bool {
        self.f.send(m)
    }
    fn pump(&mut self) {
        self.f.pump(self.allow);
        for n in self.f.take_notes() {
            if let Note::Refused { host, .. } = n {
                self.refused.push(host);
            }
        }
    }
    fn state(&self) -> PageState {
        PageState { url: self.f.url.clone(), title: self.f.title.clone(), loading: self.f.loading, can_back: self.f.can_back, can_forward: self.f.can_forward, ready: self.f.is_ready(), gone: self.f.gone.clone() }
    }
    fn take_script(&mut self, id: u32) -> Option<(bool, String)> {
        self.f.take_script_result(id)
    }
    fn frame(&self) -> Option<Frame> {
        let pm = self.f.pixmap()?;
        Some(Frame { w: pm.width(), h: pm.height(), rgba: pm.data().to_vec() })
    }
}

impl AgentTab {
    fn start(&mut self, services: &Services, session: &Session) -> Result<(), String> {
        let helper = self.helper_override.clone().or_else(iyakki_agent::webengine::embedded_helper).ok_or_else(|| format!("{}.", iyakki_agent::webengine::NOT_INSTALLED))?;
        let profile = iyakki_agent::webengine::prepare_profile(services.dir(), &session.user)?;
        let full = Full::start(Start {
            helper,
            env: iyakki_agent::webengine::helper_env(&profile),
            profile,
            size: self.size,
            scale: 1.0,
            zoom: 1.0,
            allow_hosts: session.policy.net_hosts.clone(),
            user_agent: iyakki_agent::webengine::user_agent_suffix(),
            locale: String::new(),
        })?;
        self.full = Some(full);
        self.last_use = Instant::now();
        Ok(())
    }

    /// Look at the queue: start the engine when there is work, answer what the helper has answered, close it when it is idle.
    pub fn service(&mut self, services: &Services, audit: &AuditLog, session: &Session) {
        // whatever happens in here, every waiting tool gets an answer and the shell keeps running
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.service_inner(services, audit, session)));
        if r.is_err() {
            let why = "the assistant's browser hit an internal error and was closed; it starts again on the next call";
            self.servicer.abort(&services.browser.jobs, &session.user, why);
            self.full = None;
            self.problem = Some(why.to_string());
            audit.record(&session.user, "browser.agent.error", "internal error while servicing the browser");
        }
    }

    fn service_inner(&mut self, services: &Services, audit: &AuditLog, session: &Session) {
        let user = session.user.as_str();
        let jobs = &services.browser.jobs;
        jobs.beat(user);
        if self.full.is_none() {
            if jobs.pending_for(user) == 0 {
                return;
            }
            if let Err(e) = self.start(services, session) {
                self.problem = Some(e.clone());
                jobs.fail_all(user, &format!("The assistant's browser could not be started: {e}"));
                return;
            }
            self.problem = None;
            audit.record(user, "browser.agent.start", "");
        }
        let policy = session.policy.clone();
        let allow = move |h: &str| policy.allows_host(h);
        let mut refused = Vec::new();
        let (busy, close, gone) = {
            let Some(f) = self.full.as_mut() else { return };
            let mut eng = FullEngine { f, allow: &allow, refused: &mut refused };
            let r = self.servicer.service(jobs, user, &mut eng);
            (r.busy, r.close, eng.state().gone)
        };
        for h in refused {
            audit.record(user, "browser.agent.refused", &h);
        }
        if busy {
            self.last_use = Instant::now();
        }
        let idle = self.last_use.elapsed() > IDLE_CLOSE && jobs.pending_for(user) == 0;
        if close || idle || gone.is_some() {
            let why = if close { "the browser was closed".to_string() } else if let Some(g) = gone { format!("the browser engine stopped ({g}); it is restarted for the next call") } else { "the browser was closed after 10 idle minutes".to_string() };
            self.servicer.abort(jobs, user, &why);
            self.full = None;
            if !close && !idle {
                // the engine died: the tab says so, and the next call starts a fresh one
                self.problem = Some(why.clone());
            }
            audit.record(user, "browser.agent.end", &why);
        }
    }

    /// The shared picture file of the running view (tests check it is gone after a crash).
    pub fn shm_path(&self) -> Option<PathBuf> {
        self.full.as_ref().map(|f| f.shm_path())
    }

    /// Tests: use this helper program.
    pub fn use_helper(&mut self, p: PathBuf) {
        self.helper_override = Some(p);
    }

    pub fn url(&self) -> String {
        self.full.as_ref().map(|f| f.url.clone()).unwrap_or_default()
    }

    pub fn title(&self) -> String {
        self.full.as_ref().map(|f| f.title.clone()).unwrap_or_default()
    }

    pub fn frames(&self) -> u64 {
        self.full.as_ref().map_or(0, |f| f.frames)
    }

    /// Something new is on the picture.
    pub fn take_redraw(&mut self) -> bool {
        self.full.as_mut().is_some_and(|f| f.take_redraw())
    }
}

/// The person signed out: close their assistant tab and answer anything still waiting.
pub fn close(services: &Services, user: &str) {
    let tab = tab_of(user);
    let mut t = tab.lock().unwrap_or_else(|e| e.into_inner());
    t.servicer.abort(&services.browser.jobs, user, "the person signed out");
    t.full = None;
    t.problem = None;
}

/// Called on every shell tick: services the signed-in person's queue.
pub fn tick(services: &Services, audit: &Arc<AuditLog>, session: &Session) {
    if !services.browser.has_audit() {
        let a = audit.clone();
        services.browser.set_audit(Arc::new(move |u, e, d| a.record(u, e, d)));
    }
    let tab = tab_of(&session.user);
    let mut t = tab.lock().unwrap_or_else(|e| e.into_inner());
    t.service(services, audit, session);
}

// ------------------------------------------------------------------------------------------------ the view in the BROWSER app

/// The assistant's tab as a page of the BROWSER app: a banner with the controls over the live picture.
pub fn render(g: &mut Gfx, area: R, s: f32, services: &Services, user: &str) {
    render_with(g, area, s, services, user, Some("My tabs"));
}

/// [`render`] with the label of the button that leaves the view (`None`: no such button, the host has its own way back).
pub fn render_with(g: &mut Gfx, area: R, s: f32, services: &Services, user: &str, leave: Option<&str>) {
    let ctl = services.browser.control_view(user);
    let t = tab_of(user);
    let mut tab = t.lock().unwrap_or_else(|e| e.into_inner());
    let bh = 46.0 * s;
    let (col, headline) = if ctl.taken_over {
        (ERR, "You took over this tab. The assistant is paused.".to_string())
    } else if ctl.stopped {
        (ERR, "You stopped the assistant.".to_string())
    } else if ctl.active {
        (ACCENT, format!("Assistant is controlling this tab{}", if ctl.doing.is_empty() { String::new() } else { format!(" · {}", ctl.doing) }))
    } else if tab.full.is_some() {
        (MUTED, "The assistant's tab (idle). Anything you type or click here works as usual.".to_string())
    } else if let Some(p) = &tab.problem {
        (ERR, format!("The assistant's tab could not start: {p}"))
    } else {
        (MUTED, "The assistant's tab is not open. It opens when the assistant needs to browse.".to_string())
    };
    g.gradient_rect(area.x, area.y, area.w, bh, (0.0, area.y), (0.0, area.y + bh), &[(0.0, [24, 22, 36, 255]), (1.0, [14, 16, 30, 255])]);
    g.rect(area.x, area.y + bh - 3.0 * s, area.w, 3.0 * s, [col[0], col[1], col[2], if ctl.active { 255 } else { 120 }]);
    let mut bx = area.x + area.w - 10.0 * s;
    let mut buttons: Vec<(&str, [u8; 4], usize)> = leave.map(|l| (l, MUTED, TAB_LEAVE)).into_iter().collect();
    if ctl.taken_over || ctl.stopped {
        buttons.push(("Resume", OK, TAB_RESUME));
    } else {
        buttons.push(("Take over", ACCENT, TAB_TAKE_OVER));
        buttons.push(("Stop", ERR, TAB_STOP));
    }
    for (label, c, ev) in buttons {
        let w = g.text_width(14.0 * s, label, Face::Bold) + 14.0 * s * 1.7;
        bx -= w + 8.0 * s;
        g.button(bx, area.y + 7.0 * s, 14.0 * s, label, c, Event::Tab(ev));
    }
    let head = g.fit(15.0 * s, &headline, Face::Bold, (bx - area.x - 24.0 * s).max(40.0));
    g.text(area.x + 14.0 * s, area.y + 12.0 * s, 15.0 * s, if ctl.active { ACCENT } else { TEXT }, &head, Face::Bold);
    let (url, title) = (tab.url(), tab.title());
    let sub = g.fit(12.5 * s, &format!("{}{}", title, if url.is_empty() { String::new() } else { format!("  ·  {url}") }), Face::Sans, area.w - 28.0 * s);
    let view = R::new(area.x, area.y + bh, area.w.max(1.0), (area.h - bh - 24.0 * s).max(1.0));
    g.text(area.x + 14.0 * s, area.y + area.h - 20.0 * s, 12.5 * s, DIM, &sub, Face::Sans);
    tab.size = (view.w.max(64.0) as u32, view.h.max(64.0) as u32);
    match tab.full.as_mut() {
        Some(f) => {
            f.draw(g, view, s);
            g.hit(view, Event::AreaAt(AREA_AGENT, 0.0, 0.0), "assistant page");
        }
        None => g.rect(view.x, view.y, view.w, view.h, [10, 13, 26, 255]),
    }
}

/// What the shared engine shows now, for a host that draws its own toolbar: (address, title, loading, can go back, can go forward,
/// engine ready). `None` while no engine is running.
pub fn page_view(user: &str) -> Option<(String, String, bool, bool, bool, bool)> {
    let t = tab_of(user);
    let tab = t.lock().unwrap_or_else(|e| e.into_inner());
    tab.full.as_ref().map(|f| (f.url.clone(), f.title.clone(), f.loading, f.can_back, f.can_forward, f.is_ready()))
}

/// Can an engine be started for this person: the helper program is installed (or a test gave one).
pub fn engine_available(user: &str) -> bool {
    tab_of(user).lock().unwrap_or_else(|e| e.into_inner()).helper_override.is_some() || iyakki_agent::webengine::embedded_helper().is_some()
}

/// A new picture arrived from the engine since the last call.
pub fn take_redraw(user: &str) -> bool {
    tab_of(user).lock().unwrap_or_else(|e| e.into_inner()).take_redraw()
}

/// Why the engine is not running although it was asked to (it crashed or could not start), for the person.
pub fn problem(user: &str) -> Option<String> {
    tab_of(user).lock().unwrap_or_else(|e| e.into_inner()).problem.clone()
}

/// Tests: use this helper program for the person's engine.
#[cfg(test)]
pub fn use_helper_for(user: &str, p: PathBuf) {
    tab_of(user).lock().unwrap_or_else(|e| e.into_inner()).use_helper(p);
}

/// An event while the assistant's tab is showing. Returns false when the event is not for the page (the app handles it).
pub fn handle(ev: &Event, services: &Services, user: &str) -> bool {
    let t = tab_of(user);
    let mut tab = t.lock().unwrap_or_else(|e| e.into_inner());
    match ev {
        Event::Tab(TAB_STOP) => services.browser.stop(user),
        Event::Tab(TAB_TAKE_OVER) => services.browser.take_over(user),
        Event::Tab(TAB_RESUME) => services.browser.resume(user),
        Event::AreaAt(AREA_AGENT, x, y) => {
            if let Some(f) = tab.full.as_mut() {
                // the person's own input: it works at once, and it pauses the assistant
                if !f.page_focus {
                    f.set_focus(true);
                }
                if crate::ui::dragging() {
                    f.mouse_drag(*x as i32, *y as i32);
                } else {
                    f.mouse_down(*x as i32, *y as i32, crate::ui::click_count());
                }
            }
        }
        Event::MouseMove(x, y) => {
            if let Some(f) = tab.full.as_mut().filter(|f| !f.pressed) {
                f.mouse_move(*x, *y);
            }
        }
        Event::MouseUp(x, y) => {
            if let Some(f) = tab.full.as_mut() {
                f.mouse_up(*x, *y);
            }
        }
        Event::Scroll(d) => {
            if let Some(f) = tab.full.as_mut() {
                f.wheel(*d);
            }
        }
        Event::Paste(text) => {
            if let Some(f) = tab.full.as_mut() {
                f.text(text);
            }
        }
        Event::Key(Key::Esc) => return false,
        Event::Key(k) => {
            if let Some(f) = tab.full.as_mut() {
                f.key(k);
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::{cx_with_admin_store, temp_dir};
    use iyakki_agent::browserjobs::{Op, Reply};
    use std::time::Duration;

    fn fake_helper() -> PathBuf {
        let exe = std::env::current_exe().unwrap();
        let dir = exe.parent().unwrap().parent().unwrap().to_path_buf();
        let p = dir.join("iyakki-fake-webview");
        if !p.is_file() {
            let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
            let ok = std::process::Command::new(cargo).args(["build", "-q", "-p", "iyakki-webview", "--bin", "iyakki-fake-webview"]).status().map(|s| s.success()).unwrap_or(false);
            assert!(ok && p.is_file(), "could not build the fake helper");
        }
        p
    }

    /// Run the shell's tick on a thread until told to stop (the tools block, so something must service them).
    struct Ticker {
        stop: Arc<std::sync::atomic::AtomicBool>,
        h: Option<std::thread::JoinHandle<()>>,
    }
    impl Ticker {
        fn new(cx: &crate::apps::AppCx, helper: PathBuf) -> Ticker {
            tab_of(&cx.session.user).lock().unwrap().use_helper(helper);
            let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let (s2, svc, audit, session) = (stop.clone(), cx.services.clone(), cx.audit.clone(), cx.session.clone());
            let h = std::thread::spawn(move || {
                while !s2.load(std::sync::atomic::Ordering::SeqCst) {
                    tick(&svc, &audit, &session);
                    std::thread::sleep(Duration::from_millis(3));
                }
            });
            Ticker { stop, h: Some(h) }
        }
    }
    impl Drop for Ticker {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(h) = self.h.take() {
                let _ = h.join();
            }
        }
    }

    #[test]
    fn jobs_reach_the_embedded_engine_and_the_answers_come_back() {
        let mut cx = cx_with_admin_store("webagent1");
        cx.session.user = "wa1".into();
        let _t = Ticker::new(&cx, fake_helper());
        let jobs = &cx.services.browser.jobs;
        // no page yet; navigating makes the engine start, report the page and answer scripts (the fake helper's canned replies)
        jobs.call("wa1", Op::Navigate("https://a.test/start".into()), Duration::from_secs(20)).unwrap();
        let t = std::time::Instant::now();
        let state = loop {
            match jobs.call("wa1", Op::State, Duration::from_secs(10)).unwrap() {
                Reply::State(s) if s.url == "https://a.test/start" && s.ready => break s,
                _ => assert!(t.elapsed() < Duration::from_secs(20), "the page never arrived"),
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(state.title, "fake page https://a.test/start");
        let r = jobs.call("wa1", Op::Eval { js: "JSON:{\"ok\":true,\"n\":3}".into(), timeout_ms: 2000 }, Duration::from_secs(10)).unwrap();
        assert_eq!(r, Reply::Json("{\"ok\":true,\"n\":3}".into()));
        let e = jobs.call("wa1", Op::Eval { js: "THROW:boom".into(), timeout_ms: 2000 }, Duration::from_secs(10)).unwrap_err();
        assert!(e.contains("boom"), "{e}");
        // a picture, as a PNG
        let t = std::time::Instant::now();
        let png = loop {
            match jobs.call("wa1", Op::Screenshot, Duration::from_secs(10)) {
                Ok(r @ Reply::Frame { .. }) => break r.into_png().unwrap(),
                _ => assert!(t.elapsed() < Duration::from_secs(20), "no picture"),
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        assert_eq!(&png[1..4], b"PNG");
        assert!(alive("wa1"));
        // the audit trail knows the engine started
        let audit = std::fs::read_to_string(temp_dir_audit(&cx)).unwrap_or_default();
        assert!(audit.contains("browser.agent.start"), "{audit}");
        // close: the helper is stopped
        jobs.call("wa1", Op::Close, Duration::from_secs(10)).unwrap();
        let t = std::time::Instant::now();
        while alive("wa1") {
            assert!(t.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn temp_dir_audit(cx: &crate::apps::AppCx) -> PathBuf {
        // the audit log of the test context lives next to its users file
        let _ = cx;
        iyakki_agent::testdir::find("iyakki-shell", "webagent1").join("audit.log")
    }

    #[test]
    fn without_a_helper_the_tools_are_told_why_and_nothing_hangs() {
        let mut cx = cx_with_admin_store("webagent2");
        cx.session.user = "wa2".into();
        // a helper that does not exist
        let svc = cx.services.clone();
        tab_of("wa2").lock().unwrap().use_helper(PathBuf::from("/nonexistent/helper"));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (s2, audit, session) = (stop.clone(), cx.audit.clone(), cx.session.clone());
        let svc2 = svc.clone();
        let h = std::thread::spawn(move || {
            while !s2.load(std::sync::atomic::Ordering::SeqCst) {
                tick(&svc2, &audit, &session);
                std::thread::sleep(Duration::from_millis(3));
            }
        });
        let e = svc.browser.jobs.call("wa2", Op::State, Duration::from_secs(10)).unwrap_err();
        assert!(e.contains("could not be started"), "{e}");
        stop.store(true, std::sync::atomic::Ordering::SeqCst);
        h.join().unwrap();
        assert!(tab_of("wa2").lock().unwrap().problem.is_some());
        let _ = temp_dir("unused");
    }

    #[test]
    fn the_view_has_stop_and_take_over_and_the_controls_change_the_state() {
        let mut cx = cx_with_admin_store("webagent3");
        cx.session.user = "wa3".into();
        let svc = cx.services.clone();
        let mut g = Gfx::new(900, 600);
        svc.browser.note_doing("wa3", "click button \"Send\"", "chat.test");
        render(&mut g, R::new(0.0, 0.0, 900.0, 600.0), 1.0, &svc, "wa3");
        let labels: Vec<String> = g.hits.iter().map(|h| h.label.to_string()).collect();
        assert!(labels.iter().any(|l| l == "Stop") && labels.iter().any(|l| l == "Take over"), "{labels:?}");
        assert!(svc.browser.control_view("wa3").active);
        assert!(handle(&Event::Tab(TAB_TAKE_OVER), &svc, "wa3"));
        let v = svc.browser.control_view("wa3");
        assert!(v.taken_over && !v.active);
        let mut g = Gfx::new(900, 600);
        render(&mut g, R::new(0.0, 0.0, 900.0, 600.0), 1.0, &svc, "wa3");
        let labels: Vec<String> = g.hits.iter().map(|h| h.label.to_string()).collect();
        assert!(labels.iter().any(|l| l == "Resume") && !labels.iter().any(|l| l == "Stop"), "{labels:?}");
        assert!(handle(&Event::Tab(TAB_RESUME), &svc, "wa3"));
        assert!(!svc.browser.control_view("wa3").taken_over);
        assert!(handle(&Event::Tab(TAB_STOP), &svc, "wa3"));
        assert!(svc.browser.control_view("wa3").stopped);
        assert!(!handle(&Event::Key(Key::Esc), &svc, "wa3"), "Esc leaves the view");
    }

    fn ticker_for(user: &str) -> (crate::apps::AppCx, Ticker) {
        let mut cx = cx_with_admin_store(&format!("stress-{user}"));
        cx.session.user = user.into();
        let t = Ticker::new(&cx, fake_helper());
        (cx, t)
    }

    fn call(cx: &crate::apps::AppCx, op: Op, secs: u64) -> Result<Reply, String> {
        cx.services.browser.jobs.call(&cx.session.user, op, Duration::from_secs(secs))
    }

    /// Show `url`. A call that arrives while a dead engine has not been replaced yet is answered "stopped; restarted for the next
    /// call" (that is the contract), so the navigation is simply asked again until the page is there; the deadline is generous
    /// because a loaded machine starts a helper slowly.
    fn open(cx: &crate::apps::AppCx, url: &str) {
        let t = std::time::Instant::now();
        let mut navigated = false;
        loop {
            if !navigated {
                match call(cx, Op::Navigate(url.into()), 30) {
                    Ok(_) => navigated = true,
                    Err(e) if e.contains("browser engine stopped") => {}
                    Err(e) => panic!("{url}: {e}"),
                }
            } else {
                match call(cx, Op::State, 30) {
                    Ok(Reply::State(s)) if s.url == url => return,
                    Err(e) if e.contains("browser engine stopped") => navigated = false,
                    _ => {}
                }
            }
            assert!(t.elapsed() < Duration::from_secs(90), "{url} never arrived");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_helper_that_dies_during_a_script_answers_the_tool_leaves_no_file_and_the_next_call_restarts_it() {
        let (cx, _t) = ticker_for("st1");
        open(&cx, "https://a.test/one");
        let path = tab_of("st1").lock().unwrap().shm_path().expect("running");
        assert!(path.exists());
        let t = std::time::Instant::now();
        let e = call(&cx, Op::Eval { js: "EXIT".into(), timeout_ms: 5000 }, 30).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(10), "{:?}", t.elapsed());
        assert!(e.contains("browser engine stopped") && e.contains("restarted"), "{e}");
        let t = std::time::Instant::now();
        while path.exists() && t.elapsed() < Duration::from_secs(5) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!path.exists(), "the shared file of the dead helper is gone");
        assert!(tab_of("st1").lock().unwrap().problem.as_deref().is_some_and(|p| p.contains("browser engine stopped")), "the tab says what happened");
        // the next call starts a fresh engine and works
        open(&cx, "https://a.test/two");
        assert!(matches!(call(&cx, Op::Eval { js: "JSON:{\"ok\":true}".into(), timeout_ms: 2000 }, 10), Ok(Reply::Json(_))));
        assert!(tab_of("st1").lock().unwrap().problem.is_none());
    }

    #[test]
    fn a_helper_that_crashes_while_loading_or_drawing_and_one_that_hangs_never_hang_the_tool() {
        let (cx, _t) = ticker_for("st2");
        open(&cx, "https://a.test/one");
        // a script that never answers: the tool gets an error text within its timeout (+ margin)
        let t = std::time::Instant::now();
        let e = call(&cx, Op::Eval { js: "HANG".into(), timeout_ms: 1000 }, 30).unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(25), "{:?}", t.elapsed());
        assert!(e.contains("did not answer"), "{e}");
        // the engine still works after that
        assert!(matches!(call(&cx, Op::Eval { js: "JSON:1".into(), timeout_ms: 2000 }, 10), Ok(Reply::Json(_))));
        // dying in the middle of a picture
        call(&cx, Op::Navigate("fake://crash-mid-frame".into()), 10).unwrap();
        let t = std::time::Instant::now();
        let mut stopped = false;
        while t.elapsed() < Duration::from_secs(10) {
            match call(&cx, Op::State, 10) {
                Err(e) if e.contains("browser engine stopped") => {
                    stopped = true;
                    break;
                }
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        }
        assert!(stopped || tab_of("st2").lock().unwrap().problem.is_some(), "the crash was noticed");
        open(&cx, "https://a.test/after");
    }

    #[test]
    fn inconsistent_pictures_a_flood_and_twenty_quick_screenshots_never_panic_or_hang() {
        let (cx, _t) = ticker_for("st3");
        open(&cx, "https://a.test/start");
        call(&cx, Op::Navigate("fake://badframes".into()), 10).unwrap();
        call(&cx, Op::Navigate("fake://spam".into()), 10).unwrap();
        let t = std::time::Instant::now();
        let (mut ok, mut err) = (0, 0);
        for i in 0..20 {
            if i % 3 == 0 {
                call(&cx, Op::Navigate(format!("https://a.test/p{i}")), 10).unwrap();
            }
            match call(&cx, Op::Screenshot, 20) {
                Ok(r) => {
                    let png = r.into_png().expect("a consistent picture");
                    assert_eq!(&png[1..4], b"PNG");
                    ok += 1;
                }
                Err(e) => {
                    assert!(!e.is_empty());
                    err += 1;
                }
            }
        }
        assert!(t.elapsed() < Duration::from_secs(60), "{:?}", t.elapsed());
        assert_eq!(ok + err, 20);
        assert!(ok > 0, "pictures were served ({err} errors)");
        // noisy stderr does not block the helper
        call(&cx, Op::Navigate("fake://noisy-stderr".into()), 10).unwrap();
        open(&cx, "https://a.test/quiet");
    }

    #[test]
    fn signing_out_while_a_job_runs_answers_it_and_closes_the_tab() {
        let (cx, _t) = ticker_for("st4");
        open(&cx, "https://a.test/one");
        let jobs = cx.services.browser.jobs.clone();
        let h = std::thread::spawn(move || jobs.call("st4", Op::Eval { js: "HANG".into(), timeout_ms: 60_000 }, Duration::from_secs(120)));
        std::thread::sleep(Duration::from_millis(300));
        let t = std::time::Instant::now();
        close(&cx.services, "st4");
        let e = h.join().unwrap().unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(5) && e.contains("signed out"), "{e}");
        assert!(!alive("st4"));
        // answered late or twice: harmless
        cx.services.browser.jobs.complete(999_999, Reply::Ok);
        // stop / take over while idle: the next call reports it, nothing panics
        cx.services.browser.take_over("st4");
        assert!(cx.services.browser.check_control("st4").is_err());
    }

    #[test]
    fn screenshots_decode_for_every_size_and_damaged_pictures_never_panic_the_decoder() {
        use iyakki_agent::browserjobs::png::encode_rgba;
        for (w, h) in [(1u32, 1u32), (2, 3), (7, 1), (1, 9), (63, 63), (640, 360), (1100, 800), (3, 2000)] {
            let rgba: Vec<u8> = (0..w * h * 4).map(|i| (i * 7 % 251) as u8).collect();
            let png = encode_rgba(w, h, &rgba);
            let pm = crate::apps::browser_images::to_pixmap(&png).unwrap_or_else(|e| panic!("{w}x{h}: {e:?}"));
            assert_eq!((pm.width(), pm.height()), (w, h));
        }
        // empty or short pixel data still gives a valid file; a zero size gives an error from the reply, not a bad file
        assert!(Reply::Frame { w: 0, h: 0, rgba: vec![] }.into_png().is_err());
        assert!(Reply::Frame { w: 4, h: 4, rgba: vec![0; 10] }.into_png().is_err());
        let _ = encode_rgba(4, 4, &[1, 2, 3]);
        // damaged files: cut anywhere, one byte changed anywhere
        let rgba: Vec<u8> = (0..40 * 30 * 4).map(|i| (i % 253) as u8).collect();
        let good = encode_rgba(40, 30, &rgba);
        for cut in (0..good.len()).step_by(7) {
            let _ = crate::apps::browser_images::to_pixmap(&good[..cut]);
        }
        let mut x: u32 = 0x9e37_79b9;
        for _ in 0..400 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let mut bad = good.clone();
            let at = (x as usize) % bad.len();
            bad[at] = (x >> 8) as u8;
            let _ = crate::apps::browser_images::to_pixmap(&bad);
        }
    }
}
