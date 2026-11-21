//! USAGE: the tile itself lives in the `iyakki-tile-usage` crate (it does not depend on the agent). This file is the shell's side of it:
//! the real usage log behind the tile's `UsageSource`, the conversion of the agent's chart data, and the adapter that makes the
//! tile crate's `App` an `App` of the shell. See docs/development/COMPILE-SPLIT-PILOT.md ("Second step").

use iyakki_tile_api::usage as api;
use iyakki_tile_api::{App as TileApp, TileCx};

use super::{App, AppCx, Flow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub use iyakki_tile_usage::fmt_tokens;

/// The real usage log, in the machine's time zone (the agent core serves it).
pub struct ShellUsage(std::sync::Arc<dyn api::UsageSource>);

impl ShellUsage {
    pub fn of(cx: &AppCx) -> ShellUsage {
        // a core that does not serve the usage log to this program (the UI over the socket) shows no numbers
        ShellUsage(cx.b4().usage_source().unwrap_or_else(|| std::sync::Arc::new(api::NoUsage)))
    }
}

impl api::UsageSource for ShellUsage {
    fn now_unix(&self) -> i64 {
        self.0.now_unix()
    }
    fn series(&self, range: api::Range, now_unix: i64, user: Option<&str>) -> api::Series {
        self.0.series(range, now_unix, user)
    }
}

/// The tile crate's app, as an app of the shell.
pub struct UsageApp(iyakki_tile_usage::UsageApp);

impl UsageApp {
    pub fn new(_cx: &AppCx) -> UsageApp {
        UsageApp(iyakki_tile_usage::UsageApp::new())
    }

    pub fn range(&self) -> api::Range {
        self.0.range()
    }

    fn with<T>(cx: &AppCx, f: impl FnOnce(&TileCx) -> T) -> T {
        let usage = ShellUsage::of(cx);
        f(&TileCx { user: cx.session.user.as_str(), manage_settings: cx.session.policy.manage_settings, usage: &usage, calendar: &iyakki_tile_api::calendar::NoCalendar, audit: &iyakki_tile_api::NoAudit, remote: &iyakki_tile_api::remote::NoRemote, cameras: &iyakki_tile_api::cameras::NoCameras, radio: &iyakki_tile_api::radio::NoRadio })
    }
}

impl App for UsageApp {
    fn title(&self) -> &'static str {
        self.0.title()
    }
    fn icon(&self) -> Icon {
        self.0.icon()
    }
    fn accent(&self) -> [u8; 4] {
        self.0.accent()
    }
    fn hint(&self) -> String {
        self.0.hint()
    }
    fn help(&self) -> Vec<String> {
        self.0.help()
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match Self::with(cx, |t| self.0.handle(ev, t)) {
            iyakki_tile_api::Flow::Stay => Flow::Stay,
            iyakki_tile_api::Flow::Back => Flow::Back,
        }
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        Self::with(cx, |t| self.0.render(g, area, s, t, tick));
    }
}

