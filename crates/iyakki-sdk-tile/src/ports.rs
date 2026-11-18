//! The core-backed ports: the SDK's `StoragePort` and `DockPort` over a tile's `call(domain, method, json)` (an
//! `iyakki_tile_kit::Source`). The app never sees the door; the host checks the tile manifest's capability first (`storage` and `dock`
//! must be granted), the core asks the person where the namespace says so and logs the call.
//!
//! * storage: the core's `storage` namespace, `get`/`set`/`remove`/`list` with the bridge's arguments (`{"k":..}`, `{"k":..,"v":..}`).
//!   The answer to `get` is the stored text or null; an object `{"value":..}` is read too.
//! * dock: `dock.pinned`, `dock.pin`, `dock.unpin`, `dock.open`, `dock.status` with `{"app":..}`. The browser tile host answers `pinned`,
//!   `pin` and `unpin` through the person's chat tools `display_dock` and `display_dock_set` (`iyakki-agent`, `webservice/tiledock.rs`;
//!   pin and unpin ask first). `open` and `status` have no answer yet (the framebuffer shell's act), and a host that does not know
//!   the domain refuses the call: the port answers [`DeskError::Unavailable`], which the app already handles (SAMPLE says "The dock did
//!   not allow that").
use iyakki_tile_kit::Source;
use iyakki_ui_sdk::desktop::{DeskError, DockPort, DockStatus, WallpaperPort};
use iyakki_ui_sdk::browser::{BrowserPort, Engine, FindInfo, Nav, PageInfo, PageInput, PageKey, PagePrompt, PageFamily, PageScheme, PromptKind, ViewSpec};
use iyakki_ui_sdk::files::{DirListing, EntryInfo, FilesError, FilesPort};
use iyakki_ui_sdk::wallpaper;
use iyakki_ui_sdk::ports::{check_key, PortError, StoragePort, MAX_VALUE};
use serde_json::{json, Value};
use std::cell::RefCell;
use std::rc::Rc;

/// One tile source shared by the ports of one app (a tile has a single door).
pub struct Shared<S: Source>(Rc<RefCell<S>>);

impl<S: Source> Clone for Shared<S> {
    fn clone(&self) -> Self {
        Shared(self.0.clone())
    }
}

impl<S: Source> Shared<S> {
    pub fn new(s: S) -> Shared<S> {
        Shared(Rc::new(RefCell::new(s)))
    }

    fn call(&self, domain: &str, method: &str, args: &Value) -> Result<Value, String> {
        let text = self.0.borrow_mut().call(domain, method, &args.to_string())?;
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|_| "the answer was not readable".to_string())
    }
}

pub struct CoreStorage<S: Source>(Shared<S>);

impl<S: Source> CoreStorage<S> {
    pub fn new(s: Shared<S>) -> CoreStorage<S> {
        CoreStorage(s)
    }
}

fn refusal(e: &str) -> PortError {
    if e.contains("QuotaExceeded") || e.contains("too big") {
        PortError::TooBig
    } else {
        PortError::Denied
    }
}

impl<S: Source> StoragePort for CoreStorage<S> {
    fn get(&mut self, key: &str) -> Option<String> {
        check_key(key).ok()?;
        match self.0.call("storage", "get", &json!({ "k": key })).ok()? {
            Value::String(s) => Some(s),
            Value::Object(o) => o.get("value").and_then(Value::as_str).map(str::to_string),
            _ => None,
        }
    }

    fn set(&mut self, key: &str, value: &str) -> Result<(), PortError> {
        check_key(key)?;
        if value.len() > MAX_VALUE {
            return Err(PortError::BadKey);
        }
        self.0.call("storage", "set", &json!({ "k": key, "v": value })).map(|_| ()).map_err(|e| refusal(&e))
    }

    fn remove(&mut self, key: &str) -> Result<(), PortError> {
        check_key(key)?;
        self.0.call("storage", "remove", &json!({ "k": key })).map(|_| ()).map_err(|e| refusal(&e))
    }

    fn keys(&mut self) -> Vec<String> {
        match self.0.call("storage", "list", &json!({})) {
            Ok(Value::Array(a)) => a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect(),
            _ => Vec::new(),
        }
    }
}

