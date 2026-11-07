//! AUTH: one-time codes (an authenticator). The groups of the person's own are listed on the left; the chosen group's codes are on the
//! right, each with the service, the account, the current code in big digits, a bar that counts down the seconds it still lives and,
//! in the last five seconds, the next code. Everything (secrets and names) is kept in the person's own vault, which their sign-in
//! password opens (`iyakki_agent::authcodes`); when it is locked this page says how to open it and shows nothing else.
//!
//! * Add: type the service, the account and the secret (or paste an `otpauth://` link into the secret box), press Paste link with a link
//!   on the clipboard, or press Scan QR and hold the QR code of the site up to the camera. The camera asks first, shows when it is on,
//!   and is off again as soon as a code is read. What was read opens the same Add form to look at and Save. The secret is visible only
//!   in that form; there is no way to show or export a saved secret.
//! * Copy: the Copy button or a tap on the code. The code goes on the clipboard as a secret: it is not kept in the clipboard history,
//!   pastes once and is wiped after 60 seconds.
//! * The codes need the right time. A clock that is not set (earlier than the clock floor) shows no codes and says why.
//!
//! Keys: Up / Down choose a group or a code, Right / Enter move to the codes, Left / Esc back to the groups (Esc again leaves), Enter or
//! c copies, a adds, p pastes a link, s scans, e edits, m moves, Delete removes (asks first), n makes a group, r renames, Tab switches side.
//! Audit lines name only the action and the service (`auth.add issuer=...`), never a secret or a code.

mod page;
#[cfg(test)]
mod tests;

use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_agent::authcodes::{self, Book, CodeVault, NewEntry, ServicesVault, VaultProblem};
use iyakki_agent::totp::{self, Algo};

use super::widgets::{Form, FormEvent};
use super::{App, AppCx, Flow};
use crate::gfx::{rgb, Icon};
use crate::scan_core::{self, CamSource, Session, Step};
use crate::scrolllist::ScrollList;
use crate::ui::{clipboard, Event, Key};

pub const ACCENT: [u8; 4] = rgb(150, 90, 215);

const T_GROUP: usize = 1000;
const T_COPY: usize = 3000;
const T_EDIT: usize = 3300;
const T_MOVE: usize = 3600;
const T_DEL: usize = 3900;
const T_SEL: usize = 4200;
const T_ADD: usize = 10;
const T_LINK: usize = 11;
const T_SCAN: usize = 12;
const T_NEWGROUP: usize = 13;
const T_RENAME: usize = 14;
const T_DELGROUP: usize = 15;
const T_RETRY: usize = 16;
const T_ALLOW: usize = 61_701;
const T_DENY: usize = 61_702;
const T_STOP: usize = 61_703;
const A_BAR: usize = 61_710;
const A_GBAR: usize = 61_711;

/// How long a note ("Copied.") stays.
const NOTE_SECS: u64 = 4;
/// The next code is shown when this many seconds or fewer are left.
pub(crate) const NEXT_AT: u32 = 5;

/// The settings a new code can have, as the Add form offers them: (hash, digits, seconds).
type Opts = (Algo, u32, u32);
const USUAL: Opts = (Algo::Sha1, 6, 30);
const PRESETS: [Opts; 5] = [USUAL, (Algo::Sha1, 8, 30), (Algo::Sha256, 6, 30), (Algo::Sha512, 8, 30), (Algo::Sha1, 6, 60)];

fn opts_label((a, d, p): Opts) -> String {
    format!("{d} digits, {}, {p} s{}", a.name(), if (a, d, p) == USUAL { " (usual)" } else { "" })
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Focus {
    Groups,
    Codes,
}

/// Whether the codes could be read from the vault.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Load {
    Ready,
    Locked,
    Problem(String),
}

enum Mode {
    Browse,
    /// The Add form and the settings its last field offers.
    Add(Form, Vec<Opts>),
    Edit(u64, Form),
    Move(u64, Form),
    NewGroup(Form),
    Rename(String, Form),
    DelGroup(String),
    DelEntry(u64),
    Scan(Session),
}

