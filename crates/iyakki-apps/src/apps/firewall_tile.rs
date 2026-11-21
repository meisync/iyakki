//! FIREWALL (System group, F-FW-10): the tile itself lives in the `iyakki-tile-firewall` crate (it knows no firewall engine). This file is
//! the shell's side of it: the engine's view turned into the tile's plain facts (`facts_of`), the real port over the engine and the block log,
//! and the adapter that makes the tile an `App` of the shell. The tile only reads; a button opens Settings -> Firewall.

use iyakki_fw::blocked::BlockLog;
use iyakki_fw::engine::{Status, View};
use iyakki_fw::service::Handle;
use iyakki_tile_firewall as tile;
use std::sync::Arc;

use super::{App, AppCx, Flow};
use crate::gfx::Gfx;
use crate::ui::{Event, R};

pub use tile::{summary, FwFacts, Line, Tone, ACCENT};

fn status_of(s: &Status) -> tile::FwStatus {
    match s {
        Status::On => tile::FwStatus::On,
        Status::Pending { secs_left } => tile::FwStatus::Pending { secs_left: *secs_left },
        Status::Off => tile::FwStatus::Off,
        Status::OffThisBoot => tile::FwStatus::OffThisBoot,
        Status::NotProtected(w) => tile::FwStatus::NotProtected(w.clone()),
        Status::Unavailable(w) => tile::FwStatus::Unavailable(w.clone()),
    }
}

/// The engine's view and the number of connections blocked lately as the tile's plain facts.
pub fn facts_of(v: &View, blocked_recent: usize) -> FwFacts {
    let o = &v.config.outgoing;
    FwFacts {
        status: status_of(&v.status),
        open_rules: v.config.rules.iter().filter(|r| r.enabled).count(),
        outgoing: o.enabled.then(|| tile::FwOutgoing { block_by_default: o.block_by_default, rules: o.rules.len(), forwards: o.forwards.len(), port_forwards: o.port_forwards.len() }),
        last_error: v.last_error.clone(),
        warnings: v.warnings.clone(),
        blocked_recent,
    }
}

/// The real port: the engine handle, the block log and the shell's audit log (the person's name is the session's).
struct ShellPort<'a> {
    handle: &'a Handle,
    log: &'a BlockLog,
    cx: &'a AppCx,
}

impl tile::FirewallPort for ShellPort<'_> {
    fn facts(&self) -> FwFacts {
        facts_of(&self.handle.lock().view(), self.log.recent().len())
    }
    fn audit(&self, action: &str, subject: &str) {
        self.cx.audit.record(&self.cx.session.user, action, subject);
    }
}

pub struct FirewallApp {
    handle: Handle,
    log: Arc<BlockLog>,
    tile: tile::FirewallTile,
}

impl FirewallApp {
    pub fn new(cx: &AppCx) -> FirewallApp {
        FirewallApp::with(cx, Handle::Global, iyakki_fw::service::block_log())
    }

    pub fn with(cx: &AppCx, handle: Handle, log: Arc<BlockLog>) -> FirewallApp {
        let tile = tile::FirewallTile::new(&ShellPort { handle: &handle, log: &log, cx }, &cx.session.user);
        FirewallApp { handle, log, tile }
    }

    #[cfg(test)]
    pub(super) fn lines(&self) -> &[Line] {
        self.tile.lines()
    }
}

impl App for FirewallApp {
    fn title(&self) -> &'static str {
        self.tile.title()
    }
    fn page(&self) -> String {
        self.tile.page()
    }
    fn icon(&self) -> crate::gfx::Icon {
        self.tile.icon()
    }
    fn accent(&self) -> [u8; 4] {
        self.tile.accent()
    }
    fn hint(&self) -> String {
        self.tile.hint()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match self.tile.handle(ev, &ShellPort { handle: &self.handle, log: &self.log, cx }, &cx.session.user) {
            tile::Answer::Stay => Flow::Stay,
            tile::Answer::Back => Flow::Back,
        }
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.tile.take_request()
    }
    fn wants_redraw(&mut self) -> bool {
        self.tile.wants_redraw()
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &AppCx, _tick: u64) {
        self.tile.render(g, area, s);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;
    use crate::ui::Key;
    use iyakki_fw::backend::FakeBackend;
    use iyakki_fw::engine::{Clock, Engine, Resolver, Saved, Store};
    use iyakki_fw::outbound::OutConfig;
    use std::sync::Mutex;

    struct C;
    impl Clock for C {
        fn now(&self) -> i64 {
            5_000_000
        }
    }
    struct D;
    impl Resolver for D {
        fn resolve(&self, _: &str) -> Result<Vec<std::net::IpAddr>, String> {
            Err("no".into())
        }
    }
    #[derive(Default)]
    struct S(Mutex<Option<Saved>>);
    impl Store for S {
        fn load(&self) -> Result<Option<Saved>, String> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn save(&self, s: &Saved) -> Result<(), String> {
            *self.0.lock().unwrap() = Some(s.clone());
            Ok(())
        }
    }

    fn handle() -> Handle {
        let mut e = Engine::new(Box::new(FakeBackend::default()), Box::new(C), Box::new(D), Box::new(S::default()));
        e.boot(false);
        Handle::Own(Arc::new(Mutex::new(e)))
    }

    fn view_with(f: impl FnOnce(&mut View)) -> View {
        let h = handle();
        let mut v = h.lock().view();
        f(&mut v);
        v
    }

    #[test]
    fn the_words_follow_the_state_and_say_bad_news_plainly() {
        let on = summary(&facts_of(&view_with(|_| {}), 0));
        assert_eq!(on[0].tone, Tone::Good);
        assert!(on[0].text.starts_with("The firewall is ON"));
        assert!(on.iter().any(|l| l.text == "Outgoing: not filtered (everything may leave)."));
        let off = summary(&facts_of(&view_with(|v| v.status = Status::Off), 0));
        assert_eq!(off[0].tone, Tone::Bad);
        let np = summary(&facts_of(&view_with(|v| v.status = Status::NotProtected("no kernel table".into())), 0));
        assert!(np[0].text.contains("NOT PROTECTED") && np[0].text.contains("no kernel table"));
        let pend = summary(&facts_of(&view_with(|v| v.status = Status::Pending { secs_left: 42 }), 0));
        assert!(pend[0].text.contains("42 seconds"));
    }

    #[test]
    fn the_outgoing_part_and_the_blocked_count_are_counted_in_words() {
        let v = view_with(|v| v.config.outgoing = OutConfig { enabled: true, block_by_default: true, ..OutConfig::default() });
        let l = summary(&facts_of(&v, 1));
        assert!(l.iter().any(|x| x.text.contains("blocked unless a rule allows it") && x.text.contains("0 rules")));
        assert!(l.iter().any(|x| x.text == "1 connection blocked lately."));
        assert!(summary(&facts_of(&v, 3)).iter().any(|x| x.text == "3 connections blocked lately."));
    }

    #[test]
    fn the_tile_reads_only_and_asks_for_the_settings_page_on_s() {
        let cx = cx_with_admin_store("fwtile");
        let mut a = FirewallApp::with(&cx, handle(), BlockLog::new());
        assert!(!a.lines().is_empty());
        assert!(a.take_request().is_none());
        assert!(matches!(a.handle(&Event::Key(Key::Char('s')), &cx), Flow::Stay));
        assert_eq!(a.take_request(), Some(("settings".into(), "tab:firewall".into())));
        assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
    }
}
