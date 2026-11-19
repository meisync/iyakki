use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use iyakki_core_api::browsersession::{check_address, BrowserError, BrowserResult, BrowserSessions, EnginePref, FindState, Frame, Input, Navigate, OpenSpec, PageKey, PageState, Prompt, PromptKind, Refusal, SessionId, MAX_HTML, MAX_SESSIONS_PER_APP, MAX_TEXT, MAX_URL};
use iyakki_web::doc::Doc;
use iyakki_web::fetch::{load, Loaded};
use iyakki_web::url::Url;

use crate::page::{self, FrameIn, Layout};
use crate::BrowserHost;

/// The most sessions the whole core holds (a runaway app cannot fill memory).
const MAX_SESSIONS: usize = 64;
const MAX_PROMPTS: usize = 4;
const STEP: f32 = 40.0;

enum Entry {
    Url(String),
    Html { base: String, html: String },
}

struct Page {
    url: String,
    title: String,
    doc: Doc,
    layout: Layout,
}

struct Sess {
    user: String,
    app: String,
    spec: OpenSpec,
    engine: EnginePref,
    history: Vec<Entry>,
    pos: usize,
    page: Option<Page>,
    loading: bool,
    load_gen: u64,
    error: Option<String>,
    banner: Option<String>,
    find: FindState,
    matches: Vec<(usize, usize, usize)>,
    scroll: f32,
    generation: u64,
    prompts: Vec<(Prompt, String)>,
    next_prompt: u64,
}

fn lk<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub struct LocalBrowserSessions {
    host: Arc<dyn BrowserHost>,
    sessions: Mutex<HashMap<u64, Arc<Mutex<Sess>>>>,
    next: AtomicU64,
}

fn human(n: usize) -> String {
    if n >= 1 << 20 {
        format!("{:.1} MB", n as f64 / (1 << 20) as f64)
    } else if n >= 1 << 10 {
        format!("{} KB", n >> 10)
    } else {
        format!("{n} bytes")
    }
}

impl Sess {
    fn max_scroll(&self) -> f32 {
        let top = if self.banner_text().is_some() { 8.0 + 14.0 * self.spec.text_scale * 1.6 } else { 0.0 };
        self.page.as_ref().map_or(0.0, |p| (p.layout.height + top - self.spec.height).max(0.0))
    }

    fn banner_text(&self) -> Option<String> {
        if self.loading {
            return Some(match self.history.get(self.pos) {
                Some(Entry::Url(u)) => format!("Loading {u}"),
                _ => "Loading".to_string(),
            });
        }
        if let Some(e) = &self.error {
            return Some(e.clone());
        }
        if let Some(b) = &self.banner {
            return Some(b.clone());
        }
        if self.page.is_none() {
            return Some("Nothing is open.".into());
        }
        None
    }

    fn relayout(&mut self) {
        let (w, ts, d) = (self.spec.width, self.spec.text_scale, self.spec.defaults);
        if let Some(p) = &mut self.page {
            p.layout = page::layout(&p.doc, w, ts, &d);
        }
        self.matches = self.page.as_ref().map(|p| page::find_all(&p.layout, &self.find.query)).unwrap_or_default();
        self.find.matches = self.matches.len();
        self.find.current = self.find.current.min(self.find.matches);
        self.scroll = self.scroll.clamp(0.0, self.max_scroll());
    }

    fn set_page(&mut self, url: String, doc: Doc) {
        let title = if doc.title.trim().is_empty() { url.clone() } else { doc.title.trim().chars().take(200).collect() };
        self.page = Some(Page { url, title, layout: Layout::default(), doc });
        self.scroll = 0.0;
        self.find = FindState::default();
        self.banner = None;
        self.relayout();
    }

    fn bump(&mut self) {
        self.generation += 1;
    }

    fn raise(&mut self, kind: PromptKind, origin: String, text: String, url: String) {
        if self.prompts.len() >= MAX_PROMPTS {
            return;
        }
        self.next_prompt += 1;
        self.prompts.push((Prompt { id: self.next_prompt, kind, origin, text }, url));
    }

