//! The core side of the headless harness (F-SPLIT-03): a throwaway device whose services, accounts and audit log the UI program reaches over a
//! unix socket, and the steps that drive the shell on the UI side. Linux only (peer credentials on the socket).
#![allow(dead_code)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use iyakki_agent::services::Services;
use iyakki_agent_api::ServicesHandle;
use iyakki_shell::accounts::{AuditLog, UserStore, UsersRef};
use iyakki_shell::gfx::Gfx;
use iyakki_shell::shell::Shell;
use iyakki_shell::ui::{Event, Key};

pub struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

/// The machine behind the socket: its services, its people and its audit log.
pub struct Device {
    pub dir: PathBuf,
    pub svc: Arc<Services>,
    pub users: UsersRef,
    pub audit: Arc<AuditLog>,
    pub audit_path: PathBuf,
    /// The core's chat runs: each starts, hands out the scripted events and records the request it was given.
    pub runs: Arc<iyakki_agent_api::FakeRunStarter>,
}

impl Device {
    pub fn new(tag: &str) -> Device {
        let dir = iyakki_agent::testdir::short("iyakki-uishell", tag);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let svc = Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
        let mut store = UserStore::open(&dir.join("users.json")).unwrap();
        store.create_first_admin("root", "rootpass12").unwrap();
        let audit_path = dir.join("audit.log");
        Device { svc, users: UsersRef::of_store(&Arc::new(Mutex::new(store))), audit: Arc::new(AuditLog::new(&audit_path)), audit_path, dir, runs: Arc::new(iyakki_agent_api::FakeRunStarter::new(vec![iyakki_shared::agent::RunEvent::Model { text: "thinking it over".into(), calls: vec![] }, iyakki_shared::agent::RunEvent::Final { text: "the core answers hello".into() }])) }
    }

    /// What the core serves as objects: the roots of its services, its audit log, its accounts and (as a scripted starter) its chat runs.
    pub fn roots(&self) -> iyakki_compose::rpc::RpcRoots {
        iyakki_compose::rpc::RpcRoots::new().with_audit(self.audit.clone()).with_users(self.users.clone()).with_runs(self.runs.clone())
    }

    pub fn host(&self) -> Arc<iyakki_ui_rpc::Host> {
        // over the real services: the store calls need the core's own accounts, fetch and audit (D-SPLIT-03-STORE), which only this form sets
        self.roots().host_over(&self.svc)
    }

