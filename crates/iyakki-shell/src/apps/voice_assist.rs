//! VOICE, DICTATE: the Assistant pane beside the transcript. The person gives an instruction (a chip, typed words, or one spoken instruction
//! recorded on their click) and the chat model proposes a revised text (`iyakki_agent::revise`). The proposal is shown with the changed words
//! marked; nothing changes until Apply. Applied revisions go on a bounded undo stack that lives in memory only and is cleared with the
//! transcript. If part of the transcript is selected, only that part is sent and replaced.

use crate::safeclamp::SafeClamp;
use std::sync::mpsc::{channel, Receiver};

use iyakki_agent::dictfix::Target;
use iyakki_agent::revise::{self, Kind, Op, Proposal};
use iyakki_agent::voicedesk::{self as vd, Dest};

use super::*;
use crate::fx;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

pub(crate) const T_A_TOGGLE: usize = 100;
pub(crate) const T_A_SEND: usize = 101;
pub(crate) const T_A_SAY: usize = 102;
pub(crate) const T_A_APPLY: usize = 103;
pub(crate) const T_A_DISCARD: usize = 104;
pub(crate) const T_A_UNDO: usize = 105;
pub(crate) const T_A_LANG: usize = 106;
/// The chips: `T_A_CHIP + index` of [`CHIPS`].
pub(crate) const T_A_CHIP: usize = 110;
pub(crate) const ASSIST_FIELD: usize = 62_010;

const WARN: [u8; 4] = rgb(255, 196, 94);
/// The most words of a proposal drawn (the whole text is still applied).
const SHOWN_WORDS: usize = 500;
/// The most lines the conversation log keeps.
const LOG_LINES: usize = 40;

pub(crate) const CHIPS: [&str; 8] = ["Fix grammar", "Shorter", "Longer", "Formal", "Friendly", "Bullets", "Summarise", "Translate"];

/// What was asked, waiting for a yes to send text to another machine, or on its way.
#[derive(Clone, Debug)]
pub(crate) struct Ask {
    pub kind: Kind,
    /// The characters of the transcript that were sent (all of it, or the selection).
    pub range: (usize, usize),
    pub sent: String,
}

/// A proposal on the table: not applied.
#[derive(Clone, Debug)]
pub(crate) struct Pending {
    pub ask: Ask,
    pub prop: Proposal,
    pub diff: Vec<revise::Seg>,
}

pub(crate) struct Assist {
    /// The conversation about THIS transcript: (the person said it, the words).
    pub log: Vec<(bool, String)>,
    pub input: TextField,
    pub history: revise::History,
    pub pending: Option<Pending>,
    pub working: bool,
    rx: Option<Receiver<(Ask, Proposal)>>,
    /// Waiting for the person's answer to the consent card.
    pub ask: Option<Ask>,
    pub langs: Vec<&'static str>,
    pub lang_i: usize,
    /// The recording that is running is one spoken instruction, not dictation.
    pub say_mode: bool,
    /// How far the log is scrolled up from its newest line, in pixels.
    pub scroll: f32,
}

impl Assist {
    pub fn new() -> Assist {
        let langs = vd::language_names();
        let lang_i = langs.iter().position(|l| *l == "English").unwrap_or(0);
        let mut input = TextField::new("Type an instruction, e.g. make it sound polite", false);
        input.max = revise::MAX_INSTRUCTION_CHARS;
        Assist { log: Vec::new(), input, history: revise::History::default(), pending: None, working: false, rx: None, ask: None, langs, lang_i, say_mode: false, scroll: 0.0 }
    }

    fn say(&mut self, mine: bool, t: impl Into<String>) {
        self.log.push((mine, t.into()));
        if self.log.len() > LOG_LINES {
            self.log.remove(0);
        }
        self.scroll = 0.0;
    }

    pub fn language(&self) -> &'static str {
        self.langs.get(self.lang_i).copied().unwrap_or("English")
    }
}

fn chip_kind(i: usize, lang: &str) -> Kind {
    match i {
        0 => Kind::FixGrammar,
        1 => Kind::Shorter,
        2 => Kind::Longer,
        3 => Kind::Formal,
        4 => Kind::Friendly,
        5 => Kind::Bullets,
        6 => Kind::Summarise,
        _ => Kind::Translate(lang.to_string()),
    }
}

impl VoiceApp {
    // ------------------------------------------------------------------------------------------------ the logic

