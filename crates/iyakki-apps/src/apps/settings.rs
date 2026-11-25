//! Settings: models (Ollama, OpenAI, Claude, Gemini, OpenRouter, …), RAG and external RAG,
//! vaults (local encrypted + external providers) and your account.
//!
//! Everyone sees the app; system-wide tabs are read-only unless the role has `manage_settings`
//! (admins). Every change is re-checked here, saved atomically, and written to the audit log —
//! by *name* only, never by value. Secrets typed here go straight into a vault: settings files
//! only ever hold references like `vault:openai-key`.

use crate::safeclamp::SafeClamp;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::Duration;

use iyakki_shared::providers::{valid_provider_id, Kind, ProviderCfg};
use iyakki_shared::rag_ext::ExternalRag;
use iyakki_shared::settings::{ModelChoice};
use iyakki_shared::settings::{ExtEntry};
use iyakki_shared::vault::{valid_secret_name, ProviderConfig as SecretProvider, VaultError};
use iyakki_shared::hostengines;
use iyakki_shared::hostvoice::{self, HostVoice};
use iyakki_shared::story as st;
use iyakki_shared::voice::{self, SpeechEngine};

use super::controls::{self, Slider};
use super::bluetooth_page::BtPage;
use super::devices::{DevRow, DevTab, DevicesPane, DeviceEnv, Target};
use super::settings_nav as nav;
use super::widgets::{render_confirm, Form, FormEvent, PickEvent, Picker};
use super::{blink, App, AppCx, Flow};
use crate::accounts::AccountError;
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::listwindow::{ListWindow, ROWS_PER_NOTCH};
use crate::tooltip::Tooltip;
use crate::ui::{Event, Key, TextField, DIM, ERR, MUTED, OK, R, TEXT};

/// The third kind of text-to-speech engine: the voices of the browser or desktop window the person is using.
const BROWSER_CHOICE: &str = "Browser voice (uses the voices of the browser or desktop you are using)";
const ACCENT: [u8; 4] = crate::ui::ACCENT_MAIN;

/// The headings of the left list, in order (every page sits under one).
pub(crate) const CATEGORIES: [&str; 8] = ["Overview", "Account & people", "Apps & AI", "Network & sharing", "Display & sound", "Devices", "System", "Developer"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    /// The start page: what needs attention, and the usual places to go.
    Overview,
    Models,
    Agents,
    Rag,
    Vault,
    Skills,
    Network,
    Bluetooth,
    Display,
    Displays,
    /// The screen saver and screen off (everyone's own choices).
    ScreenSaver,
    Sound,
    SystemSounds,
    Keyboard,
    Browser,
    DevCluster,
    Storage,
    Memory,
    Firewall,
    /// Settings -> Network -> SSH tunnels: see a web page behind a front-facing SSH host (docs/SSH-TUNNELS.md).
    SshTunnels,
    /// Settings -> Network -> Domains: names for services (docs/DOMAINS.md).
    Domains,
    /// Settings -> Network -> VPN: import, enable and configure WireGuard / OpenVPN profiles (docs/development/VPN-CONNECTOR-PLAN.md).
    Vpn,
    /// Settings -> Remote access: serve this desktop to browsers on the network (docs/ADMIN-GUIDE.md).
    Remote,
    Time,
    Battery,
    /// Settings -> Power: power profile and sleep after idle (F-PWR-01).
    Power,
    /// Settings -> Radio: reconnect tries and moving on to the next station when a live station drops (F-RADIO-17).
    Radio,
    /// Settings -> This computer: which settings are kept per computer (`hwprofile`), the computers this install has seen.
    Computer,
    HostName,
    /// The kernel log kept across crashes (docs/TROUBLESHOOTING.md).
    SessionLog,
    /// What is still starting after boot (the startup registry): one line per service with its step or its reason.
    Startup,
    Install,
    /// The Update centre: installed and previous version, update packs from a folder or stick, install and roll back (docs/UPDATES.md).
    Updates,
    Voice,
    SecretProviders,
    HostAccess,
    Developer,
    GitKeys,
    Shares,
    Account,
    HomeLayout,
    /// Settings -> Desktop views: the person's named views, templates, step keys and lock (docs/USER-GUIDE.md).
    Views,
    Cloud,
    /// Settings -> Cloud access: who is in the cloud tenant and its namespaces, with what role (docs/development/CLOUD-MULTITENANT-PLAN.md, 3a).
    CloudRoles,
    /// Which sources may fill the bell and the inbox (everyone's own choice).
    Notifications,
    /// Settings -> Apps access: which apps hold which device and system access (camera view in this slice), who approved it, take it away, and the access log
    /// (docs/development/CORE-ACCESS-MODEL.md sections 5 and 6b).
    AppAccess,
    /// Settings -> App events: which events each app listens to, with a switch for each (docs/sdk/events.md, F-SDK-188).
    AppEvents,
    /// Settings -> Backup and restore: one file with the person's own data (docs/BACKUP.md).
    Backup,
    /// Settings -> Accessibility: screen reader, high contrast, text size, large pointer, focus ring, reduced motion, sticky and slow keys (docs/ACCESSIBILITY.md).
    Accessibility,
    /// Settings -> Appearance: the Look (icons, launcher layout, dock, top bar items), the wallpaper, with a live preview (docs/development/LOOK-DESIGN.md).
    Appearance,
    /// Settings -> Encrypted home: the person's private folder as an encrypted container that opens at sign-in (F-STORE-05).
    EncHome,
    /// Settings -> Assistant: what the assistant remembers of the work in a workspace (forget it), and the stall limits per model provider (F-ASSIST-47, F-ASSIST-46).
    Assistant,
    /// Settings -> Model router (experimental): which models plan and which run steps, the strategy, the budget, the run in progress (F-ROUTER-07).
    ModelRouter,
    /// Settings -> Voice assistant (Gemini, experimental): the live voice session, its consent card, who speaks, the caps and Stop (docs/GEMINI-PROVIDER.md).
    GeminiVoice,
    /// Settings -> Alexa (Amazon's assistant, optional, off by default): the owner's product, the machine's switch and the person's own yes (F-VOICE-43).
    Alexa,
    /// Settings -> Fleet (experimental): the pair groups of this computer, members and their numbers, join codes and requests, work sent and received (F-FLEET-09).
    Fleet,
}

impl Tab {
    /// The heading this page sits under in the left list.
    pub fn group(self) -> &'static str {
        match self {
            Tab::Overview => CATEGORIES[0],
            Tab::Account | Tab::Vault | Tab::SecretProviders | Tab::Notifications | Tab::Backup | Tab::EncHome => CATEGORIES[1],
            Tab::Models | Tab::Agents | Tab::Skills | Tab::Rag | Tab::Assistant | Tab::ModelRouter | Tab::GeminiVoice | Tab::Alexa | Tab::Voice | Tab::Radio | Tab::Browser | Tab::HostAccess | Tab::AppAccess | Tab::AppEvents => CATEGORIES[2],
            Tab::Network | Tab::SshTunnels | Tab::Vpn | Tab::Remote | Tab::Domains | Tab::Firewall | Tab::Shares | Tab::Fleet => CATEGORIES[3],
            Tab::Display | Tab::Displays | Tab::ScreenSaver | Tab::HomeLayout | Tab::Views | Tab::Sound | Tab::SystemSounds | Tab::Accessibility | Tab::Appearance => CATEGORIES[4],
            Tab::Bluetooth | Tab::Keyboard => CATEGORIES[5],
            Tab::Time | Tab::Battery | Tab::Power | Tab::Computer | Tab::HostName | Tab::SessionLog | Tab::Startup | Tab::Memory | Tab::Storage | Tab::Install | Tab::Updates | Tab::Cloud | Tab::CloudRoles => CATEGORIES[6],
            Tab::Developer | Tab::DevCluster | Tab::GitKeys => CATEGORIES[7],
        }
    }

    /// Where the page sits in the left list: category first, then this order.
    pub(crate) const ORDER: [Tab; 56] = [
        Tab::Overview,
        Tab::Account, Tab::Notifications, Tab::Vault, Tab::Backup, Tab::EncHome, Tab::SecretProviders,
        Tab::Models, Tab::Agents, Tab::Skills, Tab::Rag, Tab::Assistant, Tab::ModelRouter, Tab::GeminiVoice, Tab::Alexa, Tab::Voice, Tab::Radio, Tab::Browser, Tab::HostAccess, Tab::AppAccess, Tab::AppEvents,
        Tab::Network, Tab::SshTunnels, Tab::Vpn, Tab::Remote, Tab::Domains, Tab::Firewall, Tab::Shares, Tab::Fleet,
        Tab::Display, Tab::Displays, Tab::ScreenSaver, Tab::HomeLayout, Tab::Views, Tab::Sound, Tab::SystemSounds, Tab::Appearance, Tab::Accessibility,
        Tab::Bluetooth, Tab::Keyboard,
        Tab::Time, Tab::Battery, Tab::Power, Tab::Computer, Tab::HostName, Tab::SessionLog, Tab::Startup, Tab::Memory, Tab::Storage, Tab::Install, Tab::Updates, Tab::Cloud, Tab::CloudRoles,
        Tab::Developer, Tab::DevCluster, Tab::GitKeys,
    ];

    /// The name the Cloud mode table (`iyakki_cloudmode::SETTINGS_PAGES`) knows a page under, when it needs hardware.
    /// Exhaustive: a new page has to be placed.
    pub fn cloud_page(self) -> Option<&'static str> {
        match self {
            Tab::Sound | Tab::SystemSounds => Some("sound"),
            Tab::Display | Tab::Displays | Tab::ScreenSaver => Some("display"),
            Tab::Keyboard => Some("keyboard"),
            Tab::Computer => Some("display"),
            Tab::Bluetooth => Some("bluetooth"),
            Tab::Overview | Tab::Assistant | Tab::ModelRouter | Tab::Fleet | Tab::GeminiVoice | Tab::Alexa | Tab::Appearance | Tab::Models | Tab::Agents | Tab::Rag | Tab::Vault | Tab::Skills | Tab::Network | Tab::Browser | Tab::DevCluster | Tab::Storage | Tab::Memory | Tab::Firewall | Tab::SshTunnels | Tab::Domains | Tab::Vpn | Tab::Remote | Tab::Time | Tab::Battery | Tab::Power | Tab::Radio | Tab::HostName | Tab::SessionLog | Tab::Startup | Tab::Install | Tab::Updates | Tab::Voice | Tab::SecretProviders | Tab::HostAccess | Tab::Developer | Tab::GitKeys | Tab::Shares | Tab::Account | Tab::HomeLayout | Tab::Views | Tab::Cloud | Tab::CloudRoles | Tab::Notifications | Tab::AppAccess | Tab::AppEvents | Tab::Backup | Tab::Accessibility | Tab::EncHome => None,
        }
    }

    /// Words that should find this page when searching.
    fn keywords(self) -> &'static str {
        match self {
            Tab::Overview => "overview home start summary attention recommended problems fix status",
            Tab::GeminiVoice => "gemini voice assistant live microphone talk speak google experimental consent yes wake phrase shortcut super v session limit cap stop red sign hands free press to talk",
            Tab::Alexa => "alexa amazon voice assistant echo microphone consent yes client id product avs optional off by default",
            Tab::Models => "ai chat llm model provider openai claude anthropic gemini ollama openrouter api key endpoint",
            Tab::Agents => "claude code copilot github coding agent cli codex gemini aider anthropic external install login api key auto approval",
            Tab::Fleet => "fleet pair group pair computers join code members heartbeat disk cpu memory worker planner assign work accept work trusted fingerprint leave remove administrator experimental",
            Tab::ModelRouter => "model router smart planner planning plan executor worker lighter cheaper bigger strategy script escalate stuck budget tokens cost zone privacy local cloud models roles run cancel experimental",
            Tab::Assistant => "assistant history memory remember forget workspace chat stall limit think thinking silent silence timeout seconds provider",
            Tab::Rag => "search knowledge documents retrieval chunk overlap results embedding index external qdrant chroma",
            Tab::Vault => "secrets keys passwords tokens encrypted lock",
            Tab::Backup => "backup restore copy save data usb stick drive archive export import recover lost passphrase encrypt bookmarks flows dashboards recordings chats vault",
            Tab::Skills => "instructions prompts reusable assistant behaviour",
            Tab::Network => "wifi wi-fi ethernet internet lan cable ip address connection wireless",
            Tab::Bluetooth => "bluetooth wireless pairing headphones keyboard mouse adapter",
            Tab::Display => "screen monitor brightness resolution backlight output hdmi text size font bigger smaller larger readable",
            Tab::ScreenSaver => "screen saver screensaver screen off blank sleep idle timeout dim backlight logo animation oled burn-in lock power saving battery wake black display turn off monitor",
            Tab::Displays => "displays monitors external second screen extend mirror arrangement resolution primary main identify layout panes memory screen features hdmi displayport edp",
            Tab::Sound => "audio volume speaker mute loud quiet card output hdmi displayport usb headphones test sound",
            Tab::Notifications => "notifications bell inbox badge number dot mute quiet off show sources new devices jobs finished flows errors alerts noise too many 99",
            Tab::AppEvents => "apps events listen notifications news online offline battery power network changes register switch off scope permission missed late queue",
            Tab::AppAccess => "apps access permissions capabilities revoke approved camera view log who used when allowed refused denied audit privacy webcam",
            Tab::SystemSounds => "system sounds chime login logout sign out notification alert job done sound effects cue beep volume quiet preview",
            Tab::Keyboard => "keyboard fn function keys media keys volume brightness backlight apple macbook touch bar f1 f12 repeat rate delay layout iso option command swap key codes pointer mouse cursor sensitivity trackpad touchpad scroll scrolling wheel speed faster slower fast slow smooth momentum natural direction",
            Tab::Browser => "web internet browser javascript scripts pictures images styles css cookies privacy sites graphics software hardware gpu freeze safe mode",
            Tab::DevCluster => "clusters multiple several local remote switch active k3s kubernetes cluster kubectl kubeconfig deploy registry namespace podman profile context node bare metal in-cluster service account vault developer",
            Tab::Storage => "disk usb drive data persistence save space format where my data is kept ram power off",
            Tab::Memory => "ram zram swap swappiness compressed memory zstd lz4 zswap pressure out of memory oom",
            Tab::Firewall => "firewall block incoming connections allow dns hostname ip range cidr port ssh ping nftables nft netfilter lockout revert keep blocked security network",
            Tab::SshTunnels => "ssh tunnel tunnels bastion jump host port forward socks proxy intranet internal web page behind vpn local forward remote site corp enable disable key fingerprint",
            Tab::Vpn => "vpn wireguard openvpn tunnel private network profile import conf ovpn kill switch full hybrid filter domain url split tunnel fail closed provider server endpoint",
            Tab::Domains => "domain domains name names dns hostname host local mdns store.local route routes reverse proxy listener port nodeport service k3s certificate https websocket wildcard",
            Tab::Remote => "remote access client connect connection web service serve browser https port fingerprint certificate address tablet phone another computer pixel stream share desktop sign in vnc rdp",
            Tab::Time => "clock time date timezone time zone utc 12 24 hour am pm seconds ntp sntp network time server sync 1970 battery wrong",
            Tab::Computer => "this computer computers hardware profile several multiple switch disk usb stick ssd move another mac laptop legion hp sound output display keyboard microphone gpu per computer shared copy reset forget",
            Tab::EncHome => "encrypted encryption encrypt home folder private files lock unlock password recovery key admin slot dm-crypt theft stolen lost drive protect move migrate",
            Tab::Appearance => "appearance look looks theme wallpaper background icons icon style shape size glass 3d flat outline launcher layout grid list columns dock pinned apps favourites top bar clock battery network volume menu personalise customise customize",
            Tab::Accessibility => "accessibility screen reader speak voice blind low vision high contrast text size bigger large pointer cursor focus ring motion animation sticky keys slow keys tremor mouse keyboard only",
            Tab::Radio => "radio internet fm station stream drop dropped reconnect reconnecting retry retries switch next station auto switch lost connection stops",
            Tab::Power => "power profile performance balanced power saver battery life cpu governor frequency turbo fan sleep suspend idle lid power button energy",
            Tab::Battery => "battery charge charging percent percentage power adapter plug laptop low shutdown critical health cycles time left remaining",
            Tab::HostName => "host name hostname computer name rename network name dns mdns",
            Tab::Startup => "startup boot starting services still starting ready failed k3s cluster clock network firewall sound web engine models flows scheduler waiting retry progress slow",
            Tab::SessionLog => "crash crashed froze freeze hang hung log kernel panic oops unexpected shutdown power cut blank screen last session dmesg",
            Tab::Developer => "developer mode adb dev install sideload pair pairing remote deploy tile app laptop computer key revoke push watch live reload logs screenshot",
            Tab::Install => "install installer disk erase internal ssd nvme sata drive usb copy computer partition boot",
            Tab::Updates => "update updates upgrade version new release install previous rollback roll back undo older newer usb stick folder pack signed restart",
            Tab::Voice => "microphone speech voice stt tts speak listen recording",
            Tab::SecretProviders => "vault external hashicorp provider secrets",
            Tab::HostAccess => "host files folders share import export drag drop computer home read-only link copy unplugged audit",
            Tab::GitKeys => "git ssh key keys ed25519 github gitlab gitea clone repository repositories token deploy key known hosts fingerprint public key qr passphrase agent",
            Tab::Shares => "share shared folder web http website serve publish download upload lan network qr link token password files local network phone preview site",
            Tab::Account => "password profile sign out user login text size font bigger smaller larger name theme themes dark light colour color contrast appearance look high contrast",
            Tab::CloudRoles => "cloud access roles tenant members namespace owner admin editor viewer member people team invite add remove permission who can audit trail rotate keys",
            Tab::Cloud => "cloud mode server headless remote desktop hide hardware screen sound camera bluetooth keyboard display virtual machine vm no screen auto on off boot parameter iyakki.mode",
            Tab::Views => "desktop views virtual desktops workspaces switch jump previous next hot keys template work media monitoring lock password pin rename delete order browser style mac style control command page up down",
            Tab::HomeLayout => "home screen layout tiles groups move group rename delete reset launcher chips web media work system custom app",
        }
    }

    /// One line under the page's title.
    fn blurb(self) -> &'static str {
        match self {
            Tab::Overview => "What needs your attention, and the things people change most.",
            Tab::Models => "The AI services the assistant can talk to, and which one it uses.",
            Tab::Agents => "Claude Code, GitHub Copilot and other coding agents that can answer the chat next to the built-in models.",
            Tab::Fleet => "Pair this computer with other IYAKKI computers: join codes, members and their disk, load and memory, and work sent to or from another member.",
            Tab::ModelRouter => "A bigger model plans the work and lighter models do the steps, asking the bigger one when stuck: which models take part, where they may run, the strategy, the limits and the run in progress.",
            Tab::Assistant => "What the assistant remembers of the work in a workspace, and how long a model may think or stay silent.",
            Tab::GeminiVoice => "A live voice session with Google Gemini (experimental, off by default): it needs your yes, shows a red sign while the microphone is open, and has its own limits and a Stop button.",
            Tab::Alexa => "Amazon's voice assistant (optional, off by default): it needs the owner's Amazon product on this computer and your own yes before anything is sent.",
            Tab::Rag => "How documents are split and searched when the assistant looks things up.",
            Tab::Vault => "Secrets such as API keys, kept encrypted.",
            Tab::Backup => "Save your own data (settings, dashboards, flows, notes, recordings, bookmarks) to one file on a stick or folder you choose, and restore from such a file.",
            Tab::Skills => "Reusable instructions the assistant can follow.",
            Tab::Network => "Ethernet and WiFi: what this computer has, and what is switched on.",
            Tab::Bluetooth => "The Bluetooth adapter, if there is one.",
            Tab::Display => "The screen in use, connected displays and brightness.",
            Tab::ScreenSaver => "What the screen does when nobody has used it for a while: the IYAKKI logo screen saver, dimming and turning the screen off.",
            Tab::Displays => "Several displays: resolution, which is the main one, where the others sit, mirror or extend, and whether layouts are used.",
            Tab::Sound => "Where sound plays (HDMI/DP, USB, speakers), a test chime, volume and mute.",
            Tab::Notifications => "What may reach the bell and the inbox: for each source, show it, keep it quiet (inbox only) or turn it off. Also where to reach you: a Matrix account.",
            Tab::AppEvents => "Which events each app listens to, what each one lets the app hear, how many wait for it, and a switch for each. Nothing is delivered to an app for an event you switched off.",
            Tab::AppAccess => "Which apps were allowed to use this computer's devices (the camera so far), who approved it, a button to take it away, and the log of who used what and when. The log stays on this computer.",
            Tab::SystemSounds => "Short quiet sounds for signing in and out, new notifications and finished jobs: switches, level and a preview of each.",
            Tab::Keyboard => "Function keys, volume and brightness keys, key repeat, mouse and touchpad speed, scrolling (speed, direction, smoothness) and the controls they drive.",
            Tab::Browser => "What the web browser may do: run scripts, show pictures, apply styles, keep cookies.",
            Tab::DevCluster => "The clusters the AI IDE can deploy to, this machine's own k3s, and where images go.",
            Tab::Storage => "Where your accounts, settings and files are kept.",
            Tab::Memory => "RAM, compressed swap in memory (zram) and an optional swap file.",
            Tab::Firewall => "Decide what other computers on the network can reach on this one. Everything is blocked unless you allow it.",
            Tab::SshTunnels => "Open a web page that sits behind an SSH host: each tunnel listens on this device only and can be enabled, disabled, edited or deleted.",
            Tab::Domains => "Give services a name such as store.local: a route table, the listener, the firewall opening and the certificates in your vault.",
            Tab::Remote => "Open this desktop in a browser on another computer on your network. Off by default; encrypted; signs in with this machine's accounts.",
            Tab::Vpn => "Import a WireGuard or OpenVPN profile you already have, and choose whether all traffic or only what your filters match goes through it.",
            Tab::Time => "The clock in the top bar: time zone, 12 or 24 hours, and setting the time from the network.",
            Tab::Computer => "Which computer this is, and which settings are kept for each computer the install starts on.",
            Tab::EncHome => "Keep your private folder encrypted so a lost or stolen computer or drive does not give your files away: it opens at sign-in with your password and closes at sign-out.",
            Tab::Appearance => "How the desktop looks: the Look (icons, launcher layout, dock, top bar), the wallpaper and a live preview.",
            Tab::Accessibility => "Make the computer easier to use without seeing the screen well or without a mouse: a screen reader, high contrast, bigger text and pointer, sticky and slow keys.",
            Tab::Radio => "What the radio does when a live station drops: how many times it reconnects, and whether it goes on to the next station of the list.",
            Tab::Power => "How fast the processor runs (performance, balanced, power saver) and whether the computer sleeps by itself when it is not used.",
            Tab::Battery => "How full the battery is, what it is doing, how worn it is, and the low-battery warnings.",
            Tab::HostName => "The name of this computer on the network.",
            Tab::Startup => "What is still starting after boot, what it waits for, and anything that did not start, with a Retry or Open button.",
            Tab::SessionLog => "The kernel log kept after a crash or a power cut, and whether the last session ended cleanly.",
            Tab::Developer => "Let a paired computer install, run and watch apps here (iyakki dev). Off by default.",
            Tab::Install => "Put IYAKKI on this computer's disk, from the stick it is running from.",
            Tab::Updates => "Install a newer IYAKKI from a folder or USB stick, keep the one you have, and go back to it if the new one is not right.",
            Tab::Voice => "Speech to text and text to speech.",
            Tab::SecretProviders => "External vaults that hold secrets for you.",
            Tab::HostAccess => "Folders of the computer this window runs on that this account may use, and what it may do with them.",
            Tab::GitKeys => "SSH keys and tokens for git: which key each git site uses, kept in your vault.",
            Tab::Shares => "Share a folder of yours over HTTP on this network: browse and download, optional uploads, or a small website.",
            Tab::Account => "Your account on this computer.",
            Tab::CloudRoles => "Who is in the cloud tenant and its namespaces, with what role: add and remove people and namespaces, and read the audit trail.",
            Tab::Cloud => "For servers, cloud machines and remote-desktop-only computers: hide what needs a local screen, speakers or camera.",
            Tab::Views => "Several named views on one screen, each with its own apps and panes: make, rename, delete and order them, start from a template, lock a view, and choose the keys that switch.",
            Tab::HomeLayout => "The groups on the home screen and where each tile is listed: make your own groups, move tiles, put everything back.",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Tab::Overview => "Overview",
            Tab::Models => "Models",
            Tab::Agents => "Agents",
            Tab::Rag => "RAG",
            Tab::ModelRouter => "Model router (experimental)",
            Tab::Fleet => "Fleet (experimental)",
            Tab::Assistant => "Assistant",
            Tab::GeminiVoice => "Gemini voice (experimental)",
            Tab::Alexa => "Alexa (Amazon)",
            Tab::Vault => "Vault",
            Tab::Backup => "Backup and restore",
            Tab::Skills => "Skills",
            Tab::Network => "Network",
            Tab::Bluetooth => "Bluetooth",
            Tab::Display => "Display",
            Tab::Displays => "Displays",
            Tab::ScreenSaver => "Screen saver",
            Tab::Sound => "Sound",
            Tab::Notifications => "Notifications",
            Tab::AppAccess => "Apps access",
            Tab::AppEvents => "App events",
            Tab::SystemSounds => "System sounds",
            Tab::Keyboard => "Keyboard",
            Tab::Browser => "Browser",
            Tab::DevCluster => "Clusters",
            Tab::Storage => "Storage",
            Tab::Memory => "Memory",
            Tab::Firewall => "Firewall",
            Tab::SshTunnels => "SSH tunnels",
            Tab::Vpn => "VPN",
            Tab::Domains => "Domains",
            Tab::Remote => "Remote access",
            Tab::Time => "Time",
            Tab::Battery => "Battery",
            Tab::Power => "Power",
            Tab::Radio => "Radio",
            Tab::Appearance => "Appearance",
            Tab::Accessibility => "Accessibility",
            Tab::EncHome => "Encrypted home",
            Tab::Computer => "This computer",
            Tab::HostName => "Host name",
            Tab::SessionLog => "Last session log",
            Tab::Startup => "Startup",
            Tab::Developer => "Developer mode",
            Tab::Install => "Install to disk",
            Tab::Updates => "System update",
            Tab::Voice => "Voice",
            Tab::SecretProviders => "Secret providers",
            Tab::HostAccess => "Host access",
            Tab::GitKeys => "Git and SSH keys",
            Tab::Shares => "Shared folders",
            Tab::Account => "Account",
            Tab::HomeLayout => "Home layout",
            Tab::Views => "Desktop views",
            Tab::Cloud => "Cloud mode",
            Tab::CloudRoles => "Cloud access",
        }
    }
}

