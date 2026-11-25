//! The acting person of a UI connection and the one-time approval ticket (D-SEC3-BIND, D-SEC3-TICKET; closes D-SEC1-SHARED and D-SEC1-APPROVED).
//!
//! * **Binding.** The core records, per connection, which account signed in over it: a successful `AccountsApi::authenticate` (a password the core
//!   checked itself), `AccountsApi::resume_session`, or `ServicesHandle::on_login` / `on_resume` while nobody is bound. From then on a served call
//!   that names an account must name THAT account (the rules of `served_auth::RULES` carry the index of the name); a call naming another account
//!   is refused unless the bound account is an administrator and the call is on [`ADMIN_ACTS`]. The accounts calls that take a `Session` must carry
//!   the bound account's own session. The shared-scope flags of the skill and package calls come from the bound account's policy. The binding ends
//!   with `on_logout` of that account or when the connection closes.
//! * **Ticket.** The platform calls carry `approved: bool`, the card the person tapped. The UI asks the core for a ticket (`Approval::issue`) that
//!   names the call, the bound account, a hash of the arguments and an expiry on the host's clock, and sends it as one extra trailing argument. The
//!   core removes it, checks it and consumes it: one use, one call, one account, one argument set, [`TICKET_TTL_MS`]. An `approved` call without a
//!   valid ticket is refused, never run as "not approved". Every issue, use and refusal is audited.
//!
//! The ticket does not make the UI trustworthy (a compromised UI program can ask for a ticket); it makes an approval a separate, audited, bounded
//! act of a known account instead of a flag, and it stops a replay, a swap of arguments, a cross-account use and a stale approval.

use std::collections::HashMap;
use std::sync::{Arc, PoisonError};

use serde_json::{json, Value};

use crate::served::NOT_SERVED;
use crate::served_auth::RULES;
use crate::{Host, Registry};

/// How long a ticket lives.
pub const TICKET_TTL_MS: u64 = 60_000;
/// The most tickets the core keeps open for one host (the oldest goes first).
const MAX_TICKETS: usize = 64;

/// The clock of the host in milliseconds (a manual one in tests).
pub type TicketClock = Arc<dyn Fn() -> u64 + Send + Sync>;

/// A served call that takes an approval: the trait, the method, the index of `approved` and how many arguments travel on the wire.
pub(crate) struct Ticketed {
    pub iface: &'static str,
    pub method: &'static str,
    pub approved: usize,
    pub wire_args: usize,
}

const fn t(iface: &'static str, method: &'static str, approved: usize, wire_args: usize) -> Ticketed {
    Ticketed { iface, method, approved, wire_args }
}

/// Every call whose `approved` is a ticketed approval (the audit test checks each is no longer an `Open` entry).
pub(crate) const TICKETED: &[Ticketed] = &[
    t("PlatformApi", "apply", 1, 2),
    t("PlatformApi", "apply_with", 1, 2),
    t("PlatformApi", "power", 2, 3),
    t("PlatformApi", "upgrade", 1, 2),
    t("PlatformApi", "backup", 2, 3),
    t("PlatformApi", "remove", 2, 3),
    t("PlatformApi", "connect", 1, 2),
    t("PlatformApi", "push", 1, 2),
    t("PlatformApi", "create_pull_request", 1, 2),
    t("PlatformApi", "connect_existing", 4, 5),
    t("PackageApi", "install", 1, 4),
];

pub(crate) fn ticketed(iface: &str, method: &str) -> Option<&'static Ticketed> {
    TICKETED.iter().find(|x| x.iface == iface && x.method == method)
}

/// Calls an administrator may make about ANOTHER account (everything else must name the bound account).
pub(crate) const ADMIN_ACTS: &[(&str, &str)] = &[("ServicesHandle", "on_logout")];

struct Ticket {
    token: String,
    conn: u64,
    account: String,
    iface: String,
    method: String,
    hash: String,
    expires: u64,
}

