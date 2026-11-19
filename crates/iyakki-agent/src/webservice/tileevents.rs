//! App events for WASM tiles (F-SDK-202): the same queue, `late` marker and scopes as for a native app, handed to a tile as `Event::App` of
//! tile ABI 3. A tile names the events it listens for in its manifest as `appevents:<name>` entries (`appevents:connectivity.changed`); when
//! its session starts the core is asked to register each (the core refuses what the tile's app does not hold the scope for, and the person's
//! switches apply), what waited is handed over first, then every step polls. The app name the core knows the tile by is `tile.<id>`.
//! A tile of ABI 1 or 2 is never sent one (`TileRunner::event` drops them), and nothing here carries a value a person could call secret:
//! payloads are the catalogue's names and numbers.

use std::sync::Arc;

use iyakki_core_api::appevents::{AppEvent, Val};
use iyakki_core_api::AppEvents;
use iyakki_tile_abi::{limits, Event};

/// The prefix of a manifest entry that asks for an app event.
pub const CAP_PREFIX: &str = "appevents:";

/// The most events handed to a tile in one step.
pub const MAX_PER_STEP: usize = 32;

/// The name the core knows a tile's app by.
pub fn app_id(tile: &str) -> String {
    format!("tile.{tile}")
}

/// The event names a manifest asks for, in order, without duplicates.
pub fn wanted(caps: &[String]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for c in caps {
        if let Some(n) = c.strip_prefix(CAP_PREFIX) {
            if !n.is_empty() && !v.iter().any(|x| x == n) {
                v.push(n.to_string());
            }
        }
    }
    v
}

/// One core event as the tile gets it. The payload is JSON text of at most the ABI's limit (a payload that does not fit says only that it was
/// too long, so the tile can read the state again itself).
pub fn to_tile(e: &AppEvent) -> Event {
    let mut o = serde_json::Map::new();
    for (k, v) in &e.payload {
        o.insert(k.clone(), match v {
            Val::Bool(b) => serde_json::Value::Bool(*b),
            Val::Int(n) => serde_json::Value::from(*n),
            Val::Text(t) => serde_json::Value::String(t.clone()),
        });
    }
    let mut payload = serde_json::Value::Object(o).to_string();
    if payload.len() > limits::MAX_APP_EVENT_PAYLOAD {
        payload = r#"{"too_long":true}"#.to_string();
    }
    let name: String = e.name.chars().take(limits::MAX_APP_EVENT_NAME).collect();
    Event::App { seq: e.seq, late: e.late, replayed: e.replayed, name, payload }
}

/// A tile's registration with the core for the length of its session. Dropping it tells the core the app stopped, so what happens from then on
/// waits and arrives `late`.
pub struct Feed {
    events: Arc<dyn AppEvents>,
    user: String,
    app: String,
    /// What the core refused at the start, as plain sentences (for the audit and the developer, never shown to the tile).
    pub refused: Vec<String>,
}

impl Feed {
    /// Register what the manifest asks for. `None` when it asks for nothing or nothing was accepted.
    pub fn begin(events: Arc<dyn AppEvents>, user: &str, tile: &str, caps: &[String]) -> Option<(Feed, Vec<Event>)> {
        let names = wanted(caps);
        if names.is_empty() {
            return None;
        }
        let app = app_id(tile);
        let mut refused = Vec::new();
        let mut any = false;
        for n in &names {
            match events.register(user, &app, n, "") {
                Ok(()) => any = true,
                Err(e) => refused.push(e),
            }
        }
        if !any {
            return None;
        }
        let first = events.start(user, &app).iter().take(MAX_PER_STEP).map(to_tile).collect();
        Some((Feed { events, user: user.to_string(), app, refused }, first))
    }

    /// What happened since the last look, oldest first.
    pub fn poll(&self) -> Vec<Event> {
        self.events.poll(&self.user, &self.app, MAX_PER_STEP).iter().map(to_tile).collect()
    }
}

