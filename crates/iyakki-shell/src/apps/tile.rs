//! A custom app in its own tile. The app is a web page (HTML, CSS, JavaScript) that `iyakki_agent::store::tile` keeps alive on IYAKKI's own
//! page engine; this screen lays the page out with the browser's layout code, draws it, turns clicks and typing into events for it, and
//! shows the standard bar of every custom app: what it is (custom app, publisher, trust level), Reload, Info (what it may do) and Remove.
//! Back is the shell's own button in the header. The app can reach nothing but its bridge: requests that need the person (a question to the
//! assistant, a file to open or save, the clipboard) come up here as a question to answer.
//!
//! What the page can use is what the reader engine draws: real `<button>`, `<input>`, `<select>`, `<textarea>` and `<a>` elements, text,
//! tables, boxes and colours from CSS. There is no canvas and plain elements do not take clicks (docs/APP-AUTHORING.md, "Tile apps").

use std::collections::HashMap;
use std::sync::Mutex;

use iyakki_agent::store::tile::{Request, TileRuntime};
use iyakki_agent::store::tilehub::{self, TileEntry};
use iyakki_agent::store::Store;
use iyakki_web::doc::{Control, ControlKind, Doc};

use super::browser_layout::{layout, Kind, Layout};
use super::card::{Action, Card, Tone};
use super::widgets::{PickEvent, Picker};
use super::{blink, App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(150, 220, 255);
const AMBER: [u8; 4] = rgb(255, 196, 94);
/// Click ids of the page's controls: `Event::Tab(CONTROL_BASE + index)`; text fields use `Event::Focus(CONTROL_BASE + index)`.
pub const CONTROL_BASE: usize = 7000;
const BAR_BUTTONS: usize = 7200;

/// Titles live as long as the program (the `App` trait wants `&'static str`); one copy of each distinct text, at most this many.
fn intern(s: &str) -> &'static str {
    static POOL: Mutex<Vec<&'static str>> = Mutex::new(Vec::new());
    let mut p = POOL.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(x) = p.iter().find(|x| **x == s) {
        return x;
    }
    if p.len() >= 256 {
        return "Custom app";
    }
    let leaked: &'static str = Box::leak(s.to_string().into_boxed_str());
    p.push(leaked);
    leaked
}

enum Dialog {
    None,
    Info(Card),
    Remove(Card),
    /// Typing the app's name to delete its data too.
    DeleteData(TextField),
    /// The app asks the assistant a question: allow or not.
    Ask(Request, Card),
    /// Choosing a file in Uploads for the app to read.
    Open(Request, Picker, Vec<std::path::PathBuf>),
    Save(Request, Card),
    Clip(Request, Card),
}

pub struct TileApp {
    entry: TileEntry,
    user: String,
    rt: Option<TileRuntime>,
    err: Option<String>,
    doc: Option<Doc>,
    layout: Option<(Layout, (i32, i32))>,
    scroll: f32,
    content_h: f32,
    view_h: f32,
    focus: Option<usize>,
    edit: TextField,
    sel: HashMap<usize, usize>,
    dialog: Dialog,
    title: String,
    toast: Option<(String, u64)>,
    dirty: bool,
    ticks: u64,
    stamp: u64,
    leave: bool,
    host_save: Option<(String, Vec<u8>)>,
    pending_ai: Option<(u32, std::sync::mpsc::Receiver<Result<String, String>>)>,
    redraw: bool,
    /// Where the last frame put the bar's buttons (tests).
    pub bar: Vec<(String, R)>,
    /// Developer mode: the build number this copy runs (a new build pushed from the computer reloads it) and the console lines already
    /// written to the app's log.
    dev_build: u64,
    console_seen: usize,
}

impl TileApp {
    pub fn new(entry: TileEntry, cx: &AppCx) -> TileApp {
        let mut a = TileApp { title: entry.title.clone(), user: cx.session.user.clone(), entry, rt: None, err: None, doc: None, layout: None, scroll: 0.0, content_h: 0.0, view_h: 0.0, focus: None, edit: TextField::new("", false), sel: HashMap::new(), dialog: Dialog::None, toast: None, dirty: true, ticks: 0, stamp: 0, leave: false, host_save: None, pending_ai: None, redraw: false, bar: vec![], dev_build: 0, console_seen: 0 };
        a.start(cx);
        a
    }

    fn start(&mut self, cx: &AppCx) {
        self.err = None;
        self.doc = None;
        self.layout = None;
        self.focus = None;
        self.scroll = 0.0;
        self.dirty = true;
        match tilehub::open(&cx.services, &cx.session.policy, &self.user, &self.entry, cx.web.clone()) {
            Ok(rt) => {
                self.rt = Some(rt);
                self.console_seen = 0;
                self.dev_build = iyakki_agent::devmode::reload_stamp(cx.services.dir(), &self.entry.name);
                if let Some(d) = &self.entry.dir {
                    self.stamp = tilehub::dev_stamp(d);
                }
            }
            Err(e) => {
                self.rt = None;
                self.err = Some(e);
            }
        }
        cx.audit.record(&self.user, "tile.open", &format!("{} {}{}", self.entry.name, self.entry.version, if self.entry.dev { " (dev)" } else { "" }));
    }

    /// The text of the panel that is open, if any (tests).
    pub fn card_text(&self) -> String {
        match &self.dialog {
            Dialog::Info(c) | Dialog::Remove(c) | Dialog::Ask(_, c) | Dialog::Save(_, c) | Dialog::Clip(_, c) => format!("{}\n{}", c.title, c.lines.iter().map(|l| l.0.clone()).collect::<Vec<_>>().join("\n")),
            _ => String::new(),
        }
    }

