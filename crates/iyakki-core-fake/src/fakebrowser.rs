//! `FakeBrowser`: the core's browser sessions (`iyakki_core_api::BrowserSessions`) in memory, for the tests of an SDK app that embeds the
//! browser. Pages are scripted (`add_page`), known-bad hosts and a switch are scripted, every call is recorded, and a frame is a real
//! display list, so the SDK's view draws and hit-tests exactly what the real core would hand it. Nothing is fetched.

use std::collections::BTreeMap;
use std::sync::Mutex;

use iyakki_core_api::browsersession::{check_address, BrowserError, BrowserResult, BrowserSessions, FindState, Frame, Input, Navigate, OpenSpec, PageKey, PageState, Prompt, PromptKind, Refusal, SessionId, MAX_HTML, MAX_SESSIONS_PER_APP, MAX_TEXT, MAX_URL};
use iyakki_core_api::browsersession::EnginePref;
use iyakki_ui_sdk::canvas::{Canvas, Hit, Role, TextStyle};
use iyakki_ui_sdk::geom::Rect;
use iyakki_ui_sdk::list::ListCanvas;

/// A scripted page: a title, lines of text, links (label and target address) and, optionally, a question it raises when it loads.
#[derive(Clone, Debug, Default)]
pub struct FakePage {
    pub title: String,
    pub lines: Vec<String>,
    pub links: Vec<(String, String)>,
    pub asks: Option<(PromptKind, String)>,
}

impl FakePage {
    pub fn new(title: &str, lines: &[&str]) -> FakePage {
        FakePage { title: title.into(), lines: lines.iter().map(|s| s.to_string()).collect(), ..FakePage::default() }
    }
    pub fn link(mut self, label: &str, target: &str) -> FakePage {
        self.links.push((label.into(), target.into()));
        self
    }
    pub fn asking(mut self, kind: PromptKind, text: &str) -> FakePage {
        self.asks = Some((kind, text.into()));
        self
    }
}

struct Sess {
    user: String,
    app: String,
    spec: OpenSpec,
    history: Vec<String>,
    pos: usize,
    page: Option<FakePage>,
    error: Option<String>,
    find: FindState,
    scroll: i32,
    generation: u64,
    prompts: Vec<Prompt>,
    next_prompt: u64,
}

#[derive(Default)]
struct State {
    pages: BTreeMap<String, FakePage>,
    bad: BTreeMap<String, String>,
    denied: Vec<String>,
    off: bool,
    full_installed: bool,
    sessions: BTreeMap<u64, Sess>,
    next: u64,
    log: Vec<String>,
    answers: Vec<(String, bool)>,
    specs: Vec<OpenSpec>,
}

#[derive(Default)]
pub struct FakeBrowser {
    st: Mutex<State>,
}

fn host_of(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split(['/', '?', '#', ':']).next().unwrap_or("").to_ascii_lowercase()
}

