//! The permission layer of the shell (docs/development/CORE-ACCESS-MODEL.md 6c; F-ARCH-05/06, F-CORE-12/13): the shell holds the one raw
//! machine handle and the broker, and hands every app a [`GuardedOs`] for that app's identity, never the raw one.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use iyakki_core_api::broker::{Broker, Caller, CallerKind};
use iyakki_core_api::guarded::{GestureCard, GuardedOs};
use iyakki_core_api::Os;

/// How long a ticket (the press handed to a job thread) stays good.
const TICKET_WINDOW: Duration = Duration::from_secs(1);

pub(super) struct OsGuard {
    raw: Arc<dyn Os>,
    broker: Arc<Broker>,
    gesture: Arc<GestureCard>,
}

impl OsGuard {
    /// The machine itself, the approvals and the access log kept in `data`.
    pub(super) fn new(data: &Path) -> OsGuard {
        OsGuard::over(Arc::new(iyakki_core_local::LocalOs::system()), data)
    }

    pub(super) fn over(raw: Arc<dyn Os>, data: &Path) -> OsGuard {
        let gesture = Arc::new(GestureCard::new(TICKET_WINDOW));
        let (broker, _log) = iyakki_agent::accessbroker::local(data, Some(gesture.clone()));
        OsGuard { raw, broker, gesture }
    }

    pub(super) fn card(&self) -> Arc<GestureCard> {
        self.gesture.clone()
    }

    /// The `Os` one app gets: `app` is its id (`PApp::id()`), `person` the signed-in user.
    pub(super) fn os_for(&self, app: &str, person: &str) -> Arc<dyn Os> {
        let caller = Caller { app: app.into(), kind: CallerKind::Builtin, person: person.into(), device: "this".into() };
        Arc::new(GuardedOs::new(self.raw.clone(), self.broker.clone(), caller))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_fake::FakeOs;

    fn guard(tag: &str) -> (OsGuard, Arc<FakeOs>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("iyakki-osguard-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let fake = Arc::new(FakeOs::new());
        (OsGuard::over(fake.clone(), &dir), fake, dir)
    }

    #[test]
    fn an_app_gets_what_the_table_gives_it_and_the_decisions_land_in_the_access_log_file() {
        let (g, fake, dir) = guard("table");
        let settings = g.os_for("settings", "admin");
        let other = g.os_for("calc", "admin");
        assert!(settings.audio().set_volume(0, 55).is_ok());
        assert!(other.audio().set_volume(0, 99).is_err(), "an app the table does not name cannot change the volume");
        assert_eq!(fake.audio().volume(0).unwrap().percent, 55);
        assert_eq!(other.sysinfo().cpu_count().unwrap(), 4, "reading is free");
        let all = std::fs::read_to_string(dir.join("access").join("log.jsonl")).unwrap();
        assert!(all.contains("system.volume.set") && all.contains("not_granted") && all.contains("\"calc\""), "{all}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_memory_change_needs_the_person_to_have_pressed_something_first() {
        let (g, fake, dir) = guard("press");
        let m = g.os_for("settings", "admin").memory();
        assert!(m.set_conf_key("zram", Some("on")).unwrap_err().contains("no_card"));
        assert!(fake.memory_fake().calls().is_empty());
        g.card().dispatch(|| m.set_conf_key("zram", Some("on")).unwrap());
        assert_eq!(fake.memory_fake().calls().len(), 1);
        assert!(m.set_conf_key("zram", Some("off")).unwrap_err().contains("no_card"), "the press does not outlive its dispatch");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