/// The state of the binding and the tickets of one host.
#[derive(Default)]
pub(crate) struct Login {
    enforce: bool,
    acting: HashMap<u64, String>,
    tickets: Vec<Ticket>,
    clock: Option<TicketClock>,
}

impl Login {
    fn now(&self) -> u64 {
        match &self.clock {
            Some(c) => c(),
            None => std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64),
        }
    }
}

impl Host {
    fn login(&self) -> std::sync::MutexGuard<'_, Login> {
        self.login.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Bind the acting person to every connection: a call that names another account, a shared-scope flag the account does not hold and an
    /// `approved` flag without a core-issued ticket are refused. Off by default so a host built for a test of one object stays simple; the core
    /// turns it on for the UI socket.
    pub fn with_login_binding(self) -> Host {
        self.login().enforce = true;
        self
    }

    /// The clock that dates tickets (milliseconds); the system clock when unset.
    pub fn with_ticket_clock(self, clock: TicketClock) -> Host {
        self.login().clock = Some(clock);
        self
    }

    /// The account bound to connection `conn`, if any (for tests and the audit line).
    pub fn acting(&self, conn: u64) -> Option<String> {
        self.login().acting.get(&conn).cloned()
    }

    /// The number of tickets the core holds open (for tests).
    pub fn open_tickets(&self) -> usize {
        self.login().tickets.len()
    }

    /// A connection closed: its binding and its tickets go.
    pub(crate) fn forget_login(&self, conn: u64) {
        let mut l = self.login();
        l.acting.remove(&conn);
        l.tickets.retain(|x| x.conn != conn);
    }
}

fn audit(reg: &Registry<'_>, what: &str, detail: &str) {
    if let Some(f) = reg.side().audit {
        f(what, detail);
    }
}

