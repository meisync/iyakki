//! The native tile connector's client (F-CLOUD-76, stage 2 of docs/development/CLOUD-MULTITENANT-PLAN.md): the local shell asks a tenant's
//! cloud web service which WASM tiles it runs (`GET /api/v1/tiles`) and drives one (`POST /api/v1/tiles/{id}/events`), getting back the
//! tile's display list to draw with the LOCAL look instead of a scaled picture of the tenant's screen.
//!
//! Same trust as the pixel-stream connector: HTTPS to the pinned certificate (a different certificate ends before any byte of the
//! sign-in goes out), the tenant's own accounts, one fresh connection per request, the bearer token kept in a zeroizing string and never
//! put in an error or a log. The tile runs on the tenant's side; this client only moves events and display lists.
//!
//! NOT here: the shell side (a tenant's tiles as tiles of the strip, input routing, the tick timer) and a run against a real tenant pod.

use serde_json::{json, Value};
use std::time::Duration;
use zeroize::Zeroizing;

use crate::cloudclient::{error_text, http, tls_open, Target};

const IO_TIMEOUT: Duration = Duration::from_secs(15);
/// The service takes at most this many events in one request.
pub const MAX_EVENTS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteTile {
    pub id: String,
    pub title: String,
    pub abi: u32,
    pub width: u32,
    pub height: u32,
    /// The tile wants a clock tick.
    pub ticks: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct FrameRequest {
    pub width: u32,
    pub height: u32,
    pub scale_pct: u32,
    pub dark: bool,
    /// Start the tile again.
    pub reset: bool,
    pub events: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RemoteFrame {
    /// The tile drew something new (otherwise `frame` is the one already shown).
    pub redraw: bool,
    pub frame: Value,
    pub stats: Value,
}

pub struct TileConnector {
    target: Target,
    token: Zeroizing<String>,
}

fn plain_id(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

impl TileConnector {
    /// Sign in with the tenant's account. `Err` says why (wrong password, certificate not the pinned one, no route).
    pub fn sign_in(t: &Target, user: &str, password: &str) -> Result<TileConnector, String> {
        let (mut tls, _) = tls_open(&t.addr, Some(t.pin), IO_TIMEOUT).map_err(|e| e.text())?;
        let body = json!({"name": user.trim(), "password": password}).to_string();
        let (st, _, b) = http(&mut tls, &t.addr, "POST", "/api/v1/login", &[("Content-Type", "application/json")], body.as_bytes()).map_err(|e| e.text())?;
        match st {
            200 => {
                let token = serde_json::from_slice::<Value>(&b).ok().and_then(|v| v["token"].as_str().map(str::to_string)).filter(|t| !t.is_empty()).ok_or("the tenant gave no session")?;
                Ok(TileConnector { target: t.clone(), token: Zeroizing::new(token) })
            }
            401 => Err(format!("the tenant refused the sign-in: {}", error_text(&b, st))),
            s => Err(format!("the tenant answered {s} to the sign-in: {}", error_text(&b, s))),
        }
    }

    fn call(&self, method: &str, path: &str, body: &[u8]) -> Result<Value, String> {
        let (mut tls, _) = tls_open(&self.target.addr, Some(self.target.pin), IO_TIMEOUT).map_err(|e| e.text())?;
        let auth = format!("Bearer {}", self.token.as_str());
        let (st, _, b) = http(&mut tls, &self.target.addr, method, path, &[("Authorization", &auth), ("Content-Type", "application/json")], body).map_err(|e| e.text())?;
        match st {
            200 => serde_json::from_slice(&b).map_err(|_| "the tenant's answer is not readable".to_string()),
            401 => Err("the tenant signed you out: sign in again".into()),
            404 => Err("the tenant has no such tile (or does not offer tiles)".into()),
            429 => Err("the tenant says slow down: too many tile events".into()),
            s => Err(format!("the tenant answered {s}: {}", error_text(&b, s))),
        }
    }

    /// The tiles the tenant offers this person.
    pub fn tiles(&self) -> Result<Vec<RemoteTile>, String> {
        let v = self.call("GET", "/api/v1/tiles", &[])?;
        let list = v["tiles"].as_array().ok_or("the tenant's tile list is not readable")?;
        Ok(list
            .iter()
            .filter_map(|t| {
                let id = t["id"].as_str().filter(|i| plain_id(i))?;
                Some(RemoteTile { id: id.to_string(), title: t["title"].as_str().unwrap_or(id).chars().filter(|c| !c.is_control()).take(80).collect(), abi: t["abi"].as_u64().unwrap_or(0) as u32, width: t["width"].as_u64().unwrap_or(0) as u32, height: t["height"].as_u64().unwrap_or(0) as u32, ticks: t["ticks"].as_bool().unwrap_or(false) })
            })
            .collect())
    }

    /// Send events to one tile and get its display list back.
    pub fn frame(&self, id: &str, r: &FrameRequest) -> Result<RemoteFrame, String> {
        if !plain_id(id) {
            return Err("not a tile name".into());
        }
        if r.events.len() > MAX_EVENTS {
            return Err(format!("at most {MAX_EVENTS} events go in one request"));
        }
        let mut body = json!({"width": r.width, "height": r.height, "scale_pct": r.scale_pct, "dark": r.dark, "events": r.events});
        if r.reset {
            body["reset"] = json!(true);
        }
        let v = self.call("POST", &format!("/api/v1/tiles/{id}/events"), body.to_string().as_bytes())?;
        if v.get("frame").is_none() {
            return Err("the tenant's answer has no picture".into());
        }
        Ok(RemoteFrame { redraw: v["redraw"].as_bool().unwrap_or(true), frame: v["frame"].clone(), stats: v["stats"].clone() })
    }

    /// End the session on the tenant's side (best effort).
    pub fn sign_out(&self) {
        let _ = self.call("POST", "/api/v1/logout", b"{}");
    }
}
