//! The SSH client: `ssh`, `scp` and `sftp` for the assistant and the person, with the safety the other network tools have.
//!
//! The program itself is not ours: it is OpenSSH on a computer, or Dropbear's `dbclient` (also installed as `ssh`, with its
//! `scp`) on the IYAKKI images. What is ours is everything around it:
//!
//! * **Options are allow-listed.** Anything that runs a program on this machine (`ProxyCommand`, `LocalCommand`, `-J`, `-S`,
//!   an `Include` or `-F` file, a batch file with `!`), forwards ports, goes to the background or changes where host keys are
//!   kept is refused before anything starts. `BatchMode` is always on: nothing is ever asked at a prompt and no password
//!   is ever put on a command line.
//! * **Host keys** live in one `known_hosts` per person under the data folder (folder 0700, file 0600). A host that is not in it is
//!   refused ("trust it first"); the person sees its fingerprint in the CLI tile and confirms it ([`probe`] fetches the key,
//!   [`KnownHosts::trust`] records it). A key that differs from the recorded one is a hard refusal.
//! * **Private keys** are kept in the person's vault as `ssh.<name>`. A run writes a key to a 0600 file in a 0700 folder that
//!   is removed when the run ends (also on errors). The assistant can only name a key as `-i uvault:ssh.<name>`; a passphrase
//!   comes from `ssh.<name>.passphrase` in the vault and reaches `ssh-keygen` through an askpass helper, never an argument.
//! * **Saved connections** ([`Connections`]) hold name, host, port, user, a key name and an optional default command; no
//!   secret is in the file.
//!
//! There is no pseudo-terminal in this build (see docs/ALPHA-SCOPE.md): runs are non-interactive, with output as it arrives.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use crate::exec::Sink;

/// Vault names of private keys start with this.
pub const KEY_PREFIX: &str = "ssh.";
const MAX_BATCH: usize = 64 * 1024;

// ------------------------------------------------------------------ the client program

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flavor {
    OpenSsh,
    Dropbear,
}

impl Flavor {
    pub fn name(self) -> &'static str {
        match self {
            Flavor::OpenSsh => "OpenSSH",
            Flavor::Dropbear => "Dropbear",
        }
    }
}

/// An SSH client found on the machine.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Client {
    pub flavor: Flavor,
    /// `ssh`, or `dbclient` when there is no `ssh`.
    pub path: PathBuf,
    /// `9.6p1`, `2026.91`.
    pub version: String,
    /// What `-V` printed (first line).
    pub banner: String,
    search: String,
}

/// An executable file called `name` in one of the folders of `search`.
pub fn find_program(name: &str, search: &str) -> Option<PathBuf> {
    search.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join(name)).find(|p| p.is_file() && p.metadata().is_ok_and(|m| m.permissions().mode() & 0o111 != 0))
}

/// The version out of a `-V` banner: `OpenSSH_9.6p1 Ubuntu-3, OpenSSL …` gives `9.6p1`, `Dropbear v2026.91` gives `2026.91`.
pub fn parse_banner(banner: &str) -> Option<(Flavor, String)> {
    let first = banner.lines().map(str::trim).find(|l| !l.is_empty())?;
    if let Some(i) = first.find("OpenSSH_") {
        let tok = first[i..].split([',', ' ']).next().unwrap_or("");
        let v = tok.rsplit('_').next().unwrap_or("");
        return (!v.is_empty()).then(|| (Flavor::OpenSsh, v.to_string()));
    }
    if first.contains("Dropbear") {
        let v = first.split_whitespace().find(|w| w.trim_start_matches('v').starts_with(|c: char| c.is_ascii_digit()))?;
        return Some((Flavor::Dropbear, v.trim_start_matches('v').to_string()));
    }
    None
}

/// Looks for `ssh` (then `dbclient`) in `search` and asks it for its version. `None`: no SSH client here.
pub fn detect(search: &str) -> Option<Client> {
    for name in ["ssh", "dbclient"] {
        let Some(path) = find_program(name, search) else { continue };
        let Ok(out) = crate::clis::run_quiet(&path.display().to_string(), &["-V"], 5) else { continue };
        if let Some((flavor, version)) = parse_banner(&out) {
            let banner = out.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(120).collect();
            return Some(Client { flavor, path, version, banner, search: search.to_string() });
        }
    }
    None
}

impl Client {
    pub fn tool(&self, name: &str) -> Option<PathBuf> {
        find_program(name, &self.search)
    }

    /// The commands that exist next to the client (`sftp` is missing with Dropbear).
    pub fn commands(&self) -> Vec<String> {
        let mut v = vec!["ssh".to_string()];
        v.extend(["scp", "sftp", "ssh-keygen"].iter().filter(|n| self.tool(n).is_some()).map(|n| n.to_string()));
        if self.flavor == Flavor::Dropbear && self.tool("dropbearkey").is_some() {
            v.push("dropbearkey".into());
        }
        v
    }
}

/// How to get an SSH client, in words, for an operating system (`std::env::consts::OS`).
pub fn install_hint(os: &str) -> String {
    match os {
        "macos" => "macOS has it built in (/usr/bin/ssh); nothing to install.".into(),
        "windows" => "Windows: Settings > Apps > Optional features > Add a feature > OpenSSH Client (or, as an administrator, `Add-WindowsCapability -Online -Name OpenSSH.Client~~~~0.0.1.0`).".into(),
        _ => "Linux: install the package yourself (IYAKKI never runs sudo): Debian/Ubuntu `sudo apt install openssh-client`, Fedora `sudo dnf install openssh-clients`, Arch `sudo pacman -S openssh`, Alpine `apk add openssh-client`. The IYAKKI images already include Dropbear's client (`dbclient`, also named `ssh`).".into(),
    }
}

// ------------------------------------------------------------------ the person's SSH folder

/// Everything the SSH client needs from the person's data: where its files live and how to read a vault secret.
#[derive(Clone)]
pub struct Env {
    /// `<data>/ssh/<user>`: `home/.ssh/known_hosts`, `connections.json`, `keys/<name>.pub`.
    pub dir: PathBuf,
    pub user: String,
    search: String,
    secret: Arc<dyn Fn(&str) -> Result<Zeroizing<String>, String> + Send + Sync>,
}

impl Env {
    pub fn new(dir: &Path, user: &str, search: &str, secret: Arc<dyn Fn(&str) -> Result<Zeroizing<String>, String> + Send + Sync>) -> Env {
        Env { dir: dir.into(), user: user.into(), search: search.into(), secret }
    }

