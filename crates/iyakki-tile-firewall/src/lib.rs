//! FIREWALL (System group, F-FW-10): the state of the firewall at a glance: on or off, what is allowed in, the outgoing part (default,
//! rules, forwarding, port forwards) and the connections blocked lately. The tile only reads; a button opens Settings -> Firewall,
//! where the rules change (with the 60-second Keep/Undo). Every word comes from `summary`, which is pure and tested with made-up facts.
//!
//! The tile knows no firewall engine: the shell reads the engine and hands over [`FwFacts`] through [`FirewallPort`].

use iyakki_uikit::gfx::{rgb, Face, Gfx, Icon};
use iyakki_uikit::ui::{Event, Key, ERR, MUTED, OK, R, TEXT};

pub const ACCENT: [u8; 4] = rgb(222, 64, 100);
pub const T_SETTINGS: usize = 1;
pub const T_REFRESH: usize = 2;

/// What the engine says about itself, in plain words of its own.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FwStatus {
    On,
    Pending { secs_left: i64 },
    Off,
    OffThisBoot,
    NotProtected(String),
    Unavailable(String),
}

impl FwStatus {
    /// `ON`, `OFF` or `NOT PROTECTED` (what the audit line records).
    pub fn headline(&self) -> &'static str {
        match self {
            FwStatus::Off | FwStatus::OffThisBoot => "OFF",
            FwStatus::On | FwStatus::Pending { .. } => "ON",
            FwStatus::NotProtected(_) | FwStatus::Unavailable(_) => "NOT PROTECTED",
        }
    }
}

/// The outgoing part of the configuration, when it is switched on.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FwOutgoing {
    pub block_by_default: bool,
    pub rules: usize,
    pub forwards: usize,
    pub port_forwards: usize,
}

/// Everything the screen says, as numbers and short texts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FwFacts {
    pub status: FwStatus,
    /// Enabled incoming rules.
    pub open_rules: usize,
    /// `None` when outgoing connections are not filtered.
    pub outgoing: Option<FwOutgoing>,
    pub last_error: Option<String>,
    pub warnings: Vec<String>,
    pub blocked_recent: usize,
}

impl FwFacts {
    /// A firewall that is on with nothing configured.
    pub fn plain_on() -> FwFacts {
        FwFacts { status: FwStatus::On, open_rules: 0, outgoing: None, last_error: None, warnings: Vec::new(), blocked_recent: 0 }
    }
}

/// What the tile needs from the shell: the facts, and a place to record what the person did.
pub trait FirewallPort {
    fn facts(&self) -> FwFacts;
    fn audit(&self, action: &str, subject: &str);
}

/// One line of the screen and whether it is good news, bad news or plain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub tone: Tone,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Bad,
    Plain,
}

fn plural(n: usize, one: &str, many: &str) -> String {
    if n == 1 { format!("1 {one}") } else { format!("{n} {many}") }
}

/// The lines for the facts.
pub fn summary(f: &FwFacts) -> Vec<Line> {
    let mut out = Vec::new();
    let mut push = |text: String, tone: Tone| out.push(Line { text, tone });
    match &f.status {
        FwStatus::On => push("The firewall is ON. New incoming connections are blocked unless a rule allows them.".into(), Tone::Good),
        FwStatus::Pending { secs_left } => push(format!("A change is waiting for you: keep it within {secs_left} seconds or it is undone."), Tone::Plain),
        FwStatus::Off => push("The firewall is OFF. Every other computer on the network can reach this one.".into(), Tone::Bad),
        FwStatus::OffThisBoot => push("The firewall is OFF for this start only (boot setting).".into(), Tone::Bad),
        FwStatus::NotProtected(why) => push(format!("NOT PROTECTED: the rules are wanted but not in force ({why})."), Tone::Bad),
        FwStatus::Unavailable(why) => push(format!("NOT PROTECTED: this kernel cannot do it ({why})."), Tone::Bad),
    }
    push(format!("Incoming: {} allowed.", plural(f.open_rules, "rule", "rules")), Tone::Plain);
    match &f.outgoing {
        Some(o) => {
            let how = if o.block_by_default { "blocked unless a rule allows it" } else { "let out unless a rule blocks it" };
            push(format!("Outgoing: connections are {how}; {}, {}, {}.", plural(o.rules, "rule", "rules"), plural(o.forwards, "forwarding rule", "forwarding rules"), plural(o.port_forwards, "port forward", "port forwards")), Tone::Plain);
        }
        None => push("Outgoing: not filtered (everything may leave).".into(), Tone::Plain),
    }
    if let Some(e) = &f.last_error {
        push(format!("Last problem: {e}"), Tone::Bad);
    }
    for w in f.warnings.iter().take(3) {
        push(w.clone(), Tone::Bad);
    }
    if f.blocked_recent > 0 {
        push(format!("{} blocked lately.", plural(f.blocked_recent, "connection", "connections")), Tone::Plain);
    }
    out
}