    /// Where the open panel and its buttons were drawn (tests).
    pub fn card_geometry(&self) -> Option<(R, Vec<R>)> {
        match &self.dialog {
            Dialog::Info(c) | Dialog::Remove(c) | Dialog::Ask(_, c) | Dialog::Save(_, c) | Dialog::Clip(_, c) => Some((c.last_panel, c.last_buttons.clone())),
            _ => None,
        }
    }

    pub fn entry(&self) -> &TileEntry {
        &self.entry
    }

    /// The text of the page now (tests).
    pub fn page_text(&mut self) -> String {
        self.rt.as_mut().map(|r| r.page_text()).unwrap_or_default()
    }

    pub fn runtime(&mut self) -> Option<&mut TileRuntime> {
        self.rt.as_mut()
    }

    pub fn error(&self) -> Option<&str> {
        self.err.as_deref()
    }

    fn rebuild(&mut self) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        if let Some(rt) = self.rt.as_mut() {
            self.doc = Some(rt.doc());
            self.layout = None;
        }
        self.redraw = true;
    }

    /// Developer mode: new lines of the page's console go to the app's log on the device (`iyakki dev logs <id> --follow`). Only for apps
    /// installed from a paired computer; other apps' consoles are never written down.
    fn dev_console(&mut self, cx: &AppCx) {
        if !self.entry.is_developer() {
            return;
        }
        let Some(rt) = self.rt.as_ref() else { return };
        let lines = &rt.page.report.console;
        if lines.len() <= self.console_seen {
            self.console_seen = self.console_seen.min(lines.len());
            return;
        }
        let dm = iyakki_agent::devmode::DevMode::open(cx.services.dir());
        for l in lines.iter().skip(self.console_seen).take(200) {
            dm.app_log(&self.entry.name, &format!("console.{}: {}", l.level, l.text));
        }
        self.console_seen = lines.len();
    }

    fn rt_note(&mut self, cx: &AppCx) {
        let Some(rt) = self.rt.as_mut() else { return };
        let (toasts, title, notices, reqs) = {
            let mut o = rt.out.lock().unwrap_or_else(|e| e.into_inner());
            (std::mem::take(&mut o.toasts), o.title.take(), std::mem::take(&mut o.notices), o.requests.clone())
        };
        if let Some(t) = toasts.last() {
            self.toast = Some((t.clone(), self.ticks + 90));
        }
        if let Some(t) = title {
            if !t.trim().is_empty() {
                self.title = t;
            }
        }
        for (t, b) in notices {
            let _ = cx.services.inbox.add(&self.user, iyakki_agent::inbox::Kind::Info, &format!("{}: {t}", self.entry.title), &b, &self.entry.name, None, iyakki_agent::clock::now_unix());
        }
        if matches!(self.dialog, Dialog::None) {
            if let Some(r) = reqs.into_iter().next() {
                self.dialog = self.dialog_for(r, cx);
            }
        }
        self.dirty = true;
    }

    fn dialog_for(&self, r: Request, cx: &AppCx) -> Dialog {
        let who = format!("“{}”", self.entry.title);
        match r.kind.as_str() {
            "agent.ask" => {
                let q = r.args["q"].as_str().unwrap_or("").to_string();
                let card = Card::new("An app asks the assistant", AMBER).line(&format!("{who} would like to ask the assistant this question. The assistant sees only the question, not your files or other apps."), Tone::Normal).line("", Tone::Normal).line(&q, Tone::Bold).action(Action::new("Allow and ask", Key::Char('y'), OK)).action(Action::new("Don't ask", Key::Esc, MUTED));
                Dialog::Ask(r, card)
            }
            "host.saveFile" => {
                let name = r.args["name"].as_str().unwrap_or("file.txt").to_string();
                let size = r.args["text"].as_str().map_or(0, str::len);
                let card = Card::new("An app wants to save a file", AMBER).line(&format!("{who} wants to save “{name}” ({size} bytes). You choose where it goes in the save dialog."), Tone::Normal).action(Action::new("Choose where…", Key::Char('y'), OK)).action(Action::new("Cancel", Key::Esc, MUTED));
                Dialog::Save(r, card)
            }
            "host.openFile" => {
                let home = iyakki_agent::scope::home_root(&cx.session.user, cx.services.dir()).join("Uploads");
                let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&home).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_file() && std::fs::metadata(p).map_or(false, |m| m.len() <= 262_144)).collect();
                files.sort();
                files.truncate(200);
                let items: Vec<String> = files.iter().map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()).collect();
                Dialog::Open(r, Picker::new(&format!("{} asks for a file from your Uploads folder (text files up to 256 KB)", self.entry.title), items), files)
            }
            _ => {
                let write = r.kind == "clipboard.write";
                let card = Card::new(if write { "An app wants to copy" } else { "An app wants to read the clipboard" }, AMBER).line(&if write { format!("{who} wants to put {} characters on the clipboard.", r.args["text"].as_str().map_or(0, |t| t.chars().count())) } else { format!("{who} wants to read what is on the clipboard now.") }, Tone::Normal).action(Action::new("Allow", Key::Char('y'), OK)).action(Action::new("Don't allow", Key::Esc, MUTED));
                Dialog::Clip(r, card)
            }
        }
    }

    fn answer(&mut self, id: u32, ok: bool, value: &str) {
        if let Some(rt) = self.rt.as_mut() {
            rt.resolve(id, ok, value);
        }
        self.dialog = Dialog::None;
        self.dirty = true;
    }

    fn info_card(&self) -> Card {
        let e = &self.entry;
        let mut c = Card::new(&format!("{} · custom app", e.title), ACCENT).line(&e.detail_line(), Tone::Bold);
        c = c.line(&format!("Version {} · id {} · group {}", e.version, e.name, e.group), Tone::Dim);
        if e.is_developer() {
            c = c.line("DEVELOPER (LOCAL): installed from a paired developer computer while developer mode is on. Nobody has reviewed it and it is never updated from a store; a new build pushed from the computer reloads it here.", Tone::Warn);
        } else if e.dev {
            c = c.line("UNSIGNED DEV APP: this is the folder you are working on, added by an administrator in developer mode. Nobody has reviewed or signed it. It reloads when its files change.", Tone::Warn);
            if let Some(d) = &e.dir {
                c = c.line(&format!("Files: {}", d.display()), Tone::Dim);
            }
        } else {
            c = c.line("Installed through the STORE: its trust level, signature and permissions were shown to you before it was installed.", Tone::Normal);
        }
        c = c.line("", Tone::Normal).line("What it may do", Tone::Bold);
        let perms = e.caps.permissions();
        if perms.is_empty() {
            c = c.line("Nothing beyond showing its own page: no saved data, no network, no files, no clipboard.", Tone::Good);
        }
        for p in perms {
            if let Ok(x) = iyakki_agent::store::manifest::Permission::parse(&p) {
                c = c.line(&format!("- {}", x.words(&self.user)), Tone::Normal);
            }
        }
        c = c.line("It can never use your vault, other apps or tiles, the shell, or any file you did not hand to it.", Tone::Dim);
        if let Some(rt) = &self.rt {
            let refused: Vec<String> = rt.out.lock().map(|o| o.refused.clone()).unwrap_or_default();
            if !refused.is_empty() {
                c = c.line("", Tone::Normal).line("Calls the app made that were refused", Tone::Bold);
                for r in refused.iter().take(5) {
                    c = c.line(&format!("- {r}"), Tone::Warn);
                }
            }
            for n in rt.page.report.notes.iter().take(5) {
                c = c.line(&format!("page: {n}"), Tone::Warn);
            }
        }
        if !e.icon.is_empty() {
            c = c.line(&format!("Icon: {}", e.icon), Tone::Dim);
        }
        c.action(Action::new("Close", Key::Esc, ACCENT))
    }

    fn remove_card(&self, cx: &AppCx) -> Card {
        let e = &self.entry;
        if e.dev {
            return Card::new(&format!("Remove “{}” from your launcher?", e.title), ERR).danger().line("This takes the dev app off your launcher. The folder and its files are not touched.", Tone::Normal).action(Action::new("Remove", Key::Char('y'), ERR)).action(Action::new("Cancel", Key::Esc, MUTED));
        }
        let store = Store::for_services(&cx.services);
        let loaded = store.local_catalogs(&cx.services);
        let lives = store.lifecycle(&cx.services, &cx.session.policy, &self.user, &loaded, &Default::default());
        let id = format!("tile:{}", e.name);
        let mut c = Card::new(&format!("Remove “{}”?", e.title), ERR).danger();
        match Store::find_life(&lives, &id) {
            Ok(l) => {
                let plan = store.plan_remove(&cx.services, &self.user, &lives, l);
                for line in plan.summary().lines() {
                    c = c.line(line, Tone::Normal);
                }
                c = c.action(Action::new(if plan.has_data() { "Remove, keep its data" } else { "Remove" }, Key::Char('y'), ERR));
                if plan.has_data() {
                    c = c.action(Action::new("Remove and delete its data…", Key::Char('d'), ERR));
                }
            }
            Err(e) => c = c.line(&format!("It cannot be removed here: {e}"), Tone::Bad),
        }
        c.action(Action::new("Cancel", Key::Esc, MUTED))
    }

    fn do_remove(&mut self, cx: &AppCx, delete_data: bool, typed: &str) {
        let e = self.entry.clone();
        if e.dev {
            match tilehub::remove_dev(&cx.services, &self.user, &e.name) {
                Ok(()) => {
                    cx.audit.record(&self.user, "tile.remove", &format!("{} (dev)", e.name));
                    self.leave = true;
                }
                Err(m) => self.toast = Some((m, self.ticks + 120)),
            }
            self.dialog = Dialog::None;
            return;
        }
        let store = Store::for_services(&cx.services);
        let loaded = store.local_catalogs(&cx.services);
        let lives = store.lifecycle(&cx.services, &cx.session.policy, &self.user, &loaded, &Default::default());
        let audit = cx.audit.clone();
        let user = self.user.clone();
        let log = move |a: &str, d: &str| audit.record(&user, a, d);
        let result = match Store::find_life(&lives, &format!("tile:{}", e.name)) {
            Ok(l) => store.remove_item(&cx.services, &cx.session.policy, &self.user, &lives, l, &iyakki_agent::store::lifecycle::RemoveOpts { delete_data, typed: typed.to_string(), ..Default::default() }, &log).map_err(|x| x.to_string()),
            Err(m) => Err(m),
        };
        match result {
            Ok(_) => {
                self.dialog = Dialog::None;
                self.leave = true;
            }
            Err(m) => {
                self.toast = Some((m, self.ticks + 160));
                self.dialog = Dialog::None;
            }
        }
    }

    fn controls_next(&self, from: Option<usize>, back: bool) -> Option<usize> {
        let doc = self.doc.as_ref()?;
        let ok = |c: &Control| !matches!(c.kind, ControlKind::Hidden) && !c.disabled;
        let idx: Vec<usize> = doc.controls.iter().enumerate().filter(|(_, c)| ok(c)).map(|(i, _)| i).collect();
        if idx.is_empty() {
            return None;
        }
        let pos = from.and_then(|f| idx.iter().position(|i| *i == f));
        Some(match (pos, back) {
            (None, false) => idx[0],
            (None, true) => *idx.last()?,
            (Some(p), false) => idx[(p + 1) % idx.len()],
            (Some(p), true) => idx[(p + idx.len() - 1) % idx.len()],
        })
    }

    fn activate(&mut self, i: usize, cx: &AppCx) {
        let Some(doc) = self.doc.clone() else { return };
        let Some(c) = doc.controls.get(i) else { return };
        self.focus = Some(i);
        match c.kind {
            ControlKind::Text | ControlKind::Password | ControlKind::Textarea => {
                self.edit.set(&c.value);
                self.edit.mask = c.kind == ControlKind::Password;
            }
            ControlKind::Select => {
                let n = c.options.len();
                if n > 0 {
                    let cur = self.sel.get(&c.node).copied().or_else(|| c.options.iter().position(|o| o.selected)).unwrap_or(0);
                    let next = (cur + 1) % n;
                    self.sel.insert(c.node, next);
                    if let Some(rt) = self.rt.as_mut() {
                        let _ = rt.page.input(c.node, &c.options[next].value);
                    }
                    self.dirty = true;
                }
            }
            ControlKind::Hidden => {}
            ControlKind::Checkbox | ControlKind::Radio | ControlKind::Button | ControlKind::Submit => {
                if let Some(rt) = self.rt.as_mut() {
                    let _ = rt.click_control(&doc, i);
                }
                self.dirty = true;
            }
        }
        self.rt_note(cx);
    }

    fn type_key(&mut self, k: &Key, cx: &AppCx) -> bool {
        let Some(i) = self.focus else { return false };
        let Some(doc) = self.doc.clone() else { return false };
        let Some(c) = doc.controls.get(i) else { return false };
        if !matches!(c.kind, ControlKind::Text | ControlKind::Password | ControlKind::Textarea) {
            return false;
        }
        if matches!(k, Key::Enter) && c.kind != ControlKind::Textarea {
            // Enter in a field presses the first submit button of its form
            let submit = doc.controls.iter().position(|x| x.kind == ControlKind::Submit && x.form == c.form && !x.disabled);
            if let Some(j) = submit {
                if let Some(rt) = self.rt.as_mut() {
                    let _ = rt.click_control(&doc, j);
                }
                self.dirty = true;
                self.rt_note(cx);
            }
            return true;
        }
        if c.readonly || c.disabled {
            return true;
        }
        let before = self.edit.value.clone();
        let used = if matches!(k, Key::Enter) && c.kind == ControlKind::Textarea { self.edit.paste("\n") } else { self.edit.handle(k) };
        if self.edit.value != before {
            let v = self.edit.value.clone();
            if let Some(rt) = self.rt.as_mut() {
                let _ = rt.type_into(&doc, i, &v);
            }
            self.dirty = true;
            self.rt_note(cx);
        }
        used
    }

    fn bar_height(s: f32) -> f32 {
        44.0 * s
    }

    fn handle_dialog(&mut self, ev: &Event, cx: &AppCx) -> bool {
        let mut d = std::mem::replace(&mut self.dialog, Dialog::None);
        let mut keep = true;
        match &mut d {
            Dialog::None => keep = false,
            Dialog::Info(c) => {
                if c.handle(ev) == Some(Key::Esc) {
                    keep = false;
                }
            }
            Dialog::Remove(c) => match c.handle(ev) {
                Some(Key::Char('y')) => {
                    self.do_remove(cx, false, "");
                    return true;
                }
                Some(Key::Char('d')) => {
                    let mut tf = TextField::new(&format!("type {} to delete its data", self.entry.name), false);
                    tf.max = 64;
                    d = Dialog::DeleteData(tf);
                }
                Some(Key::Esc) => keep = false,
                _ => {}
            },
            Dialog::DeleteData(tf) => match ev {
                Event::Key(Key::Esc) => keep = false,
                Event::Key(Key::Enter) => {
                    let typed = tf.value.clone();
                    self.do_remove(cx, true, &typed);
                    return true;
                }
                Event::Key(k) => {
                    tf.handle(k);
                }
                Event::Paste(t) => {
                    tf.paste(t);
                }
                _ => {}
            },
            Dialog::Ask(r, c) => match c.handle(ev) {
                Some(Key::Char('y')) => {
                    let (id, q) = (r.id, r.args["q"].as_str().unwrap_or("").to_string());
                    cx.audit.record(&self.user, "tile.ask", &format!("{} allowed a question", self.entry.name));
                    let chat = cx.chat.clone();
                    let (tx, rx) = std::sync::mpsc::channel();
                    std::thread::spawn(move || {
                        let msgs = vec![iyakki_agent::chat::Message::system("A small app on the person's computer asks you one question. Answer briefly and plainly. You cannot use tools or see anything but this question."), iyakki_agent::chat::Message::user(q)];
                        let _ = tx.send(chat.complete(&msgs));
                    });
                    self.pending_ai = Some((id, rx));
                    keep = false;
                    self.toast = Some(("Asking the assistant…".into(), self.ticks + 60));
                    // the request stays in the outbox until it is answered; hide the dialog meanwhile
                    if let Some(rt) = self.rt.as_mut() {
                        if let Ok(mut o) = rt.out.lock() {
                            o.requests.retain(|x| x.id != id);
                        }
                    }
                }
                Some(Key::Esc) => {
                    let id = r.id;
                    self.answer(id, false, "the person did not allow the question");
                    return true;
                }
                _ => {}
            },
            Dialog::Open(r, p, files) => match p.handle_event(ev) {
                PickEvent::Chosen(name) => {
                    let id = r.id;
                    let path = files.iter().find(|f| f.file_name().is_some_and(|n| n.to_string_lossy() == name)).cloned();
                    let text = path.and_then(|p| std::fs::read_to_string(p).ok());
                    match text {
                        Some(t) => {
                            cx.audit.record(&self.user, "tile.openfile", &format!("{} read {name}", self.entry.name));
                            let v = serde_json::json!({"name": name, "text": t}).to_string();
                            if let Some(rt) = self.rt.as_mut() {
                                let _ = rt.page.eval("open", &format!("__iyakkiResolve({id}, true, JSON.parse({}))", serde_json::to_string(&v).unwrap_or_default()));
                                if let Ok(mut o) = rt.out.lock() {
                                    o.requests.retain(|x| x.id != id);
                                }
                            }
                            self.dirty = true;
                            return true;
                        }
                        None => {
                            self.answer(id, false, "that file could not be read as text");
                            return true;
                        }
                    }
                }
                PickEvent::Cancel => {
                    let id = r.id;
                    self.answer(id, false, "the person did not choose a file");
                    return true;
                }
                PickEvent::None => {}
            },
            Dialog::Save(r, c) => match c.handle(ev) {
                Some(Key::Char('y')) => {
                    let (id, name, text) = (r.id, r.args["name"].as_str().unwrap_or("file.txt").to_string(), r.args["text"].as_str().unwrap_or("").to_string());
                    cx.audit.record(&self.user, "tile.savefile", &format!("{} {name}", self.entry.name));
                    self.host_save = Some((name, text.into_bytes()));
                    self.answer(id, true, "saved");
                    return true;
                }
                Some(Key::Esc) => {
                    let id = r.id;
                    self.answer(id, false, "the person did not save the file");
                    return true;
                }
                _ => {}
            },
            Dialog::Clip(r, c) => match c.handle(ev) {
                Some(Key::Char('y')) => {
                    let id = r.id;
                    if r.kind == "clipboard.write" {
                        crate::ui::clipboard::copy(r.args["text"].as_str().unwrap_or(""));
                        self.answer(id, true, "");
                    } else {
                        let t = crate::ui::clipboard::latest().unwrap_or_default();
                        self.answer(id, true, &t);
                    }
                    return true;
                }
                Some(Key::Esc) => {
                    let id = r.id;
                    self.answer(id, false, "the person did not allow the clipboard");
                    return true;
                }
                _ => {}
            },
        }
        if keep {
            self.dialog = d;
        }
        true
    }
}

