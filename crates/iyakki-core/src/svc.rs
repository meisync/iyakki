//! `iyakki serve --profile NAME`: the headless web service (docs/development/MINIMAL-WEBSERVICE.md). This file is only wiring:
//! it adapts the shell's accounts and audit log, and the firewall engine, to the traits the service layer in `iyakki-agent`
//! names, then runs `iyakki_agent::webservice::run`. No handler logic lives here.

use iyakki_agent::webservice::config::{ServiceConfig, DEFAULT_PATH};
use iyakki_agent::webservice::{self, PortOpener};
use iyakki_core_api::{ApiError, ApiResult, AuditSink, Authenticator, Principal};
use iyakki_shell::accounts::{AccountError, AuditLog, UserStore};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

/// The machine's accounts (`users.json`) as the service's authenticator. The store's own rules apply: Argon2id, one verification
/// for unknown names too, a throttle per account after failures.
pub struct ShellAuth(pub Arc<Mutex<UserStore>>);

fn principal(name: &str, role: iyakki_sec::policy::Role, policy: iyakki_sec::policy::Policy) -> Principal {
    Principal { user: name.to_string(), role: role.name().to_string(), admin: role == iyakki_sec::policy::Role::Admin, policy }
}

impl Authenticator for ShellAuth {
    fn login(&self, name: &str, password: &str) -> ApiResult<Principal> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        match s.authenticate(name, password) {
            Ok(sess) => Ok(principal(&sess.user, sess.role, sess.policy)),
            Err(AccountError::Throttled(n)) => Err(ApiError::Busy(format!("too many attempts; try again in {n} s"))),
            Err(_) => Err(ApiError::Denied("invalid username or password".into())),
        }
    }

    fn lookup(&self, user: &str) -> Option<Principal> {
        let mut s = self.0.lock().unwrap_or_else(|e| e.into_inner());
        s.refresh();
        s.get(user).map(|u| principal(&u.name, u.role, u.policy()))
    }
}

pub struct ShellAudit(pub Arc<AuditLog>);

impl AuditSink for ShellAudit {
    fn record(&self, user: &str, event: &str, detail: &str) {
        self.0.record(user, event, detail);
    }
}

/// Opens the service port to the named networks with one rule per network in the firewall engine (comment "IYAKKI web service").
/// Earlier rules of the same comment are replaced, so a restart or a changed list never piles rules up. The rule is kept at once:
/// the 60-second "Keep" question is for a person at a screen.
pub struct FirewallOpener;

pub const RULE_COMMENT: &str = "IYAKKI web service";

/// The settings with the service's rules for `port` and `allow_from` (pure, so a test can read it).
pub fn with_service_rules(mut cfg: iyakki_fw::model::Config, port: u16, allow_from: &[String]) -> iyakki_fw::model::Config {
    cfg.rules.retain(|r| r.comment != RULE_COMMENT);
    for net in allow_from {
        let mut r = iyakki_fw::model::Rule::new(iyakki_fw::model::Kind::Cidr, net);
        r.proto = iyakki_fw::model::Proto::Tcp;
        r.ports = port.to_string();
        r.comment = RULE_COMMENT.to_string();
        cfg.add(r);
    }
    cfg
}

impl PortOpener for FirewallOpener {
    fn open(&self, port: u16, allow_from: &[String]) -> Result<(), String> {
        let mut e = iyakki_fw::service::engine();
        let new = with_service_rules(e.config().clone(), port, allow_from);
        e.propose(new, "web service")?;
        match e.keep("web service") {
            Ok(()) | Err(_) => Ok(()), // nothing waiting means nothing changed
        }
    }
}

/// What `serve` was asked: the profile and where the configuration is.
#[derive(Debug, PartialEq, Eq)]
pub struct Ask {
    pub profile: Option<String>,
    pub config: Option<String>,
    pub listen: Option<String>,
    pub check_only: bool,
}

/// `--profile NAME` / `--profile=NAME` or `IYAKKI_PROFILE`; `--config FILE` or `IYAKKI_SERVICE_CONFIG`; `--listen ADDR` or
/// `IYAKKI_LISTEN`; `--check`. `None` when no profile was asked for: `serve` then does what it always did.
pub fn parse_args(args: &[String], env: &dyn Fn(&str) -> Option<String>) -> Result<Option<Ask>, String> {
    let mut a = Ask { profile: None, config: None, listen: None, check_only: false };
    let mut i = 0;
    while i < args.len() {
        let s = args[i].as_str();
        let mut val = |name: &str| -> Result<Option<String>, String> {
            if let Some(v) = s.strip_prefix(&format!("{name}=")) {
                return Ok(Some(v.to_string()));
            }
            if s == name {
                i += 1;
                return args.get(i).cloned().map(Some).ok_or_else(|| format!("{name} needs a value"));
            }
            Ok(None)
        };
        if let Some(v) = val("--profile")? {
            a.profile = Some(v);
        } else if let Some(v) = val("--config")? {
            a.config = Some(v);
        } else if let Some(v) = val("--listen")? {
            a.listen = Some(v);
        } else if s == "--check" {
            a.check_only = true;
        }
        i += 1;
    }
    a.profile = a.profile.or_else(|| env("IYAKKI_PROFILE")).filter(|p| !p.is_empty());
    a.config = a.config.or_else(|| env("IYAKKI_SERVICE_CONFIG")).filter(|p| !p.is_empty());
    a.listen = a.listen.or_else(|| env("IYAKKI_LISTEN")).filter(|p| !p.is_empty());
    Ok(a.profile.is_some().then_some(a))
}