impl Drop for Feed {
    fn drop(&mut self) {
        self.events.stop(&self.user, &self.app);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::appevents::{payload, FnScopes, MemStore, SharedHub};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn manifest_entries_name_events_once_and_a_core_event_becomes_a_tile_event_with_its_late_marker() {
        let caps: Vec<String> = ["home.list", "appevents:connectivity.changed", "appevents:connectivity.changed", "appevents:", "appevents:power.source"].iter().map(|s| s.to_string()).collect();
        assert_eq!(wanted(&caps), ["connectivity.changed", "power.source"]);
        assert_eq!(app_id("clock"), "tile.clock");
        let e = AppEvent { seq: 7, at_ms: 1, name: "connectivity.changed".into(), version: 1, late: true, replayed: false, payload: payload(&[("online", Val::Bool(false)), ("n", Val::Int(3)), ("who", Val::Text("x".into()))]) };
        let Event::App { seq, late, replayed, name, payload } = to_tile(&e) else { panic!("an app event") };
        assert_eq!((seq, late, replayed, name.as_str()), (7, true, false, "connectivity.changed"));
        let v: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!((v["online"].clone(), v["n"].clone(), v["who"].clone()), (serde_json::json!(false), serde_json::json!(3), serde_json::json!("x")));
        // an event that does not fit the ABI is reduced to a marker, never cut in the middle of the text
        let big = AppEvent { payload: payload_of_len(), ..e };
        let Event::App { payload, .. } = to_tile(&big) else { panic!() };
        assert_eq!(payload, r#"{"too_long":true}"#);
    }

    fn payload_of_len() -> iyakki_core_api::appevents::Payload {
        let mut p = iyakki_core_api::appevents::Payload::new();
        for i in 0..40 {
            p.insert(format!("k{i}"), Val::Text("x".repeat(40)));
        }
        p
    }

    fn hub(allowed: &'static [&'static str]) -> (Arc<SharedHub>, Arc<AtomicU64>) {
        let clock = Arc::new(AtomicU64::new(1_000));
        let c2 = clock.clone();
        let h = SharedHub::new(Box::new(MemStore::default()), Box::new(FnScopes(move |_u: &str, app: &str, scope: &str| app == "tile.clock" && allowed.contains(&scope))), Box::new(move || c2.load(Ordering::SeqCst)));
        (Arc::new(h), clock)
    }

    #[test]
    fn a_tile_hears_what_it_may_with_the_same_queue_late_marker_and_stop_as_a_native_app() {
        let (h, clock) = hub(&["events.network"]);
        let caps: Vec<String> = ["appevents:connectivity.changed", "appevents:power.source"].iter().map(|s| s.to_string()).collect();
        // a tile that asks for nothing is not registered; one whose every request is refused is not either
        assert!(Feed::begin(h.clone(), "alice", "clock", &["home.list".to_string()]).is_none());
        assert!(Feed::begin(h.clone(), "alice", "other", &caps).is_none(), "an app that holds no scope hears nothing");
        let (feed, first) = Feed::begin(h.clone(), "alice", "clock", &caps).expect("one request was accepted");
        assert!(first.is_empty());
        assert_eq!(feed.refused.len(), 1, "the power event needs a scope the tile does not hold: {:?}", feed.refused);
        h.emit("connectivity.changed", Some("alice"), payload(&[("state", Val::Text("offline".into()))])).unwrap();
        h.emit("power.source", Some("alice"), payload(&[("source", Val::Text("battery".into()))])).unwrap();
        let got = feed.poll();
        assert_eq!(got.len(), 1, "{got:?}");
        let Event::App { name, late, .. } = &got[0] else { panic!() };
        assert_eq!((name.as_str(), *late), ("connectivity.changed", false));
        assert!(feed.poll().is_empty(), "an event is handed out once");
        // the tile's session ends: what happens next waits, and arrives late with the next session
        drop(feed);
        clock.store(60_000, Ordering::SeqCst);
        h.emit("connectivity.changed", Some("alice"), payload(&[("state", Val::Text("online".into()))])).unwrap();
        let (_again, waiting) = Feed::begin(h, "alice", "clock", &caps).unwrap();
        assert_eq!(waiting.len(), 1, "{waiting:?}");
        let Event::App { late, .. } = &waiting[0] else { panic!() };
        assert!(*late, "it happened while the tile was away");
    }
}
