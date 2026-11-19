//! The SDK's `EventsPort` over the core's app events (`cx.core.app_events()`), for one person and one app (F-SDK-188). The port only maps
//! types; the core decides everything (the catalogue, the scope the app holds, the file policy, the queue, the late marker, the audit
//! lines), so a refusal arrives here as a sentence. [`CoreEventActions`] is what a handler may do: today the core has no app-facing
//! inbox, flow or tool call, so each answers with a sentence saying so (the SDK trait and its recording fake are built; binding them to the
//! core's notify, flows and tool approval path is a separate row).

use std::sync::Arc;

use iyakki_core_api::appevents as api;
use iyakki_core_api::Core;
use iyakki_ui_sdk::events as sdk;

pub struct CoreEventsPort {
    core: Arc<dyn Core>,
    user: String,
    app: String,
}

impl CoreEventsPort {
    pub fn new(core: Arc<dyn Core>, user: &str, app: &str) -> CoreEventsPort {
        CoreEventsPort { core, user: user.into(), app: app.into() }
    }
}

fn val(v: api::Val) -> sdk::EvVal {
    match v {
        api::Val::Bool(b) => sdk::EvVal::Bool(b),
        api::Val::Int(n) => sdk::EvVal::Int(n),
        api::Val::Text(t) => sdk::EvVal::Text(t),
    }
}

fn val_back(v: sdk::EvVal) -> api::Val {
    match v {
        sdk::EvVal::Bool(b) => api::Val::Bool(b),
        sdk::EvVal::Int(n) => api::Val::Int(n),
        sdk::EvVal::Text(t) => api::Val::Text(t),
    }
}

fn event(e: api::AppEvent) -> sdk::AppEvent {
    sdk::AppEvent { seq: e.seq, at_ms: e.at_ms, name: e.name, version: e.version, late: e.late, replayed: e.replayed, payload: e.payload.into_iter().map(|(k, v)| (k, val(v))).collect() }
}

fn refused(e: String) -> sdk::EventsError {
    sdk::EventsError::Refused(e)
}

impl sdk::EventsPort for CoreEventsPort {
    fn register(&mut self, ev: &str, filter: &str) -> Result<(), sdk::EventsError> {
        self.core.app_events().register(&self.user, &self.app, ev, filter).map_err(refused)
    }
    fn unregister(&mut self, ev: &str, filter: &str) -> Result<(), sdk::EventsError> {
        self.core.app_events().unregister(&self.user, &self.app, ev, filter).map_err(refused)
    }
    fn registrations(&mut self) -> Vec<sdk::EventReg> {
        self.core.app_events().list(&self.user, &self.app).into_iter().map(|r| sdk::EventReg { event: r.event, filter: r.filter, enabled: r.enabled, scope: r.scope, pending: r.pending }).collect()
    }
    fn start(&mut self) -> Vec<sdk::AppEvent> {
        self.core.app_events().start(&self.user, &self.app).into_iter().map(event).collect()
    }
    fn stop(&mut self) {
        self.core.app_events().stop(&self.user, &self.app)
    }
    fn poll(&mut self, max: usize) -> Vec<sdk::AppEvent> {
        self.core.app_events().poll(&self.user, &self.app, max).into_iter().map(event).collect()
    }
    fn replay(&mut self, after_seq: u64) -> Vec<sdk::AppEvent> {
        self.core.app_events().replay(&self.user, &self.app, after_seq).into_iter().map(event).collect()
    }
    fn capabilities(&mut self) -> Vec<String> {
        self.core.app_events().permissions(&self.user, &self.app).0
    }
    fn wake_at(&mut self, at_ms: u64, tag: &str) -> Result<(), sdk::EventsError> {
        self.core.app_events().wake_at(&self.user, &self.app, at_ms, tag).map_err(refused)
    }
    fn wake_cancel(&mut self, tag: &str) -> bool {
        self.core.app_events().wake_cancel(&self.user, &self.app, tag)
    }
    fn wakes(&mut self) -> Vec<(u64, String)> {
        self.core.app_events().wakes(&self.user, &self.app)
    }
    fn publish(&mut self, name: &str, payload: sdk::EvPayload) -> Result<usize, sdk::EventsError> {
        let p: api::Payload = payload.into_iter().map(|(k, v)| (k, val_back(v))).collect();
        self.core.app_events().publish(&self.user, &self.app, name, p).map_err(refused)
    }
}

