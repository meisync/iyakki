//! CLI: the shell's command line. Every command is checked against the signed-in user's policy:
//! file access is limited to their scopes (symlinks resolved first), `run` uses their compute
//! limits and capability tokens, and power commands need the `power` permission.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use iyakki_sec::policy::{CliLevel, Policy};

use crate::pkg::Packages;
use crate::skills::{Scope, SkillStore};
use crate::HostApi;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    Clear,
    PowerOff,
    Reboot,
}

#[derive(Debug, PartialEq, Eq)]
pub struct CliOut {
    pub text: String,
    pub action: Action,
}

fn out(s: impl Into<String>) -> CliOut {
    CliOut { text: s.into(), action: Action::None }
}

pub struct Cli {
    pub user: String,
    pub policy: Policy,
    host: Arc<dyn HostApi>,
    cwd: PathBuf,
    skills: Option<(Arc<SkillStore>, Arc<Packages>)>,
    services: Option<Arc<crate::services::Services>>,
    /// Inside a Linux apps sandbox (`linux use <name>`): the lines typed run there (`cli_linux.rs`).
    sandbox: Option<String>,
}

const RESTRICTED: &[&str] = &["help", "whoami", "policy", "echo", "clear", "pwd", "cd", "ls", "cat", "run", "skill", "tool", "store", "toollog", "curl", "grep", "sed", "tr", "cut", "sort", "uniq", "wc", "head", "tail", "tac", "rev", "nl", "seq", "date", "basename", "dirname", "calendar", "alarm", "clis"];

/// One SSH command of the terminal, owning what it needs so it can run on a thread of its own.
pub struct SshCall {
    user: String,
    policy: Policy,
    cwd: PathBuf,
    svc: Arc<crate::services::Services>,
    cmd: String,
    rest: String,
}

impl SshCall {
    /// The program name and the arguments as typed (never shown in the audit log: they can hold host names and users).
    pub fn command(&self) -> &str {
        &self.cmd
    }

    fn reach(&self, host: &str) -> Result<(), String> {
        if self.policy.allows_host(host) { Ok(()) } else { Err(format!("your account may not reach {host}")) }
    }

    fn check_local(&self, path: &str, write: bool) -> Result<PathBuf, String> {
        if write {
            let r = crate::scope::resolve_for_write(&self.cwd, path)?;
            crate::scope::check_write(&self.policy, &self.user, &r)?;
            Ok(r)
        } else {
            let r = crate::scope::resolve(&self.cwd, path)?;
            crate::scope::check_read(&self.policy, &self.user, &r)?;
            Ok(r)
        }
    }

    pub fn run(&self, sink: Option<crate::exec::Sink>) -> Result<String, String> {
        let env = crate::ssh::Env::for_services(&self.svc, &self.user);
        let args = crate::exec::split_words(&self.rest)?;
        match self.cmd.as_str() {
            "ssh" | "scp" | "sftp" => {
                let plan = crate::ssh::plan(&self.cmd, &args, crate::ssh::Mode::Person)?;
                self.reach(&plan.host)?;
                crate::ssh::run(&env, crate::ssh::Mode::Person, &self.cmd, &args, &self.cwd, 120, &|p, w| self.check_local(p, w), sink)
            }
            "sshhosts" => {
                let keys = env.known_hosts().read();
                Ok(if keys.is_empty() { "no host keys are trusted yet (sshtrust <host> shows a host's fingerprint and trusts it once you confirm)".into() } else { keys.iter().map(|k| format!("{}  {} {}", k.pattern, k.kind, k.fingerprint())).collect::<Vec<_>>().join("\n") })
            }
            "sshforget" => {
                let (host, port) = crate::ssh::host_port(&args)?;
                let n = env.known_hosts().forget(&host, port)?;
                Ok(if n == 0 { format!("nothing was trusted for {}", crate::ssh::pattern(&host, port)) } else { format!("forgot {n} key(s) for {}", crate::ssh::pattern(&host, port)) })
            }
            _ => {
                let (host, port) = crate::ssh::host_port(&args)?;
                self.reach(&host)?;
                let want = args.iter().find(|a| a.starts_with("SHA256:")).cloned();
                crate::ssh::trust_flow(&env, &host, port, want.as_deref())
            }
        }
    }
}

/// Splits a command line at `|` (not inside quotes, and not `||`).
fn split_pipes(line: &str) -> Vec<String> {
    let (mut out, mut cur) = (Vec::new(), String::new());
    let (mut q, mut esc) = (None::<char>, false);
    let c: Vec<char> = line.chars().collect();
    for (i, &ch) in c.iter().enumerate() {
        if esc {
            esc = false;
        } else if ch == '\\' && q != Some('\'') {
            esc = true;
        } else if let Some(open) = q {
            if ch == open {
                q = None;
            }
        } else if ch == '\'' || ch == '"' {
            q = Some(ch);
        } else if ch == '|' && c.get(i + 1) != Some(&'|') && (i == 0 || c[i - 1] != '|') {
            out.push(std::mem::take(&mut cur));
            continue;
        }
        cur.push(ch);
    }
    out.push(cur);
    out
}
const FULL_ONLY: &[&str] = &["detect", "net", "scan", "mkdir", "write", "poweroff", "reboot", "linux"];

#[path = "cli_linux.rs"]
mod linux;

impl Cli {
    pub fn new(user: &str, policy: Policy, host: Arc<dyn HostApi>) -> Cli {
        let home = crate::scope::user_home(user);
        let cwd = if policy.allows_read(user, home.to_str().unwrap_or("/")) && home.is_dir() { home } else { PathBuf::from("/") };
        Cli { user: user.into(), policy, host, cwd, skills: None, services: None, sandbox: None }
    }

    /// Give the terminal the `skill` and `store` commands.
    /// Gives the terminal the services it needs for `toollog`.
    pub fn with_services(mut self, services: Arc<crate::services::Services>) -> Cli {
        self.services = Some(services);
        self
    }

    pub fn with_skills(mut self, skills: Arc<SkillStore>, packages: Arc<Packages>) -> Cli {
        self.skills = Some((skills, packages));
        self
    }

    pub fn prompt(&self) -> String {
        if let Some(s) = &self.sandbox {
            return format!("{}@{s}:~$ ", self.user);
        }
        format!("{}@iyakki:{}$ ", self.user, self.cwd.display())
    }

    fn resolve(&self, p: &str) -> Result<PathBuf, String> {
        crate::scope::resolve(&self.cwd, p)
    }

    fn readable(&self, p: &Path) -> Result<(), String> {
        crate::scope::check_read(&self.policy, &self.user, p)
    }

    fn writable(&self, p: &Path) -> Result<(), String> {
        crate::scope::check_write(&self.policy, &self.user, p)
    }

    pub fn set_cwd(&mut self, cwd: &Path) {
        self.cwd = cwd.to_path_buf();
    }