#[path = "settings_sounds.rs"]
mod sysnd;
#[path = "settings_agents.rs"]
mod agents;
#[path = "settings_lemonade.rs"]
mod lemonade_page;
#[path = "settings_devcluster.rs"]
mod devcluster;
#[path = "settings_host.rs"]
mod host_access;
#[path = "settings_memory.rs"]
mod memory;
#[path = "settings_displays.rs"]
mod displays_page;
#[path = "settings_firewall.rs"]
mod firewall_page;
#[path = "settings_sshtunnels.rs"]
mod sshtun_page;
#[path = "settings_vpn.rs"]
mod vpn_page;
#[path = "settings_domains.rs"]
mod dom_page;
#[path = "settings_remote.rs"]
mod remote_page;
#[path = "settings_link.rs"]
mod settings_link_mod;
#[path = "settings_share.rs"]
mod share_page;
#[path = "settings_pagescroll.rs"]
mod pagescroll_page;
#[path = "settings_appearance.rs"]
mod appearance_page;
#[cfg(test)]
#[path = "settings_appearance_tests.rs"]
mod appearance_tests;
#[path = "settings_browserheaders.rs"]
mod bh_page;
#[path = "settings_modelopts.rs"]
mod mo_page;
#[path = "settings_overview.rs"]
mod overview_page;
#[path = "settings_vaultstate.rs"]
mod vaultstate_page;
#[path = "settings_crashlog.rs"]
mod crashlog_page;
#[path = "settings_router.rs"]
mod router_page;
#[path = "settings_fleet.rs"]
mod fleet_page;
#[path = "settings_wizard.rs"]
mod wizard_page;
#[path = "settings_updates.rs"]
mod updates_page;
#[path = "settings_startup.rs"]
mod startup_page;
#[path = "settings_fwsimple.rs"]
mod fw_simple;
use fw_simple::FwAct;
#[path = "settings_sound.rs"]
mod sound;
#[path = "settings_keyboard.rs"]
mod kbd_page;
pub(crate) use kbd_page::apply_keyboard;
#[cfg(test)]
pub(crate) use kbd_page::live_input;
#[path = "settings_battery.rs"]
mod battery_page;
#[path = "settings_radio.rs"]
mod radio_page;
// ---- pages moved out of this file (S0 of docs/development/SETTINGS-EXTRACTION-PLAN.md); one file per page, a banner per lane
#[path = "settings_pages.rs"]
mod pages_split;
// lane ST-A
#[path = "settings_forms_a.rs"]
mod forms_a;
#[path = "settings_system_port.rs"]
mod system_port;
#[path = "settings_account_port.rs"]
mod account_port;
#[path = "settings_account.rs"]
mod account_split;
#[path = "settings_vault.rs"]
mod vault_split;
#[path = "settings_secretproviders.rs"]
mod secretproviders_split;
#[path = "settings_storage.rs"]
mod storage_split;
// lane ST-B
#[path = "settings_forms_b.rs"]
mod forms_b;
#[path = "settings_apps_port.rs"]
mod apps_port;
#[cfg(test)]
#[path = "settings_b_fakes_tests.rs"]
mod b_fakes_tests;
#[cfg(test)]
#[path = "settings_b_ports_tests.rs"]
mod b_ports_tests;
#[path = "settings_models.rs"]
mod models_split;
#[path = "settings_skills.rs"]
mod skills_split;
#[path = "settings_rag.rs"]
mod rag_split;
#[path = "settings_voice.rs"]
mod voice_split;
// lane ST-C
#[path = "settings_forms_c.rs"]
mod forms_c;
#[path = "settings_browser_port.rs"]
pub(crate) mod browser_port;
#[path = "settings_network_port.rs"]
pub(crate) mod network_port;
#[cfg(test)]
#[path = "settings_network_port_tests.rs"]
mod network_port_tests;
#[cfg(test)]
#[path = "settings_browser_port_tests.rs"]
mod browser_port_tests;
#[path = "settings_browser.rs"]
mod browser_split;
#[path = "settings_network.rs"]
mod network_split;
// lane ST-D
#[path = "settings_forms_d.rs"]
mod forms_d;
#[path = "settings_display.rs"]
mod display_page;
#[path = "settings_bluetooth.rs"]
mod bluetooth_split;
#[cfg(test)]
#[path = "settings_d_fake_tests.rs"]
mod d_fake_tests;
#[path = "settings_hw.rs"]
mod hw_split;
#[path = "settings_cards.rs"]
mod cards_split;
#[path = "settings_assistant.rs"]
mod assistant_page;
#[path = "settings_power.rs"]
mod power_page;
#[path = "settings_computer.rs"]
mod computer_page;
#[path = "settings_screensaver.rs"]
mod screensaver_page;
#[path = "settings_enchome.rs"]
mod enchome_page;
#[path = "settings_voicewake.rs"]
mod voicewake_page;
#[path = "settings_browsershort.rs"]
mod settings_browsershort;
#[cfg(test)]
#[path = "settings_browsershort_tests.rs"]
mod settings_browsershort_tests;
#[path = "settings_browserdev.rs"]
mod settings_browserdev;
#[cfg(test)]
#[path = "settings_browserdev_tests.rs"]
mod settings_browserdev_tests;
#[path = "settings_browserdevices.rs"]
mod browserdevices_page;
#[cfg(test)]
#[path = "settings_browserdevices_tests.rs"]
mod browserdevices_tests;
#[path = "settings_browsersites.rs"]
mod browsersites_page;
#[cfg(test)]
#[path = "settings_browsersites_tests.rs"]
mod browsersites_tests;
#[path = "settings_browserprotect.rs"]
mod browserprotect_page;
#[path = "settings_browsercert.rs"]
mod browsercert_page;
#[path = "settings_browserbad.rs"]
mod browserbad_page;
#[path = "settings_browserproxy.rs"]
mod browserproxy_page;
#[path = "settings_browsermedia.rs"]
mod browsermedia_page;
#[cfg(test)]
#[path = "settings_browsermedia_tests.rs"]
mod browsermedia_tests;
#[cfg(test)]
#[path = "settings_browserbad_tests.rs"]
mod browserbad_tests;
#[cfg(test)]
#[path = "settings_browserproxy_tests.rs"]
mod browserproxy_tests;
#[path = "settings_browserchoices.rs"]
mod browserchoices_page;
#[path = "settings_ragorder.rs"]
mod ragorder_page;
#[cfg(test)]
#[path = "settings_choices_tests.rs"]
mod choices_tests;
#[cfg(test)]
#[path = "settings_browserprotect_tests.rs"]
mod browserprotect_tests;
#[path = "settings_browsergraphics.rs"]
mod browsergraphics_page;
#[path = "settings_browserspeed.rs"]
mod browserspeed_page;
#[path = "settings_browserblock.rs"]
mod browserblock_page;
#[path = "settings_cloud.rs"]
mod cloud_page;
#[path = "settings_cloudroles.rs"]
mod cloudroles_page;
#[path = "settings_announce.rs"]
mod announce_page;
#[path = "settings_gemini.rs"]
mod gemini_page;
#[path = "settings_geminivoice.rs"]
mod geminivoice_page;
#[path = "settings_alexa.rs"]
mod alexa_page;
#[path = "settings_homelayout.rs"]
mod homelayout_page;
#[path = "settings_views.rs"]
mod views_page;
#[path = "settings_notifs.rs"]
mod notifs_page;
#[path = "settings_appaccess.rs"]
mod appaccess_page;
#[path = "settings_appevents.rs"]
mod appevents_page;
#[path = "settings_time.rs"]
mod time;
#[path = "settings_install.rs"]
mod install_page;
#[path = "settings_usbc.rs"]
mod usbc_page;
#[path = "settings_developer.rs"]
mod developer_page;
#[path = "settings_git.rs"]
mod git_page;
#[path = "settings_matrix.rs"]
mod matrix_page;
#[path = "settings_shares.rs"]
mod shares_page;
#[path = "settings_backup.rs"]
mod backup_page;
#[path = "settings_access.rs"]
pub(crate) mod access_page;
#[path = "settings_modelhost.rs"]
mod modelhost_page;