    fn state(&self) -> PageState {
        let max = self.max_scroll();
        let url = self
            .page
            .as_ref()
            .map(|p| p.url.clone())
            .or_else(|| match self.history.get(self.pos) {
                Some(Entry::Url(u)) => Some(u.clone()),
                Some(Entry::Html { base, .. }) => Some(base.clone()),
                None => None,
            })
            .unwrap_or_default();
        PageState {
            url,
            title: self.page.as_ref().map(|p| p.title.clone()).unwrap_or_default(),
            loading: self.loading,
            can_back: self.pos > 0,
            can_forward: self.pos + 1 < self.history.len(),
            error: self.error.clone(),
            engine: self.engine,
            find: self.find.clone(),
            generation: self.generation,
            scroll_permille: if max <= 0.0 { 0 } else { ((self.scroll / max) * 1000.0).round().clamp(0.0, 1000.0) as u16 },
        }
    }
}

impl LocalBrowserSessions {
    pub fn new(host: Arc<dyn BrowserHost>) -> LocalBrowserSessions {
        LocalBrowserSessions { host, sessions: Mutex::new(HashMap::new()), next: AtomicU64::new(1) }
    }

    /// End every session of a person (sign-out).
    pub fn end_sessions_of(&self, user: &str) -> usize {
        let mut m = lk(&self.sessions);
        let before = m.len();
        m.retain(|_, s| lk(s).user != user);
        before - m.len()
    }

    pub fn open_sessions(&self) -> usize {
        lk(&self.sessions).len()
    }

    fn get(&self, user: &str, app: &str, id: SessionId) -> BrowserResult<Arc<Mutex<Sess>>> {
        let s = lk(&self.sessions).get(&id.0).cloned().ok_or(BrowserError::NoSuchSession)?;
        let ok = {
            let g = lk(&s);
            g.user == user && g.app == app
        };
        if ok {
            Ok(s)
        } else {
            Err(BrowserError::NoSuchSession)
        }
    }

    /// The address as the core will open it, or why not. Everything that opens a page goes through here.
    fn judge(&self, user: &str, app: &str, raw: &str) -> Result<String, BrowserError> {
        if !self.host.enabled(user) {
            return Err(BrowserError::Refused(Refusal::Off));
        }
        let t = check_address(raw).map_err(BrowserError::Refused)?;
        let url = Url::from_input(&t).map_err(|e| BrowserError::Refused(Refusal::BadAddress(e)))?;
        if let Some((listed, reason)) = self.host.known_bad(&url.host) {
            self.host.audit(user, app, "browser.embed.refused", &format!("known-bad {}", url.host));
            return Err(BrowserError::Refused(Refusal::KnownBad { host: listed, reason }));
        }
        if !self.host.allows_host(user, &url.host) {
            self.host.audit(user, app, "browser.embed.refused", &format!("policy {}", url.host));
            return Err(BrowserError::Refused(Refusal::Policy { host: url.host }));
        }
        Ok(url.to_string())
    }

    fn start_load(&self, sess: &Arc<Mutex<Sess>>, g: &mut Sess, url: String) {
        g.load_gen += 1;
        g.loading = true;
        g.error = None;
        g.banner = None;
        g.bump();
        let gen = g.load_gen;
        let (user, app) = (g.user.clone(), g.app.clone());
        let host = self.host.clone();
        let sess = sess.clone();
        let fetch = host.fetch(&user);
        self.host.audit(&user, &app, "browser.embed.navigate", &Url::parse(&url).map(|u| u.host).unwrap_or_default());
        std::thread::spawn(move || {
            let allow = |h: &str| host.known_bad(h).is_none() && host.allows_host(&user, h);
            let result = match Url::parse(&url) {
                Ok(u) => load(&*fetch, &u, &allow),
                Err(e) => Err(e),
            };
            let mut g = lk(&sess);
            if g.load_gen != gen {
                return; // a newer navigation, a stop or a close came first
            }
            g.loading = false;
            match result {
                Ok(Loaded::Page { url, doc, .. }) => {
                    let shown = url.to_string();
                    let at = g.pos;
                    if let Some(Entry::Url(u)) = g.history.get_mut(at) {
                        *u = shown.clone();
                    }
                    g.set_page(shown, doc);
                }
                Ok(Loaded::Unsupported { url, content_type, bytes, info }) => {
                    let name = url.path.rsplit('/').find(|s| !s.is_empty()).unwrap_or("the file").chars().take(80).collect::<String>();
                    let said = if info.is_empty() { format!("{name} is a file ({content_type}, {}), not a page.", human(bytes)) } else { format!("{name} is a file ({info}), not a page.") };
                    g.banner = Some(said);
                    g.raise(PromptKind::Download, url.host.clone(), format!("Save {name} ({}) to your downloads?", human(bytes)), url.to_string());
                }
                Err(e) => g.error = Some(e),
            }
            g.bump();
        });
    }