    /// Forget the conversation, the proposal and the undo stack (the transcript was cleared).
    pub(crate) fn assist_reset(&mut self) {
        self.assist.log.clear();
        self.assist.history.clear();
        self.assist.pending = None;
        self.assist.ask = None;
        self.assist.rx = None;
        self.assist.working = false;
        self.assist.scroll = 0.0;
    }

    pub(crate) fn assist_open(&self, cx: &AppCx) -> bool {
        cx.services.voice_settings().assistant_open
    }

    fn assist_toggle(&mut self, cx: &AppCx) {
        let on = !self.assist_open(cx);
        match cx.services.settings.lock().unwrap_or_else(|e| e.into_inner()).update(|s| s.voice.assistant_open = on) {
            Ok(()) => cx.audit.record(&cx.session.user, "settings.voice.assistant_open", if on { "on" } else { "off" }),
            Err(e) => self.say(e.to_string(), false),
        }
        if !on && self.focus == Focus::Assist {
            self.focus = Focus::None;
        }
    }

    /// Send the typed instruction.
    pub(crate) fn assist_send(&mut self, cx: &AppCx) {
        let t = self.assist.input.value.trim().to_string();
        if t.is_empty() {
            self.assist.say(false, "Type what you want changed, or press a button below.");
            return;
        }
        self.assist_run(cx, Kind::Free(t));
    }

    /// Ask for a revision. A part of the transcript that is selected is the part sent; otherwise all of it.
    pub(crate) fn assist_run(&mut self, cx: &AppCx, kind: Kind) {
        if self.assist.working {
            return;
        }
        if !self.allowed(cx, "revise") {
            return;
        }
        let (range, sent) = match (self.transcript.selection(), self.transcript.selected_text()) {
            (Some(r), Some(t)) if !t.trim().is_empty() => (r, t),
            _ => ((0, self.transcript.len()), self.transcript.text.clone()),
        };
        if sent.trim().is_empty() {
            self.assist.say(false, "There is no text to revise yet. Dictate or paste some first.");
            return;
        }
        let ask = Ask { kind, range, sent };
        match revise::target(&cx.services, &revise::Context::default()) {
            Target::NeedsConsent(_, server, text) => {
                let address = server.split_once("://").map_or(server.clone(), |(_, r)| r.to_string());
                let place = iyakki_agent::speechtime::locality(&server);
                self.assist.ask = Some(ask);
                self.modal = Some(Modal::Consent { text, dest: Dest { name: "the assistant".into(), address, server, place }, then: After::Revise });
            }
            _ => self.assist_go(cx, ask),
        }
    }

    /// The person said yes to the machine: go on with the request that waited.
    pub(crate) fn assist_resume(&mut self, cx: &AppCx) {
        if let Some(ask) = self.assist.ask.take() {
            self.assist_go(cx, ask);
        }
    }

    /// The person said no to the machine.
    pub(crate) fn assist_declined(&mut self) {
        self.assist.ask = None;
        self.assist.say(false, "Nothing was sent. The assistant is on another machine; say yes when asked to use it, or choose a model on this computer.");
        self.say("Nothing was sent to the assistant.", true);
    }

    fn assist_go(&mut self, cx: &AppCx, ask: Ask) {
        let said = match &ask.kind {
            Kind::Free(t) => t.clone(),
            Kind::Translate(l) => format!("Translate to {l}"),
            k => revise::Kind::label(k).to_string(),
        };
        let part = if ask.range == (0, self.transcript.len()) { "" } else { " (selected part)" };
        self.assist.say(true, format!("{}{part}", capital(&said)));
        if matches!(ask.kind, Kind::Free(_)) {
            self.assist.input.set("");
        }
        self.assist.pending = None;
        cx.audit.record(&cx.session.user, revise::AUDIT, ask.kind.label());
        let (tx, rx) = channel();
        let svc = cx.services.clone();
        let a = ask.clone();
        std::thread::spawn(move || {
            let p = revise::revise(&svc, &a.sent, &a.kind, &revise::Context::default());
            let _ = tx.send((a, p));
        });
        self.assist.rx = Some(rx);
        self.assist.working = true;
        self.dirty = true;
    }

    /// Pick up a finished revision (called every tick).
    pub(crate) fn assist_tick(&mut self) {
        let Some(rx) = &self.assist.rx else { return };
        let Ok((ask, prop)) = rx.try_recv() else { return };
        self.assist.rx = None;
        self.assist.working = false;
        self.dirty = true;
        if let Some(why) = &prop.kept_because {
            self.assist.say(false, format!("{why}."));
        } else if !prop.changed {
            self.assist.say(false, prop.note.clone());
        } else {
            self.assist.say(false, format!("{} Check the change below, then press Apply.", prop.note));
            let diff = revise::word_diff(&ask.sent, &prop.text);
            self.assist.pending = Some(Pending { ask, prop, diff });
        }
    }

