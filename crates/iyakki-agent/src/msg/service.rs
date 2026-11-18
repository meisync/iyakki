//! The manager the shell, the flows `notify` node and the assistant call. It owns no socket: it builds a [`MatrixChannel`] for one call
//! from the account list and the vault, applies the hourly cap, and writes audit lines that carry names and counts only (never a
//! message text, a room id of someone else, a token or a file name).

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use iyakki_core_api::{Channel, ChannelError, InMessage, OutAttachment, OutMessage};
use iyakki_proxy::Upstream;
use zeroize::Zeroizing;

use super::accounts::{Account, Accounts, RoomRef};
use super::matrix::{self, MatrixChannel};
use crate::dash::cluster::ClusterApi;
use crate::gitkeys::AuditFn;

/// Sends per account and hour. A flow that fires every second sends at most this many; the rest are counted and refused with a sentence.
pub const PER_HOUR: usize = 30;

/// How to sign in.
pub enum Auth {
    /// The password is used once, to get an access token, and is not kept.
    Password { user: String, password: Zeroizing<String> },
    /// An access token the person made in another client.
    Token(Zeroizing<String>),
}

impl Auth {
    /// For callers that do not link `zeroize`: the string is moved in and wiped when the sign-in is done.
    pub fn password(user: &str, password: String) -> Auth {
        Auth::Password { user: user.trim().to_string(), password: Zeroizing::new(password) }
    }
    pub fn token(token: String) -> Auth {
        Auth::Token(Zeroizing::new(token))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendReport {
    pub account: String,
    pub room: String,
    pub external_id: String,
    pub dry_run: bool,
    /// Plain words for a run log or a status line.
    pub note: String,
}

fn caps() -> &'static Mutex<HashMap<String, Vec<i64>>> {
    static C: OnceLock<Mutex<HashMap<String, Vec<i64>>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

pub struct MessageService {
    acc: Accounts,
    up: Arc<dyn Upstream>,
    cluster: Arc<dyn ClusterApi>,
    now: Arc<dyn Fn() -> i64 + Send + Sync>,
    sleeper: Arc<dyn Fn(Duration) + Send + Sync>,
}

fn now_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn err_text(e: &ChannelError) -> String {
    match e {
        ChannelError::RateLimited { retry_after_ms } => format!("the homeserver asks to wait {} seconds before the next message", retry_after_ms / 1000 + 1),
        other => other.to_string(),
    }
}

fn err_class(e: &ChannelError) -> &'static str {
    match e {
        ChannelError::Auth(_) => "auth",
        ChannelError::RateLimited { .. } => "rate_limited",
        ChannelError::Refused(_) => "refused",
        ChannelError::TooLarge(_) => "too_large",
        ChannelError::Offline(_) => "offline",
        ChannelError::Rejected(_) => "rejected",
    }
}

impl MessageService {
    pub fn new(acc: Accounts, up: Arc<dyn Upstream>, cluster: Arc<dyn ClusterApi>) -> MessageService {
        MessageService { acc, up, cluster, now: Arc::new(now_secs), sleeper: Arc::new(std::thread::sleep) }
    }

    /// The service of one signed-in person: their vault, the managed cluster for `cluster://` addresses, the system's HTTP client.
    pub fn for_services(svc: &Arc<crate::services::Services>, user: &str, audit: AuditFn) -> MessageService {
        let cluster: Arc<dyn ClusterApi> = Arc::new(crate::dash::cluster::KubeCluster::new(svc.k3sdev.clone(), Duration::from_secs(10)));
        MessageService::new(Accounts::for_services(svc, user, audit), svc.slow_upstream(), cluster)
    }

    pub fn with_clock(mut self, now: Arc<dyn Fn() -> i64 + Send + Sync>) -> MessageService {
        self.now = now;
        self
    }
    pub fn with_sleeper(mut self, s: Arc<dyn Fn(Duration) + Send + Sync>) -> MessageService {
        self.sleeper = s;
        self
    }

