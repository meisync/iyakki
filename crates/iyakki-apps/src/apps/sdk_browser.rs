//! The core-backed [`BrowserPort`] of an SDK app's `BrowserView` (F-SDK-60): the SDK's own port types mapped onto the core's browser
//! sessions (`iyakki_core_api::BrowserSessions`, reached as `cx.core.browser()`). One adapter serves every SDK app that embeds the browser;
//! the person and the app id are fixed when it is made, so a view can only reach the sessions of its own app. The adapter holds no
//! network, file or device: addresses are judged, pages loaded and frames drawn by the core.

use std::sync::Arc;

use iyakki_core_api::browsersession::{BrowserError, EnginePref, PageDefaults, PageFont, Scheme, FindState, Input, Navigate, OpenSpec, Palette, PageKey as CoreKey, PageState, Prompt, PromptKind as CoreKind, SessionId};
use iyakki_core_api::Core;
use iyakki_ui_sdk::browser::{BrowserPort, Engine, PageFamily, PageScheme, FindInfo, Nav, PageInfo, PageInput, PageKey, PagePrompt, PromptKind, ViewSpec};

pub struct CoreBrowserPort {
    core: Arc<dyn Core>,
    user: String,
    app: String,
}

impl CoreBrowserPort {
    pub fn new(core: Arc<dyn Core>, user: &str, app: &str) -> CoreBrowserPort {
        CoreBrowserPort { core, user: user.into(), app: app.into() }
    }
}

fn eng(e: Engine) -> EnginePref {
    match e {
        Engine::InHouse => EnginePref::InHouse,
        Engine::Full => EnginePref::Full,
        Engine::Auto => EnginePref::Auto,
    }
}

fn eng_back(e: EnginePref) -> Engine {
    match e {
        EnginePref::InHouse => Engine::InHouse,
        EnginePref::Full => Engine::Full,
        EnginePref::Auto => Engine::Auto,
    }
}

fn find(f: FindState) -> FindInfo {
    FindInfo { query: f.query, matches: f.matches, current: f.current }
}

fn key(k: PageKey) -> CoreKey {
    match k {
        PageKey::Up => CoreKey::Up,
        PageKey::Down => CoreKey::Down,
        PageKey::PageUp => CoreKey::PageUp,
        PageKey::PageDown => CoreKey::PageDown,
        PageKey::Home => CoreKey::Home,
        PageKey::End => CoreKey::End,
        PageKey::Enter => CoreKey::Enter,
        PageKey::Esc => CoreKey::Esc,
    }
}

fn state(s: PageState) -> PageInfo {
    PageInfo { url: s.url, title: s.title, loading: s.loading, can_back: s.can_back, can_forward: s.can_forward, error: s.error, engine: eng_back(s.engine), find: find(s.find), generation: s.generation, scroll_permille: s.scroll_permille }
}

fn prompt(p: Prompt) -> PagePrompt {
    PagePrompt { id: p.id, kind: if p.kind == CoreKind::Download { PromptKind::Download } else { PromptKind::Permission }, origin: p.origin, text: p.text }
}

fn text(e: BrowserError) -> String {
    e.to_string()
}

impl BrowserPort for CoreBrowserPort {
    fn open(&mut self, s: &ViewSpec) -> Result<u64, String> {
        let p = s.palette;
        let spec = OpenSpec { width: s.width, height: s.height, text_scale: s.text_scale, engine: eng(s.engine), palette: Palette { bg: p.bg, text: p.text, muted: p.muted, link: p.link, rule: p.rule, code_bg: p.code_bg, mark: p.mark }, defaults: PageDefaults { scheme: match s.scheme { PageScheme::Dark => Scheme::Dark, PageScheme::Light => Scheme::Light, PageScheme::HighContrast => Scheme::HighContrast }, font_size: s.font_size, font: match s.family { PageFamily::Sans => PageFont::Sans, PageFamily::Mono => PageFont::Mono }, reduced_motion: s.reduced_motion } };
        self.core.browser().open(&self.user, &self.app, &spec).map(|i| i.0).map_err(text)
    }

    fn close(&mut self, id: u64) {
        self.core.browser().close(&self.user, &self.app, SessionId(id));
    }

    fn navigate(&mut self, id: u64, nav: &Nav) -> Result<(), String> {
        let n = match nav {
            Nav::Url(u) => Navigate::Url(u.clone()),
            Nav::Html { base, html } => Navigate::Html { base: base.clone(), html: html.clone() },
            Nav::Back => Navigate::Back,
            Nav::Forward => Navigate::Forward,
            Nav::Reload => Navigate::Reload,
            Nav::Stop => Navigate::Stop,
        };
        self.core.browser().navigate(&self.user, &self.app, SessionId(id), &n).map_err(text)
    }

