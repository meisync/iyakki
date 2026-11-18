//! The app store as the UI sees it (F-SPLIT-03, option B): one handle, `cx.services.b4().store()`, over the agent's `Store` and its services.
//! The data types are `iyakki_shared::store_*`; the defaults say "nothing here" (a core without a store).

use std::path::Path;

use iyakki_sec::policy::Policy;
use iyakki_shared::store_catalog::{Config, Fetch, Loaded, SourceCfg, SourceKind};
use iyakki_shared::store_core::{Item, Probe, Refresh};
use iyakki_core_api::appcaps::Approval;
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
use iyakki_shared::skills::Scope;

/// The answer of a call this core does not serve.
pub const UNAVAILABLE_STORE: &str = "unavailable: this core has no store";

pub trait StoreApi: StorePublish + Send + Sync {
    fn approval(&self, installation: &str) -> Option<Approval> {
        None
    }
    fn installed_version(&self, kind: &str, name: &str, scope: &Scope) -> Option<(String, bool)> {
        None
    }
    fn is_disabled(&self, kind: &str, name: &str, scope: &Scope) -> bool {
        false
    }
    fn install(&self, actor: &Actor, review: &Review, confirm: Confirm) -> Result<Record, InstallError> {
        Err(InstallError::Failed(UNAVAILABLE_STORE.into()))
    }
    fn remove(&self, actor: &Actor, kind: &str, name: &str, scope: &Scope) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn disable(&self, actor: &Actor, kind: &str, name: &str, scope: &Scope) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn enable(&self, actor: &Actor, kind: &str, name: &str, scope: &Scope) -> Result<Record, InstallError> {
        Err(InstallError::Failed(UNAVAILABLE_STORE.into()))
    }
    fn enforce_revocations(&self) -> Vec<String> {
        Vec::new()
    }
    fn add_license(&self, actor: &Actor, text: &str, signer_of: &dyn Fn(&str) -> Vec<String>) -> Result<Token, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn installed(&self, policy: &Policy, user: &str, items: &[Item]) -> Vec<Entry> {
        Vec::new()
    }
    fn remove_installed(&self, policy: &Policy, user: &str, e: &Entry) -> Result<String, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn toggle_installed(&self, policy: &Policy, user: &str, e: &Entry) -> Result<String, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pin(&self, key: &str, version: &str) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn unpin(&self, key: &str) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn lifecycle(&self, policy: &Policy, user: &str, loaded: &[Loaded], ctx: &Ctx) -> Vec<Life> {
        Vec::new()
    }
    fn find_life<'a>(&self, lives: &'a [Life], id: &str) -> Result<&'a Life, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn check_cli_latest(&self) -> Vec<(String, Result<String, String>)> {
        Vec::new()
    }
    fn plan_update(&self, actor: &Actor, life: &Life, loaded: &[Loaded], fetch: &dyn Fetch) -> Result<UpdatePlan, LifeError> {
        Err(LifeError::Failed(UNAVAILABLE_STORE.into()))
    }
    fn recover(&self) -> Vec<String> {
        Vec::new()
    }
    fn apply_update(&self, actor: &Actor, plan: &UpdatePlan, confirm: Confirm, audit: &dyn Fn(&str, &str)) -> Result<String, LifeError> {
        Err(LifeError::Failed(UNAVAILABLE_STORE.into()))
    }
    fn rollback(&self, actor: &Actor, life: &Life, audit: &dyn Fn(&str, &str)) -> Result<String, LifeError> {
        Err(LifeError::Failed(UNAVAILABLE_STORE.into()))
    }
    fn plan_remove(&self, user: &str, lives: &[Life], life: &Life) -> RemovePlan;
    fn remove_item(&self, policy: &Policy, user: &str, lives: &[Life], life: &Life, opts: &RemoveOpts, audit: &dyn Fn(&str, &str)) -> Result<String, LifeError> {
        Err(LifeError::Failed(UNAVAILABLE_STORE.into()))
    }
    fn update_all_preview(&self, lives: &[Life]) -> Vec<(String, String, bool)> {
        Vec::new()
    }
    fn update_all(&self, policy: &Policy, user: &str, loaded: &[Loaded], fetch: &dyn Fetch, audit: &dyn Fn(&str, &str), progress: &mut dyn FnMut(&str)) -> Report {
        Default::default()
    }
    fn dir(&self) -> &Path {
        Path::new("")
    }
    fn now(&self) -> i64 {
        0
    }
    fn roots(&self) -> Roots {
        Default::default()
    }
    fn revocations(&self) -> Revocations {
        Default::default()
    }
    fn config(&self) -> Config {
        Default::default()
    }
    fn add_source(&self, actor: &Actor, name: &str, kind: SourceKind, location: &str) -> Result<String, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn official_store_url(&self) -> Option<&'static str> {
        None
    }
    fn probe_private(&self, fetch: &dyn Fetch, typed: &str) -> Result<Probe, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pin_probed(&self, actor: &Actor, name: &str, p: &Probe, confirmed: &str) -> Result<String, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn set_publish_target(&self, actor: &Actor, id: &str) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn remove_source(&self, actor: &Actor, id: &str) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn toggle_source(&self, actor: &Actor, id: &str) -> Result<bool, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn set_catalog_enabled(&self, actor: &Actor, on: bool) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn catalog_visible(&self) -> bool {
        false
    }
    fn trust_key(&self, actor: &Actor, key: &str, label: &str) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn local_catalogs(&self) -> Vec<Loaded> {
        Vec::new()
    }
    fn refresh(&self, fetch: &dyn Fetch) -> Refresh {
        Default::default()
    }
    fn items(&self, actor: &Actor, loaded: &[Loaded]) -> Vec<Item> {
        Vec::new()
    }
    fn known_names(&self, items: &[Item]) -> Vec<(String, String)> {
        Vec::new()
    }
    fn review_item(&self, actor: &Actor, item: &Item, loaded: &[Loaded], fetch: &dyn Fetch, scope: Scope, known: &[(String, String)]) -> Result<Review, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn review_sideload(&self, actor: &Actor, bytes: Vec<u8>, scope: Scope, known: &[(String, String)]) -> Review;
    /// Run the update check and keep its report (the Check button and the daily check).
    #[allow(clippy::too_many_arguments)]
    fn update_check_run(&self, _policy: &Policy, _user: &str, _fetch: &dyn Fetch, _settings: &iyakki_shared::store_updatecheck::CheckSettings, _running: &str, _variant: &str) {}
}