    pub fn audit_text(&self) -> String {
        self.audit.flush_notes();
        std::fs::read_to_string(&self.audit_path).unwrap_or_default()
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

pub fn type_text(sh: &mut Shell, s: &str) {
    for c in s.chars() {
        sh.handle(&Event::Key(Key::Char(c)));
    }
}

/// How many different colours a frame has: a screen that drew something has many, a blank one has one.
pub fn colours(g: &mut Gfx) -> usize {
    let (w, h) = (g.width() as i32, g.height() as i32);
    let px = g.grab((0, 0, w, h));
    let mut seen = std::collections::HashSet::new();
    for p in px.chunks(4) {
        seen.insert([p[0], p[1], p[2], p[3]]);
        if seen.len() > 64 {
            break;
        }
    }
    seen.len()
}

/// The steps both harness tests share: the login screen draws, the person signs in over the socket, the launcher draws, an app opens and a
/// setting changed on its page lands in the core's own settings and its audit log.
pub fn drive(sh: &mut Shell, dev: &Device) {
    let mut g = Gfx::new(1280, 720);
    sh.render(&mut g);
    assert_eq!(sh.screen_name(), "login", "the accounts come from the core: there is one, so the login screen is shown");
    assert!(colours(&mut g) > 8, "the login screen was drawn");
    type_text(sh, "root");
    sh.handle(&Event::Key(Key::Tab));
    type_text(sh, "wrongpass99");
    sh.handle(&Event::Key(Key::Enter));
    assert_eq!(sh.screen_name(), "login", "a wrong password is refused by the core's accounts");
    sh.render(&mut g);
    // the form keeps the name: go back to the password
    sh.handle(&Event::Key(Key::BackTab));
    sh.handle(&Event::Key(Key::Tab));
    for _ in 0..16 {
        sh.handle(&Event::Key(Key::Backspace));
    }
    type_text(sh, "rootpass12");
    sh.handle(&Event::Key(Key::Enter));
    for _ in 0..3 {
        sh.handle(&Event::Tick);
        sh.render(&mut g);
    }
    assert_eq!(sh.screen_name(), "launcher", "the person signed in through the core's accounts");
    assert!(colours(&mut g) > 8, "the launcher was drawn");
    assert!(sh.open_for_demo("settings"), "an app opens");
    for _ in 0..3 {
        sh.render(&mut g);
    }
    assert!(colours(&mut g) > 8, "the app was drawn");
    settings_round_trip(sh, dev, &mut g);
    chat_round_trip(sh, dev, &mut g);
}

/// The chat in agent mode: the message goes to the core as a request (data), the core's run answers, and the answer is on the screen of the program.
fn chat_round_trip(sh: &mut Shell, dev: &Device, g: &mut Gfx) {
    assert!(sh.open_for_demo("chat"), "the chat opens");
    sh.render(g);
    type_text(sh, "hello from the shell");
    sh.handle(&Event::Key(Key::Enter));
    {
        let asked = dev.runs.requests.lock().unwrap();
        assert_eq!(asked.len(), 1, "the core was asked to start one run");
        assert_eq!((asked[0].user.as_str(), asked[0].prompt.as_str()), ("root", "hello from the shell"));
    }
    // the chat is a private app (the developer-mode capture leaves its text out), so what it did is read from the core's own session store: the run's
    // events are polled on the heartbeat, and the answer is saved into the chat's session
    for _ in 0..50 {
        sh.handle(&Event::Tick);
        sh.render(g);
        let sessions = dev.svc.sessions();
        let said = sessions.list("root").iter().filter_map(|m| sessions.load("root", &m.id)).any(|s| format!("{:?}", s.messages).contains("the core answers hello"));
        if said {
            return;
        }
    }
    let sessions = dev.svc.sessions();
    let all: Vec<String> = sessions.list("root").iter().filter_map(|m| sessions.load("root", &m.id)).map(|s| format!("{} {:?}", s.id, s.messages)).collect();
    panic!("the answer of the core's run was not saved into the chat's session in the core: {all:?} said={:?} polled={:?}", dev.runs.requests.lock().unwrap().len(), sh.screen_name());
}

/// The Cloud mode page of Settings: key `a` flips "Show all AI tools" through the remote services, so the change must be in the core's own settings
/// and the line the page writes must be in the core's own audit log (the shell's `cx.audit` is the remote audit root).
fn settings_round_trip(sh: &mut Shell, dev: &Device, g: &mut Gfx) {
    // the page list scrolls: ask for each page by number until the one that names itself "Cloud mode" shows
    let found = (0..80).find(|n| {
        sh.handle(&Event::Tab(*n));
        sh.app_page() == "Cloud mode"
    });
    assert!(found.is_some(), "Settings has a Cloud mode page");
    sh.render(g);
    let before = dev.svc.show_all_ai();
    sh.handle(&Event::Key(Key::Char('a')));
    sh.render(g);
    assert_eq!(dev.svc.show_all_ai(), !before, "the change made in the shell landed in the core's own settings");
    let snap = screen_of(sh, g, &dev.dir.join("svc")).expect("the developer-mode capture answers");
    let said = snap.lines.iter().any(|l| l.text.contains(if before { "Show all AI tools: off" } else { "Show all AI tools: on" }));
    assert!(said, "the page says what the core now holds: {:?}", snap.lines.iter().map(|l| l.text.clone()).collect::<Vec<_>>());
    assert!(dev.audit_text().contains("settings.show_all_ai"), "the page's audit line is in the core's log, not the program's: {}", dev.audit_text());
    sh.handle(&Event::Key(Key::Char('a')));
    assert_eq!(dev.svc.show_all_ai(), before, "and back");
}

/// Every app the launcher can open, in the order of the tour of `iyakki-shell/examples/screens.rs`.
pub const APPS: &[&str] = &[
    "settings", "system", "users", "rag", "runs", "mcp", "skills", "calendar", "scan", "mesh", "usage", "inbox", "cli", "edit", "radio", "browser", "chat", "media", "tales", "data", "dash", "flows", "store", "ide", "cameras", "voice", "lab", "files",
    "terminal", "about", "disks", "devices", "cluster", "k8s", "git", "vault", "backup", "sound", "display", "network",
];

/// Open every app against the remote services, draw it twice and give it a heartbeat. A panic is caught and named, so one run lists every app that
/// cannot cope with an answer the core does not serve; the result is the apps that opened and the apps that panicked with the message.
pub fn sweep(sh: &mut Shell) -> (Vec<&'static str>, Vec<(String, String)>) {
    let (opened, panicked, _) = sweep_unserved(sh);
    (opened, panicked)
}

/// As [`sweep`], and the apps that opened as the "not available in this UI program" screen.
pub fn sweep_unserved(sh: &mut Shell) -> (Vec<&'static str>, Vec<(String, String)>, Vec<&'static str>) {
    let mut unserved = Vec::new();
    let mut g = Gfx::new(1280, 720);
    let (mut opened, mut panicked) = (Vec::new(), Vec::new());
    for id in APPS {
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if !sh.open_for_demo(id) {
                return false;
            }
            for _ in 0..2 {
                sh.render(&mut g);
                sh.handle(&Event::Tick);
            }
            if sh.app_is_unserved() {
                unserved.push(*id);
            }
            true
        }));
        match r {
            Ok(true) => opened.push(*id),
            Ok(false) => {}
            Err(e) => panicked.push((id.to_string(), e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default())),
        }
    }
    (opened, panicked, unserved)
}

