//! Agent-facing building blocks behind the shell's apps: CHAT (LLM client), RAG (local
//! retrieval), MCP (Model Context Protocol client) and CLI (policy-checked command line).
//! Everything is headless and unit-tested; `iyakki-shell` only draws it.

pub use iyakki_proxy;

pub mod agent;
pub mod grants;
pub mod livemode;
pub mod assistant_guard;
pub mod assistrun;
#[cfg(test)]
mod assistrun_tests;
#[cfg(test)]
mod datatools_tests;
pub mod toolblock;
pub mod toolrepair;
pub mod screencap;

pub mod chat;
pub mod cli;
pub mod embed;
pub mod kb;
pub mod config;
pub mod layout;
pub mod cloudmode;
pub mod context;
pub mod camfw;
pub mod fwpack;
pub mod macfw;
pub mod macrecovery;
pub mod mcp;
pub mod providers;
pub mod vault;
pub mod rag;
pub mod rag_ext;
pub mod services;
pub mod sessions;
pub mod scope;
pub mod runs;
pub mod settings;
pub mod consul;
pub mod pkg;
pub mod store;
pub mod devmode;
pub mod sdkcli;
pub mod directlink;
pub mod wtool;
pub mod media;
pub mod mediaindex;
pub mod secretreq;
pub mod mediadl;
pub mod mediaqueue;
pub mod mediaserve;
pub mod credguard;
pub mod qr;
pub mod qr_decode;
pub mod sharedfolder;
pub mod sharedsite;
#[cfg(test)]
mod sharedfolder_tests;
pub mod aacpipe;
pub mod audiofmt;
pub mod hls;
pub mod radio;
pub mod radio_stations;
pub mod radiotools;
pub mod camtools;
pub mod webtools;
pub mod datatools;
pub mod dash;
pub mod flows;
pub mod hostfiles;
pub mod browserconsent;
pub mod browserjobs;
pub mod browsertools;
pub mod browserqa;
pub mod qaimage;
pub mod idebrowser;
pub mod browserrun;
pub mod webpage;
pub mod webresearch;
pub mod skills;
pub mod voice;
pub mod audioout;
pub mod audiosink;
pub mod battery;
pub mod screensaver;
pub mod mixer;
pub mod softvol;
pub mod quiet;
pub mod sounddoctor;
pub mod soundprobe;
pub mod keyboard;
pub mod hostvoice;
pub mod wsproto;
pub mod httpmini;
pub mod hostengines;
pub mod indic;
pub mod voicehelper;
pub mod story;
pub mod piclog;
pub mod lab;
pub mod storyillus;
pub mod wordtime;
pub mod speechtime;
pub mod syssounds;
pub mod voicedesk;
pub mod voicepack;
pub mod scene;
pub mod webengine;
pub mod avaddon;
#[cfg(test)]
mod avaddon_tests;
pub mod exec;
pub mod ssh;
pub mod sshkey;
pub mod gitagent;
pub mod gitsite;
pub mod gitkeys;
pub mod gitbrowse;
pub mod gitconn;
pub mod gitjobs;
pub mod clock;
pub mod clockfloor;
pub mod ntp;
pub mod tzdetect;
pub mod regex;
pub mod textcmd;
pub mod toollog;
pub mod usage;
pub mod models;
pub mod calendar;
pub mod archive;
pub mod xz;
pub mod player;
pub mod clis;
pub mod cluster;
pub mod clusters;
pub mod quickinstall;
pub mod ollama;
pub mod lemonade;
pub mod omni;
pub mod modelhost;
// GPUs for local models in k3s (F-GPU-01, F-GPU-03; docs/GPU-K3S.md)
pub mod gpu;
pub mod aiprofile;
pub mod aiprofile_remote;
pub mod gpu_nvidia;
pub mod gpu_steps;
pub mod gpu_workloads;
pub mod gpucdi;
pub mod devwatch;
pub mod release;
pub mod relinfo;
pub mod sysinfo;
pub mod inbox;
pub mod featureboard;
pub mod boarddeps;
pub mod notes;
pub mod notetools;
pub mod contextintent;
pub mod assign;
pub mod instance;
pub mod workspace;
pub mod wsexec;
// Linux apps: a Debian/Ubuntu sandbox and image apps (docs/LINUX-APPS.md)
pub mod rootfs;
pub mod linuxapps;
pub mod linuxapps_dist;
pub mod linuxapps_init;
pub mod linuxwin;
#[cfg(test)]
mod linuxwin_tests;
pub mod imageapps;
#[cfg(test)]
mod linuxapps_tests;
pub mod extagent;
pub mod k8s;
pub mod disks;
pub mod disks_part;
pub mod disks_sys;
pub mod disks_fake;
#[cfg(test)]
mod disks_tests;
pub mod k8sexp;
pub mod devcluster;
pub mod deploylint;
pub mod dockerlint;
pub mod k8stools;
pub mod k8sretry;
pub mod k3sdev;
pub mod kubeprofiles;
pub mod localreg;
pub mod ocipush;
pub mod regsetup;
pub mod platform;
pub mod boardtools;
pub mod autopilot;
pub mod boardtrack;
pub mod tasks;
pub mod tasktools;
pub mod jobs;
pub mod jobtools;
#[cfg(test)]
mod jobtools_tests;
#[cfg(test)]
mod tasks_tests;
pub mod bugs;
pub mod history;
pub mod projectstate;
pub mod target;
pub mod testresults;
pub mod coverage;

use iyakki_sec::policy::Policy;

/// What the apps need from the running system. Implemented by `iyakki-core` (and by mocks in tests)
/// so this crate does not depend on the supervisor.
pub trait HostApi: Send + Sync {
    fn detect_env(&self) -> String;
    fn net_status(&self) -> String;
    /// Run a WASM module for `user` with that user's limits and capability tokens.
    fn run_wasm(&self, user: &str, policy: &Policy, wasm: &[u8], entry: &str) -> Result<String, String>;
    fn power_off(&self);
    fn reboot(&self);
    /// Whether `power_off` and `reboot` can really act here (false in windowed development mode and tests that say so); the
    /// login screen's power card tells the person instead of doing nothing.
    fn can_power(&self) -> bool {
        true
    }
}