    pub(crate) fn assist_apply(&mut self) {
        let Some(p) = self.assist.pending.take() else { return };
        match revise::splice(&self.transcript.text, p.ask.range, &p.ask.sent, &p.prop.text) {
            Ok(new) => {
                self.assist.history.push(&self.transcript.text);
                self.transcript.set(&new);
                self.fixes_forget();
                self.assist.say(false, "Applied. Undo brings the earlier text back.");
                self.focus = Focus::Transcript;
            }
            Err(why) => {
                self.assist.say(false, why);
            }
        }
    }

    pub(crate) fn assist_discard(&mut self) {
        if self.assist.pending.take().is_some() {
            self.assist.say(false, "Discarded. The text is unchanged.");
        }
    }

    pub(crate) fn assist_undo(&mut self) {
        match self.assist.history.undo() {
            Some(prev) => {
                self.transcript.set(&prev);
                self.fixes_forget();
                self.assist.pending = None;
                let left = self.assist.history.len();
                self.assist.say(false, format!("Undone. {left} earlier {} can still be undone.", if left == 1 { "step" } else { "steps" }));
            }
            None => self.assist.say(false, "Nothing to undo."),
        }
    }

    /// Record ONE spoken instruction with the recorder and the speech engine; it ends on the pause detector (or Stop) and lands in the
    /// instruction line. It never runs by itself.
    pub(crate) fn assist_say(&mut self, cx: &AppCx) {
        if self.cap.is_some() || self.transcribing {
            return;
        }
        self.start_record_for(cx, true);
    }

    /// The speech engine's text of a spoken instruction: into the line, for the person to send or edit.
    pub(crate) fn finish_instruction(&mut self, r: Result<String, String>) {
        self.assist.say_mode = false;
        self.transcribing = false;
        self.stt_rx = None;
        match r {
            Ok(t) if !t.trim().is_empty() => {
                let t: String = t.trim().chars().take(revise::MAX_INSTRUCTION_CHARS).collect();
                self.assist.input.set(&t);
                self.focus = Focus::Assist;
                self.say("Instruction heard. Check it in the Assistant line, then press Send (it has not run).", true);
            }
            Ok(_) => self.say("I did not hear an instruction. Press Say an instruction and try again.", false),
            Err(e) => self.say(e, false),
        }
    }

    /// Keys while the instruction line has the keyboard.
    pub(crate) fn assist_key(&mut self, k: &Key, cx: &AppCx) {
        match k {
            Key::Enter => self.assist_send(cx),
            Key::Ctrl('z' | 'Z') => self.assist_undo(),
            Key::PageUp => self.assist.scroll += 80.0,
            Key::PageDown => self.assist.scroll = (self.assist.scroll - 80.0).max(0.0),
            Key::Tab => self.focus = Focus::Transcript,
            other => {
                self.assist.input.handle(other);
            }
        }
    }

    /// A click on one of the pane's buttons. Returns whether it was one.
    pub(crate) fn assist_click(&mut self, n: usize, cx: &AppCx) -> bool {
        match n {
            T_A_TOGGLE => self.assist_toggle(cx),
            T_A_SEND => self.assist_send(cx),
            T_A_SAY => self.assist_say(cx),
            T_A_APPLY => self.assist_apply(),
            T_A_DISCARD => self.assist_discard(),
            T_A_UNDO => self.assist_undo(),
            T_A_LANG => {
                let items = self.assist.langs.iter().map(|l| l.to_string()).collect();
                self.modal = Some(Modal::LangPick(super::super::widgets::Picker::new("Translate into", items)));
            }
            n if (T_A_CHIP..T_A_CHIP + CHIPS.len()).contains(&n) => {
                let kind = chip_kind(n - T_A_CHIP, self.assist.language());
                self.assist_run(cx, kind);
            }
            _ => return false,
        }
        true
    }

    // ------------------------------------------------------------------------------------------------ the drawing

