//! The Notes tab of the AI IDE: the workspace's project notes (decisions, constraints, conventions, gotchas, things to do later,
//! references) that every agent of the workspace is given at the top of its context. People and agents write them; the policy
//! (secrets refused, agent notes unpinned and unreviewed until a person looks, size and rate limits) is `iyakki_agent::notes`.
//!
//! One keyboard owner: the tab's letters are shortcuts, and while a note is being edited the title, text and source fields take the
//! typing (`wants_text`). The notes are re-read when `.iyakki/notes.json` changes, so a note an agent adds shows up within a second
//! and is marked "new by agent" until it is reviewed.

use crate::safeclamp::SafeClamp;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use iyakki_agent::notes::{self, AgentsMayAdd, Filter, Kind, NewNote, Note, Notes, Status, Who};

use super::ide_chrome as chrome;
use super::multiline::MultiLine;
use crate::fx::{self, with_alpha, AMBER, CYAN, MAGENTA};
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

pub const SUB: u8 = 37;
pub const ROW_BASE: usize = 710_000;
pub const FOCUS_SEARCH: usize = 9_800_001;
pub const FOCUS_TITLE: usize = 9_800_002;
pub const FOCUS_SOURCE: usize = 9_800_003;
pub const TEXT_AREA: usize = 63_601;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Title,
    Text,
    Source,
}

/// A note being written or changed.
struct Edit {
    /// None: a new note.
    id: Option<String>,
    title: TextField,
    text: MultiLine,
    source: TextField,
    kind: Kind,
    focus: Field,
    error: Option<String>,
}

/// What the tab asks the IDE to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotesAct {
    None,
    Used,
    /// Agents wrote notes (or a person did elsewhere): a toast with this text.
    Toast(String),
    /// Leave the tab for the tree.
    Back,
}

pub struct NotesView {
    root: PathBuf,
    person: String,
    notes: Notes,
    problem: Option<String>,
    stamp: Option<SystemTime>,
    /// Unreviewed notes of agents when last read (a rise is a toast), None before the first read.
    seen_new: Option<usize>,
    pub filter: Filter,
    search: TextField,
    search_focus: bool,
    sel: usize,
    scroll: usize,
    visible: usize,
    detail_scroll: usize,
    edit: Option<Edit>,
    /// A delete is waiting for the answer (the note's id).
    confirm_delete: Option<String>,
    pub status: Option<(String, bool)>,
    pub read_only: bool,
    /// Narrow pane: the detail is a page of its own.
    pub page: bool,
    narrow: bool,
    last_poll: std::time::Instant,
    /// How often the file is looked at (0 in tests).
    pub poll_every: std::time::Duration,
    caret_on: bool,
    hover_list: bool,
    tick: u64,
}

/// The width `chrome::text_btn` gives a button with this label.
fn btn_w(g: &mut Gfx, s: f32, label: &str) -> f32 {
    g.text_width(chrome::cur().font_small * s + 1.0 * s, label, Face::Bold) + 16.0 * s
}

fn when(unix: i64) -> String {
    iyakki_agent::clock::format(&iyakki_agent::clock::Zone::system(), unix, "%Y-%m-%d %H:%M")
}

fn kind_color(k: Kind) -> [u8; 4] {
    match k {
        Kind::Decision => CYAN,
        Kind::Constraint => ERR,
        Kind::Convention => OK,
        Kind::Gotcha => AMBER,
        Kind::TodoLater => MAGENTA,
        Kind::Reference => rgb(120, 170, 255),
    }
}

impl NotesView {
    pub fn new(root: &Path, person: &str) -> NotesView {
        let mut v = NotesView {
            root: root.into(),
            person: person.into(),
            notes: Notes::default(),
            problem: None,
            stamp: None,
            seen_new: None,
            filter: Filter::default(),
            search: TextField::new("search the context  (/)", false),
            search_focus: false,
            sel: 0,
            scroll: 0,
            visible: 6,
            detail_scroll: 0,
            edit: None,
            confirm_delete: None,
            status: None,
            read_only: false,
            page: false,
            narrow: false,
            last_poll: std::time::Instant::now(),
            poll_every: std::time::Duration::from_millis(900),
            caret_on: true,
            hover_list: false,
            tick: 0,
        };
        v.reload();
        v
    }

    pub fn narrow(&self) -> bool {
        self.narrow
    }

    fn mtime(&self) -> Option<SystemTime> {
        std::fs::metadata(notes::json_path(&self.root)).and_then(|m| m.modified()).ok()
    }

    /// Read the file again (keeps the selected note selected).
    pub fn reload(&mut self) {
        let keep = self.selected().map(|n| n.id.clone());
        let (n, problem) = notes::load(&self.root);
        self.notes = n;
        self.problem = problem;
        self.stamp = self.mtime();
        if let Some(id) = keep {
            if let Some(i) = self.shown().iter().position(|n| n.id == id) {
                self.sel = i;
            }
        }
        self.sel = self.sel.min(self.shown().len().saturating_sub(1));
    }

    /// The notes on show, in order.
    fn shown(&self) -> Vec<&Note> {
        let mut f = self.filter.clone();
        f.text = self.search.value.clone();
        self.notes.view(&f)
    }

    pub fn selected(&self) -> Option<&Note> {
        self.shown().get(self.sel).copied()
    }

    /// (active notes, pinned, waiting for a review): the numbers of the chip and the header.
    pub fn counts(&self) -> (usize, usize, usize) {
        self.notes.counts()
    }

    /// "4 notes in context": what the top bar says; None when there are no active notes.
    pub fn chip(&self) -> Option<String> {
        let (n, _, _) = self.notes.counts();
        (n > 0).then(|| format!("{n} in context"))
    }

    pub fn wants_text(&self) -> bool {
        self.edit.is_some() || self.search_focus
    }

    pub fn editing(&self) -> bool {
        self.edit.is_some()
    }

