//! The lead assistant's tools for the SFTP server (`sftp.rs`, design in `docs/development/SFTP-SERVER.md`): `sftp_status` reads (no card);
//! `sftp_set` changes anything (turn it on or off, who may log in, public keys, the port, how long a start lasts, password login) and asks
//! on a card every time. Only for people who may change system settings (the caller wires it for them).
//!
//! * The card says what becomes reachable: which accounts, what they see (their own file rules), from where (the local network only),
//!   how they log in, until when, and the firewall opening.
//! * Public keys may pass through the chat (they are not secret). A private key or a password never may: a value that looks like one is
//!   refused. Password login is a switch only; the passwords stay the accounts' own.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use crate::sftp::{Sftp, EXPIRIES};

pub const PROMPT_RULE: &str = "\nSFTP SERVER: sftp_status shows whether the SFTP server is on, its port, who may log in, the keys (fingerprints) and the host key fingerprint. sftp_set changes it and asks the person on a card every time: action enable or disable; allow_user or deny_user (an account name); add_key (a PUBLIC key line `ssh-ed25519 AAAA…`, never a private key) or remove_key (a fingerprint); set_port; set_expiry (1 hour, 1 day, 7 days, never); password_login (on or off). It is off by default, only the local network can reach it, and each account sees only what its own file rules allow. Never ask for or type a password or private key in the chat.";

pub const WRITE_TOOL: &str = "sftp_set";
pub const READ_TOOL: &str = "sftp_status";

pub fn is_sftp_tool(n: &str) -> bool {
    n == READ_TOOL || n == WRITE_TOOL
}

pub struct SftpTools {
    inner: Arc<dyn Tools>,
    sftp: Arc<Sftp>,
    /// The signed-in person (default account for keys).
    me: String,
}

fn text(args: &Value, k: &str) -> String {
    args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

fn expiry_label(secs: u64) -> String {
    if secs == 0 {
        return "until stopped".into();
    }
    EXPIRIES.iter().find(|(_, s)| *s == secs).map_or(format!("{secs} s"), |(l, _)| l.to_string())
}

fn when(until: i64) -> String {
    if until == 0 { "until stopped".into() } else { format!("until {} (Unix time {until})", crate::sftp_session::clean(&until.to_string())) }
}

/// What a valid `sftp_set` call would do, or why not. Pure over the arguments and the current settings.
#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Enable,
    Disable,
    Allow(String),
    Deny(String),
    AddKey(String, String),
    RemoveKey(String, String),
    Port(u16),
    Expiry(u64),
    Password(bool),
}

pub fn parse_action(args: &Value, me: &str) -> Result<Action, String> {
    for (_k, v) in args.as_object().into_iter().flatten() {
        if let Some(s) = v.as_str() {
            if s.len() > 700 || s.contains('\n') || s.contains("PRIVATE KEY") || s.contains("-----BEGIN") {
                return Err("a value is not a plain short setting. Never send a private key or a password through a tool.".into());
            }
        }
    }
    if args.get("password").is_some() || args.get("private_key").is_some() {
        return Err("never send a password or a private key through a tool".into());
    }
    let user = || {
        let u = text(args, "user");
        let u = if u.is_empty() { me.to_string() } else { u };
        if iyakki_sec::policy::valid_username(&u) { Ok(u) } else { Err("user must be an account name (lower-case letters, digits, - and _)".to_string()) }
    };
    match text(args, "action").as_str() {
        "enable" => Ok(Action::Enable),
        "disable" => Ok(Action::Disable),
        "allow_user" => Ok(Action::Allow(user()?)),
        "deny_user" => Ok(Action::Deny(user()?)),
        "add_key" => {
            let k = text(args, "key");
            crate::sftp::parse_key_line(&k)?;
            Ok(Action::AddKey(user()?, k))
        }
        "remove_key" => {
            let f = text(args, "fingerprint");
            if !f.starts_with("SHA256:") || f.len() > 80 {
                return Err("fingerprint looks like SHA256:… (sftp_status lists them)".into());
            }
            Ok(Action::RemoveKey(user()?, f))
        }
        "set_port" => {
            let p = args.get("port").and_then(Value::as_u64).and_then(|p| u16::try_from(p).ok()).ok_or("port is a number from 1024 to 65535")?;
            Ok(Action::Port(crate::sftp::valid_port(p)?))
        }
        "set_expiry" => {
            let w = text(args, "expires").replace(' ', "");
            EXPIRIES.iter().find(|(l, _)| l.replace(' ', "") == w).map(|(_, s)| Action::Expiry(*s)).ok_or_else(|| "expires is `1 hour`, `1 day`, `7 days` or `never`".into())
        }
        "password_login" => match text(args, "value").as_str() {
            "on" => Ok(Action::Password(true)),
            "off" => Ok(Action::Password(false)),
            _ => Err("value is `on` or `off`".into()),
        },
        _ => Err("action is enable, disable, allow_user, deny_user, add_key, remove_key, set_port, set_expiry or password_login".into()),
    }
}

