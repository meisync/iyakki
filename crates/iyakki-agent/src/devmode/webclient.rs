//! A small client for a program of a paired computer (F-WEBSVC-06): it signs in to the web service with the computer's developer key
//! (no password) and then calls the routes its `api:*` scopes allow. The certificate of the service is pinned by its fingerprint.
//!
//! ```ignore
//! let home = Home::at(&client::default_home());
//! let api = ApiClient::new(&home, "192.168.1.20", 8443, "AB:CD:...")?;
//! let who = api.sign_in()?;                         // {"token", "user", "computer", "scopes": [...]}
//! let (status, flows) = api.call("GET", "/api/v1/flows", &[], None)?;
//! ```
//! The fingerprint is the one the service logs when it starts (and `iyakki serve` prints); the key is the one `iyakki dev pair` made.

use std::sync::Mutex;
use std::time::Duration;

use ed25519_dalek::SigningKey;
use serde_json::Value;

use super::auth::sign_headers;
use super::client::{https, target, Home};

pub struct ApiClient {
    host: String,
    port: u16,
    fingerprint: String,
    key: SigningKey,
    token: Mutex<Option<String>>,
}

impl ApiClient {
    /// A client for the service at `host:port` whose certificate has this fingerprint, using the key of `home`.
    pub fn new(home: &Home, host: &str, port: u16, fingerprint: &str) -> Result<ApiClient, String> {
        Ok(ApiClient::with_key(host, port, fingerprint, home.key()?))
    }

    pub fn with_key(host: &str, port: u16, fingerprint: &str, key: SigningKey) -> ApiClient {
        ApiClient { host: host.into(), port, fingerprint: fingerprint.into(), key, token: Mutex::new(None) }
    }

    /// Sign in with the developer key. On success the token is kept for [`ApiClient::call`]; the answer says who, which computer and
    /// which scopes. A computer that holds no `api:*` scope is refused (an administrator grants them on the device).
    pub fn sign_in(&self) -> Result<Value, String> {
        let headers = sign_headers(&self.key, "POST", "/api/v1/login", &[], &[], &self.fingerprint, crate::clock::now_unix())?;
        let r = https(&self.host, self.port, Some(&self.fingerprint), "POST", "/api/v1/login", &headers, &[], Duration::from_secs(15))?;
        let v = r.json();
        if r.status != 200 {
            return Err(v["error"]["message"].as_str().map(str::to_string).unwrap_or_else(|| format!("the service answered {}", r.status)));
        }
        let token = v["token"].as_str().ok_or("the service sent no token")?.to_string();
        *self.token.lock().map_err(|_| "busy".to_string())? = Some(token);
        Ok(v)
    }

    /// One call with the token (sign in first). `Err` only when the service cannot be reached or there is no token; a refusal such as
    /// 403 (the scopes do not allow the route) comes back as the status and the body.
    pub fn call(&self, method: &str, path: &str, query: &[(String, String)], body: Option<&Value>) -> Result<(u16, Value), String> {
        let token = self.token.lock().map_err(|_| "busy".to_string())?.clone().ok_or("sign in first")?;
        let mut headers = vec![("Authorization".to_string(), format!("Bearer {token}"))];
        let bytes = body.map(|b| b.to_string().into_bytes()).unwrap_or_default();
        if !bytes.is_empty() {
            headers.push(("Content-Type".into(), "application/json".into()));
        }
        let r = https(&self.host, self.port, Some(&self.fingerprint), method, &target(path, query), &headers, &bytes, Duration::from_secs(30))?;
        Ok((r.status, r.json()))
    }

    /// End the token at the service.
    pub fn sign_out(&self) -> Result<(), String> {
        let _ = self.call("POST", "/api/v1/logout", &[], None)?;
        *self.token.lock().map_err(|_| "busy".to_string())? = None;
        Ok(())
    }
}
