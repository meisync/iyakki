//! Events for apps, online and offline (docs/sdk/events.md; F-SDK-188).
//!
//! An app registers interest in named events through an [`EventsPort`] and handles them with an [`EventRouter`]. The core decides who
//! hears what: an app gets an event only if it registered for it and holds its scope, in order, with a sequence number that only grows
//! and a time. What happened while the app was not running waits in a bounded queue and arrives at the next start, before the first
//! frame, marked `late`; events that were dropped (the queue was full, too old, or too noisy) come as one `missed` marker. A payload
//! never carries a secret.
//!
//! A handler *acts* through [`EventActions`]: it can keep state in the app itself, leave a note in the inbox, start a flow or call a
//! chat tool; the host binds those to the core, so a tool that changes something asks the person as it always does.
//!
//! [`MemEvents`] is the in-memory port for tests and the browser preview. It follows the core's rules (scope, registration, order,
//! late, missed, coalescing, age, rate), with a clock the test moves.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

/// The name of the marker for dropped events.
pub const MISSED: &str = "missed";
/// The core's bounds, mirrored by [`MemEvents`].
pub const MAX_QUEUE: usize = 100;
pub const MAX_AGE_MS: u64 = 7 * 24 * 3600 * 1000;
pub const MAX_HISTORY: usize = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EvVal {
    Bool(bool),
    Int(i64),
    Text(String),
}

pub type EvPayload = BTreeMap<String, EvVal>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppEvent {
    pub seq: u64,
    pub at_ms: u64,
    pub name: String,
    pub version: u32,
    /// It happened while the app was not running.
    pub late: bool,
    pub replayed: bool,
    pub payload: EvPayload,
}

impl AppEvent {
    pub fn is_missed(&self) -> bool {
        self.name == MISSED
    }
    pub fn missed_count(&self) -> u64 {
        match self.payload.get("count") {
            Some(EvVal::Int(n)) => (*n).max(0) as u64,
            _ => 0,
        }
    }
    pub fn text(&self, field: &str) -> Option<&str> {
        match self.payload.get(field) {
            Some(EvVal::Text(t)) => Some(t),
            _ => None,
        }
    }
    pub fn int(&self, field: &str) -> Option<i64> {
        match self.payload.get(field) {
            Some(EvVal::Int(n)) => Some(*n),
            _ => None,
        }
    }
}

/// What changed in what an app may do (the event `app.permissions`): names only.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PermissionChange {
    /// Capabilities and scopes the app was given since it last heard.
    pub granted: Vec<String>,
    /// Capabilities and scopes taken away.
    pub revoked: Vec<String>,
    /// Events the app had registered for that ended with a scope it lost.
    pub dropped: Vec<String>,
    /// What the app holds now, and how many names that is (the list may be cut).
    pub effective: Vec<String>,
    pub held: usize,
    /// `person`, `administrator`, `policy`, `store` or `expiry`.
    pub by: String,
    /// The app must restart (or register again) to use what changed.
    pub restart: bool,
}

impl AppEvent {
    /// The change, when this is an `app.permissions` event.
    pub fn permissions(&self) -> Option<PermissionChange> {
        if self.name != "app.permissions" {
            return None;
        }
        let list = |f: &str| -> Vec<String> { self.text(f).unwrap_or("").split(',').filter(|x| !x.is_empty()).map(str::to_string).collect() };
        Some(PermissionChange { granted: list("granted"), revoked: list("revoked"), dropped: list("dropped"), effective: list("effective"), held: self.int("held").unwrap_or(0).max(0) as usize, by: self.text("by").unwrap_or("").to_string(), restart: matches!(self.payload.get("restart"), Some(EvVal::Bool(true))) })
    }
}

/// One registration as the person sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EventReg {
    pub event: String,
    pub filter: String,
    pub enabled: bool,
    pub scope: String,
    pub pending: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EventsError {
    /// The core's sentence (the scope the app lacks, an unknown event, a switch that is off).
    Refused(String),
}