pub struct CoreDock<S: Source> {
    src: Shared<S>,
    /// The last answer to `pinned`, kept so a refused call does not make the pin button lie about what it did.
    known: RefCell<Vec<String>>,
}

impl<S: Source> CoreDock<S> {
    pub fn new(s: Shared<S>) -> CoreDock<S> {
        CoreDock { src: s, known: RefCell::new(Vec::new()) }
    }

    fn act(&self, method: &str, app: &str) -> Result<(), DeskError> {
        self.src.call("dock", method, &json!({ "app": app })).map(|_| ()).map_err(|e| {
            if e.contains("not allowed") || e.contains("denied") || e.contains("declined") {
                DeskError::Refused(e)
            } else {
                DeskError::Unavailable
            }
        })
    }
}

impl<S: Source> DockPort for CoreDock<S> {
    fn pinned(&self) -> Vec<String> {
        if let Ok(Value::Array(a)) = self.src.call("dock", "pinned", &json!({})) {
            *self.known.borrow_mut() = a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        }
        self.known.borrow().clone()
    }

    fn pin(&mut self, app: &str) -> Result<(), DeskError> {
        self.act("pin", app)
    }

    fn unpin(&mut self, app: &str) -> Result<(), DeskError> {
        self.act("unpin", app)
    }

    fn open(&mut self, app: &str) -> Result<(), DeskError> {
        self.act("open", app)
    }

    fn set_status(&mut self, app: &str, status: DockStatus) {
        let _ = self.src.call("dock", "status", &json!({ "app": app, "badge": status.badge, "line": status.line }));
    }
}

/// The wallpaper port over the tile's `call`: `wallpaper.get` answers the key of the person's wallpaper, `wallpaper.set {wallpaper}` changes it
/// (the host asks the person first). The screen number does not matter to the host (one wallpaper per person); a refusal is
/// [`DeskError::Refused`] when the host says declined or not allowed, [`DeskError::Unavailable`] otherwise (including "the person was asked").
pub struct CoreWallpaper<S: Source> {
    src: Shared<S>,
    known: std::cell::Cell<wallpaper::Id>,
}

impl<S: Source> CoreWallpaper<S> {
    pub fn new(s: Shared<S>) -> CoreWallpaper<S> {
        CoreWallpaper { src: s, known: std::cell::Cell::new(wallpaper::Id::Theme) }
    }
}

impl<S: Source> WallpaperPort for CoreWallpaper<S> {
    fn get(&self, _screen: u32) -> wallpaper::Id {
        if let Ok(Value::String(k)) = self.src.call("wallpaper", "get", &json!({})) {
            self.known.set(wallpaper::Id::from_key(&k));
        }
        self.known.get()
    }

    fn set(&mut self, _screen: u32, id: wallpaper::Id) -> Result<(), DeskError> {
        self.src.call("wallpaper", "set", &json!({ "wallpaper": id.key() })).map(|_| self.known.set(id)).map_err(|e| {
            if e.contains("not allowed") || e.contains("denied") || e.contains("declined") {
                DeskError::Refused(e)
            } else {
                DeskError::Unavailable
            }
        })
    }
}

#[cfg(test)]
mod wallpaper_tests {
    use super::*;
    use std::cell::RefCell as Rc2;
    use std::rc::Rc;

    /// A host that keeps one wallpaper key and refuses a key it does not know, like the person's own tools.
    struct Host {
        key: Rc<Rc2<String>>,
        mode: &'static str,
    }
    impl Source for Host {
        fn call(&mut self, domain: &str, method: &str, json: &str) -> Result<String, String> {
            assert_eq!(domain, "wallpaper");
            match (method, self.mode) {
                (_, "asked") => Err("the person was asked in their browser; try again".into()),
                (_, "declined") => Err("the person declined".into()),
                ("get", _) => Ok(serde_json::to_string(&*self.key.borrow()).unwrap()),
                ("set", _) => {
                    let v: Value = serde_json::from_str(json).unwrap();
                    *self.key.borrow_mut() = v["wallpaper"].as_str().unwrap().to_string();
                    Ok("{\"ok\":true}".into())
                }
                _ => Err("unknown".into()),
            }
        }
    }