    fn input(&mut self, id: u64, input: &PageInput) -> Result<(), String> {
        let i = match input {
            PageInput::Activate(n) => Input::Activate(*n),
            PageInput::Wheel(d) => Input::Wheel(*d),
            PageInput::Key(k) => Input::Key(key(*k)),
            PageInput::Text(t) => Input::Text(t.clone()),
            PageInput::Resize { width, height } => Input::Resize { width: *width, height: *height },
        };
        self.core.browser().input(&self.user, &self.app, SessionId(id), &i).map_err(text)
    }

    fn find(&mut self, id: u64, query: &str, forward: bool) -> Result<FindInfo, String> {
        self.core.browser().find(&self.user, &self.app, SessionId(id), query, forward).map(find).map_err(text)
    }

    fn state(&mut self, id: u64) -> Result<PageInfo, String> {
        self.core.browser().state(&self.user, &self.app, SessionId(id)).map(state).map_err(text)
    }

    fn frame(&mut self, id: u64, since: u64) -> Result<Option<(u64, String)>, String> {
        self.core.browser().frame(&self.user, &self.app, SessionId(id), since).map(|f| f.map(|f| (f.generation, f.list))).map_err(text)
    }

    fn prompts(&mut self, id: u64) -> Vec<PagePrompt> {
        self.core.browser().prompts(&self.user, &self.app, SessionId(id)).into_iter().map(prompt).collect()
    }

