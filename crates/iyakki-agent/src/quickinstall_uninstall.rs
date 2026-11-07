//! Uninstall for EVERY Quick install item: a removal plan per item, a runner that checks every target before it touches it, the
//! dependency rules, and the real hooks of this machine.
//!
//! * **A plan lists targets, nothing is found by pattern.** [`plan`] gives, for one item, named steps: one Kubernetes object by kind and
//!   name, one folder under the item's own root, one vault entry, one DASH source, or a named hook (the CLI catalogue, a driver pack, a
//!   sandbox...). Nothing is deleted by wildcard, label or `--all`.
//! * **Safe by default.** A step is either plain (the program, the Deployment, the Service, the marker) or `data` (volume claims, Secrets,
//!   the person's vault entries, DASH sources, downloaded caches, the cluster's database). Data steps run only when the person ticked
//!   "also delete the data" (`delete_data`, off by default); the card says what is freed and that it cannot be undone.
//! * **Every target is checked** ([`check_step`]) against a fixed list of IYAKKI's own namespaces, objects and folders before it is
//!   touched; a target outside them is refused and nothing after it runs. The plan builder is not trusted by the runner.
//! * **Dependants.** Removing an item that other installed items need (k3s, Lemonade) is refused with their names, or removes them first
//!   when the person chose that ([`Opts::with_dependants`]); an item that other tools merely use (Node.js, Python) is warned about.
//! * **Safe to repeat.** Deletes use `--ignore-not-found`, a missing folder is fine, a hook that finds nothing says so. A removal that
//!   stops leaves the marker, so the item still shows and Retry runs the same plan again.
//! * **Audit lines** name the action and the item and whether data was deleted: never a path with a secret, never a secret.

use super::*;

use std::sync::{Arc, Mutex};

/// A fixed set of namespaces IYAKKI creates itself. A Kubernetes target outside them is refused.
const OWN_NAMESPACES: [&str; 6] = ["iyakki-data", "iyakki-monitoring", "iyakki-ai", "iyakki-matrix", "iyakki-gitea", "iyakki-objects"];
/// The registry's namespace (its own constant in `regsetup`).
const REGISTRY_NS: &str = crate::regsetup::IN_NS;
/// Namespaces only one item uses: removed with the data of that item, never otherwise.
const EXCLUSIVE_NAMESPACES: [&str; 4] = ["iyakki-monitoring", "iyakki-matrix", "iyakki-gitea", REGISTRY_NS];
/// The kinds a plan may delete, by name.
const KINDS: [&str; 10] = ["deployment", "service", "configmap", "secret", "persistentvolumeclaim", "serviceaccount", "daemonset", "clusterrole", "clusterrolebinding", "namespace"];

/// What one step does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    /// Delete one object: `ns` is `None` for a cluster-scoped kind.
    Kube { ns: Option<&'static str>, kind: &'static str, name: String },
    /// Delete one volume claim, then the retained volume it was bound to and that volume's folder (read before the claim goes).
    Claim { ns: &'static str, name: String },
    /// Delete one folder or file; it must lie under `root`, which must lie under the data folder.
    Path { path: PathBuf, root: PathBuf },
    /// Delete one vault entry of the person.
    Vault(String),
    /// Remove one DASH source by id (the person's own and the shared one).
    Source(String),
    Hook(Hook),
}