    pub fn settings(&self) -> &notes::Settings {
        &self.notes.settings
    }

    /// Select a note by id (clearing the filters that would hide it).
    pub fn select_id(&mut self, id: &str) {
        if !self.shown().iter().any(|n| n.id == id) {
            self.filter = Filter::default();
            self.search.clear();
        }
        if let Some(i) = self.shown().iter().position(|n| n.id == id) {
            self.sel = i;
        }
    }

    pub fn blur(&mut self) {
        self.search_focus = false;
    }

    fn say(&mut self, m: impl Into<String>, ok: bool) {
        self.status = Some((m.into(), ok));
    }

    /// Poll the file about once a second; a rise in agent notes nobody has looked at is a toast.
    pub fn tick(&mut self) -> NotesAct {
        if self.last_poll.elapsed() < self.poll_every {
            return NotesAct::None;
        }
        self.last_poll = std::time::Instant::now();
        if self.mtime() != self.stamp && self.edit.is_none() {
            self.reload();
        } else if self.mtime() != self.stamp {
            // being edited: read the list again without losing the form
            let (n, p) = notes::load(&self.root);
            self.notes = n;
            self.problem = p;
            self.stamp = self.mtime();
        }
        let now_new = self.notes.counts().2;
        let prev = self.seen_new.replace(now_new);
        match prev {
            Some(p) if now_new > p => {
                let who = self.notes.notes.iter().filter(|n| n.is_new_by_agent()).max_by_key(|n| n.created).map(|n| format!("{} added to the project context: {}", n.author, n.title)).unwrap_or_else(|| "An agent added to the project context".into());
                NotesAct::Toast(who)
            }
            _ => NotesAct::None,
        }
    }

    // ------------------------------------------------------------------------------------------------ changes

    fn change<T>(&mut self, f: impl FnOnce(&mut Notes) -> Result<T, String>) -> Result<T, String> {
        if self.read_only {
            return Err("Read-only: another session has this workspace open.".into());
        }
        let r = notes::edit(&self.root, f);
        self.reload();
        r
    }

    fn who(&self) -> Who {
        Who::Person(self.person.clone())
    }

    fn start_edit(&mut self, id: Option<String>) {
        if self.read_only {
            self.say("Read-only: another session has this workspace open.", false);
            return;
        }
        let existing = id.as_ref().and_then(|i| self.notes.get(i)).cloned();
        let mut title = TextField::new("a short title", false);
        title.max = notes::MAX_TITLE;
        let mut text = MultiLine::new(notes::MAX_TEXT_PERSON);
        text.min_rows = 4;
        text.max_rows = 10;
        let mut source = TextField::new("where it came from: chat, F-APP-01, a file (optional)", false);
        source.max = 120;
        let kind = match &existing {
            Some(n) => {
                title.set(&n.title);
                text.set(&n.text);
                source.set(&n.source);
                n.kind
            }
            None => self.filter.kind.unwrap_or(Kind::Decision),
        };
        self.edit = Some(Edit { id, title, text, source, kind, focus: Field::Title, error: None });
        self.search_focus = false;
        self.confirm_delete = None;
    }

    fn save_edit(&mut self) -> NotesAct {
        let Some(e) = self.edit.take() else { return NotesAct::None };
        let (title, text, source, kind, id) = (e.title.value.trim().to_string(), e.text.text.clone(), e.source.value.trim().to_string(), e.kind, e.id.clone());
        let who = self.who();
        let now = iyakki_agent::clock::now_unix();
        let r = self.change(|n| match &id {
            Some(i) => {
                n.update(i, Some(&title), Some(&text), Some(kind), &who, now)?;
                if let Some(x) = n.notes.iter_mut().find(|x| &x.id == i) {
                    x.source = source.chars().take(120).collect();
                }
                Ok(i.clone())
            }
            None => n.add(NewNote { title: title.clone(), text: text.clone(), kind, pinned: false, source: source.clone() }, &who, now).map(|a| a.duplicate_of.map(|d| format!("{d} (an entry with a similar title already exists)")).unwrap_or(a.id)),
        });
        match r {
            Ok(id) => {
                let plain = id.split(' ').next().unwrap_or("").to_string();
                if let Some(i) = self.shown().iter().position(|n| n.id == plain) {
                    self.sel = i;
                }
                self.say(if id.contains("similar") { format!("Not added twice: {id}.") } else { format!("Saved {id}.") }, true);
                NotesAct::Used
            }
            Err(msg) => {
                // the form stays open with the reason
                let mut e = e;
                e.error = Some(msg);
                self.edit = Some(e);
                NotesAct::Used
            }
        }
    }