pub struct AuthApp {
    vault: Box<dyn CodeVault>,
    now: Box<dyn Fn() -> i64>,
    cam: Option<Arc<CamSource>>,
    load: Load,
    book: Book,
    group: usize,
    sel: usize,
    focus: Focus,
    mode: Mode,
    note: Option<(String, bool, Instant)>,
    scroll: ScrollList,
    gscroll: ScrollList,
    last_sec: i64,
    /// The group and code the list was last scrolled to (so the wheel is not fought on every frame).
    shown_sel: (usize, usize),
    redraw: bool,
}

/// The words for a vault that cannot be used.
fn locked_words() -> &'static str {
    "Your vault is locked. One-time codes are kept in your vault, and your sign-in password opens it. Sign out and sign in again with your password, or open Settings, then Vault, and press u to unlock it. Then come back here."
}

impl AuthApp {
    pub fn new(cx: &AppCx) -> AuthApp {
        AuthApp::with_vault(Box::new(ServicesVault { svc: cx.services.clone(), user: cx.session.user.clone() }), Box::new(iyakki_agent::clock::now_unix))
    }

    /// An app over any vault and clock (tests).
    pub fn with_vault(vault: Box<dyn CodeVault>, now: Box<dyn Fn() -> i64>) -> AuthApp {
        let mut a = AuthApp {
            vault,
            now,
            cam: None,
            load: Load::Ready,
            book: Book::default(),
            group: 0,
            sel: 0,
            focus: Focus::Codes,
            mode: Mode::Browse,
            note: None,
            scroll: ScrollList::default(),
            gscroll: ScrollList::default(),
            last_sec: 0,
            shown_sel: (usize::MAX, 0),
            redraw: true,
        };
        a.reload();
        a
    }

    /// Cameras made up in software (tests).
    pub fn with_camera(mut self, src: Arc<CamSource>) -> AuthApp {
        self.cam = Some(src);
        self
    }

    fn reload(&mut self) {
        match self.vault.load() {
            Ok(b) => {
                self.book = b;
                self.load = Load::Ready;
            }
            Err(VaultProblem::Locked) => {
                self.book = Book::default();
                self.load = Load::Locked;
            }
            Err(VaultProblem::Other(m)) => {
                self.book = Book::default();
                self.load = Load::Problem(m);
            }
        }
        self.group = self.group.min(self.book.groups.len().saturating_sub(1));
        self.sel = self.sel.min(self.entries_len().saturating_sub(1));
    }

    fn ready(&self) -> bool {
        self.load == Load::Ready
    }

    fn clock_ok(&self) -> bool {
        authcodes::clock_is_set((self.now)())
    }

    fn group_name(&self) -> String {
        self.book.groups.get(self.group).cloned().unwrap_or_default()
    }

    fn entries_len(&self) -> usize {
        self.book.count_in(&self.group_name())
    }

    /// The id of the entry at position `i` of the chosen group.
    fn id_at(&self, i: usize) -> Option<u64> {
        let g = self.group_name();
        let id = self.book.in_group(&g).nth(i).map(|e| e.id);
        id
    }

    fn say(&mut self, msg: impl Into<String>, good: bool) {
        self.note = Some((msg.into(), good, Instant::now()));
    }

    fn note_text(&self) -> Option<(&str, bool)> {
        self.note.as_ref().filter(|(_, _, t)| t.elapsed() < Duration::from_secs(NOTE_SECS)).map(|(m, g, _)| (m.as_str(), *g))
    }

    /// Change a copy of the book, save it, and only then keep the change.
    fn commit<T>(&mut self, f: impl FnOnce(&mut Book) -> Result<T, String>) -> Result<T, String> {
        if !self.ready() {
            return Err(locked_words().into());
        }
        let mut b = self.book.clone();
        let out = f(&mut b)?;
        match self.vault.save(&b) {
            Ok(()) => {
                self.book = b;
                Ok(out)
            }
            Err(VaultProblem::Locked) => {
                self.load = Load::Locked;
                Err(locked_words().into())
            }
            Err(VaultProblem::Other(m)) => Err(m),
        }
    }

    // ------------------------------------------------------------------------------------------------------------ forms