/// The publish flow of the AI IDE over the same store handle: Publish form, private-store offer, signing keys, packing and the tile tests.
pub trait StorePublish: Send + Sync {
    fn pf_stores(&self) -> Vec<StoreChoice> {
        Vec::new()
    }
    fn pf_remote_version(&self, fetch: &dyn Fetch, server: &str, name: &str) -> Option<String> {
        None
    }
    fn pf_bumped(&self, v: &str, b: Bump) -> Result<String, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pf_apply_bump(&self, dir: &Path, b: Bump, line: &str, today: &str) -> Result<String, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pf_add_changelog(&self, dir: &Path, version: &str, line: &str, today: &str) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pf_token_present(&self, user: &str, source_id: &str) -> bool {
        false
    }
    fn pf_save_token(&self, user: &str, source_id: &str, token: &str) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pf_card(&self, user: &str, dir: &Path, src: &SourceCfg, key: &(SigningKey, String), fetch: &dyn Fetch) -> Result<(Card, String), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pf_publish(&self, user: &str, dir: &Path, src: &SourceCfg, key: Option<&(SigningKey, String)>, fetch: &dyn Fetch, now: i64) -> Result<Published, Stop> {
        Err(Stop::NeedsKey)
    }
    fn pf_review_published(&self, actor: &Actor, fetch: &dyn Fetch, name: &str, scope: Scope) -> Result<Review, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pf_install_reviewed(&self, actor: &Actor, review: &Review) -> Result<String, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pf_yank_note(&self) -> &'static str {
        ""
    }
    fn pp_offer(&self, user: &str, fetch: &dyn Fetch) -> Option<Offer> {
        None
    }
    fn pp_deployed_all(&self, user: &str) -> Vec<Deployed> {
        Vec::new()
    }
    fn pp_option_label(&self, o: &Offer) -> String {
        String::new()
    }
    fn pp_need_admin_text(&self) -> String {
        String::new()
    }
    fn pp_add_and_target(&self, actor: &Actor, fetch: &dyn Fetch, shown: &Probe, name: &str) -> Result<SourceCfg, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn keys_list(&self, user: &str) -> Vec<KeyInfo> {
        Vec::new()
    }
    fn keys_load(&self, user: &str, label: Option<&str>) -> Option<(SigningKey, String)> {
        None
    }
    fn keys_create(&self, user: &str, label: &str) -> Result<KeyInfo, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn author_pack_dir(&self, dir: &Path, key: Option<&SigningKey>) -> Result<(String, Vec<String>), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    /// Open an installed tile as a WebAssembly tile (the browser renderer of the tile host).
    fn tile_open_wasm(&self, _policy: &Policy, _user: &str, _entry: &iyakki_shared::store_tilehub::TileEntry) -> Result<iyakki_tile_host::TileRunner, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    /// Open an installed tile app: its page runs in the host's JavaScript engine, its calls to the host come back as requests.
    fn tile_open(&self, _policy: &Policy, _user: &str, _entry: &iyakki_shared::store_tilehub::TileEntry, _web: std::sync::Arc<dyn iyakki_web::Fetch>) -> Result<Box<dyn TileRt>, String> {
        Err(UNAVAILABLE_STORE.into())
    }
}