impl EventsError {
    pub fn sentence(&self) -> &str {
        let EventsError::Refused(s) = self;
        s
    }
}

/// What an app is handed to hear events. The core's side is `iyakki_core_api::AppEvents`; the shell adapter binds this to it.
pub trait EventsPort {
    /// Register (again is fine). Refused with a sentence when the app lacks the scope or the event is off.
    fn register(&mut self, event: &str, filter: &str) -> Result<(), EventsError>;
    fn unregister(&mut self, event: &str, filter: &str) -> Result<(), EventsError>;
    fn registrations(&mut self) -> Vec<EventReg>;
    /// The app starts: the `missed` marker (if any), then everything that waited, `late` as recorded.
    fn start(&mut self) -> Vec<AppEvent>;
    /// The app goes to the back or closes: from now on events wait.
    fn stop(&mut self);
    /// What arrived since the last call.
    fn poll(&mut self, max: usize) -> Vec<AppEvent>;
    /// The delivered events after this number, again (after a crash before they were handled).
    fn replay(&mut self, after_seq: u64) -> Vec<AppEvent>;
    /// Publish `custom.<this app>.<name>` for other apps.
    fn publish(&mut self, name: &str, payload: EvPayload) -> Result<usize, EventsError>;
    /// The names of what this app may do now (capabilities and scopes): read it again after an `app.permissions` event instead of keeping a copy.
    fn capabilities(&mut self) -> Vec<String> {
        Vec::new()
    }
}

// ------------------------------------------------------------------------------------------------ acting

/// What a handler may do. The host binds each to the core; a tool call goes through the normal approval path.
pub trait EventActions {
    /// Leave a note in the inbox (needs the `notify` capability; the person sees it). The refusal is a sentence.
    fn notify(&mut self, text: &str) -> Result<(), String>;
    /// Start a flow by name (the core's flows; a flow that changes something asks as always).
    fn start_flow(&mut self, name: &str) -> Result<(), String>;
    /// Ask for a chat tool to run, with its arguments (JSON text). Nothing runs at once: the core puts the request in the person's waiting
    /// list and answers with a line saying so; the tool runs only if the person approves the card that names this app and the tool.
    fn call_tool(&mut self, tool: &str, args: &str) -> Result<String, String>;
}

/// Records what a handler did (tests and the browser preview).
#[derive(Default)]
pub struct RecordingActions {
    pub notes: Vec<String>,
    pub flows: Vec<String>,
    pub tools: Vec<(String, String)>,
}

impl EventActions for RecordingActions {
    fn notify(&mut self, text: &str) -> Result<(), String> {
        self.notes.push(text.into());
        Ok(())
    }
    fn start_flow(&mut self, name: &str) -> Result<(), String> {
        self.flows.push(name.into());
        Ok(())
    }
    fn call_tool(&mut self, tool: &str, args: &str) -> Result<String, String> {
        self.tools.push((tool.into(), args.into()));
        Ok("ok".into())
    }
}

type Handler = Box<dyn FnMut(&AppEvent, &mut dyn EventActions)>;

/// Registers an app's interests and hands each event to its handler, in order, never twice.
///
/// An app that registers handles `missed` ([`EventRouter::on_missed`]) and looks at `late` (a late event is news that may be old: do not
/// ring a bell for a call that ended an hour ago); `app-check` says so when it does not.
#[derive(Default)]
pub struct EventRouter {
    interests: Vec<(String, String)>,
    handlers: Vec<(String, Handler)>,
    on_missed: Option<Handler>,
    last_seq: u64,
    /// Events no handler took (a registration without a handler is a bug of the app).
    pub unhandled: Vec<String>,
}

