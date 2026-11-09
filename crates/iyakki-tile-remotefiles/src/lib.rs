//! REMOTE FILES: an SFTP / FTPS client. Saved connections (the password or key is in the vault, never in a settings file), the server's
//! identity shown on a card to confirm the first time and refused loudly when it changes, a folder browser with sorting, upload and
//! download with progress and resume, rename, new folder and delete (after asking), and a transfer queue with cancel.
//! The tile never touches the network: it submits jobs to a `RemoteSource` and polls for the answers.

use iyakki_tile_api::remote::{clean_path, join, parent, safe_name, Auth, Conn, Entry, HostCard, Job, JobResult, Opened, Proto, State, TransferInfo, TransferReq};
use iyakki_tile_api::{App, Flow, TileCx};
use iyakki_uikit::blink::blink;
use iyakki_uikit::filepicker::{current_scope, human_size, FilePicker, PathKind, PickOut};
use iyakki_uikit::gfx::{rgb, Face, Gfx, Icon};
use iyakki_uikit::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};
use iyakki_uikit::widgets::{render_confirm, Form, FormEvent};

pub const ACCENT: [u8; 4] = rgb(170, 120, 200);
const ROW_H: f32 = 30.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortBy {
    Name,
    Size,
    Modified,
}

enum PendKind {
    Open(String),
    List(String),
    /// A change on the server; list this folder again afterwards.
    Change(String),
}

struct Pending {
    ticket: u64,
    kind: PendKind,
}

enum Ask {
    Delete(Entry),
    Overwrite(TransferReq),
    ForgetPin(String),
    RemoveConn(String),
}

enum Screen {
    List,
    ConnForm(Box<Form>, Option<String>),
    Card(HostCard, String),
    Browse,
    Transfers,
    Name(Box<Form>, NameFor),
    Pick(Box<FilePicker>, bool),
    Confirm(Ask),
}

enum NameFor {
    Mkdir,
    Rename(String),
}

pub struct RemoteFilesApp {
    screen: Screen,
    pending: Option<Pending>,
    sel: usize,
    first: usize,
    // the browser
    session: Option<u64>,
    conn: String,
    path: String,
    entries: Vec<Entry>,
    sort: SortBy,
    ascending: bool,
    // transfers (refreshed about twice a second)
    transfers: Vec<TransferInfo>,
    tsel: usize,
    note: Option<(String, bool)>,
    last_poll: u64,
    last_click: Option<(usize, u64)>,
    tick: u64,
}

impl Default for RemoteFilesApp {
    fn default() -> Self {
        Self::new()
    }
}

fn fmt_rate(b: u64) -> String {
    format!("{}/s", human_size(b))
}

fn date(t: i64) -> String {
    if t <= 0 {
        return String::new();
    }
    iyakki_uikit::filepicker::date_text(Some(std::time::UNIX_EPOCH + std::time::Duration::from_secs(t as u64)))
}

impl RemoteFilesApp {
    pub fn new() -> RemoteFilesApp {
        RemoteFilesApp { screen: Screen::List, pending: None, sel: 0, first: 0, session: None, conn: String::new(), path: "/".into(), entries: vec![], sort: SortBy::Name, ascending: true, transfers: vec![], tsel: 0, note: None, last_poll: 0, last_click: None, tick: 0 }
    }

    pub fn path(&self) -> &str {
        &self.path
    }
    pub fn connected(&self) -> bool {
        self.session.is_some()
    }
    pub fn names(&self) -> Vec<String> {
        self.entries.iter().map(|e| e.name.clone()).collect()
    }
    pub fn in_browser(&self) -> bool {
        matches!(self.screen, Screen::Browse)
    }
    pub fn on_card(&self) -> Option<&HostCard> {
        match &self.screen {
            Screen::Card(c, _) => Some(c),
            _ => None,
        }
    }
    pub fn sort_state(&self) -> (SortBy, bool) {
        (self.sort, self.ascending)
    }

    fn say(&mut self, t: impl Into<String>, ok: bool) {
        self.note = Some((t.into(), ok));
    }

    fn sort_entries(&mut self) {
        let (by, asc) = (self.sort, self.ascending);
        self.entries.sort_by(|a, b| {
            let dirs = b.is_dir.cmp(&a.is_dir);
            let by_name = a.name.to_lowercase().cmp(&b.name.to_lowercase());
            let main = match by {
                SortBy::Name => by_name,
                SortBy::Size if a.is_dir && b.is_dir => by_name,
                SortBy::Size => a.size.cmp(&b.size).then(by_name),
                SortBy::Modified => a.modified.cmp(&b.modified).then(by_name),
            };
            dirs.then(if asc { main } else { main.reverse() })
        });
    }

    fn set_sort(&mut self, by: SortBy) {
        if self.sort == by {
            self.ascending = !self.ascending;
        } else {
            self.sort = by;
            self.ascending = true;
        }
        let keep = self.selected().map(|e| e.name.clone());
        self.sort_entries();
        if let Some(n) = keep {
            self.sel = self.entries.iter().position(|e| e.name == n).unwrap_or(0);
        }
    }

    fn selected(&self) -> Option<&Entry> {
        self.entries.get(self.sel)
    }