    #[test]
    fn the_core_wallpaper_port_reads_and_sets_through_the_call_and_maps_a_refusal() {
        let key = Rc::new(Rc2::new("slate".to_string()));
        let mut p = CoreWallpaper::new(Shared::new(Host { key: key.clone(), mode: "ok" }));
        assert_eq!(p.get(1), wallpaper::Id::Slate);
        p.set(1, wallpaper::Id::Dusk).unwrap();
        assert_eq!(&*key.borrow(), "dusk");
        assert_eq!(p.get(1), wallpaper::Id::Dusk);
        let mut asked = CoreWallpaper::new(Shared::new(Host { key: key.clone(), mode: "asked" }));
        assert_eq!(asked.set(1, wallpaper::Id::Meadow), Err(DeskError::Unavailable));
        assert_eq!(asked.get(1), wallpaper::Id::Theme, "no answer: the last known one");
        let mut no = CoreWallpaper::new(Shared::new(Host { key, mode: "declined" }));
        assert!(matches!(no.set(1, wallpaper::Id::Meadow), Err(DeskError::Refused(_))));
    }
}

/// The files port over the tile's `call`: `home.list {dir}` and `home.read {path}`, answered by the host from the core's files facet for the
/// signed-in person (their home folder only). Read only: a change would need the person's yes from the page, so `change` and `decide` keep the
/// port's default and say the browser is not available for them.
pub struct CoreFiles<S: Source> {
    src: Shared<S>,
}

impl<S: Source> CoreFiles<S> {
    pub fn new(s: Shared<S>) -> CoreFiles<S> {
        CoreFiles { src: s }
    }

    fn call(&self, method: &str, args: &Value) -> Result<Value, FilesError> {
        self.src.call("home", method, args).map_err(|e| if e.contains("unknown service") || e.contains("no file browser") { FilesError::Unavailable } else { FilesError::Refused(e) })
    }
}

impl<S: Source> FilesPort for CoreFiles<S> {
    fn list(&mut self, dir: &str) -> Result<DirListing, FilesError> {
        let v = self.call("list", &json!({ "dir": dir }))?;
        let entries = v["entries"].as_array().map(|a| a.iter().filter_map(|e| Some(EntryInfo { name: e["name"].as_str()?.to_string(), is_dir: e["is_dir"].as_bool()?, size: e["size"].as_u64().unwrap_or(0) })).collect()).unwrap_or_default();
        Ok(DirListing { dir: v["dir"].as_str().unwrap_or(dir).to_string(), entries, truncated: v["truncated"].as_bool().unwrap_or(false) })
    }

    fn read_text(&mut self, path: &str, _max: usize) -> Result<String, FilesError> {
        self.call("read", &json!({ "path": path }))?["text"].as_str().map(str::to_string).ok_or_else(|| FilesError::Refused("the answer was not readable".into()))
    }
}

#[cfg(test)]
mod files_tests {
    use super::*;

