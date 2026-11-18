//! Developer mode's pure parts, split out of `iyakki_agent::devmode` (F-SPLIT-03): the scope names, the shell request, the key
//! names, the texts and the screen name the UI shows or sends. The state machine itself (pairing, grants, the listener) stays in the agent.

/// The "user" a developer screenshot is asked for: the shell answers it with whatever it is drawing (`*` is never in an account name).
pub const SCREEN_ANY: &str = "*developer*";

/// `2026-10-18 14:03:09` in the device's time zone.
pub fn stamp(t: i64) -> String {
    iyakki_clock::format(&iyakki_clock::Zone::system(), t, "%F %T")
}

pub mod scopes {
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
        pub fn requires(self) -> Option<Scope> {
            match self {
                Scope::FilesSystem => Some(Scope::Files),
                Scope::InputPassword => Some(Scope::Input),
                _ => None,
            }
        }
    }
}

pub mod grants {
    use serde::{Deserialize, Serialize};

    /// The longest a computer may ask for, and the longest a "once" grant lasts.
    pub const MAX_MINUTES: u32 = 720;
    /// What the card offers for the automatic approval.
    pub const AUTO_DEFAULT_HOURS: u32 = 8;

    fn shell_kind() -> String {
        "shell".into()
    }

    #[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
    pub struct ShellRequest {
        /// `shell` (run commands) or `power` (restart / switch off only).
        #[serde(default = "shell_kind")]
        pub kind: String,
        pub computer: String,
        pub account: String,
        /// Asked to run as root (only an account named root can get it).
        pub as_root: bool,
        pub minutes: u32,
        pub why: String,
        pub at: i64,
    }
}

pub mod input {
    /// What a paired computer is told when its input meets an approval card.
    pub const APPROVAL_SENTENCE: &str = "an approval or power card is open on the device: answer this on the device's own screen (input from a paired computer never answers it)";

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub enum InputEv {
        Tap(i32, i32),
        /// From, to, milliseconds.
        Swipe(i32, i32, i32, i32, u32),
        /// A named key: `Enter`, `Esc`, `Tab`, `Backspace`, `Delete`, `Up`..., `F1`..`F12`, `Ctrl+c`, or one character.
        Key(String),
        Text(String),
    }

    const NAMED: [&str; 22] = ["Enter", "Esc", "Tab", "Backspace", "Delete", "Up", "Down", "Left", "Right", "Home", "End", "PageUp", "PageDown", "BackTab", "Space", "F1", "F2", "F3", "F4", "F5", "F6", "F7"];

    /// The one spelling of a key name: names are not case-sensitive (`ctrl+a`, `BACKSPACE`, `f5`, `escape`, `return` all work);
    /// a single character keeps its case (`A` is not `a`). `None` when it is no key the shell knows.
    pub fn canonical_key(k: &str) -> Option<String> {
        if let Some(n) = NAMED.iter().find(|n| n.eq_ignore_ascii_case(k)) {
            return Some((*n).to_string());
        }
        match k.to_ascii_lowercase().as_str() {
            "escape" => return Some("Esc".into()),
            "return" => return Some("Enter".into()),
            "bksp" | "back" => return Some("Backspace".into()),
            "del" => return Some("Delete".into()),
            "pgup" => return Some("PageUp".into()),
            "pgdn" | "pgdown" => return Some("PageDown".into()),
            _ => {}
        }
        if k.len() > 1 && k.starts_with(['F', 'f']) {
            if let Ok(n) = k[1..].parse::<u8>() {
                return (1..=12).contains(&n).then(|| format!("F{n}"));
            }
        }
        if k.len() > 5 && k[..5].eq_ignore_ascii_case("ctrl+") {
            let mut it = k[5..].chars();
            return match (it.next(), it.next()) {
                (Some(c), None) if c.is_ascii_alphabetic() => Some(format!("Ctrl+{}", c.to_ascii_lowercase())),
                _ => None,
            };
        }
        (k.chars().count() == 1 && !k.chars().any(|c| c.is_control())).then(|| k.to_string())
    }

    /// Is this a key name the shell understands?
    pub fn valid_key(k: &str) -> bool {
        canonical_key(k).is_some()
    }
}

pub mod restarttools {
    pub const PROMPT_RULE: &str = "\nRESTART GRANT: restart_grant_status shows which paired computers may restart or switch off this device without asking (developer mode). restart_grant ticks or unticks that for one paired computer; it asks the person first and only an administrator may. The grant lasts exactly as long as the developer connection unless persist: true is said (then it is kept until the person switches it off by hand and developer mode stays on; that is a hard ask), covers restart and switch-off only (never commands, root or patching), is audited at every use, and a second restart waits until the device reported a successful start.\nTRUSTED DEV COMPUTER (dev and test machines only): trusted_computer_status shows which paired computers have every approval answered yes by themselves; trusted_computer switches that on or off for one paired computer (a hard ask: the person confirms on the device's screen every time, also in Auto mode; hours 4 or 12 for a timed switch, none to keep it until the person switches it off, developer mode then stays on). patch_grant / patch_grant_status do the same for the patch approval only (Patches without asking). Never covers passwords, the vault, running as root or a scope the computer does not hold; every auto-approval is logged and the top bar shows DEV · TRUSTED COMPUTER <name>.";

    pub const PAIRING_RULE: &str = "\nPAIRED COMPUTERS: devmode_pairings lists the computers paired with this device (developer mode): when each was paired and last used, what it may do and which shell windows it has open (free; administrators). devmode_pairing_remove removes one that is not needed any more and says what that took away (its scopes, grants and open shell windows end at once; it asks the person first, every time; the computer must pair again with a new code to come back). On the computer's own side `iyakki dev devices --prune` or `--forget NAME` removes the saved copy.";
}

pub mod apiscopes {
    /// The families a scope can name, with the words shown for each.
    pub const FAMILIES: [(&str, &str); 12] = [
        ("api:status", "who am I, version, models, tools, the API description"),
        ("api:chat", "chat runs, their events and the list of waiting approvals (a program cannot answer them)"),
        ("api:flows", "list, read and run flows, read their runs"),
        ("api:schedules", "flow schedules"),
        ("api:files", "files of the account: list, read, write, move, delete, upload, download"),
        ("api:git", "clone and look at repositories"),
        ("api:data", "the data service"),
        ("api:tenant", "tenants, namespaces, their members, configuration and approvals"),
        ("api:notify", "the notification inbox and its channels"),
        ("api:tiles", "the tiles of the web view and the event stream"),
        ("api:metrics", "the service's own numbers (`/metrics`): counts and sizes only, no names, addresses or tokens; the account that paired must be an administrator"),
        ("api:admin", "administrator routes (never grantable to a program)"),
    ];
}

pub mod contract {
    /// The contract scopes an administrator can grant a computer (never `local`: no scope unlocks a local-only call).
    pub const GRANTABLE: [&str; 9] = [
        iyakki_core_api::contract::Scope::SETTINGS_READ,
        iyakki_core_api::contract::Scope::SETTINGS_WRITE,
        iyakki_core_api::contract::Scope::VAULT_STATUS,
        iyakki_core_api::contract::Scope::VAULT_LOCK,
        iyakki_core_api::contract::Scope::VAULT_USE,
        iyakki_core_api::contract::Scope::FLOWS_READ,
        iyakki_core_api::contract::Scope::FLOWS_RUN,
        iyakki_core_api::contract::Scope::FLOWS_APPROVE,
        iyakki_core_api::contract::Scope::CONSENT_READ,
    ];
}
