//! The web service inside a server that already exists: the desktop's browser server (`iyakki serve --web`, port 8443) hands every
//! `/api/v1/...` request to [`Embedded::handle`], and the same pipeline answers it as on the headless listener (route table, origin,
//! sign-in, role, rate limit, handler, audit). No second port, no second certificate.
//!
//! Sign-in has two doors that end in the same place. The page can sign in itself (`POST /api/v1/login` with the machine's account,
//! a bearer token kept for the tab), or it can ride the desktop sign-in that already happened on this server (password, or a device
//! key through `POST /api/login-key`): the server tells [`Embedded::handle`] who the session cookie belongs to and a token for that
//! person is made once per session and used for the request. Nothing is added to a request that already carries its own
//! `Authorization`, and a request a browser says is cross-site gets nothing added. The desktop cookie is `SameSite=Strict` and
//! `HttpOnly`; the token never reaches the page on that path.
//!
//! The profile is `agent-api` (a desktop has hardware; the cloud profile's file and tenant routes belong to a pod).

use super::config::ServiceConfig;
use super::deps::Deps;
use super::server::{self, Shared};
use super::Profile;
use crate::httpmini::{Request, Response};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};

/// A desktop session as the browser server knows it: the cookie's value and the account it belongs to.
#[derive(Clone, Copy, Debug)]
pub struct DesktopSession<'a> {
    pub cookie: &'a str,
    pub user: &'a str,
}

pub struct Embedded {
    shared: Shared,
    /// SHA-256 of a desktop session cookie -> the API token made for it (the cookie itself is never stored).
    bridge: Mutex<HashMap<String, String>>,
}

/// Is this path the API's (so a miss is an API 404 and not the page's)?
pub fn owns(path: &str) -> bool {
    path == "/api/v1" || path.starts_with("/api/v1/")
}

/// Is this path a webhook's (`/hooks/<id>/<token>`)? Those need no sign-in; the hook's own address and signature are the proof.
pub fn owns_hook(path: &str) -> bool {
    super::hooks::is_hook_path(path)
}

impl Embedded {
    pub fn new(deps: Arc<Deps>) -> Result<Embedded, String> {
        Ok(Embedded { shared: Shared::embedded(Profile::AgentApi, ServiceConfig::default(), deps)?, bridge: Mutex::new(HashMap::new()) })
    }

    /// The token for a desktop session, made on first use. `None` when the account is gone or too many people are signed in.
    fn token_for(&self, s: &DesktopSession) -> Option<String> {
        let key = super::auth::hash_token(s.cookie);
        let mut map = self.bridge.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = map.get(&key) {
            if self.shared.tokens.find(t).as_deref() == Some(s.user) {
                return Some(t.clone());
            }
        }
        if map.len() >= super::auth::MAX_TOKENS {
            // forget the ones whose token has ended
            map.retain(|_, t| self.shared.tokens.find(t).is_some());
        }
        let t = self.shared.tokens.issue(s.user).ok()?;
        map.insert(key, t.clone());
        Some(t)
    }

    /// The desktop session ended (signed out, or idle): its token ends with it.
    pub fn forget(&self, cookie: &str) {
        let key = super::auth::hash_token(cookie);
        if let Some(t) = self.bridge.lock().unwrap_or_else(|e| e.into_inner()).remove(&key) {
            self.shared.tokens.revoke(&t);
        }
    }

    /// Answer one `/api/v1/` request. `session` is the desktop sign-in of this browser, if it has one.
    pub fn handle(&self, req: &Request, peer: IpAddr, session: Option<DesktopSession>) -> Response {
        let cross_site = req.header("sec-fetch-site").is_some_and(|v| v != "same-origin" && v != "none");
        let wants_own = req.header("authorization").is_some() || req.path == "/api/v1/login";
        if let (Some(s), false, false) = (session, cross_site, wants_own) {
            if let Some(t) = self.token_for(&s) {
                let mut r = req.clone();
                r.headers.insert("authorization".into(), format!("Bearer {t}"));
                return server::handle(&self.shared, &r, peer);
            }
        }
        server::handle(&self.shared, req, peer)
    }

    /// Signed-in tokens right now (tests, admin counts).
    pub fn tokens(&self) -> usize {
        self.shared.tokens.count()
    }
}
