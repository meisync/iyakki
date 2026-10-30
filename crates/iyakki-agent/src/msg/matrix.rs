//! Matrix over the Client-Server API (HTTPS+JSON), written here on the same `Upstream` HTTP seam as the rest of IYAKKI. No SDK.
//!
//! What it does: sign in with a password (the password is used once and not kept) or accept an access token, `whoami`, the joined rooms,
//! `/sync` as a long poll with a `since` token, `PUT /rooms/{id}/send/m.room.message/{txn}` with idempotent transaction ids, upload of a
//! picture or file (`m.image`, `m.file`), the 429 `retry_after_ms` wait, and size caps.
//!
//! What it refuses on purpose:
//! * **an encrypted room**: before every send the room's `m.room.encryption` state is read; if it is there, or if it cannot be read, nothing
//!   is sent and [`super::e2ee::REFUSAL`] says why (unless an add-on that implements [`super::e2ee::E2eeAddon`] is installed);
//! * **any host but the homeserver the person configured**: every URL is built from that base and checked again before it is used; the
//!   transport follows no redirect; an address typed with `cluster://` is looked up in the managed cluster the way DASH does it;
//! * **plain http to the internet**: http is for the cluster, loopback and the local network; the internet needs https.
//!
//! Inbound text is untrusted: control characters are removed, it is clipped, `is_self` is set from the sender, and an encrypted event
//! without an add-on becomes an empty message marked `unreadable_encrypted`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_core_api::{Cap, Channel, ChannelError, ChannelIdentity, ChannelInfo, Delivery, DeliveryState, Health, HealthState, InMessage, Limits, OutMessage, RateInfo, Trust};
use iyakki_proxy::Upstream;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::e2ee::{self, E2eeAddon};
use super::{clean_text, pct, scrub, strip_jpeg_metadata};
use crate::dash::cluster::ClusterApi;

pub const DEFAULT_MAX_ATTACHMENT: u64 = 2 << 20;
pub const HARD_MAX_ATTACHMENT: u64 = 10 << 20;
pub const MAX_TEXT_BYTES: usize = 8000;
/// The longest 429 wait the client sits through itself; a longer one is reported to the caller.
pub const MAX_INLINE_WAIT_MS: u64 = 10_000;
const MAX_ATTEMPTS: u32 = 3;
const MAX_EVENTS_PER_SYNC: usize = 200;
const MAX_ROOMS: usize = 500;

/// A homeserver address the person typed, checked: `cluster://ns/svc:port`, or http(s) with a host and no credentials, and https for
/// anything on the internet. Returns the address without a trailing slash.
pub fn check_homeserver(addr: &str) -> Result<String, String> {
    let t = addr.trim().trim_end_matches('/');
    match crate::dash::cluster::parse(t) {
        Some(Err(e)) => return Err(format!("the cluster address is not right: {e}")),
        Some(Ok(r)) if r.tail.is_empty() || r.tail == "/" => return Ok(t.to_string()),
        Some(Ok(_)) => return Err("write the cluster address without a path: cluster://<namespace>/<service>:<port>".into()),
        None => {}
    }
    let clean = crate::gitconn::clean_url(t)?;
    if clean.contains('?') || clean.contains('#') || clean.split("://").nth(1).is_some_and(|r| r.contains('/')) {
        return Err("write the address of the server without a path, for example https://matrix.example.org".into());
    }
    if clean.starts_with("http://") && crate::gitconn::placement_of(&clean, false) == crate::gitconn::Placement::Internet {
        return Err("a server on the internet must use https:// (the password would cross the internet in the clear)".into());
    }
    Ok(clean)
}

/// What a sign-in gives.
pub struct Login {
    pub access_token: Zeroizing<String>,
    pub user_id: String,
    pub device_id: String,
}

impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Login({} on device {}, token hidden)", self.user_id, self.device_id)
    }
}

struct St {
    since: Option<String>,
    health: Health,
    rate: RateInfo,
    user_id: String,
}

pub struct MatrixChannel {
    label: String,
    homeserver: String,
    token: Zeroizing<String>,
    up: Arc<dyn Upstream>,
    cluster: Arc<dyn ClusterApi>,
    sleeper: Arc<dyn Fn(Duration) + Send + Sync>,
    max_attachment: u64,
    addon: Option<Arc<dyn E2eeAddon>>,
    st: Mutex<St>,
}