/// One button of the theme card: a built-in theme or a theme file the person can use.
struct ThemeChoice {
    label: String,
    key: char,
    builtin: Option<crate::theme::ThemeId>,
    file: Option<String>,
    chosen: bool,
}

/// What a form lane answered: `NotMine` (not its kind, try the next lane), `Early` (the original early `return`: the modal to keep open, or none),
/// `Result` (the save result the shared tail reports).
enum FormOutcome {
    NotMine,
    Early(Option<Modal>),
    Result(Result<String, String>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FormKind {
    /// Settings -> Browser: the address of the site whose certificate is shown.
    BrowserCert,
    /// Settings -> Browser: a site to put on the known-bad list / to take off it.
    BrowserBadAdd,
    BrowserBadRemove,
    /// Settings -> Browser: the browser's HTTP proxy.
    BrowserProxy,
    /// A question step of a setup wizard an app brought (F-CORE-24).
    BundleWizard,
    /// Settings -> System update: the folder to look for an update in.
    UpdateFolder,
    UpdateBundle,
    /// Settings -> System update: download a signed update from an https address.
    UpdateFetch,
    /// The address of the system update index an administrator sets (F-UPD-04).
    UpdateSource,
    Provider(Option<String>),
    ProviderKey(String),
    /// The guided NVIDIA NIM setup: only the key is asked for.
    NvidiaQuick,
    /// Google Gemini (experimental): the setup card, and the consent card the person answers before anything is sent to Google.
    GeminiSetup,
    GeminiConsent,
    /// Settings -> Model router: the budget limits of a run, and the file of a strategy script to add.
    RouterBudget,
    RouterScript,
    /// Settings -> Fleet: make a fleet, ask another computer to let this one join.
    FleetCreate,
    FleetJoin,
    /// Where one member is reached (fleet id, member id): `host:port`, `tls://host:port#fingerprint` or `relay://MAILBOX`.
    FleetAddress(String, String),
    /// The consent card of the Gemini voice assistant: the open microphone and the tool results (Settings -> Gemini voice).
    GeminiLiveConsent,
    /// The consent card of the Alexa provider: the voice goes to Amazon during a turn (Settings -> Alexa).
    AlexaConsent,
    External(Option<String>),
    NewSecret(bool),
    /// Open the person's own vault with their login password (they signed in without typing one).
    UnlockVault,
    /// Settings -> Encrypted home: one step of a form (the logic is `iyakki_enchome::form`).
    EncHome,
    Keyfile,
    SecretProvider,
    Password,
    VoiceStt,
    VoiceTts,
    VoiceCmds,
    NewSkill,
    ImportSkill,
    /// Settings → Browser → Ads and trackers: a list address, a list file.
    BlockListUrl,
    BlockListFile,
    TokenPrice,
    /// Settings → Git and SSH keys.
    Git(git_page::GitSetting),
    /// Settings → Notifications: the Matrix account.
    Matrix,
    /// Settings -> Remote access: the SFTP server's accounts and keys.
    Sftp(remote_page::sftp_page::SftpForm),
    /// Settings -> Remote access: a machine a flow may run on (F-FLOW-16).
    Machine(remote_page::machines_page::MachineForm),
    /// Settings → Shared folders.
    Share(shares_page::ShareForm),
    /// The hotspot's Wi-Fi name and password (Share internet page, key n).
    HotspotWifi,
    /// The country the hotspot asks the radio for.
    HotspotCountry,
    /// Settings → Backup and restore.
    Backup(backup_page::BackupForm),
    /// Settings -> Apps access: a time limit for one grant (installation, capability) (F-SDK-217).
    AppAccessLimit(String, String),
    /// Settings -> Remote access: the port.
    Remote(remote_page::RemoteForm),
    /// Models → Model host on your network: nickname, address, key.
    ModelHost,
    /// (disk name, the user must also type ERASE because the disk holds data)
    Persist(String, bool),
    /// Make a virtual disk (the size is asked).
    NewVirtualDisk,
    /// The passphrase of a WiFi network found by the scan (name, what it offers).
    WifiJoin(String, iyakki_core_api::device_types::devctl::wifi::Security),
    /// A WiFi network that does not announce its name.
    WifiHidden,
    /// The login of a WPA enterprise network found by the scan (name).
    WifiEnterprise(String),
    /// The key of a WEP network found by the scan (name); asked with a plain warning.
    WifiWep(String),
    /// Remove a virtual disk (its name must be typed).
    RemoveVirtualDisk(String),
    /// Settings → Agents: the model of an agent; a custom agent (new, or the id being edited).
    AgentModel(String),
    AgentCustom(Option<String>),
    /// Settings → Dev cluster.
    Dc(devcluster::DcForm),
    /// Settings → Memory: one of its settings.
    Mem(memory::MemField),
    /// Settings → Firewall.
    Fw(firewall_page::FwForm),
    /// Settings -> Network -> SSH tunnels.
    SshTun(sshtun_page::TunForm),
    Domains(dom_page::DomForm),
    Vpn(vpn_page::VpnForm),
    /// Settings -> Browser -> Custom headers: add or edit a rule.
    BrowserHeader(bh_page::BhForm),
    /// Settings -> Browser -> Short words: add or change one (true) or remove one (false).
    BrowserShort(bool),
    /// Settings -> Browser -> Sites you allowed or blocked: forget one.
    BrowserSite,
    /// The sites of the browser data choices: false = never kept, true = left out at sign-out.
    BrowserSites(bool),
    /// The name of a reranking model for RAG (empty: none).
    RagRerankModel,
    /// Settings -> Models: one request option of a model.
    ModelOpt(mo_page::MoForm),
    /// Settings → Time: the time server; Settings → Host name: the new name.
    TimeServer,
    HostName,
    /// Settings → Home layout: a new group (None) or the new name of this one.
    HomeGroup(Option<String>),
    /// Settings -> Desktop views: a new view (None) or the new name of this one.
    View(Option<u32>),
    /// Settings -> Desktop views: the person's own step keys.
    ViewKeys,
    /// Settings → Install to disk: the questions and the typed confirmation for this disk.
    Install(String),
}

/// The extra last line of the provider form's Type choice: look at the address and use what answers.
const AUTO_TYPE: &str = "Auto-detect (Ollama, Lemonade or OpenAI-compatible)";

#[derive(Clone, Debug, PartialEq, Eq)]
enum PickKind {
    /// Settings -> Sound: which paired headphones or speaker gets all of the computer's sound (F-NET-28).
    BluetoothSink,
    /// Settings -> Model router: which strategy decides / which script to remove.
    RouterStrategy,
    RouterRemove,
    /// The lines of a site's certificate (read-only).
    CertView,
    /// A time zone from the list in Settings → Time.
    Zone,
    Chat(String),
    Embed(String),
    /// The model that looks at pictures (Models, key v): (automatic) or one a provider lists as able to see pictures.
    Vision,
    /// The model that tidies dictated text and revises text (Models, key y): (the chat model) or one a provider lists.
    Tidy,
    /// A network from the WiFi scan (or one of the choices below the list).
    Wifi,
    /// A saved WiFi network to forget.
    WifiForget,
    /// Settings -> Models: the request options of a provider's model, one row each.
    Options(String),
    /// The vault entry holding an external agent's API key.
    AgentKey(String),
    /// The model of an external agent (a line of its list, or "Custom model id…").
    AgentModel(String),
    /// Settings → Models: a model of a Lemonade provider, then what to do with it, then the reading voice.
    Lemonade(lemonade_page::LemPick),
    /// Settings → Network: the private MAC address mode for every adapter of this kind.
    MacMode(iyakki_core_api::device_types::devctl::LinkKind),
    /// Settings → Sound: Automatic or one output.
    SoundOutput,
    /// Settings → Home layout: the group to move this tile (its layout key) to.
    HomeMove(String),
    /// Settings -> Desktop views: the template for a new view.
    ViewTemplate,
    /// Settings → Firewall (simple view): the service to allow.
    FwService,
    /// Settings -> This computer: the other computer to copy settings from, or to forget.
    Computer(computer_page::CompPick),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ConfirmKind {
    /// Settings -> Sound: send all sound to the paired Bluetooth headphones or speaker (the address; empty: the only or remembered one).
    BluetoothSound(String),
    /// Settings -> Model router: a change the card described (F-ROUTER-07).
    Router(router_page::RouterAct),
    /// Settings -> Fleet: a change the card described (F-FLEET-09).
    Fleet(fleet_page::FleetAct),
    /// Settings -> System update: run the checks and data moves of the setup wizard that is open / finish it after the summary (F-CORE-24).
    WizardRun,
    WizardFinish,
    /// Take back the yes to sending audio and text to Google.
    GeminiRevoke,
    /// Settings -> Remote access: start the SFTP server / allow account passwords for it.
    Sftp(remote_page::sftp_page::SftpAct),
    Provider(String),
    External(String),
    Secret(bool, String),
    SecretProvider(String),
    ResetPersonal,
    /// Settings -> Encrypted home: start moving the existing files (the plan is on screen) / remove the old plain copy.
    EncMove,
    EncRemovePlain,
    Skill(iyakki_shared::skills::Scope, String),
    Reboot,
    AgentInstall(String),
    AgentAuto(String),
    AgentTest(String),
    /// Settings -> Models: "Check my models" (one-token requests to each NVIDIA model; spends the rate limit).
    CheckModels(String),
    AgentRemove(String),
    Dc(devcluster::DcAct),
    /// Settings → Home layout: delete this group of the person's own; put every tile back.
    HomeDelete(String),
    HomeReset,
    /// Settings -> Home layout (administrators): make the shown layout the default for new accounts; clear that default.
    HomeDefaultSet,
    HomeDefaultClear,
    /// Settings -> Desktop views: delete this view.
    ViewDelete(u32),
    /// Settings → Memory: apply the draft to the running system.
    MemApply,
    /// Settings → Network: change this adapter's MAC address now (the connection drops for a moment).
    MacApply(String),
    /// Settings → Models: pull (download) or load a Lemonade model (the approval card names the model and its size).
    Lemonade(lemonade_page::LemAct),
    /// Settings -> Sound -> Advanced: "Did the second tone sound quieter?" for this card id (y: the hardware volume works).
    SoundHwWorks(String),
    /// A share was just started and the firewall would keep other devices out of it: open the Shared folders ports for the local network?
    ShareOpenFirewall,
    /// Settings -> Shared folders: stop every running share (visitors in the middle of a download are cut off).
    ShareStopAll,
    /// Settings -> Shared folders: switch the media server on (players on the network can then list and play the shared audio and video).
    MediaServerOn,
    /// Settings -> Sound: allow the Mac's built-in (T2) microphone until the next restart.
    T2Mic,
    /// Settings → Firewall (simple view): switching to Open, or taking away something that may be how the person is connected.
    Fw(FwAct),
    /// Settings -> Network -> SSH tunnels: delete this tunnel (its own key and trusted host go with it).
    SshTunnelDelete(String),
    /// Settings -> Network -> Domains: remove this route (host and path).
    DomainDelete(String, String),
    /// Settings -> Network -> VPN: delete this profile (its keys leave the vault).
    VpnDelete(String),
    /// Settings -> Browser -> Custom headers: delete this rule (its value goes from the vault too).
    BrowserHeaderDelete(String),
    /// Settings -> Browser -> Protect browser data: switch on, switch off, delete the browser data.
    BrowserProtect,
    BrowserUnprotect,
    BrowserReset,
    /// Settings -> This computer: copy another computer's settings here, reset this computer's, forget another computer.
    Computer(computer_page::CompAct),
}

enum Modal {
    Form(FormKind, Form),
    Pick(PickKind, Picker),
    Confirm(ConfirmKind, String),
}

enum JobOut {
    /// Settings -> Browser -> a site's certificate, as lines (F-BROW-98).
    Cert(Result<Vec<String>, String>),
    Msg(Result<String, String>),
    /// A casting button's network call came back (Settings -> Shared folders, F-MEDIA-11).
    Cast(shares_page::CastDone),
    /// A setup wizard's checks and moves came back (F-CORE-24).
    Wizard(Result<(), String>),
    Wifi(Result<Vec<iyakki_core_api::device_types::devctl::wifi::Ap>, String>),
    Models { provider: String, models: Vec<String>, ms: u64 },
    Dc(devcluster::DcOut),
    Mh(modelhost_page::MhOut),
    /// Settings -> Sound -> Advanced: the two tones of the hardware volume check were played (card id, result).
    HwCheck(String, Result<String, String>),
    Backup(backup_page::BackupOut),
    /// Settings -> Models: a provider's own list of model ids.
    ModelList(mo_page::MoList),
    /// Settings -> Encrypted home: the form comes back from its background step (Make, Open, Move, ...).
    Enc(Box<iyakki_enchome::form::Form>),
}

struct Job {
    label: String,
    rx: Receiver<JobOut>,
}

/// What the Storage tab acts on. Real paths in production; tests point these at a fake machine.
#[derive(Clone)]
pub struct StorageEnv {
    pub roots: iyakki_core_api::device_types::storage::Roots,
    pub mounter: Arc<dyn iyakki_core_api::device_types::storage::Mounter>,
    /// Set when the disks are virtual (the desktop window): disks can be made and removed from the tab.
    pub virtual_disks: Option<iyakki_core_api::device_types::storage::virt::VirtualDisks>,
    pub mke2fs: std::path::PathBuf,
    pub scratch: std::path::PathBuf,
    /// The running data directory: copied onto the new disk so nothing is lost.
    pub data_dir: std::path::PathBuf,
}

/// What the WiFi part of the Network page acts on: the programs that scan and join, the saved networks, and the machine's
/// links. Real in production; tests point it at stand-ins.
#[derive(Clone)]
pub struct WifiEnv {
    pub wifi: Arc<iyakki_core_api::device_types::devctl::wifi::Wifi>,
    pub known: Arc<iyakki_core_api::device_types::devctl::wifi::KnownStore>,
    pub roots: iyakki_core_api::device_types::devctl::Roots,
}

impl WifiEnv {
    pub fn real() -> WifiEnv {
        #[cfg(any(test, feature = "testing"))]
        {
            // unit tests never scan the real air or touch the real files
            return WifiEnv { wifi: Arc::new(iyakki_core_api::device_types::devctl::wifi::Wifi { run_dir: "/nonexistent/run".into(), path: Some("/nonexistent/bin".into()) }), known: Arc::new(iyakki_core_api::device_types::devctl::wifi::KnownStore::new(std::path::Path::new("/nonexistent/wifi.json"))), roots: iyakki_core_api::device_types::devctl::Roots { sys: "/nonexistent/sys".into(), proc_: "/nonexistent/proc".into() } };
        }
        #[allow(unreachable_code)]
        WifiEnv { wifi: Arc::new(iyakki_core_api::device_types::devctl::wifi::Wifi::real()), known: Arc::new(iyakki_core_api::device_types::devctl::wifi::KnownStore::new(std::path::Path::new("/var/lib/iyakki/wifi.json"))), roots: iyakki_core_api::device_types::devctl::Roots::real() }
    }

    /// The WiFi adapters that can join networks (the hotspot's own interface never), in the order the machine lists them.
    pub fn adapters(&self) -> Vec<String> {
        iyakki_core_api::device_types::devctl::links(&self.roots).into_iter().filter(|l| l.kind == iyakki_core_api::device_types::devctl::LinkKind::Wifi && l.name != iyakki_core_api::device_types::devctl::HOTSPOT_IFACE).map(|l| l.name).collect()
    }

    /// The adapter in use: the person's choice while it is still there, else the first one.
    pub fn iface(&self) -> Option<String> {
        iyakki_core_api::device_types::devctl::wifi::pick_adapter(&self.adapters(), self.known.chosen_adapter())
    }
}

static VIRTUAL_STORAGE: std::sync::OnceLock<StorageEnv> = std::sync::OnceLock::new();

/// From now on the Storage tab shows only virtual disks kept in `dir` (sparse image files), never the machine's drives.
/// Called once at start by the desktop window; `data_dir` is what making a data disk copies onto it.
pub fn use_virtual_disks(dir: &std::path::Path, data_dir: &std::path::Path, mke2fs: Option<std::path::PathBuf>) -> Result<(), String> {
    let v = iyakki_core_api::device_types::storage::virt::VirtualDisks::open(dir)?;
    let mke2fs = mke2fs.or_else(|| ["/sbin/mke2fs", "/usr/sbin/mke2fs"].iter().map(std::path::PathBuf::from).find(|p| p.exists())).unwrap_or_else(|| "/sbin/mke2fs".into());
    let env = StorageEnv { roots: v.roots(), mounter: Arc::new(v.mounter()), mke2fs, scratch: dir.to_path_buf(), data_dir: data_dir.to_path_buf(), virtual_disks: Some(v) };
    VIRTUAL_STORAGE.set(env).map_err(|_| "virtual disks are already set up".to_string())
}

impl StorageEnv {
    pub fn real() -> StorageEnv {
        #[cfg(any(test, feature = "testing"))]
        {
            // unit tests never look at (or touch) the machine's real disks
            return StorageEnv { roots: iyakki_core_api::device_types::storage::Roots { sys_block: "/nonexistent/sys".into(), dev: "/nonexistent/dev".into(), mounts: "/nonexistent/mounts".into(), require_block: true }, mounter: Arc::new(iyakki_core_api::device_types::storage::SysMounter), mke2fs: "/nonexistent/mke2fs".into(), scratch: "/nonexistent".into(), data_dir: "/nonexistent/data".into(), virtual_disks: None };
        }
        #[allow(unreachable_code)]
        if let Some(v) = VIRTUAL_STORAGE.get() {
            return v.clone();
        }
        let mke2fs = ["/sbin/mke2fs", "/usr/sbin/mke2fs"].iter().map(std::path::PathBuf::from).find(|p| p.exists()).unwrap_or_else(|| "/sbin/mke2fs".into());
        StorageEnv { roots: iyakki_core_api::device_types::storage::Roots::real(), mounter: Arc::new(iyakki_core_api::device_types::storage::SysMounter), mke2fs, scratch: "/run".into(), data_dir: "/var/lib/iyakki".into(), virtual_disks: None }
    }
}

const SLIDER_CHUNK: usize = 7001;
const SLIDER_OVERLAP: usize = 7002;
const SLIDER_TOPK: usize = 7003;
/// The level slider of Settings -> System sounds.
const SLIDER_SYSND: usize = 7050;
/// Sliders on the hardware pages are `SLIDER_HW + row`.
const SLIDER_HW: usize = 7100;

pub struct SettingsApp {
    tabs: Vec<Tab>,
    tab: usize,
    sel: [usize; 56],
    /// Which rows of a long list are on the page, per page (a click leaves them, a key scrolls to the choice, the wheel scrolls).
    win: [ListWindow; 56],
    /// Network, Display and Sound pages.
    hw: DevicesPane,
    /// The Bluetooth page (its own view model; a popup for pairing).
    bt: BtPage,
    /// Ticks since the app opened (the hardware pages refresh on a timer).
    ticks: u64,
    /// The full text of a row of a list whose line was cut to the width (Devices, paired computers, git hosts).
    tip: crate::tooltip::Tooltip,
    /// How long the mic tests listen (milliseconds); shortened by tests.
    pub test_record_ms: u64,
    /// Voice page: the language of the test sentence (index in `VO_LANGS`), and how far the lower part is scrolled.
    vo_lang: usize,
    vo_scroll: f32,
    vo_max: f32,
    /// The Voice page's controls take most of its room: the whole page scrolls (decided by the last frame).
    vo_tight: bool,
    /// Models page, Lemonade list: how far it is scrolled, its longest scroll, the height of its view, where it was drawn, the
    /// scale, whether the arrows scroll it, and its search box (and whether that has the keyboard).
    lem_scroll: f32,
    lem_max: f32,
    lem_view_h: f32,
    lem_rect: R,
    lem_s: f32,
    lem_focus: bool,
    lem_query: TextField,
    lem_search: bool,
    /// The heading and notes the Lemonade list drew last (tests read them).
    lem_texts: Vec<String>,
    /// Vault tab: 0 = system vault, 1 = personal vault.
    section: usize,
    /// May edit the skills shared with everyone (set from the policy at open).
    share_skills: bool,
    /// The search box over the list of pages, whether it has the keyboard, and how far the list is scrolled.
    nav_query: TextField,
    nav_focus: bool,
    nav_scroll: f32,
    /// Narrow window (under [`NARROW_PX`]): one column, either the page list (`show_list`) or one page with a Back button.
    narrow: bool,
    show_list: bool,
    /// The page the list was last scrolled to show (arrow keys and links change the page without scrolling the list).
    nav_seen: usize,
    /// Where the pointer is and where the list was drawn (the wheel scrolls the list when the pointer is over it).
    mouse: (i32, i32),
    nav_rect: R,
    nav_s: f32,
    /// Width of each slider's track as last drawn, by slider id (a click's x is turned into a value with it).
    slider_w: Vec<(usize, f32)>,
    storage: StorageEnv,
    wifi: WifiEnv,
    /// The networks the last scan found, in the order the picker shows them (`None`: the extra choices below the list).
    wifi_items: Vec<(String, Option<iyakki_core_api::device_types::devctl::wifi::Ap>)>,
    /// The live signal and sign-in line of the joined WiFi network.
    link: iyakki_shared::wifilink::LinkWatch,
    link_t0: Option<std::time::Instant>,
    devices: Vec<iyakki_core_api::device_types::storage::BlockDev>,
    modal: Option<Modal>,
    job: Option<Job>,
    /// The Host access page's state.
    host: host_access::HostPanel,
    crash: crashlog_page::CrashPanel,
    router: router_page::RouterPanel,
    backup: backup_page::BackupPanel,
    /// Settings -> Encrypted home.
    ench: enchome_page::EncPanel,
    upd: updates_page::UpdPanel,
    status: Option<(String, bool)>,
    /// Settings → Dev cluster.
    dc: devcluster::Dc,
    /// Settings → Memory.
    mem: memory::MemPanel,
    /// Settings -> Displays.
    disp: displays_page::DispPanel,
    /// Settings → Firewall.
    fw: firewall_page::FwPanel,
    /// Settings -> Network -> SSH tunnels.
    tun: sshtun_page::TunPanel,
    dom: dom_page::DomPanel,
    vpn: vpn_page::VpnPanel,
    remote: remote_page::RemotePanel,
    share: share_page::SharePanel,
    bh: bh_page::BhPanel,
    time: time::TimePanel,
    hn: time::NamePanel,
    /// Settings → Install to disk.
    inst: install_page::InstallPanel,
    /// Settings → Developer mode.
    devel: developer_page::DevPanel,
    /// Settings → Git and SSH keys.
    git: git_page::GitPanel,
    matrix: matrix_page::MatrixPanel,
    shares: shares_page::SharePanel,
    /// Models -> Model host on your network (a page over the settings).
    mh: modelhost_page::MhPanel,
    kbd: kbd_page::KbdPanel,
    /// The action buttons under a page when they need more rows than a third of the window: the bar scrolls (wheel, finger, thumb).
    btn_bar: crate::scrolllist::ScrollList,
    /// The scroll position of a page that does not scroll itself, and the page it belongs to.
    body_bar: crate::scrolllist::ScrollList,
    body_tab: usize,
    /// Settings → Battery (laptops only).
    bat: battery_page::BatPanel,
    asst: assistant_page::AsstPanel,
    /// Settings -> Power.
    pw: power_page::PwPanel,
    /// Cloud mode as decided for this machine: the hardware pages it hides are not in `tabs`.
    cloud: iyakki_cloudmode::CloudMode,
    /// Cloud access page: the name being typed, the removal waiting for its answer, the lists as last read.
    roles: cloudroles_page::RolesUi,
}

fn hf_on() -> bool {
    iyakki_shared::hostfiles::enabled()
}

fn kind_labels() -> Vec<&'static str> {
    Kind::ALL.iter().map(|k| k.label()).chain([AUTO_TYPE]).collect()
}

impl SettingsApp {
    pub fn new(cx: &AppCx) -> SettingsApp {
        let admin = cx.session.policy.manage_settings;
        let mut tabs = vec![Tab::Overview, Tab::Models, Tab::Rag, Tab::Vault];
        if cx.session.policy.skills {
            tabs.push(Tab::Skills);
        }
        if admin {
            tabs.insert(1, Tab::Agents);
        }
        if admin {
            tabs.extend([Tab::Network, Tab::Bluetooth, Tab::Display, Tab::Displays, Tab::Sound, Tab::SystemSounds, Tab::Keyboard]);
            tabs.push(Tab::Storage);
            tabs.push(Tab::Memory);
            tabs.push(Tab::Firewall);
            tabs.push(Tab::SshTunnels);
            tabs.push(Tab::Domains);
            tabs.push(Tab::Vpn);
            tabs.push(Tab::Remote);
            tabs.push(Tab::HostName);
            tabs.push(Tab::ModelRouter);
            tabs.push(Tab::Fleet);
            tabs.push(Tab::SessionLog);
            tabs.push(Tab::Startup);
            tabs.push(Tab::Updates);
            tabs.push(Tab::Developer);
            tabs.push(Tab::Cloud);
            tabs.push(Tab::CloudRoles);
            tabs.push(Tab::AppAccess);
        }
        if admin && !hf_on() && cx.os.installer().available() {
            tabs.push(Tab::Install);
        }
        if admin || cx.session.policy.voice {
            tabs.push(Tab::Voice);
        }
        if admin {
            tabs.push(Tab::SecretProviders);
        }
        if Self::dc_visible(cx) {
            tabs.push(Tab::DevCluster);
        }
        if cx.session.policy.allows_app(iyakki_sec::policy::App::Ide) {
            tabs.push(Tab::GitKeys);
        }
        tabs.push(Tab::HomeLayout);
        tabs.push(Tab::Views);
        tabs.push(Tab::Notifications);
        tabs.push(Tab::AppEvents);
        tabs.push(Tab::Assistant);
        if admin || cx.session.policy.voice {
            tabs.push(Tab::GeminiVoice);
        }
        // Alexa: an administrator always sees the page (to learn it needs the owner's product); others only once it is set up
        if (admin || cx.session.policy.voice) && (admin || iyakki_shared::alexa::configured(cx.data_dir())) {
            tabs.push(Tab::Alexa);
        }
        tabs.push(Tab::ScreenSaver);
        tabs.push(Tab::Power);
        if cx.session.policy.allows_app(iyakki_sec::policy::App::Radio) {
            tabs.push(Tab::Radio);
        }
        tabs.push(Tab::Appearance);
        tabs.push(Tab::Accessibility);
        tabs.push(Tab::Computer);
        tabs.push(Tab::Time);
        tabs.push(Tab::Browser);
        tabs.push(Tab::HostAccess);
        if admin || cx.session.policy.allows_app(iyakki_sec::policy::App::Ide) || cx.session.policy.allows_app(iyakki_sec::policy::App::Edit) {
            tabs.push(Tab::Shares);
        }
        tabs.push(Tab::Account);
        tabs.push(Tab::Backup);
        if cx.session.role != iyakki_sec::policy::Role::Guest {
            tabs.push(Tab::EncHome);
        }
        tabs.sort_by_key(|t| Tab::ORDER.iter().position(|o| o == t).unwrap_or(usize::MAX));
        let mut app = SettingsApp { tabs, tab: 0, sel: [0; 56], win: [ListWindow::default(); 56], hw: DevicesPane::new(DeviceEnv::real()), bt: BtPage::new(DeviceEnv::real()), ticks: 0, tip: Default::default(), test_record_ms: 2000, vo_lang: 0, vo_tight: false, vo_scroll: 0.0, vo_max: 0.0, lem_scroll: 0.0, lem_max: 0.0, lem_view_h: 0.0, lem_rect: R::new(0.0, 0.0, 0.0, 0.0), lem_s: 1.0, lem_focus: false, lem_query: TextField::new("Search models", false), lem_search: false, lem_texts: Vec::new(), section: if admin { 0 } else { 1 }, share_skills: cx.session.policy.manage_skills, nav_query: TextField::new("Search settings", false), nav_focus: false, nav_scroll: 0.0, narrow: false, show_list: false, nav_seen: usize::MAX, mouse: (0, 0), nav_rect: R::new(0.0, 0.0, 0.0, 0.0), nav_s: 1.0, slider_w: Vec::new(), storage: StorageEnv::real(), wifi: WifiEnv::real(), wifi_items: Vec::new(), link: Default::default(), link_t0: None, devices: vec![], modal: None, job: None, host: Default::default(), crash: Default::default(), router: Default::default(), backup: Default::default(), upd: Default::default(), status: None, dc: Default::default(), mem: memory::MemPanel::new(cx.os.memory()), disp: Default::default(), fw: Default::default(), tun: Default::default(), dom: Default::default(), vpn: Default::default(), remote: Default::default(), share: Default::default(), bh: Default::default(), time: Default::default(), hn: Default::default(), inst: install_page::InstallPanel::new(cx.os.installer()), devel: Default::default(), btn_bar: Default::default(), body_bar: Default::default(), body_tab: 0, git: Default::default(), matrix: Default::default(), shares: Default::default(), mh: Default::default(), kbd: Default::default(), bat: Default::default(), asst: Default::default(), pw: Default::default(), cloud: iyakki_cloudmode::CloudMode::desktop(), roles: Default::default(), ench: Default::default() };
        // the hardware pages go through the guarded handle: switching a radio or the brightness is asked of the broker
        app.hw.set_hardware(cx.os.hardware());
        // Bluetooth audio too: the phone's connection and the pairing window are the core's, behind `bluetooth.use`
        app.bt.model.set_bt_audio(cx.os.bt_audio());
        app.hw.set_core(cx.core.clone());
        app.bat_sync_tab();
        app.refresh_devices();
        app
    }

    /// Open on this page instead of the Overview (tests that drive one page).
    #[cfg(any(test, feature = "testing"))]
    pub(crate) fn on(mut self, t: Tab) -> SettingsApp {
        self.go_to(t);
        self
    }

    /// Cloud mode: leave out the pages that need hardware this machine does not have (the shell passes its decision).
    pub fn with_cloud(mut self, c: iyakki_cloudmode::CloudMode) -> SettingsApp {
        self.tabs.retain(|t| t.cloud_page().is_none_or(|p| !c.hides_page(p)));
        self.cloud = c;
        self
    }

    /// Replace the machine the hardware pages look at (tests).
    pub fn with_hardware(mut self, env: DeviceEnv) -> SettingsApp {
        self.bt = BtPage::new(env.clone());
        let core = self.hw.core();
        self.hw = DevicesPane::new(env);
        if let Some(c) = core {
            self.hw.set_core(c);
        }
        self
    }

    /// Replace what the private MAC address setting reads and changes (tests).
    pub fn with_mac(mut self, mac: iyakki_core_api::device_types::devctl::mac::MacEnv) -> SettingsApp {
        self.hw.set_mac(mac);
        self
    }

    /// Replace what the WiFi part acts on (tests).
    pub fn with_wifi(mut self, env: WifiEnv) -> SettingsApp {
        self.wifi = env;
        self
    }

    /// Replace the storage environment (tests).
    pub fn with_storage(mut self, env: StorageEnv) -> SettingsApp {
        self.storage = env;
        self.refresh_devices();
        self
    }

    fn refresh_devices(&mut self) {
        if self.tabs.contains(&Tab::Storage) {
            self.devices = iyakki_core_api::device_types::storage::list(&self.storage.roots);
            self.sel[self.tabs.iter().position(|t| *t == Tab::Storage).unwrap_or(0)] = self.sel[self.tabs.iter().position(|t| *t == Tab::Storage).unwrap_or(0)].min(self.devices.len().saturating_sub(1));
        }
    }

    /// A click on a slider's track at `x` pixels from its left edge.
    fn slider_click(&mut self, id: usize, x: f32, cx: &AppCx) {
        let Some(w) = self.slider_w.iter().find(|(i, _)| *i == id).map(|(_, w)| *w) else { return };
        match id {
            SLIDER_CHUNK | SLIDER_OVERLAP | SLIDER_TOPK if self.admin(cx) => {
                let rag = cx.settings().lock().unwrap().get().rag.clone();
                let result = cx.settings().lock().unwrap().update(|st| match id {
                    SLIDER_CHUNK => {
                        st.rag.chunk_max = controls::value_at(x, w, 100, 4000, 100) as usize;
                        // the overlap must stay under half the chunk size: bring it down with it
                        let top = (st.rag.chunk_max / 2).saturating_sub(1);
                        st.rag.overlap = st.rag.overlap.min(top) / 20 * 20;
                    }
                    SLIDER_OVERLAP => st.rag.overlap = controls::value_at(x, w, 0, (rag.chunk_max / 2).saturating_sub(1) as i64, 20) as usize,
                    _ => st.rag.top_k = controls::value_at(x, w, 1, 50, 1) as usize,
                });
                match result {
                    Ok(()) => {
                        cx.audit.record(&cx.session.user, "settings.rag", "");
                        self.ok("Saved.");
                    }
                    Err(e) => self.fail(e.to_string()),
                }
            }
            SLIDER_SYSND => self.sysnd_slider(x, w, cx),
            id if id >= SLIDER_HW => self.hw_slider(id, x, w, cx),
            _ => {}
        }
    }

    fn nav_items(&self) -> Vec<nav::Item> {
        self.tabs.iter().enumerate().map(|(i, t)| nav::Item { tab: i, label: t.label(), group: t.group(), keywords: t.keywords() }).collect()
    }

    /// The pages the search box lets through.
    pub fn nav_matches(&self) -> Vec<usize> {
        nav::entries(&self.nav_items(), &self.nav_query.value).iter().filter_map(|e| if let nav::Entry::Item { tab, .. } = e { Some(*tab) } else { None }).collect()
    }

    /// A key while the search box has the keyboard. Returns whether it was used.
    fn nav_key(&mut self, k: &Key) -> bool {
        let entries = nav::entries(&self.nav_items(), &self.nav_query.value);
        match k {
            Key::Esc => {
                // one press: clear the text and give the keyboard back to the page keys
                self.nav_query.set("");
                self.nav_focus = false;
            }
            Key::Enter => {
                if let Some(t) = nav::first_tab(&entries) {
                    self.tab = t;
                }
                self.nav_focus = false;
            }
            Key::Down | Key::Up => {
                if let Some(t) = nav::step_tab(&entries, self.tab, if *k == Key::Down { 1 } else { -1 }) {
                    self.tab = t;
                }
            }
            Key::Tab => self.nav_focus = false,
            other => {
                self.nav_query.handle(other);
                self.nav_after_edit();
            }
        }
        self.nav_scroll = nav::scroll_to_show(self.nav_scroll, &nav::entries(&self.nav_items(), &self.nav_query.value), self.tab, self.nav_rect.h, self.nav_s);
        true
    }

    /// The search text changed: keep the shown page among the matches (the first match becomes the page).
    fn nav_after_edit(&mut self) {
        let entries = nav::entries(&self.nav_items(), &self.nav_query.value);
        if !entries.iter().any(|e| matches!(e, nav::Entry::Item { tab, .. } if *tab == self.tab)) {
            if let Some(t) = nav::first_tab(&entries) {
                self.tab = t;
            }
        }
        self.nav_scroll = nav::scroll_to_show(self.nav_scroll, &entries, self.tab, self.nav_rect.h, self.nav_s);
    }

    pub fn tab(&self) -> Tab {
        self.tabs[self.tab]
    }
    pub fn tabs(&self) -> &[Tab] {
        &self.tabs
    }
    pub fn status(&self) -> Option<&(String, bool)> {
        self.status.as_ref()
    }
    pub fn has_modal(&self) -> bool {
        self.modal.is_some() || self.bt.has_dialog() || self.mh.open || self.git_wizard_open() || self.roles_open()
    }
    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    fn ok(&mut self, m: impl Into<String>) {
        let m = m.into();
        crate::access::announce(&m); // a screen reader says what the page says (nothing happens with the reader off)
        self.status = Some((m, true));
    }
    fn fail(&mut self, m: impl Into<String>) {
        let m = m.into();
        crate::access::announce(&format!("Error. {m}"));
        self.status = Some((m, false));
    }

    fn admin(&mut self, cx: &dyn super::settings_port::PagePort) -> bool {
        if cx.is_admin() {
            return true;
        }
        cx.audit("settings.denied", self.tab().label());
        self.fail("Only administrators can change this.");
        false
    }

    fn svc(cx: &AppCx) -> &Arc<dyn iyakki_agent_api::ServicesHandle> {
        &cx.services
    }

    fn sel(&self) -> usize {
        self.sel[self.tab]
    }

    /// The pages whose rows are windowed by a [`ListWindow`].
    fn scrolls_window(&self) -> bool {
        matches!(self.tab(), Tab::Network | Tab::SshTunnels | Tab::Domains | Tab::Vpn | Tab::Display | Tab::Sound | Tab::SystemSounds | Tab::ScreenSaver)
    }

    /// The row a pointer event chose (a click on a row, on a button or switch of one, or on a slider's track), if this page windows its rows.
    fn clicked_row(&self, ev: &Event) -> Option<usize> {
        if !self.scrolls_window() {
            return None;
        }
        match ev {
            Event::Row(i) => Some(*i),
            Event::Sub(row, _) => Some(*row as usize),
            Event::AreaAt(id, ..) if self.tab() == Tab::SystemSounds && *id == SLIDER_SYSND => Some(1),
            Event::AreaAt(id, ..) if *id >= SLIDER_HW => Some(*id - SLIDER_HW),
            _ => None,
        }
    }

    fn move_sel(&mut self, delta: i32, len: usize) {
        let i = &mut self.sel[self.tab];
        *i = (*i as i32 + delta).clamp_safe(0, len.saturating_sub(1) as i32) as usize;
    }

    // ------------------------------------------------------------------ data helpers

    fn providers(cx: &AppCx) -> Vec<ProviderCfg> {
        cx.settings().lock().unwrap().get().providers.clone()
    }

    fn externals(cx: &AppCx) -> Vec<ExtEntry> {
        cx.settings().lock().unwrap().get().external_rag.clone()
    }

    fn secret_providers(cx: &AppCx) -> Vec<(String, SecretProvider)> {
        cx.settings().lock().unwrap().get().secret_providers.clone().into_iter().collect()
    }

    fn secret_names(&self, cx: &AppCx) -> Result<Vec<String>, VaultError> {
        if self.section == 0 && cx.session.policy.manage_settings {
            cx.vault().system_names()
        } else {
            cx.vault().user_names(&cx.session.user)
        }
    }

    fn store_key(&mut self, cx: &AppCx, name: &str, value: &str, system: bool) -> Result<(), String> {
        let r = if system { cx.vault().system_set(name, value) } else { cx.vault().user_set(&cx.session.user, name, value) };
        r.map_err(|e| match e {
            VaultError::Locked if system => "The system vault is locked. Sign in as an administrator who has access to it.".to_string(),
            VaultError::Locked => "Your personal vault is locked (it could not be opened at sign-in).".to_string(),
            other => other.to_string(),
        })?;
        cx.audit.record(&cx.session.user, if system { "vault.system.set" } else { "vault.user.set" }, name);
        Ok(())
    }

    // ------------------------------------------------------------------ jobs

    fn start(&mut self, label: &str, f: impl FnOnce() -> JobOut + Send + 'static) {
        if self.job.is_some() {
            self.fail("Another operation is still running.");
            return;
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        self.job = Some(Job { label: label.into(), rx });
    }

    fn poll(&mut self, cx: &AppCx) {
        let Some(job) = &self.job else { return };
        let Ok(out) = job.rx.try_recv() else { return };
        self.job = None;
        match out {
            JobOut::Msg(Ok(m)) => {
                self.ok(m);
                self.mem_after_job();
                self.refresh_devices();
            }
            JobOut::Cast(r) => self.cast_done(r),
            JobOut::Wizard(r) => self.wiz_job_done(cx, r),
            JobOut::Cert(r) => self.cert_job_done(r),
            JobOut::HwCheck(card_id, r) => self.hw_check_done(card_id, r),
            JobOut::Enc(form) => self.ench_finished(cx, *form),
            JobOut::Backup(out) => self.backup_job_done(out),
            JobOut::ModelList(out) => self.mo_list_done(cx, out),
            JobOut::Dc(out) => self.dc_job_done(cx, out),
            JobOut::Mh(out) => self.mh_job_done(cx, out),
            JobOut::Wifi(Err(e)) => self.fail(e),
            JobOut::Wifi(Ok(aps)) => self.show_wifi_picker(aps),
            JobOut::Msg(Err(e)) => {
                self.fail(e);
                self.mem_after_job();
                self.refresh_devices();
            }
            JobOut::Models { provider, models, ms } => {
                let n = models.len();
                let r = cx.settings().lock().unwrap().update(|s| {
                    if let Some(p) = s.providers.iter_mut().find(|p| p.id == provider) {
                        p.models = models;
                    }
                });
                match r {
                    Ok(()) => self.ok(format!("{provider}: connected in {ms} ms, {n} model(s) available")),
                    Err(e) => self.fail(e.to_string()),
                }
            }
        }
    }

    // ------------------------------------------------------------------ forms

    // ------------------------------------------------------------------ modal handling

    fn handle_modal(&mut self, ev: &Event, cx: &AppCx) {
        let Some(modal) = self.modal.take() else { return };
        match modal {
            Modal::Form(kind, mut form) => match form.handle_event(ev) {
                FormEvent::Cancel => {
                    if matches!(kind, FormKind::BundleWizard) {
                        self.wiz_cancel(cx);
                    }
                }
                FormEvent::Changed(1) if matches!(kind, FormKind::Provider(_)) => {
                    // switching type refreshes the URL default unless the user typed their own
                    let old_default: Vec<String> = Kind::ALL.iter().map(|k| k.default_base().unwrap_or("").to_string()).collect();
                    if let Some(k) = Kind::ALL.get(form.choice_idx(1)).filter(|_| old_default.contains(&form.value(2)) || form.value(2).is_empty()) {
                        form.set_value(2, k.default_base().unwrap_or(""));
                    }
                    self.modal = Some(Modal::Form(kind, form));
                }
                FormEvent::Submit => {
                    if let Some(next) = self.submit_form(cx, kind.clone(), &mut form) {
                        self.modal = Some(next);
                    }
                }
                _ => self.modal = Some(Modal::Form(kind, form)),
            },
            Modal::Pick(kind, mut picker) => match picker.handle_event(ev) {
                PickEvent::Cancel => {}
                PickEvent::Chosen(item) => self.apply_pick(cx, kind, &item),
                PickEvent::None => self.modal = Some(Modal::Pick(kind, picker)),
            },
            Modal::Confirm(kind, text) => match ev {
                Event::Key(Key::Char('y')) => self.apply_confirm(cx, kind),
                // the hardware volume check has a real "No": that volume control does nothing, use the software one
                Event::Key(Key::Char('n')) if matches!(kind, ConfirmKind::SoundHwWorks(_)) => self.sound_hw_answer(cx, &kind, false),
                // Esc or any letter cancels; other input (clicks on the backdrop, arrows) keeps it open
                Event::Key(Key::Esc) | Event::Key(Key::Char(_)) => {}
                _ => self.modal = Some(Modal::Confirm(kind, text)),
            },
        }
    }

    /// Returns a modal to keep open (the form again, with an error) when saving failed.
    fn submit_form(&mut self, cx: &AppCx, kind: FormKind, form: &mut Form) -> Option<Modal> {
        // each lane answers the kinds it owns (settings_forms_<lane>.rs); the first one that does not say NotMine decides
        let mut answer = FormOutcome::NotMine;
        if matches!(answer, FormOutcome::NotMine) { answer = self.submit_form_a(cx, &kind, form); }
        if matches!(answer, FormOutcome::NotMine) { answer = self.submit_form_b(cx, &kind, form); }
        if matches!(answer, FormOutcome::NotMine) { answer = self.submit_form_c(cx, &kind, form); }
        if matches!(answer, FormOutcome::NotMine) { answer = self.submit_form_d(cx, &kind, form); }
        let result = match answer {
            FormOutcome::Early(modal) => return modal,
            FormOutcome::Result(r) => r,
            FormOutcome::NotMine => return None,
        };
        match result {
            Ok(m) => {
                self.ok(m);
                None
            }
            Err(e) => {
                form.error = Some(e);
                Some(Modal::Form(kind, form.clone()))
            }
        }
    }

    fn apply_pick(&mut self, cx: &AppCx, kind: PickKind, item: &str) {
        // each lane answers the ones it owns (settings_forms_<lane>.rs); the first lane that answers decides
        if self.apply_pick_a(cx, kind.clone(), item) { return; }
        if self.apply_pick_b(cx, kind.clone(), item) { return; }
        if self.apply_pick_c(cx, kind.clone(), item) { return; }
        self.apply_pick_d(cx, kind, item);
    }

    /// Saves what an answered confirmation returns: the end of the confirmation lanes.
    fn confirm_done(&mut self, cx: &AppCx, r: Result<String, String>) {
        match r {
            Ok(m) => {
                cx.audit.record(&cx.session.user, "settings.delete", &m);
                self.ok(m);
            }
            Err(e) => self.fail(e),
        }
    }

    fn apply_confirm(&mut self, cx: &AppCx, kind: ConfirmKind) {
        // each lane answers the ones it owns (settings_forms_<lane>.rs); the first lane that answers decides
        if self.apply_confirm_a(cx, kind.clone()) { return; }
        if self.apply_confirm_b(cx, kind.clone()) { return; }
        if self.apply_confirm_c(cx, kind.clone()) { return; }
        self.apply_confirm_d(cx, kind);
    }

    // ------------------------------------------------------------------ per-tab keys

    // ------------------------------------------------------------------ voice

    // ---- reading aloud: the engines of this computer, the voice, the speed, the test and the browser voice helper

    /// Number of selectable rows on the current tab.
    fn list_len(&mut self, cx: &AppCx) -> usize {
        match self.tab() {
            Tab::Overview => self.attention(cx).len(),
            Tab::Models => Self::providers(cx).len(),
            Tab::Agents => Self::agent_ids(cx).len(),
            Tab::Rag => Self::externals(cx).len(),
            Tab::Assistant => self.asst_rows(cx).len(),
            Tab::ModelRouter => self.router_len(cx),
            Tab::Fleet => self.fleet_len(cx),
            Tab::GeminiVoice => self.gv_rows().len(),
            Tab::Alexa => self.alexa_rows().len(),
            Tab::Vault => self.secret_names(cx).map_or(0, |n| n.len()),
            Tab::Backup => self.backup_len(),
            Tab::SecretProviders => Self::secret_providers(cx).len(),
            Tab::Skills => Self::svc(cx).skills().list(&cx.session.user).len(),
            Tab::Storage => self.devices.len(),
            Tab::Network | Tab::Display | Tab::Sound => self.dev_rows().len(),
            Tab::Bluetooth => 1,
            Tab::HostAccess => Self::host_rows(cx).len(),
            Tab::Developer => Self::devmode_keys(cx).len(),
            Tab::GitKeys => Self::git_len(cx),
            Tab::Shares => Self::shares_len(cx),
            Tab::Firewall => self.fw_len(),
            Tab::SshTunnels => self.tun_len(cx),
            Tab::Domains => self.dom_len(cx),
            Tab::Vpn => self.vpn_len(cx),
            Tab::DevCluster => self.dc_len(),
            Tab::Install => self.inst_len(),
            Tab::Updates => self.upd_len(),
            Tab::Keyboard => crate::fnkeys::KEYS.len(),
            Tab::HomeLayout => Self::hl_len(cx),
            Tab::Views => Self::vw_len(cx),
            Tab::SystemSounds => sysnd::ROWS,
            Tab::Notifications => Self::notif_len(cx),
            Tab::AppAccess => Self::appaccess_len(cx),
            Tab::AppEvents => Self::appevents_len(cx),
            Tab::Displays => self.disp_len(),
            Tab::ScreenSaver => Self::ss_len(cx),
            Tab::Browser if self.bh.open => self.bh_len(cx),
            Tab::Voice | Tab::Remote | Tab::Account | Tab::Browser | Tab::Memory | Tab::Time | Tab::Battery | Tab::Power | Tab::Radio | Tab::Accessibility | Tab::Appearance | Tab::EncHome | Tab::Computer | Tab::HostName | Tab::SessionLog | Tab::Cloud => 0,
            Tab::Startup => iyakki_shared::startup::CATALOGUE.len(),
            Tab::CloudRoles => self.roles_len(cx),
        }
    }

    fn open_form(&mut self, kind: FormKind, form: Form) {
        self.modal = Some(Modal::Form(kind, form));
    }

}

impl App for SettingsApp {
    /// A form, a list to pick from or a question drawn over Settings, in the words it shows (F-A11Y-02): the screen reader says it once when it opens.
    fn dialog_title(&self) -> Option<String> {
        Some(match self.modal.as_ref()? {
            Modal::Form(_, f) => f.title.clone(),
            Modal::Pick(_, p) => p.title.clone(),
            Modal::Confirm(_, text) => text.clone(),
        })
    }

    fn title(&self) -> &'static str {
        "Settings"
    }
    fn page(&self) -> String {
        self.tabs.get(self.tab).map(|t| t.label().to_string()).unwrap_or_default()
    }
    fn icon(&self) -> Icon {
        Icon::Settings
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        if self.modal.is_some() {
            return "Esc cancel".into();
        }
        match self.tab() {
            Tab::Overview => "Up/Down choose · Enter open the page that fixes it · / find a setting",
            Tab::Models => "a add · N add NVIDIA NIM (free key) · g Google Gemini (experimental) · C its consent · f detect Ollama/Lemonade · H Mac or GPU host · e edit · d delete · t test · m chat model · v vision model · y tidy model (dictation wording) · b embeddings · k key · x on/off · c test chat · l Lemonade models · K check my models (NVIDIA)",
            Tab::Agents => "d detect · i install · l login · o logout · k API key · m model · a Auto · u use for new chats · t test · n add custom · e edit · x hide/show",
            Tab::Assistant => "h remember on/off · f forget the selected history (press twice) · o show chats · t/T think limit up/down · q/Q silence limit up/down · 0 default",
            Tab::Alexa => "Up/Down choose · Enter or o on/off · C consent card (your own yes)",
            Tab::GeminiVoice => "Up/Down choose · Enter or +/- change · C consent card · v who speaks · k start key · w wake hand-off · o on/off · x Stop now",
            Tab::Rag => "a add backend · t test · r reindex · i ingest documents · o order passages · O reranking model · g Graph RAG · / find a page",
            Tab::Backup => "b back up · o open a backup file · Space include/leave out or choose/skip · r restore the chosen things · c close the file · k look again · n to a network share · l add a chain link · y machine data (admins) · Up/Down choose",
            Tab::Vault => "n new · d delete · Tab system/personal · l lock · f key file · u unlock my vault",
            Tab::Skills => "c add from clipboard · a quick create · i import file · d delete",
            Tab::Network => "Enter turn on/off · w join a WiFi network · o open the network's sign-in page · v use the other WiFi adapter · n announce this device's name · r look again",
            Tab::Bluetooth => "Up/Down choose · Enter does the main action (pair, connect) · e Bluetooth on/off · s scan or stop · / search · t phone tethering · u forget · a Advanced · r look again",
            Tab::Display => "u bigger text · U smaller text · + brighter · − dimmer · r look again",
            Tab::Sound => "o output · t test sound · + louder · − quieter · m mute · Enter use the selected output · A advanced · M mixer · r look again",
            Tab::Keyboard => "f function keys mode · i ISO layout · o swap Option/Command · d repeat delay · r repeat rate · [ ] mouse speed · , . touchpad speed · { } < > speed presets · s scroll speed · y wheel lines · w wheel direction · n trackpad direction · g smooth scrolling · m momentum · b test screen · k test keyboard light · t test sound · Up/Down scroll the key table",
            Tab::AppEvents => "Up/Down choose an event · Enter or t switch it off or on · p take the permission it needs away from the app · r look again",
            Tab::AppAccess => "Up/Down choose an access · r take it away (asks again at the next install) · c clear the log",
            Tab::Notifications => "Up/Down choose a source · s show · q quiet (inbox only) · o off · Enter next level · v describe camera alerts · a Matrix account · t test message · x remove it",
            Tab::SystemSounds => "Up/Down choose · Enter or Space switch on or off · p preview · + louder · − quieter",
            Tab::Cloud => "c change the mode: Automatic, On, Off",
            Tab::CloudRoles => "Up/Down choose · m add person · n new namespace · a add to namespace · j join · x remove person · d delete namespace",
            Tab::Computer => "s sound · d display · k keyboard and pointer · m microphone · g GPU · p power: per computer or shared · c copy settings from another computer · r reset this computer · f forget a computer · e b n v w startup disk (administrators)",
            Tab::EncHome => "c make my home encrypted · m move my existing files · u open · r open with the recovery key · l lock now · k new recovery key · a administrator slot · s close with the screen lock · x remove the old plain copy",
            Tab::Appearance => "[ ] Look · 1-5 or w wallpaper · f text font · p code font · s style · h shape · z size · i names · y layout · c columns · e spacing · d dock · a pin an app · x unpin · o usual apps · k g n v j top bar items",
            Tab::Accessibility => "r screen reader · t test the voice · n speak notices · c high contrast · u/U bigger/smaller text · p large pointer · f big focus ring · m reduce motion · k sticky keys · w slow keys",
            Tab::Radio => "r reconnect tries (0 to 5) · + more · - fewer · a go on to the next station on or off (administrators)",
            Tab::Power => "e performance · b balanced · v power saver · s sleep after idle (next time) · w sleep while busy (administrators)",
            Tab::Battery => "p percentage in the top bar · w low battery warnings · s shut down at the critical level (administrators)",
            Tab::Browser if self.bh.open => "a add · e edit · x or Enter enable/disable · d delete · b or Esc back · Up/Down choose",
            Tab::Browser => "Up/Down/PageUp/PageDown/Home/End scroll · g graphics (automatic, software, hardware) · t try the graphics card again · H custom headers · a scripts everywhere · i pictures · s styles · c cookies · m smooth pages · d developer tools for the chat agent · P protect browser data · X reset it · q video quality · y add-on decoders on or off · I known-bad sites on or off · J add one · O remove one · f forget the allowed sites",
            Tab::DevCluster => "t test · d my default · w workspace default · k next workspace · h hide/show · x disconnect · g registry · P platform services (Gitea, private store) · C clusters (several, local and remote) · v vault profile · r look again · s set up · b/o/e start/stop/restart · l logs",
            Tab::Storage if self.storage.virtual_disks.is_some() => "n new virtual disk · x delete it · m make this disk the data disk (erases it) · r rescan",
            Tab::Storage => "m make this disk the data disk (erases it) · d DISKS tile · o drives at sign-out · u compatible USB mode (no UAS) · r rescan · b reboot",
            Tab::ScreenSaver => "Up/Down choose a card · click a button or Enter for the next choice · p preview",
            Tab::Displays => "Up/Down choose a display · r resolution · p main display · a side · m mirror or extend · y apply (tried for 15 s) · k keep · v go back · i identify · e look at external monitors · b choose one · u brighter · j darker · s screen features",
            Tab::Memory => "z zram · c compressor · w swappiness · f swap file · x zswap · p preview · y apply now · v save for next boots · r read again",
            Tab::Time => "h 12/24-hour · s seconds · z time zone · u machine's own zone · n network time on/off · c time server · y ask the server now · b set the clock to the network time",
            Tab::HostName => "e change the name · y apply now and keep it · v save for next boots · r read again",
            Tab::Startup => "Up/Down choose a service · e retry the one that did not start · o open its page · r look again",
            Tab::SessionLog => "Up/Down/PageUp/PageDown/Home/End scroll · p other boot (crash, before, this one) · y I have seen it · r read again",
            Tab::GitKeys => "a connect Git or Gitea · T test · e edit · m make default · R remove a connection · n generate a key · i import · c copy the public key · u unlock · d delete · h host → key · t token · x remove a rule · f forget a known host · Up/Down choose",
            Tab::Developer if self.devel.usb_open => "r check again · z authorize · p pair (type the code, Enter) · s speed · c copy diagnostics · f fixed addresses · m allow more · u back · Up/Down scroll",
            Tab::Developer => "Click or tap anything, or: e on/off · h switch-off time · p pair a computer · C copy the code · c cancel it · Up/Down choose · x revoke · l activity log · u USB-C · Y relay · F relay certificate · W WASM tiles (experimental)",
            Tab::Updates => "Up/Down choose an update · i install · r roll back to the kept version · y yes, do it · n no · f choose a folder · s look again · c check for updates now · m how often to check · u address of the update list (administrators)",
            Tab::Install => "Up/Down choose a disk · i install (erases it) · p show the plan · t partition table (Auto/GPT/MBR) · r look again · b restart",
            Tab::Voice => "E echo cancellation · s speech-to-text · v text-to-speech · c commands · m mic test · p speaker test · t STT test · y test voice · e engine · g voice · l language · f speed · k re-check · o voice helper",
            Tab::SecretProviders => "a add · t test · d delete",
            Tab::Fleet => "Up/Down choose a row · c create a fleet · o offer a join code · j join with a code · y/Enter approve, accept or take the work · n refuse, drop or decline · x remove the member · l leave · t trust · A administrator · m accept-work setting · r look again",
            Tab::ModelRouter => "Up/Down choose a model · o router on/off · r role · c cost · z where it runs · e use it or not · u back to the hints · m farthest place · b budget · s strategy · a add a script · x remove a script · k cancel the run",
            Tab::HostAccess => "a share a folder · o options · m read-only/read-write · c convert to a copy · u unlink · i import files · l audit trail",
            Tab::Firewall if !self.fw.advanced => "1 Standard · 2 Open · a add something to allow · d remove · Up/Down choose · A advanced · k keep · v undo",
            Tab::Firewall => "A simple view · a add rule · e edit · x allow on/off · d delete · p presets · y apply (then k keep or v revert within 60 s) · o firewall on/off · u discard changes · r look names up · c copy rules · i import · b clear log · z disable now",
            Tab::Remote => "e or Enter serve/stop · p change the port · F start/stop SFTP · U K R W T C SFTP accounts, add key, remove key, password login, time limit, copy fingerprint · Up/Down scroll",
            Tab::Vpn => "i import a file · p import the clipboard · x or Enter enable/disable · m mode and filters · d delete · r retry · Up/Down choose",
            Tab::Domains => "a add route · e edit · d delete · x listener on/off · p ports · f firewall opening · t trial certificate · i import certificate · r look at the cluster again · Up/Down choose",
            Tab::SshTunnels => "a add · e edit · x enable/disable · d delete · T test · r retry · c copy the address · k copy the public key · Up/Down choose",
            Tab::Shares => "n new share · s start/stop · c copy the link · q QR code · e edit · t new secret for the link · d delete · x stop all sharing · m media server (TVs and players) on/off · Up/Down choose",
            Tab::Account => "p change password · u bigger text · U smaller text",
            Tab::Views => "Up/Down choose · n new view · t from a template · r rename · x delete · [ ] put in order · l lock on/off (asks for your password to switch) · k step keys · j your own step keys · o after the lock",
            Tab::HomeLayout => "Up/Down choose · n new group · r rename · x delete group · [ ] put your groups in order · m move the tile · p put it back · z reset layout",
        }
        .into()
    }

    fn take_request(&mut self) -> Option<(String, String)> {
        self.hw.take_request().or_else(|| self.bt.take_request())
    }

    fn open_item(&mut self, item: &str, cx: &AppCx) {
        // the GIT tile's "Add connection...": Settings, Git, with the wizard open
        if item == "git:add" {
            if let Some(i) = self.tabs.iter().position(|x| *x == Tab::GitKeys) {
                self.tab = i;
                self.git_open_wizard(cx, None);
            }
            return;
        }
        // `tab:network` and the like: another part of the shell (the Devices popover, the clock) asks for one page
        if let Some(name) = item.strip_prefix("tab:") {
            // `tab:clusters`: the Dev cluster page with the Clusters list open (the cluster switchers' "Manage clusters...")
            if name == "clusters" {
                if let Some(i) = self.tabs.iter().position(|x| *x == Tab::DevCluster) {
                    self.tab = i;
                    self.dc_open_clusters(cx);
                }
                return;
            }
            let want = Self::tab_for_name(name);
            if let Some(i) = want.and_then(|t| self.tabs.iter().position(|x| *x == t)) {
                self.tab = i;
            }
            return;
        }
        // Store, Matrix server: Settings, Notifications, with the account form open at the server's address
        if item == "matrix:add" {
            if let Some(i) = self.tabs.iter().position(|x| *x == Tab::Notifications) {
                self.tab = i;
                self.matrix_open_form(iyakki_shared::quickinstall::matrix::HOMESERVER);
            }
            return;
        }
        if let Some(which) = item.strip_prefix("modelhost:") {
            if let Some(i) = self.tabs.iter().position(|t| *t == Tab::Models) {
                self.tab = i;
                self.mh_open(cx, which != "pc");
            }
            return;
        }
        if item == "voice" {
            if let Some(i) = self.tabs.iter().position(|t| *t == Tab::Voice) {
                self.tab = i;
                self.vo_scroll = 0.0;
                cx.host_tts().ensure();
            }
        }
        if let Some(folder) = item.strip_prefix("share:") {
            if let Some(i) = self.tabs.iter().position(|t| *t == Tab::Shares) {
                self.tab = i;
                self.shares.pending = Some(folder.to_string());
            }
        }
        if let Some(id) = item.strip_prefix("platform:") {
            if let Some(i) = self.tabs.iter().position(|t| *t == Tab::DevCluster) {
                self.tab = i;
                self.dc_open_platform(cx, Some(id));
            }
        }
    }
    fn wants_redraw(&mut self) -> bool {
        self.tip.take_changed()
    }
    /// Smooth wheel steps move what is under the pointer by pixels: the Scroll test, the model list, the Voice page and the page
    /// list on the left. Everything else keeps whole notches (the shell sends those when this says no).
    fn scroll_px(&mut self, px: i32, _cx: &AppCx) -> bool {
        if self.modal.is_some() || self.mh.open || self.host_dialog_open() || self.dc_has_overlay() || self.dc_platform_open() || self.git_wizard_open() {
            return false;
        }
        let over_nav = self.nav_rect.contains(self.mouse.0, self.mouse.1);
        let notches = px as f32 / crate::scrolleng::NOTCH_PX;
        let before = (self.kbd.test.offset, self.lem_scroll, self.vo_scroll, self.nav_scroll, self.remote.scroll, self.share.scroll, self.crash.scroll);
        let over_body = self.body_bar.scrolls() && self.body_bar.view.contains(self.mouse.0, self.mouse.1);
        let over_bar = self.btn_bar.scrolls() && self.btn_bar.view.contains(self.mouse.0, self.mouse.1);
        let taken = match self.tab() {
            _ if over_body => {
                self.body_bar.pointer = Some(self.mouse);
                self.body_bar.wheel_px(px)
            }
            _ if over_bar => {
                self.btn_bar.pointer = Some(self.mouse);
                self.btn_bar.wheel_px(px)
            }
            Tab::Keyboard if !over_nav => self.kbd_test_px(px),
            Tab::Models if !over_nav && self.lem_max > 0.0 && self.lem_rect.contains(self.mouse.0, self.mouse.1) => {
                self.lem_focus = true;
                self.lem_scroll_by(-notches * crate::scrolleng::SCREEN_NOTCH_PX * self.nav_s);
                true
            }
            _ if !over_nav && self.page_scrolls_by_pixels() => self.page_scroll_by(-notches * crate::scrolleng::SCREEN_NOTCH_PX),
            Tab::DevCluster | Tab::Developer | Tab::Memory | Tab::Install | Tab::Models | Tab::Keyboard | Tab::Voice => false,
            _ if over_nav => {
                let entries = nav::entries(&self.nav_items(), &self.nav_query.value);
                self.nav_scroll = nav::clamp_scroll(self.nav_scroll - notches * 48.0 * self.nav_s, &entries, self.nav_rect.h, self.nav_s);
                true
            }
            _ => false,
        };
        if taken && before == (self.kbd.test.offset, self.lem_scroll, self.vo_scroll, self.nav_scroll, self.remote.scroll, self.share.scroll, self.crash.scroll) {
            crate::scrolleng::note_bound(); // at an end: momentum stops
        }
        taken
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        self.tip.event(ev);
        // a click chooses the row where it is: the page's list stays put (a key that moves the choice scrolls to it when drawn)
        if let Some(row) = self.clicked_row(ev) {
            self.win[self.tab].click_select(row);
        }
        if matches!(ev, Event::Tick) {
            self.ticks += 1;
            self.poll(cx);
            self.link_tick(cx);
            self.host_dialog_event(ev);
            if self.tab() == Tab::DevCluster {
                self.dc_overlay_event(ev, cx);
                self.dc_prepare(cx);
            }
            if self.dc_platform_open() {
                self.dc_platform_event(ev);
            }
            self.ench_tick(cx);
            self.git_tick(cx);
            self.tun_tick(cx);
            self.dom_tick(cx);
            self.vpn_tick(cx);
            self.matrix_tick();
            self.git_wizard_event(ev, cx);
            if self.tab() == Tab::Bluetooth {
                self.bt_tick(cx);
            }
            return Flow::Stay;
        }
        // "Connect Git or Gitea" takes every key and click while it is open
        if self.git_wizard_event(ev, cx) {
            return Flow::Stay;
        }
        if self.tab() == Tab::DevCluster && self.dc_platform_open() {
            self.dc_platform_event(ev);
            return Flow::Stay;
        }
        if self.tab() == Tab::DevCluster && self.dc_has_overlay() {
            self.dc_overlay_event(ev, cx);
            return Flow::Stay;
        }
        // a Host access dialog takes every key and click while it is open
        if self.host_dialog_event(ev) {
            return Flow::Stay;
        }
        if self.roles_event(ev, cx) {
            return Flow::Stay;
        }
        if self.modal.is_some() {
            self.handle_modal(ev, cx);
            return Flow::Stay;
        }
        if self.tab() == Tab::Bluetooth && self.bluetooth_event(ev, cx) {
            return Flow::Stay;
        }
        if self.mh_event(ev, cx) {
            return Flow::Stay;
        }
        if self.body_bar.scrolls() && self.body_event(ev) {
            return Flow::Stay;
        }
        if self.btn_bar.scrolls() && self.bar_event(ev) {
            return Flow::Stay;
        }
        if self.tab() == Tab::Keyboard && self.kbd_test_event(ev) {
            return Flow::Stay;
        }
        if self.tab() == Tab::Developer && (self.usb_event(ev, cx) || self.relay_pin_event(ev, cx)) {
            return Flow::Stay;
        }
        if self.tab() == Tab::Memory && self.memory_event(ev) {
            return Flow::Stay;
        }
        if self.tab() == Tab::Install && self.inst.plan.handle(ev) {
            return Flow::Stay;
        }
        // a click on a button takes the typing focus away from the search box (a button may arrive as a key)
        if self.nav_focus && crate::ui::from_click() && !matches!(ev, Event::FocusAt(id, _) if *id == nav::SEARCH_ID) {
            self.nav_focus = false;
        }
        match ev {
            Event::MouseMove(x, y) => self.mouse = (*x, *y),
            Event::Tab(i) => {
                self.tab = (*i).min(self.tabs.len() - 1);
                self.nav_focus = false;
                self.show_list = false; // narrow window: show the page that was chosen
                self.nav_scroll = nav::scroll_to_show(self.nav_scroll, &nav::entries(&self.nav_items(), &self.nav_query.value), self.tab, self.nav_rect.h, self.nav_s);
            }
            Event::FocusAt(id, x) if *id == nav::SEARCH_ID => {
                self.nav_focus = true;
                self.nav_query.click(*x);
            }
            Event::Paste(t) if self.nav_focus => {
                self.nav_query.paste(t);
                self.nav_after_edit();
            }
            Event::AreaAt(id, x, _) => self.slider_click(*id, *x, cx),
            // a button on a slider card: select that row, then press the key on it
            // a model of the Lemonade panel on Settings → Models
            Event::Sub(i, Key::Char(c)) if self.tab() == Tab::Models && Self::lem_chip_index(*i, *c).is_some() => {
                self.lem_focus = true;
                self.lem_search = false;
                self.lem_chip(Self::lem_chip_index(*i, *c).unwrap_or(0), cx)
            }
            Event::Sub(row, Key::Char(c @ ('U' | 'D'))) if self.tab() == Tab::Network && self.share.open => self.share_pick(*row as usize, *c),
            Event::Sub(row, Key::Char(c)) if self.tab() == Tab::Notifications => self.notif_sub(*row as usize, *c, cx),
            Event::Sub(row, Key::Char(c)) if self.tab() == Tab::AppEvents => self.appevents_sub(*row as usize, *c, cx),
            Event::Sub(row, Key::Char(c)) if self.tab() == Tab::AppAccess => self.appaccess_sub(*row as usize, *c, cx),
            Event::Sub(row, Key::Char(c)) if self.tab() == Tab::SystemSounds => self.sysnd_sub(*row as usize, *c, cx),
            Event::Sub(row, Key::Char(c)) if self.tab() == Tab::ScreenSaver => self.ss_sub(*row as usize, *c, cx),
            Event::Sub(row, k) if self.tab() == Tab::Developer => self.dev_sub(*row, k, cx),
            Event::Sub(row, k) if self.dev_tab().is_some() => {
                self.sel[self.tab] = *row as usize;
                match self.tab() {
                    Tab::Display => self.handle_display(k, cx),
                    Tab::Sound => self.handle_sound(k, cx),
                    _ => self.handle_hw(k, cx),
                }
            }
            Event::Row(i) => {
                let len = self.list_len(cx);
                self.sel[self.tab] = (*i).min(len.saturating_sub(1));
            }
            Event::FocusAt(id, x) if *id == lemonade_page::LEM_SEARCH_ID => {
                self.lem_search = true;
                self.nav_focus = false;
                self.lem_query.click(*x);
            }
            Event::Paste(t) if self.lem_search && self.tab() == Tab::Models => {
                self.lem_query.paste(t);
                self.lem_scroll = 0.0;
            }
            Event::Key(k) if self.lem_search && self.tab() == Tab::Models && self.modal.is_none() => self.lem_search_key(k),
            Event::Key(Key::Char('/')) if self.tab() == Tab::Models && Self::providers(cx).get(self.sel()).is_some_and(|p| p.kind == Kind::Lemonade) => {
                self.lem_search = true;
                self.nav_focus = false;
            }
            Event::Key(Key::Esc) if self.tab() == Tab::Models && !self.lem_query.value.is_empty() => {
                self.lem_query.set("");
                self.lem_scroll = 0.0;
            }
            Event::Scroll(d) if self.tab() == Tab::Models && self.lem_max > 0.0 && self.lem_rect.contains(self.mouse.0, self.mouse.1) => {
                self.lem_focus = true;
                self.lem_scroll_by(-*d as f32 * crate::scrolleng::SCREEN_NOTCH_PX * self.nav_s);
            }
            Event::Scroll(d) if self.tab() == Tab::Developer && !self.nav_rect.contains(self.mouse.0, self.mouse.1) => self.dev_wheel(*d),
            Event::Scroll(d) if self.page_scrolls_by_pixels() && !self.nav_rect.contains(self.mouse.0, self.mouse.1) && self.page_scroll_by(-*d as f32 * crate::scrolleng::SCREEN_NOTCH_PX) => {}
            // the wheel scrolls the rows of these pages, not the choice
            Event::Scroll(d) if self.scrolls_window() && !self.nav_rect.contains(self.mouse.0, self.mouse.1) => self.win[self.tab].wheel_by(-*d * ROWS_PER_NOTCH),
            Event::Scroll(d) => {
                if self.nav_rect.contains(self.mouse.0, self.mouse.1) {
                    let entries = nav::entries(&self.nav_items(), &self.nav_query.value);
                    self.nav_scroll = nav::clamp_scroll(self.nav_scroll - *d as f32 * 48.0 * self.nav_s, &entries, self.nav_rect.h, self.nav_s);
                } else {
                    let len = self.list_len(cx);
                    self.move_sel(-*d, len);
                }
            }
            Event::Key(k) if self.nav_focus => {
                self.nav_key(k);
            }
            // find a page: Ctrl+F, or / when no page needs the key
            Event::Key(Key::Ctrl('f')) | Event::Key(Key::Char('/')) => self.nav_focus = true,
            Event::Key(Key::Esc) if self.bh_open() && self.modal.is_none() => self.bh.open = false,
            // a narrow window: Esc (or the Back button) goes from the page to the list, and from the list out of Settings
            Event::Key(Key::Esc) if self.narrow && !self.show_list => self.show_list = true,
            Event::Key(Key::Esc) => return Flow::Back,
            Event::Key(Key::Left) => {
                self.tab = self.tab.saturating_sub(1);
                self.show_list = false;
            }
            Event::Key(Key::Right) => {
                self.tab = (self.tab + 1).min(self.tabs.len() - 1);
                self.show_list = false;
            }
            Event::Key(other) => self.page_key(other, cx),
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        // left: search and the list of pages; right: the page
        // below about 900 px the window is one column: the page list, or one page with a Back button to the list
        // bigger text takes the room of a bigger window: at Larger and Huge the list and the page no longer fit side by side
        let narrow = area.w < narrow_px();
        self.narrow = narrow;
        let show_nav = !narrow || self.show_list;
        let show_page = !narrow || !self.show_list;
        let pad = 20.0 * s;
        let nav_w = if narrow { area.w - 2.0 * pad } else { 260.0 * s };
        let nav_r = R::new(area.x + pad, area.y + 10.0 * s, nav_w, area.h - 20.0 * s);
        self.nav_rect = if show_nav { nav_r } else { R::new(0.0, 0.0, 0.0, 0.0) };
        self.nav_s = s;
        let entries = nav::entries(&self.nav_items(), &self.nav_query.value);
        self.nav_scroll = nav::clamp_scroll(self.nav_scroll, &entries, nav_r.h, s);
        if self.nav_seen != self.tab {
            self.nav_seen = self.tab;
            self.nav_scroll = nav::scroll_to_show(self.nav_scroll, &entries, self.tab, nav_r.h, s);
        }
        if show_nav {
            nav::draw(g, nav_r, s, &entries, self.tab, self.nav_scroll, &self.nav_query, self.nav_focus, blink(tick), ACCENT);
        }
        let (x, w) = if narrow { (area.x + pad, (area.w - 2.0 * pad).max(120.0)) } else { let x = nav_r.x + nav_r.w + 28.0 * s; (x, (area.x + area.w - pad - x).max(120.0 * s)) };
        if show_page {
        let page = self.tab();
        let mut head_y = area.y + 14.0 * s;
        let tsize = if narrow { 24.0 * s } else { 28.0 * s };
        if narrow {
            // the way back to the list of pages: a button at least 44 px tall, also reached with Esc
            g.button_touch(x, area.y + 12.0 * s, 15.0 * s, "< All settings", ACCENT, Event::Key(Key::Esc), 44.0 * s);
            head_y = area.y + 56.0 * s;
        }
        let title = g.fit(tsize, page.label(), Face::Bold, w);
        g.text(x, head_y, tsize, TEXT, &title, Face::Bold);
        let blurb_text = if self.nav_focus && !self.nav_query.value.is_empty() { "Typing goes to the search box. Press Esc to use the page keys." } else { page.blurb() };
        let bar_h;
        if narrow {
            let lines = g.wrap(14.0 * s, blurb_text, Face::Sans, w);
            let shown = lines.len().min(3);
            for (k, l) in lines.iter().take(shown).enumerate() {
                g.text(x, head_y + 34.0 * s + k as f32 * 19.0 * s, 14.0 * s, MUTED, l, Face::Sans);
            }
            bar_h = head_y - area.y + 40.0 * s + shown as f32 * 19.0 * s;
        } else {
            let blurb = g.fit(14.0 * s, blurb_text, Face::Sans, w);
            g.text(x, head_y + 38.0 * s, 14.0 * s, MUTED, &blurb, Face::Sans);
            bar_h = 88.0 * s;
        }
        if self.nav_focus && show_nav {
            g.rrect_outline(nav_r.x, nav_r.y, nav_r.w, nav_r.h, 16.0 * s, 2.0 * s, ACCENT);
        }
        let admin = cx.session.policy.manage_settings;
        if self.dev_tab().is_some() {
            // the buttons of a hardware page depend on its snapshot (Bluetooth: Firmware, Load driver); read it before measuring the bar,
            // or the first frame measures one row and draws two, and the second row falls below the window
            let ticks = self.ticks;
            self.hw.prime(ticks);
        }
        let rows_all = self.measured_button_rows(g, w, s, admin);
        // at big text or a small window the buttons would take the whole window and push the page under the header: the bar keeps to a
        // third of the window and scrolls
        let rows_cap = ((area.h / 3.0) / (BTN_PITCH * s)).floor().max(1.0) as usize;
        let rows_of_buttons = rows_all.min(rows_cap);
        let btn_h = rows_of_buttons as f32 * BTN_PITCH * s;
        self.slider_w.clear();
        let body = R::new(x, area.y + bar_h, w, area.h - bar_h - 40.0 * s - btn_h);
        let no_match = entries.is_empty();
        if no_match {
            // not a whole page under a "Nothing matches" message
            g.text(body.x, body.y + 10.0 * s, 16.0 * s, MUTED, "No page matches the search.", Face::Sans);
            g.text(body.x, body.y + 38.0 * s, 14.0 * s, DIM, "Press Esc to clear it and use the page keys.", Face::Sans);
        } else {
            // (the Browser page scrolls itself, by `bh`; a second bar over it would take the wheel whenever the font made its ink taller than the room)
            // a page that does not scroll itself gets a scroll bar when it is taller than its room (big text, a small window)
            let generic = !(matches!(self.tab(), Tab::Developer | Tab::Remote | Tab::Voice | Tab::SessionLog | Tab::Keyboard | Tab::DevCluster | Tab::Bluetooth) || (self.tab() == Tab::Network && self.share.open) || (self.tab() == Tab::Browser && !self.bh.open));
            if generic {
                if self.body_tab != self.tab {
                    self.body_tab = self.tab;
                    self.body_bar.to_top();
                }
                let off = self.body_bar.offset;
                let shifted = R::new(body.x, body.y - off, body.w, body.h + off);
                let saved = std::mem::replace(&mut g.ink_bottom, 0.0);
                g.clipped(body, |g| self.render_page(g, shifted, s, cx));
                let ink = std::mem::replace(&mut g.ink_bottom, saved);
                // the content's height from where it started (the end of the last text or click region, and a little room)
                let content_h = if ink > 0.0 { ink - shifted.y + 8.0 * s } else { 0.0 };
                // the scroll bar sits in the margin to the right of the page
                self.body_bar.layout(R::new(body.x, body.y, body.w + 16.0 * s, body.h), content_h, 48.0 * s, s);
                self.body_bar.draw_bar(g, s, BODY_BAR, ACCENT);
            } else if matches!(self.tab(), Tab::Keyboard | Tab::Voice | Tab::Remote | Tab::SessionLog) {
                // these scroll inside their room already; nothing of them is drawn over the button bar under it
                self.body_bar.layout(R::new(0.0, 0.0, 0.0, 0.0), 0.0, 48.0 * s, s);
                g.clipped(body, |g| self.render_page(g, body, s, cx));
            } else {
                self.body_bar.layout(R::new(0.0, 0.0, 0.0, 0.0), 0.0, 48.0 * s, s);
                self.render_page(g, body, s, cx);
            }
        }
        if !no_match {
            let view = R::new(x, area.y + area.h - 36.0 * s - btn_h + 4.0 * s, w, btn_h);
            if rows_all > rows_of_buttons {
                // the scroll bar sits in the margin to the right of the buttons
                self.btn_bar.layout(R::new(view.x, view.y, view.w + 16.0 * s, view.h), rows_all as f32 * BTN_PITCH * s, BTN_PITCH * s, s);
                let off = self.btn_bar.offset;
                let this = &*self;
                g.clipped(view, |g| this.render_buttons(g, x, view.y - off, w, s, admin));
                self.btn_bar.draw_bar(g, s, BTN_BAR, ACCENT);
            } else {
                self.btn_bar.layout(R::new(0.0, 0.0, 0.0, 0.0), 0.0, BTN_PITCH * s, s);
                self.render_buttons(g, x, view.y, w, s, admin);
            }
        }
        // status line
        let y = area.y + area.h - 30.0 * s;
        if let Some(j) = &self.job {
            g.text(x, y, 15.0 * s, MUTED, &format!("{}{}", j.label, ".".repeat(1 + (tick / 10 % 3) as usize)), Face::Sans);
        } else if let Some((m, ok)) = &self.status {
            crate::fx::status_line(g, x, y, w, s, m, *ok);
        }
        }
        let modal_area = R::new(area.x, area.y, area.w, area.h);
        self.render_host_dialog(g, modal_area, s, tick);
        if self.mh_is_open() {
            self.mh_render(g, modal_area, s, cx, tick);
        }
        if self.tab() == Tab::Bluetooth {
            self.bt.render_dialogs(g, modal_area, s);
        }
        match &self.modal {
            Some(Modal::Form(_, f)) => f.render(g, modal_area, s, ACCENT, blink(tick)),
            Some(Modal::Pick(_, p)) => p.render(g, modal_area, s, ACCENT),
            Some(Modal::Confirm(ConfirmKind::SoundHwWorks(_), text)) => render_confirm(g, modal_area, s, text, "Yes, quieter"),
            Some(Modal::Confirm(_, text)) => render_confirm(g, modal_area, s, text, "Yes, do it"),
            None => {}
        }
        self.git_wizard_render(g, modal_area, s, tick);
        // last, over everything: the whole text of the row the pointer rests on (or the keyboard selected) when its line was cut
        self.tip.draw(g, s);
    }
}

// ---------------------------------------------------------------------- rendering

/// Below this window width (pixels) the Settings window is one column.
impl SettingsApp {
}

/// The scroll bar of the action buttons under a page.
const BTN_BAR: usize = 9730;
/// The scroll bar of a page body that is taller than its room.
const BODY_BAR: usize = 9731;

const NARROW_PX: f32 = 900.0;

/// [`NARROW_PX`] grows with the person's text size above Normal (200% text: one column below 1800 px).
fn narrow_px() -> f32 {
    NARROW_PX
}

type Btn = (&'static str, [u8; 4], char);

/// The row height of the action buttons under a page: the hit area is this tall (about 44 px at scale 1), the drawn pill less.
const BTN_PITCH: f32 = 44.0;

/// The languages the voice test and the voice list can be shown for.
const VO_LANGS: [&str; 5] = ["English", "Tamil", "Hindi", "Telugu", "Malayalam"];

/// What the page says about each engine: found ones with their voices and what they can do, then the unusable and missing ones
/// with why and how to get them. (text, colour, size at scale 1)
fn vo_engine_lines(r: &hostengines::Report, tag: &str) -> Vec<(String, [u8; 4], f32)> {
    let mut out = Vec::new();
    for e in &r.engines {
        let speaks = if e.speaks(tag) { format!("speaks {}", hostengines::lang_name(tag)) } else { format!("no {} voice", hostengines::lang_name(tag)) };
        match &e.problem {
            None => out.push((format!("● {} · {} voices · {speaks} · {}{}", e.kind.label(), e.voices.len(), if e.kind.can_file() { "can save audio" } else { "play only: Save audio is unavailable" }, if e.kind.can_speak() { "" } else { " · needs a sound player" }), OK, 14.0)),
            Some(why) => out.push((format!("○ {}: {why}", e.kind.label()), MUTED, 14.0)),
        }
    }
    for (k, why) in &r.missing {
        out.push((format!("○ {}: {why}", k.label()), DIM, 13.0));
    }
    out
}

/// Buttons that wrap; each sends its key. Returns the height used.
fn vo_flow(g: &mut Gfx, x: f32, y: f32, w: f32, size: f32, items: &[(String, char)], col: impl Fn(char) -> [u8; 4], s: f32) -> f32 {
    let (mut bx, mut by) = (x, y);
    for (label, key) in items {
        let bw = g.text_width(size, label, Face::Bold) + size * 1.7;
        if bx > x && bx + bw > x + w {
            bx = x;
            by += 36.0 * s;
        }
        bx += g.button(bx, by, size, label, col(*key), Event::Key(Key::Char(*key))) + 8.0 * s;
    }
    by + 36.0 * s - y
}

impl SettingsApp {
    fn buttons_for(&self, admin: bool) -> Vec<Btn> {
        if self.tab() == Tab::DevCluster && self.dc_platform_open() {
            return vec![]; // Platform services draws its own buttons
        }
        self.page_buttons(admin)
    }

    /// Rows the button bar needs (buttons wrap).
    fn button_rows(&self, admin: bool) -> usize {
        if self.tab() == Tab::DevCluster && self.dc_platform_open() {
            0
        } else if self.tab() == Tab::DevCluster {
            self.dc_button_rows()
        } else if self.tab() == Tab::Overview {
            0
        } else if self.tab() == Tab::Firewall {
            if self.fw.advanced { 4 } else { 2 }
        } else if self.tab() == Tab::Shares {
            3
        } else if (self.tab() == Tab::Voice && admin) || self.tab() == Tab::Agents || self.tab() == Tab::Memory || self.tab() == Tab::Displays || self.tab() == Tab::Time || self.tab() == Tab::Sound || self.tab() == Tab::SystemSounds || self.tab() == Tab::Keyboard || self.tab() == Tab::GitKeys || self.tab() == Tab::HomeLayout || self.tab() == Tab::Views || self.tab() == Tab::Notifications || (self.tab() == Tab::Models && admin) {
            2
        } else {
            1
        }
    }

    /// Rows the button bar really needs at this width (buttons wrap, so a narrow window needs more than the usual count). Pages that
    /// draw their own bar keep their own count.
    fn measured_button_rows(&self, g: &mut Gfx, w: f32, s: f32, admin: bool) -> usize {
        let own = if self.tab() == Tab::DevCluster { self.button_rows(admin) } else { 0 };
        if self.tab() == Tab::DevCluster && own == 0 {
            return 0; // Platform services draws its own buttons
        }
        let (mut rows, mut bx) = (0usize, 0.0f32);
        for (label, _, _) in self.buttons_for(admin) {
            // a button wider than the whole row is cut to the row (see `render_buttons`)
            let bw = (g.text_width(14.0 * s, label, Face::Bold) + 14.0 * s * 1.7).min(w);
            if rows == 0 || bx + bw > w {
                rows += 1;
                bx = 0.0;
            }
            bx += bw + 8.0 * s;
        }
        rows.max(own)
    }

    /// The pointer events of the scrolling button bar (the wheel over it, its scroll bar). True when it took the event.
    fn bar_event(&mut self, ev: &Event) -> bool {
        match ev {
            Event::Scroll(_) if self.btn_bar.view.contains(self.mouse.0, self.mouse.1) => {
                self.btn_bar.pointer = Some(self.mouse);
                self.btn_bar.event(ev, BTN_BAR)
            }
            Event::MouseMove(..) | Event::MouseUp(..) => self.btn_bar.event(ev, BTN_BAR),
            Event::AreaAt(id, ..) if *id == BTN_BAR => self.btn_bar.event(ev, BTN_BAR),
            _ => false,
        }
    }

    /// The pointer events of a page body that scrolls by this app's own bar. True when it took the event.
    fn body_event(&mut self, ev: &Event) -> bool {
        match ev {
            Event::Scroll(_) if self.body_bar.view.contains(self.mouse.0, self.mouse.1) && !self.modal_open_for_scroll() => {
                self.body_bar.pointer = Some(self.mouse);
                self.body_bar.event(ev, BODY_BAR)
            }
            Event::MouseMove(..) | Event::MouseUp(..) => self.body_bar.event(ev, BODY_BAR),
            Event::AreaAt(id, ..) if *id == BODY_BAR => self.body_bar.event(ev, BODY_BAR),
            _ => false,
        }
    }

    fn modal_open_for_scroll(&self) -> bool {
        self.modal.is_some() || self.mh.open || self.host_dialog_open() || self.dc_has_overlay() || self.dc_platform_open() || self.git_wizard_open()
    }

    fn render_buttons(&self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, admin: bool) {
        let (mut bx, mut by) = (x, y);
        for (label, col, key) in self.buttons_for(admin) {
            // a label wider than the whole row (a narrow window) is cut with an ellipsis instead of running out of the page
            let fitted;
            let label = if g.text_width(14.0 * s, label, Face::Bold) + 14.0 * s * 1.7 > w {
                fitted = g.fit(14.0 * s, label, Face::Bold, (w - 14.0 * s * 1.7).max(40.0 * s));
                fitted.as_str()
            } else {
                label
            };
            let bw = g.text_width(14.0 * s, label, Face::Bold) + 14.0 * s * 1.7;
            if bx + bw > x + w {
                bx = x;
                by += BTN_PITCH * s;
            }
            // 44 px tall to hit (a finger), the pill itself stays 30 px
            bx += g.button_touch(bx, by + (BTN_PITCH - 14.0 * 2.1) * s / 2.0 - 2.0 * s, 14.0 * s, label, col, Event::Key(Key::Char(key)), BTN_PITCH * s) + 8.0 * s;
        }
    }
}

/// One row of a list page: title, a line of detail and chips. A detail line that does not fit keeps its IP address (the part before
/// it gives way first), and the whole row goes to the tooltip; the full text is returned for the page's detail line.
#[allow(clippy::too_many_arguments)]
fn row(g: &mut Gfx, tip: &mut Tooltip, idx: usize, x: f32, y: f32, w: f32, h: f32, s: f32, selected: bool, title: &str, sub: &str, chips: &[(&str, [u8; 4])]) -> Option<String> {
    let hot = g.hovered(R::new(x, y, w, h));
    crate::fx::row_at(g, x, y, w, h, 12.0 * s, selected, hot, ACCENT);
    g.hit(R::new(x, y, w, h), Event::Row(idx), title);
    if selected {
        g.rect(x, y + 10.0 * s, 4.0 * s, h - 20.0 * s, ACCENT);
    }
    // the title keeps to the room left of the chips (a narrow window cut it with an ellipsis instead of running past the card)
    let chips_w: f32 = chips.iter().map(|(l, _)| g.text_width(12.0 * s, l, Face::Bold) + 24.0 * s).sum();
    let shown_title = g.fit(17.0 * s, title, Face::Bold, (w - 36.0 * s - chips_w).max(80.0 * s));
    g.text(x + 18.0 * s, y + 8.0 * s, 17.0 * s, TEXT, &shown_title, Face::Bold);
    let shown = crate::tooltip::fit_keep_ip(g, 13.0 * s, sub, Face::Mono, w - 36.0 * s - 150.0 * s);
    g.text(x + 18.0 * s, y + 34.0 * s, 13.0 * s, MUTED, &shown, Face::Mono);
    let full = (shown != sub || shown_title != title).then(|| format!("{title}\n{sub}"));
    if let Some(f) = &full {
        tip.offer(g, R::new(x, y, w, h), selected, f);
    }
    let mut cx = x + w - 14.0 * s;
    for (label, col) in chips.iter().rev() {
        let cw = g.text_width(12.0 * s, label, Face::Bold) + 18.0 * s;
        cx -= cw;
        crate::fx::chip(g, cx, y + 10.0 * s, s, label, *col, false);
        cx -= 6.0 * s;
    }
    full
}

impl SettingsApp {
    const SIZE_CARD_NOTE: &'static str = "Applies to you at every sign-in. u makes it bigger, Shift+U smaller.";

    // ---- the theme card: Dark, Light, High contrast (the choice is the person's own, applied at once)

    const THEME_CARD_NOTE: &'static str = "Applies at once and at every sign-in. t picks the next, Shift+T the previous. Looks, wallpapers, icons and the dock are in Settings, Appearance.";

}

#[cfg(test)]
#[path = "settings_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "settings_lane_a_tests.rs"]
mod tests_a;
#[cfg(test)]
#[path = "settings_lane_b_tests.rs"]
mod tests_b;
#[cfg(test)]
#[path = "settings_lane_c_tests.rs"]
mod tests_c;
#[cfg(test)]
#[path = "settings_lane_d_tests.rs"]
mod tests_d;

#[cfg(test)]
#[path = "settings_homelayout_tests.rs"]
mod homelayout_tests;

#[cfg(test)]
#[path = "settings_responsive_tests.rs"]
mod responsive_tests;
#[cfg(test)]
#[path = "settings_crashlog_tests.rs"]
mod crashlog_tests;
#[cfg(test)]
#[path = "settings_router_tests.rs"]
mod router_tests;
#[cfg(test)]
#[path = "settings_fleet_tests.rs"]
mod fleet_tests;
#[cfg(test)]
#[path = "settings_backup_tests.rs"]
mod backup_tests;
#[cfg(test)]
#[path = "settings_backup_port_tests.rs"]
mod backup_port_tests;

#[cfg(test)]
#[path = "settings_access_tests.rs"]
mod access_tests;
#[cfg(test)]
#[path = "settings_enchome_tests.rs"]
mod enchome_tests;
#[cfg(test)]
#[path = "settings_enchome_port_tests.rs"]
mod enchome_port_tests;
#[cfg(test)]
#[path = "settings_updates_tests.rs"]
mod updates_tests;
#[cfg(test)]
#[path = "settings_updates_port_tests.rs"]
mod updates_port_tests;
#[cfg(test)]
#[path = "settings_agents_tests.rs"]
mod agents_tests;
#[cfg(test)]
#[path = "settings_memory_tests.rs"]
mod memory_tests;
#[cfg(test)]
#[path = "settings_time_tests.rs"]
mod time_tests;
#[cfg(test)]
#[path = "settings_install_tests.rs"]
mod install_tests;
#[cfg(test)]
#[path = "settings_mac_tests.rs"]
mod mac_tests;
#[cfg(test)]
#[path = "settings_gemini_tests.rs"]
mod gemini_tests;
#[cfg(test)]
#[path = "settings_announce_tests.rs"]
mod announce_tests;
#[cfg(test)]
#[path = "settings_link_tests.rs"]
mod link_tests;
#[cfg(test)]
#[cfg(test)]
#[path = "settings_lemonade_tests.rs"]
mod lemonade_tests;
#[cfg(test)]
#[path = "settings_sounds_tests.rs"]
mod sysnd_tests;
#[cfg(test)]
#[path = "settings_sftp_tests.rs"]
mod settings_sftp_tests;
#[cfg(test)]
#[path = "settings_machines_tests.rs"]
mod settings_machines_tests;
#[cfg(test)]
#[path = "settings_matrix_tests.rs"]
mod matrix_tests;
#[cfg(test)]
#[path = "settings_notifs_tests.rs"]
mod notifs_tests;
#[cfg(test)]
#[path = "settings_appaccess_tests.rs"]
mod appaccess_tests;
#[cfg(test)]
#[path = "settings_appevents_tests.rs"]
mod appevents_tests;
#[cfg(test)]
#[path = "settings_sound_tests.rs"]
mod sound_tests;
#[cfg(test)]
#[path = "settings_bt_tests.rs"]
mod bt_tests;
#[cfg(test)]
#[path = "settings_keyboard_tests.rs"]
mod keyboard_tests;
#[cfg(test)]
#[path = "settings_cloudroles_tests.rs"]
mod cloudroles_tests;
#[cfg(test)]
#[path = "settings_cloudroles_port_tests.rs"]
mod cloudroles_port_tests;
#[cfg(test)]
#[path = "settings_account_port_tests.rs"]
mod account_port_tests;
#[cfg(test)]
#[path = "settings_power_port_tests.rs"]
mod power_port_tests;
#[cfg(test)]
#[path = "settings_computer_port_tests.rs"]
mod computer_port_tests;
#[cfg(test)]
#[path = "settings_memory_port_tests.rs"]
mod memory_port_tests;
#[cfg(test)]
#[path = "settings_time_port_tests.rs"]
mod time_port_tests;
#[cfg(test)]
#[path = "settings_storage_port_tests.rs"]
mod storage_port_tests;
#[cfg(test)]
#[path = "settings_vault_port_tests.rs"]
mod vault_port_tests;
#[cfg(test)]
#[path = "settings_notifs_port_tests.rs"]
mod notifs_port_tests;
#[cfg(test)]
#[path = "settings_overview_tests.rs"]
mod overview_tests;
#[cfg(test)]
#[path = "settings_cloud_tests.rs"]
mod cloud_tests;
#[cfg(test)]
#[path = "settings_startup_tests.rs"]
mod startup_tests;
#[cfg(test)]
#[path = "settings_battery_tests.rs"]
mod battery_tests;
#[cfg(test)]
#[path = "settings_radio_tests.rs"]
mod radio_tests;
#[cfg(test)]
#[path = "settings_geminivoice_tests.rs"]
mod geminivoice_tests;
#[cfg(test)]
#[path = "settings_alexa_tests.rs"]
mod alexa_tests;
#[cfg(test)]
#[path = "settings_assistant_tests.rs"]
mod assistant_tests;
#[cfg(test)]
#[path = "settings_power_tests.rs"]
mod power_tests;
#[cfg(test)]
#[path = "settings_computer_tests.rs"]
mod computer_tests;
#[cfg(test)]
#[path = "settings_screensaver_tests.rs"]
mod screensaver_tests;
#[cfg(test)]
#[path = "settings_browsergraphics_tests.rs"]
mod browsergraphics_tests;
#[cfg(test)]
#[path = "settings_browserspeed_tests.rs"]
mod browserspeed_tests;
#[cfg(test)]
#[path = "settings_browserheaders_tests.rs"]
mod browserheaders_tests;
#[cfg(test)]
#[path = "settings_modelopts_tests.rs"]
mod modelopts_tests;
#[cfg(test)]
#[path = "settings_browserblock_tests.rs"]
mod browserblock_tests;
#[cfg(test)]
#[path = "settings_storageloss_tests.rs"]
mod storageloss_tests;
#[cfg(test)]
#[path = "settings_developer_tests.rs"]
mod developer_tests;
#[cfg(test)]
#[path = "settings_usbc_tests.rs"]
mod usbc_tests;
#[cfg(test)]
#[path = "settings_git_tests.rs"]
mod git_tests;
#[cfg(test)]
#[path = "settings_shares_tests.rs"]
mod shares_tests;
#[cfg(test)]
#[path = "settings_vaultstate_tests.rs"]
mod vaultstate_tests;
#[cfg(test)]
#[path = "settings_firewall_tests.rs"]
mod firewall_tests;
#[cfg(test)]
#[path = "settings_vpn_tests.rs"]
mod vpn_tests;
#[cfg(test)]
#[path = "settings_share_tests.rs"]
mod share_tests;
#[cfg(test)]
#[path = "settings_remote_tests.rs"]
mod remote_tests;
#[cfg(test)]
#[path = "settings_sshtunnels_tests.rs"]
mod sshtun_tests;

#[cfg(test)]
#[path = "settings_modelhost_tests.rs"]
mod modelhost_tests;
#[cfg(test)]
#[path = "settings_ia_tests.rs"]
mod ia_tests;
#[cfg(test)]
#[path = "settings_aihw_tests.rs"]
mod aihw_tests;
#[cfg(test)]
#[path = "settings_domains_tests.rs"]
mod dom_tests;

#[cfg(test)]
#[path = "settings_views_tests.rs"]
mod views_tests;
