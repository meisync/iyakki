//! Fleet (F-FLEET-01, F-FLEET-02): a pair group of two or more IYAKKI systems, its signed record and the numbers the members share
//! (docs/development/FLEET-AND-MODEL-ROUTER-PLAN.md sections 3.1 and 3.3 step 4). This file is pure: records, keys, one-time codes and the
//! heartbeat figures. The channel between members is `fleet_hub.rs`, the chat tools and the permission asks are `fleettools.rs`.
//!
//! * A **fleet** has an id (128 random bits), a name, a version and a **group key** (32 random bytes). Every member has the keys of
//!   `cloudkeys::MemberKeys` (an Ed25519 signing key and an X25519 key from one secret that never leaves the member).
//! * The **record** lists the members (id = the member's signing key, so a member cannot be somebody else) and carries the group key wrapped
//!   for each member (`cloudkeys::ScopeKey::wrap`: only that member can open it). An administrator signs every new version. A member accepts a
//!   new version only if it is higher, signed by someone who is an administrator in the version it holds, and well formed, so every member holds
//!   a signed record and two members can still talk when the creator is away.
//! * **Removing** a member makes a new version with a new group key (generation + 1) wrapped for the members that remain: the removed member's
//!   key opens nothing any more, and its signatures are no longer accepted.

pub use iyakki_shared::fleetview::{Heartbeat, fingerprint};
use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cloudkeys::{hex, random, unhex, MemberKeys, PublicKeys, ScopeKey, Wrapped};

pub const DOMAIN_RECORD: &str = "iyakki-fleet-record-v1";
pub const DOMAIN_MSG: &str = "iyakki-fleet-msg-v1";
pub const DOMAIN_JOIN: &str = "iyakki-fleet-join-v1";
/// The most members of one fleet (a pair group is a handful of computers, not a datacentre).
pub const MAX_MEMBERS: usize = 32;
/// How long a join code lives, seconds.
pub const CODE_LIFE: u64 = 600;

/// Text that is safe to keep and show: no control characters, at most `max` characters.
pub fn clean(s: &str, max: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(max).collect::<String>().trim().to_string()
}