impl FakeBrowser {
    pub fn new() -> FakeBrowser {
        FakeBrowser::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.st.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// A page the fake serves at this address (written as the app would write it, `https://a.org/`).
    pub fn add_page(&self, url: &str, page: FakePage) {
        self.lock().pages.insert(url.to_string(), page);
    }
    /// A host on the person's known-bad list (every address under it is refused).
    pub fn add_bad_host(&self, host: &str, reason: &str) {
        self.lock().bad.insert(host.to_ascii_lowercase(), reason.to_string());
    }
    /// A host the person's account may not reach.
    pub fn deny_host(&self, host: &str) {
        self.lock().denied.push(host.to_ascii_lowercase());
    }
    pub fn set_off(&self, off: bool) {
        self.lock().off = off;
    }
    pub fn set_full_engine_installed(&self, yes: bool) {
        self.lock().full_installed = yes;
    }
    /// Every call the fake saw, one line each (`open alice sample`, `navigate https://a.org/`, ...).
    pub fn calls(&self) -> Vec<String> {
        self.lock().log.clone()
    }
    /// The answers given to prompts: the question's text and whether it was allowed.
    pub fn answers(&self) -> Vec<(String, bool)> {
        self.lock().answers.clone()
    }
    /// What every `open` asked for (size, engine, palette, page defaults).
    pub fn opened_specs(&self) -> Vec<OpenSpec> {
        self.lock().specs.clone()
    }
    pub fn open_sessions(&self) -> usize {
        self.lock().sessions.len()
    }
}

fn judge(st: &State, raw: &str) -> Result<String, BrowserError> {
    if st.off {
        return Err(BrowserError::Refused(Refusal::Off));
    }
    let url = check_address(raw).map_err(BrowserError::Refused)?;
    let host = host_of(&url);
    if let Some((h, why)) = st.bad.iter().find(|(h, _)| host == **h || host.ends_with(&format!(".{h}"))) {
        return Err(BrowserError::Refused(Refusal::KnownBad { host: h.clone(), reason: why.clone() }));
    }
    if st.denied.iter().any(|d| host == *d) {
        return Err(BrowserError::Refused(Refusal::Policy { host }));
    }
    let mut url = if url.contains("://") { url } else { format!("https://{url}") };
    if url.split_once("://").is_some_and(|(_, r)| !r.contains('/')) {
        url.push('/');
    }
    Ok(url)
}

fn load(s: &mut Sess, st_pages: &BTreeMap<String, FakePage>, url: &str) {
    s.page = st_pages.get(url).cloned();
    s.error = if s.page.is_none() { Some(format!("{url} was not found")) } else { None };
    s.scroll = 0;
    s.find = FindState::default();
    if let Some((kind, text)) = s.page.as_ref().and_then(|p| p.asks.clone()) {
        s.next_prompt += 1;
        s.prompts.push(Prompt { id: s.next_prompt, kind, origin: host_of(url), text });
    }
    s.generation += 1;
}

fn lines_of(p: &FakePage) -> Vec<String> {
    let mut v = vec![p.title.clone()];
    v.extend(p.lines.iter().cloned());
    v
}

fn count_matches(p: &FakePage, q: &str) -> usize {
    if q.is_empty() {
        return 0;
    }
    let q = q.to_lowercase();
    lines_of(p).iter().chain(p.links.iter().map(|(l, _)| l).collect::<Vec<_>>().into_iter()).map(|l| l.to_lowercase().matches(&q).count()).sum()
}

fn draw(s: &Sess) -> String {
    let pal = s.spec.palette;
    let (w, h) = (s.spec.width, s.spec.height);
    let mut c = ListCanvas::new(w, h);
    c.rect(Rect::new(0.0, 0.0, w, h), pal.bg);
    let size = 15.0 * s.spec.text_scale.clamp(0.5, 3.0);
    let mut y = 8.0 - s.scroll as f32;
    match (&s.page, &s.error) {
        (Some(p), _) => {
            c.text(8.0, y, size * 1.3, pal.text, &p.title, TextStyle::Bold);
            y += size * 2.0;
            for l in &p.lines {
                c.text(8.0, y, size, pal.text, l, TextStyle::Regular);
                y += size * 1.5;
            }
            for (i, (label, _)) in p.links.iter().enumerate() {
                let adv = c.text(8.0, y, size, pal.link, label, TextStyle::Regular);
                c.hit(Rect::new(8.0, y, adv.max(10.0), size * 1.3), Hit { action: i as u32 + 1, name: label.clone(), role: Role::Link, ..Default::default() });
                y += size * 1.5;
            }
        }
        (None, Some(e)) => {
            c.text(8.0, y, size, pal.muted, e, TextStyle::Regular);
        }
        (None, None) => {
            c.text(8.0, y, size, pal.muted, "Nothing is open.", TextStyle::Regular);
        }
    }
    c.finish().to_text()
}

impl BrowserSessions for FakeBrowser {
    fn enabled(&self, _user: &str) -> bool {
        !self.lock().off
    }
    fn set_enabled(&self, user: &str, on: bool) -> BrowserResult<()> {
        let mut st = self.lock();
        st.off = !on;
        st.log.push(format!("set_enabled {user} {on}"));
        if !on {
            st.sessions.retain(|_, s| s.user != user);
        }
        Ok(())
    }
    fn open_views(&self, user: &str) -> usize {
        self.lock().sessions.values().filter(|s| s.user == user).count()
    }

