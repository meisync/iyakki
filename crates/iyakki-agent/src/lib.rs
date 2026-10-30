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
#[doc(hidden)]
pub mod testdir; // per-test temporary folders; used by the tests of this crate and of iyakki-shell
pub mod toolblock;
pub mod tokenbudget;
pub mod toolrepair;
pub mod screencap;

pub mod chat;
pub mod cli;
pub mod embed;
pub mod kb;
pub mod config;
#[cfg(test)]
mod upgrade_tests;
#[cfg(test)]
mod childgone_tests;
pub mod layout;
pub mod cloudmode;
pub mod webservice;
pub mod context;
pub mod camfw;
pub mod fwextra;
pub mod fwextra_cli;
pub mod fwextra_net;
pub mod fwpack;
pub mod macfw;
pub mod macrecovery;
pub mod mcp;
pub mod providers;
pub mod vault;
pub mod profilestore;
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
pub mod contract_local;
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
pub mod enginelog;
pub mod doctor;
pub mod checkup;
pub mod pathcheck;
pub mod diagshare;
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
pub mod visionmodel;
pub mod webtools;
pub mod datatools;
pub mod dash;
pub mod flows;
pub mod hostfiles;
pub mod browserconsent;
pub mod browserheaders;
pub mod browserjobs;
pub mod browsertools;
pub mod devtools;
pub mod devtools_headers;
#[cfg(test)]
mod devtools_headers_tests;
#[cfg(test)]
mod devtools_tests;
pub mod browserqa;
pub mod qaimage;
pub mod idebrowser;
pub mod browserrun;
pub mod webpage;
pub mod webresearch;
pub mod skills;
pub mod voice;
pub mod backup;
pub mod audioout;
pub mod audiosink;
pub mod battery;
pub mod screensaver;
pub mod mixer;
pub mod softvol;
pub mod quiet;
pub mod soundhold;
pub mod webmix;
pub mod sounddoctor;
pub mod soundprobe;
pub mod keyboard;
pub mod hostvoice;
pub mod wsproto;
pub mod httpmini;
pub mod hostengines;
pub mod voicehelper;
pub mod story;
pub mod piclog;
pub mod lab;
pub mod storyillus;
pub mod wordtime;
pub mod speechtime;
pub mod syssounds;
pub mod browserdev;
pub mod voicedesk;
pub mod recordings;
pub mod voicepack;
pub mod scene;
pub mod webengine;
pub mod avaddon;
#[cfg(test)]
mod avaddon_tests;
pub mod exec;
pub mod ssh;
pub mod sshkey;
pub mod sshtunnel;
pub mod sshtunnel_mgr;
pub mod sshtunneltools;
pub mod vpn_dns;
pub mod vpn_helper;
pub mod vpn_mgr;
pub mod vpn_nft;
pub mod vpn_sys;
pub mod vpntools;
#[cfg(test)]
mod vpn_mgr_tests;
#[cfg(test)]
mod vpntools_tests;
#[cfg(test)]
mod sshtunneltools_tests;
#[cfg(test)]
mod sshtunnel_tests;
pub mod gitagent;
pub mod gitsite;
pub mod gitkeys;
pub mod gitbrowse;
pub mod gitconn;
pub mod gitdiscover;
pub mod gitnet;
pub mod netprobe;
pub mod msg;
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
pub mod study;
pub mod study_tools;
pub mod studylesson;
pub mod archive;
pub mod xz;
pub mod player;
pub mod clis;
pub mod dlmanifest;
pub mod openpgp;
pub mod cluster;
pub mod clusters;
pub mod quickinstall;
pub mod pvcguard;
pub mod vaultpending;
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
pub mod devname;
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
pub mod exposure;
pub mod k3sdev;
pub mod startup;
pub mod startup_probe;
#[cfg(test)]
mod startup_tests;
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

/// What the apps need from the running system. The trait now lives in `iyakki-core-api` (UI/OS split plan, step 2); re-exported so
/// every `iyakki_agent::HostApi` path keeps working.
pub use iyakki_core_api::HostApi;
/// The core contract's types, for the tiles and the transports that call `contract_local` (no new dependency for them).
pub use iyakki_core_api::contract as core_contract;