    fn submit(&mut self, cx: &TileCx, job: Job, kind: PendKind) {
        let ticket = cx.remote.submit(job);
        self.pending = Some(Pending { ticket, kind });
        self.poll(cx);
    }

    fn go(&mut self, cx: &TileCx, path: &str) {
        let Some(s) = self.session else { return };
        let p = clean_path(path);
        self.submit(cx, Job::List(s, p.clone()), PendKind::List(p));
    }

    fn open_connection(&mut self, cx: &TileCx, name: &str) {
        self.conn = name.to_string();
        self.submit(cx, Job::Open(name.to_string()), PendKind::Open(name.to_string()));
    }

    /// Take a finished answer, if there is one.
    fn poll(&mut self, cx: &TileCx) {
        let Some(p) = self.pending.as_ref() else { return };
        let Some(r) = cx.remote.take(p.ticket) else { return };
        let p = self.pending.take().expect("checked above");
        match (p.kind, r) {
            (PendKind::Open(name), JobResult::Opened(o)) => match o {
                Opened::Ready(id) => {
                    self.session = Some(id);
                    self.screen = Screen::Browse;
                    self.entries.clear();
                    self.sel = 0;
                    self.first = 0;
                    self.note = None;
                    self.go(cx, "/");
                }
                Opened::Unknown(card) | Opened::Changed(card) => {
                    self.screen = Screen::Card(card, name);
                }
            },
            (PendKind::List(want), JobResult::Listing(path, entries)) => {
                let _ = want;
                self.path = clean_path(&path);
                self.entries = entries;
                self.sort_entries();
                self.sel = 0;
                self.first = 0;
            }
            (PendKind::Change(dir), JobResult::Done(msg)) => {
                self.say(msg, true);
                self.go(cx, &dir);
            }
            (_, JobResult::Failed(e)) => {
                if matches!(self.screen, Screen::Browse) || self.session.is_some() {
                    self.say(e, false);
                } else {
                    self.screen = Screen::List;
                    self.say(e, false);
                }
            }
            (_, _) => self.say("the answer did not match the request", false),
        }
    }

    fn refresh_transfers(&mut self, cx: &TileCx) {
        self.transfers = cx.remote.transfers();
        self.tsel = self.tsel.min(self.transfers.len().saturating_sub(1));
    }

    fn disconnect(&mut self, cx: &TileCx) {
        if let Some(s) = self.session.take() {
            cx.remote.close(s);
        }
        self.screen = Screen::List;
        self.entries.clear();
        self.path = "/".into();
        self.sel = 0;
        self.first = 0;
    }

    fn conn_form(&self, cx: &TileCx, edit: Option<&Conn>) -> Form {
        let keys = cx.remote.key_names();
        let mut key_opts: Vec<&str> = vec!["(none)"];
        key_opts.extend(keys.iter().map(|k| k.as_str()));
        let proto = edit.map_or(0, |c| Proto::ALL.iter().position(|p| *p == c.proto).unwrap_or(0));
        let key_sel = edit.and_then(|c| keys.iter().position(|k| *k == c.key)).map_or(0, |i| i + 1);
        let mut f = Form::new(if edit.is_some() { "Edit connection" } else { "New connection" })
            .with_note("The password or key stays in your vault, never in a settings file. Leave the password empty when editing to keep the one stored. Keys are made in the CLI tile.")
            .text("Name", edit.map_or("", |c| c.name.as_str()), "a short name, such as office-nas")
            .choice("Protocol", &["SFTP (over SSH)", "FTPS (FTP with TLS)", "FTP (plain, not encrypted)"], proto)
            .text("Host", edit.map_or("", |c| c.host.as_str()), "name or address")
            .text("Port", &edit.map_or(String::new(), |c| c.port.to_string()), "empty: the protocol's usual port")
            .text("User", edit.map_or("", |c| c.user.as_str()), "login name")
            .choice("Sign in with", &["Password", "Key from the vault"], edit.map_or(0, |c| usize::from(c.auth == Auth::Key)))
            .secret("Password", if edit.is_some_and(|c| c.has_password) { "stored; empty keeps it" } else { "kept in the vault" })
            .choice("Vault key", &key_opts, key_sel)
            .choice("Plain FTP", &["I do not accept sending everything unencrypted", "I accept: my password and files may be read on the way"], usize::from(edit.is_some_and(|c| c.plain_ok)));
        f.focus = 0;
        f
    }