    /// The environment of one signed-in person: keys come from their vault.
    pub fn for_services(svc: &Arc<crate::services::Services>, user: &str) -> Env {
        let s = svc.clone();
        let u = user.to_string();
        let secret: Arc<dyn Fn(&str) -> Result<Zeroizing<String>, String> + Send + Sync> = Arc::new(move |name| s.user_get(&u, name).map_err(|e| format!("could not read `{name}` from your vault: {e}")));
        let safe: String = user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-')).collect();
        Env::new(&svc.dir().join("ssh").join(safe), user, &svc.clis.search_path(), secret)
    }

    pub fn home(&self) -> PathBuf {
        self.dir.join("home")
    }

    pub fn known_hosts(&self) -> KnownHosts {
        KnownHosts::new(&self.home().join(".ssh").join("known_hosts"))
    }

    pub fn connections(&self) -> Connections {
        Connections::new(&self.dir.join("connections.json"))
    }

    pub fn client(&self) -> Option<Client> {
        detect(&self.search)
    }

    fn secret(&self, name: &str) -> Result<Zeroizing<String>, String> {
        (self.secret)(name)
    }

    /// The public half of a key made here, for the person to copy to a server.
    pub fn public_key(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.dir.join("keys").join(format!("{name}.pub"))).ok().map(|s| s.trim().to_string())
    }
}

fn private_dir(p: &Path) -> Result<(), String> {
    std::fs::create_dir_all(p).map_err(|e| format!("could not create {}: {e}", p.display()))?;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())
}

fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    if let Some(d) = path.parent() {
        private_dir(d)?;
    }
    let tmp = path.with_extension("tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    f.write_all(data).map_err(|e| e.to_string())?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

// ------------------------------------------------------------------ what a command line asks for

/// Who is asking. The assistant gets the strictest rules.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Assistant,
    /// Typed by the person themselves (the terminal, the Connections tab).
    Person,
}

/// A local file named in a command, for the caller to check against the person's file scope.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Local {
    pub path: String,
    pub write: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Plan {
    pub program: &'static str,
    pub host: String,
    pub port: u16,
    pub user: Option<String>,
    /// The checked options, ready to pass on.
    pub opts: Vec<String>,
    /// After the options: for `ssh` the host and the command; for `scp` the files; for `sftp` the host.
    pub operands: Vec<String>,
    /// Vault names of the keys asked for.
    pub keys: Vec<String>,
    pub locals: Vec<Local>,
    /// The `sftp -b` file.
    pub batch: Option<String>,
    pub agent: bool,
}

const ALLOWED_O: &str = "Port User ConnectTimeout ServerAliveInterval ServerAliveCountMax IdentityFile IdentitiesOnly BatchMode StrictHostKeyChecking PreferredAuthentications PubkeyAuthentication HostKeyAlgorithms KexAlgorithms Ciphers MACs Compression TCPKeepAlive AddressFamily LogLevel RequestTTY";

pub fn valid_key_name(n: &str) -> bool {
    n.len() > KEY_PREFIX.len() && n.starts_with(KEY_PREFIX) && !n.ends_with(".passphrase") && crate::vault::valid_secret_name(n)
}

fn valid_host(h: &str) -> bool {
    !h.is_empty() && h.len() <= 253 && !h.starts_with(['-', '.']) && h.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
}

fn valid_user(u: &str) -> bool {
    !u.is_empty() && u.len() <= 64 && !u.starts_with('-') && u.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

fn parse_port(v: &str) -> Result<u16, String> {
    v.trim().parse::<u16>().ok().filter(|p| *p > 0).ok_or_else(|| format!("`{v}` is not a port number (1 to 65535)"))
}

fn number(v: &str, max: u32, what: &str) -> Result<u32, String> {
    v.trim().parse::<u32>().ok().filter(|n| *n <= max).ok_or_else(|| format!("{what} must be a number up to {max}"))
}

fn algo_list(v: &str) -> bool {
    !v.is_empty() && v.len() <= 400 && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ',' | '@' | '.' | '+' | '^' | '-' | '_'))
}