/// The short form of a fleet id for people (three groups).
pub fn short_id(fleet: &str) -> String {
    let f = fleet.get(..12).unwrap_or(fleet);
    f.as_bytes().chunks(4).map(|c| String::from_utf8_lossy(c).into_owned()).collect::<Vec<_>>().join("-")
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Member {
    /// The member's signing key, hex: its identity.
    pub id: String,
    /// The member's wrapping key, hex.
    pub wrap: String,
    pub name: String,
    pub admin: bool,
}

impl Member {
    pub fn keys(&self) -> PublicKeys {
        PublicKeys { wrap: self.wrap.clone(), sign: self.id.clone() }
    }
    pub fn of(keys: &MemberKeys, name: &str, admin: bool) -> Member {
        let p = keys.public();
        Member { id: p.sign, wrap: p.wrap, name: clean(name, 40), admin }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub fleet: String,
    pub name: String,
    pub version: u64,
    /// The generation of the group key (it rises when a member is removed).
    pub generation: u64,
    pub members: Vec<Member>,
    /// The group key of this generation, wrapped for each member by id.
    pub wraps: BTreeMap<String, Wrapped>,
    pub signer: String,
    pub sig: String,
}

impl Record {
    fn signing_bytes(&self) -> Vec<u8> {
        let mut c = self.clone();
        c.sig.clear();
        serde_json::to_vec(&c).unwrap_or_default()
    }

    fn sign(mut self, me: &MemberKeys) -> Record {
        self.signer = me.public().sign;
        self.sig = String::new();
        self.sig = me.sign_text(DOMAIN_RECORD, &self.signing_bytes());
        self
    }

    pub fn member(&self, id: &str) -> Option<&Member> {
        self.members.iter().find(|m| m.id == id)
    }

    pub fn admins(&self) -> Vec<&Member> {
        self.members.iter().filter(|m| m.admin).collect()
    }

    /// Whether the record is well formed and signed by one of its own administrators (does not say whether *we* trust it: see [`Record::accept_over`]).
    pub fn verify_self(&self) -> Result<(), String> {
        if self.fleet.len() != 32 || unhex(&self.fleet).is_none() {
            return Err("the fleet id is not 32 hex digits".into());
        }
        if self.members.is_empty() || self.members.len() > MAX_MEMBERS {
            return Err(format!("a fleet has 1 to {MAX_MEMBERS} members"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for m in &self.members {
            m.keys().check()?;
            if !seen.insert(m.id.clone()) {
                return Err("a member is listed twice".into());
            }
            if m.name != clean(&m.name, 40) || m.name.is_empty() {
                return Err("a member name is empty or has odd characters".into());
            }
            if !self.wraps.contains_key(&m.id) {
                return Err("a member has no wrapped group key".into());
            }
            self.wraps[&m.id].check()?;
        }
        if self.wraps.len() != self.members.len() {
            return Err("a wrapped key is for someone who is not a member".into());
        }
        if self.name != clean(&self.name, 60) || self.name.is_empty() {
            return Err("the fleet name is empty or has odd characters".into());
        }
        let signer = self.member(&self.signer).ok_or("the record is signed by somebody who is not a member")?;
        if !signer.admin {
            return Err("the record is signed by a member who is not an administrator".into());
        }
        if !signer.keys().verify_text(DOMAIN_RECORD, &self.signing_bytes(), &self.sig) {
            return Err("the signature of the record is not valid".into());
        }
        Ok(())
    }

    /// Whether `self` may replace the record `old` that this system holds: the same fleet, a higher version, well formed, and signed by someone
    /// who is an administrator in the record we hold. (A member that was removed is no administrator in the record that removes it, but it was
    /// one in the record we hold, so a removal is accepted; a member that is not in our record cannot sign anything we accept.)
    pub fn accept_over(&self, old: &Record) -> Result<(), String> {
        if self.fleet != old.fleet {
            return Err("this is another fleet".into());
        }
        if self.version <= old.version {
            return Err("this record is not newer than the one held".into());
        }
        if self.generation < old.generation {
            return Err("this record goes back to an older group key".into());
        }
        self.verify_self()?;
        match old.member(&self.signer) {
            Some(m) if m.admin => Ok(()),
            _ => Err("the record is not signed by an administrator this system knows".into()),
        }
    }

    /// A first record for a new fleet (the creator is its only member and administrator) and the group key.
    pub fn create(me: &MemberKeys, my_name: &str, fleet_name: &str) -> Result<(Record, ScopeKey), String> {
        let fleet_name = clean(fleet_name, 60);
        if fleet_name.is_empty() || clean(my_name, 40).is_empty() {
            return Err("a fleet and a member need a name".into());
        }
        let fleet = hex(&random::<16>()?);
        let key = ScopeKey::generate()?;
        let m = Member::of(me, my_name, true);
        let w = key.wrap(&fleet, &m.id, 0, &m.keys())?;
        let r = Record { fleet, name: fleet_name, version: 1, generation: 0, wraps: BTreeMap::from([(m.id.clone(), w)]), members: vec![m], signer: String::new(), sig: String::new() }.sign(me);
        Ok((r, key))
    }

    /// Open the group key of this record with this member's keys.
    pub fn open_key(&self, me: &MemberKeys) -> Result<ScopeKey, String> {
        let id = me.public().sign;
        let w = self.wraps.get(&id).ok_or("this system is not a member of this record")?;
        me.unwrap_key(&self.fleet, &id, self.generation, w)
    }

    /// The next version with a new member (same group key and generation); signed by `me`, who must be an administrator.
    pub fn with_member(&self, me: &MemberKeys, key: &ScopeKey, new: Member) -> Result<Record, String> {
        self.need_admin(me)?;
        if self.member(&new.id).is_some() {
            return Err("that computer is already a member".into());
        }
        if self.members.len() >= MAX_MEMBERS {
            return Err(format!("a fleet has at most {MAX_MEMBERS} members"));
        }
        new.keys().check()?;
        let mut r = self.clone();
        r.version += 1;
        r.wraps.insert(new.id.clone(), key.wrap(&self.fleet, &new.id, self.generation, &new.keys())?);
        r.members.push(new);
        Ok(r.sign(me))
    }

    fn need_admin(&self, me: &MemberKeys) -> Result<(), String> {
        match self.member(&me.public().sign) {
            Some(m) if m.admin => Ok(()),
            Some(_) => Err("only an administrator can do this".into()),
            None => Err("this system is not a member".into()),
        }
    }

    /// The next version without `id`: a new group key (generation + 1) wrapped for the members that remain. Returns the record and the new key.
    pub fn without_member(&self, me: &MemberKeys, id: &str) -> Result<(Record, ScopeKey), String> {
        self.need_admin(me)?;
        let target = self.member(id).ok_or("that computer is not a member")?;
        if target.admin && self.admins().len() == 1 {
            return Err("the last administrator cannot be removed: make another member an administrator first".into());
        }
        let mut r = self.clone();
        r.members.retain(|m| m.id != id);
        r.version += 1;
        r.generation += 1;
        let key = ScopeKey::generate()?;
        r.wraps = BTreeMap::new();
        for m in &r.members {
            r.wraps.insert(m.id.clone(), key.wrap(&r.fleet, &m.id, r.generation, &m.keys())?);
        }
        Ok((r.sign(me), key))
    }

    /// The next version with `id` made an administrator or not.
    pub fn with_admin(&self, me: &MemberKeys, id: &str, admin: bool) -> Result<Record, String> {
        self.need_admin(me)?;
        let mut r = self.clone();
        let m = r.members.iter_mut().find(|m| m.id == id).ok_or("that computer is not a member")?;
        if m.admin == admin {
            return Err("nothing changes".into());
        }
        m.admin = admin;
        if r.admins().is_empty() {
            return Err("a fleet needs at least one administrator".into());
        }
        r.version += 1;
        Ok(r.sign(me))
    }

    /// Find a member by its id, a prefix of its id (at least 6 digits), its fingerprint or its name (when only one member has that name).
    pub fn find(&self, what: &str) -> Result<&Member, String> {
        let w = what.trim();
        if w.is_empty() {
            return Err("say which member".into());
        }
        let lw = w.to_lowercase();
        let hits: Vec<&Member> = self.members.iter().filter(|m| m.id == lw || (lw.len() >= 6 && m.id.starts_with(&lw)) || fingerprint(&m.id) == lw || m.name.to_lowercase() == lw).collect();
        match hits.len() {
            1 => Ok(hits[0]),
            0 => Err(format!("no member matches {w}")),
            _ => Err(format!("more than one member matches {w}: use the fingerprint")),
        }
    }
}

// ---------------------------------------------------------------------------------------------- one-time codes

/// A new join code, nine digits.
pub fn new_code() -> Result<String, String> {
    let n = u64::from_be_bytes(random::<8>()?) % 1_000_000_000;
    Ok(format!("{n:09}"))
}

pub fn show_code(c: &str) -> String {
    if c.len() == 9 {
        format!("{}-{}-{}", &c[0..3], &c[3..6], &c[6..9])
    } else {
        c.to_string()
    }
}

/// Only digits count: `123-456-789` and `123456789` are the same code.
pub fn normalise_code(c: &str) -> String {
    c.chars().filter(char::is_ascii_digit).collect()
}

/// What an offer keeps instead of the code.
pub fn code_hash(fleet: &str, code: &str) -> [u8; 32] {
    Sha256::digest(format!("iyakki-fleet-code\0{fleet}\0{}", normalise_code(code)).as_bytes()).into()
}

pub fn same_hash(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

// ---------------------------------------------------------------------------------------------- heartbeats



/// Where the figures come from. The real one reads the core API; a test gives fixed numbers.
pub trait HeartbeatSource: Send + Sync {
    fn read(&self) -> Heartbeat;
}

pub struct FixedSource(pub Heartbeat);

impl HeartbeatSource for FixedSource {
    fn read(&self) -> Heartbeat {
        self.0.clone()
    }
}

/// The figures of this machine, through the core API (`Os::sysinfo`, `Os::disks`): the fleet never reads `/proc` itself.
pub struct OsSource {
    pub os: std::sync::Arc<dyn iyakki_core_api::Os>,
    /// The folder whose disk is measured (the data folder).
    pub data_dir: std::path::PathBuf,
    pub tasks: std::sync::Arc<dyn Fn() -> u32 + Send + Sync>,
    pub models: std::sync::Arc<dyn Fn() -> Vec<String> + Send + Sync>,
}

impl HeartbeatSource for OsSource {
    fn read(&self) -> Heartbeat {
        let mb = |b: u64| b / (1024 * 1024);
        let (free, total) = self.os.disks().space(&self.data_dir.to_string_lossy()).unwrap_or((0, 0));
        let cpus = self.os.sysinfo().cpu_count().unwrap_or(1).max(1);
        let load = self.os.sysinfo().loadavg().map(|l| l.one).unwrap_or(0.0);
        let mem = self.os.sysinfo().meminfo().unwrap_or_default();
        let used = if mem.total_kb == 0 { 0 } else { (100 - (mem.available_kb.min(mem.total_kb) * 100 / mem.total_kb)) as u32 };
        Heartbeat { disk_free_mb: mb(free), disk_total_mb: mb(total), cpu_load_pct: ((load / cpus as f64) * 100.0).round().clamp(0.0, 1000.0) as u32, cpus, mem_used_pct: used, mem_total_mb: mem.total_kb / 1024, tasks_running: (self.tasks)(), models: (self.models)(), battery_pct: iyakki_battery::read(&iyakki_battery::BatteryEnv::real()).percent.map(|p| p.round().max(0.0) as u32) }.sanitised()
    }
}