    fn open(&self, user: &str, app: &str, spec: &OpenSpec) -> BrowserResult<SessionId> {
        let mut st = self.lock();
        st.log.push(format!("open {user} {app}"));
        if st.off {
            return Err(BrowserError::Refused(Refusal::Off));
        }
        if spec.engine == EnginePref::Full && !st.full_installed {
            return Err(BrowserError::Unavailable("the full engine is not installed".into()));
        }
        if st.sessions.values().filter(|s| s.user == user && s.app == app).count() >= MAX_SESSIONS_PER_APP {
            return Err(BrowserError::TooMany);
        }
        if !(spec.width.is_finite() && spec.height.is_finite()) || spec.width < 1.0 || spec.height < 1.0 || spec.width > 8192.0 || spec.height > 8192.0 {
            return Err(BrowserError::Invalid("the view has no usable size".into()));
        }
        st.specs.push(spec.clone());
        st.next += 1;
        let id = st.next;
        let mut spec = spec.clone();
        spec.engine = if spec.engine == EnginePref::Auto { if st.full_installed { EnginePref::Full } else { EnginePref::InHouse } } else { spec.engine };
        st.sessions.insert(id, Sess { user: user.into(), app: app.into(), spec, history: Vec::new(), pos: 0, page: None, error: None, find: FindState::default(), scroll: 0, generation: 1, prompts: Vec::new(), next_prompt: 0 });
        Ok(SessionId(id))
    }

    fn close(&self, user: &str, app: &str, id: SessionId) {
        let mut st = self.lock();
        st.log.push(format!("close {user} {app} {}", id.0));
        if st.sessions.get(&id.0).is_some_and(|s| s.user == user && s.app == app) {
            st.sessions.remove(&id.0);
        }
    }

    fn navigate(&self, user: &str, app: &str, id: SessionId, nav: &Navigate) -> BrowserResult<()> {
        let mut st = self.lock();
        let verdict = match nav {
            Navigate::Url(u) => {
                st.log.push(format!("navigate {u}"));
                Some(judge(&st, u))
            }
            Navigate::Html { base, html } => {
                st.log.push(format!("html {base}"));
                if html.len() > MAX_HTML {
                    return Err(BrowserError::Invalid("the page is too big".into()));
                }
                None
            }
            other => {
                st.log.push(format!("{other:?}").to_lowercase());
                None
            }
        };
        let pages = st.pages.clone();
        let s = st.sessions.get_mut(&id.0).filter(|s| s.user == user && s.app == app).ok_or(BrowserError::NoSuchSession)?;
        match (nav, verdict) {
            (Navigate::Url(_), Some(Err(e))) => return Err(e),
            (Navigate::Url(_), Some(Ok(url))) => {
                s.history.truncate(if s.history.is_empty() { 0 } else { s.pos + 1 });
                s.history.push(url.clone());
                s.pos = s.history.len() - 1;
                load(s, &pages, &url);
            }
            (Navigate::Html { base, html }, _) => {
                s.history.truncate(if s.history.is_empty() { 0 } else { s.pos + 1 });
                s.history.push(base.clone());
                s.pos = s.history.len() - 1;
                s.page = Some(FakePage::new(base, &[html.as_str()]));
                s.error = None;
                s.generation += 1;
            }
            (Navigate::Back, _) if s.pos > 0 => {
                s.pos -= 1;
                let u = s.history[s.pos].clone();
                load(s, &pages, &u);
            }
            (Navigate::Forward, _) if s.pos + 1 < s.history.len() => {
                s.pos += 1;
                let u = s.history[s.pos].clone();
                load(s, &pages, &u);
            }
            (Navigate::Reload, _) if !s.history.is_empty() => {
                let u = s.history[s.pos].clone();
                load(s, &pages, &u);
            }
            _ => {}
        }
        Ok(())
    }