    fn act(&mut self, c: char) -> NotesAct {
        let sel = self.selected().cloned();
        let now = iyakki_agent::clock::now_unix();
        let who = self.who();
        match c {
            'n' => self.start_edit(None),
            'e' => {
                if let Some(n) = sel {
                    self.start_edit(Some(n.id));
                }
            }
            'p' => {
                if let Some(n) = sel {
                    let on = !n.pinned;
                    match self.change(|x| x.set_pinned(&n.id, on, now)) {
                        Ok(()) => self.say(if on { format!("{} is pinned: it comes first in every agent's context.", n.id) } else { format!("{} is no longer pinned.", n.id) }, true),
                        Err(e) => self.say(e, false),
                    }
                }
            }
            'a' => {
                if let Some(n) = sel {
                    let r = if n.status == Status::Active { self.change(|x| x.archive(&n.id, &who, now)).map(|()| format!("{} archived: agents no longer get it.", n.id)) } else { self.change(|x| x.restore(&n.id, now)).map(|()| format!("{} restored.", n.id)) };
                    match r {
                        Ok(m) => self.say(m, true),
                        Err(e) => self.say(e, false),
                    }
                }
            }
            'x' => {
                if let Some(n) = sel {
                    self.confirm_delete = Some(n.id);
                }
            }
            'y' => {
                if let Some(id) = self.confirm_delete.take() {
                    match self.change(|x| x.delete(&id).map(|_| ())) {
                        Ok(()) => self.say(format!("{id} deleted."), true),
                        Err(e) => self.say(e, false),
                    }
                }
            }
            'N' => self.confirm_delete = None,
            't' => {
                if let Some(n) = sel.filter(|n| n.by_agent) {
                    let on = !n.trusted;
                    match self.change(|x| x.set_trusted(&n.id, on)) {
                        Ok(()) => self.say(if on { format!("{} is trusted: agents follow it like your own entries.", n.id) } else { format!("{} is data again, not an instruction.", n.id) }, true),
                        Err(e) => self.say(e, false),
                    }
                } else {
                    self.say("Only an entry written by an agent can be trusted.", false);
                }
            }
            'r' => {
                if let Some(n) = sel {
                    let _ = self.change(|x| x.acknowledge(&n.id));
                    self.say(format!("{} marked as reviewed.", n.id), true);
                }
            }
            'R' => {
                let _ = self.change(|x| {
                    x.acknowledge_all();
                    Ok(())
                });
                self.say("All agent entries marked as reviewed.", true);
            }
            '[' | ']' => {
                if let Some(n) = sel {
                    let _ = self.change(|x| x.move_by(&n.id, if c == '[' { -1 } else { 1 }));
                }
            }
            'k' => {
                self.filter.kind = match self.filter.kind {
                    None => Some(Kind::ALL[0]),
                    Some(k) => Kind::ALL.iter().position(|x| *x == k).and_then(|i| Kind::ALL.get(i + 1)).copied(),
                };
                self.sel = 0;
            }
            'u' => {
                let all = self.notes.authors();
                self.filter.author = match &self.filter.author {
                    None => all.first().cloned(),
                    Some(a) => all.iter().position(|x| x == a).and_then(|i| all.get(i + 1)).cloned(),
                };
                self.sel = 0;
            }
            'f' => {
                self.filter.pinned_only = !self.filter.pinned_only;
                self.sel = 0;
            }
            'w' => {
                self.filter.new_by_agent = !self.filter.new_by_agent;
                self.sel = 0;
            }
            'v' => {
                self.filter.status = match self.filter.status {
                    None => Some(Status::Archived),
                    Some(Status::Archived) => Some(Status::Superseded),
                    _ => None,
                };
                self.sel = 0;
            }
            'g' => {
                let next = self.notes.settings.agents_may_add.next();
                match self.change(|x| {
                    x.settings.agents_may_add = next;
                    Ok(())
                }) {
                    Ok(()) => self.say(format!("Agents may add to the context: {}.", next.label()), true),
                    Err(e) => self.say(e, false),
                }
            }
            's' => {
                let on = !self.notes.settings.share_external;
                match self.change(|x| {
                    x.settings.share_external = on;
                    Ok(())
                }) {
                    Ok(()) => self.say(if on { "Trusted entries are kept between the iyakki:context markers of CLAUDE.md (and AGENTS.md when it exists).".to_string() } else { "The context block was removed from CLAUDE.md / AGENTS.md; the text around it was not touched.".to_string() }, true),
                    Err(e) => self.say(e, false),
                }
            }
            _ => return NotesAct::None,
        }
        self.detail_scroll = 0;
        NotesAct::Used
    }

    // ------------------------------------------------------------------------------------------------ events

    pub fn handle(&mut self, ev: &Event) -> NotesAct {
        if self.edit.is_some() {
            return self.edit_event(ev);
        }
        if self.search_focus {
            match ev {
                Event::Key(Key::Esc) | Event::Key(Key::Enter) => {
                    self.search_focus = false;
                    return NotesAct::Used;
                }
                Event::Key(Key::Up) | Event::Key(Key::Down) => {}
                Event::Key(k) if !matches!(k, Key::Ctrl(_)) => {
                    self.search.handle(k);
                    self.sel = 0;
                    return NotesAct::Used;
                }
                Event::Paste(t) => {
                    self.search.paste(t);
                    self.sel = 0;
                    return NotesAct::Used;
                }
                _ => {}
            }
        }
        match ev {
            Event::FocusAt(FOCUS_SEARCH, x) => {
                self.search_focus = true;
                self.search.click(*x);
                NotesAct::Used
            }
            Event::Row(r) if (ROW_BASE..ROW_BASE + 100_000).contains(r) => {
                self.search_focus = false;
                self.sel = r - ROW_BASE;
                self.detail_scroll = 0;
                self.confirm_delete = None;
                if self.narrow {
                    self.page = true;
                }
                NotesAct::Used
            }
            Event::Scroll(d) => {
                if self.narrow && !self.page || !self.narrow && self.hover_list {
                    self.scroll = (self.scroll as i32 - d).clamp_safe(0, self.shown().len().saturating_sub(self.visible) as i32) as usize;
                } else {
                    self.detail_scroll = (self.detail_scroll as i32 - d * 2).max(0) as usize;
                }
                NotesAct::Used
            }
            Event::Key(k) | Event::Sub(SUB, k) => self.key(k),
            _ => NotesAct::None,
        }
    }

