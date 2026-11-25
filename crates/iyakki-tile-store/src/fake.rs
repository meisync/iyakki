//! The fake of the store engine: canned answers for the few values a screen reads (the clock, the folder, the official address, whether the
//! catalog shows, one approval note) and an error for every other call that has an error to give. Every call is recorded by name, in
//! order, so a test can check what the screen asked. A call that has no plain answer to give panics with its name: a test that reaches it
//! has to say what the store answers.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

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

use crate::engine::StoreEngine;

/// The answer of a call the fake does not have canned.
pub const NOT_CANNED: &str = "fake store: not canned";

pub struct FakeStoreEngine {
    calls: Mutex<Vec<&'static str>>,
    now: i64,
    dir: PathBuf,
    official: Option<&'static str>,
    visible: bool,
    approval: Option<String>,
}

impl Default for FakeStoreEngine {
    fn default() -> Self {
        FakeStoreEngine { calls: Mutex::new(Vec::new()), now: 0, dir: PathBuf::new(), official: None, visible: false, approval: None }
    }
}

impl FakeStoreEngine {
    /// The clock, the store folder, the official address (`None` = not published), whether the catalog shows, and the approval note.
    pub fn with(now: i64, dir: &str, official: Option<&'static str>, visible: bool, approval: Option<&str>) -> Self {
        FakeStoreEngine { now, dir: PathBuf::from(dir), official, visible, approval: approval.map(str::to_string), ..Default::default() }
    }

    /// The names of the calls made so far, in order.
    pub fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn see(&self, name: &'static str) {
        self.calls.lock().unwrap_or_else(|e| e.into_inner()).push(name);
    }

    fn err<T>(&self, name: &'static str) -> Result<T, String> {
        self.see(name);
        Err(NOT_CANNED.to_string())
    }
}