/// The named things that are not a Kubernetes object, a folder or a vault entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hook {
    /// A program of the CLI catalogue (`clis.rs`).
    Cli(&'static str),
    /// Stop the cluster, take the program away, keep its `data` folder.
    K3sKeepData,
    /// Stop the cluster and delete the whole `k3s-dev` folder.
    K3sAll,
    /// Stop the Ollama IYAKKI started and take `ollama-local` out of Models.
    OllamaServer,
    /// Take the `lemonade-local` entries of this machine out of Models.
    LemonadeProvider,
    /// Put the earlier speech engines back and delete the voice pack's models from Lemonade.
    VoicePack,
    /// Delete the Kokoro model from Lemonade and unset the reading voice when it is ours.
    KokoroModel,
    /// A restricted firmware pack (`fwpack.rs`).
    FwPack(Item),
    /// The extra hardware firmware pack and its download.
    FwHardware,
    /// A driver pack by id (`mac-wifi-brcmfmac`, `mac-bt-broadcom`, `mac-facetimehd`).
    DriverPack(&'static str),
    /// A Linux apps sandbox by name (`debian`, `gui`).
    Sandbox(&'static str),
    /// The model hosts of the Mac guide (Models entries and the host list), by id.
    ModelHosts(Vec<String>),
    /// The Gitea record of "installed services" (not the Secrets, not the data).
    GiteaRecord,
    /// The Git connection "Local Gitea" of this install.
    GiteaConnection,
    /// The CDI specs of the NVIDIA step.
    NvidiaCdi,
    /// Take the GPU settings out of the Lemonade Deployment when it exists.
    LemonadeUnpatch,
    /// Take the GPU settings out of the Ollama Deployment (the one in the cluster) when it exists.
    OllamaUnpatch,
    /// Take the `ollama-local` entry out of Models (the Ollama of the cluster; no host server is touched).
    OllamaProvider,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Step {
    pub op: Op,
    /// Runs only when the person ticked "also delete the data".
    pub data: bool,
    /// Runs only when the data is NOT being deleted (the lighter variant of a step).
    pub unless_data: bool,
    /// A noun phrase: "the Deployment elasticsearch", "the volume elasticsearch-data (your indexes)".
    pub text: String,
}

#[derive(Clone, Debug, Default)]
pub struct PlanCtx {
    pub data: PathBuf,
    /// The Gitea record of this person: its id and the vault entries it names.
    pub gitea: Option<(String, Vec<String>)>,
    /// The model hosts of the Mac guide (ids).
    pub model_hosts: Vec<String>,
    /// The CLI catalogue's own warning for this item (`CliManager::uninstall_warning`), when it has one.
    pub cli_warning: Option<String>,
    /// The data folder is in memory (a live session): everything kept would be lost at power-off.
    pub volatile: bool,
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub item: Item,
    pub steps: Vec<Step>,
    /// The item has no part apart from its data (a sandbox, a model): removing it needs `delete_data`.
    pub data_requires: bool,
    /// What stays whatever the person chose.
    pub always_stays: Vec<String>,
    /// Volume claims in the cluster that the data deletion frees (gigabytes claimed, an upper bound).
    pub claim_gb: u32,
    /// Bytes of the data folders on this machine (read when the plan was made).
    pub data_bytes: u64,
    /// Extra lines the card shows in amber (shared namespace, a restart needed...).
    pub notes: Vec<String>,
}

fn kube(ns: &'static str, kind: &'static str, name: &str, data: bool, text: String) -> Step {
    Step { op: Op::Kube { ns: Some(ns), kind, name: name.into() }, data, unless_data: false, text }
}
fn claim(ns: &'static str, name: &str, text: String) -> Step {
    Step { op: Op::Claim { ns, name: name.into() }, data: true, unless_data: false, text }
}
fn plain(op: Op, text: &str) -> Step {
    Step { op, data: false, unless_data: false, text: text.into() }
}
fn data_step(op: Op, text: &str) -> Step {
    Step { op, data: true, unless_data: false, text: text.into() }
}

pub(crate) fn dir_bytes(p: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(p) else { return 0 };
    if m.is_dir() {
        std::fs::read_dir(p).into_iter().flatten().flatten().map(|e| dir_bytes(&e.path())).sum()
    } else {
        m.len()
    }
}

/// The Deployment and the Service of a one-pod item.
fn workload(ns: &'static str, name: &str, steps: &mut Vec<Step>) {
    steps.push(kube(ns, "deployment", name, false, format!("the Deployment {name} in {ns}")));
    steps.push(kube(ns, "service", name, false, format!("the Service {name} in {ns}")));
}

/// A one-pod item in a namespace: workload, volume claim, Secret (when it has one), vault entries, DASH source.
struct Pod<'a> {
    ns: &'static str,
    name: &'a str,
    claim: String,
    gb: u32,
    what: &'a str,
    secret: Option<String>,
    vault: Vec<String>,
    source: Option<&'a str>,
}

fn pod_plan(item: Item, p: Pod) -> Plan {
    let mut steps = Vec::new();
    workload(p.ns, p.name, &mut steps);
    steps.push(claim(p.ns, &p.claim, format!("the volume {} ({}, up to {} GB claimed)", p.claim, p.what, p.gb)));
    if let Some(s) = &p.secret {
        steps.push(kube(p.ns, "secret", s, true, format!("the Secret {s} (the generated password or keys)")));
    }
    for v in &p.vault {
        steps.push(data_step(Op::Vault(v.clone()), &format!("your vault entry {v}")));
    }
    if let Some(id) = p.source {
        steps.push(data_step(Op::Source(id.into()), &format!("the DASH connection {id}")));
    }
    if EXCLUSIVE_NAMESPACES.contains(&p.ns) {
        steps.push(kube(p.ns, "namespace", p.ns, true, format!("the namespace {} (only this item uses it)", p.ns)));
    }
    let mut always = vec!["everything outside IYAKKI's own namespaces and folders".to_string()];
    if !EXCLUSIVE_NAMESPACES.contains(&p.ns) {
        always.push(format!("the namespace {} (other items may use it)", p.ns));
    }
    Plan { item, steps, data_requires: false, always_stays: always, claim_gb: p.gb, data_bytes: 0, notes: vec![] }
}

fn cli_plan(item: Item, id: &'static str, ctx: &PlanCtx, keeps: &str) -> Plan {
    let mut notes = vec![];
    if let Some(w) = &ctx.cli_warning {
        notes.push(w.clone());
    }
    Plan { item, steps: vec![plain(Op::Hook(Hook::Cli(id)), &format!("the program {} and its folder in the tools folder", item.title()))], data_requires: false, always_stays: vec![keeps.to_string()], claim_gb: 0, data_bytes: 0, notes }
}

/// The removal plan of one item: exactly what goes, and which steps are data.
pub fn plan(item: Item, ctx: &PlanCtx) -> Plan {
    let data = &ctx.data;
    let mut p = match item {
        Item::Node | Item::Python => cli_plan(item, item.cli_id().unwrap_or_default(), ctx, "your own projects and files"),
        Item::DuckDb => cli_plan(item, "duckdb", ctx, "your own database files and DASH sources"),
        Item::ClaudeCode => cli_plan(item, "claude-code", ctx, "your sign-in, settings and projects (they are in your own folders, never touched)"),
        Item::Copilot => cli_plan(item, "github-copilot", ctx, "your sign-in, settings and projects (they are in your own folders, never touched)"),
        Item::Ffmpeg => cli_plan(item, "ffmpeg", ctx, "your media files"),
        Item::Ollama if marked(data, Item::Ollama).is_some_and(|d| ollama_cluster::is_cluster_detail(&d)) => {
            let mut pl = pod_plan(item, Pod { ns: ollama_cluster::NS, name: ollama_cluster::NAME, claim: ollama_cluster::CLAIM.into(), gb: ollama_cluster::STORAGE_GB as u32, what: "your downloaded models", secret: None, vault: vec![], source: None });
            pl.steps.insert(2, plain(Op::Hook(Hook::OllamaProvider), "the entry ollama-local in Settings, Models"));
            let host = ollama_cluster::host_models_dir(data);
            if host.exists() {
                let root = data.join("ollama");
                pl.data_bytes = dir_bytes(&host);
                pl.steps.push(data_step(Op::Path { path: root.clone(), root: root.clone() }, "the models folder of the earlier host Ollama (the folder ollama in the data folder; the move kept it)"));
                pl.notes.push("The earlier host Ollama program stays installed (remove it from the CLI tile if you do not want it); its models folder is deleted only with the data.".into());
            }
            pl
        }
        Item::Ollama => {
            let mut pl = cli_plan(item, "ollama", ctx, "your downloaded models are in the Ollama folder (see the data step)");
            pl.steps.insert(0, plain(Op::Hook(Hook::OllamaServer), "the running Ollama of IYAKKI and its Models entry ollama-local"));
            let models = data.join("ollama");
            pl.data_bytes = dir_bytes(&models);
            pl.steps.push(data_step(Op::Path { path: models.clone(), root: models }, "your downloaded Ollama models (the folder ollama in the data folder)"));
            pl
        }
        Item::K3s => {
            let l = crate::k3sdev::Layout::new(data);
            let mut keep = plain(Op::Hook(Hook::K3sKeepData), "the cluster (it is stopped) with the k3s program, its kubeconfig and its log");
            keep.unless_data = true;
            let all = data_step(Op::Hook(Hook::K3sAll), "the whole cluster folder k3s-dev: the cluster's database and EVERY volume of every item in it");
            let mut pl = Plan { item, steps: vec![keep, all], data_requires: false, always_stays: vec!["everything outside the folder k3s-dev".into()], claim_gb: 0, data_bytes: dir_bytes(&l.data()), notes: vec![] };
            pl.notes.push("The cluster's data folder (its database and the volumes of the items you installed) is kept unless you tick the box; a later install of k3s finds it.".into());
            pl
        }
        Item::Elasticsearch => pod_plan(item, Pod { ns: NAMESPACE, name: "elasticsearch", claim: "elasticsearch-data".into(), gb: 5, what: "your indexes", secret: None, vault: vec![], source: Some(ES_SOURCE_ID) }),
        Item::OpenSearch => pod_plan(item, Pod { ns: NAMESPACE, name: "opensearch", claim: "opensearch-data".into(), gb: 5, what: "your indexes", secret: None, vault: vec![], source: Some(opensearch::SOURCE_ID) }),
        Item::Prometheus => {
            let mut pl = pod_plan(item, Pod { ns: PROM_NAMESPACE, name: "prometheus", claim: "prometheus-data".into(), gb: 5, what: "seven days of metrics", secret: None, vault: vec![], source: Some(PROM_SOURCE_ID) });
            let at = pl.steps.iter().position(|s| s.data).unwrap_or(pl.steps.len());
            let extra = vec![
                kube(PROM_NAMESPACE, "configmap", "prometheus-config", false, "the ConfigMap prometheus-config".into()),
                kube(PROM_NAMESPACE, "serviceaccount", "prometheus", false, "the ServiceAccount prometheus".into()),
                Step { op: Op::Kube { ns: None, kind: "clusterrolebinding", name: "iyakki-prometheus".into() }, data: false, unless_data: false, text: "the cluster role binding iyakki-prometheus (its read-only access)".into() },
                Step { op: Op::Kube { ns: None, kind: "clusterrole", name: "iyakki-prometheus".into() }, data: false, unless_data: false, text: "the cluster role iyakki-prometheus".into() },
            ];
            for (i, s) in extra.into_iter().enumerate() {
                pl.steps.insert(at + i, s);
            }
            pl
        }
        Item::GraphDb => pod_plan(item, Pod { ns: NAMESPACE, name: "graphdb", claim: "graphdb-data".into(), gb: 5, what: "your graphs", secret: Some(GRAPH_SECRET.into()), vault: vec![GRAPH_VAULT_NAME.into()], source: Some(GRAPH_SOURCE_ID) }),
        Item::ClickHouse => pod_plan(item, Pod { ns: NAMESPACE, name: "clickhouse", claim: "clickhouse-data".into(), gb: 5, what: "your tables", secret: Some(clickhouse::SECRET.into()), vault: vec![clickhouse::VAULT_NAME.into()], source: Some(clickhouse::SOURCE_ID) }),
        Item::Matrix => pod_plan(item, Pod { ns: matrix::NS, name: "matrix", claim: "matrix-data".into(), gb: 5, what: "rooms and messages", secret: Some(matrix::SECRET.into()), vault: vec![matrix::VAULT_NAME.into()], source: None }),
        Item::SeaweedFs | Item::RustFs => {
            let e = objstore::engine_of(item).unwrap_or(&objstore::SEAWEEDFS);
            pod_plan(item, Pod { ns: objstore::NS, name: e.name, claim: e.claim(), gb: 20, what: "your stored objects", secret: Some(e.secret()), vault: vec![e.access_vault(), e.secret_vault(), e.record_name()], source: None })
        }
        Item::Gitea => {
            let ns = gitea::NAMESPACE;
            let mut steps = vec![
                kube(ns, "deployment", "gitea", false, format!("the Deployment gitea in {ns}")),
                kube(ns, "service", "gitea-http", false, format!("the Service gitea-http in {ns}")),
                kube(ns, "service", "gitea-ssh", false, format!("the Service gitea-ssh in {ns}")),
                plain(Op::Hook(Hook::GiteaRecord), "the entry in your installed services (Settings, Dev cluster)"),
                claim(ns, "gitea-data", "the volume gitea-data (your repositories, up to 10 GB claimed)".into()),
                kube(ns, "secret", crate::platform::gitea::ADMIN_SECRET, true, "the Secret gitea-admin (the administrator password)".into()),
                kube(ns, "secret", gitea::TOKEN_SECRET, true, "the Secret gitea-iyakki-token".into()),
            ];
            if let Some((_, names)) = &ctx.gitea {
                for v in names {
                    steps.push(data_step(Op::Vault(v.clone()), &format!("your vault entry {v}")));
                }
            }
            steps.push(data_step(Op::Hook(Hook::GiteaConnection), "the Git connection \"Local Gitea\""));
            steps.push(kube(ns, "namespace", ns, true, format!("the namespace {ns} (only this item uses it)")));
            Plan { item, steps, data_requires: false, always_stays: vec!["everything outside IYAKKI's own namespaces and folders".into()], claim_gb: 10, data_bytes: 0, notes: vec![] }
        }
        Item::Registry => {
            let mut steps = Vec::new();
            workload(REGISTRY_NS, crate::regsetup::IN_NAME, &mut steps);
            steps.push(claim(REGISTRY_NS, "registry-data", "the volume registry-data (every image pushed to it, 5 to 50 GB claimed)".into()));
            steps.push(kube(REGISTRY_NS, "namespace", REGISTRY_NS, true, format!("the namespace {REGISTRY_NS} (only the registry uses it)")));
            Plan { item, steps, data_requires: false, always_stays: vec!["apps already running (their pods cannot pull again from the registry)".into()], claim_gb: 50, data_bytes: 0, notes: vec!["Only the registry of the managed cluster is removed; a registry on another cluster is never touched.".into()] }
        }
        Item::Lemonade => {
            let mut pl = pod_plan(item, Pod { ns: LEMONADE_NAMESPACE, name: "lemonade", claim: "lemonade-data".into(), gb: 30, what: "your downloaded models", secret: None, vault: vec![], source: None });
            pl.steps.insert(2, plain(Op::Hook(Hook::LemonadeProvider), "the entry lemonade-local in Settings, Models"));
            pl
        }
        Item::Kokoro => Plan {
            item,
            steps: vec![data_step(Op::Hook(Hook::KokoroModel), "the Kokoro voice model kokoro-v1 in Lemonade (about 330 MB) and the reading voice that uses it")],
            data_requires: true,
            always_stays: vec!["Lemonade and its other models".into()],
            claim_gb: 0,
            data_bytes: 330 << 20,
            notes: vec!["Kokoro is a model that lives in Lemonade, so removing it is deleting the model: tick the box to confirm.".into()],
        },
        Item::VoicePack => Plan {
            item,
            steps: vec![data_step(Op::Hook(Hook::VoicePack), "the Whisper tiny and Kokoro models in Lemonade, and the speech engines go back to what they were before")],
            data_requires: true,
            always_stays: vec!["Lemonade and its other models".into()],
            claim_gb: 0,
            data_bytes: (crate::voicepack::WHISPER_MB as u64 + crate::voicepack::KOKORO_MB as u64) << 20,
            notes: vec!["The voice pack is two models in Lemonade, so removing it is deleting them: tick the box to confirm.".into()],
        },
        Item::MacHost => {
            let ids = ctx.model_hosts.clone();
            let text = if ids.is_empty() { "the model host entries of this guide (none are set)".to_string() } else { format!("the model hosts {} in Settings, Models", ids.join(", ")) };
            Plan { item, steps: vec![plain(Op::Hook(Hook::ModelHosts(ids)), &text)], data_requires: false, always_stays: vec!["the other computer and anything installed on it".into(), "the key you saved in your vault for it (delete it in the vault)".into()], claim_gb: 0, data_bytes: 0, notes: vec![] }
        }
        Item::GpuVulkan => {
            let ns = crate::gpu_workloads::NAMESPACE;
            Plan {
                item,
                steps: vec![
                    plain(Op::Hook(Hook::LemonadeUnpatch), "the GPU settings of the Lemonade Deployment (when it exists)"),
                    plain(Op::Hook(Hook::OllamaUnpatch), "the GPU settings of the Ollama Deployment (when it exists)"),
                    kube(ns, "daemonset", "iyakki-gpu-device-plugin", false, format!("the device plugin iyakki-gpu-device-plugin in {ns}")),
                ],
                data_requires: false,
                always_stays: vec!["a GPU Ollama (ollama-gpu) if you made one: it cannot start without the plugin".into(), "the namespace iyakki-ai (Lemonade uses it)".into()],
                claim_gb: 0,
                data_bytes: 0,
                notes: vec!["Models run on the processor again.".into()],
            }
        }
        Item::GpuNvidia => {
            let l = crate::gpu_nvidia::Layout::new(data);
            let base = data.join("gpu/nvidia");
            let mut pl = Plan {
                item,
                steps: vec![
                    plain(Op::Hook(Hook::LemonadeUnpatch), "the GPU settings of the Lemonade Deployment (when it exists)"),
                    plain(Op::Hook(Hook::OllamaUnpatch), "the GPU settings of the Ollama Deployment (when it exists)"),
                    plain(Op::Kube { ns: Some("kube-system"), kind: "daemonset", name: "nvidia-device-plugin".into() }, "the device plugin nvidia-device-plugin in kube-system"),
                    plain(Op::Hook(Hook::NvidiaCdi), "the CDI specs of the NVIDIA step"),
                    plain(Op::Path { path: l.extracted(), root: base.clone() }, "the unpacked driver files (folder extracted)"),
                    plain(Op::Path { path: l.stage(), root: base.clone() }, "the staged libraries, nvidia-smi and firmware (folder stage)"),
                    data_step(Op::Path { path: l.root.clone(), root: base }, "the downloaded NVIDIA driver file (about 400 MB, kept so a later install does not download it again)"),
                ],
                data_requires: false,
                always_stays: vec!["the open kernel modules of the image (they stay loaded until the next restart)".into()],
                claim_gb: 0,
                data_bytes: dir_bytes(&l.root),
                notes: vec!["The containerd CDI drop-in written for the cluster is left in place (removing it needs a cluster restart).".into()],
            };
            pl.notes.push("Models run on the processor or the Vulkan GPU again.".into());
            pl
        }
        Item::FfmpegLibs => {
            let dir = crate::avaddon::addon_dir(data);
            Plan { item, steps: vec![plain(Op::Path { path: dir.clone(), root: dir.clone() }, "the browser audio and video libraries (folder addons/ffmpeg)")], data_requires: false, always_stays: vec!["the packages in /local/packages that you supplied".into()], claim_gb: 0, data_bytes: dir_bytes(&dir), notes: vec!["The BROWSER tile plays H.264 and AAC again only after you install the add-on once more.".into()] }
        }
        Item::MacFirmware => {
            let work = crate::macfw::work_dir(data);
            Plan {
                item,
                steps: vec![
                    plain(Op::Hook(Hook::DriverPack(crate::macfw::PACK_WIFI)), "the Wi-Fi driver pack mac-wifi-brcmfmac"),
                    plain(Op::Hook(Hook::DriverPack(crate::macfw::PACK_BT)), "the Bluetooth driver pack mac-bt-broadcom"),
                    data_step(Op::Path { path: work.clone(), root: work.clone() }, "the downloaded Apple recovery image and the unpacked files (about 1 GB)"),
                ],
                data_requires: false,
                always_stays: vec!["drivers already loaded keep working until the next restart".into()],
                claim_gb: 0,
                data_bytes: dir_bytes(&work),
                notes: vec![],
            }
        }
        Item::MacCamera => {
            let work = crate::camfw::work_dir(data);
            Plan {
                item,
                steps: vec![
                    plain(Op::Hook(Hook::DriverPack(crate::camfw::PACK_ID)), "the camera driver pack mac-facetimehd"),
                    plain(Op::Path { path: crate::camfw::result_path(data), root: work.clone() }, "the record of the last camera job"),
                    data_step(Op::Path { path: work.join("download"), root: work.clone() }, "the downloaded firmware pieces (about 18 MB)"),
                ],
                data_requires: false,
                always_stays: vec!["the open facetimehd driver of the image".into()],
                claim_gb: 0,
                data_bytes: dir_bytes(&work),
                notes: vec![],
            }
        }
        Item::LinuxApps => Plan {
            item,
            steps: vec![data_step(Op::Hook(Hook::Sandbox("debian")), "the sandbox debian with everything inside it: packages you installed and files you saved there")],
            data_requires: true,
            always_stays: vec!["other sandboxes you made".into(), "the folders you shared with the sandbox (they are only mounted, never deleted)".into()],
            claim_gb: 0,
            data_bytes: dir_bytes(&data.join("linuxapps").join("debian")),
            notes: vec!["The sandbox is the item and also your data in it: tick the box to confirm.".into()],
        },
        Item::LinuxWindows => Plan {
            item,
            steps: vec![data_step(Op::Hook(Hook::Sandbox("gui")), "the sandbox gui (display server, window manager) with everything inside it")],
            data_requires: true,
            always_stays: vec!["the sandbox debian and other sandboxes".into()],
            claim_gb: 0,
            data_bytes: dir_bytes(&data.join("linuxapps").join("gui")),
            notes: vec!["The sandbox is the item and also your data in it: tick the box to confirm.".into()],
        },
        Item::FwQualcommBt | Item::FwRealtekNet | Item::FwBroadcomNet | Item::FwAtheros9170 | Item::FwMediatek => Plan {
            item,
            steps: vec![plain(Op::Hook(Hook::FwPack(item)), "the firmware files of this pack (the driver pack and its links)")],
            data_requires: false,
            always_stays: vec!["your acceptance of the vendor's licence".into(), "a driver that already loaded the firmware keeps it until the next restart".into()],
            claim_gb: 0,
            data_bytes: 0,
            notes: vec![],
        },
        Item::FwHardware => Plan {
            item,
            steps: vec![plain(Op::Hook(Hook::FwHardware), "the extra firmware pack and the pieces it downloaded")],
            data_requires: false,
            always_stays: vec!["your acceptance of the vendors' licences".into(), "a driver that already loaded the firmware keeps it until the next restart".into()],
            claim_gb: 0,
            data_bytes: crate::fwextra::leftovers(data),
            notes: vec![],
        },
    };
    if p.has_data() && !p.data_requires || p.claim_gb > 0 {
        if p.claim_gb > 0 {
            p.notes.push(format!("Where the data lives: the volumes are folders of the cluster under {} on the data volume; they are kept (reclaim policy Retain) even if a claim or namespace is deleted by hand.", storage_root(data).display()));
        }
        if ctx.volatile {
            p.notes.push("WARNING: this machine has no persistent data volume (a live session): the data is in memory and is lost at the next restart whatever you choose here.".into());
        }
    }
    p
}

impl Plan {
    fn active(&self, delete_data: bool) -> impl Iterator<Item = &Step> {
        self.steps.iter().filter(move |s| (!s.data || delete_data) && !(s.unless_data && delete_data))
    }

    /// Does the plan hold anything the person's data lives in?
    pub fn has_data(&self) -> bool {
        self.steps.iter().any(|s| s.data)
    }

    /// "about 5 GB of volume claim and 120 MB of files on this machine".
    pub fn space_text(&self) -> String {
        let mut parts = Vec::new();
        if self.claim_gb > 0 {
            parts.push(format!("up to {} GB of volume claimed in the cluster", self.claim_gb));
        }
        if self.data_bytes > 0 {
            parts.push(if self.data_bytes >= 1 << 30 { format!("{:.1} GB of files on this machine", self.data_bytes as f64 / (1u64 << 30) as f64) } else { format!("{} MB of files on this machine", (self.data_bytes >> 20).max(1)) });
        }
        if parts.is_empty() { "no measurable space".into() } else { format!("about {}", parts.join(" and ")) }
    }

    /// The review: what goes, what stays, what the data deletion adds.
    pub fn review(&self, delete_data: bool) -> Review {
        let goes: Vec<String> = self.steps.iter().filter(|s| !s.data && !(s.unless_data && delete_data)).map(|s| s.text.clone()).collect();
        let data: Vec<String> = self.steps.iter().filter(|s| s.data).map(|s| s.text.clone()).collect();
        let mut stays: Vec<String> = Vec::new();
        if !delete_data {
            stays.extend(data.iter().map(|d| format!("{d} (kept)")));
        }
        stays.extend(self.always_stays.iter().cloned());
        Review {
            title: format!("Remove {}", self.item.title()),
            goes,
            stays,
            data: if delete_data { data } else { vec![] },
            data_available: self.has_data(),
            data_required: self.data_requires,
            space: if delete_data || self.data_requires { Some(self.space_text()) } else { None },
            space_if_ticked: self.space_text(),
            notes: self.notes.clone(),
        }
    }
}

/// A removal as the card shows it (page and chat).
#[derive(Clone, Debug, PartialEq)]
pub struct Review {
    pub title: String,
    pub goes: Vec<String>,
    pub stays: Vec<String>,
    /// Only filled when the data deletion is on.
    pub data: Vec<String>,
    pub data_available: bool,
    pub data_required: bool,
    pub space: Option<String>,
    pub space_if_ticked: String,
    pub notes: Vec<String>,
}

impl Review {
    /// The whole card as lines of text (the chat card; the page draws the same parts).
    pub fn lines(&self, delete_data: bool) -> Vec<String> {
        let mut v = vec![format!("{}.", self.title)];
        if !self.goes.is_empty() {
            v.push("Goes:".into());
            v.extend(self.goes.iter().map(|g| format!("  - {g}")));
        }
        if delete_data && !self.data.is_empty() {
            v.push("ALSO DELETED, and this cannot be undone:".into());
            v.extend(self.data.iter().map(|g| format!("  - {g}")));
            v.push(format!("Frees {}.", self.space_if_ticked));
        } else if self.data_available {
            v.push(format!("Your data is kept. Deleting it too would free {} and cannot be undone.", self.space_if_ticked));
        }
        if !self.stays.is_empty() {
            v.push("Stays:".into());
            v.extend(self.stays.iter().map(|g| format!("  - {g}")));
        }
        v.extend(self.notes.iter().cloned());
        if self.data_required && !delete_data {
            v.push("This item is its own data: it is only removed when the data deletion is chosen.".into());
        }
        v
    }
}

// ------------------------------------------------------------------------------------------------ the checks

fn dns_name(n: &str) -> bool {
    !n.is_empty() && n.len() <= 63 && n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '.') && !n.starts_with('-') && !n.starts_with('.')
}

fn vault_name_ok(n: &str) -> bool {
    !n.is_empty() && n.len() <= 120 && n.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')) && (n.starts_with("svc.") || n.starts_with("s3."))
}

/// A path lies under `root` by its components (no `..`), and `root` lies under the data folder and is not the data folder itself.
pub fn path_ok(path: &Path, root: &Path, data: &Path) -> Result<(), String> {
    use std::path::Component;
    let clean = |p: &Path| p.is_absolute() && p.components().all(|c| !matches!(c, Component::ParentDir | Component::CurDir));
    if !clean(path) || !clean(root) || !clean(data) {
        return Err(format!("{} is not a plain absolute path; nothing was deleted", path.display()));
    }
    if root == data || !root.starts_with(data) {
        return Err(format!("{} is not inside IYAKKI's data folder; nothing was deleted", root.display()));
    }
    if !path.starts_with(root) {
        return Err(format!("{} is not inside {}; nothing was deleted", path.display(), root.display()));
    }
    Ok(())
}

/// Refuse any step whose target is not IYAKKI's own. Runs before every step, whatever built the plan.
pub fn check_step(s: &Step, data: &Path) -> Result<(), String> {
    match &s.op {
        Op::Kube { ns, kind, name } => {
            if !KINDS.contains(kind) || !dns_name(name) {
                return Err(format!("refused: {kind} {name} is not a plain object name"));
            }
            match (ns, *kind) {
                (None, "clusterrole" | "clusterrolebinding") if name == "iyakki-prometheus" => Ok(()),
                (None, _) => Err(format!("refused: the cluster-wide object {kind} {name} is not one of IYAKKI's")),
                (Some("kube-system"), "daemonset") if name == "nvidia-device-plugin" => Ok(()),
                (Some(n), "namespace") if EXCLUSIVE_NAMESPACES.contains(n) && name == n => Ok(()),
                (Some(_), "namespace") => Err(format!("refused: the namespace {name} is not one only IYAKKI's item uses")),
                (Some(n), _) if OWN_NAMESPACES.contains(n) || *n == REGISTRY_NS => Ok(()),
                (Some(n), _) => Err(format!("refused: the namespace {n} is not one of IYAKKI's")),
            }
        }
        Op::Claim { ns, name } => {
            if dns_name(name) && (OWN_NAMESPACES.contains(ns) || *ns == REGISTRY_NS) { Ok(()) } else { Err(format!("refused: the claim {name} in {ns} is not one of IYAKKI's")) }
        }
        Op::Path { path, root } => path_ok(path, root, data),
        Op::Vault(n) => {
            if vault_name_ok(n) { Ok(()) } else { Err("refused: not a vault entry IYAKKI made".into()) }
        }
        Op::Source(id) => {
            if dns_name(id) { Ok(()) } else { Err("refused: not a DASH connection IYAKKI made".into()) }
        }
        Op::Hook(Hook::Sandbox(n)) if !matches!(*n, "debian" | "gui") => Err(format!("refused: the sandbox {n} is not one of IYAKKI's")),
        Op::Hook(Hook::DriverPack(id)) if !matches!(*id, "mac-wifi-brcmfmac" | "mac-bt-broadcom" | "mac-facetimehd") => Err(format!("refused: the driver pack {id} is not one of Quick install's")),
        Op::Hook(Hook::ModelHosts(ids)) if ids.iter().any(|i| !dns_name(i)) => Err("refused: a model host id is not a plain name".into()),
        Op::Hook(_) => Ok(()),
    }
}

// ------------------------------------------------------------------------------------------------ dependencies

/// Installed items that need `item` to run (the install order's hard `needs`): k3s, Lemonade...
pub fn direct_dependants(item: Item, installed: &dyn Fn(Item) -> bool) -> Vec<Item> {
    Item::ALL.into_iter().filter(|i| *i != item && i.needs().contains(&item) && installed(*i)).collect()
}

/// Every installed item that needs `item`, directly or through another, the ones that need most first, so removing them in this order
/// never leaves a dependant standing.
pub fn cascade(item: Item, installed: &dyn Fn(Item) -> bool) -> Vec<Item> {
    fn visit(i: Item, installed: &dyn Fn(Item) -> bool, out: &mut Vec<Item>) {
        for d in direct_dependants(i, installed) {
            visit(d, installed, out);
            if !out.contains(&d) {
                out.push(d);
            }
        }
    }
    let mut out = Vec::new();
    visit(item, installed, &mut out);
    out
}

/// Installed items that only use `item` (a runtime): warned about, not refused.
pub fn soft_users(item: Item, installed: &dyn Fn(Item) -> bool) -> Vec<Item> {
    Item::ALL.into_iter().filter(|i| *i != item && !i.needs().contains(&item) && i.needs_now().contains(&item) && installed(*i)).collect()
}

fn names(v: &[Item]) -> String {
    v.iter().map(|i| i.title()).collect::<Vec<_>>().join(", ")
}

/// The plain refusal for an item other installed items need.
pub fn refusal_text(item: Item, deps: &[Item]) -> String {
    format!("{} cannot be removed while {} {} installed: {} need{} it. Remove {} first, or choose to remove them together with it. Nothing was changed.", item.title(), names(deps), if deps.len() == 1 { "is" } else { "are" }, if deps.len() == 1 { "it needs" } else { "they need" }.replace("it needs", "it").replace("they need", "they"), if deps.len() == 1 { "s" } else { "" }, if deps.len() == 1 { "that one" } else { "those" })
}

/// Plain words for the review: the dependants and the users of the item.
pub fn dependency_lines(item: Item, installed: &dyn Fn(Item) -> bool) -> (Vec<Item>, Vec<String>) {
    let deps = cascade(item, installed);
    let mut warn = Vec::new();
    let soft = soft_users(item, installed);
    if !soft.is_empty() {
        warn.push(format!("{} is used by {}; they stop working if it is removed (installing one of them again brings {} back first).", item.title(), names(&soft), item.title()));
    }
    (deps, warn)
}

/// Only IYAKKI's own items can be removed: a cluster found on the machine that is not IYAKKI's is never touched.
pub fn not_ours(p: &Presence) -> Option<String> {
    match p {
        Presence::Elsewhere(d) => Some(format!("this is not IYAKKI's ({d}), so IYAKKI will not remove it. Nothing was changed.")),
        _ => None,
    }
}

// ------------------------------------------------------------------------------------------------ volumes

/// The claims a cluster item keeps its data in: `(namespace, claim)`.
pub fn claims_of(item: Item) -> Vec<(&'static str, String)> {
    // the plan of Ollama depends on where it runs (the marker); its claim is the cluster one whichever it is, so it is kept (Retain)
    if item == Item::Ollama {
        return vec![(ollama_cluster::NS, ollama_cluster::CLAIM.to_string())];
    }
    plan(item, &PlanCtx::default()).steps.into_iter().filter_map(|s| if let Op::Claim { ns, name } = s.op { Some((ns, name)) } else { None }).collect()
}

/// The local-path provisioner's folder: where the volumes of the managed cluster are kept.
pub fn storage_root(data: &Path) -> PathBuf {
    crate::k3sdev::Layout::new(data).data().join("storage")
}

/// Make the volume behind each claim survive the claim: the bound volume's reclaim policy is set to Retain (k3s' local-path default is
/// Delete, which would delete the data with the claim or the namespace). Idempotent: a volume that is Retain already is not touched, a
/// claim that is not bound yet is skipped. `kubectl` is a closure so tests give a fake. Returns the volumes patched.
pub fn retain_claims(kubectl: &mut dyn FnMut(&[&str]) -> Result<String, String>, claims: &[(&'static str, String)]) -> Vec<String> {
    let mut done = Vec::new();
    for (ns, name) in claims {
        let Ok(pv) = kubectl(&["-n", ns, "get", "pvc", name, "-o", "jsonpath={.spec.volumeName}"]) else { continue };
        let pv = pv.trim().to_string();
        if !dns_name(&pv) {
            continue;
        }
        let Ok(policy) = kubectl(&["get", "pv", &pv, "-o", "jsonpath={.spec.persistentVolumeReclaimPolicy}"]) else { continue };
        if policy.trim() == "Retain" {
            continue;
        }
        if kubectl(&["patch", "pv", &pv, "-p", r#"{"spec":{"persistentVolumeReclaimPolicy":"Retain"}}"#]).is_ok() {
            done.push(pv);
        }
    }
    done
}

/// [`retain_claims`] for every cluster item that is installed, on the real cluster (when it answers). Called after an install and when the
/// Quick install page refreshes.
pub fn retain_all(svc: &crate::services::Services) -> Vec<String> {
    if svc.k3sdev.state() != crate::k3sdev::Svc::Running {
        return vec![];
    }
    let mut claims = Vec::new();
    for it in Item::ALL {
        if it.is_service() && marked(svc.dir(), it).is_some() {
            claims.extend(claims_of(it));
        }
    }
    let mut k = |a: &[&str]| {
        let o = svc.k3sdev.kubectl(a);
        if o.code == Some(0) { Ok(o.stdout) } else { Err(o.combined()) }
    };
    retain_claims(&mut k, &claims)
}

/// Delete a claim, the (retained) volume it was bound to and that volume's folder under the cluster's storage folder.
fn delete_claim(env: &mut dyn UninstallEnv, data: &Path, ns: &str, name: &str) -> Result<(), String> {
    let pv = env.kubectl(&["-n", ns, "get", "pvc", name, "-o", "jsonpath={.spec.volumeName}", "--ignore-not-found"]).unwrap_or_default().trim().to_string();
    let folder = if dns_name(&pv) {
        env.kubectl(&["get", "pv", &pv, "-o", "jsonpath={.spec.local.path}{.spec.hostPath.path}", "--ignore-not-found"]).unwrap_or_default().trim().to_string()
    } else {
        String::new()
    };
    env.kubectl(&["-n", ns, "delete", "pvc", name, "--ignore-not-found", "--wait=true", "--timeout=120s"])?;
    if dns_name(&pv) && pv.starts_with("pvc-") {
        env.kubectl(&["delete", "pv", &pv, "--ignore-not-found"])?;
        if !folder.is_empty() {
            let root = storage_root(data);
            let p = PathBuf::from(&folder);
            // only a folder directly under the cluster's own storage folder, named for this volume
            let named = p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with(&format!("{pv}_")));
            if named && p.parent() == Some(root.as_path()) {
                path_ok(&p, &root, data)?;
                remove_path(&p)?;
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------------------------------------ running

/// What the runner needs from the machine (fakes in tests). Files are handled by the runner itself with `std::fs`, inside the checks.
pub trait UninstallEnv {
    /// `kubectl <args>`: its output, or the plain reason it failed.
    fn kubectl(&mut self, args: &[&str]) -> Result<String, String>;
    /// Delete one vault entry of the person (not an error when it is not there).
    fn vault_delete(&mut self, name: &str) -> Result<(), String>;
    /// Remove a DASH source (not an error when it is not there).
    fn source_remove(&mut self, id: &str) -> Result<(), String>;
    /// A named hook; one plain line (or "was not there").
    fn hook(&mut self, h: &Hook) -> Result<String, String>;
    /// The audit line: the action and the item, never a secret.
    fn audit(&mut self, action: &str, detail: &str);
    /// The step that is running now (for the page's progress).
    fn progress(&mut self, _line: &str) {}
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Opts {
    pub delete_data: bool,
    pub with_dependants: bool,
}

fn remove_path(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => Err(format!("{} is a link; nothing was deleted", path.display())),
        Ok(m) if m.is_dir() => std::fs::remove_dir_all(path).map_err(|e| format!("{}: {e}", path.display())),
        Ok(_) => std::fs::remove_file(path).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

fn run_step(s: &Step, env: &mut dyn UninstallEnv, data: &Path) -> Result<(), String> {
    check_step(s, data)?;
    match &s.op {
        Op::Kube { ns, kind, name } => {
            let out = match ns {
                Some(n) if *kind == "namespace" => env.kubectl(&["delete", "namespace", n, "--ignore-not-found", "--wait=false"]),
                Some(n) => env.kubectl(&["-n", n, "delete", kind, name, "--ignore-not-found", "--wait=true", "--timeout=120s"]),
                None => env.kubectl(&["delete", kind, name, "--ignore-not-found"]),
            };
            out.map(|_| ())
        }
        Op::Claim { ns, name } => delete_claim(env, data, ns, name),
        Op::Path { path, .. } => remove_path(path),
        Op::Vault(n) => env.vault_delete(n),
        Op::Source(id) => env.source_remove(id),
        Op::Hook(h) => env.hook(h).map(|_| ()),
    }
}

/// Run one plan. With `delete_data` off the data steps do not run; an item that is its own data refuses to run without it.
/// The marker goes only when every step worked, so a failure leaves the item shown and Retry runs the same plan.
pub fn run(plan: &Plan, env: &mut dyn UninstallEnv, data: &Path, delete_data: bool) -> Result<String, String> {
    let (item, title) = (plan.item, plan.item.title());
    if plan.data_requires && !delete_data {
        return Err(format!("{title} is only its own data, so it is removed only when you choose to delete the data too. Nothing was changed."));
    }
    // check everything first: a refused target stops the whole removal before the first change
    for s in plan.active(delete_data) {
        check_step(s, data)?;
    }
    env.audit("quickinstall.uninstall.start", &format!("{} data={}", item.id(), if delete_data { "deleted" } else { "kept" }));
    let total = plan.active(delete_data).count();
    for (n, s) in plan.active(delete_data).enumerate() {
        env.progress(&format!("Step {} of {}: {}", n + 1, total, s.text));
        if let Err(e) = run_step(s, env, data) {
            env.audit("quickinstall.uninstall.failed", item.id());
            return Err(format!("{title}: could not remove {} ({e}). {} step(s) were done; press Retry to run the rest again.", s.text, n));
        }
    }
    unmark(data, item);
    env.audit("quickinstall.uninstall.done", &format!("{} data={}", item.id(), if delete_data { "deleted" } else { "kept" }));
    Ok(if delete_data {
        format!("{title} removed, with its data.")
    } else if plan.has_data() {
        format!("{title} removed. Your data is kept, so installing it again finds it.")
    } else {
        format!("{title} removed.")
    })
}

/// Remove an item, and its dependants first when `o.with_dependants`. Dependants are removed with their data kept.
pub fn uninstall(item: Item, o: Opts, installed: &dyn Fn(Item) -> bool, ctx: &PlanCtx, env: &mut dyn UninstallEnv) -> Result<String, String> {
    let deps = cascade(item, installed);
    if !deps.is_empty() && !o.with_dependants {
        return Err(refusal_text(item, &deps));
    }
    let mut done = Vec::new();
    for d in &deps {
        let mut c = ctx.clone();
        c.cli_warning = None;
        done.push(run(&plan(*d, &c), env, &ctx.data, false)?);
    }
    done.push(run(&plan(item, ctx), env, &ctx.data, o.delete_data)?);
    Ok(done.join(" "))
}

// ------------------------------------------------------------------------------------------------ this machine

/// The plan context of this machine and person.
pub fn real_ctx(svc: &crate::services::Services, user: &str, item: Item) -> PlanCtx {
    let data = svc.dir().to_path_buf();
    let gitea = gitea::record_for(&data, user).map(|r| {
        let mut v = vec![gitea::admin_vault(&r), gitea::token_vault(&r)];
        for n in &r.vault {
            if !v.contains(n) {
                v.push(n.clone());
            }
        }
        (r.id.clone(), v)
    });
    let model_hosts = if item == Item::MacHost { crate::modelhost::Hosts::new(&data).list().into_iter().map(|h| h.id).collect() } else { vec![] };
    let cli_warning = item.cli_id().and_then(|id| svc.clis.uninstall_warning(id));
    let volatile = iyakki_storage::data_location(&iyakki_storage::Roots::real(), &data) == iyakki_storage::DataLocation::Memory;
    PlanCtx { data, gitea, model_hosts, cli_warning, volatile }
}

/// Is the item installed here (IYAKKI's own, or installed earlier)?
pub fn is_installed(svc: &crate::services::Services, item: Item) -> bool {
    matches!(presence(svc, item), Presence::Installed(_) | Presence::Earlier(_))
}

/// Can a Remove be offered: IYAKKI's own install, or a leftover marker (a half-installed item).
pub fn removable(svc: &crate::services::Services, item: Item) -> bool {
    match presence(svc, item) {
        Presence::Missing => marked(svc.dir(), item).is_some(),
        Presence::Elsewhere(_) => false,
        _ => true,
    }
}

/// The real machine for [`run`]: kubectl of the managed cluster, the person's vault, the supervisor's hooks.
pub struct RealEnv {
    pub svc: Arc<crate::services::Services>,
    pub user: String,
    pub progress: Arc<Mutex<String>>,
}

impl RealEnv {
    pub fn new(svc: Arc<crate::services::Services>, user: &str) -> RealEnv {
        RealEnv { svc, user: user.to_string(), progress: Arc::new(Mutex::new(String::new())) }
    }

    fn remove_providers(&self, pick: &dyn Fn(&crate::providers::ProviderCfg) -> bool) -> usize {
        let ids: Vec<String> = self.svc.settings.lock().unwrap_or_else(|e| e.into_inner()).get().providers.iter().filter(|p| pick(p)).map(|p| p.id.clone()).collect();
        let mut n = 0;
        for id in ids {
            if self.svc.settings.lock().unwrap_or_else(|e| e.into_inner()).remove_provider(&id).is_ok() {
                n += 1;
            }
        }
        n
    }
}

fn say_none(what: &str) -> Result<String, String> {
    Ok(format!("{what} was not there"))
}

impl UninstallEnv for RealEnv {
    fn kubectl(&mut self, args: &[&str]) -> Result<String, String> {
        let o = self.svc.k3sdev.kubectl(args);
        if o.code == Some(0) {
            return Ok(o.stdout);
        }
        let text = o.combined().trim().to_string();
        // the cluster is not running or not installed: there is nothing in it to delete
        if self.svc.k3sdev.installed().is_none() {
            return Ok(String::new());
        }
        Err(text.lines().last().unwrap_or("kubectl failed").to_string())
    }

    fn vault_delete(&mut self, name: &str) -> Result<(), String> {
        if !self.svc.user_names(&self.user).unwrap_or_default().iter().any(|n| n == name) {
            return Ok(());
        }
        self.svc.user_delete(&self.user, name).map_err(|e| e.to_string())
    }

    fn source_remove(&mut self, id: &str) -> Result<(), String> {
        for who in [self.user.as_str(), "system"] {
            crate::dash::store::Sources::open(self.svc.dir(), who).remove(id)?;
        }
        Ok(())
    }

    fn audit(&mut self, action: &str, detail: &str) {
        self.svc.k3sdev.audit_line(&self.user, action, detail);
    }

    fn progress(&mut self, line: &str) {
        *self.progress.lock().unwrap_or_else(|e| e.into_inner()) = line.to_string();
    }

    fn hook(&mut self, h: &Hook) -> Result<String, String> {
        let data = self.svc.dir().to_path_buf();
        match h {
            Hook::Cli(id) => {
                if !self.svc.clis.installed().iter().any(|i| i.id == *id) {
                    return say_none(id);
                }
                self.svc.clis.remove(id).map(|_| format!("{id} removed"))
            }
            Hook::K3sKeepData => {
                if self.svc.k3sdev.installed().is_none() {
                    return say_none("the cluster");
                }
                self.svc.k3sdev.uninstall_keep_data(&self.user).map(|_| "the cluster program is removed, its data kept".into())
            }
            Hook::K3sAll => {
                if self.svc.k3sdev.installed().is_none() {
                    // the program may be gone already (an earlier keep-data removal): the data folder is the rest
                    let l = crate::k3sdev::Layout::new(&data);
                    return remove_path(&l.root).map(|_| "the cluster folder is removed".into());
                }
                self.svc.k3sdev.uninstall(crate::k3sdev::NODE_NAME, &self.user).map(|_| "the cluster is removed".into())
            }
            Hook::OllamaServer => {
                let _ = self.svc.ollama.stop();
                let n = self.remove_providers(&|p| p.id == "ollama-local");
                Ok(format!("Ollama stopped; {n} Models entr{} removed", if n == 1 { "y" } else { "ies" }))
            }
            Hook::LemonadeProvider => {
                let port = format!(":{}", crate::lemonade::DEFAULT_PORT);
                let n = self.remove_providers(&|p| p.kind == crate::providers::Kind::Lemonade && p.base_url.as_deref().is_some_and(|u| (u.contains("127.0.0.1") || u.contains("localhost")) && u.contains(&port)));
                Ok(format!("{n} Models entr{} removed", if n == 1 { "y" } else { "ies" }))
            }
            Hook::VoicePack => crate::voicepack::remove(&self.svc, &|m: &str| crate::voicepack::delete_local_model(m)),
            Hook::KokoroModel => {
                crate::voicepack::delete_local_model(crate::voicepack::KOKORO_MODEL)?;
                self.svc
                    .settings
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .update(|s| {
                        let ours = matches!(&s.voice.tts, Some(crate::voice::SpeechEngine::Http { model, url, .. }) if model == crate::voicepack::KOKORO_MODEL && url.contains("/api/v1/audio/"));
                        if ours {
                            s.voice.tts = None;
                        }
                    })
                    .map_err(|e| e.to_string())?;
                Ok("the Kokoro model is deleted".into())
            }
            Hook::FwPack(item) => {
                let def = item.fw_def().ok_or("not a firmware pack")?;
                if crate::fwpack::installed(&data, def).is_none() {
                    return say_none("the pack");
                }
                crate::fwpack::remove(&data, def).map(|r| r.lines.join(" "))
            }
            Hook::FwHardware => {
                if crate::fwextra::installed(&data).is_none() {
                    let _ = crate::fwextra::delete_leftovers(&data);
                    return say_none("the pack");
                }
                crate::fwextra::remove(&data).map(|r| r.lines.join(" "))
            }
            Hook::DriverPack(id) => {
                let (fs, _) = iyakki_drivers::apply::Fs::for_data(&data);
                if iyakki_drivers::apply::read_record(&fs, id).is_none() {
                    return say_none(id);
                }
                iyakki_drivers::apply::remove_pack(&fs, id).map(|r| r.lines.join(" "))
            }
            Hook::Sandbox(name) => {
                let exe = std::env::current_exe().map_err(|e| format!("cannot find this program: {e}"))?;
                let la = crate::linuxapps::LinuxApps::new(&data, &self.user, &exe, Arc::new(crate::wsexec::System));
                if la.get(name).is_err() {
                    return say_none(name);
                }
                la.delete(name, name).map(|b| format!("the sandbox {name} is deleted ({} MB)", b >> 20))
            }
            Hook::ModelHosts(ids) => {
                let mut n = 0;
                for id in ids {
                    if crate::modelhost::remove_from_models(&self.svc, id).is_ok() {
                        n += 1;
                    }
                }
                Ok(format!("{n} model host(s) removed"))
            }
            Hook::GiteaRecord => {
                let mut n = 0;
                for r in crate::platform::installed::load(&data, &self.user) {
                    if r.service == "gitea" && !r.connected && r.namespace == gitea::NAMESPACE {
                        crate::platform::installed::remove(&data, &self.user, &r.id)?;
                        n += 1;
                    }
                }
                Ok(format!("{n} record(s) removed"))
            }
            Hook::GiteaConnection => {
                let Some((id, _)) = gitea::record_for(&data, &self.user).map(|r| (r.id, ())) else { return say_none("the connection") };
                let env = crate::ssh::Env::for_services(&self.svc, &self.user);
                let conns = crate::gitconn::Conns::new(&env.dir, Arc::new(crate::gitkeys::ServicesStore::new(&self.svc, &self.user)), Arc::new(|_, _| {}));
                let mut n = 0;
                for c in conns.list().into_iter().filter(|c| c.cluster == id) {
                    if conns.remove(&c.id).is_ok() {
                        n += 1;
                    }
                }
                Ok(format!("{n} Git connection(s) removed"))
            }
            Hook::NvidiaCdi => {
                let dir = Path::new(crate::gpucdi::SPEC_DIR);
                for k in [crate::gpucdi::NVIDIA_KIND, crate::gpucdi::PLUGIN_KIND] {
                    crate::gpucdi::remove_spec(dir, k);
                }
                Ok("the CDI specs are removed".into())
            }
            Hook::OllamaProvider => {
                let n = self.remove_providers(&|p| p.id == "ollama-local");
                Ok(format!("{n} Models entr{} removed", if n == 1 { "y" } else { "ies" }))
            }
            Hook::OllamaUnpatch => {
                let ns = crate::gpu_workloads::NAMESPACE;
                let have = self.svc.k3sdev.kubectl(&["-n", ns, "get", "deployment", ollama_cluster::NAME, "-o", "name"]);
                if have.code != Some(0) {
                    return say_none("the Ollama Deployment");
                }
                let patch = crate::gpu_workloads::ollama_unpatch();
                self.kubectl(&["-n", ns, "patch", "deployment", ollama_cluster::NAME, "--type=strategic", "-p", &patch]).map(|_| "the GPU settings are taken out of Ollama".into())
            }
            Hook::LemonadeUnpatch => {
                let ns = crate::gpu_workloads::NAMESPACE;
                let have = self.svc.k3sdev.kubectl(&["-n", ns, "get", "deployment", "lemonade", "-o", "name"]);
                if have.code != Some(0) {
                    return say_none("the Lemonade Deployment");
                }
                let patch = crate::gpu_workloads::lemonade_unpatch();
                self.kubectl(&["-n", ns, "patch", "deployment", "lemonade", "--type=strategic", "-p", &patch]).map(|_| "the GPU settings are taken out of Lemonade".into())
            }
        }
    }
}

/// Remove one item for one person on this machine: the whole thing the page and the chat tool run.
pub fn uninstall_real(svc: &Arc<crate::services::Services>, user: &str, item: Item, o: Opts, progress: Option<Arc<Mutex<String>>>) -> Result<String, String> {
    let p = presence(svc, item);
    if let Some(why) = not_ours(&p) {
        return Err(format!("{}: {why}", item.title()));
    }
    if !removable(svc, item) {
        return Err(format!("{} is not installed, so there is nothing to remove.", item.title()));
    }
    let running = svc.jobs.visible(user, WORKSPACE).iter().any(|j| item_of(j) == Some(item) && j.state == crate::jobs::State::Running);
    if running {
        return Err(format!("{} is being installed: wait until that is done.", item.title()));
    }
    let ctx = real_ctx(svc, user, item);
    let mut env = RealEnv::new(svc.clone(), user);
    if let Some(p) = progress {
        env.progress = p;
    }
    let svc2 = svc.clone();
    let installed = move |i: Item| is_installed(&svc2, i);
    uninstall(item, o, &installed, &ctx, &mut env)
}

/// Names and states only, for the status tool and the page: `(id, title, state word)`.
pub fn status_rows(svc: &crate::services::Services) -> Vec<(&'static str, &'static str, String)> {
    Item::ALL.into_iter().map(|i| (i.id(), i.title(), presence(svc, i).word())).collect()
}

#[cfg(test)]
#[path = "quickinstall_uninstall_tests.rs"]
mod tests;
