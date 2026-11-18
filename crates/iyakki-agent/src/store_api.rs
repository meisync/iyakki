//! The agent's side of `iyakki_agent_api::StoreApi`: the store and the services it works over behind one handle (F-SPLIT-03, option B).

use std::path::Path;
use std::sync::Arc;

use iyakki_agent_api::StoreApi;
use iyakki_core_api::appcaps::Approval;
use iyakki_sec::policy::Policy;
use iyakki_shared::skills::Scope;
use iyakki_shared::store_catalog::{Config, Fetch, Loaded, SourceCfg, SourceKind};
use iyakki_shared::store_core::{Item, Probe, Refresh};
use iyakki_shared::store_install::{Actor, Confirm, InstallError, Record, Review};
use iyakki_shared::store_installed::Entry;
use iyakki_shared::store_license::Token;
use iyakki_shared::store_lifecycle::{Ctx, Life, LifeError, RemoveOpts, RemovePlan, Report, UpdatePlan};
use iyakki_shared::store_author::Published;
use iyakki_shared::store_keys::{KeyInfo, SigningKey};
use iyakki_shared::store_privatepub::{Deployed, Offer};
use iyakki_shared::store_publishflow::{Bump, Card, Preflight, Stop, StoreChoice};
use iyakki_shared::store_tile::TestReport;
use iyakki_shared::store_revocation::Revocations;
use iyakki_shared::store_trust::Roots;

use crate::services::Services;
use crate::store::Store;

/// The store of this install and its services.
pub struct StoreHandle {
    pub store: Store,
    pub svc: Arc<Services>,
}

impl StoreHandle {
    pub fn new(svc: &Arc<Services>) -> Arc<StoreHandle> {
        Arc::new(StoreHandle { store: Store::for_services(svc), svc: svc.clone() })
    }
}