/// What a handler may do on this machine, for one person and one app: each call goes to the core's `app_actions` (the capability check,
/// the inbox, the person's flows, the waiting list for a tool), which answers a refusal in a sentence.
pub struct CoreEventActions {
    core: Arc<dyn Core>,
    user: String,
    app: String,
}

impl CoreEventActions {
    pub fn new(core: Arc<dyn Core>, user: &str, app: &str) -> CoreEventActions {
        CoreEventActions { core, user: user.into(), app: app.into() }
    }
}

impl sdk::EventActions for CoreEventActions {
    fn notify(&mut self, text: &str) -> Result<(), String> {
        self.core.app_actions().notify(&self.user, &self.app, text)
    }
    fn start_flow(&mut self, name: &str) -> Result<(), String> {
        self.core.app_actions().start_flow(&self.user, &self.app, name).map(|_| ())
    }
    fn call_tool(&mut self, tool: &str, args: &str) -> Result<String, String> {
        let id = self.core.app_actions().request_tool(&self.user, &self.app, tool, args)?;
        Ok(format!("waiting for the person to answer ({id})"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::appevents::{payload, AppEvents, Val};
    use iyakki_core_fake::FakeCore;
    use iyakki_ui_sdk::events::EventsPort;

    #[test]
    fn the_port_maps_the_cores_rules_to_the_app_scope_late_missed_replay_and_publish() {
        let core = Arc::new(FakeCore::new());
        let f = core.app_events_fake();
        let mut p = CoreEventsPort::new(core.clone(), "pat", "sample");
        let e = p.register("connectivity.changed", "").unwrap_err();
        assert!(e.sentence().contains("events.network"), "{}", e.sentence());
        f.grant("sample", "events.network");
        f.grant("sample", "events.publish");
        f.grant("other", "events.custom");
        p.register("connectivity.changed", "").unwrap();
        assert_eq!(p.registrations().len(), 1);
        assert!(p.start().is_empty());
        p.stop();
        f.emit("connectivity.changed", Some("pat"), payload(&[("state", Val::Text("offline".into()))])).unwrap();
        let got = p.start();
        assert_eq!(got.len(), 1);
        assert!(got[0].late);
        assert_eq!(got[0].text("state"), Some("offline"));
        assert_eq!(p.replay(0)[0].seq, got[0].seq);
        // another person's event does not reach this one
        f.emit("connectivity.changed", Some("sam"), payload(&[("state", Val::Text("online".into()))])).unwrap();
        assert!(p.poll(9).is_empty());
        assert_eq!(p.publish("custom.sample.hello", Default::default()).unwrap(), 0);
        assert!(p.publish("custom.other.hello", Default::default()).is_err());
        p.unregister("connectivity.changed", "").unwrap();
    }

    #[test]
    fn the_actions_go_to_the_core_which_checks_the_capability_and_keeps_tool_requests_for_the_person() {
        use iyakki_ui_sdk::events::EventActions;
        let core = Arc::new(FakeCore::new());
        let f = core.app_actions_fake();
        let mut a = CoreEventActions::new(core.clone(), "pat", "sample");
        assert!(a.notify("hi").unwrap_err().contains("notify"));
        f.grant("sample", "notify");
        f.grant("sample", "flows.start");
        f.grant("sample", "tools.request");
        f.add_flow("Backup");
        a.notify("The network went away").unwrap();
        a.start_flow("Backup").unwrap();
        assert!(a.start_flow("Nope").is_err());
        let said = a.call_tool("wifi_status", "{}").unwrap();
        assert!(said.contains("waiting for the person"), "{said}");
        use iyakki_core_api::AppActions;
        assert_eq!(f.notes()[0].2, "The network went away");
        assert_eq!(f.started()[0].2, "Backup");
        assert_eq!(f.pending("pat")[0].tool, "wifi_status", "nothing ran: it waits");
    }
}