/// What the shell takes out of a running tile app's outbox on each tick (the rest of the outbox stays with the runtime).
#[derive(Debug, Default)]
pub struct TileTake {
    pub toasts: Vec<String>,
    pub title: Option<String>,
    pub notices: Vec<(String, String)>,
    /// The requests the app waits on (they stay in the outbox until answered).
    pub requests: Vec<iyakki_shared::store_tile::Request>,
    /// (action, detail) lines for the audit log.
    pub audit: Vec<(String, String)>,
}

/// A running tile app, as the shell drives it (the real one is the agent's `TileRuntime`).
pub trait TileRt: Send {
    fn page_text(&mut self) -> String;
    fn doc(&mut self) -> iyakki_web::Doc;
    /// Deliver answers of slow calls, stream events and the SDK's time limits.
    fn pump(&mut self) -> bool;
    /// Answer a request of the app.
    fn resolve(&mut self, id: u32, ok: bool, value: &str);
    /// Answer an "open file" request with the file's name and text, and drop the request.
    fn resolve_open(&mut self, id: u32, json: &str);
    fn click_control(&mut self, doc: &iyakki_web::Doc, idx: usize) -> Result<(), String>;
    fn type_into(&mut self, doc: &iyakki_web::Doc, idx: usize, text: &str) -> Result<(), String>;
    /// Choose an option of a select: set the control of `node` to `value`.
    fn page_input(&mut self, node: usize, value: &str) -> bool;
    /// Timers in the page run in small steps of the page's own time; whether anything ran.
    fn advance(&mut self, ms: u64) -> bool;
    fn answer_sdk(&mut self, id: u32, allow: bool) -> bool;
    /// The page's console: (level, text).
    fn console(&self) -> Vec<(String, String)>;
    /// What the page engine said about the page (errors and warnings).
    fn page_notes(&self) -> Vec<String>;
    fn take(&mut self) -> TileTake;
    /// Drop a request from the outbox (it was handed to the assistant and is no longer waiting).
    fn drop_request(&mut self, id: u32);
    /// Calls the app made that were refused.
    fn refused(&self) -> Vec<String>;
    /// One line on the app's model, voice and numbers, when it uses them.
    fn sdk_summary(&self) -> Option<String>;
}