    fn add_form(&self, issuer: &str, account: &str, secret: &str, opts: Opts, note: &str) -> Mode {
        let mut presets: Vec<Opts> = PRESETS.to_vec();
        let at = match presets.iter().position(|p| *p == opts) {
            Some(i) => i,
            None => {
                presets.insert(0, opts);
                0
            }
        };
        let groups: Vec<&str> = self.book.groups.iter().map(String::as_str).collect();
        let labels: Vec<String> = presets.iter().map(|p| opts_label(*p)).collect();
        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
        let f = Form::new("Add a one-time code")
            .with_note(note)
            .text("Service", issuer, "for example GitHub")
            .text("Account", account, "for example you@example.com")
            .text("Secret key, or a pasted otpauth:// link", secret, "the letters and digits the site shows, or its link")
            .choice("Group", &groups, self.group.min(groups.len().saturating_sub(1)))
            .choice("Kind of code", &labels, at);
        Mode::Add(f, presets)
    }

    fn open_add(&mut self) {
        if !self.ready() {
            return self.say(locked_words(), false);
        }
        self.mode = self.add_form("", "", "", USUAL, "");
        self.focus = Focus::Codes;
    }

    /// The Add form filled from a link (pasted, or read from a QR code), or a note when it is not a good link.
    fn open_add_from(&mut self, text: &str) {
        match totp::parse_otpauth(text) {
            Ok(p) => {
                let opts = (p.algo, p.digits, p.period);
                self.mode = self.add_form(&p.issuer, &p.account, &p.secret, opts, "Read from the link. Check the names, pick the group and press Save.");
            }
            Err(e) => self.say(e, false),
        }
    }

    fn submit_add(&mut self, mut form: Form, presets: Vec<Opts>, cx: &AppCx) {
        let secret = form.raw(2);
        let (mut issuer, mut account, mut opts) = (form.value(0), form.value(1), presets.get(form.choice_idx(4)).copied().unwrap_or(USUAL));
        let mut secret_text = secret.trim().to_string();
        if totp::looks_like_link(&secret_text) {
            match totp::parse_otpauth(&secret_text) {
                Ok(p) => {
                    if issuer.is_empty() {
                        issuer = p.issuer.clone();
                    }
                    if account.is_empty() {
                        account = p.account.clone();
                    }
                    opts = (p.algo, p.digits, p.period);
                    secret_text = p.secret.to_string();
                }
                Err(e) => {
                    form.error = Some(e);
                    self.mode = Mode::Add(form, presets);
                    return;
                }
            }
        }
        let group = self.book.groups.get(form.choice_idx(3)).cloned().unwrap_or_default();
        let new = NewEntry::typed(&issuer, &account, &secret_text, opts, &group);
        match self.commit(|b| b.add(&new)) {
            Ok(_) => {
                cx.audit.record(&cx.session.user, "auth.add", &format!("issuer={issuer}"));
                self.say("Added. The secret is saved in your vault and is not shown again.", true);
                if let Some(i) = self.book.groups.iter().position(|g| *g == group) {
                    self.group = i;
                }
                self.sel = self.entries_len().saturating_sub(1);
                self.focus = Focus::Codes;
            }
            Err(e) => {
                form.error = Some(e);
                self.mode = Mode::Add(form, presets);
            }
        }
    }

    fn open_edit(&mut self, id: u64) {
        if let Some(e) = self.book.entry(id) {
            self.mode = Mode::Edit(id, Form::new("Edit the names").with_note("The secret stays as it is. To change it, delete this code and add it again.").text("Service", &e.issuer, "").text("Account", &e.account, ""));
        }
    }

    fn open_move(&mut self, id: u64) {
        if let Some(e) = self.book.entry(id) {
            let groups: Vec<&str> = self.book.groups.iter().map(String::as_str).collect();
            let at = self.book.groups.iter().position(|g| *g == e.group).unwrap_or(0);
            self.mode = Mode::Move(id, Form::new(&format!("Move {}", e.title())).choice("Move to group", &groups, at));
        }
    }

    fn open_paste_link(&mut self) {
        match clipboard::paste() {
            Some(t) if totp::looks_like_link(&t) => self.open_add_from(&t),
            _ => self.say("The clipboard has no one-time-code link. Copy the otpauth:// link from the site first.", false),
        }
    }