    pub fn exec(&mut self, line: &str) -> CliOut {
        let line = line.trim();
        if line.is_empty() {
            return out("");
        }
        // inside a Linux apps sandbox every line but `linux ...` and `exit` runs there (no shell: the words are the arguments)
        if let Some(name) = self.sandbox.clone() {
            let first = line.split_whitespace().next().unwrap_or("");
            if first == "exit" {
                self.sandbox = None;
                return out("left the sandbox; back in the IYAKKI terminal");
            }
            if first != "linux" && self.policy.cli == CliLevel::Full {
                return self.sandbox_line(&name, line);
            }
        }
        let stages = split_pipes(line);
        for st in &stages {
            let cmd = st.split_whitespace().next().unwrap_or("");
            match self.policy.cli {
                CliLevel::Off => return out("The terminal is disabled for your account."),
                CliLevel::Restricted if !RESTRICTED.contains(&cmd) => {
                    return out(if FULL_ONLY.contains(&cmd) { format!("{cmd}: not permitted for your role") } else { format!("{cmd}: command not found") })
                }
                _ => {}
            }
        }
        let first = stages[0].split_whitespace().next().unwrap_or("");
        if stages.len() > 1 || crate::textcmd::is_text_command(first) {
            return match self.pipeline(&stages, false) {
                Ok(t) => out(t),
                Err(e) => out(e),
            };
        }
        let (cmd, rest) = line.split_once(char::is_whitespace).map_or((line, ""), |(c, r)| (c, r.trim()));
        match self.dispatch(cmd, rest) {
            Ok(o) => o,
            Err(e) => out(format!("{cmd}: {e}")),
        }
    }

    /// For the assistant's `terminal` tool: only commands that read and print (nothing writes, runs or reaches the
    /// network), with the account's usual file permissions.
    pub fn run_readonly(&mut self, line: &str) -> Result<String, String> {
        let stages = split_pipes(line.trim());
        if self.policy.cli == CliLevel::Off {
            return Err("the terminal is disabled for your account".into());
        }
        self.pipeline(&stages, true)
    }

    /// An installed command-line tool typed at the terminal (full terminals only): it runs under the account's network
    /// policy, like the assistant's `run_command` but without asking, because the person typed it.
    fn managed(&self, cmd: &str, rest: &str) -> Option<Result<String, String>> {
        let svc = self.services.as_ref()?;
        if self.policy.cli != CliLevel::Full {
            return None;
        }
        if let Some(call) = self.ssh_call(&format!("{cmd} {rest}")) {
            return Some(call.run(None));
        }
        let extras = svc.clis.extras();
        if !extras.iter().any(|x| x.name == cmd) {
            return None;
        }
        Some((|| {
            let args = crate::exec::split_words(rest)?;
            let host = crate::exec::check_with(&extras, cmd, &args)?;
            if host != "-" && (!self.policy.allows_host(&host) || (host == "*" && !self.policy.net_hosts.iter().any(|h| h == "*"))) {
                return Err(if host == "*" { format!("`{cmd}` can reach any server, so only an account allowed every host may run it") } else { format!("your account may not reach {host}") });
            }
            // the person's home (what the login used), and for Copilot the token from their vault, for this one program only
            let home = crate::extagent::login::login_home(&self.user, svc.dir());
            let agent = if cmd == "copilot" { crate::extagent::COPILOT } else { "" };
            let env: Vec<(String, String)> = svc.agent_vault_token(&self.user, agent).map(|(k, t)| vec![(k.to_string(), t.to_string())]).unwrap_or_default();
            crate::exec::run_with_env(&extras, cmd, &args, &self.cwd, Some(home.as_path()), 120, Some(&svc.clis.bin_dir()), &env)
        })())
    }

    /// A line that is one SSH command (`ssh`, `scp`, `sftp`, `sshtrust`, `sshforget`, `sshhosts`), ready to run on another thread
    /// (they can take a long time, and the screen must not wait). `None` for anything else, and for terminals that are not full.
    pub fn ssh_call(&self, line: &str) -> Option<SshCall> {
        let svc = self.services.clone()?;
        let line = line.trim();
        let (cmd, rest) = line.split_once(char::is_whitespace).map_or((line, ""), |(c, r)| (c, r.trim()));
        if self.policy.cli != CliLevel::Full || !matches!(cmd, "ssh" | "scp" | "sftp" | "sshtrust" | "sshforget" | "sshhosts") || line.contains('|') {
            return None;
        }
        Some(SshCall { user: self.user.clone(), policy: self.policy.clone(), cwd: self.cwd.clone(), svc, cmd: cmd.to_string(), rest: rest.to_string() })
    }

    fn read_for_text(&self, f: &str) -> Result<String, String> {
        self.read_text_limited(f, 1 << 20)
    }

    /// `a | b | c`: each stage's output is the next one's input.
    fn pipeline(&mut self, stages: &[String], readonly: bool) -> Result<String, String> {
        const FIRST_OK: [&str; 8] = ["echo", "cat", "ls", "pwd", "whoami", "curl", "toollog", "policy"];
        let mut data: Option<String> = None;
        for (i, st) in stages.iter().enumerate() {
            let st = st.trim();
            let (cmd, rest) = st.split_once(char::is_whitespace).map_or((st, ""), |(c, r)| (c, r.trim()));
            if cmd.is_empty() {
                return Err("an empty command in the pipe".into());
            }
            let managed = if i == 0 && !readonly { self.managed(cmd, rest) } else { None };
            if crate::textcmd::is_text_command(cmd) {
                let words = crate::exec::split_words(rest).map_err(|e| format!("{cmd}: {e}"))?;
                let r = crate::textcmd::run(cmd, &words, data.as_deref(), &|f| self.read_for_text(f)).unwrap_or_else(|| Err("not a text command".into()));
                data = Some(r.map_err(|e| format!("{cmd}: {e}"))?);
            } else if let Some(r) = managed {
                data = Some(r.map_err(|e| format!("{cmd}: {e}"))?);
            } else if i == 0 && FIRST_OK.contains(&cmd) && !(readonly && matches!(cmd, "curl" | "toollog" | "policy")) {
                data = Some(if cmd == "echo" {
                    crate::exec::split_words(rest).map_err(|e| format!("echo: {e}"))?.join(" ") + "\n"
                } else {
                    self.dispatch(cmd, rest).map_err(|e| format!("{cmd}: {e}"))?.text
                });
            } else if readonly {
                return Err(format!("{cmd}: not available here (the assistant may use: {}, echo, cat, ls, pwd, whoami)", crate::textcmd::COMMANDS.join(", ")));
            } else {
                return Err(format!("{cmd}: cannot be used {}", if i == 0 { "in a pipe" } else { "after a pipe: it does not read input" }));
            }
        }
        Ok(data.unwrap_or_default())
    }

    fn skill_stores(&self) -> Result<(&Arc<SkillStore>, &Arc<Packages>), String> {
        match &self.skills {
            Some((s, p)) if self.policy.skills || self.policy.manage_skills => Ok((s, p)),
            Some(_) => Err("skills are not enabled for your account".into()),
            None => Err("skills are not available here".into()),
        }
    }

    fn scope_of(&self, shared: bool) -> Scope {
        if shared {
            Scope::Shared
        } else {
            Scope::User(self.user.clone())
        }
    }

    fn read_text(&self, file: &str) -> Result<String, String> {
        self.read_text_limited(file, crate::pkg::MAX_PACKAGE_BYTES as u64)
    }

    fn read_text_limited(&self, file: &str, max: u64) -> Result<String, String> {
        let p = self.resolve(file)?;
        self.readable(&p)?;
        let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
        if !meta.is_file() || meta.len() > max {
            return Err(format!("{file}: not a regular file under {} KiB", max / 1024));
        }
        String::from_utf8(std::fs::read(&p).map_err(|e| e.to_string())?).map_err(|_| format!("{file}: not a text file"))
    }