/// What the open screen says, as the developer-mode state reads it (the same capture `iyakki dev state` uses): the shell draws while a second thread
/// asks for the frame.
pub fn screen_of(sh: &mut Shell, g: &mut Gfx, data: &std::path::Path) -> Result<iyakki_shared::devmode_state::Snapshot, String> {
    let d = data.to_path_buf();
    let asker = std::thread::spawn(move || iyakki_shared::devmode_state::fetch(&d, 5000));
    // the asker ends by itself (it has its own deadline): draw until it does, so a frame is drawn after it asked
    while !asker.is_finished() {
        sh.render(g);
        sh.handle(&Event::Tick);
        std::thread::yield_now();
    }
    asker.join().unwrap_or_else(|_| Err("the asker panicked".into()))
}

/// What the open screen says, as lines of text (the developer-mode capture).
fn screen_lines(sh: &mut Shell, dev: &Device, g: &mut Gfx) -> Vec<String> {
    screen_of(sh, g, &dev.dir.join("svc")).expect("the developer-mode capture answers").lines.into_iter().map(|l| l.text).collect()
}

/// The lines of the screen once one of them holds `needle` (the capture the core keeps is replaced by the frames the program draws, so the first
/// answers after an app opens can still be the app before it): a bounded number of frames, no clock.
fn screen_has(sh: &mut Shell, dev: &Device, g: &mut Gfx, needle: &str) -> Result<(), Vec<String>> {
    let mut lines = Vec::new();
    for _ in 0..8 {
        lines = screen_lines(sh, dev, g);
        if lines.iter().any(|l| l.contains(needle)) {
            return Ok(());
        }
    }
    Err(lines)
}

/// The tiles of the batch that used to answer "not available": each one reads and changes what the core holds through the objects the socket serves
/// (F-SPLIT-03, alpha.7 S3). Radio: a station added through the remote object is in the core's list and on the tile. Inbox: a notification the
/// core holds is on the tile, and a read mark the tile's handle makes is in the core's store. Media: a download queued through the socket is in the
/// core's queue. Rag: a document added through the socket is in the core's knowledge base. Lab, System and Cli open and draw.
pub fn tiles_round_trip(sh: &mut Shell, dev: &Device, svc: &Arc<dyn ServicesHandle>) {
    let mut g = Gfx::new(1280, 720);
    // RADIO
    let stations = svc.b5().radio_stations("root").expect("the core serves the radio stations");
    stations.add("Harness FM", "https://radio.example.com/harness").unwrap();
    assert!(dev.svc.b5().radio_stations("root").unwrap().list().iter().any(|s| s.name == "Harness FM"), "the station is in the core's own list");
    assert!(sh.open_for_demo("radio"));
    sh.render(&mut g);
    if let Err(lines) = screen_has(sh, dev, &mut g, "Harness FM") {
        panic!("the radio tile lists the station the core holds: {lines:?}");
    }
    // INBOX
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let note = dev.svc.inbox.add("root", iyakki_shared::inbox::Kind::Agent, "Harness note", "body", "harness", None, now).unwrap();
    assert!(sh.open_for_demo("inbox"));
    sh.render(&mut g);
    // the inbox is a private app: the developer-mode capture leaves its text out, so the tile is shown to draw and the store is read back
    assert!(colours(&mut g) > 8, "the inbox tile drew");
    let id = note.id.clone();
    svc.inbox().mark_read("root", &id, true).unwrap();
    assert!(dev.svc.inbox.list("root").iter().any(|n| n.id == id && n.read), "a read mark made through the socket is in the core's store");
    // MEDIA
    let queue = svc.b5().media_queue("root").expect("the core serves the download queue");
    let who = iyakki_shared::mediaqueue::Who { policy: iyakki_sec::policy::Role::Admin.template(), user: "root".into(), folders: vec![] };
    let queued = queue.enqueue(who, iyakki_shared::mediaqueue::Request { url: "https://files.example.com/harness.mp3".into(), ..Default::default() }).unwrap();
    assert!(dev.svc.b5().media_queue("root").unwrap().view(queued).is_some(), "the download is in the core's own queue");
    assert!(sh.open_for_demo("media"));
    sh.render(&mut g);
    // RAG
    svc.kb().add_document("user/root", "Harness", "harness.txt", "quokkas live on Rottnest Island", Default::default()).unwrap();
    assert_eq!(dev.svc.kb.lock().unwrap().docs("user/root").len(), 1, "the document is in the core's knowledge base");
    assert!(sh.open_for_demo("rag"));
    sh.render(&mut g);
    assert!(colours(&mut g) > 8, "the knowledge base tile drew");
    // LAB, SYSTEM, CLI
    for id in ["lab", "system", "cli"] {
        assert!(sh.open_for_demo(id), "{id} opens");
        sh.render(&mut g);
        sh.handle(&Event::Tick);
        sh.render(&mut g);
        assert!(colours(&mut g) > 8, "{id} drew");
        assert!(!sh.app_is_unserved(), "{id} is built from what the core serves");
    }
}