/// `[user@]host` split and checked.
fn split_target(t: &str) -> Result<(Option<String>, String), String> {
    let (user, host) = match t.rsplit_once('@') {
        Some((u, h)) => (Some(u.to_string()), h),
        None => (None, t),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if let Some(u) = &user {
        if !valid_user(u) {
            return Err(format!("`{u}` is not a user name this client accepts"));
        }
    }
    if !valid_host(host) {
        return Err(format!("`{host}` is not a host name this client accepts (letters, digits, `.`, `-`, and an address; no ssh:// links)"));
    }
    Ok((user, host.to_ascii_lowercase()))
}

struct State {
    mode: Mode,
    port: Option<u16>,
    user: Option<String>,
    keys: Vec<String>,
    opts: Vec<String>,
    agent: bool,
}

impl State {
    fn set_port(&mut self, v: &str) -> Result<(), String> {
        let p = parse_port(v)?;
        if self.port.is_some_and(|q| q != p) {
            return Err("give the port once".into());
        }
        self.port = Some(p);
        Ok(())
    }

    fn set_user(&mut self, v: &str) -> Result<(), String> {
        if !valid_user(v) {
            return Err(format!("`{v}` is not a user name this client accepts"));
        }
        if self.user.as_deref().is_some_and(|u| u != v) {
            return Err("give the user once".into());
        }
        self.user = Some(v.to_string());
        Ok(())
    }

    /// An identity: `uvault:ssh.NAME` for anyone; a path only when the person typed it themselves.
    fn identity(&mut self, v: &str) -> Result<String, String> {
        if let Some(name) = v.strip_prefix("uvault:") {
            if !valid_key_name(name) {
                return Err(format!("`{name}` is not a key name: keys in your vault are called ssh.<name> (Settings > Vault, or Generate key in the CLI tile's Connections)"));
            }
            if !self.keys.iter().any(|k| k == name) {
                self.keys.push(name.to_string());
            }
            return Ok(v.to_string());
        }
        if self.mode == Mode::Assistant {
            return Err("keys for the assistant must be in your vault: use -i uvault:ssh.<name> (key files in folders are not used)".into());
        }
        if v.is_empty() || v.starts_with('-') || v.contains('\0') {
            return Err("that key file name is not accepted".into());
        }
        Ok(v.to_string())
    }

    fn option(&mut self, spec: &str) -> Result<(), String> {
        let spec = spec.trim();
        let (name, val) = match spec.find(|c: char| c == '=' || c.is_whitespace()) {
            Some(i) => (&spec[..i], spec[i + 1..].trim().trim_start_matches('=').trim().trim_matches('"')),
            None => (spec, ""),
        };
        let lower = name.to_ascii_lowercase();
        let yes_no = |v: &str| matches!(v.to_ascii_lowercase().as_str(), "yes" | "no");
        let bad = |what: &str| Err(format!("ssh option {name} {what}"));
        let pass = |this: &mut State, canon: &str, v: &str| this.opts.extend(["-o".to_string(), format!("{canon}={v}")]);
        match lower.as_str() {
            "proxycommand" | "localcommand" | "permitlocalcommand" | "knownhostscommand" | "proxyjump" | "include" | "match" | "remotecommand" | "pkcs11provider" | "securitykeyprovider" | "identityagent" | "xauthlocation" | "forwardx11trusted" | "tunnel" | "tunneldevice" | "controlpath" | "controlmaster" | "controlpersist" | "userknownhostsfile" | "globalknownhostsfile" => {
                return bad(if matches!(lower.as_str(), "userknownhostsfile" | "globalknownhostsfile") { "is not allowed: host keys are kept by IYAKKI" } else { "can run a program on this machine or change how the connection is made, and is not allowed" });
            }
            "port" => {
                self.set_port(val)?;
                pass(self, "Port", val);
            }
            "user" => {
                self.set_user(val)?;
                pass(self, "User", val);
            }
            "connecttimeout" | "serveraliveinterval" | "serveralivecountmax" => {
                number(val, 3600, name)?;
                pass(self, if lower == "connecttimeout" { "ConnectTimeout" } else if lower == "serveraliveinterval" { "ServerAliveInterval" } else { "ServerAliveCountMax" }, val);
            }
            "identityfile" => {
                let v = self.identity(val)?;
                self.opts.extend(["-o".to_string(), format!("IdentityFile={v}")]);
            }
            "identitiesonly" | "compression" | "tcpkeepalive" | "pubkeyauthentication" => {
                if !yes_no(val) {
                    return bad("takes yes or no");
                }
                let canon = match lower.as_str() { "identitiesonly" => "IdentitiesOnly", "compression" => "Compression", "tcpkeepalive" => "TCPKeepAlive", _ => "PubkeyAuthentication" };
                pass(self, canon, &val.to_ascii_lowercase());
            }
            "batchmode" => {
                if !val.eq_ignore_ascii_case("yes") {
                    return bad("is always yes here: nothing can be typed at a prompt");
                }
            }
            "stricthostkeychecking" => {
                if !val.eq_ignore_ascii_case("yes") {
                    return bad("is always yes here: new host keys are trusted from the CLI tile after you check the fingerprint");
                }
            }
            "preferredauthentications" => {
                if !val.eq_ignore_ascii_case("publickey") {
                    return bad("can only be publickey: passwords are never typed on a command line");
                }
            }
            "hostkeyalgorithms" | "kexalgorithms" | "ciphers" | "macs" => {
                if !algo_list(val) {
                    return bad("has a value that is not a list of algorithm names");
                }
                let canon = match lower.as_str() { "hostkeyalgorithms" => "HostKeyAlgorithms", "kexalgorithms" => "KexAlgorithms", "ciphers" => "Ciphers", _ => "MACs" };
                pass(self, canon, val);
            }
            "addressfamily" => {
                if !matches!(val, "any" | "inet" | "inet6") {
                    return bad("takes any, inet or inet6");
                }
                pass(self, "AddressFamily", val);
            }
            "loglevel" => {
                if !matches!(val.to_ascii_uppercase().as_str(), "QUIET" | "FATAL" | "ERROR" | "INFO" | "VERBOSE" | "DEBUG" | "DEBUG1" | "DEBUG2" | "DEBUG3") {
                    return bad("takes QUIET, FATAL, ERROR, INFO, VERBOSE or DEBUG");
                }
                pass(self, "LogLevel", &val.to_ascii_uppercase());
            }
            "requesttty" => {
                if !matches!(val.to_ascii_lowercase().as_str(), "no" | "auto") {
                    return bad("takes no or auto (there is no terminal here)");
                }
                pass(self, "RequestTTY", &val.to_ascii_lowercase());
            }
            "forwardagent" => match val.to_ascii_lowercase().as_str() {
                "no" => {}
                "yes" if self.mode == Mode::Person => self.agent = true,
                _ => return bad("(agent forwarding) is off; the assistant never forwards an agent, and you can turn it on yourself in the terminal with -A"),
            },
            "forwardx11" => {
                if !val.eq_ignore_ascii_case("no") {
                    return bad("is off");
                }
            }
            _ => return Err(format!("ssh option {name} is not allowed here (allowed: {ALLOWED_O})")),
        }
        Ok(())
    }
}

fn refused_flag(c: char, program: &str) -> String {
    let why = match c {
        'F' => "another configuration file can run programs",
        'J' => "a jump host or proxy program is not allowed",
        'S' => "a program to run instead of ssh is not allowed",
        'D' | 's' => "a different program to talk to is not allowed",
        'L' | 'R' | 'W' | 'w' => "port forwarding and tunnels are not available",
        'f' | 'N' | 'M' | 'O' | 'g' => "background connections and sharing are not available",
        'y' => "accepting an unknown host key is done from the CLI tile after checking its fingerprint",
        'A' => "agent forwarding is not allowed for the assistant (type it yourself in the terminal)",
        'E' => "writing a log file is not allowed",
        _ => "this option is not supported here",
    };
    format!("{program} -{c}: {why}")
}

/// Short options: those without a value, those with one.
fn tables(program: &str) -> (&'static str, &'static str) {
    match program {
        "ssh" => ("46aCqTtvnxA", "plicmo"),
        "scp" => ("346BCpqrTv", "cilopP"),
        _ => ("1246Cafpqrv", "bBcilopPR"),
    }
}

/// Reads the options and operands of an `ssh`, `scp` or `sftp` command line and refuses what is not allowed. Nothing is run
/// and no file is touched.
pub fn plan(program: &str, args: &[String], mode: Mode) -> Result<Plan, String> {
    let program: &'static str = match program {
        "ssh" => "ssh",
        "scp" => "scp",
        "sftp" => "sftp",
        other => return Err(format!("`{other}` is not an SSH program")),
    };
    if args.iter().any(|a| a.contains('\0') || a.contains('\n')) {
        return Err("an argument holds a line break or a zero byte".into());
    }
    let (bools, valued) = tables(program);
    let mut st = State { mode, port: None, user: None, keys: vec![], opts: vec![], agent: false };
    let mut batch: Option<String> = None;
    let mut operands: Vec<String> = Vec::new();
    let mut host_seen = false;
    let mut after_dd = false;
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        let is_opt = !after_dd && a.starts_with('-') && a.len() > 1;
        if !is_opt {
            if program == "ssh" && host_seen {
                // the first word after the host starts the remote command: all that follows belongs to the other machine
                operands.extend(args[i..].iter().cloned());
                break;
            }
            if program != "ssh" && a.starts_with('-') && !after_dd && a.len() == 1 {
                return Err(format!("{program}: a lone - is not accepted"));
            }
            operands.push(a.to_string());
            host_seen = true;
            i += 1;
            continue;
        }
        if a == "--" {
            after_dd = true;
            i += 1;
            continue;
        }
        // a cluster of short options such as -4q, or one with its value (-p22, -p 22)
        let chars: Vec<char> = a[1..].chars().collect();
        let mut k = 0;
        while k < chars.len() {
            let c = chars[k];
            if bools.contains(c) {
                match c {
                    'A' => {
                        if mode == Mode::Assistant {
                            return Err(refused_flag('A', program));
                        }
                        st.agent = true;
                    }
                    _ => st.opts.push(format!("-{c}")),
                }
                k += 1;
            } else if valued.contains(c) {
                let rest: String = chars[k + 1..].iter().collect();
                let v = if !rest.is_empty() {
                    rest
                } else {
                    i += 1;
                    args.get(i).cloned().ok_or_else(|| format!("{program} -{c} needs a value"))?
                };
                match (program, c) {
                    ("ssh", 'p') | (_, 'P') => {
                        st.set_port(&v)?;
                        st.opts.extend([format!("-{c}"), v]);
                    }
                    ("ssh", 'l') => {
                        st.set_user(&v)?;
                        st.opts.extend(["-l".to_string(), v]);
                    }
                    (_, 'i') => {
                        let t = st.identity(&v)?;
                        st.opts.extend(["-i".to_string(), t]);
                    }
                    (_, 'o') => st.option(&v)?,
                    (_, 'c') | ("ssh", 'm') => {
                        if !algo_list(&v) {
                            return Err(format!("{program} -{c}: not a list of algorithm names"));
                        }
                        st.opts.extend([format!("-{c}"), v]);
                    }
                    (_, 'l') | (_, 'B') | (_, 'R') => {
                        number(&v, 1 << 30, &format!("{program} -{c}"))?;
                        st.opts.extend([format!("-{c}"), v]);
                    }
                    ("sftp", 'b') => {
                        if v == "-" {
                            return Err("sftp -b - (commands from standard input) is not available".into());
                        }
                        batch = Some(v);
                    }
                    _ => return Err(refused_flag(c, program)),
                }
                k = chars.len();
            } else {
                return Err(refused_flag(c, program));
            }
        }
        i += 1;
    }
    if program != "ssh" && operands.iter().any(|o| o.contains("://") || o.starts_with('-')) {
        return Err(format!("{program}: links like scp:// and names that start with - are not accepted (write ./name for a file)"));
    }
    let mut locals = Vec::new();
    let (target, operands_out): (String, Vec<String>) = match program {
        "ssh" => {
            let t = operands.first().cloned().ok_or("ssh needs a host: ssh [user@]host [command]")?;
            (t, operands)
        }
        "sftp" => {
            if operands.len() != 1 {
                return Err("sftp takes one operand: [user@]host (put the directory in the batch file)".into());
            }
            let t = operands[0].split_once(':').filter(|(h, _)| !h.contains('/') && !h.starts_with('[')).map_or(operands[0].clone(), |(h, _)| h.to_string());
            if operands[0].starts_with('[') && !operands[0].ends_with(']') {
                return Err("sftp takes [user@]host".into());
            }
            (t, operands)
        }
        _ => {
            if operands.len() < 2 {
                return Err("scp needs at least two operands: a source and a destination, one of them host:path".into());
            }
            let mut remote: Option<String> = None;
            let last = operands.len() - 1;
            for (n, op) in operands.iter().enumerate() {
                match scp_remote(op) {
                    Some(t) => {
                        if remote.as_deref().is_some_and(|r| r != t) {
                            return Err("scp between two remote machines is not available: copy through this machine in two steps".into());
                        }
                        remote = Some(t);
                    }
                    None => locals.push(Local { path: op.clone(), write: n == last }),
                }
            }
            (remote.ok_or("scp needs one side on another machine: host:path (write ./name for a local file with a colon in it)")?, operands)
        }
    };
    let (user, host) = split_target(&target)?;
    if let Some(u) = &user {
        st.set_user(u)?;
    }
    Ok(Plan { program, host, port: st.port.unwrap_or(22), user: st.user, opts: st.opts, operands: operands_out, keys: st.keys, locals, batch, agent: st.agent })
}