    fn open_scan(&mut self) {
        if !self.ready() {
            return self.say(locked_words(), false);
        }
        let src = self.cam.clone().unwrap_or_else(CamSource::desktop);
        let mut s = Session::new(src, "There is no camera on this device. Type the secret key or paste the link instead.");
        if cfg!(test) {
            s = s.with_budget(Duration::from_secs(10));
        }
        s.ask();
        if s.step() == Step::Off {
            let m = s.said().map(|(m, _)| m.clone()).unwrap_or_default();
            return self.say(m, false);
        }
        self.mode = Mode::Scan(s);
    }

    // ------------------------------------------------------------------------------------------------------------ actions

    fn copy_code(&mut self, i: usize, cx: &AppCx) {
        if !self.clock_ok() {
            return self.say("The clock is not set, so no code can be made.", false);
        }
        let Some(e) = self.id_at(i).and_then(|id| self.book.entry(id)) else { return };
        let (code, issuer) = (e.code_at((self.now)()), e.issuer.clone());
        match code {
            Some(c) => {
                // a secret: not kept in the clipboard history, pastes once, wiped after a minute
                clipboard::copy_secret(&c);
                cx.audit.record(&cx.session.user, "auth.copy", &format!("issuer={issuer}"));
                self.say("Copied.", true);
            }
            None => self.say("This code is damaged and cannot be made. Delete it and add it again.", false),
        }
    }

    fn delete_entry(&mut self, id: u64, cx: &AppCx) {
        match self.commit(|b| b.delete(id)) {
            Ok(e) => {
                cx.audit.record(&cx.session.user, "auth.delete", &format!("issuer={}", e.issuer));
                self.say("Deleted.", true);
                self.sel = self.sel.min(self.entries_len().saturating_sub(1));
            }
            Err(e) => self.say(e, false),
        }
    }

    fn submit_group(&mut self, mut form: Form, old: Option<String>, cx: &AppCx) {
        let name = form.value(0);
        let r = match &old {
            None => self.commit(|b| b.add_group(&name)),
            Some(o) => self.commit(|b| b.rename_group(o, &name)),
        };
        match r {
            Ok(n) => {
                cx.audit.record(&cx.session.user, if old.is_some() { "auth.group.rename" } else { "auth.group.add" }, &format!("name={n}"));
                if let Some(i) = self.book.groups.iter().position(|g| *g == n) {
                    self.group = i;
                    self.sel = 0;
                }
            }
            Err(e) => {
                form.error = Some(e);
                self.mode = match old {
                    None => Mode::NewGroup(form),
                    Some(o) => Mode::Rename(o, form),
                };
            }
        }
    }

    fn delete_group(&mut self, name: &str, cx: &AppCx) {
        match self.commit(|b| b.delete_group(name)) {
            Ok(n) => {
                cx.audit.record(&cx.session.user, "auth.group.delete", &format!("name={name}"));
                self.say(if n == 0 { "Group deleted.".to_string() } else { format!("Group deleted. Its {n} code{} moved to {}.", if n == 1 { "" } else { "s" }, self.book.groups[0]) }, true);
                self.group = 0;
                self.sel = 0;
            }
            Err(e) => self.say(e, false),
        }
    }

    // ------------------------------------------------------------------------------------------------------------ events