    fn key(&mut self, k: &Key) -> NotesAct {
        let n = self.shown().len();
        match k {
            Key::Char('/') => self.search_focus = true,
            Key::Up | Key::Char('K') => self.sel = self.sel.saturating_sub(1),
            Key::Down | Key::Char('J') => self.sel = (self.sel + 1).min(n.saturating_sub(1)),
            Key::Home => self.sel = 0,
            Key::End => self.sel = n.saturating_sub(1),
            Key::PageUp => self.sel = self.sel.saturating_sub(self.visible),
            Key::PageDown => self.sel = (self.sel + self.visible).min(n.saturating_sub(1)),
            Key::Enter => {
                if self.confirm_delete.is_some() {
                    return self.act('y');
                }
                if self.narrow {
                    self.page = true;
                } else {
                    return self.act('e');
                }
            }
            Key::Esc => {
                if self.confirm_delete.take().is_some() {
                    return NotesAct::Used;
                }
                if self.page {
                    self.page = false;
                    return NotesAct::Used;
                }
                if self.filter != Filter::default() || !self.search.value.is_empty() {
                    self.filter = Filter::default();
                    self.search.clear();
                    return NotesAct::Used;
                }
                return NotesAct::Back;
            }
            Key::Char(c) if self.confirm_delete.is_some() && matches!(c, 'y' | 'N') => return self.act(*c),
            Key::Char(c) if "nepaxtrR[]kufwvgsyN".contains(*c) => return self.act(*c),
            Key::Delete => return self.act('x'),
            _ => return NotesAct::None,
        }
        self.detail_scroll = 0;
        self.confirm_delete = None;
        NotesAct::Used
    }

    fn edit_event(&mut self, ev: &Event) -> NotesAct {
        let Some(e) = self.edit.as_mut() else { return NotesAct::None };
        match ev {
            Event::Key(Key::Esc) | Event::Sub(SUB, Key::Esc) => {
                self.edit = None;
                self.say("Not saved.", false);
            }
            Event::Key(Key::Ctrl('s')) | Event::Sub(SUB, Key::Ctrl('s')) => return self.save_edit(),
            Event::Sub(SUB, Key::Ctrl('k')) | Event::Key(Key::Ctrl('k')) => e.kind = e.kind.next(),
            Event::Key(Key::Tab) => {
                e.focus = match e.focus {
                    Field::Title => Field::Text,
                    Field::Text => Field::Source,
                    Field::Source => Field::Title,
                };
            }
            Event::Key(Key::BackTab) => {
                e.focus = match e.focus {
                    Field::Title => Field::Source,
                    Field::Text => Field::Title,
                    Field::Source => Field::Text,
                };
            }
            Event::FocusAt(FOCUS_TITLE, x) => {
                e.focus = Field::Title;
                e.title.click(*x);
            }
            Event::FocusAt(FOCUS_SOURCE, x) => {
                e.focus = Field::Source;
                e.source.click(*x);
            }
            Event::AreaAt(TEXT_AREA, x, y) => {
                e.focus = Field::Text;
                e.text.press(*x, *y);
            }
            Event::FocusAt(TEXT_AREA, x) => {
                e.focus = Field::Text;
                let pad = e.text.geo.map_or(0.0, |g| g.0);
                e.text.press(*x, pad);
            }
            Event::Paste(t) => match e.focus {
                Field::Title => {
                    e.title.paste(t.lines().next().unwrap_or(""));
                }
                Field::Text => {
                    e.text.paste(t);
                }
                Field::Source => {
                    e.source.paste(t.lines().next().unwrap_or(""));
                }
            },
            Event::Scroll(d) => {
                e.text.wheel(*d);
            }
            Event::Key(Key::Enter) if e.focus != Field::Text => {
                e.focus = if e.focus == Field::Title { Field::Text } else { Field::Title };
            }
            Event::Key(k) => {
                e.error = None;
                match e.focus {
                    Field::Title => {
                        e.title.handle(k);
                    }
                    Field::Text => {
                        e.text.key(k);
                    }
                    Field::Source => {
                        e.source.handle(k);
                    }
                }
            }
            _ => return NotesAct::None,
        }
        NotesAct::Used
    }