    /// `skill list | show <name> | add <file> [--shared] | remove <name> [--shared]`
    fn skill_cmd(&self, rest: &str) -> Result<String, String> {
        let (skills, pk) = self.skill_stores()?;
        let args: Vec<&str> = rest.split_whitespace().collect();
        let shared = args.contains(&"--shared");
        let args: Vec<&str> = args.into_iter().filter(|a| *a != "--shared").collect();
        let (can_shared, can_own) = (self.policy.manage_skills, self.policy.skills);
        match args.as_slice() {
            ["list"] | [] => {
                let all = skills.list(&self.user);
                if all.is_empty() {
                    return Ok("no skills yet. `skill add <file>` or `store install <file>`".into());
                }
                Ok(all.iter().map(|s| format!("{:<24} {:<7} {}", s.name, s.scope.label(), s.description)).collect::<Vec<_>>().join("\n"))
            }
            ["show", name] => {
                let s = skills.get(&self.scope_of(shared), name).or_else(|| skills.get(&Scope::Shared, name)).ok_or("no such skill")?;
                Ok(format!("{}\n{}\n\n{}", s.name, s.description, s.body))
            }
            ["add", file] => {
                let text = self.read_text(file)?;
                let s = skills.add(self.scope_of(shared), &text, can_shared, can_own).map_err(|e| e.to_string())?;
                Ok(format!("added skill {} ({})", s.name, s.scope.label()))
            }
            ["remove", name] => {
                pk.remove(&self.scope_of(shared), name, can_shared, can_own).map_err(|e| e.to_string())?;
                Ok(format!("removed skill {name}"))
            }
            _ => Err("usage: skill list | show <name> | add <file> [--shared] | remove <name> [--shared]".into()),
        }
    }

    fn read_bytes_limited(&self, file: &str, max: u64) -> Result<Vec<u8>, String> {
        let p = self.resolve(file)?;
        self.readable(&p)?;
        let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
        if !meta.is_file() || meta.len() > max {
            return Err(format!("{file}: not a regular file under {} KiB", max / 1024));
        }
        std::fs::read(&p).map_err(|e| e.to_string())
    }