    fn browse(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        let n_codes = self.entries_len();
        match ev {
            Event::Key(k) => return self.browse_key(k, cx),
            Event::Paste(t) if totp::looks_like_link(t) => self.open_add_from(t),
            Event::Tab(T_ADD) => self.open_add(),
            Event::Tab(T_LINK) => self.open_paste_link(),
            Event::Tab(T_SCAN) => self.open_scan(),
            Event::Tab(T_NEWGROUP) => {
                if self.ready() {
                    self.mode = Mode::NewGroup(Form::new("New group").text("Group name", "", "for example Work"));
                }
            }
            Event::Tab(T_RENAME) => {
                if self.ready() {
                    let g = self.group_name();
                    self.mode = Mode::Rename(g.clone(), Form::new("Rename the group").text("Group name", &g, ""));
                }
            }
            Event::Tab(T_DELGROUP) => {
                if self.ready() {
                    self.mode = Mode::DelGroup(self.group_name());
                }
            }
            Event::Tab(T_RETRY) => {
                self.reload();
                if self.ready() {
                    self.say("Your codes are open.", true);
                } else {
                    self.say(locked_words(), false);
                }
            }
            Event::Tab(n) if (T_GROUP..T_GROUP + 100).contains(n) => {
                let i = n - T_GROUP;
                if i < self.book.groups.len() {
                    self.group = i;
                    self.sel = 0;
                    self.scroll.to_top();
                    self.focus = Focus::Groups;
                }
            }
            Event::Tab(n) if (T_COPY..T_COPY + 300).contains(n) && n - T_COPY < n_codes => {
                self.sel = n - T_COPY;
                self.focus = Focus::Codes;
                self.copy_code(self.sel, cx);
            }
            Event::Tab(n) if (T_EDIT..T_EDIT + 300).contains(n) && n - T_EDIT < n_codes => {
                self.sel = n - T_EDIT;
                if let Some(id) = self.id_at(self.sel) {
                    self.open_edit(id);
                }
            }
            Event::Tab(n) if (T_MOVE..T_MOVE + 300).contains(n) && n - T_MOVE < n_codes => {
                self.sel = n - T_MOVE;
                if let Some(id) = self.id_at(self.sel) {
                    self.open_move(id);
                }
            }
            Event::Tab(n) if (T_DEL..T_DEL + 300).contains(n) && n - T_DEL < n_codes => {
                self.sel = n - T_DEL;
                if let Some(id) = self.id_at(self.sel) {
                    self.mode = Mode::DelEntry(id);
                }
            }
            Event::Tab(n) if (T_SEL..T_SEL + 300).contains(n) && n - T_SEL < n_codes => {
                self.sel = n - T_SEL;
                self.focus = Focus::Codes;
            }
            other => {
                let a = self.gscroll.event(other, A_GBAR);
                if !a {
                    self.scroll.event(other, A_BAR);
                }
            }
        }
        Flow::Stay
    }

    fn browse_key(&mut self, k: &Key, cx: &AppCx) -> Flow {
        let lower = match k {
            Key::Char(c) => Some(c.to_ascii_lowercase()),
            _ => None,
        };
        if !self.ready() {
            return match k {
                Key::Enter | Key::Char('r' | 'R') => self.browse(&Event::Tab(T_RETRY), cx),
                Key::Esc => Flow::Back,
                _ => Flow::Stay,
            };
        }
        match (k, lower) {
            (Key::Tab | Key::BackTab, _) => self.focus = if self.focus == Focus::Groups { Focus::Codes } else { Focus::Groups },
            (_, Some('a')) => self.open_add(),
            (_, Some('p')) => self.open_paste_link(),
            (_, Some('s')) => self.open_scan(),
            (_, Some('n')) => {
                return self.browse(&Event::Tab(T_NEWGROUP), cx);
            }
            (Key::F(2), _) | (_, Some('r')) => {
                return self.browse(&Event::Tab(T_RENAME), cx);
            }
            (Key::Esc, _) => {
                if self.focus == Focus::Codes && self.book.groups.len() > 1 {
                    self.focus = Focus::Groups;
                } else {
                    return Flow::Back;
                }
            }
            (Key::PageUp | Key::PageDown | Key::Home | Key::End, _) => {
                self.scroll.key(k);
            }
            _ => match self.focus {
                Focus::Groups => match k {
                    Key::Up if self.group > 0 => {
                        self.group -= 1;
                        self.sel = 0;
                    }
                    Key::Down if self.group + 1 < self.book.groups.len() => {
                        self.group += 1;
                        self.sel = 0;
                    }
                    Key::Right | Key::Enter => self.focus = Focus::Codes,
                    Key::Delete => return self.browse(&Event::Tab(T_DELGROUP), cx),
                    _ => {}
                },
                Focus::Codes => {
                    let n = self.entries_len();
                    match k {
                        Key::Up => self.sel = self.sel.saturating_sub(1),
                        Key::Down => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
                        Key::Left => self.focus = Focus::Groups,
                        Key::Enter => self.copy_code(self.sel, cx),
                        Key::Delete => {
                            if let Some(id) = self.id_at(self.sel) {
                                self.mode = Mode::DelEntry(id);
                            }
                        }
                        _ => match lower {
                            Some('c') => self.copy_code(self.sel, cx),
                            Some('e') => {
                                if let Some(id) = self.id_at(self.sel) {
                                    self.open_edit(id);
                                }
                            }
                            Some('m') => {
                                if let Some(id) = self.id_at(self.sel) {
                                    self.open_move(id);
                                }
                            }
                            Some('d') => {
                                if let Some(id) = self.id_at(self.sel) {
                                    self.mode = Mode::DelEntry(id);
                                }
                            }
                            _ => {}
                        },
                    }
                }
            },
        }
        Flow::Stay
    }