    fn input(&self, user: &str, app: &str, id: SessionId, input: &Input) -> BrowserResult<()> {
        let mut st = self.lock();
        st.log.push(format!("input {input:?}"));
        let pages = st.pages.clone();
        let (bad, denied, off) = (st.bad.clone(), st.denied.clone(), st.off);
        let s = st.sessions.get_mut(&id.0).filter(|s| s.user == user && s.app == app).ok_or(BrowserError::NoSuchSession)?;
        match input {
            Input::Activate(n) => {
                let target = s.page.as_ref().and_then(|p| p.links.get((*n as usize).wrapping_sub(1))).map(|(_, t)| t.clone());
                if let Some(t) = target {
                    let judged = {
                        let tmp = State { bad, denied, off, ..State::default() };
                        judge(&tmp, &t)
                    };
                    let url = judged.map_err(|e| {
                        s.error = Some(e.to_string());
                        s.generation += 1;
                        e
                    })?;
                    s.history.truncate(s.pos + 1);
                    s.history.push(url.clone());
                    s.pos = s.history.len() - 1;
                    load(s, &pages, &url);
                }
            }
            Input::Wheel(d) => {
                s.scroll = (s.scroll - d * 24).max(0);
                s.generation += 1;
            }
            Input::Key(k) => {
                match k {
                    PageKey::Down | PageKey::PageDown => s.scroll += 48,
                    PageKey::Up | PageKey::PageUp => s.scroll = (s.scroll - 48).max(0),
                    PageKey::Home => s.scroll = 0,
                    _ => {}
                }
                s.generation += 1;
            }
            Input::Text(t) if t.len() > MAX_TEXT => return Err(BrowserError::Invalid("too much text".into())),
            Input::Text(_) => {}
            Input::Resize { width, height } => {
                if !(width.is_finite() && height.is_finite()) || *width < 1.0 || *height < 1.0 {
                    return Err(BrowserError::Invalid("the view has no usable size".into()));
                }
                s.spec.width = *width;
                s.spec.height = *height;
                s.generation += 1;
            }
        }
        Ok(())
    }

    fn find(&self, user: &str, app: &str, id: SessionId, query: &str, forward: bool) -> BrowserResult<FindState> {
        let mut st = self.lock();
        st.log.push(format!("find {query:?} {forward}"));
        if query.len() > MAX_URL {
            return Err(BrowserError::Invalid("the search text is too long".into()));
        }
        let s = st.sessions.get_mut(&id.0).filter(|s| s.user == user && s.app == app).ok_or(BrowserError::NoSuchSession)?;
        let n = s.page.as_ref().map_or(0, |p| count_matches(p, query));
        s.find = if query.is_empty() || n == 0 {
            FindState { query: query.into(), matches: n, current: 0 }
        } else if s.find.query == query && s.find.current > 0 {
            let cur = if forward { s.find.current % n + 1 } else { (s.find.current + n - 2) % n + 1 };
            FindState { query: query.into(), matches: n, current: cur }
        } else {
            FindState { query: query.into(), matches: n, current: 1 }
        };
        s.generation += 1;
        Ok(s.find.clone())
    }

    fn state(&self, user: &str, app: &str, id: SessionId) -> BrowserResult<PageState> {
        let st = self.lock();
        let s = st.sessions.get(&id.0).filter(|s| s.user == user && s.app == app).ok_or(BrowserError::NoSuchSession)?;
        Ok(PageState {
            url: s.history.get(s.pos).cloned().unwrap_or_default(),
            title: s.page.as_ref().map(|p| p.title.clone()).unwrap_or_default(),
            loading: false,
            can_back: s.pos > 0,
            can_forward: s.pos + 1 < s.history.len(),
            error: s.error.clone(),
            engine: s.spec.engine,
            find: s.find.clone(),
            generation: s.generation,
            scroll_permille: 0,
        })
    }

    fn frame(&self, user: &str, app: &str, id: SessionId, since: u64) -> BrowserResult<Option<Frame>> {
        let st = self.lock();
        let s = st.sessions.get(&id.0).filter(|s| s.user == user && s.app == app).ok_or(BrowserError::NoSuchSession)?;
        Ok(if s.generation == since { None } else { Some(Frame { generation: s.generation, list: draw(s) }) })
    }

    fn prompts(&self, user: &str, app: &str, id: SessionId) -> Vec<Prompt> {
        self.lock().sessions.get(&id.0).filter(|s| s.user == user && s.app == app).map(|s| s.prompts.clone()).unwrap_or_default()
    }

