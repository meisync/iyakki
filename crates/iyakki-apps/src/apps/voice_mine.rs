//! VOICE, ASSISTANT (fifth tab, F5): "My IYAKKI", the push-to-talk voice assistant joined to the chat agent (F-VOICE-28).
//!
//! The logic is `iyakki_agent::voiceassist` (the state machine) and `voiceassist_ports` (the real microphone, speech engine, speaker and
//! agent, on a worker thread); this file is the opt-in, the tap and the keys, the indicator and the approval card. The microphone opens
//! only after a tap or key of the person, and the indicator below follows the real state of the open microphone, never a guess. A tool
//! that the hard-ask list names is approved here by a tap or key only, never by voice.

use crate::safeclamp::SafeClamp;
use std::sync::Arc;
use std::time::Duration;

use iyakki_shared::agent::{RunLimits};
use iyakki_shared::agent::{Tools};
use iyakki_shared::assistrun::Assist;
use iyakki_shared::voiceassist::{Config, State, Status};
use iyakki_shared::voiceassist::Ports;
use iyakki_shared::voiceassist_ports::{Cmd, Live};
use iyakki_agent_api::MineWorker;

use super::voice::{VoiceApp, ACCENT};
use super::{AppCx, Flow};
use crate::fx;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, DIM, ERR, MUTED, OK, R, TEXT};

pub(super) const T_TAB_MINE: usize = 5;
pub(super) const T_MINE_TALK: usize = 70;
pub(super) const T_MINE_STOP: usize = 71;
pub(super) const T_MINE_MUTE: usize = 72;
pub(super) const T_MINE_ON: usize = 73;
pub(super) const T_MINE_OFF: usize = 74;
pub(super) const T_MINE_ALLOW: usize = 75;
pub(super) const T_MINE_DENY: usize = 76;
pub(super) const T_MINE_YES: usize = 77;
pub(super) const T_MINE_NO: usize = 78;
pub(super) const T_MINE_FALSE: usize = 79;
pub(super) const T_MINE_MISS: usize = 80;
pub(super) const T_MINE_STREAM_YES: usize = 81;
pub(super) const T_MINE_STREAM_NO: usize = 82;

const REC: [u8; 4] = rgb(255, 82, 92);
const WARN: [u8; 4] = rgb(255, 196, 94);

/// The state of the Assistant tab.
pub(super) struct Mine {
    worker: Option<Box<dyn MineWorker>>,
    /// Watches the pending-work ledger while the assistant runs: finished work is spoken about later (F-VOICE-37).
    waker: Option<Box<dyn iyakki_agent_api::pending::PendingWaker>>,
    pub st: Option<Status>,
    pub note: String,
}

impl Mine {
    pub fn new() -> Mine {
        Mine { worker: None, waker: None, st: None, note: String::new() }
    }
}

const BASE_SYSTEM: &str = "You are IYAKKI's built-in assistant on this computer. Help the person with what they ask, using your tools when that is the right way, and answer briefly.";

impl VoiceApp {
    pub(super) fn mine_active(&self) -> bool {
        self.mine.st.as_ref().is_some_and(|s| s.mic_open || !matches!(s.state, State::Idle | State::Muted))
    }

    /// The worker, started on first use (only when the person turned the assistant on).
    fn mine_worker(&mut self, cx: &AppCx) -> Option<&dyn MineWorker> {
        if self.mine.worker.is_none() {
            if !cx.session.policy.voice {
                self.mine.note = "Voice is not enabled for your account.".into();
                return None;
            }
            let (user, audit) = (cx.session.user.clone(), cx.audit.clone());
            let audit_line: Arc<dyn Fn(&str, &str, &str) + Send + Sync> = Arc::new(move |u, a, d| audit.record(u, a, d));
            self.mine.worker = cx.b4().voice_assistant_spawn(&user, cx.session.policy.clone(), cx.host.clone(), cx.mcp_servers.clone(), cx.chat.clone(), audit_line, self.rig.recorders.clone());
        }
        self.mine.worker.as_deref()
    }

