//! Agent-facing building blocks behind the shell's apps: CHAT (LLM client), RAG (local
//! retrieval), MCP (Model Context Protocol client) and CLI (policy-checked command line).
//! Everything is headless and unit-tested; `iyakki-shell` only draws it.

pub use iyakki_proxy;

pub mod agent;
pub mod grants;
pub use iyakki_shared::livemode;
pub use iyakki_shared::assistant_guard;
pub mod assistrun;
pub mod embedassist;
pub mod avimjpeg;
pub mod embedmedia;
#[cfg(test)]
mod browserext_tests;
#[cfg(test)]
mod gemini_tests;
#[cfg(test)]
mod geminilive_tests;
#[cfg(test)]
mod assistrun_tests;
#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod datatools_tests;
#[doc(hidden)]
pub mod testdir;
pub use iyakki_shared::toolblock;
pub mod tokenbudget;
pub mod toolrepair;
pub use iyakki_shared::screencap;

pub use iyakki_shared::chat;
pub mod cli;
pub use iyakki_memx::embed;
pub use iyakki_memx::kb;
pub mod config;
#[cfg(test)]
mod upgrade_tests;
#[cfg(test)]
mod compat_tests;
#[cfg(test)]
mod compat_tests_schema;
#[cfg(test)]
mod compat_formats_tests;
#[cfg(test)]
mod svg_chrome_tests;
#[cfg(test)]
mod pdf_quartz_tests;
#[cfg(test)]
mod image_sips_tests;
#[cfg(test)]
mod childgone_tests;
pub use iyakki_shared::layout;
pub use iyakki_shared::views;
pub use iyakki_shared::cloudmode;
pub mod webservice;
pub mod context;
pub mod camfw;
pub mod fwextra;
pub use iyakki_shared::gemini;
pub use iyakki_shared::geminilive;
pub mod geminilive_audio;
pub mod geminilive_host;
pub mod geminilive_tools;
pub mod geminilive_wake;
pub mod geminilive_ws;
pub mod settingstools_geminivoice;
pub mod fwextra_cli;
pub mod fwextra_net;
pub mod sharing;
pub mod fwpack;
pub mod macfw;
pub use iyakki_macrecovery as macrecovery;
pub mod mcp;
pub mod utcp;
#[cfg(test)]
mod utcp_tests;
pub use iyakki_shared::providererr;
pub mod providers;
pub use iyakki_shared::vault;
pub mod profilestore;
pub mod profilepolicy;
pub mod ddcci;
pub mod livekeys;
pub mod ddcci_bus;
pub mod camvision;
pub mod audiostream;
pub mod echocancel;
pub use iyakki_shared::rag;
pub mod agenticrag;
pub mod ragtools;
pub mod ragrerank;
pub mod ragraph;
#[cfg(test)]
mod agenticrag_tests;
#[cfg(test)]
mod ragtools_tests;
#[cfg(test)]
mod rageval_tests;
pub use iyakki_shared::rag_ext;
#[cfg(test)]
mod rag_ext_tests;
pub mod services;
pub mod sessions;
pub mod scope;
pub mod terminalfacet;
pub mod runs;
pub use iyakki_shared::settings;
pub use iyakki_shared::consul;
pub mod pkg;
pub mod store;
pub mod devmode;
pub mod contract_local;
pub mod sdkcli;
pub mod directlink;
pub mod wtool;
pub mod media;
pub use iyakki_shared::mediaindex;
pub use iyakki_shared::secretreq;
pub mod mediadl;
pub mod mediaqueue;
pub use iyakki_shared::mediaserve;
pub use iyakki_shared::credguard;
pub use iyakki_shared::enginelog;
#[cfg(test)]
mod enginelog_tests;
pub use iyakki_shared::doctor;
pub mod doctor_sys;
pub mod doctor_gather;
pub mod doctortools;
pub use iyakki_shared::twophase;
pub mod wakestats;
pub mod audiotls;
pub mod cloudstream;
pub mod cloudstreamtools;
pub mod wizardrun;
pub mod wizardtools;
pub mod datamigrate;
pub mod bundlesnap;
pub mod ocipush_root;
pub mod imagebuild;
pub mod imagebuildtools;
pub mod linuxappimagetools;
pub mod k8sexptools;
#[cfg(test)]
mod tools_forward_tests;
pub mod linuxsandboxtools;
pub mod linuxpod;
pub mod linuxpodtools;
pub mod mcpadmintools;
pub mod skillstools;
#[cfg(test)]
pub(crate) mod linuxpod_tests;
pub mod sysupdate;
pub mod cloudmembers;
pub mod backup_cli;
pub use iyakki_shared::backup_schedule;
pub mod settingstools_backup;
pub mod localos;
pub mod checkup;
pub use iyakki_shared::checkup_result;
pub mod pathcheck;
pub use iyakki_shared::diagshare;
pub use iyakki_qr::qr;
pub use iyakki_qr::qr_decode;
pub mod sharedfolder;
pub mod sharedsite;
pub mod upnp;
pub mod upnp_events;
pub mod upnp_search;
pub mod upnp_serve;
#[cfg(test)]
mod sharedfolder_tests;
pub mod aacpipe;
pub mod audiofmt;
pub mod hls;
pub mod radio;
pub mod radio_stations;
pub mod radiotools;
#[cfg(test)]
mod radio_real_http_tests;
#[cfg(test)]
mod media_apple_decoder_tests;
pub use iyakki_shared::camtools;
pub mod visionmodel;
pub mod webtools;
pub mod datatools;
pub mod dash;
pub mod flows;
pub mod hostfiles;
pub mod hostservicetools;
pub use iyakki_shared::browserconsent;
pub use iyakki_shared::browserext;
pub mod browserheaders;
pub use iyakki_shared::browserjobs;
pub mod browsertools;
pub mod devtools;
pub mod devtools_headers;
#[cfg(test)]
mod devtools_headers_tests;
#[cfg(test)]
mod devtools_tests;
pub mod browserqa;
pub use iyakki_shared::qaimage;
pub mod idebrowser;
pub mod browserrun;
pub mod webpage;
pub mod webresearch;
pub mod skills;
pub mod voice;
pub use iyakki_shared::backup;
pub mod audioout;
pub mod audiosink;
pub use iyakki_battery as battery;
pub mod power;
pub use iyakki_shared::screensaver;
pub use iyakki_shared::saverslides;
pub use iyakki_shared::lockscreen;
pub mod lockscreentools;
pub mod settingstools;
pub mod settingstools_views;
pub mod settingstools_notifs;
pub mod settingstools_saver;
pub mod settingstools_gemini;
pub mod settingstools_startup;
pub mod settingstools_wasmtiles;
pub mod settingstools_sound;
pub mod settingstools_memory;
pub mod settingstools_linux;
pub mod settingstools_access;
pub mod settingstools_machine;
pub mod settingstools_helpers;
pub mod settingstools_podtls;
pub mod settingstools_install;
pub mod settingstools_net;
pub mod newapp;
pub mod settingstools_usblink;
pub mod settingstools_wifi;
pub use iyakki_shared::wifilink;
#[cfg(test)]
mod settingstools_tests;
// Encrypted per-person home folders (F-STORE-05): the lifecycle crate re-exported, and the chat tools.
pub use iyakki_enchome as enchome;
pub mod enchometools;
#[cfg(test)]
mod enchometools_tests;
#[cfg(test)]
mod lockscreentools_tests;
pub use iyakki_shared::accessibility;
pub mod accessibilitytools;
pub use iyakki_shared::browsera11y;
#[cfg(test)]
mod browsera11y_tests;
#[cfg(test)]
mod toolblock_prompt_tests;
#[cfg(test)]
mod accessibilitytools_tests;
pub mod micdefaults;
pub use iyakki_shared::micpcm;
pub mod appactions_real;
pub mod appevent_sources;
pub mod corehandle;
pub mod agentapi_gen;
pub mod pendingfacet;
mod tasks_facet;
pub mod flowsfacet;
pub mod agentfacets;
pub mod facets_b0;
#[cfg(test)]
mod facets_b0_tests;
pub mod facets_b3;
pub mod facets_s6b;
pub mod facets_b4;
pub mod facets_b6;
pub mod facets_b7;
pub mod facets_b8;
pub mod facets_g;
pub mod facets_h;
pub mod flowspanel;
pub mod accessbroker_revoke;
#[cfg(test)]
mod accessbroker_tests;
pub mod facets_b5;
pub mod store_api;
#[cfg(test)]
mod store_api_tests;
pub mod facets_b1;
pub mod facets_b2;
#[cfg(test)]
mod facets_b2_tests;
#[cfg(test)]
mod facets_h_tests;
#[cfg(test)]
mod agentfacets_tests;
pub use agentapi_gen::AgentApiImpl;
pub mod storefacets;
pub use storefacets::StoreFacetsImpl;
pub mod embedbrowser;
pub mod mixer;
pub mod softvol;
pub mod quiet;
pub mod soundhold;
pub mod webmix;
pub mod sounddoctor;
pub mod soundprobe;
pub mod inputprofile;
pub mod keyboard;
pub mod hostvoice;
pub use iyakki_shared::browsermedia;
#[cfg(test)]
#[path = "browsermedia_tests.rs"]
mod browsermedia_tests;
pub use iyakki_shared::browsersource;
pub mod browsersource_svc;
pub use iyakki_link::wsproto;
pub mod httpmini;
pub mod hostengines;
pub mod voicehelper;
pub use iyakki_shared::story;
pub mod storypdf;
pub mod studypdf;
pub use iyakki_shared::piclog;
pub use iyakki_shared::lab;
pub use iyakki_shared::storyillus;
pub use iyakki_wordtime as wordtime;
pub mod speechtime;
pub mod syssounds;
pub mod browserdev;
pub mod voicedesk;
pub mod voiceassist;
pub mod voiceassist_ports;
pub mod voiceassisttools;
pub mod passkeys;
pub mod webpush;
pub mod voicebackend;
pub mod avs;
pub mod browserpkg;
pub mod wakelisten;
#[cfg(test)]
#[path = "voiceassist_tests.rs"]
mod voiceassist_tests;
#[cfg(test)]
#[path = "voiceassist_cloud_tests.rs"]
mod voiceassist_cloud_tests;
pub mod recordings;
pub mod voicepack;
pub mod scene;
pub mod webengine;
pub mod webgraphics;
pub mod modelopts;
pub use iyakki_shared::modelavail;
#[cfg(test)]
mod modelopts_tests;
#[cfg(test)]
mod providers_loopback_tests;
#[cfg(test)]
mod assistant_wire_tests;
#[cfg(test)]
mod embedassist_wire_tests;
#[cfg(test)]
mod embedmedia_tests;
pub mod modelopttools;
#[cfg(test)]
mod modelopttools_tests;
pub mod webgraphicstools;
pub mod browsershort;
#[cfg(test)]
mod browsershort_tests;
#[cfg(test)]
mod webgraphics_tests;
#[cfg(test)]
mod webgraphicstools_tests;
pub mod avaddon;
pub mod avaddon_install;
pub mod avbuild;
#[cfg(test)]
mod avbuild_tests;
#[cfg(test)]
mod avaddon_tests;
#[cfg(test)]
mod avaddon_install_tests;
pub mod exec;
pub mod ssh;
pub use iyakki_sshfs::ssh_server;
pub mod sftp;
pub use iyakki_sshfs::sftp_fs;
pub use iyakki_sshfs::sftp_proto;
pub use iyakki_sshfs::sftp_session;
pub mod sftptools;
pub mod remotefiles;
pub mod remotefilestools;
pub use iyakki_shared::accessbroker;
pub use iyakki_shared::clientlink;
pub mod clientmedia;
pub mod cameras;
pub mod embedcameras;
pub mod camdetect;
pub mod camerastools;
pub mod camrec;
pub mod camsource;
pub use iyakki_camdetect::camrec_format;
pub use iyakki_camdetect::camrec_seal;
pub use iyakki_sshfs::rf_ftp;
pub use iyakki_sshfs::rf_sftp;
pub use iyakki_sshfs::rf_ssh;
#[cfg(test)]
mod sftp_tests;
pub use iyakki_sshfs::sshalg;
pub use iyakki_sshfs::sshkey;
pub use iyakki_shared::sshtunnel;
pub mod sshtunnel_mgr;
pub mod gitworktools;
pub mod sshtunneltools;
pub mod sshsessiontools;
pub mod objectstoragetools;
pub mod updatestools;
pub mod sessionlogtools;
#[cfg(test)]
mod sessionlogtools_tests;
pub mod componentstools;
pub mod buildtools;
#[cfg(test)]
mod buildtools_tests;
pub mod pdfnativetools;
pub use iyakki_shared::appopen;
pub mod pdftools;
pub mod tilestools;
#[cfg(test)]
mod pdftools_tests;
#[cfg(test)]
mod pdfnativetools_tests;
#[cfg(test)]
mod storypdf_tests;
#[cfg(test)]
mod studypdf_tests;
#[cfg(test)]
mod componentstools_tests;
pub mod sharetools;
pub mod sqlmini;
#[cfg(test)]
mod sqlmini_tests;
#[cfg(test)]
mod sharetools_tests;
#[cfg(test)]
mod updatestools_tests;
pub mod ollamatools;
#[cfg(test)]
mod ollamatools_tests;
pub mod lemonadecomputetools;
pub use iyakki_shared::hwprofile;
pub mod hwprofile_svc;
#[cfg(test)]
mod hwprofile_tests;
pub mod thiscomputertools;
pub mod dictfix;
pub mod dictfixtools;
pub mod revise;
pub mod quickuninstalltools;
#[cfg(test)]
mod quickuninstalltools_tests;
#[cfg(test)]
mod objectstoragetools_tests;
pub mod vpn_dns;
pub mod vpn_helper;
pub mod vpn_mgr;
pub mod vpn_nft;
pub mod vpn_sys;
pub mod vpntools;
pub use iyakki_shared::bluetoothpair;
pub mod bluetoothtools;
pub mod btaudiotools;
pub mod btroute;
#[cfg(test)]
mod vpn_mgr_tests;
#[cfg(test)]
mod vpntools_tests;
#[cfg(test)]
mod bluetoothtools_tests;