    /// The row that opens the pane again (shown instead of the pane while it is collapsed).
    pub(crate) fn draw_assist_closed(&mut self, g: &mut Gfx, x: f32, y: f32, s: f32) -> f32 {
        let bs = 13.0 * s;
        let bw = g.button(x, y, bs, "Assistant \u{25B8}", ACCENT, Event::Tab(T_A_TOGGLE));
        g.text(x + bw + 10.0 * s, y + (bs * 2.1 - g.line_height(12.5 * s, Face::Sans)) / 2.0, 12.5 * s, DIM, "revise the transcript: shorter, formal, translate, your own words", Face::Sans);
        bs * 2.1 + 8.0 * s
    }

    /// The pane inside `r`: header, the conversation and the proposal above, the buttons and the instruction line below. Everything is
    /// clipped to `r`, so it holds at any size; the conversation shows its newest lines.
    pub(crate) fn draw_assist(&mut self, g: &mut Gfx, r: R, s: f32, tick: u64) {
        fx::panel(g, r, s, ACCENT);
        let pad = 10.0 * s;
        let inner = R::new(r.x + pad, r.y + pad, (r.w - 2.0 * pad).max(40.0), (r.h - 2.0 * pad).max(20.0));
        g.clipped(r, |g| {
            let bs = 12.5 * s;
            let hh = bs * 2.1;
            g.text(inner.x, inner.y + (hh - g.line_height(15.0 * s, Face::Bold)) / 2.0, 15.0 * s, TEXT, "Assistant", Face::Bold);
            let hw = g.text_width(bs, "Hide \u{25BE}", Face::Bold) + bs * 1.7;
            g.button(inner.x + inner.w - hw, inner.y, bs, "Hide \u{25BE}", MUTED, Event::Tab(T_A_TOGGLE));
            let top = inner.y + hh + 6.0 * s;
            // the bottom block is laid out from the bottom
            let bh = self.assist_bottom(g, inner.x, 0.0, inner.w, s, tick, false);
            let by = inner.y + inner.h - bh;
            self.assist_bottom(g, inner.x, by, inner.w, s, tick, true);
            let flex = R::new(inner.x, top, inner.w, (by - top - 4.0 * s).max(0.0));
            if flex.h > 8.0 * s {
                let size = 13.0 * s;
                let total = self.assist_content(g, flex.x, 0.0, flex.w, size, s, false);
                let max_up = (total - flex.h).max(0.0);
                self.assist.scroll = self.assist.scroll.clamp_safe(0.0, max_up);
                let y0 = flex.y + flex.h - total + self.assist.scroll;
                g.clipped(flex, |g| {
                    self.assist_content(g, flex.x, y0, flex.w, size, s, true);
                });
            }
        });
    }

    /// The conversation and the proposal. Returns the height.
    #[allow(clippy::too_many_arguments)]
    fn assist_content(&self, g: &mut Gfx, x: f32, y0: f32, w: f32, size: f32, s: f32, draw: bool) -> f32 {
        let lh = g.line_height(size, Face::Sans) * 1.2;
        let mut y = y0;
        if self.assist.log.is_empty() && self.assist.pending.is_none() && !self.assist.working {
            let hint = "Ask for a change to the transcript: press a button below or type what you want. Select part of the text to change only that part. Nothing changes until you press Apply.";
            for l in g.wrap(size, hint, Face::Sans, w) {
                if draw {
                    g.text(x, y, size, DIM, &l, Face::Sans);
                }
                y += lh;
            }
        }
        for (mine, t) in &self.assist.log {
            let prefix = if *mine { "You: " } else { "" };
            let col = if *mine { ACCENT } else { TEXT };
            for (i, l) in g.wrap(size, &format!("{prefix}{t}"), Face::Sans, w).iter().enumerate() {
                if draw {
                    g.text(x, y, size, if i == 0 || !*mine { col } else { MUTED }, l, Face::Sans);
                }
                y += lh;
            }
            y += 3.0 * s;
        }
        if self.assist.working {
            if draw {
                g.text(x, y, size, WARN, &format!("Working on it{}", ".".repeat(1 + (self.ticks / 10 % 3) as usize)), Face::Sans);
            }
            y += lh;
        }
        if let Some(p) = &self.assist.pending {
            if draw {
                g.text(x, y, size, MUTED, "Proposed change (new words marked, removed words struck):", Face::Sans);
            }
            y += lh + 2.0 * s;
            y += diff_flow(g, x, y, w, size, &p.diff, draw);
        }
        y - y0
    }