    /// Called every heartbeat: copy the worker's state for drawing; close the worker when the assistant was turned off.
    pub(super) fn mine_tick(&mut self, cx: &AppCx) {
        // the wake phrase was heard (the shell opened this tile for it): show the Assistant tab and tap Talk, the same as the person would
        if cx.agent.wakelisten_take_session_request() {
            if cx.voice_settings().my_on {
                self.switch(super::voice::Area::Mine, cx);
                self.mine_click(T_MINE_TALK, cx);
            }
        }
        // the consent gate follows the Settings switch and the mute; its answer is the person's own press, never anything else
        cx.services.cloud_stream_sync();
        if !cx.voice_settings().my_on {
            if self.mine.worker.take().is_some() {
                self.mine.st = None;
                self.dirty = true;
            }
            return;
        }
        // work the person waited for has ended: the waker's text goes to the assistant, which says the answer when it is free (F-VOICE-37)
        if let Some(w) = &self.mine.worker {
            if self.mine.waker.is_none() {
                self.mine.waker = cx.pendingapi().pending_waker(&cx.session.user, "", None);
            }
            if let Some(k) = &self.mine.waker {
                if k.ready() {
                    if let Some(iyakki_shared::pending::Wake::Turn { text, ids }) = k.take() {
                        cx.audit.record(&cx.session.user, "pending.wake.voice", &ids.join(","));
                        w.wake(&text);
                    }
                }
            }
        } else {
            self.mine.waker = None;
        }
        if let Some(w) = &self.mine.worker {
            let st = w.status();
            self.dirty |= self.mine.st.as_ref().is_none_or(|o| o.state != st.state || o.mic_open != st.mic_open || o.heard != st.heard || o.said != st.said || o.note != st.note || o.approval != st.approval || o.confirm != st.confirm || o.muted != st.muted);
            self.mine.st = Some(st);
        }
    }

    fn mine_set_on(&mut self, on: bool, cx: &AppCx) {
        if !cx.session.policy.manage_settings {
            self.mine.note = "Only an administrator can turn My IYAKKI on or off (Settings -> Voice).".into();
            return;
        }
        let r = cx.settings().lock().unwrap_or_else(|e| e.into_inner()).update(|s| s.voice.my_on = on);
        match r {
            Ok(_) => {
                cx.audit.record(&cx.session.user, "voice.assistant", if on { "on" } else { "off" });
                self.mine.note = if on { "My IYAKKI is on. Press Talk or Space when you want to speak.".into() } else { "My IYAKKI is off.".into() };
                if !on {
                    self.mine.worker = None;
                    self.mine.st = None;
                }
            }
            Err(e) => self.mine.note = e.to_string(),
        }
    }

    fn mine_toggle_mute(&mut self, cx: &AppCx) {
        let now = !cx.voice_settings().my_muted;
        let _ = cx.settings().lock().unwrap_or_else(|e| e.into_inner()).update(|s| s.voice.my_muted = now);
        cx.audit.record(&cx.session.user, "voice.assistant", if now { "muted" } else { "unmuted" });
    }

