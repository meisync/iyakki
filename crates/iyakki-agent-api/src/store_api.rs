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
use iyakki_shared::store_manifest::Pricing;
use iyakki_shared::store_lifecycle::{Ctx, Life, LifeError, RemoveOpts, RemovePlan, Report, UpdatePlan};
use iyakki_shared::store_author::Published;
use iyakki_shared::store_keys::{KeyInfo, SigningKey};
use iyakki_shared::store_privatepub::{Deployed, Offer};
use iyakki_shared::store_publishflow::{Bump, Card, Stop, StoreChoice};
use iyakki_shared::store_revocation::Revocations;
use iyakki_shared::store_trust::Roots;
use iyakki_shared::skills::Scope;

/// The answer of a call this core does not serve.
pub const UNAVAILABLE_STORE: &str = "unavailable: this core has no store";

pub trait StoreApi: StorePublish + Send + Sync {
    fn approval(&self, _installation: &str) -> Option<Approval> {
        None
    }
    fn installed_version(&self, _kind: &str, _name: &str, _scope: &Scope) -> Option<(String, bool)> {
        None
    }
    fn is_disabled(&self, _kind: &str, _name: &str, _scope: &Scope) -> bool {
        false
    }
    fn install(&self, _actor: &Actor, _review: &Review, _confirm: Confirm) -> Result<Record, InstallError> {
        Err(InstallError::Failed(UNAVAILABLE_STORE.into()))
    }
    fn remove(&self, _actor: &Actor, _kind: &str, _name: &str, _scope: &Scope) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn disable(&self, _actor: &Actor, _kind: &str, _name: &str, _scope: &Scope) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn enable(&self, _actor: &Actor, _kind: &str, _name: &str, _scope: &Scope) -> Result<Record, InstallError> {
        Err(InstallError::Failed(UNAVAILABLE_STORE.into()))
    }
    fn enforce_revocations(&self) -> Vec<String> {
        Vec::new()
    }
    fn add_license(&self, _actor: &Actor, _text: &str, _signer_of: &dyn Fn(&str) -> Vec<String>) -> Result<Token, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    /// The address the Buy button opens in the browser tile: the publisher's own https checkout page plus the app name and this device's
    /// random licence id, nothing else (F-APPSTORE-13). An error says why there is nothing to buy.
    fn checkout_url(&self, _app: &str, _pricing: &Pricing) -> Result<String, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn installed(&self, _policy: &Policy, _user: &str, _items: &[Item]) -> Vec<Entry> {
        Vec::new()
    }
    fn remove_installed(&self, _policy: &Policy, _user: &str, _e: &Entry) -> Result<String, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn toggle_installed(&self, _policy: &Policy, _user: &str, _e: &Entry) -> Result<String, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pin(&self, _key: &str, _version: &str) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn unpin(&self, _key: &str) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn lifecycle(&self, _policy: &Policy, _user: &str, _loaded: &[Loaded], _ctx: &Ctx) -> Vec<Life> {
        Vec::new()
    }
    fn find_life<'a>(&self, _lives: &'a [Life], _id: &str) -> Result<&'a Life, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn check_cli_latest(&self) -> Vec<(String, Result<String, String>)> {
        Vec::new()
    }
    fn plan_update(&self, _actor: &Actor, _life: &Life, _loaded: &[Loaded], _fetch: &dyn Fetch) -> Result<UpdatePlan, LifeError> {
        Err(LifeError::Failed(UNAVAILABLE_STORE.into()))
    }
    fn recover(&self) -> Vec<String> {
        Vec::new()
    }
    fn apply_update(&self, _actor: &Actor, _plan: &UpdatePlan, _confirm: Confirm, _audit: &dyn Fn(&str, &str)) -> Result<String, LifeError> {
        Err(LifeError::Failed(UNAVAILABLE_STORE.into()))
    }
    fn rollback(&self, _actor: &Actor, _life: &Life, _audit: &dyn Fn(&str, &str)) -> Result<String, LifeError> {
        Err(LifeError::Failed(UNAVAILABLE_STORE.into()))
    }
    fn plan_remove(&self, user: &str, lives: &[Life], life: &Life) -> RemovePlan;
    fn remove_item(&self, _policy: &Policy, _user: &str, _lives: &[Life], _life: &Life, _opts: &RemoveOpts, _audit: &dyn Fn(&str, &str)) -> Result<String, LifeError> {
        Err(LifeError::Failed(UNAVAILABLE_STORE.into()))
    }
    fn update_all_preview(&self, _lives: &[Life]) -> Vec<(String, String, bool)> {
        Vec::new()
    }
    fn update_all(&self, _policy: &Policy, _user: &str, _loaded: &[Loaded], _fetch: &dyn Fetch, _audit: &dyn Fn(&str, &str), _progress: &mut dyn FnMut(&str)) -> Report {
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
    fn add_source(&self, _actor: &Actor, _name: &str, _kind: SourceKind, _location: &str) -> Result<String, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn official_store_url(&self) -> Option<&'static str> {
        None
    }
    fn probe_private(&self, _fetch: &dyn Fetch, _typed: &str) -> Result<Probe, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pin_probed(&self, _actor: &Actor, _name: &str, _p: &Probe, _confirmed: &str) -> Result<String, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn set_publish_target(&self, _actor: &Actor, _id: &str) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn remove_source(&self, _actor: &Actor, _id: &str) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn toggle_source(&self, _actor: &Actor, _id: &str) -> Result<bool, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn set_catalog_enabled(&self, _actor: &Actor, _on: bool) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn catalog_visible(&self) -> bool {
        false
    }
    fn trust_key(&self, _actor: &Actor, _key: &str, _label: &str) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn local_catalogs(&self) -> Vec<Loaded> {
        Vec::new()
    }
    fn refresh(&self, _fetch: &dyn Fetch) -> Refresh {
        Default::default()
    }
    fn items(&self, _actor: &Actor, _loaded: &[Loaded]) -> Vec<Item> {
        Vec::new()
    }
    fn known_names(&self, _items: &[Item]) -> Vec<(String, String)> {
        Vec::new()
    }
    fn review_item(&self, _actor: &Actor, _item: &Item, _loaded: &[Loaded], _fetch: &dyn Fetch, _scope: Scope, _known: &[(String, String)]) -> Result<Review, String> {
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
    fn pf_remote_version(&self, _fetch: &dyn Fetch, _server: &str, _name: &str) -> Option<String> {
        None
    }
    fn pf_bumped(&self, _v: &str, _b: Bump) -> Result<String, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pf_apply_bump(&self, _dir: &Path, _b: Bump, _line: &str, _today: &str) -> Result<String, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pf_add_changelog(&self, _dir: &Path, _version: &str, _line: &str, _today: &str) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pf_token_present(&self, _user: &str, _source_id: &str) -> bool {
        false
    }
    fn pf_save_token(&self, _user: &str, _source_id: &str, _token: &str) -> Result<(), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pf_card(&self, _user: &str, _dir: &Path, _src: &SourceCfg, _key: &(SigningKey, String), _fetch: &dyn Fetch) -> Result<(Card, String), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pf_publish(&self, _user: &str, _dir: &Path, _src: &SourceCfg, _key: Option<&(SigningKey, String)>, _fetch: &dyn Fetch, _now: i64) -> Result<Published, Stop> {
        Err(Stop::NeedsKey)
    }
    fn pf_review_published(&self, _actor: &Actor, _fetch: &dyn Fetch, _name: &str, _scope: Scope) -> Result<Review, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pf_install_reviewed(&self, _actor: &Actor, _review: &Review) -> Result<String, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn pf_yank_note(&self) -> &'static str {
        ""
    }
    fn pp_offer(&self, _user: &str, _fetch: &dyn Fetch) -> Option<Offer> {
        None
    }
    fn pp_deployed_all(&self, _user: &str) -> Vec<Deployed> {
        Vec::new()
    }
    fn pp_option_label(&self, _o: &Offer) -> String {
        String::new()
    }
    fn pp_need_admin_text(&self) -> String {
        String::new()
    }
    fn pp_add_and_target(&self, _actor: &Actor, _fetch: &dyn Fetch, _shown: &Probe, _name: &str) -> Result<SourceCfg, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn keys_list(&self, _user: &str) -> Vec<KeyInfo> {
        Vec::new()
    }
    fn keys_load(&self, _user: &str, _label: Option<&str>) -> Option<(SigningKey, String)> {
        None
    }
    fn keys_create(&self, _user: &str, _label: &str) -> Result<KeyInfo, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    fn author_pack_dir(&self, _dir: &Path, _key: Option<&SigningKey>) -> Result<(String, Vec<String>), String> {
        Err(UNAVAILABLE_STORE.into())
    }
    /// The tile app of this person with this key, as the core lists it (the core never runs an entry the UI describes: over the socket the entry
    /// of the core's own list is used, D-SPLIT-03-TILES). The default lists none.
    fn tile_listed(&self, _policy: &Policy, _user: &str, _key: &str) -> Option<iyakki_shared::store_tilehub::TileEntry> {
        None
    }
    /// Open an installed tile as a WebAssembly tile (the browser renderer of the tile host).
    fn tile_open_wasm(&self, _policy: &Policy, _user: &str, _entry: &iyakki_shared::store_tilehub::TileEntry) -> Result<Box<dyn WasmTileRt>, String> {
        Err(UNAVAILABLE_STORE.into())
    }
    /// The pictures a WebAssembly tile carries in its bundle (F-WASM-16), decoded into the tile's table. The default has none.
    fn tile_pictures(&self, _policy: &Policy, _user: &str, _entry: &iyakki_shared::store_tilehub::TileEntry) -> Result<iyakki_tile_host::ImageTable, String> {
        Ok(iyakki_tile_host::ImageTable::new())
    }
    /// Open an installed tile app: its page runs in the host's JavaScript engine, its calls to the host come back as requests.
    fn tile_open(&self, _policy: &Policy, _user: &str, _entry: &iyakki_shared::store_tilehub::TileEntry, _web: std::sync::Arc<dyn iyakki_web::Fetch>) -> Result<Box<dyn TileRt>, String> {
        Err(UNAVAILABLE_STORE.into())
    }
}

