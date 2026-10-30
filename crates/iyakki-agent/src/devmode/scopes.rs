//! What a paired computer may do: every power beyond install/run/logs/screenshot is a **scope** the administrator grants to that one
//! computer on the device (Settings → Developer mode), shown on the pairing card, listed per computer, and checked on every request.
//! Scopes only ever grow by an administrator's hand on the device: no request from a computer can change its own scopes.
//!
//! | scope | what it allows |
//! |---|---|
//! | `install` | (default) install, upgrade, remove, list, logs of apps, open an app, screenshot |
//! | `debug` | read-only: info, processes, kernel log, system log, audit tail, service status |
//! | `files` | push, pull, list, remove inside the account's home, workspaces and `/tmp/iyakki-dev` |
//! | `files:system` | also READ (never write) `/etc`, `/lib/modules`, the data folder; secrets are never readable |
//! | `config` | change settings through a whitelist; keys for providers are write-only |
//! | `input` | send taps, keys and text to the shell on the screen |
//! | `input:password` | also while a password field has focus |
//! | `shell` | run commands: needs a time-limited grant approved on the device screen as well |
//! | `power` | reboot and power off (the `shell` scope includes it) |
//! | `update` | replace the boot partition files (a system update) |
//! | `forward` | TCP port forwarding to services listening on the device's own loopback |
//! | `kube` | read-only look at the dev cluster (`iyakki dev kube get|describe|logs|top|events`); Secrets are never shown |

use iyakki_sec::policy::Policy;

use super::{DevKey, DevMode};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scope {
    Install,
    Debug,
    Files,
    FilesSystem,
    Config,
    Input,
    InputPassword,
    Shell,
    Power,
    Update,
    Forward,
    Kube,
}

impl Scope {
    pub const ALL: [Scope; 12] = [Scope::Install, Scope::Debug, Scope::Files, Scope::FilesSystem, Scope::Config, Scope::Input, Scope::InputPassword, Scope::Shell, Scope::Power, Scope::Update, Scope::Forward, Scope::Kube];

    /// What "grant all diagnostic scopes" gives a computer, to help find a fault: look (debug), read and copy files, drive the screen,
    /// look at the dev cluster, and ask for a shell (which still needs the approval on this screen every time). Never the system
    /// update, restart/power, port forwarding, system files, password fields or settings changes.
    pub const DIAGNOSTIC: [Scope; 5] = [Scope::Debug, Scope::Files, Scope::Input, Scope::Kube, Scope::Shell];

    pub fn id(self) -> &'static str {
        match self {
            Scope::Install => "install",
            Scope::Debug => "debug",
            Scope::Files => "files",
            Scope::FilesSystem => "files:system",
            Scope::Config => "config",
            Scope::Input => "input",
            Scope::InputPassword => "input:password",
            Scope::Shell => "shell",
            Scope::Power => "power",
            Scope::Update => "update",
            Scope::Forward => "forward",
            Scope::Kube => "kube",
        }
    }

    pub fn parse(s: &str) -> Option<Scope> {
        Scope::ALL.into_iter().find(|x| x.id() == s.trim().to_ascii_lowercase())
    }

    pub fn label(self) -> &'static str {
        match self {
            Scope::Install => "Install, run, logs, screenshot",
            Scope::Debug => "Debug (read-only status, processes, logs)",
            Scope::Files => "Files in home and workspaces",
            Scope::FilesSystem => "Read system files",
            Scope::Config => "Change settings",
            Scope::Input => "Send keys and taps",
            Scope::InputPassword => "Send keys into password fields",
            Scope::Shell => "SHELL: run commands",
            Scope::Power => "Reboot and power off",
            Scope::Update => "SYSTEM UPDATE: replace boot files",
            Scope::Forward => "Forward ports of this device",
            Scope::Kube => "Look at the dev cluster (read-only)",
        }
    }

    pub fn describe(self) -> &'static str {
        match self {
            Scope::Install => "install, upgrade and remove apps, read their logs, open them, take a screenshot",
            Scope::Debug => "read device info, processes, the kernel and system logs, service status, a snapshot of the screen's text, inbox titles and the diagnostic bundle; changes nothing",
            Scope::Files => "copy files to and from the account's home folder, workspaces and /tmp/iyakki-dev; delete there",
            Scope::FilesSystem => "read (never write) /etc, /lib/modules and IYAKKI's data folder; passwords, vault and keys are never readable",
            Scope::Config => "change a fixed list of settings (sound, display, time zone, host name, firewall, models, apps); API keys are write-only",
            Scope::Input => "press keys, tap, click a button by its label and type on the screen as if at the keyboard (never into password fields)",
            Scope::InputPassword => "the same while a password field has focus",
            Scope::Shell => "run any command as the account; each use needs your approval on this screen",
            Scope::Power => "restart or switch off this device",
            Scope::Update => "replace the kernel, initrd and boot menu with new ones (the old pair is kept)",
            Scope::Forward => "open TCP ports of this device's own services (local model server, dev cluster) on the computer",
            Scope::Kube => "read-only kubectl on the dev cluster (get, describe, logs, top, events); Secrets are blocked, nothing can be changed",
        }
    }

    /// Powers that can take the whole device: shown in red, and only for administrator accounts.
    pub fn dangerous(self) -> bool {
        matches!(self, Scope::Shell | Scope::Update | Scope::Forward | Scope::Power | Scope::FilesSystem | Scope::InputPassword)
    }

    /// Needs an account that may manage settings (administrator role). The others work with any role that may use developer mode,
    /// within what the role allows.
    pub fn needs_admin(self) -> bool {
        matches!(self, Scope::Shell | Scope::Update | Scope::Forward | Scope::Power | Scope::FilesSystem | Scope::Kube)
    }

    /// Scopes that must be on for this one to mean anything.
    fn requires(self) -> Option<Scope> {
        match self {
            Scope::FilesSystem => Some(Scope::Files),
            Scope::InputPassword => Some(Scope::Input),
            _ => None,
        }
    }
}