    fn go_to(&self, sess: &Arc<Mutex<Sess>>, url: String, push: bool) {
        let mut g = lk(sess);
        if push {
            let keep = if g.history.is_empty() { 0 } else { g.pos + 1 };
            g.history.truncate(keep);
            g.history.push(Entry::Url(url.clone()));
            g.pos = g.history.len() - 1;
        }
        self.start_load(sess, &mut g, url);
    }

    fn show_entry(&self, sess: &Arc<Mutex<Sess>>) {
        let mut g = lk(sess);
        match g.history.get(g.pos) {
            Some(Entry::Url(u)) => {
                let u = u.clone();
                self.start_load(sess, &mut g, u);
            }
            Some(Entry::Html { base, html }) => {
                let (base, html) = (base.clone(), html.clone());
                g.load_gen += 1;
                g.loading = false;
                g.error = None;
                g.set_page(base, iyakki_web::doc::from_html(&html));
                g.bump();
            }
            None => {}
        }
    }
}

impl BrowserSessions for LocalBrowserSessions {
    fn enabled(&self, user: &str) -> bool {
        self.host.enabled(user)
    }

    fn set_enabled(&self, user: &str, on: bool) -> BrowserResult<()> {
        self.host.set_enabled(user, on).map_err(BrowserError::Unavailable)?;
        if !on {
            self.end_sessions_of(user);
        }
        self.host.audit(user, "", "browser.embed.switch", if on { "on" } else { "off" });
        Ok(())
    }

    fn open_views(&self, user: &str) -> usize {
        lk(&self.sessions).values().filter(|s| lk(s).user == user).count()
    }

    fn open(&self, user: &str, app: &str, spec: &OpenSpec) -> BrowserResult<SessionId> {
        if !self.host.enabled(user) {
            return Err(BrowserError::Refused(Refusal::Off));
        }
        if !(spec.width.is_finite() && spec.height.is_finite()) || spec.width < 40.0 || spec.height < 20.0 || spec.width > 8192.0 || spec.height > 8192.0 {
            return Err(BrowserError::Invalid("the view has no usable size".into()));
        }
        let engine = match spec.engine {
            EnginePref::InHouse => EnginePref::InHouse,
            EnginePref::Full if self.host.full_engine(user) => EnginePref::Full,
            EnginePref::Full => return Err(BrowserError::Unavailable("the full engine is not installed or not switched on".into())),
            EnginePref::Auto if self.host.full_engine(user) => EnginePref::Full,
            EnginePref::Auto => EnginePref::InHouse,
        };
        if engine == EnginePref::Full {
            // the full engine draws into its own surface, not into a display list: not wired for embedding yet
            return Err(BrowserError::Unavailable("the full engine cannot be embedded yet".into()));
        }
        let mut m = lk(&self.sessions);
        let mine = m
            .values()
            .filter(|s| {
                let g = lk(s);
                g.user == user && g.app == app
            })
            .count();
        if m.len() >= MAX_SESSIONS || mine >= MAX_SESSIONS_PER_APP {
            return Err(BrowserError::TooMany);
        }
        let id = self.next.fetch_add(1, Ordering::SeqCst);
        let mut spec = spec.clone();
        spec.text_scale = if spec.text_scale.is_finite() { spec.text_scale.clamp(0.5, 3.0) } else { 1.0 };
        m.insert(
            id,
            Arc::new(Mutex::new(Sess {
                user: user.into(),
                app: app.into(),
                spec,
                engine,
                history: Vec::new(),
                pos: 0,
                page: None,
                loading: false,
                load_gen: 0,
                error: None,
                banner: None,
                find: FindState::default(),
                matches: Vec::new(),
                scroll: 0.0,
                generation: 1,
                prompts: Vec::new(),
                next_prompt: 0,
            })),
        );
        drop(m);
        self.host.audit(user, app, "browser.embed.open", &format!("session {id}"));
        Ok(SessionId(id))
    }