impl EventRouter {
    pub fn new() -> EventRouter {
        EventRouter::default()
    }
    /// Interest in one event; the handler runs for every event of that name.
    pub fn on(mut self, event: &str, handler: impl FnMut(&AppEvent, &mut dyn EventActions) + 'static) -> EventRouter {
        self.interests.push((event.into(), String::new()));
        self.handlers.push((event.into(), Box::new(handler)));
        self
    }
    /// Like [`EventRouter::on`] for an event that takes a prefix (a settings key, a folder).
    pub fn on_prefix(mut self, event: &str, prefix: &str, handler: impl FnMut(&AppEvent, &mut dyn EventActions) + 'static) -> EventRouter {
        self.interests.push((event.into(), prefix.into()));
        self.handlers.push((event.into(), Box::new(handler)));
        self
    }
    /// What the app does when what it may do changed (a permission granted, taken away, or a registration dropped with its scope). The
    /// event always comes: no registration, no scope, and the person cannot switch it off. Read `port.capabilities()` again here.
    pub fn on_permissions(mut self, mut handler: impl FnMut(&PermissionChange, &AppEvent, &mut dyn EventActions) + 'static) -> EventRouter {
        self.handlers.push(("app.permissions".into(), Box::new(move |e, acts| {
            if let Some(c) = e.permissions() {
                handler(&c, e, acts);
            }
        })));
        self
    }
    /// What the app does when events were dropped: re-read the state it mirrors.
    pub fn on_missed(mut self, handler: impl FnMut(&AppEvent, &mut dyn EventActions) + 'static) -> EventRouter {
        self.on_missed = Some(Box::new(handler));
        self
    }
    /// Register every interest; the refusals come back as sentences (the app shows them).
    pub fn register_all(&self, port: &mut dyn EventsPort) -> Vec<String> {
        self.interests.iter().filter_map(|(e, f)| port.register(e, f).err().map(|er| format!("{e}: {}", er.sentence()))).collect()
    }
    /// Start the app's event stream: what waited is handled now, before the first frame.
    pub fn begin(&mut self, port: &mut dyn EventsPort, acts: &mut dyn EventActions) -> usize {
        let evs = port.start();
        self.dispatch(evs, acts)
    }
    /// Handle what arrived. Returns how many events were handled.
    pub fn pump(&mut self, port: &mut dyn EventsPort, acts: &mut dyn EventActions) -> usize {
        let evs = port.poll(MAX_QUEUE);
        self.dispatch(evs, acts)
    }
    /// After a crash: handle what was delivered but not handled (the numbers after the last one this router saw).
    pub fn recover(&mut self, port: &mut dyn EventsPort, acts: &mut dyn EventActions) -> usize {
        let evs = port.replay(self.last_seq);
        self.dispatch(evs, acts)
    }
    pub fn last_seq(&self) -> u64 {
        self.last_seq
    }
    /// The app handed in the last number it had saved (after a restart).
    pub fn resume_after(&mut self, seq: u64) {
        self.last_seq = self.last_seq.max(seq);
    }
    fn dispatch(&mut self, evs: Vec<AppEvent>, acts: &mut dyn EventActions) -> usize {
        let mut n = 0;
        for e in evs {
            if e.seq <= self.last_seq {
                continue;
            }
            self.last_seq = e.seq;
            n += 1;
            if e.is_missed() {
                if let Some(h) = self.on_missed.as_mut() {
                    h(&e, acts);
                } else {
                    self.unhandled.push(e.name.clone());
                }
                continue;
            }
            let mut took = false;
            for (name, h) in self.handlers.iter_mut().filter(|(n, _)| *n == e.name) {
                let _ = name;
                h(&e, acts);
                took = true;
            }
            if !took {
                self.unhandled.push(e.name.clone());
            }
        }
        n
    }
}

// ------------------------------------------------------------------------------------------------ the memory port

struct Queued {
    name: String,
    at_ms: u64,
    late: bool,
    payload: EvPayload,
}