    struct Home;
    impl Source for Home {
        fn call(&mut self, domain: &str, method: &str, json: &str) -> Result<String, String> {
            assert_eq!(domain, "home");
            let a: Value = serde_json::from_str(json).unwrap();
            match (method, a["dir"].as_str(), a["path"].as_str()) {
                ("list", Some(""), _) => Ok(r#"{"dir":"","truncated":false,"entries":[{"name":"Docs","is_dir":true,"size":0},{"name":"a.txt","is_dir":false,"size":3}]}"#.into()),
                ("list", Some(_), _) => Err("that is not there".into()),
                ("read", _, Some("a.txt")) => Ok(r#"{"text":"abc"}"#.into()),
                _ => Err("that file cannot be read".into()),
            }
        }
    }

    struct Off;
    impl Source for Off {
        fn call(&mut self, _: &str, _: &str, _: &str) -> Result<String, String> {
            Err("unknown service".into())
        }
    }

    #[test]
    fn the_tile_files_port_lists_and_reads_through_the_home_domain_and_maps_a_refusal_and_a_missing_host() {
        let mut p = CoreFiles::new(Shared::new(Home));
        let l = p.list("").unwrap();
        assert_eq!(l.entries.iter().map(|e| (e.name.as_str(), e.is_dir, e.size)).collect::<Vec<_>>(), [("Docs", true, 0), ("a.txt", false, 3)]);
        assert_eq!(p.read_text("a.txt", 100).unwrap(), "abc");
        assert_eq!(p.list("x"), Err(FilesError::Refused("that is not there".into())));
        assert!(matches!(p.read_text("b.txt", 100), Err(FilesError::Refused(_))));
        assert!(matches!(p.change(&iyakki_ui_sdk::files::Change::Trash { path: "a.txt".into() }), Err(FilesError::Unavailable)), "a tile asks for no change");
        let mut off = CoreFiles::new(Shared::new(Off));
        assert_eq!(off.list(""), Err(FilesError::Unavailable));
    }

    #[test]
    fn a_files_view_in_a_tile_shows_the_home_folder_from_the_host() {
        let mut v = iyakki_ui_sdk::filesview::FilesView::new(Box::new(CoreFiles::new(Shared::new(Home))), 0);
        v.tick();
        assert_eq!(v.names(), ["Docs/", "a.txt"]);
        v.key(iyakki_ui_sdk::event::UiKey::Down);
        v.key(iyakki_ui_sdk::event::UiKey::Enter);
        assert_eq!(v.preview().map(|p| p.1.as_str()), Some("abc"));
    }
}

/// The embedded browser over the tile's `call` (`browser.open`, `navigate`, `input`, `find`, `state`, `frame`, `prompts`, `answer`): the host maps them
/// onto the core's browser sessions for this tile. A refusal is a sentence. `answer` with `allow` true is the person's yes, which the host takes from
/// their page (an approval card), not from the tile: the first call is told "the person was asked", and the same call is made again after they answer.
pub struct CoreBrowser<S: Source> {
    src: Shared<S>,
}

impl<S: Source> CoreBrowser<S> {
    pub fn new(s: Shared<S>) -> CoreBrowser<S> {
        CoreBrowser { src: s }
    }

    fn call(&self, method: &str, args: &Value) -> Result<Value, String> {
        self.src.call("browser", method, args)
    }
}

fn engine_key(e: Engine) -> &'static str {
    match e {
        Engine::InHouse => "in-house",
        Engine::Full => "full",
        Engine::Auto => "auto",
    }
}

fn engine_of(k: &str) -> Engine {
    match k {
        "full" => Engine::Full,
        "auto" => Engine::Auto,
        _ => Engine::InHouse,
    }
}

impl<S: Source> BrowserPort for CoreBrowser<S> {
    fn open(&mut self, s: &ViewSpec) -> Result<u64, String> {
        let p = s.palette;
        let v = self.call("open", &json!({
            "width": s.width, "height": s.height, "text_scale": s.text_scale, "engine": engine_key(s.engine),
            "scheme": match s.scheme { PageScheme::Dark => "dark", PageScheme::Light => "light", PageScheme::HighContrast => "high-contrast" },
            "family": match s.family { PageFamily::Sans => "sans", PageFamily::Mono => "mono" },
            "font_size": s.font_size, "reduced_motion": s.reduced_motion,
            "palette": {"bg": p.bg, "text": p.text, "muted": p.muted, "link": p.link, "rule": p.rule, "code_bg": p.code_bg, "mark": p.mark},
        }))?;
        v["id"].as_u64().ok_or_else(|| "the answer was not readable".to_string())
    }

    fn close(&mut self, id: u64) {
        let _ = self.call("close", &json!({ "id": id }));
    }

    fn navigate(&mut self, id: u64, nav: &Nav) -> Result<(), String> {
        let a = match nav {
            Nav::Url(u) => json!({"id": id, "kind": "url", "url": u}),
            Nav::Html { base, html } => json!({"id": id, "kind": "html", "base": base, "html": html}),
            Nav::Back => json!({"id": id, "kind": "back"}),
            Nav::Forward => json!({"id": id, "kind": "forward"}),
            Nav::Reload => json!({"id": id, "kind": "reload"}),
            Nav::Stop => json!({"id": id, "kind": "stop"}),
        };
        self.call("navigate", &a).map(|_| ())
    }