impl StoreApi for StoreHandle {
    fn approval(&self, installation: &str) -> Option<Approval> {
        self.store.approval(installation)
    }
    fn installed_version(&self, kind: &str, name: &str, scope: &Scope) -> Option<(String, bool)> {
        self.store.installed_version(&self.svc, kind, name, scope)
    }
    fn is_disabled(&self, kind: &str, name: &str, scope: &Scope) -> bool {
        self.store.is_disabled(kind, name, scope)
    }
    fn install(&self, actor: &Actor, review: &Review, confirm: Confirm) -> Result<Record, InstallError> {
        self.store.install(&self.svc, actor, review, confirm)
    }
    fn remove(&self, actor: &Actor, kind: &str, name: &str, scope: &Scope) -> Result<(), String> {
        self.store.remove(&self.svc, actor, kind, name, scope)
    }
    fn disable(&self, actor: &Actor, kind: &str, name: &str, scope: &Scope) -> Result<(), String> {
        self.store.disable(&self.svc, actor, kind, name, scope)
    }
    fn enable(&self, actor: &Actor, kind: &str, name: &str, scope: &Scope) -> Result<Record, InstallError> {
        self.store.enable(&self.svc, actor, kind, name, scope)
    }
    fn enforce_revocations(&self) -> Vec<String> {
        self.store.enforce_revocations(&self.svc)
    }
    fn add_license(&self, actor: &Actor, text: &str, signer_of: &dyn Fn(&str) -> Vec<String>) -> Result<Token, String> {
        self.store.add_license(actor, text, signer_of)
    }
    fn installed(&self, policy: &Policy, user: &str, items: &[Item]) -> Vec<Entry> {
        self.store.installed(&self.svc, policy, user, items)
    }
    fn remove_installed(&self, policy: &Policy, user: &str, e: &Entry) -> Result<String, String> {
        self.store.remove_installed(&self.svc, policy, user, e)
    }
    fn toggle_installed(&self, policy: &Policy, user: &str, e: &Entry) -> Result<String, String> {
        self.store.toggle_installed(&self.svc, policy, user, e)
    }
    fn pin(&self, key: &str, version: &str) -> Result<(), String> {
        self.store.pin(key, version)
    }
    fn unpin(&self, key: &str) -> Result<(), String> {
        self.store.unpin(key)
    }
    fn lifecycle(&self, policy: &Policy, user: &str, loaded: &[Loaded], ctx: &Ctx) -> Vec<Life> {
        self.store.lifecycle(&self.svc, policy, user, loaded, ctx)
    }
    fn find_life<'a>(&self, lives: &'a [Life], id: &str) -> Result<&'a Life, String> {
        Store::find_life(lives, id)
    }
    fn check_cli_latest(&self) -> Vec<(String, Result<String, String>)> {
        self.store.check_cli_latest(&self.svc)
    }
    fn plan_update(&self, actor: &Actor, life: &Life, loaded: &[Loaded], fetch: &dyn Fetch) -> Result<UpdatePlan, LifeError> {
        self.store.plan_update(&self.svc, actor, life, loaded, fetch)
    }
    fn recover(&self) -> Vec<String> {
        self.store.recover(&self.svc)
    }
    fn apply_update(&self, actor: &Actor, plan: &UpdatePlan, confirm: Confirm, audit: &dyn Fn(&str, &str)) -> Result<String, LifeError> {
        self.store.apply_update(&self.svc, actor, plan, confirm, audit)
    }
    fn rollback(&self, actor: &Actor, life: &Life, audit: &dyn Fn(&str, &str)) -> Result<String, LifeError> {
        self.store.rollback(&self.svc, actor, life, audit)
    }
    fn plan_remove(&self, user: &str, lives: &[Life], life: &Life) -> RemovePlan {
        self.store.plan_remove(&self.svc, user, lives, life)
    }
    fn remove_item(&self, policy: &Policy, user: &str, lives: &[Life], life: &Life, opts: &RemoveOpts, audit: &dyn Fn(&str, &str)) -> Result<String, LifeError> {
        self.store.remove_item(&self.svc, policy, user, lives, life, opts, audit)
    }
    fn update_all_preview(&self, lives: &[Life]) -> Vec<(String, String, bool)> {
        Store::update_all_preview(lives)
    }
    fn update_all(&self, policy: &Policy, user: &str, loaded: &[Loaded], fetch: &dyn Fetch, audit: &dyn Fn(&str, &str), progress: &mut dyn FnMut(&str)) -> Report {
        self.store.update_all(&self.svc, policy, user, loaded, fetch, audit, progress)
    }
    fn dir(&self) -> &Path {
        self.store.dir()
    }
    fn now(&self) -> i64 {
        self.store.now()
    }
    fn roots(&self) -> Roots {
        self.store.roots().clone()
    }
    fn revocations(&self) -> Revocations {
        self.store.revocations()
    }
    fn config(&self) -> Config {
        self.store.config()
    }
    fn add_source(&self, actor: &Actor, name: &str, kind: SourceKind, location: &str) -> Result<String, String> {
        self.store.add_source(actor, name, kind, location)
    }
    fn official_store_url(&self) -> Option<&'static str> {
        Store::official_store_url()
    }
    fn probe_private(&self, fetch: &dyn Fetch, typed: &str) -> Result<Probe, String> {
        self.store.probe_private(fetch, typed)
    }
    fn pin_probed(&self, actor: &Actor, name: &str, p: &Probe, confirmed: &str) -> Result<String, String> {
        self.store.pin_probed(actor, name, p, confirmed)
    }
    fn set_publish_target(&self, actor: &Actor, id: &str) -> Result<(), String> {
        self.store.set_publish_target(actor, id)
    }
    fn remove_source(&self, actor: &Actor, id: &str) -> Result<(), String> {
        self.store.remove_source(actor, id)
    }
    fn toggle_source(&self, actor: &Actor, id: &str) -> Result<bool, String> {
        self.store.toggle_source(actor, id)
    }
    fn set_catalog_enabled(&self, actor: &Actor, on: bool) -> Result<(), String> {
        self.store.set_catalog_enabled(actor, on)
    }
    fn catalog_visible(&self) -> bool {
        self.store.catalog_visible()
    }
    fn trust_key(&self, actor: &Actor, key: &str, label: &str) -> Result<(), String> {
        self.store.trust_key(&self.svc, actor, key, label)
    }
    fn local_catalogs(&self) -> Vec<Loaded> {
        self.store.local_catalogs(&self.svc)
    }
    fn refresh(&self, fetch: &dyn Fetch) -> Refresh {
        self.store.refresh(&self.svc, fetch)
    }
    fn items(&self, actor: &Actor, loaded: &[Loaded]) -> Vec<Item> {
        self.store.items(&self.svc, actor, loaded)
    }
    fn known_names(&self, items: &[Item]) -> Vec<(String, String)> {
        Store::known_names(items)
    }
    fn review_item(&self, actor: &Actor, item: &Item, loaded: &[Loaded], fetch: &dyn Fetch, scope: Scope, known: &[(String, String)]) -> Result<Review, String> {
        self.store.review_item(&self.svc, actor, item, loaded, fetch, scope, known)
    }
    fn review_sideload(&self, actor: &Actor, bytes: Vec<u8>, scope: Scope, known: &[(String, String)]) -> Review {
        self.store.review_sideload(&self.svc, actor, bytes, scope, known)
    }
    fn update_check_run(&self, policy: &Policy, user: &str, fetch: &dyn Fetch, settings: &iyakki_shared::store_updatecheck::CheckSettings, running: &str, variant: &str) {
        crate::store::updatecheck::run_and_keep(&self.store, &self.svc, policy, user, fetch, settings, running, variant);
    }
}