impl App for TileApp {
    fn title(&self) -> &'static str {
        intern(&self.title)
    }

    fn icon(&self) -> Icon {
        Icon::Store
    }

    fn accent(&self) -> [u8; 4] {
        ACCENT
    }

    fn hint(&self) -> String {
        "click and type in the app · Tab next control · Reload · Info · Remove".into()
    }

    fn help(&self) -> Vec<String> {
        vec!["Click or Enter/Space on a button; type into a field".into(), "Tab / Shift+Tab: next or previous control".into(), "PgUp / PgDn, wheel: scroll the app".into(), "Reload (Ctrl+R): start the app again".into(), "Info (i): what it may do and who made it".into(), "Remove: take the tile away (asks what to do with its data)".into()]
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if self.leave {
            return Flow::Back;
        }
        if !matches!(self.dialog, Dialog::None) {
            self.handle_dialog(ev, cx);
            return if self.leave { Flow::Back } else { Flow::Stay };
        }
        match ev {
            Event::Tick => {
                self.ticks += 1;
                if self.ticks % 3 == 0 {
                    if let Some(rt) = self.rt.as_mut() {
                        // timers in the page (a clock) run in small steps of its own time
                        if rt.page.advance(100) > 0 {
                            self.dirty = true;
                        }
                    }
                    self.rt_note(cx);
                    self.dev_console(cx);
                }
                if let Some((id, rx)) = &self.pending_ai {
                    if let Ok(r) = rx.try_recv() {
                        let id = *id;
                        self.pending_ai = None;
                        match r {
                            Ok(t) => self.answer(id, true, &t),
                            Err(e) => self.answer(id, false, &e),
                        }
                    }
                }
                // developer mode: a new build arrived from the paired computer (`iyakki dev push --watch`)
                if self.ticks % 10 == 5 && self.entry.is_developer() && iyakki_agent::devmode::reload_stamp(cx.services.dir(), &self.entry.name) != self.dev_build {
                    if let Some(e) = iyakki_agent::store::tilehub::installed(&cx.services, &cx.session.policy, &self.user).into_iter().find(|e| e.name == self.entry.name && e.scope == self.entry.scope) {
                        self.entry = e;
                    }
                    self.start(cx);
                    self.toast = Some((format!("Reloaded: version {} arrived from the developer computer", self.entry.version), self.ticks + 60));
                }
                if self.ticks % 30 == 0 {
                    if let Some(d) = self.entry.dir.clone() {
                        let st = tilehub::dev_stamp(&d);
                        if st != self.stamp {
                            self.stamp = st;
                            self.start(cx);
                            self.toast = Some(("Reloaded: the files changed".into(), self.ticks + 60));
                        }
                    }
                }
            }
            Event::Tab(n) if (BAR_BUTTONS..BAR_BUTTONS + 3).contains(n) => match n - BAR_BUTTONS {
                0 => {
                    self.start(cx);
                    self.toast = Some(("Reloaded".into(), self.ticks + 45));
                }
                1 => self.dialog = Dialog::Info(self.info_card()),
                _ => self.dialog = Dialog::Remove(self.remove_card(cx)),
            },
            Event::Tab(n) if *n >= CONTROL_BASE && *n < CONTROL_BASE + 5000 => self.activate(n - CONTROL_BASE, cx),
            Event::FocusAt(n, x) if *n >= CONTROL_BASE && *n < CONTROL_BASE + 5000 => {
                self.activate(n - CONTROL_BASE, cx);
                self.edit.click(*x);
            }
            Event::Scroll(d) => self.scroll = (self.scroll - *d as f32 * 48.0).clamp(0.0, (self.content_h - self.view_h).max(0.0)),
            Event::Key(Key::Ctrl('r')) | Event::Key(Key::F(5)) => {
                self.start(cx);
            }
            Event::Key(Key::PageDown) => self.scroll = (self.scroll + self.view_h * 0.9).min((self.content_h - self.view_h).max(0.0)),
            Event::Key(Key::PageUp) => self.scroll = (self.scroll - self.view_h * 0.9).max(0.0),
            Event::Key(Key::Tab) => {
                self.focus = self.controls_next(self.focus, false);
                if let (Some(i), Some(d)) = (self.focus, self.doc.as_ref()) {
                    if let Some(c) = d.controls.get(i) {
                        self.edit.set(&c.value);
                    }
                }
            }
            Event::Key(Key::BackTab) => {
                self.focus = self.controls_next(self.focus, true);
            }
            Event::Key(Key::Esc) => {
                if self.focus.is_some() {
                    self.focus = None;
                } else {
                    return Flow::Back;
                }
            }
            Event::Key(k) => {
                if !self.type_key(k, cx) {
                    match (k, self.focus) {
                        (Key::Enter | Key::Char(' '), Some(i)) => self.activate(i, cx),
                        (Key::Char('i'), None) => self.dialog = Dialog::Info(self.info_card()),
                        (Key::Down, _) => self.scroll = (self.scroll + 40.0).min((self.content_h - self.view_h).max(0.0)),
                        (Key::Up, _) => self.scroll = (self.scroll - 40.0).max(0.0),
                        _ => {}
                    }
                }
            }
            Event::Paste(t) => {
                if let Some(i) = self.focus {
                    if self.edit.paste(t) {
                        let v = self.edit.value.clone();
                        if let (Some(rt), Some(doc)) = (self.rt.as_mut(), self.doc.as_ref()) {
                            let _ = rt.type_into(doc, i, &v);
                        }
                        self.dirty = true;
                    }
                }
            }
            _ => {}
        }
        if self.leave {
            Flow::Back
        } else {
            Flow::Stay
        }
    }

    fn wants_redraw(&mut self) -> bool {
        std::mem::take(&mut self.redraw)
    }

    #[cfg(test)]
    fn as_any_tile(&mut self) -> Option<&mut TileApp> {
        Some(self)
    }

    fn take_host_save(&mut self) -> Option<(String, Vec<u8>)> {
        self.host_save.take()
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        self.rebuild();
        // the bar
        let bh = Self::bar_height(s);
        g.rect(area.x, area.y, area.w, bh, rgb(14, 19, 38));
        g.rect(area.x, area.y + bh - 1.0, area.w, 1.0, [255, 255, 255, 18]);
        let sz = 13.0 * s;
        let mut x = area.x + 16.0 * s;
        let by = area.y + (bh - sz * 2.1) / 2.0;
        let badge = if self.entry.dev { "UNSIGNED DEV APP" } else if self.entry.is_developer() { "DEVELOPER (LOCAL)" } else { "custom app" };
        x += crate::fx::chip(g, x, area.y + (bh - 13.0 * s * 1.9 * 0.85) / 2.0, s * 0.85, badge, if self.entry.dev || self.entry.is_developer() { AMBER } else { ACCENT }, true) + 8.0 * s;
        // buttons from the right: Remove, Info, Reload
        self.bar.clear();
        let mut right = area.x + area.w - 14.0 * s;
        let labels = [("Remove", 2usize, ERR), ("Info", 1, ACCENT), ("Reload", 0, ACCENT)];
        for (label, n, col) in labels {
            let w = g.text_width(sz, label, Face::Bold) + sz * 1.7;
            right -= w;
            g.button(right, by, sz, label, col, Event::Tab(BAR_BUTTONS + n));
            self.bar.push((label.to_string(), R::new(right, by, w, sz * 2.1)));
            right -= 8.0 * s;
        }
        let detail = self.entry.detail_line();
        let room = right - x - 8.0 * s;
        if room > 80.0 * s {
            let t = g.fit(12.0 * s, &detail, Face::Sans, room);
            g.text(x, area.y + (bh - g.line_height(12.0 * s, Face::Sans)) / 2.0, 12.0 * s, MUTED, &t, Face::Sans);
        }
        // the page
        let view = R::new(area.x, area.y + bh, area.w, area.h - bh);
        self.view_h = view.h;
        if let Some(e) = &self.err {
            g.text(view.x + 24.0 * s, view.y + 24.0 * s, 16.0 * s, ERR, "This app could not start", Face::Bold);
            for (i, l) in g.wrap(14.0 * s, e, Face::Sans, view.w - 48.0 * s).iter().take(6).enumerate() {
                g.text(view.x + 24.0 * s, view.y + 54.0 * s + i as f32 * 20.0 * s, 14.0 * s, MUTED, l, Face::Sans);
            }
        } else if let Some(doc) = self.doc.clone() {
            let col_w = (view.w - 32.0 * s).min(900.0 * s).max(120.0);
            let col_x = view.x + (view.w - col_w) / 2.0;
            let key = crate::apps::browser_layout::key_of(col_w, s);
            if self.layout.as_ref().is_none_or(|(_, k)| *k != key) {
                let l = layout(g, &doc, col_w, s, &[], 0);
                self.layout = Some((l, key));
            }
            let (l, _) = self.layout.as_ref().expect("laid out");
            if let Some(c) = l.canvas {
                g.rect(view.x, view.y, view.w, view.h, c);
            }
            self.content_h = l.height + 24.0 * s;
            self.scroll = self.scroll.clamp(0.0, (self.content_h - view.h).max(0.0));
            let top = view.y + 12.0 * s;
            let light = l.canvas.is_some_and(|c| super::browser_dark::is_light_color(c));
            let (scroll, focus) = (self.scroll, self.focus);
            let edit = &self.edit;
            let sel = &self.sel;
            let (l, _) = self.layout.as_ref().expect("laid out");
            for p in &l.prims {
                let (py, ph) = (p.y - scroll, p.h);
                if py + ph < -12.0 || py > view.h + 12.0 {
                    continue;
                }
                let (dx, dy) = (col_x + p.x, top + py);
                if dy + ph < view.y || dy > view.y + view.h {
                    continue;
                }
                match &p.kind {
                    Kind::Rect { color, radius } => {
                        if *radius > 0.0 {
                            g.rrect(dx, dy, p.w, p.h, *radius, *color);
                        } else {
                            g.rect(dx, dy, p.w, p.h, *color);
                        }
                    }
                    Kind::Text { text, size, face, color, link } => {
                        g.text(dx, dy, *size, *color, text, *face);
                        if link.is_some() {
                            g.hit(R::new(dx, dy, p.w, p.h), Event::Tick, "link");
                        }
                    }
                    Kind::Image { idx, .. } => {
                        let alt = doc.images.get(*idx).map_or("", |i| i.alt.as_str());
                        g.rrect(dx, dy, p.w, p.h, 4.0 * s, [255, 255, 255, 16]);
                        let t = g.fit(12.0 * s, alt, Face::Sans, p.w - 8.0 * s);
                        g.text(dx + 4.0 * s, dy + 4.0 * s, 12.0 * s, DIM, &t, Face::Sans);
                    }
                    Kind::Control { idx } => {
                        if let Some(c) = doc.controls.get(*idx) {
                            let r = R::new(dx, dy, p.w, p.h);
                            let shown_sel = sel.get(&c.node).copied();
                            draw_control(g, c, r, s, focus == Some(*idx), edit, tick, *idx, light, shown_sel);
                        }
                    }
                }
            }
            // a slim scroll bar
            if self.content_h > view.h {
                let tr = R::new(view.x + view.w - 8.0 * s, view.y, 4.0 * s, view.h);
                let th = (tr.h * view.h / self.content_h).max(18.0 * s);
                let ty = tr.y + (tr.h - th) * self.scroll / (self.content_h - view.h);
                g.rrect(tr.x, ty, tr.w, th, 2.0 * s, [255, 255, 255, 70]);
            }
        } else {
            g.text(view.x + 24.0 * s, view.y + 24.0 * s, 14.0 * s, MUTED, "Starting…", Face::Sans);
        }
        if let Some((t, until)) = &self.toast {
            if tick_since(self.ticks, *until) {
                let w = g.text_width(14.0 * s, t, Face::Bold) + 28.0 * s;
                let tr = R::new(area.x + (area.w - w.min(area.w - 20.0 * s)) / 2.0, area.y + area.h - 52.0 * s, w.min(area.w - 20.0 * s), 34.0 * s);
                g.rrect(tr.x, tr.y, tr.w, tr.h, 17.0 * s, rgb(28, 36, 70));
                let ft = g.fit(14.0 * s, t, Face::Bold, tr.w - 24.0 * s);
                g.text(tr.x + 14.0 * s, tr.y + (tr.h - g.line_height(14.0 * s, Face::Bold)) / 2.0, 14.0 * s, TEXT, &ft, Face::Bold);
            } else {
                self.toast = None;
            }
        }
        // dialogs on top
        match &mut self.dialog {
            Dialog::None => {}
            Dialog::Info(c) | Dialog::Remove(c) | Dialog::Ask(_, c) | Dialog::Save(_, c) | Dialog::Clip(_, c) => c.render(g, area, s),
            Dialog::DeleteData(tf) => {
                let p = crate::fx::modal(g, area, s, 520.0 * s, 190.0 * s, ERR, true);
                let y = crate::fx::modal_title(g, p, s, "Delete its data too?", ERR);
                g.text(p.x + 26.0 * s, y + 4.0 * s, 14.0 * s, MUTED, &format!("This cannot be undone. Type {} and press Enter.", self.entry.name), Face::Sans);
                tf.render(g, R::new(p.x + 26.0 * s, y + 34.0 * s, p.w - 52.0 * s, 38.0 * s), true, s, blink(tick), Face::Sans, 7001);
            }
            Dialog::Open(_, p, _) => p.render(g, area, s, ACCENT),
        }
        let _ = cx;
    }
}