    fn answer(&mut self, id: u64, p: u64, allow: bool) -> Result<(), String> {
        self.core.browser().answer_prompt(&self.user, &self.app, SessionId(id), p, allow).map_err(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_fake::{FakeCore, FakePage};
    use iyakki_ui_sdk::browser::{BrowserEvent, BrowserView};
    use iyakki_ui_sdk::canvas::Canvas;
    use iyakki_ui_sdk::desktop::{Arrangement, MemDock, MemWallpaper, Pane, ViewInfo};
    use iyakki_ui_sdk::event::UiEvent;
    use iyakki_ui_sdk::geom::Rect;
    use iyakki_ui_sdk::list::ListCanvas;
    use iyakki_ui_sdk::look::Look;
    use iyakki_ui_sdk::cx::UiCx;

    fn view(core: &Arc<FakeCore>) -> BrowserView {
        BrowserView::new(Box::new(CoreBrowserPort::new(core.clone(), "alice", "sample")), 5_000)
    }

    /// Draw the view once the way an app does, on a recording canvas; returns the list.
    fn draw(v: &mut BrowserView) -> iyakki_ui_sdk::list::DisplayList {
        let look = Look::default();
        let arr = Arrangement::single(800, 480);
        let pane = Pane { screen: 0, view: "main".into(), w: 800.0, h: 480.0 };
        let vi = ViewInfo { name: "main".into(), index: 0, count: 1 };
        let (mut dock, mut wp) = (MemDock::knowing(&["sample"]), MemWallpaper::default());
        let cx = UiCx { look: &look, arrangement: &arr, pane: &pane, view: &vi, touch: false, text_scale: 1.0, now_ms: 0, type_style: Default::default(), dock: &mut dock, wallpaper: &mut wp };
        let mut c = ListCanvas::new(800.0, 480.0);
        v.draw(&mut c as &mut dyn Canvas, &cx, Rect::new(10.0, 10.0, 780.0, 400.0));
        c.finish()
    }

    fn core() -> Arc<FakeCore> {
        let core = Arc::new(FakeCore::new());
        let b = core.browser_fake();
        b.add_page("https://a.org/", FakePage::new("Alpha", &["alpha text"]).link("Zip", "https://a.org/f.zip"));
        b.add_page("https://a.org/f.zip", FakePage::new("File", &["a file"]).asking(iyakki_core_api::browsersession::PromptKind::Download, "Save f.zip?"));
        b.add_bad_host("bad.example", "phishing");
        core
    }

    #[test]
    fn a_view_opens_a_session_through_the_core_draws_its_frame_inside_its_rectangle_and_closes_it() {
        let core = core();
        let mut v = view(&core);
        v.load_url("a.org");
        let l = draw(&mut v);
        assert!(l.texts().contains(&"alpha text"), "{:?}", l.texts());
        // the page is drawn inside the rectangle the app gave the view, and its link is a named control in the view's action range
        let link = l.ops.iter().find_map(|o| if let iyakki_ui_sdk::list::Op::Hit(r, h) = o { (h.name == "Zip").then_some((*r, h.action)) } else { None }).unwrap();
        assert!(link.0.x >= 10.0 && link.0.y >= 10.0 && link.0.bottom() <= 410.0, "{:?}", link.0);
        assert!(v.owns_action(link.1));
        assert_eq!((v.title(), v.url()), ("Alpha", "https://a.org/"));
        let ev = v.take_events();
        assert!(ev.contains(&BrowserEvent::Opened) && ev.contains(&BrowserEvent::Title("Alpha".into())) && ev.contains(&BrowserEvent::Url("https://a.org/".into())), "{ev:?}");
        // the person and the app that were given to the port are what the core saw
        assert!(core.browser_fake().calls().iter().any(|c| c == "open alice sample"));
        // a tap on the link goes to the core as the page's action, not the view's
        assert!(v.event(&UiEvent::Action(link.1)));
        assert_eq!(v.title(), "File");
        assert_eq!(core.browser_fake().calls().iter().filter(|c| c.starts_with("input Activate(1)")).count(), 1);
        v.close();
        assert_eq!(core.browser_fake().open_sessions(), 0);
    }

    #[test]
    fn the_themes_scheme_size_and_motion_choice_reach_the_core_as_the_pages_defaults() {
        use iyakki_core_api::browsersession::Scheme;
        let core = core();
        let mut v = view(&core);
        v.load_url("a.org");
        draw(&mut v);
        let d = core.browser_fake().opened_specs()[0].defaults;
        assert_eq!((d.scheme, d.font_size, d.reduced_motion), (Scheme::Dark, 15.0, false), "the default Look is the dark theme with full motion");
        let sp = core.browser_fake().opened_specs()[0].clone();
        assert_eq!(sp.palette.text, Look::default().theme.text, "the page's colours are the Look's, none of the view's own");
    }

    #[test]
    fn a_download_question_is_routed_to_the_person_and_the_answer_goes_back_through_the_core() {
        let core = core();
        let mut v = view(&core);
        v.load_url("a.org");
        let l = draw(&mut v);
        let zip = l.ops.iter().find_map(|o| if let iyakki_ui_sdk::list::Op::Hit(_, h) = o { (h.name == "Zip").then_some(h.action) } else { None }).unwrap();
        v.event(&UiEvent::Action(zip));
        assert_eq!(v.prompts().len(), 1);
        assert!(v.take_events().iter().any(|e| matches!(e, BrowserEvent::Prompt(p) if p.text == "Save f.zip?")));
        let l = draw(&mut v);
        assert!(l.a11y().iter().any(|n| n.name == "Allow") && l.a11y().iter().any(|n| n.name == "Deny"), "the bar has both answers");
        let deny = l.ops.iter().find_map(|o| if let iyakki_ui_sdk::list::Op::Hit(_, h) = o { (h.name == "Deny").then_some(h.action) } else { None }).unwrap();
        v.event(&UiEvent::Action(deny));
        assert_eq!(core.browser_fake().answers(), vec![("Save f.zip?".to_string(), false)]);
        assert!(v.prompts().is_empty());
    }

    #[test]
    fn a_site_the_core_refuses_is_reported_by_the_view_and_nothing_was_loaded() {
        let core = core();
        let mut v = view(&core);
        v.load_url("https://login.bad.example/");
        draw(&mut v);
        let ev = v.take_events();
        assert!(ev.iter().any(|e| matches!(e, BrowserEvent::Failed(Some(w)) if w.contains("known-bad"))), "{ev:?}");
        assert_eq!(v.url(), "");
    }

    #[test]
    fn a_core_without_browsers_makes_the_view_say_so_and_offer_a_retry() {
        let core = Arc::new(FakeCore::new());
        core.browser_fake().set_off(true);
        let mut v = view(&core);
        v.load_url("a.org");
        let l = draw(&mut v);
        assert!(l.texts().iter().any(|t| t.contains("not available") && t.contains("switched off")), "{:?}", l.texts());
        assert!(l.a11y().iter().any(|n| n.name == "Try again"));
        // switched on again: the retry opens it
        core.browser_fake().set_off(false);
        core.browser_fake().add_page("https://a.org/", FakePage::new("Alpha", &["alpha text"]));
        let retry = l.ops.iter().find_map(|o| if let iyakki_ui_sdk::list::Op::Hit(_, h) = o { (h.name == "Try again").then_some(h.action) } else { None }).unwrap();
        v.event(&UiEvent::Action(retry));
        v.load_url("a.org");
        assert!(draw(&mut v).texts().contains(&"alpha text"));
    }

    /// F-SDK-61 (UT2): the rest of the view's promises through the core: app content, back, forward, reload, stop, find, the engine choice and
    /// its plain failure, the size policies, a permission question answered with Allow, and the wheel and keys going to the page.
    #[test]
    fn history_app_content_find_engine_size_policy_and_a_permission_question_all_go_through_the_core() {
        use iyakki_core_api::browsersession::PromptKind;
        use iyakki_ui_sdk::browser::{Engine, SizePolicy};
        let core = core();
        core.browser_fake().add_page("https://a.org/cam", FakePage::new("Camera", &["a meeting"]).asking(PromptKind::Permission, "Use the camera?"));
        let mut v = view(&core);
        v.load_url("a.org");
        draw(&mut v);
        v.load_url("https://a.org/cam");
        let l = draw(&mut v);
        assert_eq!(v.title(), "Camera");
        assert!(v.can_back() && !v.can_forward());
        assert!(l.a11y().iter().any(|n| n.name == "Allow"), "the permission question has its bar");
        let allow = l.ops.iter().find_map(|o| if let iyakki_ui_sdk::list::Op::Hit(_, h) = o { (h.name == "Allow").then_some(h.action) } else { None }).unwrap();
        v.event(&UiEvent::Action(allow));
        assert_eq!(core.browser_fake().answers(), vec![("Use the camera?".to_string(), true)]);
        v.back();
        assert_eq!((v.title(), v.can_forward()), ("Alpha", true));
        v.forward();
        assert_eq!(v.title(), "Camera");
        v.reload();
        v.stop();
        let calls = core.browser_fake().calls();
        assert!(["reload", "stop"].iter().all(|w| calls.iter().any(|c| c == w)), "{calls:?}");
        // content the app brings is shown under its own base and is never fetched
        v.load_html("https://app.local/help", "made by the app");
        assert!(draw(&mut v).texts().iter().any(|t| t.contains("made by the app")));
        assert_eq!(v.url(), "https://app.local/help");
        // find in page
        v.load_url("a.org");
        draw(&mut v);
        let f = v.find("text", true);
        assert_eq!((f.matches, f.current), (1, 1));
        assert!(v.take_events().iter().any(|e| matches!(e, BrowserEvent::Find(f) if f.matches == 1)));
        // the wheel and the keys go to the page only while the view is focused
        v.set_focused(false);
        assert!(!v.event(&UiEvent::Wheel(-1)));
        v.set_focused(true);
        assert!(v.event(&UiEvent::Wheel(-1)));
        // the engine choice: the full engine that is not installed is said in plain words; auto falls back to the in-house one
        let mut full = BrowserView::new(Box::new(CoreBrowserPort::new(core.clone(), "alice", "sample")), 7_000).with_engine(Engine::Full);
        full.load_url("a.org");
        let l = draw(&mut full);
        assert!(l.texts().iter().any(|t| t.contains("not available") && t.contains("not installed")), "{:?}", l.texts());
        let mut auto = BrowserView::new(Box::new(CoreBrowserPort::new(core.clone(), "alice", "sample")), 8_000).with_engine(Engine::Auto);
        auto.load_url("a.org");
        draw(&mut auto);
        assert_eq!(auto.engine_used(), Some(Engine::InHouse));
        // the size policies: the page is drawn inside the part of the rectangle the policy takes
        let mut fixed = BrowserView::new(Box::new(CoreBrowserPort::new(core.clone(), "alice", "sample")), 9_000).with_size_policy(SizePolicy::Fixed(200.0, 100.0));
        fixed.load_url("a.org");
        let l = draw(&mut fixed);
        let link = l.ops.iter().find_map(|o| if let iyakki_ui_sdk::list::Op::Hit(r, h) = o { (h.name == "Zip").then_some(*r) } else { None }).unwrap();
        assert!(link.right() <= 210.0 && link.bottom() <= 110.0, "inside the fixed 200 by 100 at the area's corner: {link:?}");
        let sp = core.browser_fake().opened_specs().last().cloned().unwrap();
        assert_eq!((sp.width, sp.height), (200.0, 100.0), "the core was told the size the view uses");
        v.close();
        full.close();
        auto.close();
        fixed.close();
        assert_eq!(core.browser_fake().open_sessions(), 0);
    }
}