    pub fn accounts(&self) -> &Accounts {
        &self.acc
    }

    fn channel(&self, a: &Account) -> Result<MatrixChannel, String> {
        let token = self.acc.token(&a.id)?;
        Ok(MatrixChannel::new(&a.label, &a.homeserver, token, self.up.clone(), self.cluster.clone())
            .with_sleeper(self.sleeper.clone())
            .with_max_attachment(a.max_attachment)
            .with_since(self.acc.since(&a.id))
            .with_known_user(&a.user_id))
    }

    /// Sign in (or check the token), keep the token in the vault, remember the account. With `room_id` that room is saved as `main`;
    /// without it, an account that is in exactly one room gets that one.
    pub fn add_matrix(&self, label: &str, homeserver: &str, auth: Auth, room_id: &str) -> Result<Account, String> {
        let hs = matrix::check_homeserver(homeserver)?;
        let (token, user_id, device_id) = match auth {
            Auth::Password { user, password } => {
                let l = matrix::login(&self.up, &self.cluster, &hs, &user, &password).map_err(|e| err_text(&e))?;
                (l.access_token, l.user_id, l.device_id)
            }
            Auth::Token(t) => {
                if t.trim().is_empty() {
                    return Err("paste the access token".into());
                }
                let t = Zeroizing::new(t.trim().to_string());
                let ch = MatrixChannel::new(label, &hs, t.clone(), self.up.clone(), self.cluster.clone());
                let uid = ch.whoami().map_err(|e| err_text(&e))?;
                (t, uid, String::new())
            }
        };
        let a = self.acc.add(Account { id: String::new(), kind: "matrix".into(), label: label.trim().into(), homeserver: hs.clone(), user_id, device_id, max_attachment: 0, created: (self.now)() }, &token)?;
        self.acc.audit("account.add", &format!("account=\"{}\" kind=matrix server={}", a.label, crate::gitconn::authority_of(&hs)));
        let room = room_id.trim();
        if !room.is_empty() {
            self.acc.add_room(&a.id, "main", room)?;
        } else if let Ok(ch) = self.channel(&a) {
            if let Ok(rooms) = ch.joined_rooms() {
                if rooms.len() == 1 {
                    let _ = self.acc.add_room(&a.id, "main", &rooms[0]);
                }
            }
        }
        Ok(a)
    }

    pub fn remove(&self, id: &str) -> Result<(), String> {
        let a = self.acc.pick(id)?;
        self.acc.remove(&a.id)?;
        self.acc.audit("account.remove", &format!("account=\"{}\"", a.label));
        Ok(())
    }

    /// Who the homeserver says we are (the "Test" of an account).
    pub fn test(&self, id: &str) -> Result<String, String> {
        let a = self.acc.pick(id)?;
        let ch = self.channel(&a)?;
        let me = ch.whoami().map_err(|e| err_text(&e))?;
        Ok(format!("signed in as {me} on {}", crate::gitconn::authority_of(&a.homeserver)))
    }

    /// The rooms the account has joined (ids), for choosing one.
    pub fn joined_rooms(&self, id: &str) -> Result<Vec<String>, String> {
        let a = self.acc.pick(id)?;
        self.channel(&a)?.joined_rooms().map_err(|e| err_text(&e))
    }

    /// A room by the name the person gave it; an empty name is the only room, or `main`.
    pub fn resolve_room(&self, a: &Account, name: &str) -> Result<RoomRef, String> {
        let rooms = self.acc.rooms(&a.id);
        let n = name.trim();
        if n.is_empty() {
            return match rooms.len() {
                0 => Err("no room is set for this account (Settings, Notifications, Matrix account, Room)".into()),
                1 => Ok(rooms[0].clone()),
                _ => rooms.iter().find(|r| r.name == "main").cloned().ok_or_else(|| "there are several rooms: say which one".to_string()),
            };
        }
        rooms.into_iter().find(|r| r.name.eq_ignore_ascii_case(n)).ok_or_else(|| format!("there is no room called `{n}` for this account; rooms are the names you saved in Settings"))
    }