/// The checked configuration for `ask`: the file (`--config`, else `/etc/iyakki/service.json` when it exists, else the defaults),
/// then the command line over it.
pub fn load(ask: &Ask) -> Result<webservice::config::Checked, String> {
    let cfg = match (&ask.config, std::path::Path::new(DEFAULT_PATH).exists()) {
        (Some(p), _) => ServiceConfig::load(std::path::Path::new(p))?,
        (None, true) => ServiceConfig::load(std::path::Path::new(DEFAULT_PATH))?,
        (None, false) => ServiceConfig::default(),
    };
    cfg.with_overrides(ask.profile.as_deref(), ask.listen.as_deref()).check()
}

/// Run the service over the shell's environment until `stop`.
pub fn run(chk: &webservice::config::Checked, env: iyakki_shell::shell::ShellEnv, stop: &AtomicBool) -> Result<(), String> {
    let deps = webservice::real::assemble(Arc::new(ShellAuth(env.store.clone())), Arc::new(ShellAudit(env.audit.clone())), env.services.clone(), env.host.clone());
    webservice::run(chk, deps, &FirewallOpener, stop)
}

/// The one-line summary `--check` prints.
pub fn summary(chk: &webservice::config::Checked) -> String {
    format!("profile {} on https://{} ({}), {} tools, firewall rule {}", chk.profile.name(), chk.addr, if chk.addr.ip().is_loopback() { "loopback only" } else { "LAN opt-in" }, chk.profile.tools().len(), if chk.addr.ip().is_loopback() { "none" } else { "yes" })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_shell::accounts::UserStore;

    fn no_env(_: &str) -> Option<String> {
        None
    }
    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_command_line_and_the_environment_choose_the_profile_and_nothing_else_starts_it() {
        assert_eq!(parse_args(&args(&["--web"]), &no_env).unwrap(), None, "no profile: serve does what it always did");
        let a = parse_args(&args(&["--profile", "agent-api", "--config=/x.json", "--check"]), &no_env).unwrap().unwrap();
        assert_eq!((a.profile.as_deref(), a.config.as_deref(), a.check_only), (Some("agent-api"), Some("/x.json"), true));
        let a = parse_args(&args(&["--profile=cloud", "--listen", "127.0.0.1:9"]), &no_env).unwrap().unwrap();
        assert_eq!((a.profile.as_deref(), a.listen.as_deref()), (Some("cloud"), Some("127.0.0.1:9")));
        let env = |k: &str| (k == "IYAKKI_PROFILE").then(|| "agent-api".to_string());
        assert_eq!(parse_args(&[], &env).unwrap().unwrap().profile.as_deref(), Some("agent-api"));
        assert!(parse_args(&args(&["--profile"]), &no_env).is_err());
        let bad = Ask { profile: Some("nope".into()), config: None, listen: None, check_only: true };
        assert!(load(&bad).unwrap_err().contains("unknown profile"));
    }

    #[test]
    fn the_firewall_rules_are_one_per_network_for_the_port_and_replace_the_old_ones() {
        let base = iyakki_fw::model::Config::default();
        let a = with_service_rules(base.clone(), 8444, &["10.0.0.0/8".into(), "192.168.1.0/24".into()]);
        let mine: Vec<_> = a.rules.iter().filter(|r| r.comment == RULE_COMMENT).collect();
        assert_eq!(mine.len(), 2);
        assert!(mine.iter().all(|r| r.ports == "8444" && r.proto == iyakki_fw::model::Proto::Tcp && r.enabled));
        let b = with_service_rules(a, 9000, &["10.0.0.0/8".into()]);
        let mine: Vec<_> = b.rules.iter().filter(|r| r.comment == RULE_COMMENT).collect();
        assert_eq!((mine.len(), mine[0].ports.as_str()), (1, "9000"));
        assert!(with_service_rules(b, 1, &[]).rules.is_empty(), "no networks: the rules are removed");
    }

    #[test]
    fn the_shell_accounts_sign_people_in_and_a_removed_account_is_gone() {
        let dir = std::env::temp_dir().join(format!("iyakki-svc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut s = UserStore::open(&dir.join("users.json")).unwrap();
        s.add_local("ann", "ann-password-1", iyakki_sec::policy::Role::User).unwrap();
        s.add_local("root", "root-password-1", iyakki_sec::policy::Role::Admin).unwrap();
        let auth = ShellAuth(Arc::new(Mutex::new(s)));
        let p = auth.login("ann", "ann-password-1").unwrap();
        assert_eq!((p.user.as_str(), p.role.as_str(), p.admin), ("ann", "user", false));
        assert!(auth.login("root", "root-password-1").unwrap().admin);
        assert_eq!(auth.login("ann", "wrong-password").unwrap_err().code(), "denied");
        assert_eq!(auth.login("nobody", "whatever-123").unwrap_err().code(), "denied", "one answer for both");
        assert!(auth.lookup("ann").is_some() && auth.lookup("nobody").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
