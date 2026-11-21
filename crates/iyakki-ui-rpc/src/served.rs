//! The accessors of `ServicesHandle` that took objects of the UI (an audit callback, a login check, a gate, a file rule) and the traits that no
//! generator reaches (F-SPLIT-03 part 3, D-SPLIT-03-SERVED in docs/development/ALPHA-7-DECISIONS.md).
//!
//! The rule of D-SPLIT-03-HANDLE (b) holds for all of them: what the UI would have passed is the core's own (`ServicesRoot::audit`,
//! [`ServedParts`]); the wire carries data arguments only; the answer is a remote object or data. Nothing here makes up a success: a core that
//! lacks the part (no accounts for a login check, no gate) refuses by name.
//!
//! This file holds the common pieces and the accessors whose traits are generated (`store_facets`, `platform_real`) or small (`remote_kit`); the
//! other served forms are in `served_*.rs`. `router_panel` and `sftp_server` are served with the core's `CoreEnv` (D-SPLIT-03-ENV, generator kind `env`).

use std::sync::Arc;

use iyakki_agent_api::{AccountsApi, RemoteKit, StoreCatalog, StoreComponents, StoreDriverSource, StoreDrivers, StoreHub, StoreUpdates};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

use crate::gen_remote::{RemoteStoreCatalog, RemoteStoreComponents, RemoteStoreDriverSource, RemoteStoreHub, RemoteStoreUpdates};
use crate::{de, ser, Lost, Obj, Registry, RemoteServicesHandle, Rpc, ServicesRoot};

pub(crate) const NOT_SERVED: &str = "not served over the UI socket";