/// An in-memory port with the core's rules. The test says which events the app holds the scope for, moves the clock and makes events
/// happen with [`MemEvents::happen`].
#[derive(Default)]
pub struct MemEvents {
    now: u64,
    scoped: BTreeSet<String>,
    coalesce: BTreeSet<String>,
    per_min: BTreeMap<String, u32>,
    regs: Vec<EventReg>,
    queue: VecDeque<Queued>,
    missed: u64,
    next_seq: u64,
    running: bool,
    history: VecDeque<AppEvent>,
    window: BTreeMap<String, (u64, u32)>,
    pub published: Vec<(String, EvPayload)>,
    can_publish: bool,
    held: Vec<String>,
}

impl MemEvents {
    pub fn new() -> MemEvents {
        MemEvents::default()
    }
    /// The app holds the scope for this event.
    pub fn allow(&mut self, event: &str) {
        self.scoped.insert(event.into());
    }
    pub fn allow_publish(&mut self) {
        self.can_publish = true;
    }
    /// Only the latest of this event waits.
    pub fn coalescing(&mut self, event: &str) {
        self.coalesce.insert(event.into());
    }
    pub fn rate_limit(&mut self, event: &str, per_min: u32) {
        self.per_min.insert(event.into(), per_min);
    }
    pub fn set_time(&mut self, ms: u64) {
        self.now = ms;
    }
    /// The app holds these names now (what `capabilities` answers).
    pub fn hold(&mut self, names: &[&str]) {
        self.held = names.iter().map(|n| n.to_string()).collect();
    }
    /// What the core does when something changes what the app may do: the event is queued (no registration is needed), later changes are
    /// added up, and an event the app is no longer allowed to hear is dropped with its registration.
    pub fn permission_change(&mut self, granted: &[&str], revoked: &[&str], by: &str, restart: bool) {
        let mut dropped = Vec::new();
        for r in revoked {
            if self.scoped.remove(*r) {
                // the test names an event as its own scope: the registrations for it end with it
            }
        }
        let scoped = self.scoped.clone();
        self.regs.retain(|r| {
            let keep = scoped.contains(&r.event);
            if !keep {
                dropped.push(r.event.clone());
            }
            keep
        });
        for g in granted {
            self.scoped.insert((*g).to_string());
        }
        let mut p = EvPayload::new();
        let join = |v: &[&str]| v.join(",");
        p.insert("granted".into(), EvVal::Text(join(granted)));
        p.insert("revoked".into(), EvVal::Text(join(revoked)));
        p.insert("dropped".into(), EvVal::Text(dropped.join(",")));
        p.insert("effective".into(), EvVal::Text(self.held.join(",")));
        p.insert("held".into(), EvVal::Int(self.held.len() as i64));
        p.insert("by".into(), EvVal::Text(by.into()));
        p.insert("restart".into(), EvVal::Bool(restart));
        self.queue.retain(|q| q.name != "app.permissions");
        self.queue.push_back(Queued { name: "app.permissions".into(), at_ms: self.now, late: !self.running, payload: p });
    }
    pub fn pending(&self) -> usize {
        self.queue.len()
    }
    /// Something happened: queued if the app registered, holds the scope and the person has not switched it off.
    pub fn happen(&mut self, event: &str, payload: EvPayload) {
        if !self.regs.iter().any(|r| r.event == event && r.enabled) || !self.scoped.contains(event) {
            return;
        }
        if let Some(&lim) = self.per_min.get(event) {
            let w = self.window.entry(event.into()).or_insert((self.now, 0));
            if self.now.saturating_sub(w.0) >= 60_000 {
                *w = (self.now, 0);
            }
            if w.1 >= lim {
                self.missed += 1;
                return;
            }
            w.1 += 1;
        }
        if self.coalesce.contains(event) {
            self.queue.retain(|q| q.name != event);
        }
        self.queue.push_back(Queued { name: event.into(), at_ms: self.now, late: !self.running, payload });
        if self.queue.len() > MAX_QUEUE {
            self.queue.pop_front();
            self.missed += 1;
        }
    }
    /// The person's switch in Settings.
    pub fn switch(&mut self, event: &str, on: bool) {
        for r in self.regs.iter_mut().filter(|r| r.event == event) {
            r.enabled = on;
        }
        if !on {
            self.queue.retain(|q| q.name != event);
        }
    }
    fn drain(&mut self, max: usize) -> Vec<AppEvent> {
        while self.queue.front().is_some_and(|q| self.now.saturating_sub(q.at_ms) > MAX_AGE_MS) {
            self.queue.pop_front();
            self.missed += 1;
        }
        let mut out = Vec::new();
        if self.missed > 0 && max > 0 {
            self.next_seq += 1;
            let mut p = EvPayload::new();
            p.insert("count".into(), EvVal::Int(std::mem::take(&mut self.missed) as i64));
            out.push(AppEvent { seq: self.next_seq, at_ms: self.now, name: MISSED.into(), version: 1, late: true, replayed: false, payload: p });
        }
        while out.len() < max {
            let Some(q) = self.queue.pop_front() else { break };
            if q.name != "app.permissions" && !self.regs.iter().any(|r| r.event == q.name && r.enabled) {
                continue;
            }
            self.next_seq += 1;
            out.push(AppEvent { seq: self.next_seq, at_ms: q.at_ms, name: q.name, version: 1, late: q.late, replayed: false, payload: q.payload });
        }
        self.history.extend(out.iter().cloned());
        while self.history.len() > MAX_HISTORY {
            self.history.pop_front();
        }
        out
    }
}