    fn close(&self, user: &str, app: &str, id: SessionId) {
        if self.get(user, app, id).is_ok() {
            lk(&self.sessions).remove(&id.0);
            self.host.audit(user, app, "browser.embed.close", &format!("session {}", id.0));
        }
    }

    fn navigate(&self, user: &str, app: &str, id: SessionId, nav: &Navigate) -> BrowserResult<()> {
        let sess = self.get(user, app, id)?;
        match nav {
            Navigate::Url(raw) => {
                let url = self.judge(user, app, raw)?;
                self.go_to(&sess, url, true);
            }
            Navigate::Html { base, html } => {
                if !self.host.enabled(user) {
                    return Err(BrowserError::Refused(Refusal::Off));
                }
                if html.len() > MAX_HTML {
                    return Err(BrowserError::Invalid("the page is too big".into()));
                }
                if base.len() > MAX_URL || base.chars().any(char::is_control) {
                    return Err(BrowserError::Invalid("the address given for the page is not usable".into()));
                }
                let mut g = lk(&sess);
                let keep = if g.history.is_empty() { 0 } else { g.pos + 1 };
                g.history.truncate(keep);
                g.history.push(Entry::Html { base: base.clone(), html: html.clone() });
                g.pos = g.history.len() - 1;
                drop(g);
                self.show_entry(&sess);
            }
            Navigate::Back => {
                let ok = {
                    let mut g = lk(&sess);
                    if g.pos > 0 {
                        g.pos -= 1;
                        true
                    } else {
                        false
                    }
                };
                if ok {
                    self.show_entry(&sess);
                }
            }
            Navigate::Forward => {
                let ok = {
                    let mut g = lk(&sess);
                    if g.pos + 1 < g.history.len() {
                        g.pos += 1;
                        true
                    } else {
                        false
                    }
                };
                if ok {
                    self.show_entry(&sess);
                }
            }
            Navigate::Reload => self.show_entry(&sess),
            Navigate::Stop => {
                let mut g = lk(&sess);
                if g.loading {
                    g.load_gen += 1;
                    g.loading = false;
                    g.bump();
                }
            }
        }
        Ok(())
    }

    fn input(&self, user: &str, app: &str, id: SessionId, input: &Input) -> BrowserResult<()> {
        let sess = self.get(user, app, id)?;
        match input {
            Input::Activate(n) => {
                let target = {
                    let g = lk(&sess);
                    let p = g.page.as_ref().ok_or(BrowserError::Invalid("there is no page".into()))?;
                    let href = p.doc.links.get((*n as usize).wrapping_sub(1)).cloned();
                    href.map(|h| (h, p.url.clone()))
                };
                let Some((href, base)) = target else { return Ok(()) };
                let joined = Url::parse(&base).and_then(|b| b.join(&href)).map(|u| u.to_string());
                match joined {
                    Err(e) => {
                        let mut g = lk(&sess);
                        g.error = Some(e.clone());
                        g.bump();
                        return Err(BrowserError::Refused(Refusal::BadAddress(e)));
                    }
                    Ok(u) => match self.judge(user, app, &u) {
                        Ok(url) => self.go_to(&sess, url, true),
                        Err(e) => {
                            let mut g = lk(&sess);
                            g.error = Some(e.to_string());
                            g.bump();
                            return Err(e);
                        }
                    },
                }
            }
            Input::Wheel(d) => {
                let mut g = lk(&sess);
                let max = g.max_scroll();
                g.scroll = (g.scroll - *d as f32 * STEP).clamp(0.0, max);
                g.bump();
            }
            Input::Key(k) => {
                let mut g = lk(&sess);
                let (max, page_h) = (g.max_scroll(), g.spec.height * 0.9);
                g.scroll = match k {
                    PageKey::Up => g.scroll - STEP,
                    PageKey::Down => g.scroll + STEP,
                    PageKey::PageUp => g.scroll - page_h,
                    PageKey::PageDown => g.scroll + page_h,
                    PageKey::Home => 0.0,
                    PageKey::End => max,
                    PageKey::Enter | PageKey::Esc => g.scroll,
                }
                .clamp(0.0, max);
                g.bump();
            }
            Input::Text(t) => {
                if t.len() > MAX_TEXT {
                    return Err(BrowserError::Invalid("too much text".into()));
                }
            }
            Input::Resize { width, height } => {
                if !(width.is_finite() && height.is_finite()) || *width < 40.0 || *height < 20.0 || *width > 8192.0 || *height > 8192.0 {
                    return Err(BrowserError::Invalid("the view has no usable size".into()));
                }
                let mut g = lk(&sess);
                let wider = (g.spec.width - *width).abs() >= 1.0;
                g.spec.width = *width;
                g.spec.height = *height;
                if wider {
                    g.relayout();
                }
                g.scroll = g.scroll.clamp(0.0, g.max_scroll());
                g.bump();
            }
        }
        Ok(())
    }