    fn answer_prompt(&self, user: &str, app: &str, id: SessionId, prompt: u64, allow: bool) -> BrowserResult<()> {
        let mut st = self.lock();
        let s = st.sessions.get_mut(&id.0).filter(|s| s.user == user && s.app == app).ok_or(BrowserError::NoSuchSession)?;
        let at = s.prompts.iter().position(|p| p.id == prompt).ok_or(BrowserError::Invalid("no such question".into()))?;
        let p = s.prompts.remove(at);
        st.answers.push((p.text, allow));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_ui_sdk::list::DisplayList;

    fn fake() -> FakeBrowser {
        let b = FakeBrowser::new();
        b.add_page("https://a.org/", FakePage::new("Home", &["hello world", "second line hello"]).link("Next", "https://a.org/two"));
        b.add_page("https://a.org/two", FakePage::new("Two", &["page two"]).asking(PromptKind::Download, "Save two.zip?"));
        b
    }

    #[test]
    fn the_fake_serves_pages_with_history_find_and_a_frame_that_parses() {
        let b = fake();
        let id = b.open("alice", "sample", &OpenSpec::new(300.0, 200.0)).unwrap();
        b.navigate("alice", "sample", id, &Navigate::Url("a.org/".into())).unwrap();
        let st = b.state("alice", "sample", id).unwrap();
        assert_eq!((st.title.as_str(), st.url.as_str(), st.can_back), ("Home", "https://a.org/", false));
        let f = b.frame("alice", "sample", id, 0).unwrap().unwrap();
        let list = DisplayList::parse(&f.list).unwrap();
        assert!(list.texts().contains(&"hello world"));
        assert!(b.frame("alice", "sample", id, f.generation).unwrap().is_none(), "unchanged: nothing to send");
        let link = list.a11y().into_iter().find(|n| n.name == "Next").unwrap();
        assert_eq!(link.role, Role::Link);
        b.input("alice", "sample", id, &Input::Activate(1)).unwrap();
        assert_eq!(b.state("alice", "sample", id).unwrap().title, "Two");
        assert_eq!(b.prompts("alice", "sample", id)[0].kind, PromptKind::Download);
        b.answer_prompt("alice", "sample", id, 1, false).unwrap();
        assert_eq!(b.answers(), vec![("Save two.zip?".to_string(), false)]);
        b.navigate("alice", "sample", id, &Navigate::Back).unwrap();
        assert_eq!(b.state("alice", "sample", id).unwrap().title, "Home");
        let f = b.find("alice", "sample", id, "hello", true).unwrap();
        assert_eq!((f.matches, f.current), (2, 1));
        assert_eq!(b.find("alice", "sample", id, "hello", true).unwrap().current, 2);
        assert_eq!(b.find("alice", "sample", id, "hello", true).unwrap().current, 1);
    }

    #[test]
    fn the_fake_refuses_what_the_real_core_refuses_and_keeps_sessions_apart() {
        let b = fake();
        b.add_bad_host("bad.example", "phishing");
        b.deny_host("work.example");
        let id = b.open("alice", "sample", &OpenSpec::new(300.0, 200.0)).unwrap();
        assert!(matches!(b.navigate("alice", "sample", id, &Navigate::Url("https://x.bad.example/".into())), Err(BrowserError::Refused(Refusal::KnownBad { .. }))));
        assert!(matches!(b.navigate("alice", "sample", id, &Navigate::Url("https://work.example/".into())), Err(BrowserError::Refused(Refusal::Policy { .. }))));
        assert!(matches!(b.navigate("alice", "sample", id, &Navigate::Url("javascript:alert(1)".into())), Err(BrowserError::Refused(Refusal::BadAddress(_)))));
        // another person or app never reaches this session, and the answer does not say whether it exists
        assert_eq!(b.state("bob", "sample", id), Err(BrowserError::NoSuchSession));
        assert_eq!(b.state("alice", "other", id), Err(BrowserError::NoSuchSession));
        b.close("bob", "sample", id);
        assert_eq!(b.open_sessions(), 1, "a stranger cannot close it");
        for _ in 0..MAX_SESSIONS_PER_APP - 1 {
            b.open("alice", "sample", &OpenSpec::new(10.0, 10.0)).unwrap();
        }
        assert_eq!(b.open("alice", "sample", &OpenSpec::new(10.0, 10.0)), Err(BrowserError::TooMany));
        b.set_off(true);
        assert_eq!(b.open("bob", "sample", &OpenSpec::new(10.0, 10.0)), Err(BrowserError::Refused(Refusal::Off)));
    }

    #[test]
    fn the_full_engine_is_asked_for_by_name_and_auto_falls_back() {
        let b = fake();
        let mut spec = OpenSpec::new(100.0, 100.0);
        spec.engine = EnginePref::Full;
        assert!(matches!(b.open("alice", "s", &spec), Err(BrowserError::Unavailable(_))));
        spec.engine = EnginePref::Auto;
        let id = b.open("alice", "s", &spec).unwrap();
        assert_eq!(b.state("alice", "s", id).unwrap().engine, EnginePref::InHouse);
        b.set_full_engine_installed(true);
        let id2 = b.open("alice", "s", &spec).unwrap();
        assert_eq!(b.state("alice", "s", id2).unwrap().engine, EnginePref::Full);
    }

    /// F-SDK-60 (UT2): the rest of the facet end to end on the fake: forward, reload and stop, a page given as text, wheel, key, resize and typed
    /// text (each moves the frame's generation or is refused), a permission question and its answer, and a bad answer or size.
    #[test]
    fn history_input_resize_and_a_permission_question_work_through_the_facet_and_every_change_moves_the_generation() {
        let b = fake();
        b.add_page("https://a.org/cam", FakePage::new("Camera", &["a meeting"]).asking(PromptKind::Permission, "Use the camera?"));
        let id = b.open("alice", "sample", &OpenSpec::new(300.0, 200.0)).unwrap();
        b.navigate("alice", "sample", id, &Navigate::Url("https://a.org/".into())).unwrap();
        b.navigate("alice", "sample", id, &Navigate::Url("https://a.org/cam".into())).unwrap();
        let st = b.state("alice", "sample", id).unwrap();
        assert_eq!((st.can_back, st.can_forward, st.title.as_str()), (true, false, "Camera"));
        let ask = b.prompts("alice", "sample", id);
        assert_eq!((ask.len(), ask[0].kind), (1, PromptKind::Permission));
        b.answer_prompt("alice", "sample", id, ask[0].id, true).unwrap();
        assert_eq!(b.answers(), vec![("Use the camera?".to_string(), true)]);
        assert!(b.prompts("alice", "sample", id).is_empty(), "an answered question is gone");
        assert!(matches!(b.answer_prompt("alice", "sample", id, 99, true), Err(BrowserError::Invalid(_))));
        b.navigate("alice", "sample", id, &Navigate::Back).unwrap();
        b.navigate("alice", "sample", id, &Navigate::Forward).unwrap();
        assert_eq!(b.state("alice", "sample", id).unwrap().title, "Camera");
        b.navigate("alice", "sample", id, &Navigate::Reload).unwrap();
        b.navigate("alice", "sample", id, &Navigate::Stop).unwrap();
        // each input that changes what is drawn moves the generation, so the next frame is a new one
        let gen = |b: &FakeBrowser| b.state("alice", "sample", id).unwrap().generation;
        for input in [Input::Wheel(-2), Input::Key(PageKey::Down), Input::Key(PageKey::Home), Input::Resize { width: 120.0, height: 90.0 }] {
            let before = gen(&b);
            b.input("alice", "sample", id, &input).unwrap();
            assert!(gen(&b) > before, "{input:?}");
            assert!(b.frame("alice", "sample", id, before).unwrap().is_some(), "{input:?}");
        }
        let before = gen(&b);
        b.input("alice", "sample", id, &Input::Text("hello".into())).unwrap();
        assert_eq!(gen(&b), before, "the in-house page has no text field: typed text changes nothing");
        assert!(matches!(b.input("alice", "sample", id, &Input::Text("x".repeat(MAX_TEXT + 1))), Err(BrowserError::Invalid(_))));
        assert!(matches!(b.input("alice", "sample", id, &Input::Resize { width: 0.0, height: 10.0 }), Err(BrowserError::Invalid(_))));
        // a page given as text opens in the same session and joins the history
        b.navigate("alice", "sample", id, &Navigate::Html { base: "https://a.org/mine".into(), html: "made by the app".into() }).unwrap();
        let f = b.frame("alice", "sample", id, 0).unwrap().unwrap();
        assert!(DisplayList::parse(&f.list).unwrap().texts().iter().any(|t| t.contains("made by the app")));
        assert!(matches!(b.navigate("alice", "sample", id, &Navigate::Html { base: "https://a.org/x".into(), html: "x".repeat(MAX_HTML + 1) }), Err(BrowserError::Invalid(_))));
        b.close("alice", "sample", id);
        assert_eq!(b.state("alice", "sample", id), Err(BrowserError::NoSuchSession));
    }
}