impl iyakki_agent_api::StorePublish for StoreHandle {
    fn tile_open(&self, policy: &Policy, user: &str, entry: &iyakki_shared::store_tilehub::TileEntry, web: Arc<dyn iyakki_web::Fetch>) -> Result<Box<dyn iyakki_agent_api::TileRt>, String> {
        crate::store::tilehub::open(&self.svc, policy, user, entry, web).map(|r| Box::new(r) as Box<dyn iyakki_agent_api::TileRt>)
    }
    fn tile_open_wasm(&self, policy: &Policy, user: &str, entry: &iyakki_shared::store_tilehub::TileEntry) -> Result<iyakki_tile_host::TileRunner, String> {
        crate::store::tilehub::open_wasm(&self.svc, policy, user, entry)
    }
    fn pf_stores(&self) -> Vec<StoreChoice> {
        crate::store::publishflow::stores(&self.store)
    }
    fn pf_remote_version(&self, fetch: &dyn Fetch, server: &str, name: &str) -> Option<String> {
        crate::store::publishflow::remote_version(fetch, server, name)
    }
    fn pf_bumped(&self, v: &str, b: Bump) -> Result<String, String> {
        crate::store::publishflow::bumped(v, b)
    }
    fn pf_apply_bump(&self, dir: &Path, b: Bump, line: &str, today: &str) -> Result<String, String> {
        crate::store::publishflow::apply_bump(dir, b, line, today)
    }
    fn pf_add_changelog(&self, dir: &Path, version: &str, line: &str, today: &str) -> Result<(), String> {
        crate::store::publishflow::add_changelog(dir, version, line, today)
    }
    fn pf_token_present(&self, user: &str, source_id: &str) -> bool {
        crate::store::publishflow::token_present(&self.svc, user, source_id)
    }
    fn pf_save_token(&self, user: &str, source_id: &str, token: &str) -> Result<(), String> {
        crate::store::publishflow::save_token(&self.svc, user, source_id, token)
    }
    fn pf_card(&self, user: &str, dir: &Path, src: &SourceCfg, key: &(SigningKey, String), fetch: &dyn Fetch) -> Result<(Card, String), String> {
        crate::store::publishflow::card(&self.svc, user, dir, src, key, fetch)
    }
    fn pf_publish(&self, user: &str, dir: &Path, src: &SourceCfg, key: Option<&(SigningKey, String)>, fetch: &dyn Fetch, now: i64) -> Result<Published, Stop> {
        crate::store::publishflow::publish(&self.svc, user, dir, src, key, fetch, now)
    }
    fn pf_review_published(&self, actor: &Actor, fetch: &dyn Fetch, name: &str, scope: Scope) -> Result<Review, String> {
        crate::store::publishflow::review_published(&self.svc, &self.store, actor, fetch, name, scope)
    }
    fn pf_install_reviewed(&self, actor: &Actor, review: &Review) -> Result<String, String> {
        crate::store::publishflow::install_reviewed(&self.svc, &self.store, actor, review)
    }
    fn pf_yank_note(&self) -> &'static str {
        crate::store::publishflow::YANK_NOTE
    }
    fn pp_offer(&self, user: &str, fetch: &dyn Fetch) -> Option<Offer> {
        crate::store::privatepub::offer(&self.svc, user, &self.store, fetch)
    }
    fn pp_deployed_all(&self, user: &str) -> Vec<Deployed> {
        crate::store::privatepub::deployed_all(&self.svc, user, &self.store)
    }
    fn pp_option_label(&self, o: &Offer) -> String {
        crate::store::privatepub::option_label(o)
    }
    fn pp_need_admin_text(&self) -> String {
        crate::store::privatepub::need_admin_text()
    }
    fn pp_add_and_target(&self, actor: &Actor, fetch: &dyn Fetch, shown: &Probe, name: &str) -> Result<SourceCfg, String> {
        crate::store::privatepub::add_and_target(&self.store, actor, fetch, shown, name)
    }
    fn keys_list(&self, user: &str) -> Vec<KeyInfo> {
        crate::store::keys::list(&self.svc, user)
    }
    fn keys_load(&self, user: &str, label: Option<&str>) -> Option<(SigningKey, String)> {
        crate::store::keys::load(&self.svc, user, label)
    }
    fn keys_create(&self, user: &str, label: &str) -> Result<KeyInfo, String> {
        crate::store::keys::create(&self.svc, user, label)
    }
    fn author_pack_dir(&self, dir: &Path, key: Option<&SigningKey>) -> Result<(String, Vec<String>), String> {
        crate::store::author::pack_dir(dir, key)
    }
}
impl iyakki_agent_api::TileRt for crate::store::tile::TileRuntime {
    fn page_text(&mut self) -> String {
        crate::store::tile::TileRuntime::page_text(self)
    }
    fn doc(&mut self) -> iyakki_web::Doc {
        crate::store::tile::TileRuntime::doc(self)
    }
    fn pump(&mut self) -> bool {
        crate::store::tile::TileRuntime::pump(self)
    }
    fn resolve(&mut self, id: u32, ok: bool, value: &str) {
        crate::store::tile::TileRuntime::resolve(self, id, ok, value)
    }
    fn resolve_open(&mut self, id: u32, json: &str) {
        let _ = self.page.eval("open", &format!("__iyakkiResolve({id}, true, JSON.parse({}))", serde_json::to_string(json).unwrap_or_default()));
        if let Ok(mut o) = self.out.lock() {
            o.requests.retain(|x| x.id != id);
        }
    }
    fn click_control(&mut self, doc: &iyakki_web::Doc, idx: usize) -> Result<(), String> {
        crate::store::tile::TileRuntime::click_control(self, doc, idx)
    }
    fn type_into(&mut self, doc: &iyakki_web::Doc, idx: usize, text: &str) -> Result<(), String> {
        crate::store::tile::TileRuntime::type_into(self, doc, idx, text)
    }
    fn page_input(&mut self, node: usize, value: &str) -> bool {
        self.page.input(node, value).is_ok()
    }
    fn advance(&mut self, ms: u64) -> bool {
        self.page.advance(ms) > 0
    }
    fn answer_sdk(&mut self, id: u32, allow: bool) -> bool {
        crate::store::tile::TileRuntime::answer_sdk(self, id, allow)
    }
    fn console(&self) -> Vec<(String, String)> {
        self.page.report.console.iter().map(|l| (l.level.to_string(), l.text.clone())).collect()
    }
    fn page_notes(&self) -> Vec<String> {
        self.page.report.notes.clone()
    }
    fn take(&mut self) -> iyakki_agent_api::TileTake {
        let mut o = self.out.lock().unwrap_or_else(|e| e.into_inner());
        iyakki_agent_api::TileTake { toasts: std::mem::take(&mut o.toasts), title: o.title.take(), notices: std::mem::take(&mut o.notices), requests: o.requests.clone(), audit: std::mem::take(&mut o.audit) }
    }
    fn drop_request(&mut self, id: u32) {
        if let Ok(mut o) = self.out.lock() {
            o.requests.retain(|x| x.id != id);
        }
    }
    fn refused(&self) -> Vec<String> {
        self.out.lock().map(|o| o.refused.clone()).unwrap_or_default()
    }
    fn sdk_summary(&self) -> Option<String> {
        self.sdk.as_ref().and_then(|s| s.summary())
    }
}