    fn find(&self, user: &str, app: &str, id: SessionId, query: &str, forward: bool) -> BrowserResult<FindState> {
        let sess = self.get(user, app, id)?;
        if query.len() > MAX_URL || query.chars().any(char::is_control) {
            return Err(BrowserError::Invalid("the search text is not usable".into()));
        }
        let mut g = lk(&sess);
        let same = g.find.query == query && g.find.current > 0;
        let matches = g.page.as_ref().map(|p| page::find_all(&p.layout, query)).unwrap_or_default();
        let n = matches.len();
        let cur = if query.is_empty() || n == 0 {
            0
        } else if same {
            if forward {
                g.find.current % n + 1
            } else {
                (g.find.current + n - 2) % n + 1
            }
        } else {
            1
        };
        g.matches = matches;
        g.find = FindState { query: query.to_string(), matches: n, current: cur };
        if cur > 0 {
            let (li, _, _) = g.matches[cur - 1];
            let y = g.page.as_ref().map_or(0.0, |p| p.layout.lines[li].y);
            let max = g.max_scroll();
            g.scroll = (y - g.spec.height / 3.0).clamp(0.0, max);
        }
        g.bump();
        Ok(g.find.clone())
    }

    fn state(&self, user: &str, app: &str, id: SessionId) -> BrowserResult<PageState> {
        let s = self.get(user, app, id)?;
        let st = lk(&s).state();
        Ok(st)
    }

    fn frame(&self, user: &str, app: &str, id: SessionId, since: u64) -> BrowserResult<Option<Frame>> {
        let sess = self.get(user, app, id)?;
        let g = lk(&sess);
        if g.generation == since {
            return Ok(None);
        }
        let empty = Layout::default();
        let layout = g.page.as_ref().map_or(&empty, |p| &p.layout);
        let banner = g.banner_text();
        let link_count = g.page.as_ref().map_or(0, |p| p.doc.links.len());
        let list = page::frame(&FrameIn { layout, width: g.spec.width, height: g.spec.height, palette: g.spec.palette, scroll: g.scroll, find: &g.find, matches: &g.matches, banner: banner.as_deref(), link_count, defaults: g.spec.defaults, text_scale: g.spec.text_scale });
        Ok(Some(Frame { generation: g.generation, list }))
    }

    fn prompts(&self, user: &str, app: &str, id: SessionId) -> Vec<Prompt> {
        self.get(user, app, id).map(|s| lk(&s).prompts.iter().map(|(p, _)| p.clone()).collect()).unwrap_or_default()
    }

    fn answer_prompt(&self, user: &str, app: &str, id: SessionId, prompt: u64, allow: bool) -> BrowserResult<()> {
        let sess = self.get(user, app, id)?;
        let (p, url) = {
            let mut g = lk(&sess);
            let at = g.prompts.iter().position(|(p, _)| p.id == prompt).ok_or(BrowserError::Invalid("no such question".into()))?;
            g.prompts.remove(at)
        };
        let said = match (p.kind, allow) {
            (PromptKind::Download, true) => {
                let r = self.host.save_download(user, app, &url);
                self.host.audit(user, app, "browser.embed.download", &format!("{} {}", p.origin, if r.is_ok() { "saved" } else { "failed" }));
                Some(match r {
                    Ok(m) => m,
                    Err(e) => e,
                })
            }
            (PromptKind::Download, false) => {
                self.host.audit(user, app, "browser.embed.download", &format!("{} declined", p.origin));
                Some("The download was declined.".to_string())
            }
            (PromptKind::Permission, _) => None,
        };
        let mut g = lk(&sess);
        if let Some(s) = said {
            g.banner = Some(s);
        }
        g.bump();
        Ok(())
    }
}