impl EventsPort for MemEvents {
    fn register(&mut self, event: &str, filter: &str) -> Result<(), EventsError> {
        if !self.scoped.contains(event) {
            return Err(EventsError::Refused(format!("this app does not hold the permission to hear {event}: ask the person")));
        }
        if !self.regs.iter().any(|r| r.event == event && r.filter == filter) {
            self.regs.push(EventReg { event: event.into(), filter: filter.into(), enabled: true, scope: String::new(), pending: 0 });
        }
        Ok(())
    }
    fn unregister(&mut self, event: &str, filter: &str) -> Result<(), EventsError> {
        let n = self.regs.len();
        self.regs.retain(|r| !(r.event == event && r.filter == filter));
        if n == self.regs.len() {
            return Err(EventsError::Refused("this app has not registered for that event".into()));
        }
        self.queue.retain(|q| q.name != event);
        Ok(())
    }
    fn registrations(&mut self) -> Vec<EventReg> {
        let q = &self.queue;
        self.regs.iter().cloned().map(|mut r| {
            r.pending = q.iter().filter(|x| x.name == r.event).count();
            r
        }).collect()
    }
    fn start(&mut self) -> Vec<AppEvent> {
        self.running = true;
        self.drain(MAX_QUEUE + 1)
    }
    fn stop(&mut self) {
        self.running = false;
    }
    fn poll(&mut self, max: usize) -> Vec<AppEvent> {
        self.drain(max)
    }
    fn replay(&mut self, after_seq: u64) -> Vec<AppEvent> {
        self.history.iter().filter(|e| e.seq > after_seq).cloned().map(|mut e| {
            e.replayed = true;
            e
        }).collect()
    }
    fn capabilities(&mut self) -> Vec<String> {
        self.held.clone()
    }
    fn publish(&mut self, name: &str, payload: EvPayload) -> Result<usize, EventsError> {
        if !self.can_publish {
            return Err(EventsError::Refused("this app does not hold events.publish".into()));
        }
        if !name.starts_with("custom.") {
            return Err(EventsError::Refused("a custom event is called custom.<your app>.<name>".into()));
        }
        self.published.push((name.into(), payload));
        Ok(0)
    }
}

