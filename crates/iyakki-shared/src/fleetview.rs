use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Accept {
    /// Every assignment waits for this person (the default).
    #[default]
    Ask,
    /// Nothing is accepted.
    Never,
    /// Assignments from the members this person trusts are accepted at once; others wait.
    Members,
}

impl Accept {
    pub fn parse(s: &str) -> Option<Accept> {
        match s.trim().to_ascii_lowercase().as_str() {
            "ask" => Some(Accept::Ask),
            "never" => Some(Accept::Never),
            "members" => Some(Accept::Members),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            Accept::Ask => "ask me for each task",
            Accept::Never => "accept no work",
            Accept::Members => "accept work from members I trust, ask for the others",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InboxItem {
    pub id: String,
    pub fleet: String,
    pub from: String,
    pub from_name: String,
    pub body: AssignBody,
    /// `waiting`, `accepted` or `declined`.
    pub state: String,
    pub at: u64,
    /// The id of the local task that runs it (empty until it runs).
    #[serde(default)]
    pub task: String,
    /// What the person on this computer may want to read: why it could not start, how far it is.
    #[serde(default)]
    pub detail: String,
    /// What the planner answered to the task's question, waiting to be given to the task (F-FLEET-13).
    #[serde(default)]
    pub answer: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutItem {
    pub id: String,
    pub fleet: String,
    pub to: String,
    pub to_name: String,
    pub title: String,
    /// `sent`, `waiting` (queued for the other person), `accepted`, `declined` or `failed: ...`.
    pub state: String,
    pub at: u64,
    #[serde(default)]
    pub rows: Vec<String>,
    /// The assignment as sent, kept to send it again when the member goes silent.
    #[serde(default)]
    pub body: Option<AssignBody>,
    #[serde(default)]
    pub detail: String,
    /// The head commit and the test result the worker reported on hand-back.
    #[serde(default)]
    pub head: String,
    #[serde(default)]
    pub tests: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MemberView {
    pub id: String,
    pub fingerprint: String,
    pub name: String,
    pub admin: bool,
    pub me: bool,
    pub trusted: bool,
    /// `this computer`, `up`, `lost` (silent for three heartbeats), `unknown` (not heard yet) or `refused` (it said we are not a member).
    pub state: String,
    pub seen_secs_ago: Option<u64>,
    pub address_known: bool,
    /// How this system reaches the member: `tls` (over pinned TLS, F-FLEET-08), `lan` (the plain LAN channel) or empty (no address yet).
    pub transport: String,
    pub heartbeat: Option<Heartbeat>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FleetView {
    pub id: String,
    pub short: String,
    pub name: String,
    pub version: u64,
    pub generation: u64,
    pub i_am_admin: bool,
    pub members: Vec<MemberView>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OfferView {
    pub fleet: String,
    pub short: String,
    pub code: String,
    pub expires_in: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RequestView {
    pub id: String,
    pub fleet: String,
    pub short: String,
    pub name: String,
    pub fingerprint: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PendingView {
    pub ticket: String,
    pub addr: String,
    /// `waiting`, `offered`, `declined` or the failure.
    pub state: String,
    /// When offered: the fleet's name, the signer's fingerprint and the members, for the card.
    pub offer: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Status {
    pub me: String,
    pub my_fingerprint: String,
    pub listening: Option<u16>,
    pub accept: Accept,
    pub fleets: Vec<FleetView>,
    pub offers: Vec<OfferView>,
    pub requests: Vec<RequestView>,
    pub pending: Vec<PendingView>,
    pub inbox: Vec<InboxItem>,
    pub outbox: Vec<OutItem>,
    /// Whether this computer's channel speaks TLS (F-FLEET-15).
    pub tls: bool,
    /// `relay://MAILBOX` when the relay is on: what another member is given to reach this computer without a direct address (F-FLEET-10).
    pub relay: String,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// What an assignment carries: plain fields only. Unknown fields are refused, so an approval, a capability or a path cannot ride along.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssignBody {
    pub id: String,
    pub title: String,
    pub goal: String,
    #[serde(default)]
    pub base_branch: String,
    #[serde(default)]
    pub base_commit: String,
    #[serde(default)]
    pub rows: Vec<String>,
    /// A hash of the shared context the planner used (not the context).
    #[serde(default)]
    pub context_digest: String,
    #[serde(default)]
    pub budget_tokens: u64,
    /// The kind of model wanted: `small`, `large`, `vision`, `audio` or empty.
    #[serde(default)]
    pub model_class: String,
    /// Ids of tasks whose work must be applied before this one (the planner's dependency order).
    #[serde(default)]
    pub after: Vec<String>,
}

impl AssignBody {
    pub fn check(&self) -> Result<(), String> {
        let plain = |s: &str, max: usize, what: &str| if s.chars().count() > max || s.chars().any(|c| c.is_control() && c != '\n') { Err(format!("{what}: too long or odd characters")) } else { Ok(()) };
        if self.id.len() != 32 || !self.id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err("the task id is not 32 hex digits".into());
        }
        if self.title.trim().is_empty() {
            return Err("give the task a title".into());
        }
        plain(&self.title, 120, "title")?;
        if self.title.contains('\n') {
            return Err("title: one line".into());
        }
        if self.goal.trim().is_empty() {
            return Err("give the task a goal".into());
        }
        plain(&self.goal, 4000, "goal")?;
        let word = |s: &str, max: usize, what: &str| if s.len() > max || !s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '/' | '-' | ':')) { Err(format!("{what}: letters, digits and . _ / - : only, at most {max}")) } else { Ok(()) };
        word(&self.base_branch, 100, "base branch")?;
        word(&self.base_commit, 64, "base commit")?;
        word(&self.context_digest, 64, "context digest")?;
        word(&self.model_class, 24, "model class")?;
        if self.after.len() > 50 || self.after.iter().any(|a| a.len() != 32 || !a.chars().all(|c| c.is_ascii_hexdigit())) {
            return Err("after: at most 50 task ids of 32 hex digits".into());
        }
        if self.rows.len() > 50 {
            return Err("at most 50 board rows".into());
        }
        for r in &self.rows {
            word(r, 40, "board row")?;
        }
        if self.budget_tokens > 50_000_000 {
            return Err("the token budget is above the limit of 50 million".into());
        }
        for (t, w) in [(&self.title, "title"), (&self.goal, "goal")] {
            if looks_secret(t) {
                return Err(format!("the {w} looks like it holds a secret; no secret travels between computers, leave it out and let the worker use its own vault"));
            }
        }
        Ok(())
    }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use sha2::{Digest, Sha256};

/// What a person can compare by eye on both screens: sixteen hex digits of a hash of the member's key.
pub fn fingerprint(member_id: &str) -> String {
    let h = Sha256::digest(member_id.as_bytes());
    let x = hex(&h[..8]);
    format!("{}-{}-{}-{}", &x[0..4], &x[4..8], &x[8..12], &x[12..16])
}

/// What a member shares every 10 to 30 seconds: numbers (and the names of the models it has), read through the core API. No path, no content.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Heartbeat {
    pub disk_free_mb: u64,
    pub disk_total_mb: u64,
    /// The one-minute load as a percent of the processors (100 = all busy).
    pub cpu_load_pct: u32,
    pub cpus: u32,
    pub mem_used_pct: u32,
    pub mem_total_mb: u64,
    pub tasks_running: u32,
    pub models: Vec<String>,
    /// The battery's charge in percent on a machine that has one (absent on a desktop, and from members older than this field).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub battery_pct: Option<u32>,
}

impl Heartbeat {
    /// Keep only what a heartbeat may hold: percents at most 100 (load up to 1000), at most 24 model names of plain characters.
    pub fn sanitised(mut self) -> Heartbeat {
        self.cpu_load_pct = self.cpu_load_pct.min(1000);
        self.mem_used_pct = self.mem_used_pct.min(100);
        self.battery_pct = self.battery_pct.map(|b| b.min(100));
        self.disk_free_mb = self.disk_free_mb.min(self.disk_total_mb.max(self.disk_free_mb));
        let mut names: Vec<String> = Vec::new();
        for m in self.models.iter() {
            let tail = m.rsplit('/').next().unwrap_or(m);
            let n: String = tail.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ':' | '+' | '-')).take(48).collect();
            if !n.is_empty() && !names.contains(&n) {
                names.push(n);
            }
            if names.len() >= 24 {
                break;
            }
        }
        self.models = names;
        self
    }

    /// The same figures in one line for a screen or the chat.
    pub fn line(&self) -> String {
        let gb = |mb: u64| format!("{:.1} GB", mb as f64 / 1024.0);
        format!("disk {} free of {}, load {}% of {} cpus, memory {}% of {}, {} task(s) running, {} model(s)", gb(self.disk_free_mb), gb(self.disk_total_mb), self.cpu_load_pct, self.cpus, self.mem_used_pct, gb(self.mem_total_mb), self.tasks_running, self.models.len())
    }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----


/// Words that mean a secret is in the text. A coarse screen on both ends: nothing is meant to travel that looks like one.
pub fn looks_secret(t: &str) -> bool {
    let l = t.to_lowercase();
    if ["-----begin", "api_key", "apikey", "api-key", "secret=", "secret:", "password", "passwd", "token=", "bearer ", "private key", "recovery key", "ghp_", "akia"].iter().any(|w| l.contains(w)) {
        return true;
    }
    // "sk-" followed by a long run of letters and digits (an API key)
    let b = l.as_bytes();
    (0..b.len().saturating_sub(3)).any(|i| &b[i..i + 3] == b"sk-" && b[i + 3..].iter().take(16).filter(|c| c.is_ascii_alphanumeric()).count() >= 16)
}

/// What a confirmed fleet card does (the Settings page asks the services to run it: `ServicesHandle::fleet_act`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum FleetAct {
    Create(String, String),
    Offer(String),
    Join(String, String, String),
    Approve(String),
    Refuse(String),
    Accept(String),
    Drop(String),
    Remove(String, String),
    Leave(String),
    Admin(String, String, bool),
    Mode(String),
    Trust(String, String, bool),
    Decide(String, bool),
    /// The TLS switch of the channel (F-FLEET-15).
    Tls(bool),
    /// Where a member is reached: fleet, member, address (F-FLEET-15).
    Address(String, String, String),
    /// The relay switch (F-FLEET-10).
    Relay(bool),
    /// Apply what workers handed back to the integration branch; the flag sends the follow-ups too (F-FLEET-14).
    Integrate(String, bool),
    /// Put the integration branch into the person's own branch (F-FLEET-14).
    Finalize(String),
}