    /// The camera card: the question, then the live picture.
    fn scan_event(&mut self, mut sess: Session, ev: &Event) {
        match (sess.step(), ev) {
            (Step::Asking, Event::Tab(T_ALLOW)) | (Step::Asking, Event::Key(Key::Enter)) => sess.allow(),
            (Step::Asking, Event::Tab(T_DENY)) | (Step::Asking, Event::Key(Key::Esc)) => sess.deny(),
            (Step::On, Event::Tab(T_STOP)) | (Step::On, Event::Key(Key::Esc)) => sess.stop(Some("The camera is off. Nothing was added.")),
            _ => {}
        }
        self.keep_scan(sess);
    }

    /// Keep the card while the camera is asking or on; when it is off say why.
    fn keep_scan(&mut self, sess: Session) {
        if sess.step() == Step::Off {
            let (m, good) = sess.said().cloned().unwrap_or(("The camera is off.".to_string(), true));
            self.say(m, good);
        } else {
            self.mode = Mode::Scan(sess);
        }
    }

    /// Look at the camera (every heartbeat while the card is up). True when something on screen changed.
    fn poll_scan(&mut self) -> bool {
        if !matches!(&self.mode, Mode::Scan(s) if s.step() == Step::On) {
            return false;
        }
        let Mode::Scan(mut sess) = std::mem::replace(&mut self.mode, Mode::Browse) else {
            return false;
        };
        let got = sess.poll(|bytes| match std::str::from_utf8(bytes) {
            Ok(t) if totp::looks_like_link(t) => Ok((t.to_string(), "Read a one-time-code link. The camera is off.".to_string())),
            _ => Err("That QR code is not a one-time-code link. Still looking.".to_string()),
        });
        match got {
            Some(text) => self.open_add_from(&text),
            None => self.keep_scan(sess),
        }
        true
    }

    fn form_event(&mut self, mode: Mode, ev: &Event, cx: &AppCx) {
        match mode {
            Mode::Add(mut f, presets) => match f.handle_event(ev) {
                FormEvent::Cancel => {}
                FormEvent::Submit => self.submit_add(f, presets, cx),
                _ => self.mode = Mode::Add(f, presets),
            },
            Mode::Edit(id, mut f) => match f.handle_event(ev) {
                FormEvent::Cancel => {}
                FormEvent::Submit => match self.commit(|b| b.edit(id, &f.value(0), &f.value(1))) {
                    Ok(()) => {
                        cx.audit.record(&cx.session.user, "auth.edit", &format!("issuer={}", f.value(0)));
                        self.say("Saved.", true);
                    }
                    Err(e) => {
                        f.error = Some(e);
                        self.mode = Mode::Edit(id, f);
                    }
                },
                _ => self.mode = Mode::Edit(id, f),
            },
            Mode::Move(id, mut f) => match f.handle_event(ev) {
                FormEvent::Cancel => {}
                FormEvent::Submit => {
                    let to = self.book.groups.get(f.choice_idx(0)).cloned().unwrap_or_default();
                    match self.commit(|b| b.move_to(id, &to)) {
                        Ok(()) => {
                            let issuer = self.book.entry(id).map(|e| e.issuer.clone()).unwrap_or_default();
                            cx.audit.record(&cx.session.user, "auth.move", &format!("issuer={issuer} group={to}"));
                            self.say(format!("Moved to {to}."), true);
                            self.sel = self.sel.min(self.entries_len().saturating_sub(1));
                        }
                        Err(e) => {
                            f.error = Some(e);
                            self.mode = Mode::Move(id, f);
                        }
                    }
                }
                _ => self.mode = Mode::Move(id, f),
            },
            Mode::NewGroup(mut f) => match f.handle_event(ev) {
                FormEvent::Cancel => {}
                FormEvent::Submit => self.submit_group(f, None, cx),
                _ => self.mode = Mode::NewGroup(f),
            },
            Mode::Rename(old, mut f) => match f.handle_event(ev) {
                FormEvent::Cancel => {}
                FormEvent::Submit => self.submit_group(f, Some(old), cx),
                _ => self.mode = Mode::Rename(old, f),
            },
            Mode::DelGroup(name) => match ev {
                Event::Key(Key::Char('y' | 'Y')) => self.delete_group(&name, cx),
                Event::Key(Key::Esc) | Event::Key(Key::Char('n' | 'N')) => {}
                _ => self.mode = Mode::DelGroup(name),
            },
            Mode::DelEntry(id) => match ev {
                Event::Key(Key::Char('y' | 'Y')) => self.delete_entry(id, cx),
                Event::Key(Key::Esc) | Event::Key(Key::Char('n' | 'N')) => {}
                _ => self.mode = Mode::DelEntry(id),
            },
            Mode::Scan(s) => self.scan_event(s, ev),
            Mode::Browse => {}
        }
    }
}