    /// Count a send against the hourly cap; refuse when it is used up. `record: false` only looks.
    fn cap(&self, a: &Account, record: bool) -> Result<(), String> {
        let now = (self.now)();
        let key = format!("{}|{}", self.acc_key(), a.id);
        let mut m = caps().lock().unwrap_or_else(|e| e.into_inner());
        let v = m.entry(key).or_default();
        v.retain(|t| now - *t < 3600);
        if v.len() >= PER_HOUR {
            let wait = v.first().map_or(0, |t| 3600 - (now - *t));
            return Err(format!("not sent: {PER_HOUR} messages in the last hour are the limit for this account (the next one can go in about {} minutes)", wait / 60 + 1));
        }
        if record {
            v.push(now);
        }
        Ok(())
    }

    fn acc_key(&self) -> String {
        self.acc.dir_key()
    }

    /// Send a text, and a picture if given, to a saved room. `dedupe_key` makes a retry of the same message harmless. With `dry_run`
    /// nothing leaves the machine and no cap is used up.
    pub fn send(&self, account: &str, room: &str, text: &str, attach: Option<OutAttachment>, dedupe_key: &str, dry_run: bool) -> Result<SendReport, String> {
        let a = self.acc.pick(account)?;
        let r = self.resolve_room(&a, room)?;
        if dry_run {
            self.cap(&a, false)?;
            return Ok(SendReport { account: a.label.clone(), room: r.name.clone(), external_id: String::new(), dry_run: true, note: format!("dry run: would send {} characters{} to the room \"{}\" on {} (nothing was sent; the room's encryption was not checked)", text.chars().count(), if attach.is_some() { " and a picture" } else { "" }, r.name, a.label) });
        }
        self.cap(&a, true)?;
        let bytes = text.len();
        let ch = self.channel(&a)?;
        let key = if dedupe_key.is_empty() { format!("{}-{}", (self.now)(), text.len()) } else { dedupe_key.to_string() };
        let m = OutMessage { to: r.room_id.clone(), text: text.to_string(), attachment: attach, dedupe_key: key };
        match ch.send(&m) {
            Ok(d) => {
                self.acc.audit("send", &format!("account=\"{}\" channel=matrix room={} result=ok bytes={bytes}", a.label, r.name));
                Ok(SendReport { account: a.label, room: r.name.clone(), external_id: d.external_id, dry_run: false, note: format!("accepted by the homeserver for the room \"{}\" (accepted is not read)", r.name) })
            }
            Err(e) => {
                self.acc.audit("send", &format!("account=\"{}\" channel=matrix room={} result={}", a.label, r.name, err_class(&e)));
                Err(err_text(&e))
            }
        }
    }

    /// The fixed harmless message of the test-send button. It counts against the cap.
    pub fn test_send(&self, account: &str, room: &str) -> Result<SendReport, String> {
        self.send(account, room, "IYAKKI test: this is a test message from your computer.", None, &format!("test-{}", (self.now)()), false)
    }

    /// What arrived since the last call (a long poll of up to `wait`); the sync position is kept for the next call. Every message is
    /// untrusted (`Trust::External`).
    pub fn poll_once(&self, account: &str, wait: Duration) -> Result<Vec<InMessage>, String> {
        let a = self.acc.pick(account)?;
        let ch = self.channel(&a)?;
        let out = ch.poll(wait);
        if let Some(s) = ch.since() {
            let _ = self.acc.set_since(&a.id, &s);
        }
        match out {
            Ok(v) => {
                self.acc.audit("recv", &format!("account=\"{}\" channel=matrix count={}", a.label, v.len()));
                Ok(v)
            }
            Err(e) => {
                self.acc.audit("health", &format!("account=\"{}\" state={}", a.label, err_class(&e)));
                Err(err_text(&e))
            }
        }
    }
}