pub fn ev_text(pairs: &[(&str, &str)]) -> EvPayload {
    pairs.iter().map(|(k, v)| (k.to_string(), EvVal::Text(v.to_string()))).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    fn mem() -> MemEvents {
        let mut m = MemEvents::new();
        m.allow("connectivity.changed");
        m.allow("peers.changed");
        m.coalescing("connectivity.changed");
        m
    }

    #[test]
    fn registration_needs_the_scope_and_the_refusal_is_a_sentence() {
        let mut m = MemEvents::new();
        let e = m.register("power.source", "").unwrap_err();
        assert!(e.sentence().contains("power.source"));
        m.allow("power.source");
        m.register("power.source", "").unwrap();
        m.register("power.source", "").unwrap();
        assert_eq!(m.registrations().len(), 1);
    }

    #[test]
    fn handlers_get_events_in_order_and_can_act() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        let mut r = EventRouter::new()
            .on("peers.changed", move |e, acts| {
                s.borrow_mut().push((e.seq, e.late));
                if e.text("change") == Some("paired") {
                    let _ = acts.notify("a computer was paired");
                    let _ = acts.call_tool("fleet_status", "{}");
                }
            })
            .on_missed(|_, _| {});
        let mut m = mem();
        assert!(r.register_all(&mut m).is_empty());
        let mut acts = RecordingActions::default();
        assert_eq!(r.begin(&mut m, &mut acts), 0);
        m.happen("peers.changed", ev_text(&[("device", "x"), ("change", "online")]));
        m.happen("peers.changed", ev_text(&[("device", "x"), ("change", "paired")]));
        assert_eq!(r.pump(&mut m, &mut acts), 2);
        assert_eq!(*seen.borrow(), [(1, false), (2, false)]);
        assert_eq!(acts.notes, ["a computer was paired"]);
        assert_eq!(acts.tools[0].0, "fleet_status");
        assert_eq!(r.pump(&mut m, &mut acts), 0);
    }

    #[test]
    fn what_waited_is_handled_at_the_start_marked_late_and_missed_comes_first() {
        let order = Rc::new(RefCell::new(Vec::new()));
        let (a, b) = (order.clone(), order.clone());
        let mut r = EventRouter::new().on("peers.changed", move |e, _| a.borrow_mut().push(format!("{}{}", e.seq, if e.late { "L" } else { "" }))).on_missed(move |e, _| b.borrow_mut().push(format!("missed{}", e.missed_count())));
        let mut m = mem();
        m.set_time(1);
        r.register_all(&mut m);
        for i in 0..(MAX_QUEUE + 3) {
            m.set_time(1_000 + i as u64);
            m.happen("peers.changed", ev_text(&[("device", "d"), ("change", "online")]));
        }
        let mut acts = RecordingActions::default();
        r.begin(&mut m, &mut acts);
        let o = order.borrow();
        assert_eq!(o[0], "missed3");
        assert_eq!(o.len(), 1 + MAX_QUEUE);
        assert!(o[1].ends_with('L'));
    }

    #[test]
    fn state_events_coalesce_and_a_rate_limit_counts_what_it_drops() {
        let mut m = mem();
        m.register("connectivity.changed", "").unwrap();
        m.register("peers.changed", "").unwrap();
        m.rate_limit("peers.changed", 2);
        for s in ["online", "offline", "online"] {
            m.happen("connectivity.changed", ev_text(&[("state", s)]));
        }
        for _ in 0..5 {
            m.happen("peers.changed", ev_text(&[("device", "d"), ("change", "online")]));
        }
        let got = m.start();
        assert_eq!(got.len(), 1 + 1 + 2);
        assert!(got[0].is_missed() && got[0].missed_count() == 3);
        assert!(got.windows(2).all(|w| w[0].seq < w[1].seq));
    }

    #[test]
    fn an_event_is_never_handled_twice_and_a_crash_can_recover() {
        let n = Rc::new(RefCell::new(0));
        let c = n.clone();
        let mut r = EventRouter::new().on("peers.changed", move |_, _| *c.borrow_mut() += 1).on_missed(|_, _| {});
        let mut m = mem();
        r.register_all(&mut m);
        let mut acts = RecordingActions::default();
        r.begin(&mut m, &mut acts);
        m.happen("peers.changed", ev_text(&[("device", "d"), ("change", "online")]));
        m.happen("peers.changed", ev_text(&[("device", "e"), ("change", "online")]));
        // the app crashed after the core delivered both, having saved only seq 0
        let delivered = m.poll(9);
        assert_eq!(delivered.len(), 2);
        assert_eq!(r.recover(&mut m, &mut acts), 2);
        assert_eq!(*n.borrow(), 2);
        assert_eq!(r.recover(&mut m, &mut acts), 0, "not twice");
    }

    #[test]
    fn the_person_can_switch_an_event_off() {
        let mut m = mem();
        m.register("peers.changed", "").unwrap();
        m.switch("peers.changed", false);
        m.happen("peers.changed", ev_text(&[("device", "d"), ("change", "online")]));
        assert_eq!(m.pending(), 0);
        assert!(!m.registrations()[0].enabled);
    }

    #[test]
    fn an_event_nobody_handles_is_reported() {
        let mut r = EventRouter::new();
        let mut m = mem();
        m.register("peers.changed", "").unwrap();
        m.start();
        m.happen("peers.changed", ev_text(&[("device", "d"), ("change", "online")]));
        let mut acts = RecordingActions::default();
        r.pump(&mut m, &mut acts);
        assert_eq!(r.unhandled, ["peers.changed"]);
    }

    #[test]
    fn a_permission_change_reaches_the_handler_without_a_registration_and_a_lost_scope_drops_the_registration() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        let mut r = EventRouter::new()
            .on("connectivity.changed", |_, _| {})
            .on_permissions(move |c, e, _| s.borrow_mut().push((c.clone(), e.late)))
            .on_missed(|_, _| {});
        let mut m = mem();
        m.hold(&["connectivity.changed", "peers.changed"]);
        assert!(r.register_all(&mut m).is_empty(), "the permission event needs no registration");
        assert_eq!(m.registrations().len(), 1);
        let mut acts = RecordingActions::default();
        r.begin(&mut m, &mut acts);
        m.permission_change(&[], &["connectivity.changed"], "person", false);
        m.hold(&["peers.changed"]);
        assert_eq!(r.pump(&mut m, &mut acts), 1);
        let (c, late) = seen.borrow()[0].clone();
        assert_eq!((c.revoked, c.dropped, c.by, late), (vec!["connectivity.changed".to_string()], vec!["connectivity.changed".to_string()], "person".to_string(), false));
        assert!(m.registrations().is_empty(), "the registration ended with its scope");
        assert_eq!(m.capabilities(), ["peers.changed"], "the app reads what it holds now");
    }

    #[test]
    fn a_change_while_the_app_was_closed_is_late_and_several_are_one() {
        let seen = Rc::new(RefCell::new(Vec::new()));
        let s = seen.clone();
        let mut r = EventRouter::new().on_permissions(move |c, e, _| s.borrow_mut().push((c.granted.clone(), c.restart, e.late)));
        let mut m = MemEvents::new();
        m.permission_change(&["camera.view"], &[], "store", true);
        m.permission_change(&["files.read"], &[], "administrator", false);
        let mut acts = RecordingActions::default();
        assert_eq!(r.begin(&mut m, &mut acts), 1);
        assert_eq!(seen.borrow().len(), 1);
        assert!(seen.borrow()[0].2);
    }

    #[test]
    fn publishing_needs_the_scope_and_a_custom_name() {
        let mut m = MemEvents::new();
        assert!(m.publish("custom.a.b", EvPayload::new()).is_err());
        m.allow_publish();
        assert!(m.publish("b", EvPayload::new()).is_err());
        m.publish("custom.a.b", EvPayload::new()).unwrap();
    }
}