impl App for AuthApp {
    fn title(&self) -> &'static str {
        "One-time codes"
    }
    fn page(&self) -> String {
        match (&self.load, &self.mode) {
            (Load::Ready, Mode::Scan(_)) => "scan".into(),
            (Load::Ready, Mode::Add(..)) => "add".into(),
            (Load::Ready, _) => format!("group:{}", self.group_name().to_lowercase().replace(' ', "-")),
            _ => "locked".into(),
        }
    }
    fn icon(&self) -> Icon {
        Icon::Lock
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        "↑↓ choose · Enter copy · A add · P paste link · S scan QR · E edit · Del delete · Esc back".into()
    }
    fn help(&self) -> Vec<String> {
        [
            "AUTH makes the one-time codes that web sites ask for after your password. Your groups are on the left, the codes of the chosen group on the right. Each code changes every 30 seconds (some sites use 60); the bar shows the time it has left, and in the last five seconds the next code is shown too.",
            "Copy puts the code on the clipboard as a secret: it is not kept in the clipboard history, it pastes once and is wiped after 60 seconds. A tap or a click on the code copies it too.",
            "Add a code by typing the service, the account and the secret key the site shows, by pasting the otpauth:// link into the secret box (or pressing Paste link with the link on the clipboard), or by pressing Scan QR and holding the site's QR code up to the camera. The camera asks first and turns off as soon as a code is read.",
            "Everything is kept in your own vault, which your sign-in password opens. A saved secret is never shown again and cannot be exported; to change one, delete the code and add it again. Deleting asks first.",
            "Groups are your own names (Personal, Work, ...). Make, rename and delete them with the buttons or n, r and Delete; Move puts a code in another group. Deleting a group keeps its codes: they move to the first group.",
            "The codes depend on the right time. If this computer's clock is not set, no codes are shown until it is (connect to the network, or set the time in Settings).",
            "Keys: Up / Down choose a group or a code, Right or Enter go to the codes, Left or Esc go back to the groups, Enter or c copies, a adds, p pastes a link, s scans, e edits, m moves, Delete removes, Tab switches sides, Esc leaves.",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        self.redraw = true;
        let mode = std::mem::replace(&mut self.mode, Mode::Browse);
        match mode {
            Mode::Browse => self.browse(ev, cx),
            other => {
                self.form_event(other, ev, cx);
                Flow::Stay
            }
        }
    }

    fn scroll_px(&mut self, px: i32, _cx: &AppCx) -> bool {
        self.gscroll.wheel_px(px) || self.scroll.wheel_px(px)
    }

    fn wants_redraw(&mut self) -> bool {
        let scanning = self.poll_scan() || matches!(&self.mode, Mode::Scan(s) if s.step() == Step::On);
        // the codes and the bars move once a second
        let sec = (self.now)();
        let tick = sec != self.last_sec;
        self.last_sec = sec;
        let asked = std::mem::take(&mut self.redraw);
        scanning || tick || asked
    }

    fn render(&mut self, g: &mut crate::gfx::Gfx, area: crate::ui::R, s: f32, _cx: &AppCx, tick: u64) {
        self.draw(g, area, s, tick);
    }
}