fn real_sleep(d: Duration) {
    std::thread::sleep(d);
}

/// The homeserver to talk to now: a cluster address becomes the Service's current cluster IP.
fn resolve_base(homeserver: &str, cluster: &dyn ClusterApi) -> Result<String, ChannelError> {
    match crate::dash::cluster::parse(homeserver) {
        Some(Err(e)) => Err(ChannelError::Refused(format!("the cluster address is not right: {e}"))),
        Some(Ok(r)) => crate::dash::cluster::resolve(cluster, &r).map(|b| b.trim_end_matches('/').to_string()).map_err(|e| ChannelError::Offline(format!("{} ({})", e.message, e.fix.clone().unwrap_or_default()))),
        None => check_homeserver(homeserver).map_err(ChannelError::Refused),
    }
}

/// One HTTP exchange with the configured homeserver. The base is fixed; `path` must be a Matrix API path; the finished URL is compared
/// with the base once more. The token goes in a header only.
struct Wire<'a> {
    up: &'a dyn Upstream,
    base: String,
    token: Option<&'a str>,
    sleeper: &'a (dyn Fn(Duration) + Send + Sync),
}

struct Reply {
    status: u16,
    json: Value,
    waited: u32,
}

impl Wire<'_> {
    fn url(&self, path: &str) -> Result<String, ChannelError> {
        if !path.starts_with("/_matrix/") || path.contains("://") || path.contains("//") || path.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\' || c == '@') {
            return Err(ChannelError::Refused("internal error: a path outside the Matrix API was refused".into()));
        }
        let u = format!("{}{}", self.base, path);
        if crate::gitconn::authority_of(&u) != crate::gitconn::authority_of(&self.base) || !u.starts_with(&format!("{}/_matrix/", self.base)) {
            return Err(ChannelError::Refused("internal error: the address left the configured homeserver and was refused".into()));
        }
        Ok(u)
    }

    fn call(&self, method: &str, path: &str, ctype: &str, body: &[u8]) -> Result<Reply, ChannelError> {
        let url = self.url(path)?;
        let mut waited = 0;
        loop {
            let mut headers: Vec<(String, String)> = vec![("Accept".into(), "application/json".into())];
            if !body.is_empty() {
                headers.push(("Content-Type".into(), ctype.into()));
            }
            if let Some(t) = self.token {
                headers.push(("Authorization".into(), format!("Bearer {t}")));
            }
            let (resp, rh) = self.up.request_full(method, &url, &headers, body).map_err(|e| ChannelError::Offline(scrub(&clean_text(&e, 200), self.token.unwrap_or(""))))?;
            let json: Value = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
            if resp.status == 429 {
                let ms = json.get("retry_after_ms").and_then(Value::as_u64).or_else(|| rh.iter().find(|(k, _)| k == "retry-after").and_then(|(_, v)| v.trim().parse::<u64>().ok()).map(|s| s * 1000)).unwrap_or(1000);
                if ms > MAX_INLINE_WAIT_MS || waited + 1 >= MAX_ATTEMPTS {
                    return Err(ChannelError::RateLimited { retry_after_ms: ms });
                }
                (self.sleeper)(Duration::from_millis(ms));
                waited += 1;
                continue;
            }
            return Ok(Reply { status: resp.status, json, waited });
        }
    }

    fn json(&self, method: &str, path: &str, body: Option<&Value>) -> Result<Reply, ChannelError> {
        let bytes = body.map(|b| serde_json::to_vec(b).unwrap_or_default()).unwrap_or_default();
        self.call(method, path, "application/json", &bytes)
    }
}

/// A server's error text is outside text: short, plain.
fn server_error(r: &Reply, token: &str) -> String {
    let code = r.json.get("errcode").and_then(Value::as_str).unwrap_or("");
    let msg = r.json.get("error").and_then(Value::as_str).unwrap_or("");
    scrub(&clean_text(&format!("{} {} (HTTP {})", code, msg, r.status), 160), token)
}

fn check_ok(r: Reply, token: &str) -> Result<Reply, ChannelError> {
    match r.status {
        200..=299 => Ok(r),
        401 => Err(ChannelError::Auth("the homeserver does not accept the token any more (sign in again)".into())),
        413 => Err(ChannelError::TooLarge("the homeserver refused it as too large".into())),
        _ if r.json.get("errcode").and_then(Value::as_str) == Some("M_UNKNOWN_TOKEN") => Err(ChannelError::Auth("the homeserver does not accept the token any more (sign in again)".into())),
        _ => Err(ChannelError::Rejected(server_error(&r, token))),
    }
}