    fn conn_from_form(f: &Form, keys: &[String]) -> Result<(Conn, Option<String>), String> {
        let proto = Proto::ALL[f.choice_idx(1).min(2)];
        let port = {
            let t = f.value(3);
            if t.trim().is_empty() { proto.default_port() } else { t.trim().parse::<u16>().map_err(|_| "the port is a number from 1 to 65535".to_string())? }
        };
        let auth = if f.choice_idx(5) == 1 { Auth::Key } else { Auth::Password };
        let key = match f.choice_idx(7) {
            0 => String::new(),
            n => keys.get(n - 1).cloned().unwrap_or_default(),
        };
        if auth == Auth::Key && (key.is_empty() || proto != Proto::Sftp) {
            return Err(if proto != Proto::Sftp { "keys are for SFTP; FTP and FTPS use a password".into() } else { "choose a key from your vault, or sign in with a password".to_string() });
        }
        let plain_ok = f.choice_idx(8) == 1;
        if proto == Proto::Ftp && !plain_ok {
            return Err("Plain FTP sends your password and files unencrypted. Choose SFTP or FTPS, or accept the warning in the last field.".into());
        }
        let pw = f.value(6);
        let conn = Conn { name: f.value(0).trim().to_string(), host: f.value(2).trim().to_string(), port, user: f.value(4).trim().to_string(), proto, auth, key, plain_ok: proto == Proto::Ftp && plain_ok, has_password: false };
        Ok((conn, if pw.is_empty() { None } else { Some(pw) }))
    }

    fn start_download(&mut self, cx: &TileCx, e: &Entry, dest: Option<String>) {
        let Some(s) = self.session else { return };
        if e.is_dir {
            self.say("Open the folder and pick the files in it (folders are not downloaded whole).", false);
            return;
        }
        let Some(name) = safe_name(&e.name) else {
            self.say("That file name cannot be used on this computer.", false);
            return;
        };
        let dir = dest.unwrap_or_else(|| cx.remote.download_dir());
        let local = join(&dir, &name);
        let req = TransferReq { session: s, download: true, remote: join(&self.path, &e.name), local, overwrite: false };
        match cx.remote.start_transfer(&req) {
            Ok(_) => self.say(format!("Downloading {} (press t for the queue)", e.name), true),
            Err(err) => self.say(err, false),
        }
    }

    fn start_upload(&mut self, cx: &TileCx, local: &str) {
        let Some(s) = self.session else { return };
        let Some(base) = local.rsplit('/').next().and_then(safe_name) else {
            self.say("That file has no usable name.", false);
            return;
        };
        let req = TransferReq { session: s, download: false, remote: join(&self.path, &base), local: local.to_string(), overwrite: false };
        if self.entries.iter().any(|e| e.name == base) {
            self.screen = Screen::Confirm(Ask::Overwrite(req));
            return;
        }
        self.queue(cx, &req);
    }

    fn queue(&mut self, cx: &TileCx, req: &TransferReq) {
        match cx.remote.start_transfer(req) {
            Ok(_) => self.say(if req.download { "Download queued (t shows the queue)" } else { "Upload queued (t shows the queue)" }, true),
            Err(e) => self.say(e, false),
        }
    }

    fn handle_list(&mut self, ev: &Event, cx: &TileCx) -> Flow {
        let conns = cx.remote.connections();
        let n = conns.len();
        match ev {
            Event::Key(Key::Esc) | Event::Key(Key::Char('q')) => return Flow::Back,
            Event::Key(Key::Up) => self.sel = self.sel.saturating_sub(1),
            Event::Key(Key::Down) => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Event::Row(i) if *i < n => {
                let second = self.last_click == Some((*i, self.tick / 20));
                self.sel = *i;
                self.last_click = Some((*i, self.tick / 20));
                if second {
                    self.open_connection(cx, &conns[*i].name);
                }
            }
            Event::Key(Key::Enter) => {
                if let Some(c) = conns.get(self.sel) {
                    self.open_connection(cx, &c.name);
                }
            }
            Event::Key(Key::Char('n')) | Event::Tab(30) => self.screen = Screen::ConnForm(Box::new(self.conn_form(cx, None)), None),
            Event::Key(Key::Char('e')) => {
                if let Some(c) = conns.get(self.sel) {
                    self.screen = Screen::ConnForm(Box::new(self.conn_form(cx, Some(c))), Some(c.name.clone()));
                }
            }
            Event::Key(Key::Char('d')) | Event::Key(Key::Delete) => {
                if let Some(c) = conns.get(self.sel) {
                    self.screen = Screen::Confirm(Ask::RemoveConn(c.name.clone()));
                }
            }
            Event::Key(Key::Char('t')) => {
                self.refresh_transfers(cx);
                self.screen = Screen::Transfers;
            }
            _ => {}
        }
        Flow::Stay
    }