    pub(super) fn mine_click(&mut self, n: usize, cx: &AppCx) {
        match n {
            T_MINE_ON => self.mine_set_on(true, cx),
            T_MINE_OFF => self.mine_set_on(false, cx),
            T_MINE_MUTE => self.mine_toggle_mute(cx),
            T_MINE_TALK => {
                if cx.voice_settings().my_on {
                    if let Some(w) = self.mine_worker(cx) {
                        w.send(Cmd::Tap);
                    }
                }
            }
            T_MINE_STOP => {
                if let Some(w) = &self.mine.worker {
                    w.send(Cmd::Stop);
                }
            }
            T_MINE_ALLOW | T_MINE_DENY => {
                let yes = n == T_MINE_ALLOW;
                let id = self.mine.st.as_ref().and_then(|s| s.approval.as_ref().map(|a| a.id.clone()));
                if let (Some(id), Some(w)) = (id, &self.mine.worker) {
                    w.send(Cmd::Approve(id, yes));
                }
            }
            T_MINE_FALSE | T_MINE_MISS => {
                // the person's own count of a wrong wake and of a missed one (numbers only, `wakestats`)
                let (kind, what) = if n == T_MINE_FALSE { ("false_wake", "false-wake") } else { ("miss", "miss") };
                match cx.services.wake_mark(kind) {
                    Ok(()) => {
                        cx.audit.record(&cx.session.user, "voice.wake", &format!("marked {what}"));
                        self.mine.note = if n == T_MINE_FALSE { "Counted as a false wake. Thank you.".into() } else { "Counted as a miss. Thank you.".into() };
                    }
                    Err(e) => self.mine.note = e,
                }
            }
            T_MINE_STREAM_YES | T_MINE_STREAM_NO => {
                // the answer to the consent card: only this tap or key, never a spoken word
                cx.services.cloud_stream_press(n == T_MINE_STREAM_YES);
                cx.audit.record(&cx.session.user, "voice.cloud_stream", if n == T_MINE_STREAM_YES { "card: allowed for this session" } else { "card: refused for this session" });
            }
            T_MINE_YES | T_MINE_NO => {
                if let Some(w) = &self.mine.worker {
                    w.send(Cmd::Confirm(n == T_MINE_YES));
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    /// Keys of the Assistant tab: Space or Enter talks (and ends the request), s stops, m mutes, y/n answer a card.
    pub(super) fn mine_key(&mut self, k: &Key, cx: &AppCx) -> Option<Flow> {
        let st = self.mine.st.clone();
        let card = st.as_ref().is_some_and(|s| s.approval.is_some());
        let ask = st.as_ref().is_some_and(|s| s.confirm.is_some());
        let stream = cx.services.cloud_stream_card().is_some() && !card && !ask;
        let n = match k {
            Key::Char('y') if stream => T_MINE_STREAM_YES,
            Key::Char('n') if stream => T_MINE_STREAM_NO,
            Key::Char(' ') | Key::Enter if !card && !ask => T_MINE_TALK,
            Key::Char('s') => T_MINE_STOP,
            Key::Char('m') => T_MINE_MUTE,
            Key::Char('y' | 'a') if card => T_MINE_ALLOW,
            Key::Char('n' | 'd') if card => T_MINE_DENY,
            Key::Char('y') if ask => T_MINE_YES,
            Key::Char('n') if ask => T_MINE_NO,
            Key::Esc if self.mine_active() => T_MINE_STOP,
            _ => return None,
        };
        self.mine_click(n, cx);
        Some(Flow::Stay)
    }

    pub(super) fn draw_mine(&mut self, g: &mut Gfx, body: R, s: f32, cx: &AppCx, tick: u64) {
        let (x, w) = (body.x, body.w);
        let mut y = body.y;
        let size = 14.0 * s;
        let gap = 8.0 * s;
        let v = cx.voice_settings();
        let st = self.mine.st.clone();
        let para = |g: &mut Gfx, y: f32, c: [u8; 4], sz: f32, t: &str, max: usize| -> f32 {
            let lines = g.wrap(sz, t, Face::Sans, w);
            let lh = g.line_height(sz, Face::Sans) * 1.2;
            for (i, l) in lines.iter().take(max).enumerate() {
                g.text(x, y + i as f32 * lh, sz, c, l, Face::Sans);
            }
            lines.len().clamp_safe(1, max) as f32 * lh + 2.0 * s
        };
        if !v.my_on {
            y += para(g, y, TEXT, size, "My IYAKKI is a voice assistant joined to the chat assistant: you press Talk, speak, and it answers aloud and can use its tools. It is off.", 4) + gap;
            y += para(g, y, MUTED, 13.0 * s, "When on: the microphone opens only after your tap or key, and a red dot shows whenever it is open. Your voice goes to the speech engine of Settings -> Voice (on this computer unless you agreed to another machine). No sound is kept. A tool that asks you shows a card here; the risky ones (deleting, installing, sending, sharing) can only be approved by a tap or key, never by saying yes. Say stop, cancel, louder, quieter, mute, repeat or that's all at any time. The wake phrase is not built yet.", 9) + gap;
            if cx.session.policy.manage_settings {
                g.button(x, y, size, "Turn on My IYAKKI", ACCENT, Event::Tab(T_MINE_ON));
            } else {
                para(g, y, WARN, size, "Only an administrator can turn it on (Settings -> Voice).", 2);
            }
            if !self.mine.note.is_empty() {
                para(g, y + size * 3.0, DIM, 13.0 * s, &self.mine.note.clone(), 3);
            }
            return;
        }
        let state = st.as_ref().map_or(if v.my_muted { State::Muted } else { State::Idle }, |s| s.state);
        let mic = st.as_ref().is_some_and(|s| s.mic_open);
        // the indicator: a red dot only while the microphone is really open
        let ph = 66.0 * s;
        let panel = R::new(x, y, w, ph);
        fx::panel(g, panel, s, if mic { REC } else { ACCENT });
        let bh = size * 2.1;
        let cy = y + 10.0 * s + bh / 2.0;
        if mic {
            let pulse = if (tick / 15) % 2 == 0 { 255 } else { 150 };
            g.circle(x + 20.0 * s, cy, 7.0 * s, [REC[0], REC[1], REC[2], pulse]);
            g.circle(x + 20.0 * s, cy, 12.0 * s, fx::with_alpha(REC, 50));
        } else {
            g.circle(x + 20.0 * s, cy, 6.0 * s, if state == State::Muted { WARN } else { DIM });
        }
        let label = if mic { format!("MICROPHONE OPEN: {}", state.label()) } else { format!("Microphone off: {}", state.label()) };
        let talk = if mic || state == State::Speaking || state == State::Thinking { "Stop" } else { "● Talk" };
        let bw = g.text_width(size, talk, Face::Bold) + size * 1.7;
        let bx = x + w - bw - 12.0 * s;
        let label = g.fit(15.0 * s, &label, Face::Bold, (bx - x - 44.0 * s).max(40.0 * s));
        g.text(x + 38.0 * s, cy - g.line_height(15.0 * s, Face::Bold) / 2.0, 15.0 * s, if mic { REC } else { TEXT }, &label, Face::Bold);
        if state == State::Muted {
            g.button_off(bx, y + 10.0 * s, size, talk);
        } else if talk == "Stop" {
            g.button(bx, y + 10.0 * s, size, talk, REC, Event::Tab(T_MINE_STOP));
        } else {
            g.button(bx, y + 10.0 * s, size, talk, REC, Event::Tab(T_MINE_TALK));
        }
        let mr = R::new(x + 16.0 * s, y + ph - 20.0 * s, w - 32.0 * s, 10.0 * s);
        fx::progress_bar(g, mr, st.as_ref().map_or(0.0, |x| x.level).clamp(0.0, 1.0), OK, s);
        y += ph + gap;

        // the consent card for sending microphone sound to a cloud machine (F-VOICE-38): a tap or key answers it, never a spoken yes
        if let Some(text) = cx.services.cloud_stream_card() {
            let lines = g.wrap(size, &text, Face::Sans, w - 28.0 * s);
            let lh = g.line_height(size, Face::Sans) * 1.2;
            let ch = lines.len().min(6) as f32 * lh + bh + 30.0 * s;
            fx::panel(g, R::new(x, y, w, ch), s, WARN);
            for (i, l) in lines.iter().take(6).enumerate() {
                g.text(x + 14.0 * s, y + 10.0 * s + i as f32 * lh, size, TEXT, l, Face::Sans);
            }
            let by = y + 14.0 * s + lines.len().min(6) as f32 * lh;
            let aw = g.button(x + 14.0 * s, by, size, "Allow", OK, Event::Tab(T_MINE_STREAM_YES));
            g.button(x + 14.0 * s + aw + 10.0 * s, by, size, "Do not allow", ERR, Event::Tab(T_MINE_STREAM_NO));
            y += ch + gap;
        }
        // the approval card and the read-back card
        if let Some(a) = st.as_ref().and_then(|s| s.approval.clone()) {
            let text = format!("IYAKKI wants to: {}", a.summary);
            let lines = g.wrap(size, &text, Face::Sans, w - 28.0 * s);
            let lh = g.line_height(size, Face::Sans) * 1.2;
            let note = if a.hard {
                "This one can only be approved by a tap or key (Y), never by voice. Saying no works."
            } else if !a.voice_ok {
                "Tap Allow or press Y. Spoken approvals are off (Settings -> Voice). Saying no works."
            } else {
                "Say yes, do it, or no; or tap."
            };
            let ch = lines.len().min(4) as f32 * lh + bh + 40.0 * s;
            fx::panel(g, R::new(x, y, w, ch), s, WARN);
            for (i, l) in lines.iter().take(4).enumerate() {
                g.text(x + 14.0 * s, y + 10.0 * s + i as f32 * lh, size, TEXT, l, Face::Sans);
            }
            let ny = y + 12.0 * s + lines.len().min(4) as f32 * lh;
            let nl = g.fit(12.5 * s, note, Face::Sans, w - 28.0 * s);
            g.text(x + 14.0 * s, ny, 12.5 * s, MUTED, &nl, Face::Sans);
            let aw = g.button(x + 14.0 * s, ny + 20.0 * s, size, "Allow (Y)", OK, Event::Tab(T_MINE_ALLOW));
            g.button(x + 14.0 * s + aw + 10.0 * s, ny + 20.0 * s, size, "Deny (N)", ERR, Event::Tab(T_MINE_DENY));
            y += ch + gap;
        } else if let Some(c) = st.as_ref().and_then(|s| s.confirm.clone()) {
            let text = format!("Did you say: {c}?");
            let ch = bh + 44.0 * s;
            fx::panel(g, R::new(x, y, w, ch), s, WARN);
            let t = g.fit(size, &text, Face::Sans, w - 28.0 * s);
            g.text(x + 14.0 * s, y + 10.0 * s, size, TEXT, &t, Face::Sans);
            let aw = g.button(x + 14.0 * s, y + 14.0 * s + size * 1.4, size, "Yes (Y)", OK, Event::Tab(T_MINE_YES));
            g.button(x + 14.0 * s + aw + 10.0 * s, y + 14.0 * s + size * 1.4, size, "No (N)", ERR, Event::Tab(T_MINE_NO));
            y += ch + gap;
        }

        if let Some(ss) = &st {
            if !ss.heard.is_empty() {
                y += para(g, y, TEXT, size, &format!("You: {}", ss.heard), 3);
            }
            if !ss.said.is_empty() {
                y += para(g, y, ACCENT, size, &format!("IYAKKI: {}", ss.said), 5);
            }
            if !ss.note.is_empty() {
                y += para(g, y, WARN, 13.0 * s, &ss.note, 3);
            }
        }
        if !self.mine.note.is_empty() {
            y += para(g, y, DIM, 13.0 * s, &self.mine.note.clone(), 2);
        }
        y += gap;
        let mute = if v.my_muted { "Unmute (M)" } else { "Mute (M)" };
        let mw = g.button(x, y, size, mute, WARN, Event::Tab(T_MINE_MUTE));
        if cx.session.policy.manage_settings {
            g.button(x + mw + 10.0 * s, y, size, "Turn off", DIM, Event::Tab(T_MINE_OFF));
        }
        y += bh + gap;
        if v.my_wake {
            // the wake phrase is experimental and labelled so here too; the counts are the person's own marks and the spotter's own numbers
            let stats = cx.services.wake_stats_line();
            y += para(g, y, WARN, 13.0 * s, "Wake phrase (EXPERIMENTAL, off by default): it may miss you or wake by mistake. Tell it when it does.", 2);
            y += para(g, y, MUTED, 13.0 * s, &stats, 3);
            let fw = g.button(x, y, size, "That was a false wake", DIM, Event::Tab(T_MINE_FALSE));
            g.button(x + fw + 10.0 * s, y, size, "It missed me", DIM, Event::Tab(T_MINE_MISS));
            y += bh + gap;
        }
        para(g, y, DIM, 12.5 * s, "Space or Enter: talk, and again to end your request · S or Esc: stop · the follow-up window listens again after an answer for a few seconds. No sound is kept.", 3);
    }
}