/// The tile apps of the store run in the core and the program draws what comes back (F-SPLIT-03, D-SPLIT-03-TILES). An HTML app the core lists
/// opens through `StoreApi::tile_open`: the page runs in the core's page engine and the page model crosses the socket; a WebAssembly app opens
/// through `tile_open_wasm` and `tile_pictures`: the core runs the tile and the program paints the display lists it gets. Both are the core's own
/// apps (the dev apps of the root account), neither opens as the "not available" screen, and what the program did is in the core's audit log.
pub fn tile_apps_round_trip(sh: &mut Shell, dev: &Device) {
    let mut g = Gfx::new(1280, 720);
    let admin = iyakki_sec::policy::Role::Admin.template();
    let write = |name: &str, files: &[(&str, &[u8])]| {
        let d = dev.dir.join(name);
        std::fs::create_dir_all(&d).unwrap();
        for (f, b) in files {
            std::fs::write(d.join(f), b).unwrap();
        }
        d
    };
    let html = write("app-html", &[("app.json", r#"{"id":"harness-html","name":"Harness page","icon":"🧩"}"#.as_bytes()), ("index.html", b"<h1>Harness heading</h1><p>the page runs in the core</p>")]);
    let wasm = write("app-wasm", &[("app.json", r#"{"id":"harness-wasm","name":"Harness tile","icon":"🔷","wasm":"tile.wasm"}"#.as_bytes()), ("index.html", b"<p>wasm</p>"), ("tile.wasm", iyakki_tile_example::TILE_WASM)]);
    iyakki_agent::store::tilehub::add_dev(&dev.svc, &admin, "root", &html).expect("the core adds the HTML app");
    iyakki_agent::store::tilehub::add_dev(&dev.svc, &admin, "root", &wasm).expect("the core adds the WebAssembly app");
    // the HTML app: opened through the socket, drawn from the page model the core sent
    assert!(sh.open_custom_by_name("harness-html"), "the HTML app opens: screen {} listed {:?}", sh.screen_name(), iyakki_agent::store::tilehub::installed(&dev.svc, &admin, "root").iter().map(|e| e.name.clone()).collect::<Vec<_>>());
    sh.render(&mut g);
    sh.handle(&Event::Tick);
    sh.render(&mut g);
    assert!(!sh.app_is_unserved(), "a tile app is not the unserved screen");
    if let Err(lines) = screen_has(sh, dev, &mut g, "heading") {
        panic!("the page of the app that runs in the core is on the program's screen: {lines:?}");
    }
    assert!(dev.audit_text().contains(r#""event":"tile.open","detail":"harness-html 0.1.0 (dev)""#), "the open is in the core's audit log: {}", dev.audit_text());
    // the WebAssembly app is refused in the core's own words until the switch is on, then runs there
    assert!(sh.open_custom_by_name("harness-wasm"), "the app opens, as a screen that says why it does not run");
    sh.render(&mut g);
    if let Err(lines) = screen_has(sh, dev, &mut g, "experimental") {
        panic!("the core's refusal reaches the screen: {lines:?}");
    }
    dev.svc.settings.lock().unwrap().update(|s| s.wasm_tiles = true).unwrap();
    assert!(sh.open_custom_by_name("harness-wasm"), "the WebAssembly app opens");
    for _ in 0..3 {
        sh.render(&mut g);
        sh.handle(&Event::Tick);
    }
    assert!(colours(&mut g) > 8, "the display list of the tile the core runs was painted");
    assert!(dev.audit_text().contains(r#""event":"tile.open","detail":"harness-wasm 0.1.0 (wasm"#), "{}", dev.audit_text());
}