impl StoreEngine for FakeStoreEngine {
    fn approval_audit(&self, _installation: &str, _app_version: &str) -> Option<String> {
        self.see("approval_audit");
        self.approval.clone()
    }
    fn installed_version(&self, _kind: &str, _name: &str, _scope: &Scope) -> Option<(String, bool)> {
        self.see("installed_version");
        None
    }
    fn is_disabled(&self, _kind: &str, _name: &str, _scope: &Scope) -> bool {
        self.see("is_disabled");
        false
    }
    fn install(&self, _actor: &Actor, _review: &Review, _confirm: Confirm) -> Result<Record, InstallError> {
        self.see("install");
        Err(InstallError::Failed(NOT_CANNED.into()))
    }
    fn remove(&self, _actor: &Actor, _kind: &str, _name: &str, _scope: &Scope) -> Result<(), String> {
        self.err("remove")
    }
    fn disable(&self, _actor: &Actor, _kind: &str, _name: &str, _scope: &Scope) -> Result<(), String> {
        self.err("disable")
    }
    fn enable(&self, _actor: &Actor, _kind: &str, _name: &str, _scope: &Scope) -> Result<Record, InstallError> {
        self.see("enable");
        Err(InstallError::Failed(NOT_CANNED.into()))
    }
    fn enforce_revocations(&self) -> Vec<String> {
        self.see("enforce_revocations");
        Vec::new()
    }
    fn recover(&self) -> Vec<String> {
        self.see("recover");
        Vec::new()
    }
    fn add_license(&self, _actor: &Actor, _text: &str, _signer_of: &dyn Fn(&str) -> Vec<String>) -> Result<Token, String> {
        self.err("add_license")
    }
    fn checkout_url(&self, _app: &str, _pricing: &Pricing) -> Result<String, String> {
        self.err("checkout_url")
    }
    fn remove_installed(&self, _policy: &Policy, _user: &str, _e: &Entry) -> Result<String, String> {
        self.err("remove_installed")
    }
    fn toggle_installed(&self, _policy: &Policy, _user: &str, _e: &Entry) -> Result<String, String> {
        self.err("toggle_installed")
    }
    fn pin(&self, _key: &str, _version: &str) -> Result<(), String> {
        self.err("pin")
    }
    fn unpin(&self, _key: &str) -> Result<(), String> {
        self.err("unpin")
    }
    fn lifecycle(&self, _policy: &Policy, _user: &str, _loaded: &[Loaded], _ctx: &Ctx) -> Vec<Life> {
        self.see("lifecycle");
        Vec::new()
    }
    fn check_cli_latest(&self) -> Vec<(String, Result<String, String>)> {
        self.see("check_cli_latest");
        Vec::new()
    }
    fn plan_update(&self, _actor: &Actor, _life: &Life, _loaded: &[Loaded], _fetch: &dyn Fetch) -> Result<UpdatePlan, LifeError> {
        self.see("plan_update");
        Err(LifeError::Failed(NOT_CANNED.into()))
    }
    fn apply_update(&self, _actor: &Actor, _plan: &UpdatePlan, _confirm: Confirm, _audit: &dyn Fn(&str, &str)) -> Result<String, LifeError> {
        self.see("apply_update");
        Err(LifeError::Failed(NOT_CANNED.into()))
    }
    fn rollback(&self, _actor: &Actor, _life: &Life, _audit: &dyn Fn(&str, &str)) -> Result<String, LifeError> {
        self.see("rollback");
        Err(LifeError::Failed(NOT_CANNED.into()))
    }
    fn plan_remove(&self, _user: &str, _lives: &[Life], _life: &Life) -> RemovePlan {
        self.see("plan_remove");
        panic!("{NOT_CANNED}: plan_remove has no plain answer in the fake")
    }
    fn remove_item(&self, _policy: &Policy, _user: &str, _lives: &[Life], _life: &Life, _opts: &RemoveOpts, _audit: &dyn Fn(&str, &str)) -> Result<String, LifeError> {
        self.see("remove_item");
        Err(LifeError::Failed(NOT_CANNED.into()))
    }
    fn update_all_preview(&self, _lives: &[Life]) -> Vec<(String, String, bool)> {
        self.see("update_all_preview");
        Vec::new()
    }
    fn update_all(&self, _policy: &Policy, _user: &str, _loaded: &[Loaded], _fetch: &dyn Fetch, _audit: &dyn Fn(&str, &str), _progress: &mut dyn FnMut(&str)) -> Report {
        self.see("update_all");
        Report::default()
    }
    fn dir(&self) -> &Path {
        self.see("dir");
        &self.dir
    }
    fn now(&self) -> i64 {
        self.see("now");
        self.now
    }
    fn roots(&self) -> Roots {
        self.see("roots");
        Roots::default()
    }
    fn revocations(&self) -> Revocations {
        self.see("revocations");
        Revocations::default()
    }
    fn config(&self) -> Config {
        self.see("config");
        Config::default()
    }
    fn add_source(&self, _actor: &Actor, _name: &str, _kind: SourceKind, _location: &str) -> Result<String, String> {
        self.err("add_source")
    }
    fn official_store_url(&self) -> Option<&'static str> {
        self.see("official_store_url");
        self.official
    }
    fn probe_private(&self, _fetch: &dyn Fetch, _typed: &str) -> Result<Probe, String> {
        self.err("probe_private")
    }
    fn pin_probed(&self, _actor: &Actor, _name: &str, _p: &Probe, _confirmed: &str) -> Result<String, String> {
        self.err("pin_probed")
    }
    fn set_publish_target(&self, _actor: &Actor, _id: &str) -> Result<(), String> {
        self.err("set_publish_target")
    }
    fn remove_source(&self, _actor: &Actor, _id: &str) -> Result<(), String> {
        self.err("remove_source")
    }
    fn toggle_source(&self, _actor: &Actor, _id: &str) -> Result<bool, String> {
        self.err("toggle_source")
    }
    fn set_catalog_enabled(&self, _actor: &Actor, _on: bool) -> Result<(), String> {
        self.err("set_catalog_enabled")
    }
    fn catalog_visible(&self) -> bool {
        self.see("catalog_visible");
        self.visible
    }
    fn trust_key(&self, _actor: &Actor, _key: &str, _label: &str) -> Result<(), String> {
        self.err("trust_key")
    }
    fn local_catalogs(&self) -> Vec<Loaded> {
        self.see("local_catalogs");
        Vec::new()
    }
    fn refresh(&self, _fetch: &dyn Fetch) -> Refresh {
        self.see("refresh");
        Refresh::default()
    }
    fn items(&self, _actor: &Actor, _loaded: &[Loaded]) -> Vec<Item> {
        self.see("items");
        Vec::new()
    }
    fn known_names(&self, _items: &[Item]) -> Vec<(String, String)> {
        self.see("known_names");
        Vec::new()
    }
    fn review_item(&self, _actor: &Actor, _item: &Item, _loaded: &[Loaded], _fetch: &dyn Fetch, _scope: Scope, _known: &[(String, String)]) -> Result<Review, String> {
        self.err("review_item")
    }
    fn review_sideload(&self, _actor: &Actor, _bytes: Vec<u8>, _scope: Scope, _known: &[(String, String)]) -> Review {
        self.see("review_sideload");
        panic!("{NOT_CANNED}: review_sideload has no plain answer in the fake")
    }
    fn update_check_run(&self, _policy: &Policy, _user: &str, _fetch: &dyn Fetch, _settings: &CheckSettings, _running: &str, _variant: &str) {
        self.see("update_check_run");
    }
}
