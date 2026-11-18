//! The assistant's tools for pair groups (F-FLEET-09, chat half) and the one place that asks the core before the hub changes anything
//! (F-FLEET-07). `fleet_status` is free (it reads, through `fleet.read`); `fleet_pair`, `fleet_leave`, `fleet_assign` and `fleet_receive` change
//! who this computer is paired with, or send or accept work, so each ALWAYS asks, even in Auto mode (`agent::hard_ask`), asks the core
//! (`fleet.join`, `fleet.manage`, `fleet.assign`, `fleet.receive`: a card every time) and writes an audit line with ids and numbers, never the
//! goal. The Settings page calls the same [`FleetApi`], so the screen and the chat are one thing.
//!
//! Nothing here reaches the machine or the network itself: it asks the core, then uses the hub (`fleet_hub.rs`), which reads the machine only
//! through the core API (`fleet::OsSource`).

use std::sync::Arc;

use iyakki_core_api::fleet::Fleet;
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::cloudkeys::{hex, random};
use crate::fleet::{fingerprint, HeartbeatSource};
use crate::fleet_hub::{Accept, AssignBody, Hub, Status};

pub const STATUS: &str = "fleet_status";
pub const PAIR: &str = "fleet_pair";
pub const LEAVE: &str = "fleet_leave";
pub const ASSIGN: &str = "fleet_assign";
pub const RECEIVE: &str = "fleet_receive";
pub const WORK: &str = "fleet_work";

/// The port the channel listens on unless the person's settings say another.
pub const DEFAULT_PORT: u16 = 7671;

pub const PROMPT_RULE: &str = "\nFLEET: a fleet is a pair group of two or more IYAKKI computers. fleet_status shows the fleets, members, their numbers (disk, load, memory), pending join requests and the inbox of work from other members; it never shows a join code. Creating a fleet, offering a one-time join code, joining with a code, approving or removing a member, leaving, sending work (fleet_assign) and accepting or declining work or changing whether work is accepted (fleet_receive) all ask the person every time, and the person on the other computer is asked too. Never join, pair or send work the person did not ask for. No secret goes in a task: leave passwords, keys and tokens out; the other computer uses its own vault.";

pub fn is_fleet_tool(n: &str) -> bool {
    matches!(n, STATUS | PAIR | LEAVE | ASSIGN | RECEIVE | WORK)
}

/// What the screen and the tools share: the hub, the core's door and the audit callback.
#[derive(Clone)]
pub struct FleetApi {
    pub hub: Arc<Hub>,
    /// The data folder, for the certificate of the TLS channel (the developer-mode one); none in tests that do not use TLS.
    dir: Option<std::path::PathBuf>,
    gate: Arc<dyn Fleet>,
    source: Arc<dyn HeartbeatSource>,
}

fn text<'a>(a: &'a Value, k: &str) -> &'a str {
    a.get(k).and_then(Value::as_str).unwrap_or("").trim()
}

/// Ask the core with the press that approved the call (the tool runs on another thread than the card's).
fn ask<R>(f: impl FnOnce() -> R) -> R {
    crate::routertools::with_press(f)
}

