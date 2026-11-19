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
use iyakki_ui_sdk::assistant::{AssistError, AssistResult, AssistantPort, ChatEvent, Choice, ContextItem, ContextKind, HistoryLine, SessionConfig, Speaker};
use iyakki_ui_sdk::canvas::{Image, ImageFormat};
use iyakki_ui_sdk::cameras::{CamAction, CamEvent, CamInfo, CamResult, CamStatus, CameraPort, Mark, Overlay, Phase, Ptz, Shape, ViewState};
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

/// The files port over the tile's `call`: `home.list {dir}`, `home.read {path}` and `home.change {kind, ...}`, answered by the host from the core's files
/// facet for the signed-in person (their home folder only). A change is the person's to allow, and the tile cannot allow it: `change` makes the
/// host ask in the person's page, and the question the view shows says so; `decide` with a yes asks the host again, which makes the change only if
/// the person said yes in the page, and says "waiting" otherwise (the view keeps its question). A no from the tile just drops the tile's question:
/// the person's own card in the page is theirs to answer.
pub struct CoreFiles<S: Source> {
    src: Shared<S>,
    /// The question the view shows: its call, the change, and what the host already answered when the person's yes was there before.
    waiting: Option<(String, iyakki_ui_sdk::files::Change, Option<String>)>,
    n: u32,
}

/// The host's sentence for a change that waits for the person in the page.
const ASKED_IN_PAGE: &str = "asked in their page";

fn change_args(c: &iyakki_ui_sdk::files::Change) -> Value {
    use iyakki_ui_sdk::files::Change as C;
    match c {
        C::MakeDir { dir, name } => json!({"kind": "make_dir", "dir": dir, "name": name}),
        C::Rename { path, new_name } => json!({"kind": "rename", "path": path, "new_name": new_name}),
        C::Trash { path } => json!({"kind": "trash", "path": path}),
        C::Save { path, text } => json!({"kind": "save", "path": path, "text": text}),
    }
}

fn change_words(c: &iyakki_ui_sdk::files::Change) -> String {
    use iyakki_ui_sdk::files::Change as C;
    match c {
        C::MakeDir { dir, name } => format!("Make the folder `{name}` in {}", if dir.is_empty() { "your home folder".to_string() } else { format!("`{dir}`") }),
        C::Rename { path, new_name } => format!("Rename `{path}` to `{new_name}`"),
        C::Trash { path } => format!("Move `{path}` to the trash"),
        C::Save { path, text } => format!("Save `{path}` ({} bytes), replacing it if it is there", text.len()),
    }
}