impl SftpTools {
    pub fn new(inner: Arc<dyn Tools>, sftp: Arc<Sftp>, me: &str) -> SftpTools {
        SftpTools { inner, sftp, me: me.to_string() }
    }

    fn card(&self, args: &Value) -> Result<String, String> {
        let a = parse_action(args, &self.me)?;
        let st = self.sftp.status();
        let c = &st.config;
        Ok(match a {
            Action::Enable => {
                if let Some(p) = self.sftp.problem() {
                    return Err(p);
                }
                if st.running {
                    return Err("the SFTP server is already running".into());
                }
                format!(
                    "Turn ON the SFTP server (file transfer from other computers).\n• Port {} on this computer; the firewall is opened for the LOCAL NETWORK ONLY for this port, {}.\n• Accounts that may log in: {}. Each sees only what its own file rules allow (its home folder and other folders it may read).\n• Login: {}.\n• It stops by itself {}; it does not start again after a restart.",
                    c.port,
                    if c.expiry_secs == 0 { "until you turn it off".to_string() } else { format!("and closes again after {}", expiry_label(c.expiry_secs)) },
                    c.users.join(", "),
                    if c.password_login { "authorised keys or the account password" } else { "authorised public keys only" },
                    if c.expiry_secs == 0 { "only when you turn it off".to_string() } else { format!("after {}", expiry_label(c.expiry_secs)) }
                )
            }
            Action::Disable => "Turn OFF the SFTP server: connections end now and the firewall opening is removed.".to_string(),
            Action::Allow(u) => format!("Allow the account “{u}” to log in to the SFTP server. It will see only what its own file rules allow."),
            Action::Deny(u) => format!("Stop allowing the account “{u}” to log in to the SFTP server (its keys are kept)."),
            Action::AddKey(u, k) => {
                let (blob, comment) = crate::sftp::parse_key_line(&k)?;
                format!("Add a public key for the account “{u}”: {} {}. Whoever holds the matching private key can log in as {u} if {u} is allowed.", crate::sshkey::fingerprint_of_blob(&blob), comment)
            }
            Action::RemoveKey(u, f) => format!("Remove the key {f} of the account “{u}”."),
            Action::Port(p) => format!("Use port {p} for the SFTP server (now {}). {}", c.port, if st.running { "A running server keeps its port until it is turned off and on." } else { "" }),
            Action::Expiry(s) => format!("Make each start of the SFTP server last {} (now {}).", expiry_label(s), expiry_label(c.expiry_secs)),
            Action::Password(on) => {
                if on {
                    "Allow logging in to the SFTP server with the ACCOUNT PASSWORD as well as keys. Passwords are checked by the account system (slow hash, throttled), never stored by the server; a key is safer.".to_string()
                } else {
                    "Allow only public keys at the SFTP server (no passwords).".to_string()
                }
            }
        })
    }

    fn status_text(&self) -> String {
        let st = self.sftp.status();
        let c = &st.config;
        let mut s = if st.running { format!("SFTP server: ON, port {}, {}, started by {}.", st.port, when(st.until), st.by) } else { format!("SFTP server: off (port {} when on, {} per start).", c.port, expiry_label(c.expiry_secs)) };
        s.push_str(&format!("\nLogin: {}. Local network only.", if c.password_login { "keys or account password" } else { "keys only" }));
        s.push_str(&format!("\nHost key: {}", st.host_fingerprint));
        if c.users.is_empty() {
            s.push_str("\nAllowed accounts: none.");
        }
        for (u, keys) in &st.keys {
            s.push_str(&format!("\nAllowed account {u}: {} key(s){}", keys.len(), keys.iter().map(|k| format!("\n  {k}")).collect::<String>()));
        }
        if st.running && !st.addresses.is_empty() {
            s.push_str(&format!("\nConnect: sftp -P {} ACCOUNT@{}", st.port, st.addresses[0]));
        }
        if let Some(p) = self.sftp.problem() {
            s.push_str(&format!("\nNote: {p}."));
        }
        s
    }