/// What a freshly paired computer gets.
pub fn default_scopes() -> Vec<String> {
    vec![Scope::Install.id().to_string()]
}

impl DevKey {
    /// Does this computer hold the scope (`shell` includes `power`)?
    pub fn has(&self, s: Scope) -> bool {
        self.scopes.iter().any(|x| x == s.id()) || (s == Scope::Power && self.scopes.iter().any(|x| x == Scope::Shell.id()))
    }

    pub fn scope_list(&self) -> Vec<Scope> {
        Scope::ALL.into_iter().filter(|s| self.scopes.iter().any(|x| x == s.id())).collect()
    }

    /// `install, debug, files`
    pub fn scope_text(&self) -> String {
        self.scope_list().iter().map(|s| s.id()).collect::<Vec<_>>().join(", ")
    }
}

/// Use-time check: the computer holds the scope AND its account's role may use it.
pub fn check(k: &DevKey, policy: &Policy, s: Scope) -> Result<(), String> {
    if !k.has(s) {
        return Err(format!("this computer does not hold the {} scope: an administrator grants it on the device (Settings, Developer mode)", s.id()));
    }
    if let Some(r) = s.requires() {
        if !k.has(r) {
            return Err(format!("the {} scope needs the {} scope as well", s.id(), r.id()));
        }
    }
    if s.needs_admin() && !policy.manage_settings {
        return Err(format!("the {} scope works only for an administrator account; {} is not one", s.id(), k.account));
    }
    Ok(())
}

fn normalise(list: &mut Vec<String>) {
    let mut keep: Vec<String> = Vec::new();
    for s in Scope::ALL {
        let on = list.iter().any(|x| x == s.id());
        let allowed = s.requires().is_none_or(|r| list.iter().any(|x| x == r.id()));
        if on && allowed {
            keep.push(s.id().to_string());
        }
    }
    *list = keep;
}

/// Turn a scope on or off in a list (also drops what depended on it).
pub fn toggled(list: &[String], s: Scope, on: bool) -> Result<Vec<String>, String> {
    let mut v = list.to_vec();
    v.retain(|x| x != s.id());
    if on {
        if let Some(r) = s.requires() {
            if !v.iter().any(|x| x == r.id()) {
                return Err(format!("grant {} first: {} needs it", r.id(), s.id()));
            }
        }
        v.push(s.id().to_string());
    }
    normalise(&mut v);
    Ok(v)
}

impl DevMode {
    /// The scopes of a paired computer, by name.
    pub fn scopes_of(&self, computer: &str) -> Option<Vec<Scope>> {
        self.keys().into_iter().find(|k| k.name == computer).map(|k| k.scope_list())
    }

    /// Grant or take away a scope of a paired computer (administrators, on the device). Takes effect on its next request; taking a
    /// scope away also ends that computer's open sessions of it.
    pub fn set_scope(&self, policy: &Policy, user: &str, computer: &str, s: Scope, on: bool) -> Result<DevKey, String> {
        Self::need_admin(policy)?;
        let _one_at_a_time = super::keys_lock();
        let mut keys = self.keys();
        let k = keys.iter_mut().find(|k| k.name == computer).ok_or_else(|| format!("no paired computer called {computer}"))?;
        if s == Scope::Install && !on {
            // allowed: a computer with no scope can only ask who it is
        }
        k.scopes = toggled(&k.scopes, s, on)?;
        let k = k.clone();
        self.write("keys.json", &keys)?;
        self.log(user, user, if on { "devmode.scope.grant" } else { "devmode.scope.revoke" }, &format!("{} {} -> now: {}", computer, s.id(), if k.scopes.is_empty() { "nothing".to_string() } else { k.scope_text() }), true);
        if !on {
            super::sessions::end_scope(&self.data, &k.name, s);
        }
        Ok(k)
    }

    /// "Grant all diagnostic scopes" (administrators, on the device): adds [`Scope::DIAGNOSTIC`] to what the computer holds. Nothing is
    /// removed, and the system update, power, forward, system files, password input and settings stay as they were. The 4 h switch-off
    /// timer of developer mode is not touched.
    pub fn grant_diagnostic(&self, policy: &Policy, user: &str, computer: &str) -> Result<DevKey, String> {
        Self::need_admin(policy)?;
        let _one_at_a_time = super::keys_lock();
        let mut keys = self.keys();
        let k = keys.iter_mut().find(|k| k.name == computer).ok_or_else(|| format!("no paired computer called {computer}"))?;
        for s in Scope::DIAGNOSTIC {
            k.scopes = toggled(&k.scopes, s, true)?;
        }
        let k = k.clone();
        self.write("keys.json", &keys)?;
        self.log(user, user, "devmode.scope.grant", &format!("{computer} diagnostic scopes -> now: {}", k.scope_text()), true);
        Ok(k)
    }

    /// The scopes a computer paired from the open code will get (shown on the pairing card).
    pub fn set_pairing_scope(&self, policy: &Policy, user: &str, s: Scope, on: bool) -> Result<Vec<String>, String> {
        Self::need_admin(policy)?;
        let _one_at_a_time = super::keys_lock();
        let mut p = self.pairing().ok_or("no pairing code is open")?;
        p.scopes = toggled(&p.scopes, s, on)?;
        self.write("pairing.json", &p)?;
        self.log(user, user, "devmode.pair.scope", &format!("the computer that pairs will get: {}", p.scopes.join(", ")), true);
        Ok(p.scopes)
    }
}