/// `[user@]host` of an scp operand that names another machine (`host:path`, `user@host:path`, `[::1]:path`), or `None` for a
/// local file (a name with a `/` before any `:` is local).
fn scp_remote(op: &str) -> Option<String> {
    if let Some(rest) = op.strip_prefix('[') {
        let (h, tail) = rest.split_once(']')?;
        return tail.starts_with(':').then(|| h.to_string());
    }
    let colon = op.find(':')?;
    if op[..colon].contains('/') {
        return None;
    }
    Some(op[..colon].to_string())
}

// ------------------------------------------------------------------ host keys

/// One line of a `known_hosts` file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostKey {
    /// `host`, or `[host]:port` when the port is not 22.
    pub pattern: String,
    pub kind: String,
    pub b64: String,
}

impl HostKey {
    /// `SHA256:…`, as OpenSSH prints it.
    pub fn fingerprint(&self) -> String {
        let blob = crate::models::b64_decode(&self.b64).unwrap_or_default();
        let digest = Sha256::digest(&blob);
        format!("SHA256:{}", crate::story::base64_encode(&digest).trim_end_matches('='))
    }

    fn line(&self) -> String {
        format!("{} {} {}", self.pattern, self.kind, self.b64)
    }
}

pub fn pattern(host: &str, port: u16) -> String {
    if port == 22 { host.to_ascii_lowercase() } else { format!("[{}]:{port}", host.to_ascii_lowercase()) }
}

pub struct KnownHosts {
    path: PathBuf,
}

impl KnownHosts {
    pub fn new(path: &Path) -> KnownHosts {
        KnownHosts { path: path.into() }
    }

    pub fn read(&self) -> Vec<HostKey> {
        let text = std::fs::read_to_string(&self.path).unwrap_or_default();
        text.lines().filter_map(parse_known_line).collect()
    }

    pub fn for_host(&self, host: &str, port: u16) -> Vec<HostKey> {
        let want = pattern(host, port);
        self.read().into_iter().filter(|k| k.pattern == want).collect()
    }