/// Sign in with a user name and password. The password is not kept anywhere by this function.
pub fn login(up: &Arc<dyn Upstream>, cluster: &Arc<dyn ClusterApi>, homeserver: &str, user: &str, password: &str) -> Result<Login, ChannelError> {
    if user.trim().is_empty() || password.is_empty() {
        return Err(ChannelError::Refused("give the user name and the password".into()));
    }
    let base = resolve_base(homeserver, cluster.as_ref())?;
    let wire = Wire { up: up.as_ref(), base, token: None, sleeper: &real_sleep };
    let body = Zeroizing::new(serde_json::to_vec(&json!({"type": "m.login.password", "identifier": {"type": "m.id.user", "user": user.trim()}, "password": password, "initial_device_display_name": "IYAKKI"})).unwrap_or_default());
    let r = wire.call("POST", "/_matrix/client/v3/login", "application/json", &body)?;
    match r.status {
        200 => {}
        400 if r.json.get("errcode").and_then(Value::as_str) == Some("M_UNRECOGNIZED") => return Err(ChannelError::Rejected("this server does not offer password sign-in: paste an access token instead".into())),
        401 | 403 => return Err(ChannelError::Auth("the user name or the password is not right".into())),
        _ => return Err(ChannelError::Rejected(server_error(&r, password))),
    }
    let token = r.json.get("access_token").and_then(Value::as_str).ok_or_else(|| ChannelError::Rejected("the server answered without an access token".into()))?;
    Ok(Login { access_token: Zeroizing::new(token.to_string()), user_id: r.json.get("user_id").and_then(Value::as_str).unwrap_or("").to_string(), device_id: r.json.get("device_id").and_then(Value::as_str).unwrap_or("").to_string() })
}

impl MatrixChannel {
    pub fn new(label: &str, homeserver: &str, token: Zeroizing<String>, up: Arc<dyn Upstream>, cluster: Arc<dyn ClusterApi>) -> MatrixChannel {
        MatrixChannel {
            label: label.into(),
            homeserver: homeserver.into(),
            token,
            up,
            cluster,
            sleeper: Arc::new(real_sleep),
            max_attachment: DEFAULT_MAX_ATTACHMENT,
            addon: e2ee::installed(),
            st: Mutex::new(St { since: None, health: Health::unknown(), rate: RateInfo::default(), user_id: String::new() }),
        }
    }

    pub fn with_sleeper(mut self, s: Arc<dyn Fn(Duration) + Send + Sync>) -> MatrixChannel {
        self.sleeper = s;
        self
    }
    pub fn with_max_attachment(mut self, bytes: u64) -> MatrixChannel {
        self.max_attachment = if bytes == 0 { DEFAULT_MAX_ATTACHMENT } else { bytes.min(HARD_MAX_ATTACHMENT) };
        self
    }
    pub fn with_since(self, since: Option<String>) -> MatrixChannel {
        self.st.lock().unwrap().since = since;
        self
    }
    pub fn with_known_user(self, user_id: &str) -> MatrixChannel {
        self.st.lock().unwrap().user_id = user_id.into();
        self
    }
    /// Use this add-on for encrypted rooms (tests, and the loader of the real add-on).
    pub fn with_e2ee(mut self, a: Option<Arc<dyn E2eeAddon>>) -> MatrixChannel {
        self.addon = a;
        self
    }
    /// The sync position to keep for the next start.
    pub fn since(&self) -> Option<String> {
        self.st.lock().unwrap().since.clone()
    }

    fn t(&self) -> &str {
        self.token.as_str()
    }

    fn note(&self, r: &Result<(), &ChannelError>) {
        let mut s = self.st.lock().unwrap();
        s.health = match r {
            Ok(()) => Health::ok(),
            Err(ChannelError::Auth(m)) => Health { state: HealthState::AuthFailed, reason: m.clone() },
            Err(ChannelError::RateLimited { retry_after_ms }) => {
                s.rate.retry_after_ms = Some(*retry_after_ms);
                Health { state: HealthState::RateLimited, reason: format!("the homeserver asked to wait {} s", retry_after_ms / 1000 + 1) }
            }
            Err(ChannelError::Offline(m)) => Health { state: HealthState::Offline, reason: m.clone() },
            // a rule or a bad request is not a sick connection
            Err(_) => return,
        };
        if matches!(r, Ok(())) {
            s.rate.retry_after_ms = None;
        }
    }