fn tick_since(now: u64, until: u64) -> bool {
    now < until
}

#[allow(clippy::too_many_arguments)]
fn draw_control(g: &mut Gfx, c: &Control, r: R, s: f32, focused: bool, edit: &TextField, tick: u64, idx: usize, light: bool, sel: Option<usize>) {
    let (field, dim, text_c, card, hot_c) = if light { (rgb(255, 255, 255), rgb(118, 118, 118), rgb(32, 33, 36), rgb(241, 243, 244), rgb(232, 234, 237)) } else { (crate::ui::FIELD, DIM, TEXT, crate::ui::CARD, crate::ui::CARD_HOVER) };
    let size = 16.0 * s;
    let lh = g.line_height(size, Face::Sans);
    let col = if c.disabled { dim } else { text_c };
    let id = CONTROL_BASE + idx;
    match c.kind {
        ControlKind::Text | ControlKind::Password => {
            if focused {
                edit.render(g, r, true, s, blink(tick), Face::Sans, id);
                return;
            }
            g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, field);
            g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, 1.0 * s, dim);
            let (shown, color) = if c.value.is_empty() { (c.placeholder.clone(), dim) } else if c.kind == ControlKind::Password { ("•".repeat(c.value.chars().count()), col) } else { (c.value.clone(), col) };
            let t = g.fit(size, &shown, Face::Sans, r.w - 24.0 * s);
            g.text(r.x + 12.0 * s, r.y + (r.h - lh) / 2.0, size, color, &t, Face::Sans);
            g.hit(r, Event::Focus(id), "field");
        }
        ControlKind::Textarea => {
            g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, field);
            g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, if focused { 2.0 * s } else { 1.0 * s }, if focused { ACCENT } else { dim });
            let text = if focused { edit.value.clone() } else { c.value.clone() };
            for (i, l) in g.wrap(size, &text, Face::Sans, r.w - 24.0 * s).iter().take(((r.h - 12.0 * s) / lh).max(1.0) as usize).enumerate() {
                g.text(r.x + 12.0 * s, r.y + 6.0 * s + i as f32 * lh, size, col, l, Face::Sans);
            }
            g.hit(r, Event::Focus(id), "text area");
        }
        ControlKind::Checkbox | ControlKind::Radio => {
            let round = c.kind == ControlKind::Radio;
            if round {
                g.circle(r.x + r.w / 2.0, r.y + r.h / 2.0, r.w / 2.0, dim);
                g.circle(r.x + r.w / 2.0, r.y + r.h / 2.0, r.w / 2.0 - 1.5 * s, field);
                if c.checked {
                    g.circle(r.x + r.w / 2.0, r.y + r.h / 2.0, r.w / 4.0, ACCENT);
                }
            } else {
                g.rrect(r.x, r.y, r.w, r.h, 4.0 * s, field);
                g.rrect_outline(r.x, r.y, r.w, r.h, 4.0 * s, 1.5 * s, if c.checked { ACCENT } else { dim });
                if c.checked {
                    let tw = g.text_width(size, "✓", Face::Bold);
                    g.text(r.x + (r.w - tw) / 2.0, r.y + (r.h - lh) / 2.0, size, ACCENT, "✓", Face::Bold);
                }
            }
            if focused {
                g.rrect_outline(r.x - 2.0 * s, r.y - 2.0 * s, r.w + 4.0 * s, r.h + 4.0 * s, 6.0 * s, 2.0 * s, ACCENT);
            }
            g.hit(r, Event::Tab(id), "checkbox");
        }
        ControlKind::Button | ControlKind::Submit => {
            let hot = g.hovered(r);
            g.rrect(r.x, r.y, r.w, r.h, r.h / 2.0, if hot { hot_c } else { card });
            g.rrect_outline(r.x, r.y, r.w, r.h, r.h / 2.0, 1.5 * s, if c.kind == ControlKind::Submit { ACCENT } else { dim });
            let label = g.fit(size, &c.label, Face::Bold, r.w - 12.0 * s);
            let tw = g.text_width(size, &label, Face::Bold);
            g.text(r.x + (r.w - tw) / 2.0, r.y + (r.h - lh) / 2.0, size, col, &label, Face::Bold);
            if focused {
                g.rrect_outline(r.x - 2.0 * s, r.y - 2.0 * s, r.w + 4.0 * s, r.h + 4.0 * s, r.h / 2.0 + 2.0, 2.0 * s, ACCENT);
            }
            g.hit(r, Event::Tab(id), &c.label);
        }
        ControlKind::Select => {
            g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, field);
            g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, if focused { 2.0 * s } else { 1.0 * s }, if focused { ACCENT } else { dim });
            let i = sel.or_else(|| c.options.iter().position(|o| o.selected)).unwrap_or(0);
            let label = c.options.get(i).map_or(String::new(), |o| o.label.clone());
            let t = g.fit(size, &label, Face::Sans, r.w - 40.0 * s);
            g.text(r.x + 12.0 * s, r.y + (r.h - lh) / 2.0, size, col, &t, Face::Sans);
            g.text(r.x + r.w - 24.0 * s, r.y + (r.h - lh) / 2.0, size, MUTED, "▾", Face::Sans);
            g.hit(r, Event::Tab(id), "drop-down (click for the next choice)");
        }
        ControlKind::Hidden => {}
    }
}

/// The launcher's tile and the Open button of the store make an app the same way.
pub fn open_entry(entry: TileEntry, cx: &AppCx) -> Box<dyn App> {
    Box::new(TileApp::new(entry, cx))
}