/// A hand-written object the core hands out (`Obj::Served`): the trait's name and the call by method name.
pub trait Served: Send + Sync {
    fn iface(&self) -> &'static str;
    fn serve(&self, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String>;
    /// The bytes an upload object holds (once; it is emptied). Every other object has none.
    fn take_bytes(&self) -> Option<Vec<u8>> {
        None
    }
}

/// What only the composition root can supply to the served forms: the core's own objects that the UI used to pass in.
#[derive(Clone, Default)]
pub struct ServedParts {
    /// The machine's accounts: the login check and the account lookup of the SFTP server, and the policy a file rule is judged by.
    pub accounts: Option<Arc<dyn AccountsApi>>,
    /// The file rules of the machine (what an account may read and write).
    pub file_scope: Option<Arc<dyn iyakki_core_api::FileScope>>,
    /// The machine's host (power, network status, programs) a terminal session sees.
    pub host: Option<Arc<dyn iyakki_core_api::HostApi>>,
    /// The terminal of the core contract (sessions and whole-session pty), built over the machine's accounts.
    pub core_terminal: Option<Arc<dyn iyakki_core_api::Terminal>>,
    /// The core's own handle: the sound facets (mixer, output selector, cues, sound route) are served from it.
    pub core: Option<Arc<dyn iyakki_core_api::Core>>,
    /// The vault of one account as the git keys' secret store (the UI never reads from it, see `served_git.rs`).
    pub secret_store: Option<SecretStoreOf>,
}

/// The secret store of the named account, made in the core.
pub type SecretStoreOf = Arc<dyn Fn(&str) -> Arc<dyn iyakki_shared::gitkeys::SecretStore> + Send + Sync>;

impl ServedParts {
    pub(crate) fn need<T: Clone>(part: &Option<T>, what: &str) -> Result<T, String> {
        part.clone().ok_or_else(|| format!("{NOT_SERVED}: this core has no {what}"))
    }
}

/// One call on a served object, the quiet value when the core cannot answer.
pub(crate) fn call<T: DeserializeOwned + Lost>(rpc: &Rpc, iface: &str, obj: u64, method: &str, args: Value) -> T {
    rpc.call::<T>(iface, obj, method, args).unwrap_or_else(|e| T::lost(&e))
}

/// The ids of the objects a call handed out (a lost call hands out none; an object that does not exist answers quietly).
fn ids(rpc: &Rpc, method: &str, args: Value) -> Vec<u64> {
    rpc.call::<Vec<u64>>("ServicesHandle", 0, method, args).unwrap_or_default()
}

// ---------------------------------------------------------------------------------------------------------------- the core's end

/// The calls of the hand-written accessors, by method name. Any other name is refused (`gen_services.rs` falls through to here).
pub(crate) fn serve_manual(root: &ServicesRoot, reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
    let t: &dyn iyakki_agent_api::ServicesHandle = root.svc.as_ref();
    match method {
        "store_facets" => {
            let f = t.store_facets();
            let hub: Arc<dyn StoreHub> = f.clone();
            let updates: Arc<dyn StoreUpdates> = f.clone();
            let components: Arc<dyn StoreComponents> = f.clone();
            let drivers: Arc<dyn StoreDriverSource> = f.clone();
            let catalog: Arc<dyn StoreCatalog> = f;
            ser(&[reg.add(Obj::StoreHub(hub)), reg.add(Obj::StoreUpdates(updates)), reg.add(Obj::StoreComponents(components)), reg.add(Obj::StoreDriverSource(drivers)), reg.add(Obj::StoreCatalog(catalog))])
        }
        "platform_real" => {
            let (user,): (String,) = de(a)?;
            ser(&reg.add(Obj::PlatformApi(t.platform_real(&user, root.audit.clone()))))
        }
        "policy_of" => {
            // the person's own policy as the core reads it: what a UI-side store (the stories) holds in place of a policy the UI would name
            let (user,): (String,) = de(a)?;
            ser(&reg.policy_of(&user)?)
        }
        "remote_kit" => {
            let (user, downloads): (String, String) = de(a)?;
            let scope = ServedParts::need(&root.parts.file_scope, "file rules")?;
            let accounts = ServedParts::need(&root.parts.accounts, "accounts")?;
            let policy = accounts.get_public(&user).ok_or_else(|| format!("{NOT_SERVED}: no account {user}"))?.policy();
            let data = t.dir().to_path_buf();
            let local = local_path_rule(scope, policy, user.clone(), data.clone());
            ser(&reg.add(Obj::Served(Arc::new(RemoteKitObj(t.remote_kit(&user, &data, &downloads, local, root.audit.clone()))))))
        }
        other => crate::served_hosts::serve_hosts(root, reg, other, a.clone()).or_else(|| crate::served_person::serve_person(root, reg, other, a.clone())).or_else(|| crate::served_apps::serve_apps(root, reg, other, a.clone())).or_else(|| crate::served_flows::serve_flows(root, reg, other, a.clone())).or_else(|| crate::served_git::serve_git(root, reg, other, a)).unwrap_or_else(|| Err(format!("ServicesHandle has no call {other}"))),
    }
}

/// The rule a remote-files manager asks before it reads or writes a path of this computer: the same as the tile's own, over the core's file rules.
pub(crate) fn local_path_rule(scope: Arc<dyn iyakki_core_api::FileScope>, policy: iyakki_sec::policy::Policy, user: String, data: std::path::PathBuf) -> Arc<dyn Fn(&str, bool) -> Result<std::path::PathBuf, String> + Send + Sync> {
    Arc::new(move |text: &str, write: bool| {
        let roots = scope.roots(&policy, &user, &data);
        let expanded = std::path::PathBuf::from(roots.expand(text));
        let path = if expanded.is_absolute() { expanded } else { roots.home.join(expanded) };
        if write {
            let name = path.file_name().ok_or("that path has no file name")?.to_owned();
            let parent = std::fs::canonicalize(path.parent().ok_or("that path has no folder")?).map_err(|_| "that folder does not exist or is not allowed".to_string())?;
            let real = parent.join(name);
            if roots.contains(&parent) && roots.write_ok(&policy, &user, &real) { Ok(real) } else { Err("outside the folders your account may write to".to_string()) }
        } else {
            let real = std::fs::canonicalize(&path).map_err(|_| "not found, or not allowed".to_string())?;
            if roots.read_ok(&policy, &user, &real) { Ok(real) } else { Err("outside the folders your account may read".to_string()) }
        }
    })
}

// ---------------------------------------------------------------------------------------------------------------- the UI's end

impl RemoteServicesHandle {
    /// The store facets: five remote objects of the generated traits behind one handle.
    pub(crate) fn m_store_facets(&self) -> Arc<dyn iyakki_agent_api::StoreFacets> {
        let v = ids(&self.rpc, "store_facets", json!([]));
        let id = |i: usize| v.get(i).copied().unwrap_or(u64::MAX);
        Arc::new(RemoteStoreFacets {
            hub: RemoteStoreHub::new(self.rpc.clone(), id(0)),
            updates: RemoteStoreUpdates::new(self.rpc.clone(), id(1)),
            components: RemoteStoreComponents::new(self.rpc.clone(), id(2)),
            drivers: RemoteStoreDriverSource::new(self.rpc.clone(), id(3)),
            catalog: RemoteStoreCatalog::new(self.rpc.clone(), id(4)),
        })
    }