    /// The Apply / Discard / Undo row, the instruction line, Say / Send and the chips. Returns the height.
    #[allow(clippy::too_many_arguments)]
    fn assist_bottom(&mut self, g: &mut Gfx, x: f32, y0: f32, w: f32, s: f32, tick: u64, draw: bool) -> f32 {
        let bs = 12.5 * s;
        let (h, gap) = (bs * 2.1, 6.0 * s);
        let mut y = y0;
        let pending = self.assist.pending.is_some();
        let can_undo = !self.assist.history.is_empty();
        let items: [(&str, usize, bool, [u8; 4]); 3] = [("Apply", T_A_APPLY, pending, OK), ("Discard", T_A_DISCARD, pending, MUTED), ("Undo", T_A_UNDO, can_undo, ACCENT)];
        y += wrap_buttons(g, x, y, w, bs, &items, draw) + gap;
        // the instruction line
        let fh = 30.0 * s;
        if draw {
            self.assist.input.render(g, R::new(x, y, w, fh), self.focus == Focus::Assist, s, crate::apps::blink(tick), Face::Sans, ASSIST_FIELD);
        }
        y += fh + gap;
        let busy = self.assist.working;
        let recording = self.cap.is_some() && self.assist.say_mode;
        let can_say = !self.mics.is_empty() && self.cap.is_none() && !self.transcribing && !busy;
        let say_label = if recording { "Listening\u{2026}" } else { "Say an instruction" };
        let items: [(&str, usize, bool, [u8; 4]); 2] = [(say_label, T_A_SAY, can_say, ACCENT), ("Send", T_A_SEND, !busy && !self.assist.input.value.trim().is_empty(), OK)];
        y += wrap_buttons(g, x, y, w, bs, &items, draw) + gap;
        // the chips, then the language for Translate
        let lang = format!("Into: {} \u{25BE}", self.assist.language());
        let mut chips: Vec<(&str, usize, bool, [u8; 4])> = CHIPS.iter().enumerate().map(|(i, c)| (*c, T_A_CHIP + i, !busy, MUTED)).collect();
        chips.push((lang.as_str(), T_A_LANG, true, MUTED));
        y += wrap_buttons(g, x, y, w, bs, &chips, draw);
        let _ = h;
        y - y0
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map_or(String::new(), |f| f.to_uppercase().collect::<String>() + c.as_str())
}

/// Buttons left to right, wrapping when the row is full. Returns the height used.
fn wrap_buttons(g: &mut Gfx, x: f32, y: f32, w: f32, size: f32, items: &[(&str, usize, bool, [u8; 4])], draw: bool) -> f32 {
    let (h, gap) = (size * 2.1, 6.0 * size / 12.5);
    let (mut cx, mut cy) = (x, y);
    for (label, n, on, color) in items {
        let bw = g.text_width(size, label, Face::Bold) + size * 1.7;
        if cx > x && cx + bw > x + w {
            cx = x;
            cy += h + gap;
        }
        if draw {
            if *on {
                g.button(cx, cy, size, label, *color, Event::Tab(*n));
            } else {
                g.button_off(cx, cy, size, label);
            }
        }
        cx += bw + gap;
    }
    cy + h - y
}

/// The words of a diff flowed into `w`: new words on a green ground, removed words dim and struck through.
fn diff_flow(g: &mut Gfx, x: f32, y0: f32, w: f32, size: f32, segs: &[revise::Seg], draw: bool) -> f32 {
    let lh = g.line_height(size, Face::Sans) * 1.25;
    let space = g.text_width(size, " ", Face::Sans);
    let (mut cx, mut cy) = (x, y0);
    let mut shown = 0usize;
    'outer: for seg in segs {
        for word in seg.text.split_whitespace() {
            if shown >= SHOWN_WORDS {
                if draw {
                    g.text(cx, cy, size, DIM, "\u{2026}", Face::Sans);
                }
                cy += lh;
                break 'outer;
            }
            shown += 1;
            let ww = g.text_width(size, word, Face::Sans);
            if cx > x && cx + ww > x + w {
                cx = x;
                cy += lh;
            }
            if draw {
                match seg.op {
                    Op::Same => {
                        g.text(cx, cy, size, TEXT, word, Face::Sans);
                    }
                    Op::Added => {
                        g.rect(cx - 1.0, cy, ww + 2.0, lh * 0.95, fx::with_alpha(OK, 60));
                        g.text(cx, cy, size, OK, word, Face::Sans);
                    }
                    Op::Removed => {
                        g.text(cx, cy, size, ERR, word, Face::Sans);
                        g.rect(cx, cy + lh * 0.42, ww, (size / 14.0).max(1.0), ERR);
                    }
                }
            }
            cx += ww + space;
        }
    }
    cy + lh - y0
}