    fn write(&self, keys: &[HostKey]) -> Result<(), String> {
        let mut text = String::new();
        for k in keys {
            text.push_str(&k.line());
            text.push('\n');
        }
        write_private(&self.path, text.as_bytes())
    }

    /// Records these keys for the host, replacing what was there (the person has confirmed them).
    pub fn trust(&self, host: &str, port: u16, keys: &[HostKey]) -> Result<(), String> {
        if keys.is_empty() {
            return Err("there is no host key to trust".into());
        }
        let want = pattern(host, port);
        let mut all: Vec<HostKey> = self.read().into_iter().filter(|k| k.pattern != want).collect();
        for k in keys {
            let k = HostKey { pattern: want.clone(), ..k.clone() };
            if !all.contains(&k) {
                all.push(k);
            }
        }
        self.write(&all)
    }

    pub fn forget(&self, host: &str, port: u16) -> Result<usize, String> {
        let want = pattern(host, port);
        let all = self.read();
        let keep: Vec<HostKey> = all.iter().filter(|k| k.pattern != want).cloned().collect();
        let gone = all.len() - keep.len();
        if gone > 0 {
            self.write(&keep)?;
        }
        Ok(gone)
    }
}

fn parse_known_line(l: &str) -> Option<HostKey> {
    let l = l.trim();
    if l.is_empty() || l.starts_with('#') || l.starts_with('|') || l.starts_with('@') {
        return None;
    }
    let mut it = l.split_whitespace();
    let (hosts, kind, b64) = (it.next()?, it.next()?, it.next()?);
    if !kind.starts_with("ssh-") && !kind.starts_with("ecdsa-") && !kind.starts_with("sk-") {
        return None;
    }
    crate::models::b64_decode(b64)?;
    Some(HostKey { pattern: hosts.split(',').next()?.to_ascii_lowercase(), kind: kind.into(), b64: b64.into() })
}

/// Fetches the keys a server presents, for the person to compare with a fingerprint they know. It connects and leaves without
/// logging in (`ssh-keyscan` when there is one; otherwise the client itself in a throw-away home, with no key to offer).
/// Nothing is recorded: [`KnownHosts::trust`] is the person's decision.
pub fn probe(client: &Client, host: &str, port: u16) -> Result<Vec<HostKey>, String> {
    let (h, p) = (split_target(host)?.1, port.to_string());
    let want = pattern(&h, port);
    let found: Vec<HostKey> = if let Some(scan) = client.tool("ssh-keyscan") {
        let out = std::process::Command::new(&scan).args(["-T", "10", "-p", &p, "-t", "ed25519,ecdsa,rsa", "--", &h]).stdin(std::process::Stdio::null()).output().map_err(|e| format!("could not run ssh-keyscan: {e}"))?;
        String::from_utf8_lossy(&out.stdout).lines().filter_map(parse_known_line).collect()
    } else {
        let tmp = TempDir::new("probe")?;
        let kh = match client.flavor {
            Flavor::OpenSsh => {
                let kh = tmp.path().join("kh");
                let args = ["-F", "/dev/null", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=accept-new", "-o", "HashKnownHosts=no", "-o", "GlobalKnownHostsFile=/dev/null", "-o", "ConnectTimeout=10", "-o", "IdentityFile=/dev/null", "-o", "IdentitiesOnly=yes", "-o", "PreferredAuthentications=publickey", "-o", "ClearAllForwardings=yes", "-o"];
                let mut cmd = Command::new(&client.path);
                cmd.args(args).arg(format!("UserKnownHostsFile={}", kh.display())).args(["-p", &p, "--", &h, "true"]);
                let _ = cmd_run(cmd, &tmp.path().join("home"), 20);
                kh
            }
            Flavor::Dropbear => {
                let home = tmp.path().join("home");
                private_dir(&home.join(".ssh"))?;
                let mut cmd = Command::new(&client.path);
                cmd.args(["-y", "-o", "BatchMode=yes", "-p", &p, &h, "true"]);
                let _ = cmd_run(cmd, &home, 20);
                home.join(".ssh").join("known_hosts")
            }
        };
        KnownHosts::new(&kh).read()
    };
    let keys: Vec<HostKey> = found.into_iter().map(|k| HostKey { pattern: want.clone(), ..k }).collect();
    if keys.is_empty() {
        return Err(format!("{h}:{port} sent no host key (is an SSH server running there, and is the address right?)"));
    }
    Ok(keys)
}

/// Runs a prepared command with a cut-down environment and a time limit, output ignored.
fn cmd_run(mut cmd: Command, home: &Path, seconds: u64) -> Result<String, String> {
    private_dir(home)?;
    cmd.env_clear().env("HOME", home);
    if let Ok(p) = std::env::var("PATH") {
        cmd.env("PATH", p);
    }
    crate::exec::run_prepared(cmd, "ssh", seconds, None)
}

// ------------------------------------------------------------------ temporary files

/// A private folder under the system's temp folder, removed (with everything in it) when dropped.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Result<TempDir, String> {
        let mut r = [0u8; 8];
        iyakki_sec::random_fill(&mut r).map_err(|e| e.to_string())?;
        let name: String = r.iter().map(|b| format!("{b:02x}")).collect();
        let p = std::env::temp_dir().join(format!("iyakki-ssh-{tag}-{name}"));
        std::fs::create_dir(&p).map_err(|e| format!("could not make a private folder: {e}"))?;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
        Ok(TempDir(p))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Whether private-key text needs a passphrase (an old PEM header, or an OpenSSH key whose cipher is not `none`).
pub fn key_is_encrypted(text: &str) -> bool {
    if text.contains("ENCRYPTED") {
        return true;
    }
    let Some(a) = text.find("-----BEGIN OPENSSH PRIVATE KEY-----") else { return false };
    let body: String = text[a + 35..].lines().take_while(|l| !l.starts_with("-----")).collect();
    let Some(raw) = crate::models::b64_decode(&body) else { return false };
    let magic = b"openssh-key-v1\0";
    if !raw.starts_with(magic) || raw.len() < magic.len() + 4 {
        return false;
    }
    let n = u32::from_be_bytes([raw[15], raw[16], raw[17], raw[18]]) as usize;
    raw.get(19..19 + n).is_some_and(|c| c != b"none")
}

/// Writes the keys a run asked for into `dir` (mode 0600), returns the file for each vault name. An encrypted key is opened with
/// its passphrase from the vault (through `ssh-keygen` and an askpass helper) so the client never needs to ask.
fn write_keys(env: &Env, client: &Client, names: &[String], dir: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut out = Vec::new();
    for (n, name) in names.iter().enumerate() {
        let secret = env.secret(name)?;
        let bytes: Vec<u8> = match secret.strip_prefix("b64:") {
            Some(b) => crate::models::b64_decode(b).ok_or_else(|| format!("the key `{name}` in your vault is damaged"))?,
            None => {
                let mut t = secret.trim_end().to_string();
                t.push('\n');
                t.into_bytes()
            }
        };
        let path = dir.join(format!("key{n}"));
        let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&path).map_err(|e| e.to_string())?;
        f.write_all(&bytes).map_err(|e| e.to_string())?;
        drop(f);
        if key_is_encrypted(&String::from_utf8_lossy(&bytes)) {
            let pass = env.secret(&format!("{name}.passphrase")).map_err(|_| format!("the key `{name}` has a passphrase. Store it in your vault as `{name}.passphrase` (Settings > Vault); it is never typed on a command line"))?;
            let keygen = client.tool("ssh-keygen").ok_or_else(|| format!("the key `{name}` has a passphrase and this machine's SSH client cannot open such keys (no ssh-keygen). Make a key without a passphrase with Generate key"))?;
            let askpass = dir.join("askpass");
            std::fs::write(&askpass, "#!/bin/sh\nprintf '%s\\n' \"$IYAKKI_SSH_PASSPHRASE\"\n").map_err(|e| e.to_string())?;
            std::fs::set_permissions(&askpass, std::fs::Permissions::from_mode(0o700)).map_err(|e| e.to_string())?;
            let mut cmd = Command::new(keygen);
            cmd.args(["-p", "-N", "", "-f"]).arg(&path).env_clear().env("SSH_ASKPASS", &askpass).env("SSH_ASKPASS_REQUIRE", "force").env("IYAKKI_SSH_PASSPHRASE", pass.as_str()).env("PATH", env_path());
            crate::exec::run_prepared(cmd, "ssh-keygen", 20, None).map_err(|_| format!("the passphrase in your vault for `{name}` did not open the key"))?;
        }
        out.push((name.clone(), path));
    }
    Ok(out)
}

