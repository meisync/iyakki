//! Why Claude Code looked signed out after a restart: a status found before anyone signed in (HOME=/) stayed in the cache, and a
//! failure that was not "signed out" (network, slow start) was read as signed out. A fake `claude` stands in for the real one.

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use super::*;
use crate::wsexec::{Out, Runner, Sink};

struct Off;
impl iyakki_proxy::Upstream for Off {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-auth-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    std::fs::canonicalize(&d).unwrap()
}

/// Prints `loggedIn: true` only when it was started with the expected HOME and CLAUDE_CONFIG_DIR (the file `expect` names the
/// home); records what it was started with in `seen.log`. It never touches a real home.
fn fake(dir: &std::path::Path) -> PathBuf {
    let p = dir.join("claude");
    std::fs::write(
        &p,
        format!(
            r#"#!/bin/sh
echo "$1 HOME=$HOME CFG=$CLAUDE_CONFIG_DIR USER=$USER CWD=$(pwd) UID=$(id -u)" >> "{d}/seen.log"
case "$1" in
  --version) echo "2.1.307 (Claude Code)"; exit 0;;
  auth)
    if [ -f "{d}/expect" ] && [ "$HOME" = "$(cat {d}/expect)" ] && [ "$CLAUDE_CONFIG_DIR" = "$HOME/.claude" ]; then
      echo '{{"loggedIn":true,"authMethod":"claude.ai","subscriptionType":"pro"}}'; exit 0
    fi
    echo '{{"loggedIn":false,"authMethod":"none"}}'; exit 1;;
esac
exit 0
"#,
            d = dir.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn wait_idle(c: &Arc<StatusCache>) {
    std::thread::sleep(Duration::from_millis(50));
    for _ in 0..1500 {
        if !c.busy() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("the lookup never ended");
}

fn wait_for(c: &Arc<StatusCache>, f: impl Fn(&Status) -> bool) -> Status {
    for _ in 0..1500 {
        if let Some(s) = c.get(CLAUDE_CODE).filter(|s| f(s)) {
            return s;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("no matching status: {:?}", c.get(CLAUDE_CODE));
}

#[test]
fn status_follows_the_sign_in_in_the_order_of_a_real_boot() {
    let dir = tmp("boot");
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let claude = fake(&bin);
    let svc = crate::services::Services::open(&dir.join("svc"), Arc::new(Off), Arc::new(Off)).unwrap();
    svc.settings.lock().unwrap().update(|s| s.agents.claude.command = claude.display().to_string()).unwrap();

    // (a) boot: nobody signed in yet, no home of the person's: the lookup looks in the process's own home and finds "sign in"
    // (this is what used to stay in the cache after the sign-in)
    let cfg = svc.agents_cfg();
    assert!(cfg.env.is_empty());
    svc.ext_status.refresh(cfg);
    wait_idle(&svc.ext_status);
    let s = svc.ext_status.get(CLAUDE_CODE).unwrap();
    assert!(s.installed && s.auth == Auth::SignedOut && s.chip == "sign in", "{s:?}");

    // (b) the person signs in: what was found is void and the lookup runs again with their home
    let user = "tester9x";
    svc.on_login(user, "pw", false);
    assert!(svc.ext_status.get(CLAUDE_CODE).is_none_or(|s| s.auth != Auth::SignedOut || svc.ext_status.busy()), "the old answer is void");
    wait_idle(&svc.ext_status);
    let cfg = svc.agents_cfg();
    let home = cfg.env.iter().find(|(k, _)| k == "HOME").unwrap().1.clone();
    assert_eq!(cfg.env.iter().find(|(k, _)| k == "CLAUDE_CONFIG_DIR").unwrap().1, format!("{home}/.claude"));
    std::fs::write(bin.join("expect"), &home).unwrap();
    svc.ext_status.refresh(svc.agents_cfg());
    wait_idle(&svc.ext_status);
    let s = wait_for(&svc.ext_status, |s| s.auth != Auth::Unknown);
    assert!(matches!(s.auth, Auth::SignedIn(_)) && s.ready() && s.chip == "ready", "{s:?}");
    let seen = std::fs::read_to_string(bin.join("seen.log")).unwrap();
    assert!(seen.lines().any(|l| l.starts_with("auth ") && l.contains(&format!("HOME={home} CFG={home}/.claude"))), "{seen}");

    // (c) a status found while signed out (an old one) mends itself when a screen asks again
    let before = seen.lines().count();
    svc.ext_status.set(CLAUDE_CODE, Status { installed: true, version: "x".into(), auth: Auth::SignedOut, mode: Mode::Bridged, chip: "sign in".into(), fix: "f".into() });
    svc.ext_status.recheck_after(svc.agents_cfg(), CLAUDE_CODE, Duration::ZERO);
    wait_for(&svc.ext_status, |s| s.ready());
    assert!(std::fs::read_to_string(bin.join("seen.log")).unwrap().lines().count() > before);

    // a stale lookup that started for the old home is dropped, never stored after a sign-out
    svc.on_logout(user, false);
    assert!(svc.ext_status.get(CLAUDE_CODE).is_none() || svc.agents_cfg().env.is_empty());
    assert!(svc.agents_cfg().env.is_empty(), "nobody is signed in any more");
    wait_idle(&svc.ext_status);

    // (d) a chat run is started with the same home, in the same process user
    svc.on_login(user, "pw", false);
    wait_idle(&svc.ext_status);
    let cfg = svc.agents_cfg();
    let a = cfg.agent(CLAUDE_CODE).unwrap();
    let ws = dir.join("ws");
    std::fs::create_dir_all(&ws).unwrap();
    let inv = a.invocation(&Plan { workspace: &ws, prompt: "hi", system: "", model: "sonnet", resume: None, mcp_config: None, key: None, max_turns: 3, no_tools: false });
    assert!(inv.env.contains(&("HOME".into(), home.clone())) && inv.env.contains(&("CLAUDE_CONFIG_DIR".into(), format!("{home}/.claude"))), "{:?}", inv.env);
    let mut sp = proc::spawn(&["/bin/sh".into(), "-c".into(), format!("{} auth status", claude.display())], &ws, &inv.env, None).unwrap();
    let _ = sp.child.wait();
    let seen = std::fs::read_to_string(bin.join("seen.log")).unwrap();
    let last = seen.lines().last().unwrap();
    assert!(last.contains(&format!("HOME={home} CFG={home}/.claude")) && last.contains(&format!("CWD={}", ws.display())), "{last}");
    assert!(last.contains(&format!("UID={}", unsafe { libc::getuid() })), "a run is the same user as the lookup: {last}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_failure_that_is_not_signed_out_is_never_shown_as_sign_in() {
    let a = claude_code::ClaudeCode::new(AgentCfg::default());
    let out = |code: Option<i32>, stdout: &str, stderr: &str, timed_out: bool| Out { code, stdout: stdout.into(), stderr: stderr.into(), timed_out, ..Default::default() };
    assert_eq!(a.parse_auth(&out(Some(1), r#"{"loggedIn":false,"authMethod":"none"}"#, "", false)), Auth::SignedOut);
    assert_eq!(a.parse_auth(&out(Some(1), "", "Error: getaddrinfo ENOTFOUND api.example.test", false)), Auth::Unknown);
    assert_eq!(a.parse_auth(&out(Some(1), "", "fetch failed: network unreachable", false)), Auth::Unknown);
    assert_eq!(a.parse_auth(&out(None, "", "", true)), Auth::Unknown, "a timeout says nothing");
    assert_eq!(a.parse_auth(&out(Some(1), "", "Not logged in", false)), Auth::SignedOut);
    assert!(matches!(a.parse_auth(&out(Some(0), r#"{"loggedIn":true,"authMethod":"claude.ai"}"#, "", false)), Auth::SignedIn(_)));
}

struct Slow {
    calls: Mutex<Vec<Duration>>,
}
impl Runner for Slow {
    fn run(&self, _cwd: &std::path::Path, argv: &[String], _env: &[(String, String)], timeout: Duration, _cap: usize, _s: Option<Sink>) -> Out {
        self.calls.lock().unwrap().push(timeout);
        // the first try at each command times out (a cold start); the second answers
        let n = self.calls.lock().unwrap().len();
        if n % 2 == 1 {
            return Out { timed_out: true, ..Default::default() };
        }
        if argv.iter().any(|a| a == "--version") {
            Out { code: Some(0), stdout: "2.1.307".into(), ..Default::default() }
        } else {
            Out { code: Some(0), stdout: r#"{"loggedIn":true,"authMethod":"claude.ai"}"#.into(), ..Default::default() }
        }
    }
    fn has(&self, _: &str) -> bool {
        true
    }
}

#[test]
fn a_slow_first_start_gets_a_longer_second_try_instead_of_not_installed_or_sign_in() {
    let r = Slow { calls: Mutex::new(vec![]) };
    let a = claude_code::ClaudeCode::new(AgentCfg::default());
    let s = detect(&a, &r, &std::env::temp_dir());
    assert!(s.installed && s.ready() && matches!(s.auth, Auth::SignedIn(_)), "{s:?}");
    let c = r.calls.lock().unwrap().clone();
    assert_eq!(c.len(), 4);
    assert!(c[1] > c[0] && c[3] > c[2], "{c:?}");
}

#[test]
fn the_log_line_says_why_and_never_holds_a_token_or_an_address() {
    let r = redact(r#"{"accessToken":"sk-ant-oat01-abcdefghijklmnopqrstuvwxyz0123456789","email":"someone@example.test","loggedIn":false}"#);
    assert!(!r.contains("abcdefghijkl") && !r.contains("someone@") && r.contains("loggedIn"), "{r}");
    assert!(!redact("ghp_abcdefghijklmnop").contains("abcdefgh"));
    assert_eq!(redact("Not logged in"), "Not logged in");
    let dir = tmp("line");
    let home = dir.join("home");
    std::fs::create_dir_all(home.join(".claude")).unwrap();
    std::fs::write(home.join(".claude/.credentials.json"), "{}").unwrap();
    let env = vec![("HOME".to_string(), home.display().to_string()), ("CLAUDE_CONFIG_DIR".to_string(), home.join(".claude").display().to_string())];
    let a = claude_code::ClaudeCode::new(AgentCfg { env: env.clone(), ..Default::default() });
    let o = Out { code: Some(1), stdout: r#"{"loggedIn":false,"token":"eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9"}"#.into(), ..Default::default() };
    let l = signed_out_line(&a, &env, Some(&o));
    assert!(l.contains("claude-code") && l.contains("exit 1") && l.contains(&format!("{}/.claude/.credentials.json exists", home.display())), "{l}");
    assert!(l.contains(&format!("HOME={}", home.display())) && !l.contains("eyJhbGci"), "{l}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn copilot_with_no_home_yet_cannot_say_instead_of_signed_out() {
    let c = copilot::Copilot::new(AgentCfg { env: vec![], ..Default::default() });
    // the process's own home is `/` on the system: no answer, not "sign in" (checked through the same path the cache uses)
    if std::env::var("HOME").map_or(true, |h| h == "/" || h.is_empty()) {
        assert_eq!(c.auth_offline(), Auth::Unknown);
    }
    let dir = tmp("copilot");
    let withhome = copilot::Copilot::new(AgentCfg { env: vec![("HOME".into(), dir.display().to_string())], ..Default::default() });
    assert_eq!(withhome.auth_offline(), Auth::SignedOut, "a real home with no ~/.copilot is signed out");
    std::fs::create_dir_all(dir.join(".copilot")).unwrap();
    std::fs::write(dir.join(".copilot/config.json"), r#"{"last_logged_in_user":{"login":"octo"}}"#).unwrap();
    assert!(matches!(withhome.auth_offline(), Auth::SignedIn(_)));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn settings_cache_drops_a_lookup_that_began_for_the_old_home() {
    let c = Arc::new(StatusCache::default());
    c.set(CLAUDE_CODE, Status { installed: true, version: String::new(), auth: Auth::SignedOut, mode: Mode::Bridged, chip: "sign in".into(), fix: String::new() });
    c.invalidate();
    assert!(c.get(CLAUDE_CODE).is_none(), "after a sign-in the screens show checking…");
}
