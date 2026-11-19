//! The real implementation of the store facets of `iyakki_agent_api` (`StoreHub`, `StoreUpdates`, `StoreComponents`): each call is one
//! function of `crate::store` or `crate::componentstools`, the services stay inside this handle (F-SPLIT-03, UI-BOUNDARY.md).

use std::path::Path;
use std::sync::Arc;

use iyakki_agent_api::{ComponentReview, StoreCatalog, StoreComponents, StoreDriverSource, StoreDrivers, StoreHub, StoreUpdates, WizardLine};
use iyakki_sec::policy::Policy;
use iyakki_shared::store_compapply::{ApplyResult, StoreLine};
use iyakki_shared::store_tilehub::TileEntry;
use iyakki_shared::store_updatecheck::{CheckSettings, Report};

use crate::componentstools::{Backend, RealBackend};
use crate::services::Services;
use crate::store::{compapply, tilehub, updatecheck, wizardhook, Store};

pub struct StoreFacetsImpl {
    svc: Arc<Services>,
}

impl Services {
    /// The store facets over these services: what the UI holds instead of the services themselves (`AppCx::storeapi`).
    pub fn store_facets(self: &Arc<Self>) -> Arc<dyn iyakki_agent_api::StoreFacets> {
        Arc::new(StoreFacetsImpl::new(self.clone()))
    }
}

impl StoreFacetsImpl {
    pub fn new(svc: Arc<Services>) -> StoreFacetsImpl {
        StoreFacetsImpl { svc }
    }
    fn backend(&self, user: Option<&str>) -> RealBackend {
        RealBackend { svc: self.svc.clone(), user: user.map(str::to_string) }
    }
}

impl StoreHub for StoreFacetsImpl {
    fn tiles(&self, policy: &Policy, user: &str) -> Vec<TileEntry> {
        tilehub::installed(&self.svc, policy, user)
    }
    fn preview_tile(&self, user: &str, dir: &Path) -> Result<TileEntry, String> {
        tilehub::preview_entry(user, dir)
    }
    fn add_dev_tile(&self, policy: &Policy, user: &str, dir: &Path) -> Result<TileEntry, String> {
        tilehub::add_dev(&self.svc, policy, user, dir)
    }
    fn remove_dev_tile(&self, user: &str, name: &str) -> Result<(), String> {
        tilehub::remove_dev(&self.svc, user, name)
    }
    fn wasm_tiles_enabled(&self) -> bool {
        tilehub::wasm_enabled(&self.svc)
    }
}

impl StoreUpdates for StoreFacetsImpl {
    fn update_report(&self) -> Option<Report> {
        Report::load(self.svc.dir())
    }
    fn update_check_due(&self, settings: &CheckSettings) -> bool {
        let store = Store::for_services(&self.svc);
        let last = Report::load(self.svc.dir()).map_or(0, |r| r.checked_at);
        updatecheck::due(settings, last, store.now(), crate::meteredlink::metered_now(), &store.licenses().device_id()) == updatecheck::Due::Now
    }
    fn update_check_run(&self, policy: &Policy, user: &str, settings: &CheckSettings, running: &str, variant: &str) -> Report {
        let store = Store::for_services(&self.svc);
        let fetch = crate::store::catalog::HttpFetch(self.svc.quick_upstream());
        updatecheck::run_and_keep(&store, &self.svc, policy, user, &fetch, settings, running, variant)
    }
    fn update_announce_stores(&self) -> Vec<(String, String)> {
        let store = Store::for_services(&self.svc);
        store.config().sources.into_iter().filter(|s| s.enabled && !s.pin.is_empty() && s.kind == crate::store::catalog::SourceKind::Url).map(|s| (s.name.clone(), crate::store::sysannounce::url_for_catalog(&s.location))).collect()
    }
    fn pending_wizards(&self, user: &str) -> Vec<WizardLine> {
        let pending = wizardhook::pending(self.svc.dir(), user);
        if pending.is_empty() {
            return vec![];
        }
        let specs = wizardhook::installed(&Store::for_services(&self.svc), user);
        pending
            .into_iter()
            .map(|p| {
                let outline = specs.iter().find(|(a, _, w)| *a == p.app && w.id == p.wizard).map(|(_, _, w)| wizardhook::outline(w)).unwrap_or_default();
                WizardLine { pending: p, outline }
            })
            .collect()
    }
}

impl StoreComponents for StoreFacetsImpl {
    fn components_channel(&self) -> String {
        compapply::channel(self.svc.dir())
    }
    fn components_set_channel(&self, channel: &str) -> Result<(), String> {
        compapply::set_channel(self.svc.dir(), channel)
    }
    fn components_check_all(&self) -> Vec<StoreLine> {
        self.backend(None).check_all(&compapply::channel(self.svc.dir()))
    }
    fn components_check_lines(&self) -> Vec<String> {
        compapply::load_check(self.svc.dir()).iter().map(compapply::check_sentence).collect()
    }
    fn components_last_result(&self) -> Option<ApplyResult> {
        compapply::load_result(self.svc.dir())
    }
    fn components_review(&self, channel: &str) -> Result<ComponentReview, String> {
        let be = self.backend(None);
        let stores = be.stores();
        if stores.is_empty() {
            return Err("No store with a checked key is added yet. Add your private store in the STORE (Settings) first.".into());
        }
        let mut said = vec![];
        for s in stores {
            match be.review_for(&s, channel) {
                Ok((_plan, review)) => return Ok(ComponentReview { store: s.name, channel: channel.to_string(), review }),
                Err(e) => said.push(format!("{}: {e}", s.name)),
            }
        }
        Err(said.join(" "))
    }
    fn components_apply(&self, review: &ComponentReview, user: &str) -> Result<String, String> {
        let be = self.backend(Some(user));
        let store = be.stores().into_iter().find(|s| s.name == review.store).ok_or_else(|| format!("{} is no longer a pinned store", review.store))?;
        // pressing y is the approval of exactly this review
        let approval = compapply::Approval::given(&review.review, user);
        be.apply_approved(&store, &review.channel, &approval).map(|a| a.text())
    }
}
impl StoreCatalog for StoreFacetsImpl {
    fn private_store_status(&self, user: &str) -> iyakki_shared::store_private::StatusInfo {
        crate::store::privatestore::status_info(self.svc.dir(), user, &Store::for_services(&self.svc))
    }
    fn permission_words(&self, permission: &str, user: &str) -> Option<String> {
        crate::store::manifest::Permission::parse(permission).ok().map(|p| p.words(user))
    }
    fn stick_stores(&self) -> Vec<std::path::PathBuf> {
        crate::store::catalog::discover_sticks(&crate::store::catalog::stick_mounts().iter().map(|p| p.as_path()).collect::<Vec<_>>())
    }
    fn scaffold_project(&self, kind: &str, name: &str, root: &Path, dir: &str) -> Result<String, String> {
        crate::store::author::scaffold(kind, name, root, dir)
    }
    fn project_version(&self, dir: &Path) -> String {
        crate::store::vet::read_project(dir).and_then(|v| v["version"].as_str().map(str::to_string)).unwrap_or_default()
    }
}

impl StoreDriverSource for StoreFacetsImpl {
    fn drivers(&self) -> Arc<dyn StoreDrivers> {
        Arc::new(crate::store::drivers::Drivers::for_services(&self.svc))
    }
}

#[cfg(test)]
#[path = "storefacets_tests.rs"]
mod tests;
