//! Web Push for web pages in the shell's browser (F-INBOX-09): `navigator.serviceWorker.register()` and `PushManager.subscribe()`.
//!
//! The script engine gives a secure page a service worker registration and a `PushManager` only when the browser hands it a host
//! (`iyakki_js::Sites::push`). This module hands every page of this browser the core's page-facing push: the core keeps the registrations,
//! asks the person on a card (once per site) and makes the subscription at its push service; the pushed messages that wait there are shown
//! in the INBOX with the site's name. A core that serves no page push (the default of the facet) leaves pages with the plain stub.

use super::AppCx;
use iyakki_agent_api::{PagePushHost, PagePushSub};
use iyakki_js::modules::PushSub;
use std::sync::{Arc, Mutex};

/// The core's host seen as the script engine's own trait (the engine does not know the agent API).
struct Host(Arc<Mutex<dyn PagePushHost>>);

fn sub(s: PagePushSub) -> PushSub {
    PushSub { endpoint: s.endpoint, expiration_ms: s.expiration_ms, key: s.key, p256dh: s.p256dh, auth: s.auth }
}

impl Host {
    fn with<T>(&self, f: impl FnOnce(&mut dyn PagePushHost) -> T) -> T {
        f(&mut *self.0.lock().unwrap_or_else(|e| e.into_inner()))
    }
}

impl iyakki_js::modules::PushHost for Host {
    fn permission(&mut self, origin: &str) -> String {
        self.with(|h| h.permission(origin))
    }
    fn register(&mut self, origin: &str, scope: &str, script: &str) -> Result<(), String> {
        self.with(|h| h.register(origin, scope, script))
    }
    fn unregister(&mut self, origin: &str, scope: &str) -> Result<bool, String> {
        self.with(|h| h.unregister(origin, scope))
    }
    fn subscribe(&mut self, origin: &str, scope: &str, key: &str) -> Result<PushSub, String> {
        self.with(|h| h.subscribe(origin, scope, key)).map(sub)
    }
    fn get(&mut self, origin: &str, scope: &str) -> Option<PushSub> {
        self.with(|h| h.get(origin, scope)).map(sub)
    }
    fn unsubscribe(&mut self, origin: &str, scope: &str) -> Result<bool, String> {
        self.with(|h| h.unsubscribe(origin, scope))
    }
}

/// The push host of a new browser window, when the core serves page push.
pub fn host_for(cx: &AppCx) -> Option<Arc<Mutex<dyn iyakki_js::modules::PushHost>>> {
    let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
    let host = cx.services.page_push(&cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d)))?;
    Some(Arc::new(Mutex::new(Host(host))))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake {
        calls: Vec<String>,
    }
    impl PagePushHost for Fake {
        fn permission(&mut self, origin: &str) -> String {
            self.calls.push(format!("permission {origin}"));
            "prompt".into()
        }
        fn register(&mut self, origin: &str, scope: &str, script: &str) -> Result<(), String> {
            self.calls.push(format!("register {origin} {scope} {script}"));
            Ok(())
        }
        fn unregister(&mut self, _: &str, _: &str) -> Result<bool, String> {
            Ok(true)
        }
        fn subscribe(&mut self, origin: &str, _: &str, key: &str) -> Result<PagePushSub, String> {
            Ok(PagePushSub { endpoint: format!("https://push.example.net/{}", origin.len()), expiration_ms: None, key: key.into(), p256dh: "P".into(), auth: "A".into() })
        }
        fn get(&mut self, _: &str, _: &str) -> Option<PagePushSub> {
            None
        }
        fn unsubscribe(&mut self, _: &str, _: &str) -> Result<bool, String> {
            Err("NotAllowedError: no".into())
        }
    }

    #[test]
    fn the_adapter_passes_the_pages_origin_and_the_cores_answers_through_unchanged() {
        use iyakki_js::modules::PushHost;
        let inner = Arc::new(Mutex::new(Fake { calls: vec![] }));
        let mut h = Host(inner.clone());
        assert_eq!(h.permission("https://a.example"), "prompt");
        h.register("https://a.example", "https://a.example/", "https://a.example/sw.js").unwrap();
        let s = h.subscribe("https://a.example", "https://a.example/", "KEY").unwrap();
        assert_eq!((s.endpoint.as_str(), s.key.as_str(), s.p256dh.as_str(), s.auth.as_str()), ("https://push.example.net/17", "KEY", "P", "A"));
        assert!(h.get("https://a.example", "https://a.example/").is_none());
        assert!(h.unregister("https://a.example", "https://a.example/").unwrap());
        assert_eq!(h.unsubscribe("https://a.example", "https://a.example/").unwrap_err(), "NotAllowedError: no");
        assert_eq!(inner.lock().unwrap().calls, vec!["permission https://a.example", "register https://a.example https://a.example/ https://a.example/sw.js"]);
    }

    #[test]
    fn the_browser_hands_the_cores_page_push_to_a_new_window_through_the_site_data() {
        let cx = crate::apps::tests::cx_with_admin_store("pagepush");
        // the in-process services of this test serve the real page push (a card and a vault)
        let sites = super::super::browser_passkeys::sites_for(&cx);
        assert!(sites.push.is_some());
    }
}