impl<S: Source> CoreFiles<S> {
    pub fn new(s: Shared<S>) -> CoreFiles<S> {
        CoreFiles { src: s, waiting: None, n: 0 }
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

    fn change(&mut self, c: &iyakki_ui_sdk::files::Change) -> Result<iyakki_ui_sdk::files::Ask, FilesError> {
        // the host puts the person's own question in their page now; this tile's question only says that it waits there
        match self.call("change", &change_args(c)) {
            Err(FilesError::Refused(e)) if e.contains(ASKED_IN_PAGE) => {
                self.n += 1;
                let call = format!("page{}", self.n);
                self.waiting = Some((call.clone(), c.clone(), None));
                Ok(iyakki_ui_sdk::files::Ask { call, summary: format!("{} (you are asked in the page too: answer there)", change_words(c)) })
            }
            // a yes that was already given for exactly this change (the person answered before the tile asked again): it was made
            Ok(v) => {
                self.n += 1;
                let call = format!("done{}", self.n);
                self.waiting = Some((call.clone(), c.clone(), Some(v["done"].as_str().unwrap_or("Done.").to_string())));
                Ok(iyakki_ui_sdk::files::Ask { call, summary: format!("{} (already allowed in the page)", change_words(c)) })
            }
            Err(e) => Err(e),
        }
    }

    fn decide(&mut self, call: &str, approve: bool) -> Result<String, FilesError> {
        let Some((_, c, made)) = self.waiting.clone().filter(|(id, _, _)| id == call) else { return Err(FilesError::Refused("there is no such question waiting".into())) };
        if let Some(done) = made {
            // the person's yes was already there and the host made the change when it was asked: no second question
            self.waiting = None;
            return Ok(done);
        }
        if !approve {
            self.waiting = None;
            return Ok("Not changed here: answer the question in the page too.".into());
        }
        match self.call("change", &change_args(&c)) {
            Ok(v) => {
                self.waiting = None;
                Ok(v["done"].as_str().unwrap_or("Done.").to_string())
            }
            Err(FilesError::Refused(e)) if e.contains(ASKED_IN_PAGE) => {
                Err(FilesError::Waiting("Waiting for your answer in the page.".into()))
            }
            Err(e) => {
                self.waiting = None;
                Err(e)
            }
        }
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
        let mut off = CoreFiles::new(Shared::new(Off));
        assert_eq!(off.list(""), Err(FilesError::Unavailable));
        assert!(matches!(off.change(&iyakki_ui_sdk::files::Change::Trash { path: "a.txt".into() }), Err(FilesError::Unavailable)), "a host with no file browser changes nothing");
    }

    /// A host that asks the person in their page: it says "asked in their page" until they answered, then makes the change once.
    struct Page {
        answered: Option<bool>,
        made: Vec<String>,
        asked: u32,
    }
    #[derive(Clone)]
    struct PageHost(std::rc::Rc<std::cell::RefCell<Page>>);
    impl Source for PageHost {
        fn call(&mut self, domain: &str, method: &str, json: &str) -> Result<String, String> {
            assert_eq!((domain, method), ("home", "change"));
            let mut p = self.0.borrow_mut();
            match p.answered {
                None => {
                    p.asked += 1;
                    Err("the person was asked in their page; try the same call again after they answer".into())
                }
                Some(false) => Err("the person said no".into()),
                Some(true) => {
                    p.made.push(json.to_string());
                    p.answered = None;
                    Ok(r#"{"done":"Saved."}"#.into())
                }
            }
        }
    }

    #[test]
    fn a_tile_change_waits_for_the_person_in_the_page_and_is_made_by_the_host_only_after_their_yes() {
        use iyakki_ui_sdk::files::Change;
        let host = PageHost(std::rc::Rc::new(std::cell::RefCell::new(Page { answered: None, made: vec![], asked: 0 })));
        let mut p = CoreFiles::new(Shared::new(host.clone()));
        let c = Change::Save { path: "a.txt".into(), text: "hi".into() };
        let ask = p.change(&c).unwrap();
        assert!(ask.summary.contains("Save `a.txt` (2 bytes)") && ask.summary.contains("answer there"), "{ask:?}");
        assert_eq!(host.0.borrow().asked, 1);
        // the tile's own yes counts for nothing: the host still says it waits, and the port says so in a state of its own
        assert_eq!(p.decide(&ask.call, true), Err(FilesError::Waiting("Waiting for your answer in the page.".into())));
        assert!(host.0.borrow().made.is_empty());
        // the person answers yes in the page: the next look makes it, once
        host.0.borrow_mut().answered = Some(true);
        assert_eq!(p.decide(&ask.call, true).unwrap(), "Saved.");
        assert_eq!(host.0.borrow().made.len(), 1);
        assert!(host.0.borrow().made[0].contains(r#""kind":"save""#));
        assert!(p.decide(&ask.call, true).is_err(), "answered once");
        // a no in the page is a refusal with the person's words; a no from the tile only drops its own question
        let mut p2 = CoreFiles::new(Shared::new(host.clone()));
        let a2 = p2.change(&c).unwrap();
        host.0.borrow_mut().answered = Some(false);
        assert_eq!(p2.decide(&a2.call, true), Err(FilesError::Refused("the person said no".into())));
        host.0.borrow_mut().answered = None;
        let a3 = p2.change(&c).unwrap();
        assert!(p2.decide(&a3.call, false).unwrap().contains("Not changed"));
        // the view keeps its question while the page waits
        let mut v = iyakki_ui_sdk::filesview::FilesView::new(Box::new(CoreFiles::new(Shared::new(host.clone()))), 0);
        assert!(v.save("b.txt", "x"));
        assert!(v.pending_question().is_some());
        v.decide(true);
        assert!(v.pending_question().is_some(), "still waiting in the page");
        assert!(v.note().is_some_and(|n| n.contains("Waiting")), "{:?}", v.note());
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

/// The embedded assistant over the tile's `call` (`assistant.open`, `send`, `poll`, `decide`, `answer`, `cancel`, `history`, `choose`, `models`,
/// `roles`, `set_context`, `close`): the host maps them onto the core's assistant for this tile's id and only the sessions it opened. One port is
/// one session; the host's session id is kept here. A yes to a tool's card is the person's: the first `decide` with `approve` true is told "the
/// person was asked in their page", and the same call is made again after they answer there.
pub struct CoreAssistant<S: Source> {
    src: Shared<S>,
    session: Option<String>,
}

impl<S: Source> CoreAssistant<S> {
    pub fn new(s: Shared<S>) -> CoreAssistant<S> {
        CoreAssistant { src: s, session: None }
    }

    fn call(&self, method: &str, args: &Value) -> AssistResult<Value> {
        self.src.call("assistant", method, args).map_err(AssistError::Refused)
    }

    fn with_session(&self, method: &str, mut args: Value) -> AssistResult<Value> {
        let id = self.session.clone().ok_or_else(|| AssistError::Refused("the conversation is not open".into()))?;
        args["session"] = json!(id);
        self.call(method, &args)
    }
}

fn kind_key(k: ContextKind) -> &'static str {
    match k {
        ContextKind::Document => "document",
        ContextKind::Form => "form",
        ContextKind::Selection => "selection",
        ContextKind::Other => "other",
    }
}

fn context_json(ctx: &[ContextItem]) -> Value {
    Value::Array(ctx.iter().map(|c| json!({"kind": kind_key(c.kind), "name": c.name, "text": c.text})).collect())
}

fn choices_of(v: Value) -> Vec<Choice> {
    v.as_array().map(|a| a.iter().filter_map(|c| Some(Choice { id: c["id"].as_str()?.to_string(), label: c["label"].as_str()?.to_string() })).collect()).unwrap_or_default()
}

fn event_of(e: &Value) -> Option<ChatEvent> {
    let t = |k: &str| e[k].as_str().map(str::to_string);
    Some(match e["type"].as_str()? {
        "partial" => ChatEvent::Partial(t("text")?),
        "note" => ChatEvent::Note(t("text")?),
        "tool_request" => ChatEvent::ToolRequest { call: t("call")?, tool: t("tool")?, args: t("args")? },
        "approval" => ChatEvent::Approval { call: t("call")?, tool: t("tool")?, summary: t("summary")?, detail: t("detail").unwrap_or_default() },
        "decision" => ChatEvent::Decision { call: t("call")?, tool: t("tool")?, approved: e["approved"].as_bool()?, by: t("by").unwrap_or_default() },
        "tool_call" => ChatEvent::ToolCall { call: t("call")?, tool: t("tool")?, args: t("args")? },
        "tool_result" => ChatEvent::ToolResult { call: t("call")?, tool: t("tool")?, ok: e["ok"].as_bool()?, text: t("text").unwrap_or_default() },
        "final" => ChatEvent::Final(t("text")?),
        "failed" => ChatEvent::Failed(t("error")?),
        "stopped" => ChatEvent::Stopped(t("reason")?),
        _ => return None,
    })
}

impl<S: Source> AssistantPort for CoreAssistant<S> {
    fn models(&mut self) -> Vec<Choice> {
        self.call("models", &json!({})).map(choices_of).unwrap_or_default()
    }

    fn roles(&mut self) -> Vec<Choice> {
        self.call("roles", &json!({})).map(choices_of).unwrap_or_default()
    }

    fn open(&mut self, cfg: &SessionConfig) -> AssistResult<bool> {
        let tools: Vec<Value> = cfg.tools.iter().map(|t| json!({"name": t.name, "description": t.description, "schema": t.schema, "approval": if t.ask { "ask" } else { "safe" }})).collect();
        let v = self.call("open", &json!({"conversation": cfg.conversation, "title": cfg.title, "tools": tools, "context": context_json(&cfg.context), "model": cfg.model, "role": cfg.role}))?;
        self.session = Some(v["id"].as_str().ok_or_else(|| AssistError::Refused("the answer was not readable".into()))?.to_string());
        Ok(v["resumed"].as_bool().unwrap_or(false))
    }

    fn set_context(&mut self, ctx: &[ContextItem]) -> AssistResult<()> {
        self.with_session("set_context", json!({"context": context_json(ctx)})).map(|_| ())
    }

    fn choose(&mut self, model: Option<&str>, role: Option<&str>) -> AssistResult<()> {
        self.with_session("choose", json!({"model": model, "role": role})).map(|_| ())
    }

    fn send(&mut self, text: &str) -> AssistResult<()> {
        self.with_session("send", json!({"text": text})).map(|_| ())
    }

    fn poll(&mut self) -> Vec<ChatEvent> {
        match self.with_session("poll", json!({})) {
            Ok(Value::Array(a)) => a.iter().filter_map(event_of).collect(),
            _ => Vec::new(),
        }
    }

    fn decide(&mut self, call: &str, approve: bool) -> AssistResult<()> {
        self.with_session("decide", json!({"call": call, "approve": approve})).map(|_| ())
    }

    fn answer(&mut self, call: &str, ok: bool, text: &str) -> AssistResult<()> {
        self.with_session("answer", json!({"call": call, "ok": ok, "text": text})).map(|_| ())
    }

    fn cancel(&mut self) -> AssistResult<()> {
        self.with_session("cancel", json!({})).map(|_| ())
    }

    fn history(&mut self) -> Vec<HistoryLine> {
        match self.with_session("history", json!({})) {
            Ok(Value::Array(a)) => a
                .iter()
                .filter_map(|h| Some(HistoryLine { who: match h["role"].as_str()? { "user" => Speaker::User, "assistant" => Speaker::Assistant, _ => Speaker::Tool }, text: h["text"].as_str()?.to_string() }))
                .collect(),
            _ => Vec::new(),
        }
    }

    fn close(&mut self) {
        if self.with_session("close", json!({})).is_ok() {
            self.session = None;
        }
    }
}

#[cfg(test)]
mod assistant_tests {
    use super::*;
    use iyakki_ui_sdk::assistant::ToolSpec;

    #[derive(Default)]
    struct Host {
        log: Vec<String>,
        asked: bool,
    }

    impl Source for Host {
        fn call(&mut self, domain: &str, method: &str, json: &str) -> Result<String, String> {
            assert_eq!(domain, "assistant");
            self.log.push(format!("{method} {json}"));
            let a: Value = serde_json::from_str(json).unwrap();
            if method != "open" && method != "models" && method != "roles" {
                assert_eq!(a["session"], "as-3", "{method}");
            }
            match method {
                "models" => Ok(r#"[{"id":"default","label":"Settings model"}]"#.into()),
                "roles" => Ok(r#"[{"id":"concise","label":"Concise"}]"#.into()),
                "open" => Ok(r#"{"id":"as-3","resumed":true}"#.into()),
                "poll" => Ok(r#"[{"type":"partial","text":"Adding"},{"type":"approval","call":"c1","tool":"add_note","summary":"Add a note","detail":""},{"type":"tool_call","call":"c1","tool":"add_note","args":"{}"},{"type":"final","text":"Done"},{"type":"odd"}]"#.into()),
                "history" => Ok(r#"[{"role":"user","text":"hi"},{"role":"assistant","text":"hello"},{"role":"tool","text":"ok"}]"#.into()),
                "decide" if a["approve"] == true && !self.asked => {
                    self.asked = true;
                    Err("the person was asked in their page; try the same call again after they answer".into())
                }
                _ => Ok(r#"{"ok":true}"#.into()),
            }
        }
    }

    fn cfg() -> SessionConfig {
        SessionConfig { conversation: "main".into(), title: "Notes".into(), tools: vec![ToolSpec::new("add_note", "adds a note", "{\"type\":\"object\"}", true), ToolSpec::new("list_notes", "lists", "{}", false)], context: vec![ContextItem::new(ContextKind::Document, "doc", "text")], model: None, role: Some("concise".into()) }
    }

    #[test]
    fn every_call_of_the_chat_reaches_the_host_in_the_wire_form_and_the_events_are_read_back() {
        let shared = Shared::new(Host::default());
        let mut p = CoreAssistant::new(shared.clone());
        assert!(p.send("hi").is_err(), "no session before open");
        assert_eq!(p.models(), vec![Choice { id: "default".into(), label: "Settings model".into() }]);
        assert_eq!(p.roles().len(), 1);
        assert!(p.open(&cfg()).unwrap(), "resumed");
        p.set_context(&[ContextItem::new(ContextKind::Form, "f", "t")]).unwrap();
        p.choose(Some("default"), None).unwrap();
        p.send("add a note").unwrap();
        let ev = p.poll();
        assert_eq!(ev.len(), 4, "the unknown kind is dropped: {ev:?}");
        assert!(matches!(&ev[1], ChatEvent::Approval { call, tool, .. } if call == "c1" && tool == "add_note"));
        assert!(matches!(&ev[2], ChatEvent::ToolCall { .. }) && ev[3] == ChatEvent::Final("Done".into()));
        p.answer("c1", true, "added").unwrap();
        p.cancel().unwrap();
        assert_eq!(p.history().iter().map(|h| h.who).collect::<Vec<_>>(), [Speaker::User, Speaker::Assistant, Speaker::Tool]);
        p.close();
        assert!(p.send("again").is_err(), "closed");
        let log = shared.0.borrow().log.join("\n");
        assert!(log.contains("\"approval\":\"ask\"") && log.contains("\"approval\":\"safe\"") && log.contains("\"kind\":\"document\"") && log.contains("\"role\":\"concise\""), "{log}");
        assert!(log.contains("\"kind\":\"form\"") && log.contains("\"text\":\"add a note\""), "{log}");
    }

    #[test]
    fn a_yes_to_a_card_is_told_the_person_was_asked_and_goes_through_when_the_same_call_is_made_again() {
        let mut p = CoreAssistant::new(Shared::new(Host::default()));
        p.open(&cfg()).unwrap();
        assert!(p.decide("c1", false).is_ok(), "a no needs nobody");
        let first = p.decide("c1", true).unwrap_err();
        assert!(first.sentence().contains("the person was asked in their page"), "{}", first.sentence());
        assert!(p.decide("c1", true).is_ok());
    }
}

/// Camera views over the tile's `call` (`cameras.list`, `open`, `decide`, `state`, `set_visible`, `frame`, `overlay`, `snapshot`, `ptz`, `events`,
/// `close`): the host maps them onto the core's camera facet for this tile's id and only the view it opened. One port is one view. A yes to a
/// consent card is the person's: the first `decide` with `approve` true is told "the person was asked in their page", and the same call is
/// made again after they answer there. A picture comes as base64 inside the answer (JPEG unless the view asks for RGBA).
pub struct CoreCameras<S: Source> {
    src: Shared<S>,
    view: Option<String>,
}

impl<S: Source> CoreCameras<S> {
    pub fn new(s: Shared<S>) -> CoreCameras<S> {
        CoreCameras { src: s, view: None }
    }

    fn call(&self, method: &str, args: &Value) -> CamResult<Value> {
        self.src.call("cameras", method, args)
    }

    fn with_view(&self, method: &str, mut args: Value) -> CamResult<Value> {
        let id = self.view.clone().ok_or_else(|| "No camera view is open.".to_string())?;
        args["view"] = json!(id);
        self.call(method, &args)
    }
}

fn cam_status(k: &str) -> CamStatus {
    match k {
        "live" => CamStatus::Live,
        "lost" => CamStatus::Lost,
        "needs_addon" => CamStatus::NeedsAddon,
        "busy" => CamStatus::Busy,
        "needs_password" => CamStatus::NeedsPassword,
        "dark" => CamStatus::Dark,
        _ => CamStatus::Connecting,
    }
}

fn cam_action(k: &str) -> CamAction {
    match k {
        "snapshot" => CamAction::Snapshot,
        "ptz" => CamAction::Ptz,
        _ => CamAction::View,
    }
}

fn cam_info(c: &Value) -> Option<CamInfo> {
    Some(CamInfo {
        id: c["id"].as_str()?.to_string(),
        name: c["name"].as_str()?.to_string(),
        status: cam_status(c["status"].as_str().unwrap_or("")),
        detail: c["detail"].as_str().unwrap_or("").to_string(),
        ptz: c["ptz"].as_bool().unwrap_or(false),
        recording: c["recording"].as_bool().unwrap_or(false),
        motion_watch: c["motion_watch"].as_bool().unwrap_or(false),
        size: (c["size"][0].as_u64().unwrap_or(0) as u32, c["size"][1].as_u64().unwrap_or(0) as u32),
    })
}

fn cam_list(v: &Value) -> Vec<CamInfo> {
    v.as_array().map(|a| a.iter().filter_map(cam_info).collect()).unwrap_or_default()
}

fn cam_mark(m: &Value) -> Option<Mark> {
    let b = &m["bbox"];
    Some(Mark { id: m["id"].as_u64()?, at_ms: m["at_ms"].as_u64()?, zone: m["zone"].as_str()?.to_string(), score: m["score"].as_u64()?.min(255) as u8, bbox: [b[0].as_f64()? as f32, b[1].as_f64()? as f32, b[2].as_f64()? as f32, b[3].as_f64()? as f32], active: m["active"].as_bool().unwrap_or(false) })
}

fn cam_event(e: &Value) -> Option<CamEvent> {
    let t = |k: &str| e[k].as_str().map(str::to_string);
    Some(match e["type"].as_str()? {
        "approval" => CamEvent::Approval { call: t("call")?, action: cam_action(e["action"].as_str()?), camera: t("camera")?, summary: t("summary")?, detail: t("detail").unwrap_or_default() },
        "decision" => CamEvent::Decision { call: t("call")?, action: cam_action(e["action"].as_str()?), approved: e["approved"].as_bool()?, by: t("by").unwrap_or_default() },
        "motion" => CamEvent::Motion { camera: t("camera")?, mark: cam_mark(&e["mark"])? },
        "snapshot" => CamEvent::Snapshot { camera: t("camera")?, file: t("file")?, folder: t("folder")? },
        "failed" => CamEvent::Failed { camera: t("camera")?, action: cam_action(e["action"].as_str()?), error: t("error")? },
        "status" => CamEvent::Status { camera: t("camera")?, status: cam_status(e["status"].as_str()?), detail: t("detail").unwrap_or_default() },
        _ => return None,
    })
}

impl<S: Source> CameraPort for CoreCameras<S> {
    fn list(&mut self) -> CamResult<Vec<CamInfo>> {
        self.call("list", &json!({})).map(|v| cam_list(&v))
    }

    fn open(&mut self, cameras: &[String], large: bool, label: &str) -> CamResult<Vec<CamInfo>> {
        if self.view.is_some() {
            self.close();
        }
        let v = self.call("open", &json!({"cameras": cameras, "size": if large { "large" } else { "small" }, "label": label}))?;
        self.view = Some(v["id"].as_str().ok_or_else(|| "the answer was not readable".to_string())?.to_string());
        Ok(cam_list(&v["cameras"]))
    }

    fn decide(&mut self, call: &str, approve: bool) -> CamResult<()> {
        self.with_view("decide", json!({"call": call, "approve": approve})).map(|_| ())
    }

    fn state(&mut self) -> CamResult<ViewState> {
        let v = self.with_view("state", json!({}))?;
        let phase = match v["phase"]["phase"].as_str() {
            Some("awaiting_consent") => Phase::AwaitingConsent,
            Some("open") => Phase::Open,
            Some("denied") => Phase::Denied(v["phase"]["reason"].as_str().unwrap_or("").to_string()),
            _ => Phase::Closed,
        };
        Ok(ViewState { phase, indicator: v["indicator"].as_bool().unwrap_or(false), cameras: cam_list(&v["cameras"]) })
    }

    fn set_visible(&mut self, visible: bool) -> CamResult<()> {
        self.with_view("set_visible", json!({"visible": visible})).map(|_| ())
    }

    fn frame(&mut self, camera: &str, after: u64, format: ImageFormat) -> CamResult<Option<Image>> {
        let v = self.with_view("frame", json!({"camera": camera, "after": after, "format": format.key()}))?;
        if v.is_null() {
            return Ok(None);
        }
        let data = iyakki_ui_sdk::list::b64_decode(v["data"].as_str().unwrap_or("")).ok_or_else(|| "the picture was not readable".to_string())?;
        let f = ImageFormat::from_key(v["format"].as_str().unwrap_or("")).ok_or_else(|| "the picture was not readable".to_string())?;
        Ok(Some(Image { format: f, width: v["width"].as_u64().unwrap_or(0) as u32, height: v["height"].as_u64().unwrap_or(0) as u32, seq: v["seq"].as_u64().unwrap_or(0), data: std::sync::Arc::new(data) }))
    }

    fn overlay(&mut self, camera: &str) -> CamResult<Overlay> {
        let v = self.with_view("overlay", json!({"camera": camera}))?;
        let zones = v["zones"]
            .as_array()
            .map(|a| a.iter().filter_map(|z| Some(Shape { name: z["name"].as_str()?.to_string(), include: z["include"].as_bool()?, mask: z["mask"].as_bool()?, points: z["points"].as_array()?.iter().filter_map(|p| Some((p[0].as_f64()? as f32, p[1].as_f64()? as f32))).collect() })).collect())
            .unwrap_or_default();
        let marks = v["marks"].as_array().map(|a| a.iter().filter_map(cam_mark).collect()).unwrap_or_default();
        Ok(Overlay { zones, marks })
    }

    fn snapshot(&mut self, camera: &str) -> CamResult<String> {
        let v = self.with_view("snapshot", json!({"camera": camera}))?;
        v["call"].as_str().map(str::to_string).ok_or_else(|| "the answer was not readable".to_string())
    }

    fn ptz(&mut self, camera: &str, cmd: Ptz) -> CamResult<Option<String>> {
        let a = match cmd {
            Ptz::Move { pan, tilt, zoom } => json!({"camera": camera, "cmd": "move", "pan": pan, "tilt": tilt, "zoom": zoom}),
            Ptz::Stop => json!({"camera": camera, "cmd": "stop"}),
        };
        let v = self.with_view("ptz", a)?;
        Ok(v["call"].as_str().map(str::to_string))
    }

    fn events(&mut self) -> Vec<CamEvent> {
        match self.with_view("events", json!({})) {
            Ok(Value::Array(a)) => a.iter().filter_map(cam_event).collect(),
            _ => Vec::new(),
        }
    }

    fn close(&mut self) {
        if self.with_view("close", json!({})).is_ok() {
            self.view = None;
        }
    }
}

#[cfg(test)]
mod cameras_tests {
    use super::*;

    #[derive(Default)]
    struct Host {
        log: Vec<String>,
        asked: bool,
    }

    impl Source for Host {
        fn call(&mut self, domain: &str, method: &str, json: &str) -> Result<String, String> {
            assert_eq!(domain, "cameras");
            self.log.push(format!("{method} {json}"));
            let a: Value = serde_json::from_str(json).unwrap();
            if !["list", "open"].contains(&method) {
                assert_eq!(a["view"], "v-7", "{method}");
            }
            let cam = r#"{"id":"front","name":"Front door","source":"network","status":"live","detail":"","ptz":true,"recording":false,"motion_watch":true,"size":[640,480]}"#;
            match method {
                "list" => Ok(format!("[{cam}]")),
                "open" => Ok(format!(r#"{{"id":"v-7","cameras":[{cam}]}}"#)),
                "state" => Ok(format!(r#"{{"phase":{{"phase":"awaiting_consent","call":"c1"}},"indicator":false,"visible":true,"cameras":[{cam}]}}"#)),
                "frame" if a["after"] == 5 => Ok("null".into()),
                "frame" => Ok(format!(r#"{{"seq":5,"width":4,"height":2,"format":"{}","data":"{}"}}"#, a["format"].as_str().unwrap(), iyakki_ui_sdk::list::b64_encode(&[1u8; 32]))),
                "overlay" => Ok(r#"{"zones":[{"name":"Door","include":true,"mask":false,"points":[[0.1,0.1],[0.9,0.1],[0.9,0.9]]}],"marks":[{"id":3,"at_ms":9,"zone":"Door","score":80,"bbox":[0.1,0.2,0.3,0.4],"active":true}]}"#.into()),
                "snapshot" => Ok(r#"{"call":"c2"}"#.into()),
                "ptz" => Ok(r#"{"call":null}"#.into()),
                "events" => Ok(r#"[{"type":"approval","call":"c1","action":"view","camera":"","summary":"app wants to show 1 camera: Front door","detail":"Live pictures only."},{"type":"motion","camera":"front","mark":{"id":3,"at_ms":9,"zone":"Door","score":80,"bbox":[0.1,0.2,0.3,0.4],"active":true}},{"type":"odd"}]"#.into()),
                "decide" if a["approve"] == true && !self.asked => {
                    self.asked = true;
                    Err("the person was asked in their page; try the same call again after they answer".into())
                }
                _ => Ok(r#"{"ok":true}"#.into()),
            }
        }
    }

    #[test]
    fn every_call_of_the_view_reaches_the_host_in_the_wire_form_and_the_answers_are_read_back() {
        let shared = Shared::new(Host::default());
        let mut p = CoreCameras::new(shared.clone());
        assert!(p.state().is_err(), "no view before open");
        assert_eq!(p.list().unwrap()[0].name, "Front door");
        let cams = p.open(&["front".into()], true, "Panel").unwrap();
        assert_eq!((cams[0].status, cams[0].ptz, cams[0].size), (CamStatus::Live, true, (640, 480)));
        assert_eq!(p.state().unwrap().phase, Phase::AwaitingConsent);
        let f = p.frame("front", 0, ImageFormat::Jpeg).unwrap().unwrap();
        assert_eq!((f.width, f.height, f.seq, f.format, f.data.len()), (4, 2, 5, ImageFormat::Jpeg, 32));
        assert!(p.frame("front", 5, ImageFormat::Jpeg).unwrap().is_none(), "nothing newer");
        let o = p.overlay("front").unwrap();
        assert_eq!((o.zones[0].points.len(), o.marks[0].score, o.marks[0].bbox), (3, 80, [0.1, 0.2, 0.3, 0.4]));
        assert_eq!(p.snapshot("front").unwrap(), "c2");
        assert_eq!(p.ptz("front", Ptz::Move { pan: 0.5, tilt: 0.0, zoom: 0.0 }).unwrap(), None);
        p.ptz("front", Ptz::Stop).unwrap();
        p.set_visible(false).unwrap();
        let ev = p.events();
        assert_eq!(ev.len(), 2, "the unknown kind is dropped: {ev:?}");
        assert!(matches!(&ev[0], CamEvent::Approval { call, action: CamAction::View, .. } if call == "c1"));
        p.close();
        assert!(p.state().is_err(), "closed");
        let log = shared.0.borrow().log.join("\n");
        assert!(log.contains("\"size\":\"large\"") && log.contains("\"cmd\":\"move\"") && log.contains("\"cmd\":\"stop\"") && log.contains("\"visible\":false") && log.contains("\"format\":\"jpeg\""), "{log}");
    }

    #[test]
    fn a_yes_to_the_consent_card_is_told_the_person_was_asked_and_goes_through_when_the_same_call_is_made_again() {
        let mut p = CoreCameras::new(Shared::new(Host::default()));
        p.open(&["front".into()], false, "").unwrap();
        assert!(p.decide("c1", false).is_ok(), "a no needs nobody");
        let first = p.decide("c1", true).unwrap_err();
        assert!(first.contains("the person was asked in their page"), "{first}");
        assert!(p.decide("c1", true).is_ok());
    }
}
