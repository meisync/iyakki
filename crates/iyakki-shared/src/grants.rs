//! Types moved out of `iyakki_agent::grants` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use serde_json::Value;
use std::sync::Arc;
use std::sync::Mutex;

/// The text a card appends to the summary of a call that can never be covered by a session grant.
pub const ALWAYS_ASKS: &str = "\nalways asks: ";

/// The text a card appends to the summary of a call that "Allow for this session" would cover, and what it would cover.
pub const SESSION_COVERS: &str = "\nallow for this session covers: ";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Grant {
    pub tool: String,
    /// `*`: every call of the tool (the old "always allow this tool"); otherwise the key from [`scope_of`].
    pub scope: String,
    /// One line for the list the person can revoke from.
    pub label: String,
}

#[derive(Default)]
pub struct Grants {
    pub list: Mutex<Vec<Grant>>,
}

impl std::fmt::Debug for Grants {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Grants({})", self.len())
    }
}

/// Grants compare by identity: two run limits are equal when they share one store.
impl PartialEq for Grants {
    fn eq(&self, o: &Grants) -> bool {
        std::ptr::eq(self, o)
    }
}

impl Eq for Grants {}

impl Grants {
    pub fn new() -> Arc<Grants> {
        Arc::new(Grants::default())
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Grant>> {
        self.list.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn covers(&self, tool: &str, scope: &str) -> bool {
        self.lock().iter().any(|g| g.tool == tool && (g.scope == scope || g.scope == "*"))
    }

    pub fn add(&self, tool: &str, scope: &str, label: &str) {
        let mut l = self.lock();
        if !l.iter().any(|g| g.tool == tool && g.scope == scope) {
            l.push(Grant { tool: tool.into(), scope: scope.into(), label: label.into() });
        }
    }

    pub fn list(&self) -> Vec<Grant> {
        self.lock().clone()
    }

    pub fn len(&self) -> usize {
        self.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Take one grant back; the next such call asks again.
    pub fn revoke(&self, i: usize) -> bool {
        let mut l = self.lock();
        if i < l.len() {
            l.remove(i);
            true
        } else {
            false
        }
    }

    pub fn clear(&self) {
        self.lock().clear();
    }
}

/// Split a card summary into the question and the `always asks` reason, if there is one.
pub fn split_summary(s: &str) -> (&str, Option<&str>) {
    match s.split_once(ALWAYS_ASKS) {
        Some((q, why)) => (q, Some(why)),
        None => (s, None),
    }
}

/// A card summary taken apart: the question, why it always asks (no session button), or what a session grant would cover.
#[derive(Debug, PartialEq, Eq)]
pub struct Card<'a> {
    pub question: &'a str,
    pub always: Option<&'a str>,
    pub covers: Option<&'a str>,
}

pub fn split_card(s: &str) -> Card<'_> {
    if let (q, Some(why)) = split_summary(s) {
        return Card { question: q, always: Some(why), covers: None };
    }
    match s.split_once(SESSION_COVERS) {
        Some((q, c)) => Card { question: q, always: None, covers: Some(c) },
        None => Card { question: s, always: None, covers: None },
    }
}

/// The summary a card shows: the question, plus why it always asks when it does (the chat adds what "Allow for this session"
/// would cover itself, from the call it already holds, so lists elsewhere keep the plain question).
pub fn card_summary(question: String, blocker: &Option<String>) -> String {
    match blocker {
        Some(why) => format!("{question}{ALWAYS_ASKS}{why}"),
        None => question,
    }
}






pub fn text(args: &Value, k: &str) -> String {
    args.get(k).and_then(Value::as_str).unwrap_or("").trim().to_string()
}

pub fn argv(args: &Value) -> Vec<String> {
    args.get("args").and_then(Value::as_array).map(|a| a.iter().map(|x| x.as_str().map_or_else(|| x.to_string(), str::to_string)).collect()).unwrap_or_default()
}

/// Programs whose first word after the name is what they do.
pub const SUBCOMMAND: [&str; 22] = ["git", "kubectl", "helm", "podman", "docker", "cargo", "go", "pip", "pip3", "apt", "apt-get", "systemctl", "gh", "kustomize", "nerdctl", "crictl", "k3s", "just", "rustup", "terraform", "oc", "flatpak"];
/// Like [`SUBCOMMAND`] but the script or target name matters too (`npm run build` is not `npm run deploy`).
pub const SUBCOMMAND2: [&str; 6] = ["npm", "yarn", "pnpm", "make", "bun", "deno"];
/// Programs that run whatever they are told or reach other machines: only the exact same command line is covered.
pub const EXACT: [&str; 40] = [
    "sh", "bash", "zsh", "dash", "fish", "env", "xargs", "eval", "python", "python3", "node", "perl", "ruby", "php", "lua", "awk", "sed", "ssh", "scp", "sftp", "rsync", "curl", "wget", "nc", "ncat", "socat", "find", "timeout", "nice", "nohup", "watch", "npx", "bunx", "pipx", "uvx", "tsx", "ts-node", "busybox", "time", "sudo",
];
/// Flags that name a cluster, namespace, host or registry: their values are part of the key.
pub const TARGET_FLAGS: [&str; 16] = ["--context", "-n", "--namespace", "--kubeconfig", "--server", "--cluster", "--user", "--host", "-H", "--url", "--registry", "--remote", "--target", "--namespace-name", "--address", "--repo"];

pub fn base(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

pub fn flag_targets(v: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let a = &v[i];
        if let Some((k, val)) = a.split_once('=') {
            if TARGET_FLAGS.contains(&k) {
                out.push(format!("{k}={val}"));
            }
        } else if TARGET_FLAGS.contains(&a.as_str()) {
            out.push(format!("{a}={}", v.get(i + 1).cloned().unwrap_or_default()));
            i += 1;
        }
        i += 1;
    }
    out.sort();
    out
}

pub fn positional(v: &[String]) -> Vec<&str> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < v.len() {
        let a = v[i].as_str();
        if TARGET_FLAGS.contains(&a) {
            i += 2;
            continue;
        }
        if !a.starts_with('-') {
            out.push(a);
        }
        i += 1;
    }
    out
}