/// A running WebAssembly tile, as the shell drives it: events in, a display list out (the real one is the host's `TileRunner`, in the core's
/// process; over the UI socket the UI holds a remote object and draws the lists the core sends, D-SPLIT-03-TILES).
pub trait WasmTileRt: Send {
    fn init(&mut self, w: i32, h: i32, scale_pct: u32, theme: iyakki_tile_abi::Event) -> Result<(), String>;
    /// Deliver events; whether the tile asked for a redraw.
    fn event(&mut self, events: &[iyakki_tile_abi::Event]) -> Result<bool, String>;
    /// A tap at a point; the hit id of the topmost hit region of the last frame there.
    fn click(&mut self, x: i32, y: i32) -> Result<Option<u32>, String>;
    /// The newest picture of the tile.
    fn frame(&mut self) -> Result<iyakki_tile_abi::DisplayList, String>;
    /// What waits for the person's answer: (id, kind, arguments as JSON), oldest first.
    fn requests(&self) -> Vec<(u32, String, String)>;
    fn answer(&self, id: u32, allow: bool) -> bool;
    /// The app the tile asked the dock to open since the last time, once.
    fn take_dock_open(&self) -> Option<String>;
    /// Whether the tile reported ABI 3 and so may be sent app events.
    fn takes_app_events(&self) -> bool;
}