    /// Run one wire exchange with a fresh base (a cluster address is looked up each time), noting health and 429 waits.
    fn with_wire<T>(&self, f: impl FnOnce(&Wire) -> Result<T, ChannelError>) -> Result<T, ChannelError> {
        let out = resolve_base(&self.homeserver, self.cluster.as_ref()).and_then(|base| f(&Wire { up: self.up.as_ref(), base, token: Some(self.t()), sleeper: self.sleeper.as_ref() }));
        self.note(&out.as_ref().map(|_| ()));
        out
    }

    fn count_waits(&self, r: &Reply) {
        if r.waited > 0 {
            self.st.lock().unwrap().rate.limited_count += r.waited;
        }
    }

    pub fn whoami(&self) -> Result<String, ChannelError> {
        let id = self.with_wire(|w| {
            let r = check_ok(w.json("GET", "/_matrix/client/v3/account/whoami", None)?, self.t())?;
            self.count_waits(&r);
            Ok(r.json.get("user_id").and_then(Value::as_str).unwrap_or("").to_string())
        })?;
        if id.is_empty() {
            return Err(ChannelError::Rejected("the homeserver did not say who we are".into()));
        }
        self.st.lock().unwrap().user_id = id.clone();
        Ok(id)
    }

    pub fn joined_rooms(&self) -> Result<Vec<String>, ChannelError> {
        self.with_wire(|w| {
            let r = check_ok(w.json("GET", "/_matrix/client/v3/joined_rooms", None)?, self.t())?;
            self.count_waits(&r);
            Ok(r.json.get("joined_rooms").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).filter(|s| super::accounts::valid_room_id(s)).take(MAX_ROOMS).map(String::from).collect()).unwrap_or_default())
        })
    }

    /// `Ok(true)`: the room is encrypted. A room whose state cannot be read counts as encrypted for the caller (fail closed).
    fn is_encrypted(&self, w: &Wire, room: &str) -> Result<bool, ChannelError> {
        let r = w.json("GET", &format!("/_matrix/client/v3/rooms/{}/state/m.room.encryption", pct(room)), None)?;
        match r.status {
            200..=299 => Ok(true),
            404 => Ok(false),
            403 => Err(ChannelError::Refused("this account is not in that room (join it first)".into())),
            _ => Err(ChannelError::Refused(format!("could not tell whether the room is encrypted, so nothing was sent ({})", server_error(&r, self.t())))),
        }
    }

    fn txn(key: &str, part: &str) -> String {
        let h = Sha256::digest(format!("iyakki-matrix|{key}|{part}").as_bytes());
        format!("iy-{}", h.iter().take(12).map(|b| format!("{b:02x}")).collect::<String>())
    }

    fn put_event(&self, w: &Wire, room: &str, event_type: &str, content: &Value, txn: &str) -> Result<String, ChannelError> {
        let r = check_ok(w.json("PUT", &format!("/_matrix/client/v3/rooms/{}/send/{}/{}", pct(room), pct(event_type), pct(txn)), Some(content))?, self.t())?;
        self.count_waits(&r);
        r.json.get("event_id").and_then(Value::as_str).map(String::from).ok_or_else(|| ChannelError::Rejected("the server accepted the message but gave no event id".into()))
    }

    /// Send one event; in an encrypted room only through the installed add-on.
    fn send_event(&self, w: &Wire, room: &str, content: Value, txn: &str, encrypted: bool) -> Result<String, ChannelError> {
        if !encrypted {
            return self.put_event(w, room, "m.room.message", &content, txn);
        }
        let Some(a) = &self.addon else { return Err(ChannelError::Refused(e2ee::REFUSAL.into())) };
        let (ty, enc) = a.encrypt(room, "m.room.message", &content).map_err(|e| ChannelError::Refused(format!("the encryption add-on could not encrypt it, so nothing was sent: {}", clean_text(&e, 160))))?;
        self.put_event(w, room, &ty, &enc, txn)
    }

    fn upload(&self, w: &Wire, name: &str, mime: &str, bytes: &[u8]) -> Result<String, ChannelError> {
        let safe: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_')).take(80).collect();
        let path = format!("/_matrix/media/v3/upload?filename={}", pct(if safe.is_empty() { "file" } else { &safe }));
        let ctype = if mime.contains(['\r', '\n']) || mime.is_empty() { "application/octet-stream" } else { mime };
        let r = check_ok(w.call("POST", &path, ctype, bytes)?, self.t())?;
        self.count_waits(&r);
        r.json.get("content_uri").and_then(Value::as_str).filter(|u| u.starts_with("mxc://")).map(String::from).ok_or_else(|| ChannelError::Rejected("the server accepted the upload but gave no address for it".into()))
    }

    fn read_attachment(&self, path: &std::path::Path, mime: &str) -> Result<Vec<u8>, ChannelError> {
        let meta = std::fs::metadata(path).map_err(|_| ChannelError::Refused("the picture to send is not there any more".into()))?;
        if !meta.is_file() {
            return Err(ChannelError::Refused("the picture to send is not a file".into()));
        }
        if meta.len() > self.max_attachment {
            return Err(ChannelError::TooLarge(format!("the file is {} KB, the limit for this account is {} KB", meta.len() / 1024, self.max_attachment / 1024)));
        }
        let bytes = std::fs::read(path).map_err(|e| ChannelError::Refused(format!("could not read the file: {e}")))?;
        Ok(if mime == "image/jpeg" { strip_jpeg_metadata(&bytes) } else { bytes })
    }

    fn parse_events(&self, v: &Value, own: &str, out: &mut Vec<InMessage>) {
        let Some(rooms) = v.pointer("/rooms/join").and_then(Value::as_object) else { return };
        for (room, data) in rooms {
            let Some(events) = data.pointer("/timeline/events").and_then(Value::as_array) else { continue };
            for e in events {
                if out.len() >= MAX_EVENTS_PER_SYNC {
                    return;
                }
                let ty = e.get("type").and_then(Value::as_str).unwrap_or("");
                let from = clean_text(e.get("sender").and_then(Value::as_str).unwrap_or(""), 120);
                let base = |text: String, unreadable: bool| InMessage {
                    room: room.clone(),
                    external_id: clean_text(e.get("event_id").and_then(Value::as_str).unwrap_or(""), 120),
                    from: from.clone(),
                    time_ms: e.get("origin_server_ts").and_then(Value::as_i64).unwrap_or(0),
                    text,
                    is_self: !own.is_empty() && from == own,
                    unreadable_encrypted: unreadable,
                    trust: Trust::External,
                };
                match ty {
                    "m.room.message" => out.push(base(Self::message_text(e.get("content").unwrap_or(&Value::Null)), false)),
                    "m.room.encrypted" => match self.addon.as_ref().and_then(|a| a.decrypt(room, e).ok()) {
                        Some(clear) => out.push(base(Self::message_text(&clear), false)),
                        None => out.push(base(String::new(), true)),
                    },
                    _ => {}
                }
            }
        }
    }

    fn message_text(content: &Value) -> String {
        let body = clean_text(content.get("body").and_then(Value::as_str).unwrap_or(""), 2000);
        match content.get("msgtype").and_then(Value::as_str).unwrap_or("") {
            "m.text" | "m.notice" | "m.emote" => body,
            "m.image" => format!("[picture: {}]", clean_text(&body, 80)),
            "m.file" | "m.video" | "m.audio" => format!("[file: {}]", clean_text(&body, 80)),
            _ => "[message of a kind this screen does not show]".into(),
        }
    }
}