pub struct FirewallTile {
    lines: Vec<Line>,
    request: Option<(String, String)>,
    dirty: bool,
}

/// What a key or click asked the shell to do with the tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Stay,
    Back,
}

impl FirewallTile {
    /// Reads once (and records the read) so the first picture has its words.
    pub fn new(port: &dyn FirewallPort, user: &str) -> FirewallTile {
        let mut a = FirewallTile { lines: Vec::new(), request: None, dirty: true };
        a.refresh(port, user);
        a
    }

    pub fn refresh(&mut self, port: &dyn FirewallPort, user: &str) {
        let f = port.facts();
        self.lines = summary(&f);
        let _ = user;
        port.audit("firewall.tile.read", f.status.headline());
        self.dirty = true;
    }

    pub fn lines(&self) -> &[Line] {
        &self.lines
    }

    pub fn title(&self) -> &'static str {
        "Firewall"
    }
    pub fn page(&self) -> String {
        "status".into()
    }
    pub fn icon(&self) -> Icon {
        Icon::Lock
    }
    pub fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    pub fn hint(&self) -> String {
        "s open the rules in Settings · r read again · Esc back".into()
    }

    pub fn handle(&mut self, ev: &Event, port: &dyn FirewallPort, user: &str) -> Answer {
        match ev {
            Event::Key(Key::Esc) => return Answer::Back,
            Event::Tab(T_SETTINGS) | Event::Key(Key::Char('s')) | Event::Key(Key::Enter) => {
                port.audit("firewall.tile.open_settings", "");
                self.request = Some(("settings".into(), "tab:firewall".into()));
            }
            Event::Tab(T_REFRESH) | Event::Key(Key::Char('r')) => self.refresh(port, user),
            _ => {}
        }
        self.dirty = true;
        Answer::Stay
    }
    /// A request for the shell to open another place: `(what, detail)`, once.
    pub fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }
    pub fn wants_redraw(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
    pub fn render(&mut self, g: &mut Gfx, area: R, s: f32) {
        let pad = 20.0 * s;
        let x = area.x + pad;
        let w = (area.w - 2.0 * pad).max(40.0 * s);
        let mut y = area.y + 12.0 * s;
        let size = 14.0 * s;
        let bh = size * 2.1;
        let mut bx = x;
        for (label, ev, c) in [("Open the rules", Event::Tab(T_SETTINGS), ACCENT), ("Read again", Event::Tab(T_REFRESH), TEXT)] {
            let bw = g.text_width(size, label, Face::Bold) + size * 1.7;
            if bx > x && bx + bw > x + w {
                bx = x;
                y += bh + 6.0 * s;
            }
            bx += g.button(bx, y, size, label, c, ev) + 8.0 * s;
        }
        y += bh + 14.0 * s;
        for l in &self.lines {
            let color = match l.tone {
                Tone::Good => OK,
                Tone::Bad => ERR,
                Tone::Plain => MUTED,
            };
            for part in g.wrap(15.0 * s, &l.text, Face::Sans, w) {
                if y + 20.0 * s > area.y + area.h {
                    return;
                }
                g.text(x, y, 15.0 * s, color, &part, Face::Sans);
                y += 21.0 * s;
            }
            y += 6.0 * s;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        facts: RefCell<FwFacts>,
        log: RefCell<Vec<(String, String)>>,
    }
    impl Fake {
        fn new(f: FwFacts) -> Fake {
            Fake { facts: RefCell::new(f), log: RefCell::new(Vec::new()) }
        }
    }
    impl FirewallPort for Fake {
        fn facts(&self) -> FwFacts {
            self.facts.borrow().clone()
        }
        fn audit(&self, action: &str, subject: &str) {
            self.log.borrow_mut().push((action.into(), subject.into()));
        }
    }

    fn facts_with(f: impl FnOnce(&mut FwFacts)) -> FwFacts {
        let mut v = FwFacts::plain_on();
        f(&mut v);
        v
    }

    #[test]
    fn the_words_follow_the_state_and_say_bad_news_plainly() {
        let on = summary(&facts_with(|_| {}));
        assert_eq!(on[0].tone, Tone::Good);
        assert!(on[0].text.starts_with("The firewall is ON"));
        assert!(on.iter().any(|l| l.text == "Outgoing: not filtered (everything may leave)."));
        let off = summary(&facts_with(|v| v.status = FwStatus::Off));
        assert_eq!(off[0].tone, Tone::Bad);
        let boot = summary(&facts_with(|v| v.status = FwStatus::OffThisBoot));
        assert!(boot[0].text.contains("for this start only"));
        let np = summary(&facts_with(|v| v.status = FwStatus::NotProtected("no kernel table".into())));
        assert!(np[0].text.contains("NOT PROTECTED") && np[0].text.contains("no kernel table"));
        let un = summary(&facts_with(|v| v.status = FwStatus::Unavailable("old kernel".into())));
        assert!(un[0].text.contains("this kernel cannot do it") && un[0].tone == Tone::Bad);
        let pend = summary(&facts_with(|v| v.status = FwStatus::Pending { secs_left: 42 }));
        assert!(pend[0].text.contains("42 seconds"));
    }

    #[test]
    fn the_outgoing_part_and_the_blocked_count_are_counted_in_words() {
        let v = facts_with(|v| v.outgoing = Some(FwOutgoing { block_by_default: true, ..FwOutgoing::default() }));
        let mut one = v.clone();
        one.blocked_recent = 1;
        let l = summary(&one);
        assert!(l.iter().any(|x| x.text.contains("blocked unless a rule allows it") && x.text.contains("0 rules")));
        assert!(l.iter().any(|x| x.text == "1 connection blocked lately."));
        let mut three = v;
        three.blocked_recent = 3;
        assert!(summary(&three).iter().any(|x| x.text == "3 connections blocked lately."));
        let open = facts_with(|v| v.outgoing = Some(FwOutgoing { block_by_default: false, rules: 1, forwards: 2, port_forwards: 1 }));
        assert!(summary(&open).iter().any(|x| x.text.contains("let out unless a rule blocks it; 1 rule, 2 forwarding rules, 1 port forward")));
    }

    #[test]
    fn problems_are_listed_and_warnings_stop_at_three() {
        let f = facts_with(|v| {
            v.last_error = Some("table busy".into());
            v.warnings = (1..=5).map(|i| format!("warn {i}")).collect();
            v.open_rules = 1;
        });
        let l = summary(&f);
        assert!(l.iter().any(|x| x.text == "Last problem: table busy" && x.tone == Tone::Bad));
        assert!(l.iter().any(|x| x.text == "Incoming: 1 rule allowed."));
        assert_eq!(l.iter().filter(|x| x.text.starts_with("warn ")).count(), 3);
    }

    #[test]
    fn the_headline_is_what_the_audit_records() {
        assert_eq!(FwStatus::On.headline(), "ON");
        assert_eq!(FwStatus::Pending { secs_left: 1 }.headline(), "ON");
        assert_eq!(FwStatus::OffThisBoot.headline(), "OFF");
        assert_eq!(FwStatus::Unavailable("x".into()).headline(), "NOT PROTECTED");
    }

    #[test]
    fn the_tile_reads_only_and_asks_for_the_settings_page_on_s() {
        let p = Fake::new(FwFacts::plain_on());
        let mut a = FirewallTile::new(&p, "amy");
        assert!(!a.lines().is_empty());
        assert!(a.take_request().is_none());
        assert_eq!(a.handle(&Event::Key(Key::Char('s')), &p, "amy"), Answer::Stay);
        assert_eq!(a.take_request(), Some(("settings".into(), "tab:firewall".into())));
        assert!(a.take_request().is_none());
        assert_eq!(a.handle(&Event::Key(Key::Esc), &p, "amy"), Answer::Back);
        let log = p.log.borrow();
        assert_eq!(log[0], ("firewall.tile.read".to_string(), "ON".to_string()));
        assert_eq!(log[1], ("firewall.tile.open_settings".to_string(), String::new()));
    }

    #[test]
    fn read_again_picks_up_a_new_state_and_asks_for_a_redraw() {
        let p = Fake::new(FwFacts::plain_on());
        let mut a = FirewallTile::new(&p, "amy");
        assert!(a.wants_redraw());
        assert!(!a.wants_redraw());
        p.facts.borrow_mut().status = FwStatus::Off;
        a.handle(&Event::Key(Key::Char('r')), &p, "amy");
        assert!(a.lines()[0].text.contains("OFF"));
        assert!(a.wants_redraw());
        assert_eq!(p.log.borrow().last().unwrap().1, "OFF");
        a.handle(&Event::Tab(T_REFRESH), &p, "amy");
        assert_eq!(p.log.borrow().iter().filter(|(a, _)| a == "firewall.tile.read").count(), 3);
    }

    #[test]
    fn it_draws_its_two_buttons_and_the_words_at_every_standard_size() {
        let p = Fake::new(facts_with(|v| v.status = FwStatus::Off));
        let mut a = FirewallTile::new(&p, "amy");
        for (w, h) in [(1280u32, 720u32), (1024, 600), (800, 480), (480, 600)] {
            let mut g = Gfx::new(w, h);
            let s = w as f32 / 1280.0;
            a.render(&mut g, R { x: 0.0, y: 72.0, w: w as f32, h: h as f32 - 72.0 }, s.max(0.6));
            let seen = g.text_log.iter().map(|t| t.2.clone()).collect::<Vec<_>>().join("|");
            assert!(seen.contains("Open the rules") && seen.contains("Read again"), "{w}x{h}: {seen}");
        }
    }
}