impl FleetApi {
    pub fn new(hub: Arc<Hub>, gate: Arc<dyn Fleet>, source: Arc<dyn HeartbeatSource>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> FleetApi {
        hub.set_audit(audit);
        let (weak, g) = (Arc::downgrade(&hub), gate.clone());
        hub.set_lost_hook(Arc::new(move || {
            if let Some(h) = weak.upgrade() {
                let _ = requeue_with(&h, g.as_ref(), false);
            }
        }));
        FleetApi { hub, dir: None, gate, source }
    }

    /// Give the door the data folder: the certificate of the TLS channel is kept there (`tls/`, the developer-mode one).
    pub fn with_dir(mut self, dir: &std::path::Path) -> FleetApi {
        self.dir = Some(dir.to_path_buf());
        self
    }

    /// Load the certificate when the person turned the TLS channel on, so the node about to start listens over TLS.
    fn apply_tls(&self) -> Result<(), String> {
        if !self.hub.tls_wanted() {
            return Ok(());
        }
        let dir = self.dir.as_ref().ok_or("this system has no certificate folder")?;
        let id = crate::webservice::tls::self_signed(&crate::devmode::tls_dir(dir), &[])?;
        self.hub.set_tls(id.config, &id.fingerprint);
        Ok(())
    }

    /// Turn the TLS channel on or off (it asks first). The channel restarts, so members learn the new transport from the next heartbeat.
    pub fn use_tls(&self, on: bool) -> Result<String, String> {
        self.manage(if on { "make the fleet channel use TLS" } else { "make the fleet channel plain again" })?;
        self.hub.set_tls_wanted(on);
        self.apply_tls()?;
        let was = self.hub.listening().is_some();
        self.hub.stop_node();
        if was || on {
            self.ensure_node()?;
        }
        match (on, self.hub.tls_fingerprint()) {
            (true, Some(fp)) => Ok(format!("The fleet channel now uses TLS on port {}. Its certificate fingerprint is {}; another computer far away reaches this one at tls://THIS-ADDRESS:{}#{} after the person compared the fingerprint.", self.hub.listening().map(|a| a.port()).unwrap_or(0), crate::fleet_hub::colon_fp(&fp), self.hub.listening().map(|a| a.port()).unwrap_or(0), crate::fleet_hub::colon_fp(&fp))),
            _ => Ok("The fleet channel is plain again (LAN only).".into()),
        }
    }

    /// How often this computer shares its figures with the other members, 10 to 30 seconds (it asks first).
    pub fn set_interval(&self, secs: u64) -> Result<String, String> {
        self.manage(&format!("share this computer's figures every {secs} seconds"))?;
        self.hub.set_person_interval(secs)?;
        Ok(format!("This computer now shares its figures every {secs} seconds."))
    }

    /// Tell this computer where a member is: `host:port`, or `tls://host:port#fingerprint` for a member beyond the LAN (it asks first).
    pub fn address(&self, fleet: &str, member: &str, addr: &str) -> Result<String, String> {
        let id = self.hub.fleet_id(fleet)?;
        self.manage(&format!("set where {member} is reached ({})", addr.trim().split('#').next().unwrap_or("")))?;
        let t = self.hub.set_member_address(&id, member, addr.trim())?;
        Ok(format!("This computer will reach that member at {t}."))
    }

    /// Start the channel when this computer is in a fleet or is making one. Listening is the person's doing (a card made the fleet), never automatic
    /// for a computer that has never paired.
    pub fn ensure_node(&self) -> Result<(), String> {
        if self.hub.listening().is_some() {
            return Ok(());
        }
        self.apply_tls()?;
        let bind = self.hub.bind();
        match self.hub.start_node(&bind, self.source.clone()) {
            Ok(_) => Ok(()),
            Err(_) if bind.ends_with(&format!(":{DEFAULT_PORT}")) => self.hub.start_node("0.0.0.0:0", self.source.clone()).map(|_| ()),
            Err(e) => Err(e),
        }
    }

    /// Start the channel again after a restart if this computer is already in a fleet. Reads nothing from the network.
    pub fn resume(&self) {
        if !self.hub.status().fleets.is_empty() {
            let _ = self.ensure_node();
        }
    }

    fn manage(&self, label: &str) -> Result<(), String> {
        ask(|| self.gate.manage(label))
    }

    pub fn status(&self) -> Result<Status, String> {
        self.gate.read("fleet members and numbers")?;
        self.hub.poll_pending();
        Ok(self.hub.status())
    }

    /// The same as a plain text for the chat.
    pub fn status_text(&self) -> Result<String, String> {
        let s = self.status()?;
        let mut out = vec![format!("This computer: {} (fingerprint {}); channel {}; work from other members: {}.", s.me.get(..8).unwrap_or(&s.me), s.my_fingerprint, s.listening.map(|p| format!("listening on port {p}")).unwrap_or_else(|| "not listening".into()), s.accept.label())];
        if s.fleets.is_empty() {
            out.push("This computer is in no fleet.".into());
        }
        for f in &s.fleets {
            out.push(format!("Fleet {} ({}), version {}, key generation {}{}:", f.name, f.short, f.version, f.generation, if f.i_am_admin { ", you administer it" } else { "" }));
            for m in &f.members {
                let hb = m.heartbeat.as_ref().map(|h| h.line()).unwrap_or_else(|| "no numbers yet".into());
                let seen = m.seen_secs_ago.map(|s| format!(", seen {s}s ago")).unwrap_or_default();
                out.push(format!("  {} [{}]{}{}{}{} {}: {}{seen}", m.name, m.fingerprint, if m.admin { " administrator" } else { "" }, if m.me { " (this computer)" } else { "" }, if m.trusted { " trusted" } else { "" }, match m.transport.as_str() { "tls" => " over TLS", "tls-left" => " (it left TLS: set its new address)", _ => "" }, m.state, hb));
            }
        }
        for o in &s.offers {
            out.push(format!("A join code for {} is open for {} more seconds (the person sees it on the Fleet page; it is never shown here).", o.short, o.expires_in));
        }
        for r in &s.requests {
            out.push(format!("Join request {}: {} (fingerprint {}) wants to join {}. Approve with fleet_pair approve after the person compared the fingerprint.", &r.id[..8], r.name, r.fingerprint, r.short));
        }
        for p in &s.pending {
            out.push(format!("Our request {}: {}{}", &p.ticket[..8.min(p.ticket.len())], p.state, p.offer.as_ref().map(|o| format!(". {o} Accept with fleet_pair accept.")).unwrap_or_default()));
        }
        for x in s.inbox.iter().rev().take(10) {
            out.push(format!("Work in: {} from {} [{}]: {}", &x.id[..8], x.from_name, x.state, x.body.title));
        }
        for x in s.outbox.iter().rev().take(10) {
            out.push(format!("Work sent: {} to {} [{}]: {}{}", &x.id[..8], x.to_name, x.state, x.title, if x.detail.is_empty() { String::new() } else { format!(" ({})", x.detail) }));
        }
        for r in self.hub.rows_with_members() {
            out.push(format!("Board row {} is with {} [{}].", r.row, r.member, if r.state == "needs_you" { "waiting on their person".to_string() } else { r.state.clone() }));
        }
        for r in self.hub.returned("") {
            out.push(format!("Handed back: {} from {} [{}], head {}, tests: {}", &r.id[..8], r.from_name, r.status, r.head.get(..10).unwrap_or(""), r.tests.lines().next().unwrap_or("")));
        }
        if let Some(a) = self.hub.attached() {
            for f in &s.fleets {
                for c in a.ctx.conflicts(&f.id) {
                    out.push(format!("Context conflict: \"{}\" exists in two versions (from {}); settle it with fleet_work context_resolve (note {}).", c.title, c.from, c.theirs));
                }
            }
        }
        Ok(out.join("\n"))
    }

    fn my_name(a: &Value) -> String {
        let n = text(a, "my_name");
        if n.is_empty() { "IYAKKI computer".into() } else { n.to_string() }
    }

    pub fn create(&self, fleet_name: &str, my_name: &str) -> Result<String, String> {
        self.manage(&format!("create the fleet {}", crate::fleet::clean(fleet_name, 60)))?;
        let id = self.hub.create_fleet(fleet_name, my_name)?;
        self.ensure_node()?;
        Ok(format!("Created the fleet {} ({}). This computer is its administrator. Offer a join code with fleet_pair offer when another computer should join.", crate::fleet::clean(fleet_name, 60), crate::fleet::short_id(&id)))
    }

    pub fn offer(&self, fleet: &str) -> Result<String, String> {
        let id = self.hub.fleet_id(fleet)?;
        self.manage(&format!("offer a join code for {}", crate::fleet::short_id(&id)))?;
        self.ensure_node()?;
        let (code, life) = self.hub.offer_code(&id)?;
        let port = self.hub.listening().map(|a| a.port()).unwrap_or(0);
        Ok(format!("Join code {code} (works once, for {} minutes). The other computer joins with this code and this computer's address and port {port}. When it asks, compare its fingerprint, then approve the request.", life / 60))
    }

    pub fn join(&self, addr: &str, code: &str, my_name: &str) -> Result<String, String> {
        if addr.trim().is_empty() || code.trim().is_empty() {
            return Err("give the other computer's address (host:port) and the join code".into());
        }
        ask(|| self.gate.join(&format!("ask {} to let this computer join", addr.trim())))?;
        self.ensure_node()?;
        let t = self.hub.join_start(addr.trim(), code, my_name)?;
        Ok(format!("Asked {} to let this computer join (request {}). An administrator there must approve it; look at fleet_status until the offer arrives, then accept it.", addr.trim(), &t[..8.min(t.len())]))
    }

    fn pending_ticket(&self, what: &str) -> Result<String, String> {
        self.hub.poll_pending();
        let s = self.hub.status();
        let hits: Vec<_> = s.pending.iter().filter(|p| what.is_empty() || p.ticket.starts_with(what)).collect();
        match hits.len() {
            1 => Ok(hits[0].ticket.clone()),
            0 => Err("there is no such join request of this computer".into()),
            _ => Err("more than one join request: say which".into()),
        }
    }

    pub fn accept_offer(&self, what: &str) -> Result<String, String> {
        let t = self.pending_ticket(what)?;
        let card = self.hub.offer_card(&t)?;
        ask(|| self.gate.join(&card))?;
        let id = self.hub.join_accept(&t)?;
        self.ensure_node()?;
        Ok(format!("Joined the fleet {}.", crate::fleet::short_id(&id)))
    }

    pub fn drop_offer(&self, what: &str) -> Result<String, String> {
        let t = self.pending_ticket(what)?;
        ask(|| self.gate.join("drop a join request"))?;
        self.hub.join_decline(&t)?;
        Ok("Dropped the join request.".into())
    }

    fn request_id(&self, what: &str) -> Result<(String, String), String> {
        let s = self.hub.status();
        let hits: Vec<_> = s.requests.iter().filter(|r| what.is_empty() || r.id.starts_with(what) || r.fingerprint == what).collect();
        match hits.len() {
            1 => Ok((hits[0].id.clone(), format!("{} ({})", hits[0].name, hits[0].fingerprint))),
            0 => Err("there is no such join request waiting".into()),
            _ => Err("more than one join request is waiting: say which".into()),
        }
    }

    pub fn approve(&self, what: &str) -> Result<String, String> {
        let (id, who) = self.request_id(what)?;
        self.manage(&format!("let {who} join"))?;
        let f = self.hub.approve_join(&id)?;
        Ok(format!("Approved {who} for the fleet {}. They must accept on their side.", crate::fleet::short_id(&f)))
    }

    pub fn reject(&self, what: &str) -> Result<String, String> {
        let (id, who) = self.request_id(what)?;
        self.manage(&format!("refuse {who}"))?;
        self.hub.decline_join(&id)?;
        Ok(format!("Refused {who}."))
    }

    pub fn remove(&self, fleet: &str, who: &str) -> Result<String, String> {
        let id = self.hub.fleet_id(fleet)?;
        self.manage(&format!("remove {who} from {} (the group key changes)", crate::fleet::short_id(&id)))?;
        let n = self.hub.remove_member(&id, who)?;
        Ok(format!("Removed {n}. The group key changed; {n} can no longer read or send fleet messages."))
    }

    pub fn admin(&self, fleet: &str, who: &str, on: bool) -> Result<String, String> {
        let id = self.hub.fleet_id(fleet)?;
        self.manage(&format!("make {who} {} an administrator of {}", if on { "" } else { "no longer" }, crate::fleet::short_id(&id)))?;
        let n = self.hub.set_admin(&id, who, on)?;
        Ok(format!("{n} is {} an administrator.", if on { "now" } else { "no longer" }))
    }

    pub fn leave(&self, fleet: &str) -> Result<String, String> {
        let id = self.hub.fleet_id(fleet)?;
        self.manage(&format!("leave the fleet {}", crate::fleet::short_id(&id)))?;
        let n = self.hub.leave(&id)?;
        Ok(format!("Left the fleet {n}."))
    }

    pub fn assign(&self, a: &Value) -> Result<String, String> {
        let id = hex(&random::<16>()?);
        let body = AssignBody {
            id,
            title: text(a, "title").into(),
            goal: text(a, "goal").into(),
            base_branch: text(a, "base_branch").into(),
            base_commit: text(a, "base_commit").into(),
            rows: a.get("rows").and_then(Value::as_array).map(|v| v.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
            context_digest: text(a, "context_digest").into(),
            budget_tokens: a.get("budget_tokens").and_then(Value::as_u64).unwrap_or(0),
            model_class: text(a, "model_class").into(),
            after: a.get("after").and_then(Value::as_array).map(|v| v.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default(),
        };
        body.check()?;
        let fid = self.hub.fleet_id(text(a, "fleet"))?;
        // no member named: the one whose models and load fit (from the heartbeats)
        let chosen;
        let to = if text(a, "to").is_empty() {
            chosen = crate::fleet_work::pick_member(&self.hub.status(), &fid, &crate::fleet_work::Want { vision: text(a, "model_class") == "vision", audio: text(a, "model_class") == "audio", not: vec![] }).ok_or("no member fits right now (none is up with a free place, enough memory and disk and a model for the kind of work): say who, or try later")?.id;
            chosen.as_str()
        } else {
            text(a, "to")
        };
        let who = self.hub.status().fleets.iter().find(|f| f.id == fid).and_then(|f| f.members.iter().find(|m| m.id == to || m.name.eq_ignore_ascii_case(to) || m.fingerprint == to || (to.len() >= 6 && m.id.starts_with(to))).map(|m| format!("{} [{}]", m.name, m.fingerprint))).unwrap_or_else(|| to.to_string());
        ask(|| self.gate.assign(&format!("send one task to {who}")))?;
        let state = self.hub.assign(&fid, to, &body)?;
        Ok(format!("Sent the task {} to {who}: {state}. {}", &body.id[..8], if state == "waiting" { "Their person decides whether to accept." } else { "" }))
    }

    pub fn set_accept(&self, mode: Accept) -> Result<String, String> {
        ask(|| self.gate.receive(&format!("work from other members: {}", mode.label())))?;
        self.hub.set_accept(mode);
        Ok(format!("Work from other members: {}.", mode.label()))
    }

    /// The setting by its word (`ask`, `never`, `members`), for a screen that keeps words.
    pub fn set_accept_word(&self, w: &str) -> Result<String, String> {
        self.set_accept(Accept::parse(w).ok_or("the setting is ask, never or members")?)
    }

    pub fn trust(&self, fleet: &str, who: &str, on: bool) -> Result<String, String> {
        ask(|| self.gate.receive(&format!("{} work from {who} without asking", if on { "accept" } else { "stop accepting" })))?;
        let n = self.hub.trust(fleet, who, on)?;
        Ok(format!("{n} is {} trusted. Takes effect when the setting is \"members\".", if on { "now" } else { "no longer" }))
    }

    pub fn decide(&self, id: &str, accept: bool) -> Result<String, String> {
        ask(|| self.gate.receive(&format!("{} a task from another member", if accept { "accept" } else { "decline" })))?;
        let t = self.hub.inbox_decide(id, accept)?;
        if !accept {
            return Ok(format!("Declined \"{t}\"."));
        }
        match self.hub.attached() {
            Some(a) => {
                let lines = a.worker.tick();
                Ok(format!("Accepted \"{t}\". {}", if lines.is_empty() { "It is queued as a task of the open workspace.".to_string() } else { lines.join(" ") }))
            }
            None => Ok(format!("Accepted \"{t}\". No IDE workspace is open on this computer, so it waits in the inbox as accepted; it starts as a task when a workspace is opened in the IDE.")),
        }
    }
}

/// The tools: wraps another set (the chat chain).
pub struct FleetTools {
    inner: Arc<dyn Tools>,
    api: FleetApi,
}

impl FleetTools {
    pub fn new(inner: Arc<dyn Tools>, api: FleetApi) -> FleetTools {
        FleetTools { inner, api }
    }

    fn pair(&self, a: &Value) -> Result<String, String> {
        match text(a, "action") {
            "create" => self.api.create(text(a, "name"), &FleetApi::my_name(a)),
            "offer" => self.api.offer(text(a, "fleet")),
            "join" => self.api.join(text(a, "address"), text(a, "code"), &FleetApi::my_name(a)),
            "accept" => self.api.accept_offer(text(a, "request")),
            "drop" => self.api.drop_offer(text(a, "request")),
            "approve" => self.api.approve(text(a, "request")),
            "refuse" => self.api.reject(text(a, "request")),
            "remove" => self.api.remove(text(a, "fleet"), text(a, "member")),
            "admin" => self.api.admin(text(a, "fleet"), text(a, "member"), a.get("on").and_then(Value::as_bool).unwrap_or(true)),
            "address" => self.api.address(text(a, "fleet"), text(a, "member"), text(a, "address")),
            "tls" => self.api.use_tls(a.get("on").and_then(Value::as_bool).unwrap_or(true)),
            "interval" => self.api.set_interval(a.get("seconds").and_then(Value::as_u64).unwrap_or(0)),
            _ => Err("action is one of create, offer, join, accept, drop, approve, refuse, remove, admin, address, tls, interval".into()),
        }
    }

    fn receive(&self, a: &Value) -> Result<String, String> {
        match text(a, "action") {
            "accept" => self.api.decide(text(a, "task"), true),
            "decline" => self.api.decide(text(a, "task"), false),
            "mode" => match Accept::parse(text(a, "mode")) {
                Some(m) => self.api.set_accept(m),
                None => Err("mode is ask, never or members".into()),
            },
            "trust" => self.api.trust(text(a, "fleet"), text(a, "member"), a.get("on").and_then(Value::as_bool).unwrap_or(true)),
            _ => Err("action is one of accept, decline, mode, trust".into()),
        }
    }
}

impl Tools for FleetTools {
    fn inner_tools(&self) -> Option<&dyn Tools> {
        Some(&*self.inner)
    }

    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_fleet_tool(&d.spec.name));
        let t = |name: &str, d: &str, schema: Value, risk: Risk| ToolDef { spec: ToolSpec { name: name.into(), description: d.into(), schema }, risk, untrusted: false };
        v.push(t(STATUS, "Show the fleets (pair groups of IYAKKI computers) this computer is in: members, their disk, load, memory and models, join requests, and the work sent and received. Never shows a join code. Reads only.", json!({"type": "object", "properties": {}}), Risk::Safe));
        v.push(t(
            PAIR,
            "Pair computers. action: create (name, my_name), offer (fleet: make a one-time join code the person shows to the other computer), join (address host:port, or tls://host:port#fingerprint for a computer beyond the LAN, code, my_name: ask another computer to let this one join), accept (request: accept the offer that arrived), drop, approve (request: let a computer that asked to join in), refuse, remove (fleet, member: the group key changes), admin (fleet, member, on), address (fleet, member, address: where a member is reached, host:port or tls://host:port#fingerprint), tls (on: make this computer's fleet channel use TLS with the developer-mode certificate; other computers pin the fingerprint), interval (seconds, 10 to 30: how often this computer shares its figures). Always asks the person.",
            json!({"type": "object", "properties": {"action": {"type": "string"}, "name": {"type": "string"}, "my_name": {"type": "string"}, "fleet": {"type": "string"}, "address": {"type": "string"}, "code": {"type": "string"}, "request": {"type": "string"}, "member": {"type": "string"}, "on": {"type": "boolean"}, "seconds": {"type": "integer"}}, "required": ["action"]}),
            Risk::Ask,
        ));
        v.push(t(LEAVE, "Leave a fleet: the other members are told and this computer forgets the fleet. Always asks the person.", json!({"type": "object", "properties": {"fleet": {"type": "string"}}}), Risk::Ask));
        v.push(t(
            ASSIGN,
            "Send one task to another member of a fleet (to: a name or fingerprint; leave it out and the member whose models and load fit is chosen): title, goal, after (ids of tasks that must be applied first), optional base_branch, base_commit, rows, budget_tokens, model_class. No secrets in it. The other computer's person decides whether to accept. Always asks the person.",
            json!({"type": "object", "properties": {"fleet": {"type": "string"}, "to": {"type": "string"}, "title": {"type": "string"}, "goal": {"type": "string"}, "base_branch": {"type": "string"}, "base_commit": {"type": "string"}, "rows": {"type": "array", "items": {"type": "string"}}, "budget_tokens": {"type": "integer"}, "model_class": {"type": "string"}}, "required": ["to", "title", "goal"]}),
            Risk::Ask,
        ));
        v.push(t(
            RECEIVE,
            "Answer work another member sent (action accept or decline, task), choose whether this computer accepts work (action mode: ask, never or members), or trust a member (action trust, fleet, member, on). Always asks the person.",
            json!({"type": "object", "properties": {"action": {"type": "string"}, "task": {"type": "string"}, "mode": {"type": "string"}, "fleet": {"type": "string"}, "member": {"type": "string"}, "on": {"type": "boolean"}}, "required": ["action"]}),
            Risk::Ask,
        ));
        v.push(t(
            WORK,
            "Finish fleet work and share the project Context. action: integrate (fleet, send_followups: apply the bundles the other members handed back to an integration branch in dependency order, run the tests, and make work that did not apply into new tasks), finalize (fleet: put the integration branch into the workspace's own branch), requeue (send the tasks of a silent member to another member that fits), answer (task, text: answer the question a member's task asked, shown as `asks: ...` in fleet_status), auto_requeue_on / auto_requeue_off (do that by itself after each heartbeat round; off until switched on), context_on / context_off (fleet: whether this computer shares the project Context), context_push (fleet: send the notes that changed), context_resolve (fleet, note, keep: mine, theirs, both or merge, which joins the two texts line by line). Always asks the person.",
            json!({"type": "object", "properties": {"action": {"type": "string"}, "fleet": {"type": "string"}, "send_followups": {"type": "boolean"}, "note": {"type": "string"}, "keep": {"type": "string"}, "task": {"type": "string"}, "text": {"type": "string"}}, "required": ["action"]}),
            Risk::Ask,
        ));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            WORK => self.work(args),
            STATUS => self.api.status_text(),
            PAIR => self.pair(args),
            LEAVE => self.api.leave(text(args, "fleet")),
            ASSIGN => self.api.assign(args),
            RECEIVE => self.receive(args),
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            STATUS => "Show the fleets of this computer".into(),
            PAIR => match text(args, "action") {
                "create" => format!("Create the fleet {}", text(args, "name")),
                "offer" => "Make a one-time join code".into(),
                "join" => format!("Ask {} to let this computer join its fleet", text(args, "address")),
                "accept" => "Accept the offer to join a fleet".into(),
                "approve" => "Let a computer join the fleet".into(),
                "remove" => format!("Remove {} from the fleet", text(args, "member")),
                "address" => format!("Set where {} is reached", text(args, "member")),
                "tls" => "Make the fleet channel use TLS (for members beyond the LAN)".into(),
                "interval" => format!("Share this computer's figures every {} seconds", args.get("seconds").and_then(Value::as_u64).unwrap_or(0)),
                a => format!("Fleet: {a}"),
            },
            LEAVE => "Leave the fleet".into(),
            ASSIGN => format!("Send the task \"{}\" to {}", text(args, "title"), text(args, "to")),
            RECEIVE => format!("Fleet work: {}", text(args, "action")),
            WORK => match text(args, "action") {
                "integrate" => "Apply the work other members handed back to an integration branch".into(),
                "finalize" => "Merge the integration branch into your branch".into(),
                a => format!("Fleet: {a}"),
            },
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            ASSIGN => Some(format!("To: {}\nTitle: {}\nBase: {} {}\nBudget: {} tokens\nGoal:\n{}\n\nNo secret is sent; the other computer's own approvals and limits apply.", text(args, "to"), text(args, "title"), text(args, "base_branch"), text(args, "base_commit"), args.get("budget_tokens").and_then(Value::as_u64).unwrap_or(0), text(args, "goal"))),
            PAIR if text(args, "action") == "accept" => self.api.pending_ticket(text(args, "request")).ok().and_then(|t| self.api.hub.offer_card(&t).ok()),
            PAIR if text(args, "action") == "approve" => self.api.request_id(text(args, "request")).ok().map(|(_, w)| format!("{w} asked to join. Approve only if the fingerprint is the one the other computer shows.")),
            _ if is_fleet_tool(name) => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name == ASSIGN {
            if let Err(e) = (AssignBody { id: "0".repeat(32), title: text(args, "title").into(), goal: text(args, "goal").into(), base_branch: text(args, "base_branch").into(), base_commit: text(args, "base_commit").into(), rows: vec![], context_digest: String::new(), budget_tokens: args.get("budget_tokens").and_then(Value::as_u64).unwrap_or(0), model_class: text(args, "model_class").into(), after: vec![] }).check() {
                return Some(e);
            }
        }
        if is_fleet_tool(name) {
            return None;
        }
        self.inner.refusal(name, args)
    }
}

/// Send the tasks of silent members to other members that fit. `first_ask`: the core is asked once for the batch (the person's call); without
/// it (the automatic requeue the person switched on) the moves are written to the access log instead.
pub fn requeue_with(hub: &Arc<Hub>, gate: &dyn Fleet, first_ask: bool) -> Result<String, String> {
    let lost = hub.requeue_lost();
    if lost.is_empty() {
        return Ok("No member holding work is silent.".into());
    }
    if first_ask {
        ask(|| gate.assign(&format!("send {} task(s) of a silent member to other members", lost.len())))?;
    } else {
        gate.record("fleet.requeue.auto", &format!("tasks={}", lost.len()));
    }
    let mut out = vec![];
    for o in lost {
        let Some(mut body) = o.body.clone() else { continue };
        let st = hub.status();
        let want = crate::fleet_work::Want { not: vec![o.to.clone()], ..Default::default() };
        match crate::fleet_work::pick_member(&st, &o.fleet, &want) {
            Some(m) => {
                body.id = hex(&random::<16>()?);
                match hub.assign(&o.fleet, &m.id, &body) {
                    Ok(state) => out.push(format!("\"{}\" went from {} (silent) to {}: {state}.", o.title, o.to_name, m.name)),
                    Err(e) => out.push(format!("\"{}\": could not send to {}: {e}", o.title, m.name)),
                }
            }
            None => out.push(format!("\"{}\" was with {} (silent) and no other member fits; it is marked requeued.", o.title, o.to_name)),
        }
    }
    Ok(out.join("\n"))
}

/// One line per other member of the person's fleets for the IDE Board's member strip: its name, whether it is up, its figures and the Board
/// rows that are with it. Read only; nothing is sent. Empty when this computer is in no fleet.
pub fn member_chips(hub: &Hub) -> Vec<(String, bool)> {
    let st = hub.status();
    let rows = hub.rows_with_members();
    let mut out = vec![];
    for f in &st.fleets {
        for m in f.members.iter().filter(|m| !m.me) {
            let up = m.state == "up";
            let mut label = format!("{} · {}", m.name, m.state);
            if let Some(h) = &m.heartbeat {
                label.push_str(&format!(" · load {}% · memory {}% · {} task{}", h.cpu_load_pct, h.mem_used_pct, h.tasks_running, if h.tasks_running == 1 { "" } else { "s" }));
            }
            let mine: Vec<&str> = rows.iter().filter(|r| r.member == m.name).map(|r| r.row.as_str()).collect();
            if !mine.is_empty() {
                label.push_str(&format!(" · {}", mine.join(", ")));
            }
            out.push((label, up));
        }
    }
    out
}

/// What the screens and the chat need from the agent's services without naming agent types (the UI reaches the agent through the services).
impl crate::services::Services {
    /// The fleet door for a person: the hub of this data folder, the core's `Fleet` handle and the heartbeat source over the core API.
    pub fn fleet_api(self: &Arc<Self>, os: Arc<dyn iyakki_core_api::Os>, gate: Arc<dyn Fleet>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Result<FleetApi, String> {
        let hub = Hub::open(self.dir())?;
        let svc = self.clone();
        let models = move || {
            let s = svc.settings.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            s.get().providers.iter().filter(|p| p.enabled).flat_map(|p| p.models.iter().chain(p.default_model.iter()).cloned().collect::<Vec<_>>()).collect::<Vec<String>>()
        };
        let source: Arc<dyn HeartbeatSource> = Arc::new(crate::fleet::OsSource { os, data_dir: self.dir().to_path_buf(), tasks: Arc::new(|| 0), models: Arc::new(models) });
        Ok(FleetApi::new(hub, gate, source, audit).with_dir(self.dir()))
    }

    /// The copies of the other members' notes that wait to be settled (the ids of the copy notes), over every fleet this computer shares the
    /// project Context with. Empty when no workspace is open or nothing waits.
    pub fn fleet_context_conflicts(self: &Arc<Self>) -> Vec<String> {
        let Ok(h) = Hub::open(self.dir()) else { return vec![] };
        let Some(a) = h.attached() else { return vec![] };
        h.status().fleets.iter().flat_map(|f| a.ctx.conflicts(&f.id)).map(|c| c.theirs).collect()
    }

    /// Settle one conflict for the person at the Context tab: `keep` is mine, theirs, both or merge. The person's own press on the note's
    /// buttons is the permission (the same call the chat tool makes after its card).
    pub fn fleet_context_settle(self: &Arc<Self>, copy_id: &str, keep: &str) -> Result<String, String> {
        let h = Hub::open(self.dir())?;
        let a = h.attached().ok_or("no project is open on this computer")?;
        let fleets: Vec<String> = h.status().fleets.iter().map(|f| f.id.clone()).collect();
        for f in fleets {
            if a.ctx.conflicts(&f).iter().any(|c| c.theirs == copy_id) {
                let r = a.ctx.resolve(&f, copy_id, keep)?;
                h.audit_line("fleet.context.settle", &format!("fleet={} how={keep}", crate::fleet::short_id(&f)));
                return Ok(r);
            }
        }
        Err("no such conflict".into())
    }

    /// The member strip of the IDE Board (see [`member_chips`]).
    pub fn fleet_member_chips(self: &Arc<Self>) -> Vec<(String, bool)> {
        match Hub::open(self.dir()) {
            Ok(h) => member_chips(&h),
            Err(_) => vec![],
        }
    }

    /// Put the fleet tools on top of `inner` (the chat chain).
    pub fn wrap_fleet_tools(self: &Arc<Self>, inner: Arc<dyn Tools>, os: Arc<dyn iyakki_core_api::Os>, gate: Arc<dyn Fleet>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn Tools> {
        match self.fleet_api(os, gate, audit) {
            Ok(api) => {
                api.resume();
                Arc::new(FleetTools::new(inner, api))
            }
            Err(_) => inner,
        }
    }

    /// The IDE opened a workspace: work other members send runs as a task of its task manager, and its Context can be shared. The last
    /// workspace opened is the one that takes the work.
    pub fn fleet_attach_workspace(self: &Arc<Self>, os: Arc<dyn iyakki_core_api::Os>, gate: Arc<dyn Fleet>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, mgr: Arc<dyn iyakki_agent_api::TaskMgr>, root: &std::path::Path, test: &str) {
        let Ok(mgr) = mgr.agent_manager_any().downcast::<crate::tasks::Manager>() else { return };
        if let Ok(api) = self.fleet_api(os, gate, audit) {
            api.attach(mgr, root, test, self.dir());
            if let Some(a) = api.hub.attached() {
                let svc = self.clone();
                a.worker.set_models(Arc::new(move || {
                    let providers = svc.settings.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get().providers.clone();
                    crate::router_registry::build(&providers, &Default::default())
                }));
            }
            api.resume();
        }
    }

    /// The cameras of this computer as the router's eyes (F-ROUTER-03): each photo goes through the camera hub, whose broker asks as usual.
    pub fn router_camera_senses(&self, hub: Arc<crate::cameras::Hub>, who: crate::cameras::Who) -> Arc<dyn crate::router::Senses> {
        Arc::new(crate::router::CameraSenses { hub, who })
    }

    /// The fleets as a place router steps may go (F-ROUTER-08, F-ROUTER-15): any fleet this computer is in. A step waits at most two minutes for
    /// the member's answer.
    pub fn router_fleet_runner(self: &Arc<Self>, os: Arc<dyn iyakki_core_api::Os>, gate: Arc<dyn Fleet>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Option<Arc<dyn crate::router::FleetRunner>> {
        let api = self.fleet_api(os, gate.clone(), audit).ok()?;
        // every fleet of this computer: with one, that one; with several, the member that fits best across them (the person's zone and the
        // core's `fleet.assign` card still apply to each step)
        if api.hub.status().fleets.is_empty() {
            return None;
        }
        Some(Arc::new(crate::fleet_work::FleetDispatch::new(api.hub.clone(), gate, "", std::time::Duration::from_secs(120))))
    }

    /// The check the IDE gives the router for a project (F-ROUTER-12): steps whose check says `tests` run the project's test command; any
    /// other step is accepted on the executor's claim as before.
    pub fn router_project_verifier(self: &Arc<Self>, root: &std::path::Path) -> Arc<dyn crate::router::Verifier> {
        match crate::router::ProjectTestCheck::for_project(root) {
            Some(c) => Arc::new(crate::router::CheckedVerifier(vec![Arc::new(c)])),
            None => Arc::new(crate::router::AcceptClaim),
        }
    }

    /// What a setting word means in plain words (for a card).
    pub fn fleet_accept_label(&self, w: &str) -> String {
        Accept::parse(w).map(|a| a.label().to_string()).unwrap_or_else(|| w.to_string())
    }

    /// The line of the chat's system prompt about fleets.
    pub fn fleet_prompt_rule(&self) -> &'static str {
        PROMPT_RULE
    }

    /// Keep the press that approves a fleet tool for the thread that carries the call out (the same ticket the router uses).
    pub fn note_fleet_press(&self, tool: &str) {
        if is_fleet_tool(tool) {
            crate::routertools::note_press();
        }
    }
}

/// A fingerprint, for the screens that show a member.
pub fn fp(id: &str) -> String {
    fingerprint(id)
}

impl crate::services::Services {
    /// Run `f` with the fleet door of this person. The screens use this so they do not have to name the agent's types.
    pub fn with_fleet<R>(self: &Arc<Self>, os: Arc<dyn iyakki_core_api::Os>, gate: Arc<dyn Fleet>, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, f: impl FnOnce(&FleetApi) -> R) -> Result<R, String> {
        self.fleet_api(os, gate, audit).map(|api| f(&api))
    }
}

impl FleetApi {
    /// What a screen shows: the same as [`FleetApi::status`] but without asking the network (the channel's own thread keeps requests current).
    pub fn view(&self) -> Result<Status, String> {
        self.gate.read("fleet members and numbers")?;
        Ok(self.hub.status())
    }
}

// ------------------------------------------------------------------------------------------------ work: hand-back, integration, context

impl FleetApi {
    /// Make the IDE workspace `root` (its task manager `mgr`) the place work from other members runs and the workspace whose Context is shared.
    pub fn attach(&self, mgr: Arc<crate::tasks::Manager>, root: &std::path::Path, test: &str, data_dir: &std::path::Path) {
        crate::fleet_work::attach(&self.hub, mgr, root, test, &data_dir.join("fleet").join("context"));
    }

    fn attached(&self) -> Result<Arc<crate::fleet_work::Attached>, String> {
        self.hub.attached().ok_or_else(|| "no IDE workspace is open on this computer: open the project in the IDE first".to_string())
    }

    /// Apply what the workers handed back to an integration branch in dependency order and run the workspace's tests. Asks the core.
    pub fn integrate(&self, fleet: &str, send_followups: bool) -> Result<String, String> {
        let a = self.attached()?;
        let fid = self.hub.fleet_id(fleet)?;
        let items: Vec<(crate::fleet_hub::Returned, std::path::PathBuf)> = self.hub.returned(&fid).into_iter().filter(|r| r.bundle && r.status == "done").filter_map(|r| self.hub.bundle_path(&r.id).ok().map(|p| (r, p))).collect();
        if items.is_empty() {
            return Err("nothing has been handed back with a bundle yet".into());
        }
        ask(|| self.gate.receive(&format!("apply {} returned task(s) to an integration branch of the open workspace and run its tests", items.len())))?;
        let test = a.test.as_ref().map(|(p, args)| (p.as_str(), args.as_slice()));
        let r = crate::fleet_integrate::integrate(&a.root, &crate::fleet::short_id(&fid).replace('-', ""), &items, test)?;
        self.hub.audit_line("fleet.integrate", &format!("fleet={} applied={} followups={}", crate::fleet::short_id(&fid), r.applied.len(), r.followups.len()));
        let mut out = vec![format!("Integration branch {}: {} applied ({}), {} skipped, {} to do again. Head {}. {}", r.branch, r.applied.len(), r.applied.iter().map(|i| &i[..8]).collect::<Vec<_>>().join(", "), r.skipped.len(), r.followups.len(), r.head.get(..10).unwrap_or(""), r.tests.lines().next().unwrap_or(""))];
        for f in &r.followups {
            out.push(format!("Not applied: task {}: {}. New task: \"{}\" ({}).", &f.task[..8], f.reason, f.next.title, &f.next.id[..8]));
        }
        if send_followups {
            let st = self.hub.status();
            for f in &r.followups {
                let who = crate::fleet_work::pick_member(&st, &fid, &crate::fleet_work::Want::default());
                match who {
                    Some(m) => match ask(|| self.gate.assign(&format!("send the follow-up \"{}\" to {} (with the commits of {} it starts from)", f.next.title, m.name, f.next.base_branch))).and_then(|_| self.ship_base(&a.root, &fid, &m.id, &f.next)).and_then(|_| self.hub.assign(&fid, &m.id, &f.next)) {
                        Ok(state) => out.push(format!("Sent the follow-up {} to {}: {state}.", &f.next.id[..8], m.name)),
                        Err(e) => out.push(format!("Could not send the follow-up {}: {e}", &f.next.id[..8])),
                    },
                    None => out.push(format!("No member fits the follow-up {} right now.", &f.next.id[..8])),
                }
            }
        }
        out.push("Nothing is in your own branch yet: finalize puts the integration branch in it, after you say so.".into());
        Ok(out.join("\n"))
    }

    /// Send a worker the commits a follow-up starts from: a bundle of the integration branch (asked once with the follow-up's card).
    fn ship_base(&self, root: &std::path::Path, fleet: &str, to: &str, next: &crate::fleet_hub::AssignBody) -> Result<(), String> {
        if next.base_commit.is_empty() || next.base_branch.is_empty() {
            return Ok(());
        }
        let file = root.join(".iyakki").join(format!("fleet-base-{}.bundle", &next.base_commit[..12.min(next.base_commit.len())]));
        std::fs::create_dir_all(file.parent().unwrap_or(root)).map_err(|e| e.to_string())?;
        let f = file.to_string_lossy().into_owned();
        let (ok, _, err) = crate::tasks::git_run(root, &["bundle", "create", &f, &format!("refs/heads/{}", next.base_branch)]);
        if !ok {
            return Err(format!("could not make the bundle of {}: {}", next.base_branch, err.trim()));
        }
        let r = self.hub.send_base(fleet, to, &next.base_commit, &file);
        let _ = std::fs::remove_file(&file);
        r
    }

    /// Answer the question a member's task asked: `task` is the task's id (or its first characters, from `fleet_status`). Asks the core.
    pub fn answer_task(&self, task: &str, text: &str) -> Result<String, String> {
        if text.trim().is_empty() {
            return Err("give the answer in `text`".into());
        }
        let st = self.hub.status();
        let o = st.outbox.iter().find(|o| !task.is_empty() && o.id.starts_with(task)).ok_or("no task out with a member starts with that")?.clone();
        ask(|| self.gate.assign(&format!("answer the question of the task \"{}\" for {}", o.title, o.to_name)))?;
        self.hub.send_answer(&o.fleet, &o.to, &o.id, text)?;
        Ok(format!("Sent your answer to {}; it goes to the task as guidance.", o.to_name))
    }

    /// Put the integration branch into the person's own branch. Asks the core.
    pub fn finalize(&self, fleet: &str) -> Result<String, String> {
        let a = self.attached()?;
        let fid = self.hub.fleet_id(fleet)?;
        ask(|| self.gate.receive("merge the integration branch into the open workspace's branch"))?;
        let r = crate::fleet_integrate::finalize(&a.root, &crate::fleet::short_id(&fid).replace('-', ""))?;
        self.hub.audit_line("fleet.finalize", &format!("fleet={}", crate::fleet::short_id(&fid)));
        Ok(r)
    }

    /// Send the tasks of silent members to other members that fit. Asks the core once for the batch.
    pub fn requeue_lost(&self) -> Result<String, String> {
        requeue_with(&self.hub, self.gate.as_ref(), true)
    }

    /// The person allows (or stops) the automatic requeue: after each heartbeat round the tasks of a silent member go to a member that fits,
    /// without a card each time (the person's yes here is the standing permission; every move is written to the access log). Asks the core.
    pub fn set_auto_requeue(&self, on: bool) -> Result<String, String> {
        ask(|| self.gate.assign(&format!("{} sending the tasks of a silent member to other members by itself", if on { "start" } else { "stop" })))?;
        self.hub.set_auto_requeue(on);
        Ok(if on { "Automatic requeue is on: the tasks of a silent member go to another member that fits.".into() } else { "Automatic requeue is off.".into() })
    }

    /// `on`/`off`: whether this computer shares the project Context of a fleet (the person's setting; asks the core).
    pub fn context_share(&self, fleet: &str, on: bool) -> Result<String, String> {
        self.attached()?;
        ask(|| self.gate.receive(&format!("{} the project Context shared with the fleet", if on { "start taking part in" } else { "stop taking part in" })))?;
        let fid = self.hub.set_context_sync(fleet, on)?;
        Ok(format!("The project Context is {} for the fleet {}.", if on { "shared" } else { "no longer shared" }, crate::fleet::short_id(&fid)))
    }

    /// Send the notes that changed since the last send to every other member. Asks the core.
    pub fn context_push(&self, fleet: &str) -> Result<String, String> {
        let a = self.attached()?;
        let fid = self.hub.fleet_id(fleet)?;
        if !self.hub.context_sync_on(&fid) {
            return Err("this computer does not share the project Context with that fleet yet (context_on)".into());
        }
        let (msg, pairs) = a.ctx.outgoing(&fid);
        if pairs.is_empty() {
            return Ok("Nothing changed in the project Context since it was last sent.".into());
        }
        ask(|| self.gate.assign(&format!("send {} changed note(s) of the project Context to the other members", pairs.len())))?;
        let results = self.hub.broadcast(&fid, "context", &msg)?;
        let ok = results.iter().all(|r| r.1.is_ok());
        let lines: Vec<String> = results.iter().map(|(n, r)| format!("{n}: {}", r.as_ref().map(|_| "taken".to_string()).unwrap_or_else(|e| e.clone()))).collect();
        if ok {
            a.ctx.sent(&fid, &pairs)?;
        }
        Ok(format!("Sent {} note(s). {}{}", pairs.len(), lines.join("; "), if ok { "" } else { " (not marked as sent: some member did not take it, it is sent again next time)" }))
    }

    pub fn context_resolve(&self, fleet: &str, note: &str, keep: &str) -> Result<String, String> {
        let a = self.attached()?;
        let fid = self.hub.fleet_id(fleet)?;
        ask(|| self.gate.receive("settle a conflict in the shared project Context"))?;
        a.ctx.resolve(&fid, note, keep)
    }
}

impl FleetTools {
    fn work(&self, a: &Value) -> Result<String, String> {
        let fleet = text(a, "fleet");
        match text(a, "action") {
            "integrate" => self.api.integrate(fleet, a.get("send_followups").and_then(Value::as_bool).unwrap_or(false)),
            "finalize" => self.api.finalize(fleet),
            "requeue" => self.api.requeue_lost(),
            "answer" => self.api.answer_task(text(a, "task"), text(a, "text")),
            "auto_requeue_on" => self.api.set_auto_requeue(true),
            "auto_requeue_off" => self.api.set_auto_requeue(false),
            "context_on" => self.api.context_share(fleet, true),
            "context_off" => self.api.context_share(fleet, false),
            "context_push" => self.api.context_push(fleet),
            "context_resolve" => self.api.context_resolve(fleet, text(a, "note"), text(a, "keep")),
            _ => Err("action is one of integrate, finalize, requeue, answer, auto_requeue_on, auto_requeue_off, context_on, context_off, context_push, context_resolve".into()),
        }
    }
}