    // ------------------------------------------------------------------------------------------------ drawing

    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4], tick: u64) {
        self.caret_on = super::blink(tick);
        self.tick = tick;
        self.hover_list = false;
        let narrow = r.w < 620.0 * s;
        self.narrow = narrow;
        if !narrow {
            self.page = false;
        }
        let top = self.draw_header(g, r, s, accent, narrow);
        let body = R::new(r.x, r.y + top, r.w, (r.h - top).max(0.0));
        let list_w = if narrow { body.w } else { (body.w * 0.40).clamp_safe(240.0 * s, 460.0 * s) };
        // in a narrow pane the form is a page of its own
        let page = self.page || self.edit.is_some();
        let show_list = !narrow || !page;
        let show_detail = !narrow || page;
        if show_list {
            let lr = R::new(body.x, body.y, if show_detail { list_w - 6.0 * s } else { body.w }, body.h);
            self.hover_list = g.hovered(lr);
            self.draw_list(g, lr, s, focused, accent);
        }
        if show_detail {
            let dr = if narrow { body } else { R::new(body.x + list_w, body.y, body.w - list_w, body.h) };
            self.draw_detail(g, dr, s, focused, accent, narrow);
        }
    }

    /// The counts, the filters, the settings and the status line. Returns their height.
    fn draw_header(&mut self, g: &mut Gfx, r: R, s: f32, accent: [u8; 4], narrow: bool) -> f32 {
        let m = chrome::cur();
        let pad = 8.0 * s;
        let rowh = chrome::tool(s);
        let bounds = R::new(r.x - 300.0 * s, r.y, r.w + 600.0 * s, r.h + 100.0 * s);
        let cs = s * 0.85;
        let mut y = r.y + 3.0 * s;
        let (active, pinned, fresh) = self.notes.counts();
        // row 1: counts and the main buttons
        let mut x = r.x + pad;
        x += fx::chip(g, x, y + 1.0 * s, cs, &format!("{active} active"), CYAN, active > 0) + 5.0 * s;
        x += fx::chip(g, x, y + 1.0 * s, cs, &format!("{pinned} pinned"), MAGENTA, pinned > 0) + 5.0 * s;
        if fresh > 0 {
            let w = fx::chip(g, x, y + 1.0 * s, cs, &format!("{fresh} new by agents"), AMBER, true);
            g.hit(R::new(x, y, w, rowh), Event::Sub(SUB, Key::Char('w')), "show the entries agents added");
            x += w + 5.0 * s;
        }
        let mut bx = r.x + r.w - pad;
        let nb = "+ New entry";
        let bw = btn_w(g, s, nb);
        bx -= bw;
        chrome::text_btn(g, bx, y, s, nb, OK, !self.read_only, Event::Sub(SUB, Key::Char('n')), "New entry", Some("Add something every agent and teammate will remember (n)"), bounds);
        if fresh > 0 {
            let label = "Mark all reviewed";
            let w = btn_w(g, s, label);
            if bx - w - 6.0 * s > x {
                bx -= w + 4.0 * s;
                chrome::text_btn(g, bx, y, s, label, AMBER, true, Event::Sub(SUB, Key::Char('R')), "Mark all reviewed", Some("Clear the new-by-agent badges (R)"), bounds);
            }
        }
        y += rowh + 1.0 * s;
        let intro = g.fit(m.font_small * s, "Shared project context for the whole team and every agent. It is committed to git, so teammates and their agents get it.", Face::Sans, r.w - 2.0 * pad);
        g.text(r.x + pad, y, m.font_small * s, chrome::muted(), &intro, Face::Sans);
        y += g.line_height(m.font_small * s, Face::Sans) + 3.0 * s;
        // row 2: filters and the search box
        let mut x = r.x + pad;
        let filters = [
            ('k', format!("kind: {}", self.filter.kind.map_or("all".to_string(), |k| k.label().to_lowercase())), self.filter.kind.is_some()),
            ('u', format!("author: {}", self.filter.author.clone().unwrap_or_else(|| "all".into())), self.filter.author.is_some()),
            ('f', format!("pinned: {}", if self.filter.pinned_only { "only" } else { "any" }), self.filter.pinned_only),
            ('v', format!("show: {}", self.filter.status.map_or("active", |st| st.key())), self.filter.status.is_some()),
        ];
        let search_room = if narrow { 0.0 } else { (r.w * 0.22).clamp_safe(0.0, 220.0 * s) };
        for (c, label, on) in filters {
            let label = g.fit(m.font_small * s, &label, Face::Bold, 150.0 * s);
            let w = g.text_width(m.font_small * s, &label, Face::Bold) + 16.0 * s;
            if x + w > r.x + r.w - pad - search_room {
                break;
            }
            let b = R::new(x, y, w, rowh);
            let hot = g.hovered(b);
            if on || hot {
                g.rrect(b.x, b.y, b.w, b.h, 4.0 * s, if on { fx::over(fx::WELL, with_alpha(MAGENTA, 60)) } else { rgb(34, 44, 78) });
            }
            g.text(b.x + 8.0 * s, b.y + (b.h - g.line_height(m.font_small * s, Face::Bold)) / 2.0, m.font_small * s, if on { TEXT } else { fx::readable(MUTED, fx::PANEL, 4.5) }, &label, Face::Bold);
            g.hit(b, Event::Sub(SUB, Key::Char(c)), &label);
            x += w + 2.0 * s;
        }
        if search_room > 90.0 * s {
            let fr = R::new(r.x + r.w - pad - search_room, y, search_room, rowh - 2.0 * s);
            self.search.render(g, fr, self.search_focus, s * 0.8, self.caret_on, Face::Sans, FOCUS_SEARCH);
        }
        y += rowh + 3.0 * s;
        // row 3: the two settings, each a button that says what it is now
        let a = format!("Agents may add to the context: {}", self.notes.settings.agents_may_add.label());
        let sh = format!("Share with external agents via CLAUDE.md / AGENTS.md: {}", if self.notes.settings.share_external { "on" } else { "off" });
        let aw = btn_w(g, s, &a);
        let sw = btn_w(g, s, &sh);
        let mut x = r.x + pad - 8.0 * s;
        let aon = self.notes.settings.agents_may_add != AgentsMayAdd::Off;
        if aw + sw + 12.0 * s <= r.w - 2.0 * pad {
            x += chrome::text_btn(g, x, y, s, &a, if aon { fx::readable(OK, fx::PANEL, 4.5) } else { MUTED }, !self.read_only, Event::Sub(SUB, Key::Char('g')), &a, Some("Whether agents may add to the context: on with a review badge, ask each time, or off (g)"), bounds) + 6.0 * s;
            chrome::text_btn(g, x, y, s, &sh, if self.notes.settings.share_external { fx::readable(OK, fx::PANEL, 4.5) } else { MUTED }, !self.read_only, Event::Sub(SUB, Key::Char('s')), &sh, Some("Keep the trusted notes between markers in CLAUDE.md and AGENTS.md for external agents (s)"), bounds);
            y += rowh + 3.0 * s;
        } else {
            // narrow: shorter labels, one under the other
            let a2 = format!("Agents add entries: {}", self.notes.settings.agents_may_add.key());
            let s2 = format!("Share via CLAUDE.md/AGENTS.md: {}", if self.notes.settings.share_external { "on" } else { "off" });
            let w1 = chrome::text_btn(g, x, y, s, &a2, if aon { fx::readable(OK, fx::PANEL, 4.5) } else { MUTED }, !self.read_only, Event::Sub(SUB, Key::Char('g')), &a2, Some("Whether agents may add to the context (g)"), bounds);
            let w2 = btn_w(g, s, &s2);
            if x + w1 + 4.0 * s + w2 <= r.x + r.w - pad {
                chrome::text_btn(g, x + w1 + 4.0 * s, y, s, &s2, if self.notes.settings.share_external { fx::readable(OK, fx::PANEL, 4.5) } else { MUTED }, !self.read_only, Event::Sub(SUB, Key::Char('s')), &s2, Some("Share the trusted notes with external agents (s)"), bounds);
                y += rowh + 3.0 * s;
            } else {
                y += rowh + 1.0 * s;
                chrome::text_btn(g, x, y, s, &s2, if self.notes.settings.share_external { fx::readable(OK, fx::PANEL, 4.5) } else { MUTED }, !self.read_only, Event::Sub(SUB, Key::Char('s')), &s2, Some("Share the trusted notes with external agents (s)"), bounds);
                y += rowh + 3.0 * s;
            }
        }
        let shared = g.fit(m.font_small * s, "These settings are shared with everyone who clones this project.", Face::Sans, r.w - 2.0 * pad);
        g.text(r.x + pad, y, m.font_small * s, DIM, &shared, Face::Sans);
        y += g.line_height(m.font_small * s, Face::Sans) + 3.0 * s;
        // the one-line status or a problem with the file
        let line = if let Some(p) = &self.problem {
            Some((p.clone(), false))
        } else if self.notes.skipped > 0 {
            Some((format!("{} entr{} of context.json could not be read and will be dropped at the next change.", self.notes.skipped, if self.notes.skipped == 1 { "y" } else { "ies" }), false))
        } else {
            self.status.clone()
        };
        if let Some((msg, ok)) = line {
            let t = g.fit(m.font_small * s, &msg, Face::Sans, r.w - 2.0 * pad);
            g.text(r.x + pad, y, m.font_small * s, if ok { fx::readable(OK, fx::PANEL, 4.5) } else { fx::readable(ERR, fx::PANEL, 4.5) }, &t, Face::Sans);
            y += g.line_height(m.font_small * s, Face::Sans) + 3.0 * s;
        }
        let _ = accent;
        y - r.y
    }

    fn draw_list(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4]) {
        let m = chrome::cur();
        let pad = 8.0 * s;
        let list: Vec<Note> = self.shown().into_iter().cloned().collect();
        self.sel = self.sel.min(list.len().saturating_sub(1));
        if list.is_empty() {
            let lines = if self.notes.notes.is_empty() { ["The context is empty.", "Shared by the whole team and every agent of this project, and committed to git. Write down what everyone must remember: a decision and why, a hard limit, a convention, a trap you hit.", "Press n to add. Say \"remember that ...\" in the chat to add from there. Agents can add too (marked new until you look); everything is given to every run, pinned first."] } else { ["No entry matches.", "Esc clears the filters.", ""] };
            let mut y = r.y + 8.0 * s;
            for (i, l) in lines.iter().enumerate().filter(|(_, l)| !l.is_empty()) {
                for w in g.wrap(m.font_ui * s, l, Face::Sans, r.w - 2.0 * pad) {
                    g.text(r.x + pad, y, m.font_ui * s, if i == 0 { TEXT } else { MUTED }, &w, if i == 0 { Face::Bold } else { Face::Sans });
                    y += g.line_height(m.font_ui * s, Face::Sans) + 2.0 * s;
                }
                y += 6.0 * s;
            }
            return;
        }
        let ch = (m.row_h * s * 2.3).max(46.0 * s);
        self.visible = ((r.h / ch) as usize).max(1);
        if self.sel < self.scroll {
            self.scroll = self.sel;
        } else if self.sel >= self.scroll + self.visible {
            self.scroll = self.sel + 1 - self.visible;
        }
        self.scroll = self.scroll.min(list.len().saturating_sub(self.visible));
        let cs = s * 0.8;
        for (i, n) in list.iter().enumerate().skip(self.scroll).take(self.visible) {
            let y = r.y + (i - self.scroll) as f32 * ch;
            let rect = R::new(r.x + pad, y, r.w - 2.0 * pad - 6.0 * s, ch - 4.0 * s);
            let selected = i == self.sel;
            let hot = g.hovered(rect);
            let col = kind_color(n.kind);
            g.rrect(rect.x, rect.y, rect.w, rect.h, 6.0 * s, if selected { fx::over(rgb(22, 30, 58), with_alpha(accent, if focused { 50 } else { 24 })) } else if hot { rgb(20, 27, 50) } else { fx::WELL });
            g.rect(rect.x, rect.y + 3.0 * s, 3.0 * s, rect.h - 6.0 * s, col);
            let mut x = rect.x + 10.0 * s;
            if n.pinned {
                g.circle(x + 3.0 * s, rect.y + 11.0 * s, 3.0 * s, MAGENTA);
                x += 11.0 * s;
            }
            let kw0 = g.text_width(11.0 * s * 0.8, n.kind.label(), Face::Bold) + 16.0 * s;
            let kw = fx::chip(g, rect.x + rect.w - 6.0 * s - kw0, rect.y + 3.0 * s, cs, n.kind.label(), col, false);
            let title = g.fit(m.font_ui * s, &n.title, Face::Bold, rect.w - (x - rect.x) - kw - 16.0 * s);
            g.text(x, rect.y + 4.0 * s, m.font_ui * s, if n.status == Status::Active { TEXT } else { MUTED }, &title, Face::Bold);
            let badge = notes::badge(n);
            let l2 = format!("{} · {}{}{}", n.author, when(n.updated.max(n.created)), if n.source.is_empty() { String::new() } else { format!(" · {}", n.source) }, if n.status != Status::Active { format!(" · {}", n.status.key()) } else { String::new() });
            let bw = if badge.is_empty() { 0.0 } else { g.text_width(11.0 * s * 0.8, badge, Face::Bold) + 20.0 * s };
            let l2 = g.fit(m.font_small * s, &l2, Face::Sans, rect.w - 20.0 * s - bw);
            let ly = rect.y + rect.h - g.line_height(m.font_small * s, Face::Sans) - 5.0 * s;
            g.text(rect.x + 10.0 * s, ly, m.font_small * s, chrome::muted(), &l2, Face::Sans);
            if !badge.is_empty() {
                fx::chip(g, rect.x + rect.w - bw - 4.0 * s, ly - 3.0 * s, cs, badge, if n.is_new_by_agent() { AMBER } else { MUTED }, n.is_new_by_agent());
            }
            g.hit(rect, Event::Row(ROW_BASE + i), &format!("entry {}", n.id));
        }
        fx::scrollbar(g, R::new(r.x + r.w - 6.0 * s, r.y, 6.0 * s, r.h), s, self.scroll, self.visible, list.len(), accent);
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_detail(&mut self, g: &mut Gfx, r: R, s: f32, focused: bool, accent: [u8; 4], narrow: bool) {
        fx::panel(g, r, s, accent);
        if self.edit.is_some() {
            self.draw_edit(g, r, s, focused, accent, narrow);
            return;
        }
        let m = chrome::cur();
        let bounds = R::new(r.x - 300.0 * s, r.y, r.w + 600.0 * s, r.h + 100.0 * s);
        let pad = 10.0 * s;
        let rowh = chrome::tool(s);
        let mut y = r.y + 6.0 * s;
        if narrow {
            chrome::text_btn(g, r.x + 4.0 * s, y, s, "‹ Context", CYAN, true, Event::Sub(SUB, Key::Esc), "back to the list", Some("Back to the list (Esc)"), bounds);
            y += rowh + 2.0 * s;
        }
        let Some(n) = self.selected().cloned() else {
            g.text(r.x + pad, y + 8.0 * s, m.font_ui * s, MUTED, "Select an entry.", Face::Sans);
            return;
        };
        let col = kind_color(n.kind);
        let mut cx = r.x + pad;
        cx += fx::chip(g, cx, y, s * 0.85, n.kind.label(), col, true) + 5.0 * s;
        cx += fx::chip(g, cx, y, s * 0.85, n.status.key(), if n.status == Status::Active { OK } else { MUTED }, false) + 5.0 * s;
        if n.pinned {
            cx += fx::chip(g, cx, y, s * 0.85, "pinned", MAGENTA, true) + 5.0 * s;
        }
        if n.pin_requested && cx + 120.0 * s < r.x + r.w {
            cx += fx::chip(g, cx, y, s * 0.85, "pin requested", AMBER, false) + 5.0 * s;
        }
        let badge = notes::badge(&n);
        if !badge.is_empty() && cx + 100.0 * s < r.x + r.w {
            fx::chip(g, cx, y, s * 0.85, badge, if n.is_new_by_agent() { AMBER } else { MUTED }, n.is_new_by_agent());
        }
        y += 24.0 * s;
        let tsz = m.font_ui * s * 1.15;
        for l in g.wrap(tsz, &n.title, Face::Bold, r.w - 2.0 * pad).iter().take(2) {
            g.text(r.x + pad, y, tsz, TEXT, l, Face::Bold);
            y += g.line_height(tsz, Face::Bold) + 1.0 * s;
        }
        y += 3.0 * s;
        // buttons: what can be done with this note
        if let Some(id) = self.confirm_delete.clone().filter(|i| i == &n.id) {
            let msg = format!("Delete {id} for good?");
            g.text(r.x + pad, y + (rowh - g.line_height(m.font_small * s, Face::Bold)) / 2.0, m.font_small * s, ERR, &msg, Face::Bold);
            let mw = g.text_width(m.font_small * s, &msg, Face::Bold) + 8.0 * s;
            let w = chrome::text_btn(g, r.x + pad + mw, y, s, "Delete", ERR, true, Event::Sub(SUB, Key::Char('y')), "Delete", Some("Delete it for good (y or Enter)"), bounds);
            chrome::text_btn(g, r.x + pad + mw + w + 4.0 * s, y, s, "Keep", MUTED, true, Event::Sub(SUB, Key::Char('N')), "Keep", Some("Keep it (Esc)"), bounds);
            y += rowh + 4.0 * s;
        } else {
            let mut bx = r.x + pad - 8.0 * s;
            let ro = self.read_only;
            let mut buttons: Vec<(&str, char, [u8; 4], &str)> = vec![("Edit", 'e', CYAN, "Change the entry (e)"), (if n.pinned { "Unpin" } else { "Pin" }, 'p', MAGENTA, "Pinned entries come first in every agent's context (p)"), (if n.status == Status::Active { "Archive" } else { "Restore" }, 'a', AMBER, "Archived notes are kept but no longer given to agents (a)")];
            if n.by_agent {
                buttons.push((if n.trusted { "Untrust" } else { "Trust" }, 't', OK, "Trusted agent entries are followed like your own (t)"));
                if !n.reviewed {
                    buttons.push(("Reviewed", 'r', MUTED, "Clear the new-by-agent badge (r)"));
                }
            }
            buttons.push(("↑", '[', MUTED, "Move up ([)"));
            buttons.push(("↓", ']', MUTED, "Move down (])"));
            buttons.push(("Delete", 'x', ERR, "Delete the entry; asks first (x)"));
            for (label, c, color, tip) in buttons {
                let w = btn_w(g, s, label);
                if bx + w > r.x + r.w - 4.0 * s && bx > r.x + 40.0 * s {
                    bx = r.x + pad - 8.0 * s;
                    y += rowh + 2.0 * s;
                }
                bx += chrome::text_btn(g, bx, y, s, label, color, !ro, Event::Sub(SUB, Key::Char(c)), label, Some(tip), bounds) + 2.0 * s;
            }
            y += rowh + 4.0 * s;
        }
        chrome::focus_line(g, r.x + 1.0, y, r.w - 2.0, s, focused, accent);
        y += 6.0 * s;
        // the text, then the facts
        let body_h = (r.y + r.h - y - 4.0 * s).max(0.0);
        let size = m.font_ui * s;
        let lh = g.line_height(size, Face::Sans) + 2.0 * s;
        let mut lines: Vec<(String, [u8; 4], Face)> = Vec::new();
        if n.text.is_empty() {
            lines.push(("(no text: the title says it all)".into(), DIM, Face::Sans));
        }
        for para in n.text.lines() {
            if para.trim().is_empty() {
                lines.push((String::new(), TEXT, Face::Sans));
            }
            for l in g.wrap(size, para, Face::Sans, r.w - 2.0 * pad - 8.0 * s) {
                lines.push((l, TEXT, Face::Sans));
            }
        }
        lines.push((String::new(), TEXT, Face::Sans));
        let facts = [
            format!("{} by {}{}", n.id, n.author, if n.by_agent { " (agent)" } else { "" }),
            format!("created {} · updated {}", when(n.created), when(n.updated.max(n.created))),
            if n.source.is_empty() { String::new() } else { format!("source: {}", n.source) },
            if n.superseded_by.is_empty() { String::new() } else { format!("replaced by {}", n.superseded_by) },
            if n.by_agent && !n.trusted { "An agent wrote this. Agents see it as data, not as an instruction, until you trust it.".into() } else { String::new() },
        ];
        for f in facts.iter().filter(|f| !f.is_empty()) {
            for l in g.wrap(m.font_small * s, f, Face::Sans, r.w - 2.0 * pad - 8.0 * s) {
                lines.push((l, chrome::muted(), Face::Sans));
            }
        }
        let rows = ((body_h / lh) as usize).max(1);
        self.detail_scroll = self.detail_scroll.min(lines.len().saturating_sub(rows));
        for (i, (l, c, face)) in lines.iter().enumerate().skip(self.detail_scroll).take(rows) {
            let sz = if *c == chrome::muted() { m.font_small * s } else { size };
            g.text(r.x + pad, y + (i - self.detail_scroll) as f32 * lh, sz, *c, l, *face);
        }
        if lines.len() > rows {
            fx::scrollbar(g, R::new(r.x + r.w - 6.0 * s, y, 6.0 * s, body_h), s, self.detail_scroll, rows, lines.len(), accent);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_edit(&mut self, g: &mut Gfx, r: R, s: f32, _focused: bool, accent: [u8; 4], narrow: bool) {
        let m = chrome::cur();
        let bounds = R::new(r.x - 300.0 * s, r.y, r.w + 600.0 * s, r.h + 100.0 * s);
        let pad = 10.0 * s;
        let rowh = chrome::tool(s);
        let caret = self.caret_on;
        let tick_now = self.tick;
        let Some(e) = self.edit.as_mut() else { return };
        let mut y = r.y + 6.0 * s;
        let head = match &e.id {
            Some(i) => format!("Edit {i}"),
            None => "New entry".to_string(),
        };
        g.text(r.x + pad, y + 2.0 * s, m.font_ui * s * 1.1, TEXT, &head, Face::Bold);
        let kl = format!("kind: {}  (Ctrl+K)", e.kind.label());
        let kw = btn_w(g, s, &kl);
        chrome::text_btn(g, r.x + r.w - kw - 6.0 * s, y, s, &kl, kind_color(e.kind), true, Event::Sub(SUB, Key::Ctrl('k')), "kind", Some("Cycle the kind: decision, constraint, convention, gotcha, todo-later, reference"), bounds);
        y += rowh + 6.0 * s;
        let label = |g: &mut Gfx, y: f32, t: &str, on: bool| {
            g.text(r.x + pad, y, m.font_small * s, if on { fx::readable(accent, fx::PANEL, 4.5) } else { chrome::muted() }, t, Face::Bold);
        };
        label(g, y, "Title", e.focus == Field::Title);
        y += g.line_height(m.font_small * s, Face::Bold) + 2.0 * s;
        let fr = R::new(r.x + pad, y, r.w - 2.0 * pad, 26.0 * s);
        e.title.render(g, fr, e.focus == Field::Title, s * 0.85, caret, Face::Sans, FOCUS_TITLE);
        y += fr.h + 8.0 * s;
        label(g, y, &format!("Text (what to remember and why; at most {} characters)", notes::MAX_TEXT_PERSON), e.focus == Field::Text);
        y += g.line_height(m.font_small * s, Face::Bold) + 2.0 * s;
        // the box takes the room that is left above the source line and the buttons
        let below = g.line_height(m.font_small * s, Face::Bold) + 2.0 * s + 26.0 * s + 8.0 * s + rowh + 14.0 * s + if e.error.is_some() { 18.0 * s } else { 0.0 };
        let room = (r.y + r.h - y - below).max(40.0 * s);
        let size = m.font_ui * s;
        let lh = g.line_height(size, Face::Sans);
        e.text.min_rows = ((room / lh) as usize).clamp(2, 10);
        e.text.max_rows = e.text.min_rows;
        let used = e.text.render(g, r.x + pad, y, r.w - 2.0 * pad, s, size, 6.0 * s, 40.0 * s, e.focus == Field::Text, tick_now, "Write it so the next person or agent understands without asking.", accent, TEXT_AREA, "note text", false, true);
        y += used.max(room.min(used)) + 6.0 * s;
        label(g, y, "Source (optional)", e.focus == Field::Source);
        y += g.line_height(m.font_small * s, Face::Bold) + 2.0 * s;
        let fr = R::new(r.x + pad, y, r.w - 2.0 * pad, 26.0 * s);
        e.source.render(g, fr, e.focus == Field::Source, s * 0.85, caret, Face::Sans, FOCUS_SOURCE);
        y += fr.h + 8.0 * s;
        if let Some(err) = &e.error {
            let t = g.fit(m.font_small * s, err, Face::Sans, r.w - 2.0 * pad);
            g.text(r.x + pad, y, m.font_small * s, fx::readable(ERR, fx::PANEL, 4.5), &t, Face::Sans);
            y += 18.0 * s;
        }
        let w = chrome::text_btn(g, r.x + pad - 8.0 * s, y, s, "Save", OK, true, Event::Sub(SUB, Key::Ctrl('s')), "Save", Some("Save the entry (Ctrl+S)"), bounds);
        chrome::text_btn(g, r.x + pad - 8.0 * s + w + 4.0 * s, y, s, "Cancel", MUTED, true, Event::Sub(SUB, Key::Esc), "Cancel", Some("Discard the changes (Esc)"), bounds);
        let _ = narrow;
    }
}