    fn handle_browse(&mut self, ev: &Event, cx: &TileCx) -> Flow {
        let n = self.entries.len();
        match ev {
            Event::Key(Key::Esc) | Event::Key(Key::Char('q')) => self.disconnect(cx),
            Event::Key(Key::Up) => self.sel = self.sel.saturating_sub(1),
            Event::Key(Key::Down) => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Event::Key(Key::PageUp) => self.sel = self.sel.saturating_sub(10),
            Event::Key(Key::PageDown) => self.sel = (self.sel + 10).min(n.saturating_sub(1)),
            Event::Key(Key::Home) => self.sel = 0,
            Event::Key(Key::End) => self.sel = n.saturating_sub(1),
            Event::Scroll(d) => self.sel = if *d > 0 { self.sel.saturating_sub(3) } else { (self.sel + 3).min(n.saturating_sub(1)) },
            Event::Key(Key::Left) | Event::Key(Key::Backspace) | Event::Tab(40) => {
                let up = parent(&self.path);
                if up != self.path {
                    self.go(cx, &up);
                }
            }
            Event::Row(i) if *i < n => {
                let second = self.last_click == Some((*i, self.tick / 20));
                self.sel = *i;
                self.last_click = Some((*i, self.tick / 20));
                if second {
                    return self.handle_browse(&Event::Key(Key::Enter), cx);
                }
            }
            Event::Key(Key::Enter) | Event::Key(Key::Right) => {
                if let Some(e) = self.selected().cloned() {
                    if e.is_dir {
                        self.go(cx, &join(&self.path, &e.name));
                    } else if matches!(ev, Event::Key(Key::Enter)) {
                        self.start_download(cx, &e, None);
                    }
                }
            }
            Event::Key(Key::Char('1')) | Event::Tab(41) => self.set_sort(SortBy::Name),
            Event::Key(Key::Char('2')) | Event::Tab(42) => self.set_sort(SortBy::Size),
            Event::Key(Key::Char('3')) | Event::Tab(43) => self.set_sort(SortBy::Modified),
            Event::Key(Key::F(5)) | Event::Tab(44) => {
                let p = self.path.clone();
                self.go(cx, &p);
            }
            Event::Key(Key::Char('d')) | Event::Tab(45) => {
                if let Some(e) = self.selected().cloned() {
                    self.start_download(cx, &e, None);
                }
            }
            Event::Key(Key::Char('D')) => {
                if self.selected().is_some_and(|e| !e.is_dir) {
                    self.screen = Screen::Pick(Box::new(FilePicker::new(PathKind::Folder, &[], "Download to which folder?", &cx.remote.download_dir(), current_scope())), true);
                }
            }
            Event::Key(Key::Char('u')) | Event::Tab(46) => {
                self.screen = Screen::Pick(Box::new(FilePicker::new(PathKind::File, &[], "Upload which file?", "", current_scope())), false);
            }
            Event::Key(Key::Char('n')) | Event::Tab(47) => {
                self.screen = Screen::Name(Box::new(Form::new("New folder").text("Name", "", "folder name")), NameFor::Mkdir);
            }
            Event::Key(Key::Char('m')) | Event::Key(Key::F(2)) | Event::Tab(48) => {
                if let Some(e) = self.selected() {
                    self.screen = Screen::Name(Box::new(Form::new("Rename").text("New name", &e.name, "a name, not a path")), NameFor::Rename(e.name.clone()));
                }
            }
            Event::Key(Key::Delete) | Event::Key(Key::Char('x')) | Event::Tab(49) => {
                if let Some(e) = self.selected().cloned() {
                    self.screen = Screen::Confirm(Ask::Delete(e));
                }
            }
            Event::Key(Key::Char('t')) | Event::Tab(50) => {
                self.refresh_transfers(cx);
                self.screen = Screen::Transfers;
            }
            _ => {}
        }
        Flow::Stay
    }

    fn handle_transfers(&mut self, ev: &Event, cx: &TileCx) {
        let n = self.transfers.len();
        match ev {
            Event::Key(Key::Esc) | Event::Key(Key::Char('q')) => self.screen = if self.session.is_some() { Screen::Browse } else { Screen::List },
            Event::Key(Key::Up) => self.tsel = self.tsel.saturating_sub(1),
            Event::Key(Key::Down) => self.tsel = (self.tsel + 1).min(n.saturating_sub(1)),
            Event::Row(i) if *i < n => self.tsel = *i,
            Event::Key(Key::Char('c')) => {
                if let Some(t) = self.transfers.get(self.tsel) {
                    cx.remote.cancel_transfer(t.id);
                }
            }
            Event::Key(Key::Char('r')) => {
                if let Some(t) = self.transfers.get(self.tsel).filter(|t| matches!(t.state, State::Failed | State::Cancelled)) {
                    match cx.remote.retry_transfer(t.id) {
                        Ok(_) => self.say("Continuing from the bytes already there", true),
                        Err(e) => self.say(e, false),
                    }
                }
            }
            Event::Key(Key::Char('o')) => {
                if let (Some(t), Some(s)) = (self.transfers.get(self.tsel), self.session) {
                    if t.state == State::Failed {
                        let req = TransferReq { session: s, download: t.download, remote: t.remote.clone(), local: t.local.clone(), overwrite: true };
                        self.screen = Screen::Confirm(Ask::Overwrite(req));
                    }
                }
            }
            Event::Key(Key::Char('x')) => cx.remote.clear_finished(),
            _ => {}
        }
        self.refresh_transfers(cx);
    }
}