impl Channel for MatrixChannel {
    fn info(&self) -> ChannelInfo {
        ChannelInfo { kind: "matrix".into(), display: self.label.clone(), caps: vec![Cap::Text, Cap::Images, Cap::Files, Cap::InboundPoll, Cap::Groups], encrypted_rooms: self.addon.is_some() }
    }

    fn identity(&self) -> Result<ChannelIdentity, ChannelError> {
        let known = self.st.lock().unwrap().user_id.clone();
        let id = if known.is_empty() { self.whoami()? } else { known };
        Ok(ChannelIdentity { display: id.split(':').next().unwrap_or(&id).trim_start_matches('@').to_string(), id })
    }

    fn health(&self) -> Health {
        self.st.lock().unwrap().health.clone()
    }

    fn send(&self, m: &OutMessage) -> Result<Delivery, ChannelError> {
        if !super::accounts::valid_room_id(&m.to) {
            return Err(ChannelError::Refused("that is not a Matrix room id".into()));
        }
        if m.text.trim().is_empty() && m.attachment.is_none() {
            return Err(ChannelError::Refused("there is nothing to send".into()));
        }
        let mut text = m.text.clone();
        if text.len() > MAX_TEXT_BYTES {
            let mut end = MAX_TEXT_BYTES - 12;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
            text.push_str("\n[cut short]");
        }
        // read and check the picture before anything leaves, so a too-large file stops the whole send
        let att = match &m.attachment {
            Some(a) => Some((a, self.read_attachment(&a.path, &a.mime)?)),
            None => None,
        };
        self.with_wire(|w| {
            let encrypted = self.is_encrypted(w, &m.to)?;
            if encrypted && self.addon.is_none() {
                return Err(ChannelError::Refused(e2ee::REFUSAL.into()));
            }
            let uri = match &att {
                Some((a, bytes)) => Some(self.upload(w, &a.name, &a.mime, bytes)?),
                None => None,
            };
            let mut last = String::new();
            if !text.trim().is_empty() {
                last = self.send_event(w, &m.to, json!({"msgtype": "m.text", "body": text}), &Self::txn(&m.dedupe_key, "text"), encrypted)?;
            }
            if let (Some((a, bytes)), Some(uri)) = (&att, uri) {
                let kind = if a.mime.starts_with("image/") { "m.image" } else { "m.file" };
                let name: String = a.name.chars().filter(|c| !c.is_control()).take(80).collect();
                let content = json!({"msgtype": kind, "body": if name.is_empty() { "file".to_string() } else { name }, "url": uri, "info": {"mimetype": a.mime, "size": bytes.len()}});
                last = self.send_event(w, &m.to, content, &Self::txn(&m.dedupe_key, "file"), encrypted).map_err(|e| match e {
                    ChannelError::Rejected(r) if !text.trim().is_empty() => ChannelError::Rejected(format!("the text was sent but the picture was not: {r}")),
                    other => other,
                })?;
            }
            Ok(Delivery { external_id: last, state: DeliveryState::Accepted })
        })
    }