fn env_path() -> String {
    std::env::var("PATH").unwrap_or_default()
}

// ------------------------------------------------------------------ running

/// Local files named by the command, resolved and checked by the caller against the person's file scope: given the path as
/// written and whether it is written to, the real path to use.
pub type LocalCheck<'a> = &'a dyn Fn(&str, bool) -> Result<PathBuf, String>;

fn changed_text(host: &str, port: u16) -> String {
    format!("HOST KEY CHANGED for {}: the key it presents is not the one you trusted. This can mean that someone is intercepting the connection. Nothing was sent. If you know the server was reinstalled, use Forget host key in the CLI tile (Connections), then trust the new key after comparing its fingerprint.", pattern(host, port))
}

fn unknown_text(host: &str, port: u16) -> String {
    format!("{} has not been trusted yet. IYAKKI never accepts a new host key by itself: open the CLI tile, Connections, choose Trust host key, compare the fingerprint with the one the server's owner gave you, and confirm (in the terminal: `sshtrust {host}{}`).", pattern(host, port), if port == 22 { String::new() } else { format!(" {port}") })
}

/// A batch file for the assistant may only name remote commands and local files inside its scope; `!` and `lcd`-style commands
/// that run or change things here are refused.
fn check_batch(text: &str, local: LocalCheck) -> Result<(), String> {
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches(['-', '@']);
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let words = crate::exec::split_words(line).map_err(|e| format!("sftp batch: {e}"))?;
        let cmd = words[0].as_str();
        match cmd {
            "ls" | "cd" | "pwd" | "mkdir" | "rmdir" | "rm" | "rename" | "ln" | "symlink" | "chmod" | "chown" | "chgrp" | "df" | "stat" | "version" | "help" | "bye" | "exit" | "quit" | "realpath" | "lstat" | "progress" => {}
            "get" | "reget" | "put" | "reput" => {
                let args: Vec<&String> = words.iter().skip(1).filter(|w| !w.starts_with('-')).collect();
                let get = cmd.ends_with("get");
                // put <local> [remote]; get <remote> <local>
                let local_arg = if get { args.get(1) } else { args.first() };
                let Some(l) = local_arg else { return Err(format!("sftp batch: `{cmd}` must name the local file")) };
                local(l, get)?;
            }
            other => return Err(format!("sftp batch: `{other}` is not allowed (it would run something on this machine or change its folder)")),
        }
    }
    Ok(())
}