/// The key for a program and its arguments (`run_command`, or a coding agent's Bash after it was split into words).
pub fn command_scope(program: &str, v: &[String]) -> String {
    let prog = program.trim();
    let name = base(prog).to_lowercase();
    if EXACT.contains(&name.as_str()) {
        return format!("cmd:{prog} {}", v.join(" "));
    }
    let pos = positional(v);
    let mut key = if prog.contains('/') { prog.to_string() } else { name.clone() };
    if SUBCOMMAND.contains(&name.as_str()) {
        key.push(' ');
        key.push_str(pos.first().copied().unwrap_or(""));
    } else if SUBCOMMAND2.contains(&name.as_str()) {
        key.push(' ');
        key.push_str(&pos.iter().take(2).copied().collect::<Vec<_>>().join(" "));
    }
    let t = flag_targets(v);
    if !t.is_empty() {
        key.push_str(" @");
        key.push_str(&t.join(","));
    }
    format!("cmd:{key}")
}

/// A shell line from an external agent: a plain command is split like `run_command`; anything with pipes, redirects,
/// substitution or several commands is covered only as that exact line.
pub fn shell_scope(line: &str) -> String {
    let l = line.trim();
    if l.chars().any(|c| matches!(c, ';' | '&' | '|' | '`' | '$' | '>' | '<' | '\n' | '(' | ')' | '{' | '}' | '"' | '\'' | '\\' | '*' | '?')) {
        return format!("line:{l}");
    }
    let mut w = l.split_whitespace().map(str::to_string);
    let prog = w.next().unwrap_or_default();
    command_scope(&prog, &w.collect::<Vec<_>>())
}

/// A relative path stays in the workspace: any file there. An absolute path is covered with the files of its own folder only;
/// `..` and `~` only as that exact path.
pub fn path_scope(key: &str, p: &str) -> String {
    if p.starts_with('~') || p.split(['/', '\\']).any(|c| c == "..") {
        return format!("{key}={p}");
    }
    if p.starts_with('/') {
        let dir = p.rsplit_once('/').map_or("/", |x| if x.0.is_empty() { "/" } else { x.0 });
        return format!("dir={dir}");
    }
    "in=workspace".to_string()
}

