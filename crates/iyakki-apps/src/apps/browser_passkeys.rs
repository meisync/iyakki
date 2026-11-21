//! Passkeys for web pages in the shell's browser (F-BROX-03, D-PASSKEY-PAGE default: asks the person with a card).
//!
//! The script engine gives a secure page `navigator.credentials.create()` and `.get()` only when the browser hands it a host
//! (`iyakki_js::Sites::passkeys`). This module hands every page of this browser the core's page-facing passkeys: the site rule is checked
//! first, then a card that names the page's origin, the site and the account is shown over the screen on EVERY use (answered on this
//! machine, never covered by a grant, not answerable by input sent from a paired computer), then the person's vault makes or uses the
//! passkey. A core that serves no page passkeys (the default of the facet) leaves pages without `navigator.credentials`, as before.
//! Uses are audited by the core without keys, ids or signatures.

use super::AppCx;
use iyakki_agent_api::PagePasskeyHost;
use std::sync::{Arc, Mutex};

/// The core's host seen as the script engine's own trait (the engine does not know the agent API).
struct Host(Arc<Mutex<dyn PagePasskeyHost>>);

impl iyakki_js::modules::PasskeyHost for Host {
    fn create(&mut self, origin: &str, request: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).create(origin, request)
    }
    fn get(&mut self, origin: &str, request: &serde_json::Value) -> Result<serde_json::Value, String> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).get(origin, request)
    }
}

/// The site data of a new browser window: every page of every site gets the person's page passkeys when the core serves them.
pub fn sites_for(cx: &AppCx) -> iyakki_js::Sites {
    let (audit, user) = (cx.audit.clone(), cx.session.user.clone());
    let host = cx.services.page_passkeys(&cx.session.user, Arc::new(move |a, d| audit.record(&user, a, d)));
    let mut sites = iyakki_js::Sites::default();
    sites.push = super::browser_push::host_for(cx);
    sites.passkeys = host.map(|h| Arc::new(Mutex::new(Host(h))) as Arc<Mutex<dyn iyakki_js::modules::PasskeyHost>>);
    sites
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::cx_with_admin_store;

    struct Nothing;
    impl iyakki_web::Fetch for Nothing {
        fn get(&self, _: &iyakki_web::Url) -> Result<iyakki_web::Response, String> {
            Err("no network in this test".into())
        }
    }

    fn typeof_credentials(sites: &iyakki_js::Sites, url: &str) -> String {
        let url = iyakki_web::Url::parse(url).unwrap();
        let data = sites.for_origin(&url);
        let mut p = iyakki_js::LivePage::load("<body><p id=o>x</p></body>", &url, Arc::new(Nothing), |_| true, &data, iyakki_js::Limits::default(), Vec::new()).expect("engine");
        p.eval("t", "typeof PublicKeyCredential").unwrap()
    }

    #[test]
    fn the_browser_hands_the_cores_page_passkeys_to_a_secure_page_and_an_http_page_gets_none() {
        let cx = cx_with_admin_store("pagepasskeys");
        let sites = sites_for(&cx);
        assert!(sites.passkeys.is_some(), "the core serves page passkeys, the browser hands them to every page");
        assert_eq!(typeof_credentials(&sites, "https://login.example.com/"), "function");
        assert_eq!(typeof_credentials(&sites, "http://login.example.com/"), "undefined", "an unprotected page has no PublicKeyCredential");
    }

    #[test]
    fn a_core_that_serves_none_leaves_pages_without_navigator_credentials() {
        let sites = iyakki_js::Sites::default();
        assert!(sites.passkeys.is_none());
        assert_eq!(typeof_credentials(&sites, "https://login.example.com/"), "undefined");
    }
}