    /// The platform services (deploy, upgrade, back up, remove, connect): the core's own runner, cluster settings and audit log; every call that
    /// runs something takes the core's own approval path (`approved` is a flag the person's card set, the core still checks the plan it made).
    pub(crate) fn m_platform_real(&self, user: &str, _audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn iyakki_agent_api::PlatformApi> {
        let id = self.rpc.call::<u64>("ServicesHandle", 0, "platform_real", json!([user])).unwrap_or(u64::MAX);
        Arc::new(crate::gen_remote::RemotePlatformApi::new(self.rpc.clone(), id))
    }

    /// The same as [`Self::m_platform_real`]: the command runner, the cluster environment and the scratch folder a caller names are the ones of a
    /// test seam, and the UI is never given a way to run a command through the core (D-SPLIT-03-SERVED).
    pub(crate) fn m_platform_with(&self, user: &str, _run: Arc<dyn iyakki_shared::wsexec::Runner>, _env: iyakki_shared::devcluster::Env, audit: Arc<dyn Fn(&str, &str) + Send + Sync>, _tmp: std::path::PathBuf) -> Arc<dyn iyakki_agent_api::PlatformApi> {
        self.m_platform_real(user, audit)
    }

    /// The remote files manager: the core's file rules and audit log stand in for the UI's closures.
    pub(crate) fn m_remote_kit(&self, user: &str, _data: &std::path::Path, downloads: &str, _local: Arc<dyn Fn(&str, bool) -> Result<std::path::PathBuf, String> + Send + Sync>, _audit: Arc<dyn Fn(&str, &str) + Send + Sync>) -> Arc<dyn RemoteKit> {
        let id = self.rpc.call::<u64>("ServicesHandle", 0, "remote_kit", json!([user, downloads])).unwrap_or(u64::MAX);
        Arc::new(RemoteRemoteKit { rpc: self.rpc.clone(), obj: id })
    }
}

/// The five store facets as one object.
pub(crate) struct RemoteStoreFacets {
    hub: RemoteStoreHub,
    updates: RemoteStoreUpdates,
    components: RemoteStoreComponents,
    drivers: RemoteStoreDriverSource,
    catalog: RemoteStoreCatalog,
}

use iyakki_agent_api::{ComponentReview, WizardLine};
use iyakki_sec::policy::Policy;
use iyakki_shared::store_compapply::{ApplyResult, StoreLine};
use iyakki_shared::store_private::StatusInfo;
use iyakki_shared::store_tilehub::TileEntry;
use iyakki_shared::store_updatecheck::{CheckSettings, Report};
use std::path::{Path, PathBuf};

impl StoreHub for RemoteStoreFacets {
    fn tiles(&self, policy: &Policy, user: &str) -> Vec<TileEntry> {
        self.hub.tiles(policy, user)
    }
    fn preview_tile(&self, user: &str, dir: &Path) -> Result<TileEntry, String> {
        self.hub.preview_tile(user, dir)
    }
    fn add_dev_tile(&self, policy: &Policy, user: &str, dir: &Path) -> Result<TileEntry, String> {
        self.hub.add_dev_tile(policy, user, dir)
    }
    fn remove_dev_tile(&self, user: &str, name: &str) -> Result<(), String> {
        self.hub.remove_dev_tile(user, name)
    }
    fn wasm_tiles_enabled(&self) -> bool {
        self.hub.wasm_tiles_enabled()
    }
}
impl StoreUpdates for RemoteStoreFacets {
    fn update_report(&self) -> Option<Report> {
        self.updates.update_report()
    }
    fn update_check_due(&self, settings: &CheckSettings) -> bool {
        self.updates.update_check_due(settings)
    }
    fn update_check_run(&self, policy: &Policy, user: &str, settings: &CheckSettings, running: &str, variant: &str) -> Report {
        self.updates.update_check_run(policy, user, settings, running, variant)
    }
    fn update_announce_stores(&self) -> Vec<(String, String)> {
        self.updates.update_announce_stores()
    }
    fn pending_wizards(&self, user: &str) -> Vec<WizardLine> {
        self.updates.pending_wizards(user)
    }
}
impl StoreComponents for RemoteStoreFacets {
    fn components_channel(&self) -> String {
        self.components.components_channel()
    }
    fn components_set_channel(&self, channel: &str) -> Result<(), String> {
        self.components.components_set_channel(channel)
    }
    fn components_check_all(&self) -> Vec<StoreLine> {
        self.components.components_check_all()
    }
    fn components_check_lines(&self) -> Vec<String> {
        self.components.components_check_lines()
    }
    fn components_last_result(&self) -> Option<ApplyResult> {
        self.components.components_last_result()
    }
    fn components_review(&self, channel: &str) -> Result<ComponentReview, String> {
        self.components.components_review(channel)
    }
    fn components_apply(&self, review: &ComponentReview, user: &str) -> Result<String, String> {
        self.components.components_apply(review, user)
    }
}
impl StoreDriverSource for RemoteStoreFacets {
    fn drivers(&self) -> Arc<dyn StoreDrivers> {
        self.drivers.drivers()
    }
}
impl StoreCatalog for RemoteStoreFacets {
    fn private_store_status(&self, user: &str) -> StatusInfo {
        self.catalog.private_store_status(user)
    }
    fn permission_words(&self, permission: &str, user: &str) -> Option<String> {
        self.catalog.permission_words(permission, user)
    }
    fn stick_stores(&self) -> Vec<PathBuf> {
        self.catalog.stick_stores()
    }
    fn scaffold_project(&self, kind: &str, name: &str, root: &Path, dir: &str) -> Result<String, String> {
        self.catalog.scaffold_project(kind, name, root, dir)
    }
    fn project_version(&self, dir: &Path) -> String {
        self.catalog.project_version(dir)
    }
}

// ---------------------------------------------------------------------------------------------------------------- remote files

/// The remote files types, for the callers that name them through this crate.
pub use iyakki_tile_api::remote as remote_files;
use iyakki_tile_api::remote::{Conn, Job, JobResult, RemoteSource, TransferInfo, TransferReq};

/// The remote files manager as the UI sees it.
pub struct RemoteRemoteKit {
    pub(crate) rpc: Rpc,
    pub(crate) obj: u64,
}
impl Drop for RemoteRemoteKit {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}
impl RemoteRemoteKit {
    fn c<T: DeserializeOwned + Lost>(&self, m: &str, args: Value) -> T {
        call(&self.rpc, "RemoteKit", self.obj, m, args)
    }
}
impl RemoteSource for RemoteRemoteKit {
    fn connections(&self) -> Vec<Conn> {
        self.c("connections", json!([]))
    }
    fn save(&self, conn: &Conn, password: Option<&str>, replacing: Option<&str>) -> Result<(), String> {
        self.c("save", json!([conn, password, replacing]))
    }
    fn remove(&self, name: &str) -> Result<(), String> {
        self.c("remove", json!([name]))
    }
    fn key_names(&self) -> Vec<String> {
        self.c("key_names", json!([]))
    }
    fn submit(&self, job: Job) -> u64 {
        self.c("submit", json!([job]))
    }
    fn take(&self, ticket: u64) -> Option<JobResult> {
        match self.rpc.call::<Option<JobResult>>("RemoteKit", self.obj, "take", json!([ticket])) {
            Ok(r) => r,
            // a job whose answer cannot be fetched has failed: the tile stops waiting for it
            Err(e) => Some(JobResult::Failed(e)),
        }
    }
    fn trust(&self, name: &str, fingerprint: &str) -> Result<(), String> {
        self.c("trust", json!([name, fingerprint]))
    }
    fn forget_host(&self, name: &str) -> Result<(), String> {
        self.c("forget_host", json!([name]))
    }
    fn close(&self, session: u64) {
        self.c::<bool>("close", json!([session]));
    }
    fn start_transfer(&self, req: &TransferReq) -> Result<u64, String> {
        self.c("start_transfer", json!([req]))
    }
    fn transfers(&self) -> Vec<TransferInfo> {
        self.c("transfers", json!([]))
    }
    fn cancel_transfer(&self, id: u64) {
        self.c::<bool>("cancel_transfer", json!([id]));
    }
    fn retry_transfer(&self, id: u64) -> Result<u64, String> {
        self.c("retry_transfer", json!([id]))
    }
    fn clear_finished(&self) {
        self.c::<bool>("clear_finished", json!([]));
    }
    fn download_dir(&self) -> String {
        self.c("download_dir", json!([]))
    }
}
/// The assistant's `remote_*` tools are built by the core's chat runs; the UI's tool chain is not wrapped.
impl RemoteKit for RemoteRemoteKit {}

/// The core's end of [`RemoteKit`].
struct RemoteKitObj(Arc<dyn RemoteKit>);
impl Served for RemoteKitObj {
    fn iface(&self) -> &'static str {
        "RemoteKit"
    }
    fn serve(&self, _reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        let t = &self.0;
        match method {
            "connections" => ser(&t.connections()),
            "save" => {
                let (c, p, r): (Conn, Option<String>, Option<String>) = de(a)?;
                ser(&t.save(&c, p.as_deref(), r.as_deref()))
            }
            "remove" => {
                let (n,): (String,) = de(a)?;
                ser(&t.remove(&n))
            }
            "key_names" => ser(&t.key_names()),
            "submit" => {
                let (j,): (Job,) = de(a)?;
                ser(&t.submit(j))
            }
            "take" => {
                let (id,): (u64,) = de(a)?;
                ser(&t.take(id))
            }
            "trust" => {
                let (n, f): (String, String) = de(a)?;
                ser(&t.trust(&n, &f))
            }
            "forget_host" => {
                let (n,): (String,) = de(a)?;
                ser(&t.forget_host(&n))
            }
            "close" => {
                let (id,): (u64,) = de(a)?;
                {
                    t.close(id);
                    ser(&true)
                }
            }
            "start_transfer" => {
                let (r,): (TransferReq,) = de(a)?;
                ser(&t.start_transfer(&r))
            }
            "transfers" => ser(&t.transfers()),
            "cancel_transfer" => {
                let (id,): (u64,) = de(a)?;
                {
                    t.cancel_transfer(id);
                    ser(&true)
                }
            }
            "retry_transfer" => {
                let (id,): (u64,) = de(a)?;
                ser(&t.retry_transfer(id))
            }
            "clear_finished" => {
                t.clear_finished();
                ser(&true)
            }
            "download_dir" => ser(&t.download_dir()),
            other => Err(format!("RemoteKit has no call {other}")),
        }
    }
}