    fn input(&mut self, id: u64, input: &PageInput) -> Result<(), String> {
        let a = match input {
            PageInput::Activate(n) => json!({"id": id, "kind": "activate", "n": n}),
            PageInput::Wheel(n) => json!({"id": id, "kind": "wheel", "n": n}),
            PageInput::Key(k) => json!({"id": id, "kind": "key", "key": match k { PageKey::Up => "up", PageKey::Down => "down", PageKey::PageUp => "page_up", PageKey::PageDown => "page_down", PageKey::Home => "home", PageKey::End => "end", PageKey::Enter => "enter", PageKey::Esc => "esc" }}),
            PageInput::Text(t) => json!({"id": id, "kind": "text", "text": t}),
            PageInput::Resize { width, height } => json!({"id": id, "kind": "resize", "width": width, "height": height}),
        };
        self.call("input", &a).map(|_| ())
    }

    fn find(&mut self, id: u64, query: &str, forward: bool) -> Result<FindInfo, String> {
        let v = self.call("find", &json!({"id": id, "query": query, "forward": forward}))?;
        Ok(FindInfo { query: v["query"].as_str().unwrap_or("").to_string(), matches: v["matches"].as_u64().unwrap_or(0) as usize, current: v["current"].as_u64().unwrap_or(0) as usize })
    }

    fn state(&mut self, id: u64) -> Result<PageInfo, String> {
        let v = self.call("state", &json!({ "id": id }))?;
        Ok(PageInfo {
            url: v["url"].as_str().unwrap_or("").to_string(),
            title: v["title"].as_str().unwrap_or("").to_string(),
            loading: v["loading"].as_bool().unwrap_or(false),
            can_back: v["can_back"].as_bool().unwrap_or(false),
            can_forward: v["can_forward"].as_bool().unwrap_or(false),
            error: v["error"].as_str().map(str::to_string),
            engine: engine_of(v["engine"].as_str().unwrap_or("")),
            find: FindInfo { query: v["find"]["query"].as_str().unwrap_or("").to_string(), matches: v["find"]["matches"].as_u64().unwrap_or(0) as usize, current: v["find"]["current"].as_u64().unwrap_or(0) as usize },
            generation: v["generation"].as_u64().unwrap_or(0),
            scroll_permille: v["scroll_permille"].as_u64().unwrap_or(0).min(1000) as u16,
        })
    }

    fn frame(&mut self, id: u64, since: u64) -> Result<Option<(u64, String)>, String> {
        let v = self.call("frame", &json!({"id": id, "since": since}))?;
        Ok(v["list"].as_str().map(|l| (v["generation"].as_u64().unwrap_or(0), l.to_string())))
    }

    fn prompts(&mut self, id: u64) -> Vec<PagePrompt> {
        match self.call("prompts", &json!({ "id": id })) {
            Ok(Value::Array(a)) => a
                .iter()
                .filter_map(|p| Some(PagePrompt { id: p["id"].as_u64()?, kind: if p["kind"] == "download" { PromptKind::Download } else { PromptKind::Permission }, origin: p["origin"].as_str()?.to_string(), text: p["text"].as_str()?.to_string() }))
                .collect(),
            _ => Vec::new(),
        }
    }

    fn answer(&mut self, id: u64, prompt: u64, allow: bool) -> Result<(), String> {
        self.call("answer", &json!({"id": id, "prompt": prompt, "allow": allow})).map(|_| ())
    }
}

#[cfg(test)]
mod browser_tests {
    use super::*;
    use iyakki_ui_sdk::browser::{PagePalette, ViewSpec};

    /// A host that records what the tile asked and answers like the real one.
    #[derive(Default)]
    struct Host {
        log: Vec<String>,
        asked: bool,
    }

