//! Moved out of `iyakki_agent::quickinstall_uninstall` (F-SPLIT-03, bucket b0); the agent re-exports it.
use std::path::PathBuf;

use crate::quickinstall::{Item, Presence};

const REGISTRY_NS: &str = crate::regsetup::IN_NS;


/// A fixed set of namespaces IYAKKI creates itself. A Kubernetes target outside them is refused.
pub const OWN_NAMESPACES: [&str; 6] = ["iyakki-data", "iyakki-monitoring", "iyakki-ai", "iyakki-matrix", "iyakki-gitea", "iyakki-objects"];

/// Namespaces only one item uses: removed with the data of that item, never otherwise.
pub const EXCLUSIVE_NAMESPACES: [&str; 4] = ["iyakki-monitoring", "iyakki-matrix", "iyakki-gitea", REGISTRY_NS];

/// The kinds a plan may delete, by name.
pub const KINDS: [&str; 10] = ["deployment", "service", "configmap", "secret", "persistentvolumeclaim", "serviceaccount", "daemonset", "clusterrole", "clusterrolebinding", "namespace"];

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
    /// The toolkit folder `<data>/datasci` (Python, packages, state; never the sandbox or your data).
    DataScience,
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

impl Plan {
    pub fn active(&self, delete_data: bool) -> impl Iterator<Item = &Step> {
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

pub fn names(v: &[Item]) -> String {
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

#[derive(Clone, Copy, Debug, Default)]
pub struct Opts {
    pub delete_data: bool,
    pub with_dependants: bool,
}