    fn poll(&self, wait: Duration) -> Result<Vec<InMessage>, ChannelError> {
        let since = self.st.lock().unwrap().since.clone();
        let own = {
            let known = self.st.lock().unwrap().user_id.clone();
            if known.is_empty() { self.whoami().unwrap_or_default() } else { known }
        };
        let filter = json!({"room": {"timeline": {"limit": if since.is_some() { 20 } else { 1 }}, "state": {"lazy_load_members": true}, "ephemeral": {"types": []}, "account_data": {"types": []}}, "presence": {"types": []}, "account_data": {"types": []}});
        let timeout_ms = if since.is_some() { wait.as_millis().min(30_000) as u64 } else { 0 };
        let mut path = format!("/_matrix/client/v3/sync?timeout={timeout_ms}&filter={}", pct(&filter.to_string()));
        if let Some(s) = &since {
            path.push_str(&format!("&since={}", pct(s)));
        }
        let v = self.with_wire(|w| {
            let r = check_ok(w.json("GET", &path, None)?, self.t())?;
            self.count_waits(&r);
            Ok(r.json)
        })?;
        let next = v.get("next_batch").and_then(Value::as_str).filter(|s| !s.is_empty() && s.len() < 512).map(String::from);
        let mut out = Vec::new();
        // the very first sync only finds our place: old messages are not replayed
        if since.is_some() {
            self.parse_events(&v, &own, &mut out);
        }
        if let Some(a) = &self.addon {
            // the add-on may need the homeserver (keys): it asks, we do the call here, against the same host
            if let Ok(reqs) = a.on_sync(&v) {
                for rq in reqs.into_iter().take(10) {
                    let method = if rq.method.eq_ignore_ascii_case("GET") { "GET" } else { "POST" };
                    let _ = self.with_wire(|w| w.json(method, &format!("/_matrix/client/{}", rq.path.trim_start_matches('/')), Some(&rq.body)));
                }
            }
        }
        if let Some(n) = next {
            self.st.lock().unwrap().since = Some(n);
        }
        Ok(out)
    }

    fn limits(&self) -> Limits {
        Limits { max_text_bytes: MAX_TEXT_BYTES, max_attachment_bytes: self.max_attachment, per_hour: 30 }
    }

    fn rate_info(&self) -> RateInfo {
        self.st.lock().unwrap().rate
    }
}