    /// `tool list | show <name|file> | install <file> [--yes] [--shared] | remove <name> [--shared]
    /// | build <manifest.json> <module> [--key <keyfile>] [--out <file>] | keygen <keyfile>`
    fn tool_cmd(&self, rest: &str) -> Result<String, String> {
        const USAGE: &str = "usage: tool list | show <name|package file> | install <package file> [--yes] [--shared] | remove <name> [--shared] | build <manifest.json> <module.wasm> [--key <keyfile>] [--out <file>] | keygen <keyfile>";
        let (_, pk) = self.skill_stores()?;
        let mut words: Vec<&str> = Vec::new();
        let (mut shared, mut yes, mut key, mut outfile) = (false, false, None, None);
        let mut it = rest.split_whitespace();
        while let Some(w) = it.next() {
            match w {
                "--shared" => shared = true,
                "--yes" => yes = true,
                "--key" => key = Some(it.next().ok_or(USAGE)?),
                "--out" => outfile = Some(it.next().ok_or(USAGE)?),
                _ => words.push(w),
            }
        }
        let (can_shared, can_own) = (self.policy.manage_skills, self.policy.skills);
        match words.as_slice() {
            ["list"] | [] => {
                let mut rows = Vec::new();
                for (scope, tag) in [(self.scope_of(false), ""), (Scope::Shared, " (shared)")] {
                    for (n, r) in pk.list_tools(&scope) {
                        let access = pk.get_tool(&scope, &n).map_or("not loadable: changed or damaged".to_string(), |t| t.access_text(&self.user));
                        rows.push(format!("pkg_{:<24} {:<9} {:<9} {}", format!("{n}{tag}"), r.version, r.signer.as_deref().map_or("unsigned".to_string(), |k| format!("{}…", &k[..8.min(k.len())])), access));
                    }
                }
                Ok(if rows.is_empty() { "no tools installed. `tool install <file>`".into() } else { rows.join("\n") })
            }
            ["show", what] if what.contains(['.', '/']) => {
                let r = pk.review_tool(&String::from_utf8(self.read_bytes_limited(what, crate::pkg::MAX_TOOL_PACKAGE_BYTES as u64)?).map_err(|_| "not a text file")?, &self.scope_of(shared)).map_err(|e| e.to_string())?;
                Ok(r.summary(&self.user))
            }
            ["show", name] => {
                let (scope, t) = [self.scope_of(false), Scope::Shared].into_iter().find_map(|s| pk.get_tool(&s, name).map(|t| (s, t))).ok_or("no such tool")?;
                let b = &t.manifest.budget;
                Ok(format!("pkg_{} {} ({}) — {}\ncapabilities: {}\nbudget: {} instructions, {} MiB, {} s\narguments: {}", t.manifest.name, t.manifest.version, scope.label(), t.manifest.description, t.access_text(&self.user), b.fuel, b.memory_mb, b.seconds, t.manifest.schema))
            }
            ["install", file] => {
                let text = String::from_utf8(self.read_bytes_limited(file, crate::pkg::MAX_TOOL_PACKAGE_BYTES as u64)?).map_err(|_| "not a text file")?;
                let r = pk.review_tool(&text, &self.scope_of(shared)).map_err(|e| e.to_string())?;
                if !matches!(r.trust, crate::pkg::Trust::Trusted(_)) && !yes {
                    return Ok(format!("{}\n\nNothing installed. Read the capabilities above; if you accept them, run again with --yes.", r.summary(&self.user)));
                }
                pk.install_tool(&r, yes, can_shared, can_own).map_err(|e| e.to_string())?;
                Ok(format!("installed tool {} {} ({}); the assistant sees it as pkg_{}", r.tool.manifest.name, r.package.manifest.version, r.scope.label(), r.tool.manifest.name))
            }
            ["remove", name] => {
                pk.remove_tool(&self.scope_of(shared), name, can_shared, can_own).map_err(|e| e.to_string())?;
                Ok(format!("removed tool {name}"))
            }
            ["keygen", file] => {
                let p = self.resolve(file)?;
                self.writable(&p)?;
                if p.exists() {
                    return Err(format!("{file} exists already"));
                }
                use std::os::unix::fs::OpenOptionsExt;
                let k = crate::pkg::new_signing_key().map_err(|e| e.to_string())?;
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&p).map_err(|e| e.to_string())?;
                std::io::Write::write_all(&mut f, format!("{}\n", crate::pkg::signing_key_hex(&k)).as_bytes()).map_err(|e| e.to_string())?;
                Ok(format!("signing key written to {file} (keep it private)\npublisher key: {}", crate::pkg::public_key_hex(&k)))
            }
            ["build", manifest, module] => {
                let m = String::from_utf8(self.read_bytes_limited(manifest, 64 * 1024)?).map_err(|_| "the manifest is not text")?;
                let wasm = self.read_bytes_limited(module, crate::wtool::MAX_MODULE_BYTES as u64)?;
                let key = match key {
                    Some(f) => Some(crate::pkg::signing_key_from_hex(&self.read_text_limited(f, 1024)?).map_err(|e| e.to_string())?),
                    None => None,
                };
                let text = crate::pkg::build_tool(&m, &wasm, key.as_ref()).map_err(|e| e.to_string())?;
                let pkg = crate::pkg::parse(&text).map_err(|e| e.to_string())?;
                let name = outfile.map_or_else(|| format!("{}-{}.iyp", pkg.manifest.name, pkg.manifest.version), str::to_string);
                let p = self.resolve(&name)?;
                self.writable(&p)?;
                std::fs::write(&p, text).map_err(|e| e.to_string())?;
                Ok(format!("wrote {name} ({}, {})", pkg.manifest.sha256.get(..12).unwrap_or(""), if key.is_some() { "signed" } else { "not signed" }))
            }
            _ => Err(USAGE.into()),
        }
    }

    /// `store show <file> | install <file> [--yes] [--shared] | list | remove <name> | keys | trust <key> <label>`
    fn store_cmd(&self, rest: &str) -> Result<String, String> {
        let words: Vec<&str> = rest.split_whitespace().collect();
        // check, update, roll back, pin and remove for everything installed (the same layer as the STORE tile)
        if let Some(svc) = &self.services {
            if let Some(r) = crate::store::storecmd::run(svc, &self.policy, &self.user, &words) {
                return r;
            }
        }
        let (_, pk) = self.skill_stores()?;
        let shared = words.contains(&"--shared");
        let yes = words.contains(&"--yes");
        let args: Vec<&str> = words.iter().copied().filter(|a| !a.starts_with("--")).collect();
        let (can_shared, can_own) = (self.policy.manage_skills, self.policy.skills);
        match args.as_slice() {
            ["show", file] => Ok(pk.review(&self.read_text(file)?, &self.scope_of(shared)).map_err(|e| e.to_string())?.summary()),
            ["install", file] => {
                let r = pk.review(&self.read_text(file)?, &self.scope_of(shared)).map_err(|e| e.to_string())?;
                if !matches!(r.trust, crate::pkg::Trust::Trusted(_)) && !yes {
                    return Ok(format!("{}\n\nNothing installed. Read the instructions above; if you accept them, run again with --yes.", r.summary()));
                }
                let s = pk.install(&r, yes, can_shared, can_own).map_err(|e| e.to_string())?;
                Ok(format!("installed {} {} ({})", s.name, r.package.manifest.version, s.scope.label()))
            }
            ["list"] | [] => {
                let mut rows = pk.list(&self.scope_of(false));
                rows.extend(pk.list(&Scope::Shared).into_iter().map(|(n, r)| (format!("{n} (shared)"), r)));
                if rows.is_empty() {
                    return Ok("no packages installed".into());
                }
                Ok(rows.iter().map(|(n, r)| format!("{:<28} {:<9} {}", n, r.version, r.signer.as_deref().map_or("unsigned".to_string(), |k| format!("key {}…", &k[..12])))).collect::<Vec<_>>().join("\n"))
            }
            ["remove", name] => {
                pk.remove(&self.scope_of(shared), name, can_shared, can_own).map_err(|e| e.to_string())?;
                Ok(format!("removed {name}"))
            }
            ["keys"] => {
                let k = pk.trusted_keys();
                Ok(if k.is_empty() { "no trusted publishers".into() } else { k.iter().map(|(k, l)| format!("{l}  {k}")).collect::<Vec<_>>().join("\n") })
            }
            ["trust", key, label @ ..] => {
                if !self.policy.manage_skills {
                    return Err("only people who manage shared skills can trust a publisher".into());
                }
                pk.trust_key(key, &label.join(" ")).map_err(|e| e.to_string())?;
                Ok("publisher trusted".into())
            }
            _ => Err(format!("usage: store show <file> | install <file> [--yes] [--shared] | list | remove <name> | keys | trust <key> <label>\nlifecycle: {}", crate::store::storecmd::USAGE)),
        }
    }

    fn dispatch(&mut self, cmd: &str, rest: &str) -> Result<CliOut, String> {
        Ok(match cmd {
            "help" => out(if self.policy.cli == CliLevel::Full {
                "commands: help whoami policy echo clear pwd cd ls cat mkdir write run skill tool store toollog curl grep sed tr cut sort uniq wc head tail date calendar alarm clis linux ssh scp sftp sshtrust sshhosts sshforget detect net scan poweroff reboot"
            } else {
                "commands: help whoami policy echo clear pwd cd ls cat run skill tool store toollog curl grep sed tr cut sort uniq wc head tail date calendar alarm clis"
            }),
            "whoami" => out(&self.user),
            "echo" => out(rest),
            "clear" => CliOut { text: String::new(), action: Action::Clear },
            "pwd" => out(self.cwd.display().to_string()),
            "policy" => {
                let p = &self.policy;
                out(format!(
                    "apps: {}\ncli: {:?}  fuel: {}  mem: {} MiB  disk: {} MiB\nread: {}\nwrite: {}\nnet: {}\nmcp: {}",
                    p.apps.iter().map(|a| a.id()).collect::<Vec<_>>().join(", "),
                    p.cli,
                    p.max_fuel,
                    p.max_memory_mb,
                    p.vfs_quota_mb,
                    p.read_scopes(&self.user).join(" "),
                    p.write_scopes(&self.user).join(" "),
                    p.net_hosts.join(" "),
                    p.mcp_servers.join(" ")
                ))
            }
            "cd" => {
                let p = self.resolve(if rest.is_empty() { "/" } else { rest })?;
                self.readable(&p)?;
                if !p.is_dir() {
                    return Err("not a directory".into());
                }
                self.cwd = p;
                out("")
            }
            "ls" => {
                let p = self.resolve(if rest.is_empty() { "." } else { rest })?;
                self.readable(&p)?;
                let mut names: Vec<String> = std::fs::read_dir(&p)
                    .map_err(|e| e.to_string())?
                    .flatten()
                    .map(|e| {
                        let d = e.file_type().is_ok_and(|t| t.is_dir());
                        format!("{}{}", e.file_name().to_string_lossy(), if d { "/" } else { "" })
                    })
                    .collect();
                names.sort();
                out(names.join("\n"))
            }
            "cat" => {
                let p = self.resolve(rest)?;
                self.readable(&p)?;
                let meta = std::fs::metadata(&p).map_err(|e| e.to_string())?;
                if !meta.is_file() || meta.len() > (1 << 20) {
                    return Err("not a regular file under 1 MiB".into());
                }
                out(String::from_utf8_lossy(&std::fs::read(&p).map_err(|e| e.to_string())?).into_owned())
            }
            "mkdir" => {
                let p = self.resolve(rest)?;
                self.writable(&p)?;
                std::fs::create_dir_all(&p).map_err(|e| e.to_string())?;
                out("")
            }
            "write" => {
                let (path, text) = rest.split_once(char::is_whitespace).ok_or("usage: write <path> <text>")?;
                let p = self.resolve(path)?;
                self.writable(&p)?;
                std::fs::write(&p, text.trim()).map_err(|e| e.to_string())?;
                out("")
            }
            "run" => {
                let mut it = rest.split_whitespace();
                let file = it.next().ok_or("usage: run <module.wasm|.wat> [entry]")?;
                let entry = it.next().unwrap_or("_start");
                let p = self.resolve(file)?;
                self.readable(&p)?;
                let bytes = std::fs::read(&p).map_err(|e| e.to_string())?;
                out(self.host.run_wasm(&self.user, &self.policy, &bytes, entry)?)
            }
            "skill" => out(self.skill_cmd(rest)?),
            "tool" => out(self.tool_cmd(rest)?),
            "curl" => {
                let svc = self.services.as_ref().ok_or("the network is not available here")?;
                let r = crate::exec::parse_curl(rest)?;
                let host = iyakki_sec::url_host(&r.url).ok_or("not an http(s) address")?;
                if !self.policy.allows_host(&host) {
                    return Err(format!("your account may not reach {host}"));
                }
                let reply = svc.quick_upstream().request(&r.method, &r.url, &r.headers, r.body.as_bytes());
                let shown = match &reply {
                    Ok(x) if r.head_only => format!("HTTP {}", x.status),
                    Ok(x) => format!("{}{}", if (200..300).contains(&x.status) { String::new() } else { format!("HTTP {}\n", x.status) }, String::from_utf8_lossy(&iyakki_proxy::sanitize(&x.body.iter().copied().take(64 * 1024).collect::<Vec<u8>>()))),
                    Err(e) => e.clone(),
                };
                svc.toollog.record(svc.tool_log_days(), &self.user, "curl", &format!("{} {}", r.method, r.url), reply.is_ok(), &shown);
                out(shown)
            }
            "linux" => out(self.linux_cmd(rest)?.text),
            "clis" => {
                let svc = self.services.as_ref().ok_or("the tool manager is not available here")?;
                let w: Vec<&str> = rest.split_whitespace().collect();
                match w.as_slice() {
                    [] | ["list"] => {
                        let inst = svc.clis.installed();
                        out(crate::clis::CATALOG.iter().map(|d| match inst.iter().find(|i| i.id == d.id) {
                            Some(i) => format!("{:<10} installed {}{}   {}", d.id, i.version, if i.verified { " (checksum verified)" } else { "" }, d.bins.iter().map(|b| b.0).filter(|n| !n.ends_with("_completer")).collect::<Vec<_>>().join(" ")),
                            None => format!("{:<10} not installed   {}", d.id, d.about),
                        }).collect::<Vec<_>>().join("\n"))
                    }
                    ["install", id] => {
                        if !self.policy.manage_settings || self.policy.cli != CliLevel::Full {
                            return Err("only administrators may install command-line tools".into());
                        }
                        svc.clis.install(id, &self.user)?;
                        out(format!("installing {id} in the background; `clis` shows how far it is, and the CLI tile's CLIs tab shows progress"))
                    }
                    ["remove", id] | ["remove", id, "yes"] => {
                        if !self.policy.manage_settings || self.policy.cli != CliLevel::Full {
                            return Err("only administrators may remove command-line tools".into());
                        }
                        // a runtime (Node.js, Python) that installed tools use is removed only when asked twice
                        if w.len() == 2 {
                            if let Some(warning) = svc.clis.uninstall_warning(id) {
                                return Err(format!("{warning} Run `clis remove {id} yes` to remove it anyway."));
                            }
                        }
                        svc.clis.remove(id)?;
                        out(format!("removed {id}"))
                    }
                    _ => return Err("usage: clis | clis install <id> | clis remove <id> [yes]".into()),
                }
            }
            "calendar" | "alarm" => {
                let svc = self.services.as_ref().ok_or("the calendar is not available here")?;
                if !self.policy.apps.contains(&iyakki_sec::policy::App::Calendar) {
                    return Err("your account may not use the calendar".into());
                }
                let (zone, now) = (crate::clock::Zone::system(), crate::clock::now_unix());
                let w = crate::exec::split_words(rest)?;
                let w: Vec<&str> = w.iter().map(String::as_str).collect();
                let cal = &svc.calendar;
                let u = self.user.as_str();
                out(match (cmd, w.as_slice()) {
                    ("calendar", [] | ["list"]) => {
                        let from = zone.local_to_unix(zone.local(now).year, zone.local(now).month, zone.local(now).day, 0, 0, 0);
                        let mut lines: Vec<String> = cal.events_in(u, from, from + 7 * 86_400, &zone).iter().map(|o| crate::calendar::occurrence_line(&zone, o)).collect();
                        lines.extend(cal.alarms_in(u, from, from + 7 * 86_400, &zone).iter().map(|a| format!("alarm [{}] {}  {}{}", a.id, crate::calendar::when_text(&zone, a.at, false), a.label, if a.enabled { "" } else { " (off)" })));
                        if lines.is_empty() { "nothing in the next 7 days".to_string() } else { lines.join("\n") }
                    }
                    ("calendar", ["add", title, date, more @ ..]) => {
                        // calendar add "Dentist" 2026-10-12 [15:30] [daily|weekly|monthly|yearly]
                        let (time, rest) = match more.first() { Some(t) if t.contains(':') => (Some(*t), &more[1..]), _ => (None, more) };
                        let when = match time { Some(t) => format!("{date} {t}"), None => date.to_string() };
                        let (start, all_day) = crate::calendar::parse_local(&zone, &when)?;
                        let repeat = crate::calendar::Repeat::parse(rest.first().copied().unwrap_or("once"))?;
                        let e = cal.add_event(u, title, start, if all_day { 0 } else { 60 }, all_day, repeat, "", "user", None)?;
                        format!("added [{}] {} — {}", e.id, e.title, crate::calendar::when_text(&zone, e.start, e.all_day))
                    }
                    ("calendar", ["rm", id]) => {
                        cal.remove_event(u, id)?;
                        format!("deleted {id}")
                    }
                    ("alarm", [] | ["list"]) => {
                        let all = cal.alarms(u);
                        if all.is_empty() { "no alarms".to_string() } else { all.iter().map(|a| crate::calendar::alarm_line(&zone, a, now)).collect::<Vec<_>>().join("\n") }
                    }
                    ("alarm", ["add", time, more @ ..]) => {
                        // alarm add 07:30 [daily|weekdays|weekends|mon,wed|once] [label…]
                        let (h, m) = crate::calendar::parse_hm(time).ok_or("the time must look like 07:30 or 7:30am")?;
                        let (days, label) = match more.first().map(|d| crate::calendar::parse_days(d)) { Some(Ok(d)) => (d, more[1..].join(" ")), _ => (0, more.join(" ")) };
                        let a = cal.add_alarm(u, &label, h, m, days, now, &zone, "user")?;
                        format!("alarm set: {}", crate::calendar::alarm_line(&zone, &a, now))
                    }
                    ("alarm", ["rm", id]) => {
                        cal.remove_alarm(u, id)?;
                        format!("deleted {id}")
                    }
                    ("calendar", _) => return Err("usage: calendar | calendar add <title> <YYYY-MM-DD> [HH:MM] [daily|weekly|monthly|yearly] | calendar rm <id>".into()),
                    _ => return Err("usage: alarm | alarm add <HH:MM> [daily|weekdays|weekends|mon,wed|once] [label] | alarm rm <id>".into()),
                })
            }
            "toollog" => {
                let svc = self.services.as_ref().ok_or("the log is not available here")?;
                let everyone = rest == "all";
                if everyone && !self.policy.manage_settings {
                    return Err("only administrators can read everyone's entries".into());
                }
                let days = svc.tool_log_days();
                let rows = svc.toollog.list(days, if everyone { None } else { Some(self.user.as_str()) });
                if rows.is_empty() {
                    return Ok(out(format!("nothing in the last {days} days. (what the assistant's tools did is kept {days} days)")));
                }
                let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
                out(rows.iter().take(50).map(|e| {
                    let ago = now.saturating_sub(e.ts);
                    let when = if ago < 3600 { format!("{}m ago", ago / 60) } else if ago < 86_400 { format!("{}h ago", ago / 3600) } else { format!("{}d ago", ago / 86_400) };
                    format!("{when:>7}  {}{}  {} {}  → {}", if everyone { format!("{}  ", e.user) } else { String::new() }, e.tool, if e.ok { "ok " } else { "ERR" }, e.what, e.result.lines().next().unwrap_or(""))
                }).collect::<Vec<_>>().join("\n"))
            }
            "store" => out(self.store_cmd(rest)?),
            "scan" => {
                // a few addresses at a time; a whole network is the NET SCAN tile's job (it shows progress and can stop)
                let targets = if rest.is_empty() { return Err("usage: scan <address | 192.168.1.10-20>   (a whole network: use the NET SCAN tile)".into()) } else { iyakki_netscan::net::parse_targets(rest)? };
                if targets.len() > 16 {
                    return Err(format!("{} addresses is a lot for the terminal: use the NET SCAN tile", targets.len()));
                }
                if let Some(ip) = targets.iter().find(|ip| !self.policy.allows_host(&ip.to_string())) {
                    return Err(format!("your account may not scan {ip}"));
                }
                let hosts = iyakki_netscan::scan_blocking(targets, iyakki_netscan::Options { announce_wait: None, ..Default::default() });
                if hosts.is_empty() {
                    return Ok(out("nothing answered"));
                }
                let mut lines = Vec::new();
                for h in &hosts {
                    lines.push(format!("{}{}", h.ip, h.mac.as_deref().map(|m| format!("  {m}")).unwrap_or_default()));
                    for s in &h.services {
                        lines.push(format!("  {:>5}/{}  {} — {}{}{}", s.port, s.proto, s.kind.label(), s.name, if s.models.is_empty() { String::new() } else { format!(" [{}]", s.models.iter().take(5).cloned().collect::<Vec<_>>().join(", ")) }, s.risk.as_ref().map(|r| format!("  WARNING: {r}")).unwrap_or_default()));
                    }
                }
                out(lines.join("\n"))
            }
            "detect" => out(self.host.detect_env()),
            "net" => out(self.host.net_status()),
            "poweroff" | "reboot" => {
                if !self.policy.power {
                    return Err("not permitted for your role".into());
                }
                if cmd == "poweroff" {
                    CliOut { text: "powering off…".into(), action: Action::PowerOff }
                } else {
                    CliOut { text: "rebooting…".into(), action: Action::Reboot }
                }
            }
            other => match self.managed(other, rest) {
                Some(r) => out(r?),
                None => return Err("command not found".into()),
            },
        })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use iyakki_sec::policy::Role;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct MockHost {
        pub calls: Mutex<Vec<String>>,
    }
    impl HostApi for MockHost {
        fn detect_env(&self) -> String {
            "baremetal".into()
        }
        fn net_status(&self) -> String {
            "eth0 Ethernet".into()
        }
        fn run_wasm(&self, user: &str, p: &Policy, wasm: &[u8], entry: &str) -> Result<String, String> {
            self.calls.lock().unwrap().push(format!("run {user} fuel={} {}b {entry}", p.max_fuel, wasm.len()));
            Ok("ok".into())
        }
        fn power_off(&self) {}
        fn reboot(&self) {}
    }

    fn sandbox_dirs() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("iyakki-cli-{}-{:?}", std::process::id(), std::thread::current().id())).join("x");
        let _ = std::fs::remove_dir_all(root.parent().unwrap());
        let (mine, other) = (root.join("alice"), root.join("bob"));
        std::fs::create_dir_all(&mine).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(mine.join("a.txt"), "mine").unwrap();
        std::fs::write(other.join("secret.txt"), "bobs").unwrap();
        std::os::unix::fs::symlink(&other, mine.join("sneaky")).unwrap();
        (mine, other)
    }

    fn cli_with(role: Role, mine: &Path) -> (Cli, Arc<MockHost>) {
        let mut p = role.template();
        if role != Role::Admin {
            p.fs_read = vec![mine.display().to_string()];
            p.fs_write = if role == Role::User || role == Role::Operator { vec![mine.display().to_string()] } else { vec![] };
        }
        let h = Arc::new(MockHost::default());
        let mut c = Cli::new("alice", p, h.clone());
        c.cwd = mine.to_path_buf();
        (c, h)
    }

    #[test]
    fn file_commands_stay_in_scope_even_through_symlinks() {
        let (mine, other) = sandbox_dirs();
        let (mut c, _) = cli_with(Role::User, &mine);
        assert_eq!(c.exec("ls").text, "a.txt\nsneaky"); // symlinks are listed but never followed
        assert_eq!(c.exec("cat a.txt").text, "mine");
        assert!(c.exec("cat sneaky/secret.txt").text.contains("permission denied"));
        assert!(c.exec("ls sneaky").text.contains("permission denied"));
        assert!(c.exec(&format!("cat {}/secret.txt", other.display())).text.contains("permission denied"));
        assert!(c.exec("cat ../bob/secret.txt").text.contains("permission denied"));
        assert!(c.exec("cd /etc").text.contains("permission denied"));
        assert_eq!(c.exec("pwd").text, mine.display().to_string());
    }

    #[test]
    fn restricted_vs_full_commands() {
        let (mine, _) = sandbox_dirs();
        let (mut user, _) = cli_with(Role::User, &mine);
        assert!(user.exec("detect").text.contains("not permitted"));
        assert!(user.exec("poweroff").text.contains("not permitted"));
        assert!(user.exec("frobnicate").text.contains("not found"));
        assert!(user.exec("mkdir x").text.contains("not permitted"));
        let (mut op, _) = cli_with(Role::Operator, &mine);
        assert_eq!(op.exec("detect").text, "baremetal");
        assert_eq!(op.exec("net").text, "eth0 Ethernet");
        assert_eq!(op.exec("mkdir proj").text, "");
        assert_eq!(op.exec("write proj/n.txt hello world").text, "");
        assert_eq!(op.exec("cat proj/n.txt").text, "hello world");
        assert_eq!(op.exec("poweroff").action, Action::PowerOff);
        let (mut guest, _) = cli_with(Role::Guest, &mine);
        assert!(guest.exec("ls").text.contains("disabled"));
        assert_eq!(guest.exec("poweroff").action, Action::None);
    }

    #[test]
    fn write_scope_is_enforced() {
        let (mine, other) = sandbox_dirs();
        let (mut op, _) = cli_with(Role::Operator, &mine);
        assert!(op.exec(&format!("write {}/x.txt pwn", other.display())).text.contains("permission denied"));
        assert!(!other.join("x.txt").exists());
        assert!(op.exec("write sneaky/x.txt pwn").text.contains("permission denied"));
        assert!(!other.join("x.txt").exists());
    }

    #[test]
    fn run_uses_policy_limits() {
        let (mine, _) = sandbox_dirs();
        std::fs::write(mine.join("m.wat"), "(module)").unwrap();
        let (mut c, h) = cli_with(Role::User, &mine);
        assert_eq!(c.exec("run m.wat main").text, "ok");
        assert_eq!(h.calls.lock().unwrap()[0], format!("run alice fuel={} 8b main", Role::User.template().max_fuel));
        assert!(c.exec("run ../bob/x.wat").text.contains("permission denied"));
        assert_eq!(h.calls.lock().unwrap().len(), 1, "denied run never reaches the host");
    }

    #[test]
    fn curl_follows_the_host_policy_sends_the_method_and_headers_and_is_logged() {
        use std::sync::Mutex;
        struct Echo(Mutex<Vec<String>>);
        impl iyakki_proxy::Upstream for Echo {
            fn request(&self, m: &str, u: &str, h: &[(String, String)], b: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
                self.0.lock().unwrap().push(format!("{m} {u} {h:?} {}", String::from_utf8_lossy(b)));
                Ok(iyakki_proxy::UpstreamResponse { status: if u.contains("missing") { 404 } else { 200 }, body: b"hello".to_vec() })
            }
        }
        let d = std::env::temp_dir().join(format!("iyakki-cli-curl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let net = Arc::new(Echo(Mutex::new(vec![])));
        let svc = crate::services::Services::open(&d, net.clone(), net.clone()).unwrap();
        let (mine, _) = sandbox_dirs();
        let (c, _) = cli_with(Role::User, &mine);
        let mut c = c.with_services(svc.clone());
        c.policy.net_hosts = vec!["example.com".into()];
        assert_eq!(c.exec("curl https://example.com/a").text, "hello");
        assert!(c.exec("curl https://evil.test/").text.contains("may not reach evil.test"));
        assert_eq!(c.exec("curl -X PUT -H 'X-A: b' -d 'v=1' https://example.com/p").text, "hello");
        assert!(c.exec("curl https://example.com/missing").text.starts_with("HTTP 404"));
        assert_eq!(c.exec("curl -I https://example.com/a").text, "HTTP 200");
        let sent = net.0.lock().unwrap().clone();
        assert_eq!(sent.len(), 4, "the refused address never reached the network: {sent:?}");
        assert!(sent[1].starts_with("PUT https://example.com/p") && sent[1].contains("X-A") && sent[1].ends_with("v=1"), "{sent:?}");
        assert!(svc.toollog.list(3, Some("alice")).iter().any(|e| e.tool == "curl" && e.what == "PUT https://example.com/p"));
    }

    #[test]
    fn calendar_and_alarm_commands_use_the_same_store_as_the_app_and_the_chat() {
        struct NoNet;
        impl iyakki_proxy::Upstream for NoNet {
            fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
                Err("offline".into())
            }
        }
        let d = std::env::temp_dir().join(format!("iyakki-cli-cal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let net = Arc::new(NoNet);
        let svc = crate::services::Services::open(&d, net.clone(), net).unwrap();
        let (mine, _) = sandbox_dirs();
        let (c, _) = cli_with(Role::User, &mine);
        let mut c = c.with_services(svc.clone());
        assert_eq!(c.exec("calendar").text, "nothing in the next 7 days");
        let r = c.exec("calendar add \"Team lunch\" 2030-06-03 12:30 weekly").text;
        assert!(r.starts_with("added [") && r.contains("Team lunch") && r.contains("Mon 3 Jun 2030 12:30"), "{r}");
        assert!(c.exec("calendar add Holiday 2030-06-04").text.contains("Tue 4 Jun 2030"));
        assert_eq!(svc.calendar.events("alice").len(), 2, "the app and the chat read what the terminal wrote");
        assert!(c.exec("calendar add x notadate").text.contains("is not a date"));
        assert!(c.exec("calendar bogus").text.contains("usage"));
        let a = c.exec("alarm add 6:45 weekdays Wake up").text;
        assert!(a.starts_with("alarm set:") && a.contains("06:45") && a.contains("Wake up") && a.contains("weekdays"), "{a}");
        assert!(c.exec("alarm add 7:15").text.contains("once"));
        assert!(c.exec("alarm").text.contains("Wake up"));
        let id = svc.calendar.alarms("alice")[0].id.clone();
        assert!(c.exec(&format!("alarm rm {id}")).text.starts_with("deleted"));
        assert!(c.exec("alarm rm nope").text.contains("no alarm"));
        assert!(c.exec("alarm add 25:00").text.contains("time must look like"));
        // not for an account without the app
        let (c2, _) = cli_with(Role::User, &mine);
        let mut c2 = c2.with_services(svc);
        c2.policy.apps.remove(&iyakki_sec::policy::App::Calendar);
        assert!(c2.exec("calendar").text.contains("may not use the calendar"));
    }

    #[test]
    fn installed_command_line_tools_run_at_the_terminal_in_pipes_and_under_the_network_policy_and_clis_lists_them() {
        struct NoNet;
        impl iyakki_proxy::Upstream for NoNet {
            fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
                Err("offline".into())
            }
        }
        let d = std::env::temp_dir().join(format!("iyakki-cli-managed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let net = Arc::new(NoNet);
        let svc = crate::services::Services::open(&d, net.clone(), net).unwrap();
        // a tool "installed" earlier: its program, its link and its entry
        use std::os::unix::fs::PermissionsExt;
        let root = d.join("cli");
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let prog = root.join("pkgs/kubectl/v1.31.0/kubectl");
        std::fs::create_dir_all(prog.parent().unwrap()).unwrap();
        std::fs::write(&prog, "#!/bin/sh\necho \"pods: web db cache\"\necho \"args: $@\"\n").unwrap();
        std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(&prog, root.join("bin/kubectl")).unwrap();
        std::fs::write(root.join("state.json"), r#"{"kubectl":{"id":"kubectl","version":"v1.31.0","reported":"","at":1,"bins":["kubectl"],"verified":true,"by":"root"}}"#).unwrap();
        let (mine, _) = sandbox_dirs();
        let (c, _) = cli_with(Role::Admin, &mine);
        let mut c = c.with_services(svc.clone());
        let l = c.exec("clis").text;
        assert!(l.contains("kubectl") && l.contains("installed v1.31.0") && l.contains("checksum verified") && l.contains("helm") && l.contains("not installed"), "{l}");
        let r = c.exec("kubectl get pods").text;
        assert!(r.contains("pods: web db cache") && r.contains("args: get pods"), "{r}");
        assert_eq!(c.exec("kubectl get pods | grep db | wc -l").text.trim(), "1", "installed tools feed pipes");
        // a restricted terminal, or an account that may not reach every host, does not run it
        let (u, _) = cli_with(Role::User, &mine);
        let mut u = u.with_services(svc.clone());
        assert!(u.exec("kubectl get pods").text.contains("command not found"));
        c.policy.net_hosts = vec!["example.com".into()];
        assert!(c.exec("kubectl get pods").text.contains("can reach any server"));
        assert!(u.exec("clis install helm").text.contains("only administrators"));
        assert!(c.exec("clis bogus").text.contains("usage"));
        assert!(c.exec("clis remove nothing").text.contains("not in the list"));
        // removal through the terminal
        let mut c = cli_with(Role::Admin, &mine).0.with_services(svc);
        assert_eq!(c.exec("clis remove kubectl").text, "removed kubectl");
        assert!(c.exec("kubectl get pods").text.contains("command not found"));
    }

    #[test]
    fn toollog_shows_my_entries_and_everyones_only_to_administrators() {
        let (mine, _) = sandbox_dirs();
        let d = std::env::temp_dir().join(format!("iyakki-cli-toollog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        struct NoNet;
        impl iyakki_proxy::Upstream for NoNet {
            fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
                Err("offline".into())
            }
        }
        let net = Arc::new(NoNet);
        let svc = crate::services::Services::open(&d, net.clone(), net).unwrap();
        svc.toollog.record(3, "alice", "run_command", "run `aws s3 ls`", true, "bucket-a\nbucket-b");
        svc.toollog.record(3, "bob", "http_request", "POST https://x.test/", false, "HTTP 500");
        let (c, _) = cli_with(Role::User, &mine);
        let mut c = c.with_services(svc.clone());
        let t = c.exec("toollog").text;
        assert!(t.contains("run `aws s3 ls`") && t.contains("bucket-a") && !t.contains("x.test"), "{t}");
        assert!(c.exec("toollog all").text.contains("only administrators"));
        let (a, _) = cli_with(Role::Admin, &mine);
        let mut a = a.with_services(svc);
        let t = a.exec("toollog all").text;
        assert!(t.contains("alice") && t.contains("bob") && t.contains("ERR"), "{t}");
    }

    #[test]
    fn policy_and_help_text() {
        let (mine, _) = sandbox_dirs();
        let (mut c, _) = cli_with(Role::User, &mine);
        let p = c.exec("policy").text;
        assert!(p.contains("cli: Restricted") && p.contains("apps: chat, rag, mcp, cli"));
        assert!(!c.exec("help").text.contains("poweroff"));
        assert_eq!(c.exec("clear").action, Action::Clear);
        assert_eq!(c.exec("   ").text, "");
    }

    #[test]
    fn skill_and_store_commands_review_before_installing() {
        use crate::embed::HashEmbed;
        use crate::kb::KnowledgeBase;
        use crate::pkg;
        let (mine, _) = sandbox_dirs();
        let kb = Arc::new(Mutex::new(KnowledgeBase::in_memory(Arc::new(HashEmbed::default()))));
        let skills = Arc::new(SkillStore::new(&mine.parent().unwrap().join("skills"), kb));
        let pk = Arc::new(Packages::new(&mine.parent().unwrap().join("pkg"), skills.clone()));
        let (c, _) = cli_with(Role::User, &mine);
        let mut c = c.with_skills(skills, pk);
        let md = "---\nname: tidy\ndescription: Keep answers short\n---\nBe brief.\n";
        std::fs::write(mine.join("tidy.md"), md).unwrap();
        std::fs::write(mine.join("tidy.iyp"), pkg::build_skill(md, "1.0.0", "MIT", None).unwrap()).unwrap();

        // review first: nothing is installed without --yes, and the instructions are shown
        let r = c.exec("store install tidy.iyp").text;
        assert!(r.contains("not signed") && r.contains("Be brief.") && r.contains("--yes"), "{r}");
        assert!(c.exec("skill list").text.contains("no skills"));
        assert!(c.exec("store install tidy.iyp --yes").text.contains("installed tidy 1.0.0"));
        assert!(c.exec("skill list").text.contains("tidy"));
        assert!(c.exec("store list").text.contains("tidy") && c.exec("store list").text.contains("unsigned"));
        assert!(c.exec("store install tidy.iyp --yes").text.contains("not newer"));
        // plain skill files and removal
        assert!(c.exec("skill remove tidy").text.contains("removed"));
        assert!(c.exec("skill add tidy.md").text.contains("added skill tidy"));
        assert!(c.exec("skill show tidy").text.contains("Be brief."));
        // a plain user may not write shared skills or trust publishers; files outside the scope are unreadable
        assert!(c.exec("skill add tidy.md --shared").text.contains("may not change shared"));
        assert!(c.exec("store trust abc x").text.contains("only people who manage"));
        assert!(c.exec("skill add ../bob/secret.txt").text.contains("permission denied"));
        // without the services the commands say so
        let (mut bare, _) = cli_with(Role::User, &mine);
        assert!(bare.exec("skill list").text.contains("not available"));
    }

    #[test]
    fn tool_commands_build_sign_review_install_list_show_and_remove() {
        use crate::embed::HashEmbed;
        use crate::kb::KnowledgeBase;
        use crate::wtool::tests::{manifest, ECHO_WAT};
        let (mine, _) = sandbox_dirs();
        let kb = Arc::new(Mutex::new(KnowledgeBase::in_memory(Arc::new(HashEmbed::default()))));
        let skills = Arc::new(SkillStore::new(&mine.parent().unwrap().join("skills"), kb));
        let pk = Arc::new(Packages::new(&mine.parent().unwrap().join("pkg"), skills.clone()));
        let (c, _) = cli_with(Role::User, &mine);
        let mut c = c.with_skills(skills, pk);
        std::fs::write(mine.join("shout.json"), manifest("shout", &["net:example.com"], serde_json::json!({}))).unwrap();
        std::fs::write(mine.join("shout.wat"), ECHO_WAT).unwrap();

        // build: unsigned, then signed with a generated key
        assert!(c.exec("tool build shout.json shout.wat").text.contains("wrote shout-1.0.0.iyp") && c.exec("tool build shout.json shout.wat").text.contains("not signed"));
        let k = c.exec("tool keygen me.key").text;
        assert!(k.contains("publisher key: "), "{k}");
        assert!(c.exec("tool keygen me.key").text.contains("exists already"));
        let b = c.exec("tool build shout.json shout.wat --key me.key --out signed.iyp").text;
        assert!(b.contains("wrote signed.iyp") && b.contains("signed") && !b.contains("not signed"), "{b}");
        assert!(std::fs::read_to_string(mine.join("signed.iyp")).unwrap().contains("\nsigner: "));
        // build checks its inputs
        std::fs::write(mine.join("bad.json"), "{\"name\":\"x\"}").unwrap();
        assert!(c.exec("tool build bad.json shout.wat").text.contains("manifest"));
        assert!(c.exec("tool build shout.json missing.wasm").text.contains("No such file"));
        assert!(c.exec("tool build ../bob/x.json shout.wat").text.contains("permission denied"));

        // review first: nothing is installed without --yes, and the capabilities are shown
        let r = c.exec("tool install signed.iyp").text;
        assert!(r.contains("unknown key") && r.contains("connect to example.com") && r.contains("--yes"), "{r}");
        assert!(c.exec("tool list").text.contains("no tools"));
        assert!(c.exec("tool show signed.iyp").text.contains("capabilities: connect to example.com"));
        let i = c.exec("tool install signed.iyp --yes").text;
        assert!(i.contains("installed tool shout 1.0.0") && i.contains("pkg_shout"), "{i}");
        let l = c.exec("tool list").text;
        assert!(l.contains("pkg_shout") && l.contains("connect to example.com"), "{l}");
        assert!(c.exec("tool show shout").text.contains("arguments:"));
        assert!(c.exec("tool install signed.iyp --yes").text.contains("not newer"));
        assert!(c.exec("tool install signed.iyp --yes --shared").text.contains("may not change shared"));
        // a package changed after it was built is refused
        let text = std::fs::read_to_string(mine.join("signed.iyp")).unwrap().replace("net:example.com", "net:evil.example");
        std::fs::write(mine.join("evil.iyp"), text).unwrap();
        assert!(c.exec("tool install evil.iyp --yes").text.contains("hash does not match"));
        // the skill commands do not take tool packages
        assert!(c.exec("store show signed.iyp").text.contains("tool package"));
        // removal
        assert!(c.exec("tool remove shout").text.contains("removed tool shout"));
        assert!(c.exec("tool list").text.contains("no tools") && c.exec("tool show shout").text.contains("no such tool"));
        assert!(c.exec("tool nonsense").text.contains("usage: tool"));
        let (mut bare, _) = cli_with(Role::User, &mine);
        assert!(bare.exec("tool list").text.contains("not available"));
    }

    #[test]
    fn scan_is_for_full_terminals_stays_local_and_finds_services() {
        let (mine, _) = sandbox_dirs();
        let (mut c, _) = cli_with(Role::User, &mine);
        assert!(c.exec("scan 127.0.0.1").text.contains("not permitted"), "{}", c.exec("scan 127.0.0.1").text);
        let (mut c, _) = cli_with(Role::Admin, &mine);
        assert!(c.exec("scan").text.contains("usage"));
        assert!(c.exec("scan 8.8.8.8").text.contains("not on the local network"));
        assert!(c.exec("scan 10.0.0.0/24").text.contains("NET SCAN tile"));
        // something listening on this machine's loopback address is found by its port
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        drop(l);
        assert!(c.exec("scan 127.0.0.1").text.contains("127.0.0.1") || c.exec("scan 127.0.0.1").text == "nothing answered");
        // the account's host policy applies
        let mut p = Role::Admin.template();
        p.net_hosts = vec!["other.example".into()];
        let mut c2 = Cli::new("alice", p, Arc::new(MockHost::default()));
        assert!(c2.exec("scan 127.0.0.1").text.contains("may not scan"));
    }
}
