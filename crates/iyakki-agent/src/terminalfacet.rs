//! The core's terminal for apps that embed one (`iyakki_core_api::terminal::Terminal`, F-SDK-102): sessions of the IYAKKI command line (`cli.rs`)
//! checked by the signed-in person's own policy. The policy comes from [`PolicySource`] (the accounts), never from the app. Lines that only
//! read and print run at once; anything else (a write, a program, the network, another session) waits for the person's yes through
//! [`Terminal::decide`]. The machine is never switched off or restarted from here. Each open, run, answer and close is one audit line
//! carrying the command word only, never the arguments.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use iyakki_core_api::terminal::{self as t, PolicySource, TermApproval, TermOpen, TermReply, Terminal};
use iyakki_sec::policy::Policy;

use crate::cli::{Action, Cli};
use crate::services::Services;

/// The host a terminal session sees: it can detect and report, and it can do nothing to the machine.
struct SealedHost;

impl iyakki_core_api::HostApi for SealedHost {
    fn detect_env(&self) -> String {
        "embedded terminal".into()
    }
    fn net_status(&self) -> String {
        "not reported here".into()
    }
    fn run_wasm(&self, _user: &str, _policy: &Policy, _wasm: &[u8], _entry: &str) -> Result<String, String> {
        Err("programs are not run from an embedded terminal".into())
    }
    fn power_off(&self) {}
    fn reboot(&self) {}
    fn can_power(&self) -> bool {
        false
    }
}

struct Session {
    app: String,
    cli: Cli,
    waiting: Option<(String, String)>,
}

pub struct Terminals {
    services: Arc<Services>,
    policies: Mutex<Option<Arc<dyn PolicySource>>>,
    sessions: Mutex<HashMap<(String, String), Session>>,
    next: std::sync::atomic::AtomicU64,
}

fn lk<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Terminals {
    pub fn new(services: Arc<Services>) -> Terminals {
        Terminals { services, policies: Mutex::new(None), sessions: Mutex::new(HashMap::new()), next: std::sync::atomic::AtomicU64::new(1) }
    }

    pub fn set_policies(&self, p: Arc<dyn PolicySource>) {
        *lk(&self.policies) = Some(p);
    }

    fn audit(&self, user: &str, what: &str, word: &str) {
        self.services.browser.audit(user, &format!("terminal.embedded.{what}"), word);
    }

    fn finish(&self, s: &mut Session, line: &str) -> TermReply {
        let out = s.cli.exec(line);
        let (text, cut) = t::cap_output(&out.text);
        TermReply { text, prompt: s.cli.prompt(), clear: out.action == Action::Clear, cut, approval: None }
    }
}

impl Terminal for Terminals {
    fn open(&self, user: &str, app: &str) -> Result<TermOpen, String> {
        let policy = lk(&self.policies).as_ref().and_then(|p| p.policy_of(user)).ok_or("this account has no terminal here")?;
        if policy.cli == iyakki_sec::policy::CliLevel::Off {
            return Err("The terminal is disabled for your account.".into());
        }
        let mut g = lk(&self.sessions);
        if g.keys().filter(|(u, _)| u == user).count() >= t::MAX_SESSIONS {
            return Err(format!("at most {} terminals can be open at once", t::MAX_SESSIONS));
        }
        let mut cli = Cli::new(user, policy, Arc::new(SealedHost)).with_services(self.services.clone());
        cli.set_cwd(&crate::scope::home_root_for_writes(user, self.services.dir()));
        let id = format!("t{}", self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
        let prompt = cli.prompt();
        g.insert((user.to_string(), id.clone()), Session { app: app.chars().take(40).collect(), cli, waiting: None });
        drop(g);
        self.audit(user, "open", app);
        Ok(TermOpen { id, prompt, banner: "IYAKKI terminal. Lines that change something wait for your yes; the machine is not switched off from here.".into() })
    }

    fn run(&self, user: &str, session: &str, line: &str) -> Result<TermReply, String> {
        t::check_line(line)?;
        let mut g = lk(&self.sessions);
        let s = g.get_mut(&(user.to_string(), session.to_string())).ok_or("that terminal is not open")?;
        if s.waiting.is_some() {
            return Err("answer the waiting question first".into());
        }
        let word = t::commands_of(line).first().copied().unwrap_or("").chars().take(24).collect::<String>();
        if t::is_power(line) {
            self.audit(user, "refused", &word);
            return Ok(TermReply { text: t::NO_POWER.into(), prompt: s.cli.prompt(), ..TermReply::default() });
        }
        if !t::runs_without_asking(line) {
            let call = format!("c{}", self.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
            let summary = format!("Run in your terminal: {}", line.chars().take(200).collect::<String>());
            s.waiting = Some((call.clone(), line.to_string()));
            self.audit(user, "asked", &word);
            return Ok(TermReply { prompt: s.cli.prompt(), approval: Some(TermApproval { call, summary }), ..TermReply::default() });
        }
        self.audit(user, "run", &word);
        let app = s.app.clone();
        let _ = app;
        Ok(self.finish(s, line))
    }

    fn decide(&self, user: &str, session: &str, call: &str, approve: bool) -> Result<TermReply, String> {
        let mut g = lk(&self.sessions);
        let s = g.get_mut(&(user.to_string(), session.to_string())).ok_or("that terminal is not open")?;
        let Some((waiting, line)) = s.waiting.clone().filter(|(c, _)| c == call) else { return Err("there is no such question waiting".into()) };
        s.waiting = None;
        let word = t::commands_of(&line).first().copied().unwrap_or("").chars().take(24).collect::<String>();
        let _ = waiting;
        if !approve {
            self.audit(user, "denied", &word);
            return Ok(TermReply { text: "Not run: you said no.".into(), prompt: s.cli.prompt(), ..TermReply::default() });
        }
        self.audit(user, "approved", &word);
        Ok(self.finish(s, &line))
    }

    fn close(&self, user: &str, session: &str) {
        if lk(&self.sessions).remove(&(user.to_string(), session.to_string())).is_some() {
            self.audit(user, "close", "");
        }
    }
}