/// The arguments in one canonical text (object keys sorted), so a hash does not depend on the order a map was built in.
fn canon(v: &Value, out: &mut String) {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for k in keys {
                out.push_str(&Value::String(k.clone()).to_string());
                out.push(':');
                canon(&m[k], out);
                out.push(',');
            }
            out.push('}');
        }
        Value::Array(a) => {
            out.push('[');
            for x in a {
                canon(x, out);
                out.push(',');
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// The hash a ticket binds: the call and its arguments, the `approved` slot read as `true` and any ticket argument left out.
pub(crate) fn args_hash(tk: &Ticketed, args: &Value) -> String {
    let mut a: Vec<Value> = args.as_array().cloned().unwrap_or_default();
    a.truncate(tk.wire_args);
    if let Some(s) = a.get_mut(tk.approved) {
        *s = Value::Bool(true);
    }
    let mut text = format!("{}::{}|", tk.iface, tk.method);
    canon(&Value::Array(a), &mut text);
    iyakki_dash::s3::sha256_hex(text.as_bytes())
}

fn token() -> Result<String, String> {
    let mut b = [0u8; 16];
    iyakki_sec::random_fill(&mut b).map_err(|e| format!("the core has no random numbers for a ticket: {e:?}"))?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

fn refuse(m: &str) -> String {
    format!("{NOT_SERVED}: {m}")
}

/// `Approval::issue(iface, method, args)`: the person approved on the card, the core issues a ticket for exactly this call.
pub(crate) fn issue(reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    let (iface, method, args): (String, String, Value) = crate::de(a)?;
    let tk = ticketed(&iface, &method).ok_or_else(|| refuse(&format!("{iface}::{method} takes no approval ticket")))?;
    let mut l = reg.host.login();
    let account = match l.acting.get(&reg.conn) {
        Some(n) => n.clone(),
        None if l.enforce => return Err(refuse("nobody is signed in on this connection, so there is no one to approve")),
        None => String::new(),
    };
    let (now, tok) = (l.now(), token()?);
    l.tickets.retain(|x| x.expires > now);
    if l.tickets.len() >= MAX_TICKETS {
        l.tickets.remove(0);
    }
    let hash = args_hash(tk, &args);
    l.tickets.push(Ticket { token: tok.clone(), conn: reg.conn, account: account.clone(), iface, method: method.clone(), hash: hash.clone(), expires: now + TICKET_TTL_MS });
    drop(l);
    audit(reg, "ui.approval.issued", &format!("{account} {method} {}", &hash[..12]));
    Ok(json!(tok))
}

/// Check and consume the ticket of a call.
fn redeem(reg: &Registry<'_>, tk: &Ticketed, who: &str, tok: Option<&str>, args: &Value) -> Result<(), String> {
    let deny = |why: &str| {
        audit(reg, "ui.approval.refused", &format!("{who} {} {why}", tk.method));
        Err(refuse(&format!("{} needs the approval of the person and the core holds no valid ticket for it ({why})", tk.method)))
    };
    let Some(tok) = tok else { return deny("no ticket") };
    let hash = args_hash(tk, args);
    let mut l = reg.host.login();
    let now = l.now();
    let Some(pos) = l.tickets.iter().position(|x| x.token == tok) else { return deny("unknown or already used") };
    let x = l.tickets.remove(pos); // single use, even when it does not fit
    let why = if x.expires <= now {
        Some("expired")
    } else if x.conn != reg.conn || x.account != who {
        Some("issued to another person or connection")
    } else if x.iface != tk.iface || x.method != tk.method {
        Some("issued for another call")
    } else if x.hash != hash {
        Some("issued for other arguments")
    } else {
        None
    };
    drop(l);
    match why {
        Some(w) => deny(w),
        None => {
            audit(reg, "ui.approval.used", &format!("{who} {} {}", tk.method, &hash[..12]));
            Ok(())
        }
    }
}

fn name_at(a: &Value, i: usize) -> Result<String, String> {
    a.get(i).and_then(Value::as_str).map(str::to_string).ok_or_else(|| refuse("the call names no account"))
}

fn bind(reg: &Registry<'_>, name: &str, how: &str) {
    reg.host.login().acting.insert(reg.conn, name.to_string());
    audit(reg, "ui.login.bound", &format!("{name} {how}"));
}

/// Before the call runs: take off the trailing ticket and, when the host binds the acting person, hold the call to it.
pub(crate) fn gate(reg: &Registry<'_>, iface: &str, method: &str, a: &mut Value) -> Result<(), String> {
    // the ticket argument is never read by anything but this step
    let mut tok: Option<String> = None;
    if let Some(tk) = ticketed(iface, method) {
        if let Some(arr) = a.as_array_mut() {
            if arr.len() == tk.wire_args + 1 {
                tok = arr.pop().and_then(|v| v.as_str().map(str::to_string));
            }
        }
    }
    // what the person approved is what the UI sent, before the core writes its own reading over the claims
    let sent = a.clone();
    let (enforce, acting) = {
        let l = reg.host.login();
        (l.enforce, l.acting.get(&reg.conn).cloned())
    };
    if !enforce {
        return Ok(());
    }
    if iface == "AccountsApi" && method == "resume_session" {
        return resume_allowed(reg, a);
    }
    // signing in
    if iface == "ServicesHandle" && (method == "on_login" || method == "on_resume") {
        let name = name_at(a, 0)?;
        return match acting {
            Some(me) if me == name => Ok(()),
            Some(me) => Err(refuse(&format!("{me} is signed in on this connection; sign out before signing in as {name}"))),
            None => {
                let accounts = reg.side().accounts.ok_or_else(|| Registry::missing("accounts"))?;
                let ok = if method == "on_login" { accounts.authenticate(&name, a.get(1).and_then(Value::as_str).unwrap_or("")).is_ok() } else { accounts.resume_session(&name).is_some() };
                if !ok {
                    return Err(refuse(&format!("the core could not sign {name} in")));
                }
                bind(reg, &name, method);
                Ok(())
            }
        };
    }
    let rule = RULES.iter().find(|r| r.iface == iface && r.method == method);
    if let Some(r) = rule {
        if let Some(i) = r.user {
            let name = name_at(a, i)?;
            let me = acting.clone().ok_or_else(|| refuse("nobody is signed in on this connection"))?;
            if name != me && !(ADMIN_ACTS.contains(&(iface, method)) && reg.admin_of(&me)?) {
                audit(reg, "ui.login.refused", &format!("{me} named {name} in {method}"));
                return Err(refuse(&format!("{me} is signed in on this connection and may not act as {name}")));
            }
        }
        for c in r.claims {
            match c {
                crate::served_auth::Claim::Session(i) => {
                    let who = a.get(*i).and_then(|s| s.get("user")).and_then(Value::as_str).map(str::to_string).ok_or_else(|| refuse("the session names no account"))?;
                    let me = acting.clone().ok_or_else(|| refuse("nobody is signed in on this connection"))?;
                    if who != me {
                        audit(reg, "ui.login.refused", &format!("{me} sent the session of {who} in {method}"));
                        return Err(refuse(&format!("{me} is signed in on this connection and may not act as {who}")));
                    }
                }
                crate::served_auth::Claim::ScopeFlags { scope, field, shared, own } => {
                    let me = acting.clone().ok_or_else(|| refuse("nobody is signed in on this connection"))?;
                    let s = a.get(*scope).ok_or_else(|| refuse("the call has no scope"))?;
                    let s = match field {
                        Some(f) => s.get(*f).cloned().unwrap_or(Value::Null),
                        None => s.clone(),
                    };
                    let p = reg.policy_of(&me)?;
                    match s.get("User").and_then(Value::as_str) {
                        Some(owner) if owner != me => return Err(refuse(&format!("{me} is signed in on this connection and may not change the skills of {owner}"))),
                        Some(_) => {} // the personal scope: vouch writes the account's own flags
                        None => {
                            // the shared scope: the right is the bound account's
                            let arr = a.as_array_mut().ok_or_else(|| refuse("the arguments are not a list"))?;
                            arr[*shared] = json!(p.manage_skills);
                            arr[*own] = json!(p.skills);
                        }
                    }
                }
                _ => {}
            }
        }
    }
    if let Some(tk) = ticketed(iface, method) {
        if a.get(tk.approved).and_then(Value::as_bool) == Some(true) {
            let me = acting.ok_or_else(|| refuse("nobody is signed in on this connection"))?;
            redeem(reg, tk, &me, tok.as_deref(), &sent)?;
        }
    }
    Ok(())
}

/// After the call ran: a verified sign-in binds the connection, a sign-out frees it.
pub(crate) fn settle(reg: &Registry<'_>, iface: &str, method: &str, a: &Value, answer: &Value) {
    match (iface, method) {
        ("AccountsApi", "authenticate") => {
            if let Some(u) = answer.get("Ok").and_then(|s| s.get("user")).and_then(Value::as_str) {
                bind(reg, u, "password");
            }
        }
        ("AccountsApi", "resume_session") => {
            if let Some(u) = answer.get("user").and_then(Value::as_str) {
                bind(reg, u, "resume");
            }
        }
        ("ServicesHandle", "on_logout") => {
            let mut l = reg.host.login();
            if l.acting.get(&reg.conn).map(String::as_str) == a.get(0).and_then(Value::as_str) {
                l.acting.remove(&reg.conn);
            }
        }
        _ => {}
    }
}

/// `resume_session` binds without a password (the documented trust-base limit), so it may not move a connection from one person to another.
pub(crate) fn resume_allowed(reg: &Registry<'_>, a: &Value) -> Result<(), String> {
    let (enforce, acting) = {
        let l = reg.host.login();
        (l.enforce, l.acting.get(&reg.conn).cloned())
    };
    match (enforce, acting, a.get(0).and_then(Value::as_str)) {
        (true, Some(me), Some(n)) if me != n => Err(refuse(&format!("{me} is signed in on this connection; sign out before resuming the session of {n}"))),
        _ => Ok(()),
    }
}