#[cfg(test)]
mod tests {
    use iyakki_agent::usage as agent;
    use iyakki_clock::Zone;
    use super::*;
    use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir, Offline};
    use iyakki_shared::chat::{ChatBackend, Message};
    use std::sync::Arc;

    fn admin_cx(tag: &str) -> AppCx {
        let mut cx = cx_with_admin_store(tag);
        let d = temp_dir(&format!("{tag}-svc"));
        cx.services = services_in(&d, Arc::new(Offline));
        cx.services.on_login("root", "rootpass12", true);
        cx
    }

    fn fill(cx: &AppCx) {
        let now = iyakki_clock::now_unix();
        let u = &cx.usage();
        for (ago_h, user, model, i, o) in [(1, "root", "qwen3:8b", 1200u64, 800u64), (3, "root", "qwen3:8b", 5000, 2500), (30, "root", "llama3.2", 900, 400), (24 * 5, "root", "big-model", 40_000, 20_000), (24 * 40, "root", "big-model", 90_000, 30_000), (2, "alice", "qwen3:8b", 100_000, 50_000)] {
            u.record_at(now - ago_h * 3600, user, "p", model, i, o);
        }
    }

    /// The shell's adapter: the tile crate, driven through the shell's `App` trait, shows the real log's numbers for the person, or for
    /// everyone for an administrator; the converted chart data equals the agent's.
    #[test]
    fn the_shell_adapter_feeds_the_real_usage_log_to_the_tile_crate() {
        let cx = admin_cx("usage1");
        fill(&cx);
        let mut a = UsageApp::new(&cx);
        let mut g = Gfx::new(1200, 700);
        let area = R::new(0.0, 46.0, 1200.0, 654.0);
        a.render(&mut g, area, 1.0, &cx, 0);
        assert_eq!(a.range(), api::Range::Week);
        assert!(matches!(a.handle(&Event::Key(crate::ui::Key::Char('1')), &cx), Flow::Stay));
        assert_eq!(a.range(), api::Range::Day);
        assert!(matches!(a.handle(&Event::Key(crate::ui::Key::Esc), &cx), Flow::Back));
        assert_eq!((a.title(), a.accent()), ("Usage", iyakki_tile_usage::UsageApp::new().accent()));
        let src = ShellUsage::of(&cx);
        let now = iyakki_clock::now_unix();
        use api::UsageSource;
        assert_eq!(src.series(api::Range::Year, now, Some("root")).calls, 5);
        assert_eq!(src.series(api::Range::Day, now, Some("root")).calls, 2);
        let all = src.series(api::Range::Day, now, None);
        assert_eq!(all.calls, 3);
        let direct = cx.usage().series(agent::Range::Day, now, &Zone::system(), None);
        assert_eq!((all.models.clone(), all.totals.clone(), all.buckets.len()), (direct.models.clone(), direct.totals.clone(), direct.buckets.len()));
        assert_eq!(all.buckets.iter().map(|b| b.by_model.clone()).collect::<Vec<_>>(), direct.buckets.iter().map(|b| b.by_model.clone()).collect::<Vec<_>>());
    }

    #[test]
    fn what_goes_through_the_chat_backend_is_counted_for_the_person_and_the_model() {
        use iyakki_shared::settings::ModelChoice;
        let cx = admin_cx("usage3");
        {
            let mut st = cx.settings().lock().unwrap();
            let mut p = iyakki_shared::providers::ProviderCfg::new("local", iyakki_shared::providers::Kind::Ollama);
            p.models = vec!["gemma3".into()];
            st.upsert_provider(p).unwrap();
            st.set_chat(Some(ModelChoice { provider: "local".into(), model: "gemma3".into() })).unwrap();
        }
        struct Echo;
        impl ChatBackend for Echo {
            fn complete(&self, _: &[Message]) -> Result<String, String> {
                Ok("x".repeat(400))
            }
        }
        let m = iyakki_agent::usage::Metered::new(Arc::new(Echo), iyakki_agent::services::Services::from_handle(&cx.services.clone()), "root");
        assert_eq!(m.complete(&[Message::user("y".repeat(800))]).unwrap().len(), 400);
        let e = cx.usage().entries(Some("root"));
        assert_eq!(e.len(), 1, "{e:?}");
        assert_eq!((e[0].model.as_str(), e[0].provider.as_str(), e[0].calls), ("gemma3", "local", 1));
        assert!(e[0].output == 100 && e[0].input >= 200, "{e:?}");
        // a failed call costs nothing
        struct Fail;
        impl ChatBackend for Fail {
            fn complete(&self, _: &[Message]) -> Result<String, String> {
                Err("down".into())
            }
        }
        let f = iyakki_agent::usage::Metered::new(Arc::new(Fail), iyakki_agent::services::Services::from_handle(&cx.services.clone()), "root");
        assert!(f.complete(&[Message::user("hi")]).is_err());
        assert_eq!(cx.usage().entries(None).len(), 1);
    }
}