/// `scheme://host:port`, lower case, no credentials, no path.
pub fn origin(url: &str) -> String {
    let u = url.trim();
    let (scheme, rest) = u.split_once("://").unwrap_or(("", u));
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit('@').next().unwrap_or(host);
    format!("{}://{}", scheme.to_lowercase(), host.to_lowercase())
}

pub const TARGET_KEYS: [&str; 22] = ["cluster", "context", "namespace", "ns", "env", "environment", "kubeconfig", "host", "server", "node", "release", "registry", "repo", "account", "to", "recipient", "email", "channel", "device", "drive", "disk", "target"];

/// What a grant for this call would cover (see the module doc). Two calls with the same tool and the same key are the same
/// kind of call.
pub fn scope_of(name: &str, args: &Value) -> String {
    match name {
        "run_command" => command_scope(&text(args, "program"), &argv(args)) + &dir_part(args),
        "Bash" => shell_scope(&text(args, "command")),
        "http_request" | "fetch_url" | "web_fetch" => format!("{} {}", text(args, "method").to_uppercase(), origin(&text(args, "url"))),
        _ => {
            let mut parts = Vec::new();
            for k in TARGET_KEYS {
                if let Some(v) = args.get(k).filter(|v| !v.is_null()) {
                    parts.push(format!("{k}={}", v.as_str().map_or_else(|| v.to_string(), str::to_string)));
                }
            }
            if let Some(u) = args.get("url").and_then(Value::as_str) {
                parts.push(format!("origin={}", origin(u)));
            }
            for k in ["path", "file_path", "file", "dir", "manifest_path", "dest", "destination"] {
                if let Some(p) = args.get(k).and_then(Value::as_str) {
                    parts.push(path_scope(k, p));
                }
            }
            parts.sort();
            parts.dedup();
            format!("tool:{}", parts.join(" "))
        }
    }
}

pub fn dir_part(args: &Value) -> String {
    let mut s = String::new();
    for k in ["cwd", "dir", "workdir"] {
        let d = text(args, k);
        if !d.is_empty() && d != "." {
            s.push_str(&format!(" in:{d}"));
        }
    }
    s
}

/// One line for the list of grants: `run_command: git status`, `write_file: files in this workspace`.
pub fn label_of(name: &str, args: &Value) -> String {
    match name {
        "run_command" => {
            let v = argv(args);
            let s = scope_of(name, args);
            let what = s.strip_prefix("cmd:").unwrap_or(&s);
            let exact = EXACT.contains(&base(&text(args, "program")).to_lowercase().as_str());
            if exact {
                format!("run_command: exactly `{} {}`", text(args, "program"), v.join(" ")).trim_end().to_string()
            } else {
                format!("run_command: {what} (any arguments)")
            }
        }
        "Bash" => format!("Bash: {}", scope_of(name, args).split_once(':').map_or("", |x| x.1)),
        "http_request" | "fetch_url" | "web_fetch" => format!("{name}: {}", scope_of(name, args).trim()),
        _ => {
            let s = scope_of(name, args);
            let rest = s.strip_prefix("tool:").unwrap_or("").trim();
            if rest.is_empty() {
                format!("{name}: any call")
            } else if rest == "in=workspace" {
                format!("{name}: files in this workspace")
            } else if let Some(d) = rest.strip_prefix("dir=").filter(|d| !d.contains(' ')) {
                format!("{name}: files in the folder {d}")
            } else {
                format!("{name}: {rest}")
            }
        }
    }
}

/// Why this call can never be covered by a session grant (`None`: it can). The tool set may add reasons of its own
/// (a remote or production cluster), see `Tools::grant_blocker`.
pub fn blocker(name: &str, args: &Value) -> Option<String> {
    if let Some(why) = crate::toolpolicy::hard_ask(name, args) {
        return Some(why.to_string());
    }
    let n = name.to_lowercase();
    if n.contains("usb") || n.contains("drive_write") || n.contains("flash") || n.contains("disk") || n.starts_with("format_") || n == "cluster_start" {
        return Some("writing to drives and changing cluster services always asks".into());
    }
    let text_args = args.to_string().to_lowercase();
    if text_args.contains("/dev/") && (n == "run_command" || n == "bash") {
        return Some("anything touching a drive always asks".into());
    }
    None
}