/// Runs `ssh`, `scp` or `sftp` for the person or the assistant. See the module comment for what is checked.
#[allow(clippy::too_many_arguments)]
pub fn run(env: &Env, mode: Mode, program: &str, args: &[String], cwd: &Path, seconds: u64, local: LocalCheck, sink: Option<Sink>) -> Result<String, String> {
    let plan = plan(program, args, mode)?;
    if !cwd.is_dir() {
        return Err(format!("{} is not a folder", cwd.display()));
    }
    let client = env.client().ok_or_else(|| format!("no SSH client was found on this machine. {}", install_hint(std::env::consts::OS)))?;
    let prog_path = match program {
        "ssh" => client.path.clone(),
        other => client.tool(other).ok_or_else(|| if other == "sftp" { format!("this machine's SSH client ({} {}) has no sftp; use scp, or ssh with a command", client.flavor.name(), client.version) } else { format!("`{other}` was not found next to the SSH client") })?,
    };
    // local files: inside the person's scope, and the real path is the one used
    let mut local_paths: Vec<(String, PathBuf)> = Vec::new();
    for l in &plan.locals {
        local_paths.push((l.path.clone(), local(&l.path, l.write)?));
    }
    let mut batch_path = None;
    if let Some(b) = &plan.batch {
        let real = local(b, false)?;
        if mode == Mode::Assistant {
            let text = std::fs::read(&real).map_err(|e| format!("could not read {b}: {e}"))?;
            if text.len() > MAX_BATCH {
                return Err("the sftp batch file is larger than 64 KiB".into());
            }
            check_batch(&String::from_utf8_lossy(&text), local)?;
        }
        batch_path = Some(real);
    }
    // host key: known, or refused
    let hosts = env.known_hosts();
    if hosts.for_host(&plan.host, plan.port).is_empty() {
        return Err(unknown_text(&plan.host, plan.port));
    }
    let tmp = TempDir::new("run")?;
    let keys = write_keys(env, &client, &plan.keys, tmp.path())?;
    let key_file = |v: &str| -> String {
        match v.strip_prefix("uvault:").and_then(|n| keys.iter().find(|(k, _)| k == n)) {
            Some((_, p)) => p.display().to_string(),
            None => v.to_string(),
        }
    };
    let mut argv: Vec<String> = Vec::new();
    match client.flavor {
        Flavor::OpenSsh => {
            argv.extend(["-F", "/dev/null", "-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes"].map(String::from));
            argv.extend(["-o".to_string(), format!("UserKnownHostsFile={}", hosts_path(env).display())]);
            argv.extend(["-o", "GlobalKnownHostsFile=/dev/null", "-o", "ConnectTimeout=10", "-o", "PreferredAuthentications=publickey", "-o", "VerifyHostKeyDNS=no", "-o", "UpdateHostKeys=no", "-o", "ForwardX11=no", "-o", "PermitLocalCommand=no", "-o", "ProxyCommand=none", "-o", "ClearAllForwardings=yes"].map(String::from));
            if !keys.is_empty() {
                argv.extend(["-o", "IdentitiesOnly=yes"].map(String::from));
            }
            if !plan.agent {
                argv.extend(["-o", "ForwardAgent=no"].map(String::from));
            }
        }
        Flavor::Dropbear => argv.extend(["-o", "BatchMode=yes", "-o", "StrictHostKeyChecking=yes"].map(String::from)),
    }
    // the person's options, with key references swapped for the temporary files
    let mut it = plan.opts.iter();
    while let Some(o) = it.next() {
        if o == "-i" {
            argv.push(o.clone());
            argv.push(key_file(it.next().map_or("", String::as_str)));
        } else if o == "-o" {
            let v = it.next().cloned().unwrap_or_default();
            argv.push(o.clone());
            argv.push(match v.strip_prefix("IdentityFile=") {
                Some(f) => format!("IdentityFile={}", key_file(f)),
                None => v,
            });
        } else {
            argv.push(o.clone());
        }
    }
    if let Some(b) = &batch_path {
        argv.extend(["-b".to_string(), b.display().to_string()]);
    }
    if client.flavor == Flavor::OpenSsh {
        argv.push("--".into());
    }
    match program {
        "scp" => {
            let mut next_local = local_paths.iter();
            for op in &plan.operands {
                if scp_remote(op).is_none() {
                    argv.push(next_local.next().map_or(op.clone(), |l| l.1.display().to_string()));
                } else {
                    argv.push(op.clone());
                }
            }
        }
        _ => argv.extend(plan.operands.iter().cloned()),
    }
    // OpenSSH reads the person's managed file directly. Dropbear keeps no port in known_hosts, so it gets a home of its own for
    // this run holding only the keys trusted for this host
    let home = match client.flavor {
        Flavor::OpenSsh => env.home(),
        Flavor::Dropbear => {
            let h = tmp.path().join("home");
            let lines: String = hosts.for_host(&plan.host, plan.port).iter().map(|k| format!("{} {} {}\n", plan.host, k.kind, k.b64)).collect();
            write_private(&h.join(".ssh").join("known_hosts"), lines.as_bytes())?;
            h
        }
    };
    let mut cmd = Command::new(&prog_path);
    cmd.args(&argv).current_dir(cwd).env_clear().env("HOME", &home);
    for k in ["PATH", "LANG", "LC_ALL", "TERM", "TZ"] {
        if let Ok(v) = std::env::var(k) {
            cmd.env(k, v);
        }
    }
    cmd.env("PATH", format!("{}:{}", client.path.parent().map_or(String::new(), |p| p.display().to_string()), client.search));
    if plan.agent {
        if let Ok(v) = std::env::var("SSH_AUTH_SOCK") {
            cmd.env("SSH_AUTH_SOCK", v);
        }
    }
    let redacting: Option<Sink> = sink.map(|s| -> Sink { Arc::new(move |t: &str| s(&crate::bugs::redact(t))) });
    let result = crate::exec::run_prepared(cmd, program, seconds, redacting);
    drop(tmp);
    match result {
        Ok(t) => Ok(crate::bugs::redact(&t)),
        Err(e) => {
            let low = e.to_ascii_lowercase();
            if ["host key verification failed", "remote host identification has changed", "host key mismatch", "host key does not match", "offending"].iter().any(|m| low.contains(m)) {
                Err(changed_text(&plan.host, plan.port))
            } else {
                Err(crate::bugs::redact(&e))
            }
        }
    }
}

fn hosts_path(env: &Env) -> PathBuf {
    env.home().join(".ssh").join("known_hosts")
}

// ------------------------------------------------------------------ keys in the vault

/// Makes a new ed25519 key with the machine's own tool, stores the private half in the person's vault as `name` (`ssh.<x>`) and
/// the public half next to the connections. Returns the public key line to copy to a server. The key never touches a folder the
/// person can see: it is made in a private temp folder that is removed at once.
pub fn generate_key(env: &Env, name: &str, comment: &str, store: &dyn Fn(&str, &str) -> Result<(), String>) -> Result<String, String> {
    if !valid_key_name(name) {
        return Err(format!("`{name}` is not a key name: use ssh.<name> with lower-case letters, digits, `.`, `_` or `-`"));
    }
    if env.secret(name).is_ok() {
        return Err(format!("`{name}` already exists in your vault; choose another name or delete it first"));
    }
    let comment: String = comment.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '@' | '.' | '-' | '_')).take(60).collect();
    let client = env.client().ok_or("no SSH client was found on this machine")?;
    let tmp = TempDir::new("gen")?;
    let file = tmp.path().join("key");
    let (private, public) = if let Some(kg) = client.tool("ssh-keygen") {
        let mut cmd = Command::new(kg);
        cmd.args(["-q", "-t", "ed25519", "-N", "", "-C", &comment, "-f"]).arg(&file).env_clear().env("PATH", env_path()).env("HOME", tmp.path());
        crate::exec::run_prepared(cmd, "ssh-keygen", 30, None).map_err(|e| format!("ssh-keygen failed: {e}"))?;
        (std::fs::read_to_string(&file).map_err(|e| e.to_string())?, std::fs::read_to_string(tmp.path().join("key.pub")).map_err(|e| e.to_string())?)
    } else if let Some(dk) = client.tool("dropbearkey") {
        let mut cmd = Command::new(dk);
        cmd.args(["-t", "ed25519", "-C", &comment, "-f"]).arg(&file).env_clear().env("PATH", env_path());
        let out = crate::exec::run_prepared(cmd, "dropbearkey", 30, None).map_err(|e| format!("dropbearkey failed: {e}"))?;
        let public = out.lines().find(|l| l.starts_with("ssh-")).ok_or("dropbearkey printed no public key")?.to_string();
        let raw = std::fs::read(&file).map_err(|e| e.to_string())?;
        (format!("b64:{}", crate::story::base64_encode(&raw)), public)
    } else {
        return Err("this machine has neither ssh-keygen nor dropbearkey, so no key can be made here".into());
    };
    store(name, &private)?;
    write_private(&env.dir.join("keys").join(format!("{name}.pub")), format!("{}\n", public.trim()).as_bytes())?;
    Ok(public.trim().to_string())
}