impl WasmTileRt for iyakki_tile_host::TileRunner {
    fn init(&mut self, w: i32, h: i32, scale_pct: u32, theme: iyakki_tile_abi::Event) -> Result<(), String> {
        iyakki_tile_host::TileRunner::init(self, w, h, scale_pct, theme).map_err(|e| e.to_string())
    }
    fn event(&mut self, events: &[iyakki_tile_abi::Event]) -> Result<bool, String> {
        iyakki_tile_host::TileRunner::event(self, events).map_err(|e| e.to_string())
    }
    fn click(&mut self, x: i32, y: i32) -> Result<Option<u32>, String> {
        iyakki_tile_host::TileRunner::click(self, x, y).map_err(|e| e.to_string())
    }
    fn frame(&mut self) -> Result<iyakki_tile_abi::DisplayList, String> {
        iyakki_tile_host::TileRunner::frame(self).map_err(|e| e.to_string())
    }
    fn requests(&self) -> Vec<(u32, String, String)> {
        iyakki_tile_host::TileRunner::requests(self)
    }
    fn answer(&self, id: u32, allow: bool) -> bool {
        iyakki_tile_host::TileRunner::answer(self, id, allow)
    }
    fn take_dock_open(&self) -> Option<String> {
        iyakki_tile_host::TileRunner::take_dock_open(self)
    }
    fn takes_app_events(&self) -> bool {
        iyakki_tile_host::TileRunner::takes_app_events(self)
    }
}

/// What the shell takes out of a running tile app's outbox on each tick (the rest of the outbox stays with the runtime).
#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
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
/// A viewer an app mounted inside its page, as the screen paints it (`iyakki.embed`).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TileEmbed {
    pub id: u32,
    pub kind: String,
    /// The target as JSON: an element id string, or `{"x":..,"y":..,"w":..,"h":..}` in page pixels.
    pub target_json: String,
    pub text: String,
    pub hosts: Vec<String>,
    pub profile: String,
    /// The options the app mounted it with, as JSON (a browser viewer's `url`, a media viewer's `file`, a dashboard viewer's `dashboard`).
    pub options_json: String,
}

/// What a media or dashboard viewer shows inside its rectangle: a heading, lines of text and, for a player, how far it has played
/// (`(position, length)` in seconds, the length 0 when unknown). Read by the tile screen (`TileRt::sdk_embed_data`), which draws it.
#[derive(Clone, Debug, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct TileEmbedData {
    pub heading: String,
    pub lines: Vec<String>,
    pub progress: Option<(u64, u64)>,
}

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
    /// Lines for the Info panel: what the person said yes to once (and Revoke takes back) and the money limit. Empty when there is nothing.
    fn sdk_info_lines(&self) -> Vec<String> {
        Vec::new()
    }
    /// The person takes back the first-use answers (Info -> Revoke).
    fn sdk_revoke(&self) {}
    /// The viewers the app has mounted inside its page, oldest first.
    fn sdk_embeds(&self) -> Vec<TileEmbed> {
        Vec::new()
    }
    /// The calls the app made on a viewer (`embed.call`: method and the arguments as JSON), oldest first; taken once.
    fn sdk_embed_calls(&mut self, _id: u32) -> Vec<(String, String)> {
        Vec::new()
    }
    /// What a media viewer (the app's playback state) or a dashboard viewer (the dashboard's data) shows now; kept for a moment, so it is cheap to ask on every frame.
    fn sdk_embed_data(&mut self, _id: u32) -> Option<TileEmbedData> {
        None
    }
}