    impl Source for Host {
        fn call(&mut self, domain: &str, method: &str, json: &str) -> Result<String, String> {
            assert_eq!(domain, "browser");
            self.log.push(format!("{method} {json}"));
            let a: Value = serde_json::from_str(json).unwrap();
            match method {
                "open" => Ok(r#"{"id":4}"#.into()),
                "navigate" | "input" | "close" => Ok(r#"{"ok":true}"#.into()),
                "find" => Ok(r#"{"query":"cat","matches":3,"current":1}"#.into()),
                "state" => Ok(r#"{"url":"https://example.org/","title":"Example","loading":false,"can_back":true,"can_forward":false,"error":null,"engine":"full","find":{"query":"","matches":0,"current":0},"generation":9,"scroll_permille":250}"#.into()),
                "frame" if a["since"] == 9 => Ok("null".into()),
                "frame" => Ok(r#"{"generation":9,"list":"IYAKKI-DL 1"}"#.into()),
                "prompts" => Ok(r#"[{"id":7,"kind":"download","origin":"example.org","text":"save a.zip"}]"#.into()),
                "answer" if a["allow"] == true && !self.asked => {
                    self.asked = true;
                    Err("the person was asked in their page; try the same call again after they answer".into())
                }
                "answer" => Ok(r#"{"ok":true}"#.into()),
                _ => Err("that is not a call this tile may make".into()),
            }
        }
    }

    fn spec() -> ViewSpec {
        ViewSpec { scheme: PageScheme::Light, font_size: 16.0, family: PageFamily::Mono, reduced_motion: true, width: 320.0, height: 200.0, text_scale: 1.5, engine: Engine::Full, palette: PagePalette { bg: [1, 2, 3, 255], text: [4, 5, 6, 255], muted: [7, 8, 9, 255], link: [10, 11, 12, 255], rule: [13, 14, 15, 255], code_bg: [16, 17, 18, 255], mark: [19, 20, 21, 255] } }
    }

    #[test]
    fn every_browser_call_of_the_view_reaches_the_host_in_the_wire_form_and_the_answers_are_read_back() {
        let shared = Shared::new(Host::default());
        let mut p = CoreBrowser::new(shared.clone());
        assert_eq!(p.open(&spec()).unwrap(), 4);
        p.navigate(4, &Nav::Url("https://example.org/".into())).unwrap();
        p.navigate(4, &Nav::Back).unwrap();
        p.input(4, &PageInput::Key(PageKey::PageDown)).unwrap();
        p.input(4, &PageInput::Resize { width: 100.0, height: 80.0 }).unwrap();
        assert_eq!(p.find(4, "cat", true).unwrap(), FindInfo { query: "cat".into(), matches: 3, current: 1 });
        let st = p.state(4).unwrap();
        assert_eq!((st.title.as_str(), st.engine, st.can_back, st.generation, st.scroll_permille), ("Example", Engine::Full, true, 9, 250));
        assert_eq!(p.frame(4, 0).unwrap(), Some((9, "IYAKKI-DL 1".to_string())));
        assert_eq!(p.frame(4, 9).unwrap(), None, "nothing new");
        assert_eq!(p.prompts(4), vec![PagePrompt { id: 7, kind: PromptKind::Download, origin: "example.org".into(), text: "save a.zip".into() }]);
        p.close(4);
        let log = shared.0.borrow().log.join("\n");
        assert!(log.contains("\"scheme\":\"light\"") && log.contains("\"engine\":\"full\"") && log.contains("\"family\":\"mono\"") && log.contains("\"bg\":[1,2,3,255]"), "{log}");
        assert!(log.contains("\"kind\":\"url\"") && log.contains("\"key\":\"page_down\"") && log.contains("close"), "{log}");
    }

    #[test]
    fn a_yes_to_a_prompt_is_told_the_person_was_asked_and_goes_through_when_the_same_call_is_made_again() {
        let mut p = CoreBrowser::new(Shared::new(Host::default()));
        assert!(p.answer(4, 7, false).is_ok(), "a no needs nobody");
        let first = p.answer(4, 7, true).unwrap_err();
        assert!(first.contains("the person was asked in their page"), "{first}");
        assert!(p.answer(4, 7, true).is_ok(), "after they answered in the page");
    }
}