    fn run(&self, args: &Value) -> Result<String, String> {
        match parse_action(args, &self.me)? {
            Action::Enable => {
                let st = self.sftp.enable(&self.me)?;
                Ok(format!("SFTP server is ON at port {}, {}. Local network only. Host key {}.{}", st.port, when(st.until), st.host_fingerprint, st.addresses.first().map(|a| format!(" Connect: sftp -P {} ACCOUNT@{a}", st.port)).unwrap_or_default()))
            }
            Action::Disable => Ok(if self.sftp.disable(&self.me) { "SFTP server turned off; the firewall opening is removed.".into() } else { "SFTP server was not running (any leftover firewall opening was removed).".into() }),
            Action::Allow(u) => {
                self.sftp.configure(|c| {
                    c.users.push(u.clone());
                    Ok(())
                })?;
                Ok(format!("{u} may log in to the SFTP server once it is on."))
            }
            Action::Deny(u) => {
                self.sftp.configure(|c| {
                    c.users.retain(|x| *x != u);
                    Ok(())
                })?;
                Ok(format!("{u} may no longer log in (a connection already open ends when the server is turned off)."))
            }
            Action::AddKey(u, k) => Ok(format!("Added key {} for {u}.", crate::sftp::add_key(self.sftp_dir(), &u, &k)?)),
            Action::RemoveKey(u, f) => Ok(format!("Removed {} key(s) of {u}.", crate::sftp::remove_key(self.sftp_dir(), &u, &f)?)),
            Action::Port(p) => {
                self.sftp.configure(|c| {
                    c.port = p;
                    Ok(())
                })?;
                Ok(format!("The SFTP server will use port {p}."))
            }
            Action::Expiry(s) => {
                self.sftp.configure(|c| {
                    c.expiry_secs = s;
                    Ok(())
                })?;
                Ok(format!("Each start now lasts {}.", expiry_label(s)))
            }
            Action::Password(on) => {
                self.sftp.configure(|c| {
                    c.password_login = on;
                    Ok(())
                })?;
                Ok(if on { "Password login is allowed (takes effect at the next start).".into() } else { "Password login is off (takes effect at the next start).".into() })
            }
        }
    }

    fn sftp_dir(&self) -> &std::path::Path {
        self.sftp.dir()
    }
}

impl Tools for SftpTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_sftp_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "Show the SFTP server: on or off, port, until when, who may log in, their key fingerprints, the host key fingerprint and what is missing. Reading needs no approval.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef {
            spec: ToolSpec {
                name: WRITE_TOOL.into(),
                description: "Change the SFTP server (file transfer from other computers on the local network). The person approves a card every time. Off by default; each account sees only what its own file rules allow. Never send a password or a private key.".into(),
                schema: json!({"type": "object", "properties": {
                    "action": {"type": "string", "enum": ["enable", "disable", "allow_user", "deny_user", "add_key", "remove_key", "set_port", "set_expiry", "password_login"]},
                    "user": {"type": "string", "description": "Account name (default: the signed-in person)."},
                    "key": {"type": "string", "description": "add_key: one PUBLIC key line, ssh-ed25519 AAAA… comment."},
                    "fingerprint": {"type": "string", "description": "remove_key: SHA256:… as sftp_status shows."},
                    "port": {"type": "integer", "description": "set_port: 1024 to 65535."},
                    "expires": {"type": "string", "enum": ["1 hour", "1 day", "7 days", "never"], "description": "set_expiry: how long each start lasts."},
                    "value": {"type": "string", "enum": ["on", "off"], "description": "password_login."}
                }, "required": ["action"]}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => Ok(self.status_text()),
            WRITE_TOOL => {
                self.card(args)?;
                self.run(args)
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if name == WRITE_TOOL {
            return match self.card(args) {
                Ok(c) => c.lines().next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            };
        }
        if name == READ_TOOL { "Show the SFTP server".into() } else { self.inner.describe(name, args) }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if name == WRITE_TOOL { self.card(args).ok() } else if name == READ_TOOL { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if name == WRITE_TOOL { self.card(args).err() } else if name == READ_TOOL { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_sftp_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != WRITE_TOOL && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == WRITE_TOOL { Some("the SFTP server asks every time".into()) } else { self.inner.grant_blocker(name, args) }
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }
    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }
    fn begin_step(&self) {
        self.inner.begin_step()
    }
    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }
    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
}
