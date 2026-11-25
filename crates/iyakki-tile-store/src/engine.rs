//! The store as the STORE tile calls it: the calls the screens make on the agent's `StoreApi`, named with the store's pure types only (the
//! model crate `iyakki-store-model` and the account policy). No agent, no core API, no shared crate. The shell's forwarding wrapper is
//! `AgentStore` in `crates/iyakki-apps/src/apps/store_port.rs`; a call that the agent answers "unavailable" stays the same answer.

use std::path::Path;

use iyakki_sec::policy::Policy;
use iyakki_store_model::skills::Scope;
use iyakki_store_model::store_catalog::{Config, Fetch, Loaded, SourceKind};
use iyakki_store_model::store_core::{Item, Probe, Refresh};
use iyakki_store_model::store_install::{Actor, Confirm, InstallError, Record, Review};
use iyakki_store_model::store_installed::Entry;
use iyakki_store_model::store_license::Token;
use iyakki_store_model::store_lifecycle::{Ctx, Life, LifeError, RemoveOpts, RemovePlan, Report, UpdatePlan};
use iyakki_store_model::store_manifest::Pricing;
use iyakki_store_model::store_revocation::Revocations;
use iyakki_store_model::store_trust::Roots;
use iyakki_store_model::store_updatecheck::CheckSettings;

/// The calls the STORE screens make on the store. Every method mirrors the agent's `StoreApi` method of the same name.
pub trait StoreEngine: Send + Sync {
    /// The audit line of an approval kept for this installation when it is for this app version with grants (none otherwise).
    fn approval_audit(&self, installation: &str, app_version: &str) -> Option<String>;
    fn installed_version(&self, kind: &str, name: &str, scope: &Scope) -> Option<(String, bool)>;
    fn is_disabled(&self, kind: &str, name: &str, scope: &Scope) -> bool;
    fn install(&self, actor: &Actor, review: &Review, confirm: Confirm) -> Result<Record, InstallError>;
    fn remove(&self, actor: &Actor, kind: &str, name: &str, scope: &Scope) -> Result<(), String>;
    fn disable(&self, actor: &Actor, kind: &str, name: &str, scope: &Scope) -> Result<(), String>;
    fn enable(&self, actor: &Actor, kind: &str, name: &str, scope: &Scope) -> Result<Record, InstallError>;
    fn enforce_revocations(&self) -> Vec<String>;
    fn recover(&self) -> Vec<String>;
    fn add_license(&self, actor: &Actor, text: &str, signer_of: &dyn Fn(&str) -> Vec<String>) -> Result<Token, String>;
    /// The address the Buy button opens (see the agent's `StoreApi::checkout_url`).
    fn checkout_url(&self, app: &str, pricing: &Pricing) -> Result<String, String>;
    fn remove_installed(&self, policy: &Policy, user: &str, e: &Entry) -> Result<String, String>;
    fn toggle_installed(&self, policy: &Policy, user: &str, e: &Entry) -> Result<String, String>;
    fn pin(&self, key: &str, version: &str) -> Result<(), String>;
    fn unpin(&self, key: &str) -> Result<(), String>;
    fn lifecycle(&self, policy: &Policy, user: &str, loaded: &[Loaded], ctx: &Ctx) -> Vec<Life>;
    fn check_cli_latest(&self) -> Vec<(String, Result<String, String>)>;
    fn plan_update(&self, actor: &Actor, life: &Life, loaded: &[Loaded], fetch: &dyn Fetch) -> Result<UpdatePlan, LifeError>;
    fn apply_update(&self, actor: &Actor, plan: &UpdatePlan, confirm: Confirm, audit: &dyn Fn(&str, &str)) -> Result<String, LifeError>;
    fn rollback(&self, actor: &Actor, life: &Life, audit: &dyn Fn(&str, &str)) -> Result<String, LifeError>;
    fn plan_remove(&self, user: &str, lives: &[Life], life: &Life) -> RemovePlan;
    fn remove_item(&self, policy: &Policy, user: &str, lives: &[Life], life: &Life, opts: &RemoveOpts, audit: &dyn Fn(&str, &str)) -> Result<String, LifeError>;
    fn update_all_preview(&self, lives: &[Life]) -> Vec<(String, String, bool)>;
    fn update_all(&self, policy: &Policy, user: &str, loaded: &[Loaded], fetch: &dyn Fetch, audit: &dyn Fn(&str, &str), progress: &mut dyn FnMut(&str)) -> Report;
    fn dir(&self) -> &Path;
    fn now(&self) -> i64;
    fn roots(&self) -> Roots;
    fn revocations(&self) -> Revocations;
    fn config(&self) -> Config;
    fn add_source(&self, actor: &Actor, name: &str, kind: SourceKind, location: &str) -> Result<String, String>;
    fn official_store_url(&self) -> Option<&'static str>;
    fn probe_private(&self, fetch: &dyn Fetch, typed: &str) -> Result<Probe, String>;
    fn pin_probed(&self, actor: &Actor, name: &str, p: &Probe, confirmed: &str) -> Result<String, String>;
    fn set_publish_target(&self, actor: &Actor, id: &str) -> Result<(), String>;
    fn remove_source(&self, actor: &Actor, id: &str) -> Result<(), String>;
    fn toggle_source(&self, actor: &Actor, id: &str) -> Result<bool, String>;
    fn set_catalog_enabled(&self, actor: &Actor, on: bool) -> Result<(), String>;
    fn catalog_visible(&self) -> bool;
    fn trust_key(&self, actor: &Actor, key: &str, label: &str) -> Result<(), String>;
    fn local_catalogs(&self) -> Vec<Loaded>;
    fn refresh(&self, fetch: &dyn Fetch) -> Refresh;
    fn items(&self, actor: &Actor, loaded: &[Loaded]) -> Vec<Item>;
    fn known_names(&self, items: &[Item]) -> Vec<(String, String)>;
    fn review_item(&self, actor: &Actor, item: &Item, loaded: &[Loaded], fetch: &dyn Fetch, scope: Scope, known: &[(String, String)]) -> Result<Review, String>;
    fn review_sideload(&self, actor: &Actor, bytes: Vec<u8>, scope: Scope, known: &[(String, String)]) -> Review;
    /// Run the update check and keep its report (the Check button and the daily check).
    fn update_check_run(&self, policy: &Policy, user: &str, fetch: &dyn Fetch, settings: &CheckSettings, running: &str, variant: &str);
}

/// The audit log the store's background work records into (a line per action, for the person who acted).
pub trait AuditSink: Send + Sync {
    fn record(&self, user: &str, action: &str, detail: &str);
}