// ------------------------------------------------------------------ saved connections

fn default_port() -> u16 {
    22
}

/// A saved host. No secret is stored: the key is a vault name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Connection {
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub user: String,
    /// The vault name of the private key (`ssh.<x>`), or empty.
    #[serde(default)]
    pub key: String,
    /// What to run when connecting; empty runs a quick check (`true`).
    #[serde(default)]
    pub command: String,
}

impl Connection {
    pub fn validate(&self) -> Result<(), String> {
        let n = &self.name;
        if n.is_empty() || n.len() > 40 || !n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' ')) {
            return Err("the name is 1 to 40 letters, digits, spaces, `.`, `_` or `-`".into());
        }
        if !valid_host(&self.host) {
            return Err(format!("`{}` is not a host name or address", self.host));
        }
        if self.port == 0 {
            return Err("the port is 1 to 65535".into());
        }
        if !self.user.is_empty() && !valid_user(&self.user) {
            return Err(format!("`{}` is not a user name", self.user));
        }
        if !self.key.is_empty() && !valid_key_name(&self.key) {
            return Err(format!("`{}` is not a vault key name (ssh.<name>)", self.key));
        }
        if self.command.len() > 500 || self.command.contains(|c: char| c.is_control()) {
            return Err("the default command is up to 500 characters on one line".into());
        }
        crate::exec::split_words(&self.command)?;
        Ok(())
    }

    /// `user@host`, or just the host.
    pub fn target(&self) -> String {
        if self.user.is_empty() { self.host.clone() } else { format!("{}@{}", self.user, self.host) }
    }

    /// The arguments of the `ssh` command that connects (the default command, or `true` for a quick check).
    pub fn args(&self) -> Vec<String> {
        let mut a = vec!["-p".to_string(), self.port.to_string()];
        if !self.key.is_empty() {
            a.extend(["-i".to_string(), format!("uvault:{}", self.key)]);
        }
        a.push(self.target());
        let cmd = crate::exec::split_words(&self.command).unwrap_or_default();
        if cmd.is_empty() {
            a.push("true".into());
        } else {
            a.extend(cmd);
        }
        a
    }
}

pub struct Connections {
    path: PathBuf,
}

impl Connections {
    pub fn new(path: &Path) -> Connections {
        Connections { path: path.into() }
    }

    pub fn list(&self) -> Vec<Connection> {
        let mut v: Vec<Connection> = std::fs::read(&self.path).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        v.sort_by_key(|c| c.name.to_ascii_lowercase());
        v
    }

    pub fn get(&self, name: &str) -> Option<Connection> {
        self.list().into_iter().find(|c| c.name == name)
    }

    fn store(&self, all: &[Connection]) -> Result<(), String> {
        write_private(&self.path, &serde_json::to_vec_pretty(all).map_err(|e| e.to_string())?)
    }

    /// Adds a connection, or (with `replacing`) changes the one called that, which may rename it.
    pub fn save(&self, c: Connection, replacing: Option<&str>) -> Result<(), String> {
        c.validate()?;
        let mut all = self.list();
        if all.iter().any(|x| x.name.eq_ignore_ascii_case(&c.name) && Some(x.name.as_str()) != replacing) {
            return Err(format!("there is already a connection called {}", c.name));
        }
        if let Some(old) = replacing {
            all.retain(|x| x.name != old);
        }
        if all.len() >= 200 {
            return Err("at most 200 saved connections".into());
        }
        all.push(Connection { host: c.host.to_ascii_lowercase(), ..c });
        self.store(&all)
    }

    pub fn remove(&self, name: &str) -> Result<(), String> {
        let mut all = self.list();
        let n = all.len();
        all.retain(|x| x.name != name);
        if all.len() == n {
            return Err(format!("no connection called {name}"));
        }
        self.store(&all)
    }
}

/// `host [port]` of the `sshtrust` and `sshforget` commands (a `SHA256:` word is for the caller).
pub fn host_port(args: &[String]) -> Result<(String, u16), String> {
    let words: Vec<&String> = args.iter().filter(|a| !a.starts_with("SHA256:")).collect();
    let host = words.first().ok_or("usage: sshtrust <host> [port] [SHA256:fingerprint]")?;
    let (_, host) = split_target(host)?;
    let port = words.get(1).map_or(Ok(22), |p| parse_port(p))?;
    Ok((host, port))
}

/// The terminal's `sshtrust`: without a fingerprint it fetches the host's key and shows it; with the fingerprint the person
/// typed (compared by them with what the server's owner gave them) it records the key that has that fingerprint.
pub fn trust_flow(env: &Env, host: &str, port: u16, want: Option<&str>) -> Result<String, String> {
    let client = env.client().ok_or_else(|| format!("no SSH client was found on this machine. {}", install_hint(std::env::consts::OS)))?;
    let keys = probe(&client, host, port)?;
    let known = env.known_hosts().for_host(host, port);
    let shown = describe_keys(&keys).join("\n  ");
    let changed = !known.is_empty() && !keys.iter().any(|k| known.contains(k));
    match want {
        None => Ok(format!(
            "{} presents:\n  {shown}\n{}Compare with the fingerprint the server's owner gave you (on the server: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub). If it is the same, run: sshtrust {host}{} {}",
            pattern(host, port),
            if changed { "WARNING: this is NOT the key you trusted before. Only continue if you know the server was reinstalled.\n" } else if known.is_empty() { "" } else { "(it is already trusted)\n" },
            if port == 22 { String::new() } else { format!(" {port}") },
            keys[0].fingerprint()
        )),
        Some(fp) => {
            let hit: Vec<HostKey> = keys.iter().filter(|k| k.fingerprint() == fp).cloned().collect();
            if hit.is_empty() {
                return Err(format!("{} does not present a key with the fingerprint {fp}; nothing was trusted. It presents:\n  {shown}", pattern(host, port)));
            }
            env.known_hosts().trust(host, port, &hit)?;
            Ok(format!("trusted {} for {}", describe_keys(&hit).join(", "), pattern(host, port)))
        }
    }
}

/// A one-line fingerprint list for the screen and the terminal.
pub fn describe_keys(keys: &[HostKey]) -> Vec<String> {
    keys.iter().map(|k| format!("{} {}", k.kind, k.fingerprint())).collect()
}

#[cfg(test)]
#[path = "ssh_tests.rs"]
mod tests;