impl App for RemoteFilesApp {
    fn title(&self) -> &'static str {
        "Remote files"
    }
    fn icon(&self) -> Icon {
        Icon::Data
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        match self.screen {
            Screen::List => "Enter connect · n new · e edit · d remove · t transfers · Esc back".into(),
            Screen::Browse => "Enter open or download · u upload · d download · m rename · n new folder · x delete · 1 2 3 sort · t transfers · Esc disconnect".into(),
            Screen::Transfers => "c cancel · r resume · o replace · x clear finished · Esc back".into(),
            _ => "Enter confirm · Esc cancel".into(),
        }
    }
    fn help(&self) -> Vec<String> {
        [
            "Saved connections: n adds one (SFTP, FTPS or plain FTP), e edits, d removes. The password or key is kept in your vault.",
            "The first time you connect, the server's identity (a fingerprint) is shown. Compare it with the one the server's owner gave you, then press y to trust it.",
            "If a trusted server later shows a different identity the connection is refused. Only forget the old one (f) if you know the server was reinstalled.",
            "In a folder: Enter opens a folder or downloads a file, Left or Backspace goes up, u uploads a file, d downloads, D downloads into a folder you pick.",
            "m renames, n makes a folder, x deletes (after asking; folders must be empty). 1 2 3 sort by name, size, date; press again to reverse. F5 reloads.",
            "t shows the transfer queue: c cancels, r continues a stopped transfer from the bytes already there, o replaces an existing file, x clears finished ones.",
            "Plain FTP sends everything unencrypted and is only available after you accept that on the connection.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn handle(&mut self, ev: &Event, cx: &TileCx) -> Flow {
        if matches!(ev, Event::Tick) {
            self.tick += 1;
        }
        self.poll(cx);
        if self.pending.is_some() {
            // waiting for the server: Esc stops waiting (the answer is dropped)
            if matches!(ev, Event::Key(Key::Esc)) {
                self.pending = None;
                if self.session.is_none() {
                    self.screen = Screen::List;
                }
                self.say("Stopped waiting.", false);
            }
            return Flow::Stay;
        }
        let screen = std::mem::replace(&mut self.screen, Screen::List);
        match screen {
            Screen::List => {
                self.screen = Screen::List;
                return self.handle_list(ev, cx);
            }
            Screen::Browse => {
                self.screen = Screen::Browse;
                return self.handle_browse(ev, cx);
            }
            Screen::Transfers => {
                self.screen = Screen::Transfers;
                self.handle_transfers(ev, cx);
            }
            Screen::ConnForm(mut f, replacing) => match f.handle_event(ev) {
                FormEvent::Cancel => self.screen = Screen::List,
                FormEvent::Submit => {
                    let keys = cx.remote.key_names();
                    match Self::conn_from_form(&f, &keys) {
                        Err(e) => {
                            f.error = Some(e);
                            self.screen = Screen::ConnForm(f, replacing);
                        }
                        Ok((conn, pw)) => match cx.remote.save(&conn, pw.as_deref(), replacing.as_deref()) {
                            Ok(()) => {
                                self.screen = Screen::List;
                                self.say(format!("Saved {}", conn.name), true);
                            }
                            Err(e) => {
                                f.error = Some(e);
                                self.screen = Screen::ConnForm(f, replacing);
                            }
                        },
                    }
                }
                _ => self.screen = Screen::ConnForm(f, replacing),
            },
            Screen::Card(card, name) => {
                let changed = card.pinned.is_some();
                match ev {
                    Event::Key(Key::Char('y')) if !changed => match cx.remote.trust(&name, &card.fingerprint) {
                        Ok(()) => {
                            self.screen = Screen::List;
                            self.open_connection(cx, &name);
                        }
                        Err(e) => {
                            self.screen = Screen::List;
                            self.say(e, false);
                        }
                    },
                    Event::Key(Key::Char('f')) if changed => self.screen = Screen::Confirm(Ask::ForgetPin(name)),
                    Event::Key(Key::Esc) | Event::Key(Key::Char('n')) => self.screen = Screen::List,
                    _ => self.screen = Screen::Card(card, name),
                }
            }
            Screen::Name(mut f, what) => match f.handle_event(ev) {
                FormEvent::Cancel => self.screen = Screen::Browse,
                FormEvent::Submit => {
                    let name = f.value(0).trim().to_string();
                    if name.is_empty() || name.contains('/') || name == "." || name == ".." || name.chars().any(char::is_control) {
                        f.error = Some("a name, not a path: no slashes, not . or ..".into());
                        self.screen = Screen::Name(f, what);
                    } else if let Some(s) = self.session {
                        self.screen = Screen::Browse;
                        let dir = self.path.clone();
                        match what {
                            NameFor::Mkdir => self.submit(cx, Job::Mkdir(s, join(&dir, &name)), PendKind::Change(dir)),
                            NameFor::Rename(old) => self.submit(cx, Job::Rename(s, join(&dir, &old), join(&dir, &name)), PendKind::Change(dir)),
                        }
                    }
                }
                _ => self.screen = Screen::Name(f, what),
            },
            Screen::Pick(mut p, download) => match p.handle_event(ev) {
                PickOut::Stay => self.screen = Screen::Pick(p, download),
                PickOut::Cancel => self.screen = Screen::Browse,
                PickOut::Chosen(path) => {
                    self.screen = Screen::Browse;
                    let path = path.to_string_lossy().into_owned();
                    if download {
                        if let Some(e) = self.selected().cloned() {
                            self.start_download(cx, &e, Some(path));
                        }
                    } else {
                        self.start_upload(cx, &path);
                    }
                }
            },
            Screen::Confirm(ask) => {
                let back = if matches!(ask, Ask::RemoveConn(_) | Ask::ForgetPin(_)) { Screen::List } else if matches!(ask, Ask::Overwrite(_)) && self.session.is_none() { Screen::List } else { Screen::Browse };
                match ev {
                    Event::Key(Key::Char('y')) => {
                        self.screen = back;
                        match ask {
                            Ask::Delete(e) => {
                                if let Some(s) = self.session {
                                    let dir = self.path.clone();
                                    self.submit(cx, Job::Delete(s, join(&dir, &e.name), e.is_dir), PendKind::Change(dir));
                                }
                            }
                            Ask::Overwrite(mut req) => {
                                req.overwrite = true;
                                self.queue(cx, &req);
                            }
                            Ask::ForgetPin(name) => match cx.remote.forget_host(&name) {
                                Ok(()) => self.say("The old identity is forgotten. The next connection will ask you to confirm the new one.", true),
                                Err(e) => self.say(e, false),
                            },
                            Ask::RemoveConn(name) => match cx.remote.remove(&name) {
                                Ok(()) => {
                                    self.sel = 0;
                                    self.say(format!("Removed {name} (its password is removed from the vault)"), true)
                                }
                                Err(e) => self.say(e, false),
                            },
                        }
                    }
                    Event::Key(Key::Esc) | Event::Key(Key::Char('n')) => self.screen = back,
                    _ => self.screen = Screen::Confirm(ask),
                }
            }
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &TileCx, tick: u64) {
        self.tick = self.tick.max(tick);
        self.poll(cx);
        if tick.wrapping_sub(self.last_poll) >= 15 || self.last_poll > tick {
            self.last_poll = tick;
            self.refresh_transfers(cx);
        }
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        // `Screen` is borrowed for drawing; the modal ones draw over the background screen
        match &mut self.screen {
            Screen::List => {
                let conns = cx.remote.connections();
                self.draw_list(g, area, s, &conns);
            }
            Screen::Browse => self.draw_browse(g, area, s),
            Screen::Transfers => self.draw_transfers(g, area, s),
            Screen::ConnForm(f, _) => f.render(g, area, s, ACCENT, blink(tick)),
            Screen::Name(f, _) => f.render(g, area, s, ACCENT, blink(tick)),
            Screen::Pick(p, _) => p.render(g, area, s, ACCENT, blink(tick)),
            Screen::Card(card, name) => {
                let (card, name) = (card.clone(), name.clone());
                draw_card(g, area, s, &card, &name);
            }
            Screen::Confirm(ask) => {
                let (text, yes) = match ask {
                    Ask::Delete(e) => (format!("Delete the {} “{}” on the server?", if e.is_dir { "empty folder" } else { "file" }, e.name), "Delete"),
                    Ask::Overwrite(r) => (format!("Replace “{}” {}?", r.remote.rsplit('/').next().unwrap_or(""), if r.download { "on this computer" } else { "on the server" }), "Replace"),
                    Ask::ForgetPin(n) => (format!("Forget the pinned identity of “{n}”? Do this only if you know the server was reinstalled."), "Forget"),
                    Ask::RemoveConn(n) => (format!("Remove the connection “{n}” and its stored password?"), "Remove"),
                };
                render_confirm(g, area, s, &text, yes);
            }
        }
        if self.pending.is_some() {
            let dots = ".".repeat(1 + (tick / 10 % 3) as usize);
            g.text(x, area.y + area.h - 28.0 * s, 14.0 * s, ACCENT, &format!("Working{dots}  (Esc stops waiting)"), Face::Bold);
        } else if let Some((t, ok)) = &self.note {
            if !matches!(self.screen, Screen::Card(..) | Screen::Confirm(_) | Screen::ConnForm(..) | Screen::Name(..) | Screen::Pick(..)) {
                let t = g.fit(14.0 * s, t, Face::Sans, w);
                g.text(x, area.y + area.h - 28.0 * s, 14.0 * s, if *ok { OK } else { ERR }, &t, Face::Sans);
            }
        }
    }
}

fn draw_card(g: &mut Gfx, area: R, s: f32, c: &HostCard, name: &str) {
    let changed = c.pinned.is_some();
    let p = iyakki_uikit::fx::modal(g, area, s, 680.0 * s, if changed { 340.0 } else { 300.0 } * s, ACCENT, changed);
    let mut y = iyakki_uikit::fx::modal_title(g, p, s, if changed { "THE SERVER'S IDENTITY HAS CHANGED" } else { "Is this the right server?" }, if changed { ERR } else { ACCENT });
    let x = p.x + 26.0 * s;
    let lw = p.w - 52.0 * s;
    let lines: Vec<String> = if changed {
        vec![
            format!("The connection “{name}” to {}:{} was REFUSED. The server now shows a different {} than the one you trusted.", c.host, c.port, c.kind),
            "This happens when the server was reinstalled, but also when someone is listening in between. Nothing was sent to it: no password, no file.".into(),
        ]
    } else {
        vec![format!("First contact with {}:{} for “{name}”. Compare this fingerprint with the one the server's owner gave you.", c.host, c.port)]
    };
    for l in lines {
        for w in g.wrap(14.0 * s, &l, Face::Sans, lw) {
            g.text(x, y, 14.0 * s, if changed { TEXT } else { MUTED }, &w, Face::Sans);
            y += 20.0 * s;
        }
    }
    y += 6.0 * s;
    g.text(x, y, 12.0 * s, DIM, &format!("NOW ({})", c.kind), Face::Bold);
    y += 16.0 * s;
    let fp = g.fit(15.0 * s, &c.fingerprint, Face::Mono, lw);
    g.text(x, y, 15.0 * s, if changed { ERR } else { TEXT }, &fp, Face::Mono);
    y += 26.0 * s;
    if let Some(old) = &c.pinned {
        g.text(x, y, 12.0 * s, DIM, "PINNED BEFORE", Face::Bold);
        y += 16.0 * s;
        let fp = g.fit(15.0 * s, old, Face::Mono, lw);
        g.text(x, y, 15.0 * s, TEXT, &fp, Face::Mono);
    }
    let by = p.y + p.h - 58.0 * s;
    if changed {
        g.button(x, by, 14.0 * s, "Close", ERR, Event::Key(Key::Esc));
        g.button(x + 140.0 * s, by, 14.0 * s, "Forget the old identity…", MUTED, Event::Key(Key::Char('f')));
    } else {
        g.button(x, by, 14.0 * s, "Trust and connect (y)", ACCENT, Event::Key(Key::Char('y')));
        g.button(x + 230.0 * s, by, 14.0 * s, "Cancel", MUTED, Event::Key(Key::Esc));
    }
}

impl RemoteFilesApp {
    fn draw_list(&mut self, g: &mut Gfx, area: R, s: f32, conns: &[Conn]) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let y = area.y + 10.0 * s;
        let bar = [("New connection".to_string(), ACCENT, Event::Tab(30)), ("Transfers (t)".to_string(), MUTED, Event::Key(Key::Char('t')))];
        let (bar_h, _) = g.button_flow(x, y, w, 14.0 * s, 8.0 * s, &bar);
        let top = y + bar_h.max(38.0 * s) + 10.0 * s;
        if conns.is_empty() {
            iyakki_uikit::fx::empty_state(g, R::new(x, top, w, area.h - 100.0 * s), s, Icon::Data, "No saved connections", "Press n to add an SFTP, FTPS or FTP server. Passwords and keys stay in your vault.", ACCENT);
            return;
        }
        self.sel = self.sel.min(conns.len() - 1);
        for (i, c) in conns.iter().enumerate() {
            let r = R::new(x, top + i as f32 * 54.0 * s, w, 48.0 * s);
            if r.y + r.h > area.y + area.h - 36.0 * s {
                break;
            }
            let hot = g.hovered(r);
            iyakki_uikit::fx::list_row(g, r, s, i == self.sel, hot, ACCENT);
            g.hit(r, Event::Row(i), &c.name);
            g.text(r.x + 16.0 * s, r.y + 6.0 * s, 16.0 * s, TEXT, &c.name, Face::Bold);
            let sub = format!("{} · {}@{}:{} · {}", c.proto.label(), if c.user.is_empty() { "?" } else { &c.user }, c.host, c.port, if c.auth == Auth::Key { format!("key {}", c.key) } else if c.has_password { "password in the vault".into() } else { "no password stored".into() });
            let sub = g.fit(12.0 * s, &sub, Face::Sans, w - 40.0 * s);
            g.text(r.x + 16.0 * s, r.y + 28.0 * s, 12.0 * s, if c.proto == Proto::Ftp { ERR } else { MUTED }, &sub, Face::Sans);
        }
    }

    fn draw_browse(&mut self, g: &mut Gfx, area: R, s: f32) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 8.0 * s;
        let mut bx = x;
        for (label, ev) in [("Up", Event::Tab(40)), ("Upload", Event::Tab(46)), ("Download", Event::Tab(45)), ("Folder", Event::Tab(47)), ("Rename", Event::Tab(48)), ("Delete", Event::Tab(49)), ("Reload", Event::Tab(44)), ("Queue", Event::Tab(50))] {
            bx += g.button(bx, y, 13.0 * s, label, if label == "Delete" { ERR } else { ACCENT }, ev) + 8.0 * s;
        }
        y += 40.0 * s;
        let p = g.fit(15.0 * s, &format!("{}  ·  {}", self.conn, self.path), Face::Mono, w);
        g.text(x, y, 15.0 * s, TEXT, &p, Face::Mono);
        y += 26.0 * s;
        // headers: click to sort
        let arrow = |by: SortBy, me: &Self| if me.sort == by { if me.ascending { " ▲" } else { " ▼" } } else { "" };
        let cols = [("Name", 0.0, 41, SortBy::Name), ("Size", w - 330.0 * s, 42, SortBy::Size), ("Modified", w - 200.0 * s, 43, SortBy::Modified)];
        for (label, off, tab, by) in cols {
            let t = format!("{}{}", label.to_uppercase(), arrow(by, self));
            let tw = g.text_width(11.0 * s, &t, Face::Bold);
            g.text(x + 12.0 * s + off, y, 11.0 * s, DIM, &t, Face::Bold);
            g.hit(R::new(x + 12.0 * s + off, y - 2.0 * s, tw + 8.0 * s, 18.0 * s), Event::Tab(tab), label);
        }
        y += 22.0 * s;
        let bottom = area.y + area.h - 56.0 * s;
        let rows = (((bottom - y) / (ROW_H * s)).floor() as usize).max(1);
        self.sel = self.sel.min(self.entries.len().saturating_sub(1));
        if self.sel < self.first {
            self.first = self.sel;
        }
        if self.sel >= self.first + rows {
            self.first = self.sel + 1 - rows;
        }
        if self.entries.is_empty() {
            iyakki_uikit::fx::empty_state(g, R::new(x, y, w, bottom - y), s, Icon::Data, "This folder is empty", "Press u to upload a file, or n to make a folder.", ACCENT);
        }
        for (k, e) in self.entries.iter().enumerate().skip(self.first).take(rows) {
            let r = R::new(x, y + (k - self.first) as f32 * ROW_H * s, w, (ROW_H - 2.0) * s);
            let hot = g.hovered(r);
            iyakki_uikit::fx::list_row(g, r, s, k == self.sel, hot, ACCENT);
            g.hit(r, Event::Row(k), &e.name);
            let shown = format!("{}{}", e.name.chars().map(|c| if c.is_control() { '?' } else { c }).collect::<String>(), if e.is_dir { "/" } else if e.is_link { " →" } else { "" });
            let nm = g.fit(14.0 * s, &shown, Face::Sans, w - 360.0 * s);
            g.text(r.x + 12.0 * s, r.y + 5.0 * s, 14.0 * s, if e.is_dir { ACCENT } else { TEXT }, &nm, Face::Sans);
            if !e.is_dir {
                let sz = human_size(e.size);
                let sw = g.text_width(13.0 * s, &sz, Face::Sans);
                g.text(r.x + w - 330.0 * s + 70.0 * s - sw, r.y + 6.0 * s, 13.0 * s, MUTED, &sz, Face::Sans);
            }
            g.text(r.x + w - 200.0 * s, r.y + 6.0 * s, 13.0 * s, MUTED, &date(e.modified), Face::Sans);
        }
        // a line about the queue
        let running = self.transfers.iter().filter(|t| !t.state.finished()).count();
        if running > 0 {
            if let Some(t) = self.transfers.iter().find(|t| !t.state.finished()) {
                let r = R::new(x, bottom + 4.0 * s, w * 0.5, 8.0 * s);
                iyakki_uikit::fx::progress_bar(g, r, frac(t), ACCENT, s);
                g.text(x + w * 0.5 + 12.0 * s, bottom - 1.0 * s, 12.0 * s, MUTED, &format!("{running} transfer(s) running"), Face::Sans);
            }
        }
    }

    fn draw_transfers(&mut self, g: &mut Gfx, area: R, s: f32) {
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 10.0 * s;
        g.text(x, y, 18.0 * s, TEXT, "Transfers", Face::Bold);
        y += 36.0 * s;
        if self.transfers.is_empty() {
            iyakki_uikit::fx::empty_state(g, R::new(x, y, w, area.h - 100.0 * s), s, Icon::Data, "No transfers", "Downloads and uploads you start are listed here.", ACCENT);
            return;
        }
        for (i, t) in self.transfers.iter().enumerate() {
            let r = R::new(x, y + i as f32 * 62.0 * s, w, 56.0 * s);
            if r.y + r.h > area.y + area.h - 36.0 * s {
                break;
            }
            iyakki_uikit::fx::list_row(g, r, s, i == self.tsel, g.hovered(r), ACCENT);
            g.hit(r, Event::Row(i), &t.remote);
            let name = t.remote.rsplit('/').next().unwrap_or("");
            let head = format!("{} {}", if t.download { "Download" } else { "Upload" }, name);
            let head = g.fit(14.0 * s, &head, Face::Bold, w * 0.55);
            g.text(r.x + 14.0 * s, r.y + 6.0 * s, 14.0 * s, TEXT, &head, Face::Bold);
            let (label, color) = match t.state {
                State::Queued => ("waiting".to_string(), MUTED),
                State::Running => (format!("{} · {}", if t.total > 0 { format!("{} of {}", human_size(t.done), human_size(t.total)) } else { human_size(t.done) }, fmt_rate(t.rate)), ACCENT),
                State::Done => ("done".into(), OK),
                State::Failed => (format!("failed: {}", t.note), ERR),
                State::Cancelled => ("cancelled".into(), MUTED),
            };
            let label = g.fit(12.0 * s, &label, Face::Sans, w - 40.0 * s);
            g.text(r.x + 14.0 * s, r.y + 26.0 * s, 12.0 * s, color, &label, Face::Sans);
            iyakki_uikit::fx::progress_bar(g, R::new(r.x + 14.0 * s, r.y + 46.0 * s, r.w - 28.0 * s, 6.0 * s), frac(t), color, s);
        }
    }
}

fn frac(t: &TransferInfo) -> f32 {
    if t.state == State::Done {
        1.0
    } else if t.total == 0 {
        0.0
    } else {
        t.done as f32 / t.total as f32
    }
}

#[cfg(test)]
mod tests;