#[cfg(test)]
mod gitworktools_tests;
#[cfg(test)]
mod sshtunneltools_tests;
#[cfg(test)]
mod sshsessiontools_tests;
#[cfg(test)]
mod sharedvault_tests;
#[cfg(test)]
mod sharedvaulttools_tests;
#[cfg(test)]
mod sshtunnel_tests;
#[cfg(test)]
mod sshtunnel_real_tests;
#[cfg(test)]
mod ssh_real_tests;
pub mod gitagent;
pub use iyakki_shared::gitsite;
pub mod gitkeys;
pub use iyakki_shared::gitbrowse;
#[cfg(test)]
#[path = "gitbrowse_tests.rs"]
mod gitbrowse_tests;
#[cfg(test)]
#[path = "gitreal_tests.rs"]
mod gitreal_tests;
#[cfg(test)]
#[path = "githost_loopback_tests.rs"]
mod githost_loopback_tests;
pub mod gitconn;
pub use iyakki_shared::gitdiscover;
pub mod gitnet;
pub mod netprobe;
pub mod nl80211_os;
pub mod hotspot_radio;
pub mod meteredlink;
pub mod quietio;
pub mod wifi_ap;
pub mod wifi_ap_host;
pub use iyakki_devctl::{bnep, bt_pan};
pub mod msg;
pub mod gitjobs;
pub use iyakki_clock as clock;
// the pure floor logic is in iyakki-clockfloor; the two boot steps here hand it the real clock setter (`ntp::set_clock`)
pub mod clockfloor {
    pub use iyakki_clockfloor::*;
    /// Boot, first step: see `iyakki_clockfloor::apply_early`.
    pub fn apply_early() -> Applied {
        iyakki_clockfloor::apply_early(&mut |t| crate::ntp::set_clock(t as f64))
    }
    /// Boot, second step: see `iyakki_clockfloor::apply_after_data`.
    pub fn apply_after_data(on_disk: bool) -> Applied {
        iyakki_clockfloor::apply_after_data(on_disk, &mut |t| crate::ntp::set_clock(t as f64))
    }
}
pub mod ntp;
pub use iyakki_shared::totp;
pub mod authcodes;
pub mod tzdetect;
pub use iyakki_regex as regex;
pub mod textcmd;
pub mod toollog;
pub mod usage;
pub mod router_registry;
pub mod router;
pub mod router_js;
pub mod routertools;
pub mod router_pool;
pub mod fleet;
pub mod fleet_context;
pub mod fleet_hub;
pub mod fleet_integrate;
pub mod fleet_work;
pub mod fleettools;
#[cfg(test)]
mod fleet_tests;
#[cfg(test)]
mod fleettools_tests;
#[cfg(test)]
mod router_tests;
pub use iyakki_shared::models;
pub use iyakki_shared::calendar;
pub use iyakki_shared::study;
pub mod study_tools;
pub use iyakki_shared::studylesson;
// `archive::tests` (fixtures) comes through the glob when the dev-dependency turns on the feature `testing`
pub mod archive {
    pub use iyakki_archive::archive::*;
}
pub use iyakki_bindelta::bindelta;
pub use iyakki_archive::xz;
pub mod player;
pub mod clis;
#[cfg(test)]
mod clis_limit_tests;
pub mod dlmanifest;
pub use iyakki_openpgp as openpgp;
pub mod cluster;
pub mod clusters;
pub mod compatreport;
pub mod quickinstall;
pub mod pvcguard;
pub mod vaultpending;
pub mod ollama;
pub use iyakki_shared::lemonade;
pub mod omni;
pub use iyakki_shared::modelhost;
pub mod modelhost_svc;
#[cfg(test)]
mod modelhost_tests;
// GPUs for local models in k3s (F-GPU-01, F-GPU-03; docs/GPU-K3S.md)
pub mod gpu;
pub mod aiprofile;
pub use iyakki_shared::aiprofile_remote;
pub mod gpu_nvidia;
pub mod gpu_steps;
pub mod gpu_workloads;
pub mod gpu_rocm;
pub mod gpucdi;
pub use iyakki_shared::devname;
pub use iyakki_shared::webaccess;
pub use iyakki_shared::devwatch;
pub use iyakki_shared::release;
pub use iyakki_shared::relinfo;
pub use iyakki_core_local::sysinfo;
pub mod procowner;
pub mod sysprocs;
pub use iyakki_shared::inbox;
pub mod notifyhub;
pub use iyakki_shared::featureboard;
pub use iyakki_shared::boarddeps;
pub use iyakki_shared::notes;
pub mod notetools;
pub mod contextintent;
pub mod assign;
pub mod instance;
pub use iyakki_shared::workspace;
pub use iyakki_shared::wsexec;
// Linux apps: a Debian/Ubuntu sandbox and image apps (docs/LINUX-APPS.md)
pub use iyakki_shared::rootfs;
pub use iyakki_shared::linuxapps;
pub use iyakki_shared::linuxapps_dist;
pub use iyakki_shared::linuxapps_init;
pub use iyakki_shared::linuxapps_net;
pub mod linuxapps_net_selftest;
pub use iyakki_disksx::oscompat;
#[cfg(test)]
mod macos_fallback_tests;
pub use iyakki_shared::linuxwin;
#[cfg(test)]
mod linuxwin_tests;
pub use iyakki_shared::imageapps;
#[cfg(test)]
mod linuxapps_tests;
pub mod extagent;
pub use iyakki_shared::k8s;
pub use iyakki_disksx::disks;
pub use iyakki_disksx::disks_part;
pub use iyakki_disksx::disks_fmt;
pub use iyakki_disksx::disks_sys;
pub use iyakki_disksx::disks_fake;
pub use iyakki_diskcrypt as disks_crypt;
pub use iyakki_disksx::disks_helper;
pub use iyakki_disksx::disks_unlock;
pub mod disksunlocktools;
pub mod disksformattools;
#[cfg(test)]
mod disksunlocktools_tests;
#[cfg(test)]
mod disksformattools_tests;
pub use iyakki_shared::k8sexp;
pub use iyakki_shared::k8scr;
pub mod k8sforward;
pub use iyakki_shared::devcluster;
pub use iyakki_shared::clusterprobe;
pub use iyakki_lint::deploylint;
pub use iyakki_lint::dockerlint;
pub mod k8stools;
pub use iyakki_shared::k8sretry;
#[cfg(test)]
mod k8sretry_tests;
pub use iyakki_shared::exposure;
pub mod k3sdev;
pub use iyakki_shared::startup;
pub mod startup_probe;
pub mod installstartup;
pub mod touchfix;
pub mod touchprobe;
pub mod touchrecover;
pub mod usbawake;
pub mod usbports;
pub mod startupdisk;
#[cfg(test)]
mod startup_tests;
pub use iyakki_shared::kubeprofiles;
pub mod kubeprofiles_svc;
#[cfg(test)]
mod k8sexp_ledger_tests;
#[cfg(test)]
mod aiprofile_remote_tests;
#[cfg(test)]
mod kubeprofiles_tests;
pub mod localreg;
pub mod ocipush;
pub mod cloudaccess;
pub mod cloudcontrol;
pub mod cloudfiles;
pub mod cloudflows;
pub mod cloudsnapshot;
pub mod cloudsnapshot_kms;
pub mod cloudlaunch;
pub mod cloudlisten_tunnel;
pub mod cloudlisten;
pub mod cloudlisten_h2;
pub mod domains;
pub mod domainstools;
pub use iyakki_shared::cloudroles;
pub mod cloudroute_cert;
#[cfg(test)]
mod cloudroute_tests;
pub mod cloudroute {
    pub use crate::cloudroute_cert::{check_certificate, load_certified, self_signed};
    pub use iyakki_shared::cloudroute::*;
}
pub mod cloudstorage;
pub mod cloudquota;
pub mod cloudtenant;
pub mod cloudvault;
pub mod cloudkeys;
pub mod cloudvaultd;
pub mod cloudvaultguard;
pub mod cloudrotate;
pub mod cloudvaultlink;
pub use iyakki_shared::sharedvault;
pub mod sharedvaulttools;
pub mod cloudstoragetools;
pub mod toolrouting;
#[cfg(test)]
mod toolrouting_tests;
pub mod tenantconfig;
pub mod sandbox;
pub mod commonvault;
pub use iyakki_link::cloudclient;
pub use iyakki_link::pixelcodec;
pub use iyakki_shared::cloudtenants;
pub mod cloudtenantdir;
pub mod cloudcli;
pub mod cloudadopt;
pub mod cloudseed;
pub mod healthcheck;
pub mod regsetup;
pub mod platform;
pub mod boardtools;
pub mod autopilot;
pub use iyakki_shared::boardtrack;
pub mod tasks;
pub mod tasktools;
pub mod taskenv;
#[cfg(test)]
mod taskenv_tests;
pub mod jobs;
pub mod jobtools;
pub mod pending;
pub use iyakki_shared::phrases;
#[cfg(test)]
mod phrases_tests;
pub mod pendingtools;
pub mod pendingplus;
#[cfg(test)]
mod pendingplus_tests;
#[cfg(test)]
mod pending_tests;
#[cfg(test)]
mod jobtools_tests;
#[cfg(test)]
mod tasks_tests;
pub use iyakki_shared::bugs;
pub use iyakki_memx::history;
pub mod projectstate;
pub mod target;
pub use iyakki_shared::testresults;
pub mod edittest;
#[cfg(test)]
mod edittest_tests;
pub use iyakki_shared::coverage;

/// What the apps need from the running system. The trait now lives in `iyakki-core-api` (UI/OS split plan, step 2); re-exported so
/// every `iyakki_agent::HostApi` path keeps working.
pub use iyakki_core_api::{HostApi, WasmIo, WasmTask};
/// The core contract's types, for the tiles and the transports that call `contract_local` (no new dependency for them).
pub use iyakki_core_api::contract as core_contract;
#[cfg(test)]
mod fuzzkit;
#[cfg(test)]
mod upnp_fuzz_tests;
#[cfg(test)]
mod checkup_fuzz_tests;
#[cfg(test)]
mod sdkmoney_fuzz_tests;
pub mod mediasync;
#[cfg(test)]
mod mediaindex_tests;
pub mod studyhost;
#[cfg(test)]
mod study_tests;
#[cfg(test)]
mod studylesson_tests;
#[cfg(test)]
mod lemonade_tests;
pub mod storykit;
#[cfg(test)]
mod story_tests;
pub mod sharedvault_builtin;
pub mod fleetkit;
pub mod labkit;
#[cfg(test)]
mod lab_tests;
pub mod camkit;
#[cfg(test)]
mod camtools_tests;
pub mod storekit;
