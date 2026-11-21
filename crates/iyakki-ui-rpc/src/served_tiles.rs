//! The running tile apps of the store over the core socket (F-SPLIT-03, D-SPLIT-03-TILES): `StoreApi::tile_open` (an HTML app whose page runs in the
//! core's JavaScript page engine), `tile_open_wasm` (a WebAssembly app in the core's sandbox) and `tile_pictures` (the pictures of a WebAssembly
//! tile's bundle).
//!
//! The program that runs the page is the core; the UI only draws. An HTML app crosses as a remote [`TileRt`]: the page model (`Doc`, serde in
//! `iyakki-web`) comes over when the page changed and every click, key, answer and tick goes back as a call. A WebAssembly app crosses as a remote
//! [`WasmTileRt`]: the tile ABI's events go to the core in their own binary form, the display list comes back in its own binary form (both base64
//! inside the call) and the UI paints it with the host's renderer. Nothing of the UI is used by the core: the web object of the UI leaves the
//! signature (the core's own fetch is used), the person's policy is read again from the core's accounts, and the entry of the app is the one in the
//! core's own list (a folder a UI names for an app that is not listed must lie where some account may read, and is then run as a developer app).
#![allow(non_snake_case)] // the generator names the hand-written entry points c_<Trait>_<method>

use std::sync::{Arc, Mutex, PoisonError};

use iyakki_agent_api::{StoreApi, TileEmbed, TileEmbedData, TileRt, TileTake, WasmTileRt};
use iyakki_sec::policy::Policy;
use iyakki_shared::b64::{b64_decode, base64_encode};
use iyakki_shared::store_tilehub::{TileEntry, DEVELOPER_TRUST};
use iyakki_tile_abi::{DisplayList, Event, EventBatch};
use iyakki_tile_host::ImageTable;
use iyakki_web::Doc;
use serde_json::{json, Value};

use crate::served::{Served, NOT_SERVED};
use crate::{de, ser, Registry, Rpc};

type Remote = crate::RemoteStoreApi;

fn unb64(s: &str) -> Result<Vec<u8>, String> {
    b64_decode(s).ok_or_else(|| "the bytes are not valid base64".to_string())
}

/// A call that did not reach an answer: the core's own sentence when it has the socket's wording already, else the socket's.
fn unreached(e: String) -> String {
    if e.contains(NOT_SERVED) {
        e
    } else {
        format!("{NOT_SERVED}: {e}")
    }
}

fn locked<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

// ---------------------------------------------------------------------------------------------------------------- the entry the core runs

/// The entry the core runs for a claim of the UI: the core's own listing wins; an app that is not listed but names a folder is judged by the file
/// rules and runs as a developer app; anything else is refused.
fn entry_of(t: &Arc<dyn StoreApi>, reg: &Registry<'_>, policy: &Policy, user: &str, claim: TileEntry) -> Result<TileEntry, String> {
    if let Some(listed) = t.tile_listed(policy, user, &claim.key) {
        return Ok(listed);
    }
    let Some(dir) = claim.dir.clone() else { return Err("that app is no longer installed".into()) };
    if claim.name.is_empty() || claim.name.contains(['/', '\\']) || claim.name.contains("..") {
        return Err("that app has no usable name".into());
    }
    let real = reg.project_dir(&dir, false)?;
    Ok(TileEntry { dir: Some(real), dev: true, trust: DEVELOPER_TRUST.to_string(), ..claim })
}

// ---------------------------------------------------------------------------------------------------------------- the UI's end of StoreApi

pub(crate) fn c_StoreApi_tile_open(s: &Remote, _policy: &Policy, user: &str, entry: &TileEntry, _web: Arc<dyn iyakki_web::Fetch>) -> Result<Box<dyn TileRt>, String> {
    let obj = s.rpc.call::<Result<u64, String>>("StoreApi", s.obj, "tile_open", json!([user, entry])).map_err(unreached)??;
    if obj == 0 {
        return Err(format!("{NOT_SERVED}: the core holds too many open objects"));
    }
    Ok(Box::new(RemoteTileRt { rpc: s.rpc.clone(), obj }))
}

pub(crate) fn c_StoreApi_tile_open_wasm(s: &Remote, _policy: &Policy, user: &str, entry: &TileEntry) -> Result<Box<dyn WasmTileRt>, String> {
    let obj = s.rpc.call::<Result<u64, String>>("StoreApi", s.obj, "tile_open_wasm", json!([user, entry])).map_err(unreached)??;
    if obj == 0 {
        return Err(format!("{NOT_SERVED}: the core holds too many open objects"));
    }
    Ok(Box::new(RemoteWasmTile { rpc: s.rpc.clone(), obj, abi3: std::sync::OnceLock::new() }))
}

pub(crate) fn c_StoreApi_tile_pictures(s: &Remote, _policy: &Policy, user: &str, entry: &TileEntry) -> Result<ImageTable, String> {
    let pics = s.rpc.call::<Result<Vec<(u32, i32, i32, String)>, String>>("StoreApi", s.obj, "tile_pictures", json!([user, entry])).map_err(unreached)??;
    let mut table = ImageTable::new();
    for (handle, w, h, rgba) in pics {
        table.put(handle, w, h, unb64(&rgba)?).map_err(|e| e.to_string())?;
    }
    Ok(table)
}

// ---------------------------------------------------------------------------------------------------------------- the core's end of StoreApi

pub(crate) fn s_StoreApi_tile_open(t: &Arc<dyn StoreApi>, reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    let (user, claim): (String, TileEntry) = de(a)?;
    let policy = reg.policy_of(&user)?;
    let web = reg.side().web.unwrap_or_else(|| Arc::new(iyakki_web::fetch::NoNetwork));
    let opened = entry_of(t, reg, &policy, &user, claim).and_then(|entry| t.tile_open(&policy, &user, &entry, web));
    ser(&opened.map(|rt| reg.add(crate::Obj::Served(Arc::new(TileObj { rt: Mutex::new(rt), last: Mutex::new(None) })))))
}

pub(crate) fn s_StoreApi_tile_open_wasm(t: &Arc<dyn StoreApi>, reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    let (user, claim): (String, TileEntry) = de(a)?;
    let policy = reg.policy_of(&user)?;
    let opened = entry_of(t, reg, &policy, &user, claim).and_then(|entry| t.tile_open_wasm(&policy, &user, &entry));
    ser(&opened.map(|rt| reg.add(crate::Obj::Served(Arc::new(WasmTileObj(Mutex::new(rt)))))))
}

pub(crate) fn s_StoreApi_tile_pictures(t: &Arc<dyn StoreApi>, reg: &Registry<'_>, a: Value) -> Result<Value, String> {
    let (user, claim): (String, TileEntry) = de(a)?;
    let policy = reg.policy_of(&user)?;
    let table = entry_of(t, reg, &policy, &user, claim).and_then(|entry| t.tile_pictures(&policy, &user, &entry));
    ser(&table.map(|t| t.iter().map(|(h, p)| (h, p.w, p.h, base64_encode(&p.rgba))).collect::<Vec<(u32, i32, i32, String)>>()))
}

// ---------------------------------------------------------------------------------------------------------------- an HTML app

/// A running HTML tile app of the core. The newest page model it handed out is kept: a click or a key names a control of that page, and the core
/// uses its own copy, never one the UI sends back.
struct TileObj {
    rt: Mutex<Box<dyn TileRt>>,
    last: Mutex<Option<Doc>>,
}

impl Served for TileObj {
    fn iface(&self) -> &'static str {
        "TileRt"
    }
    fn serve(&self, _reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        let mut rt = locked(&self.rt);
        match method {
            "page_text" => ser(&rt.page_text()),
            "doc" => {
                let doc = rt.doc();
                *locked(&self.last) = Some(doc.clone());
                ser(&doc)
            }
            "pump" => ser(&rt.pump()),
            "resolve" => {
                let (id, ok, value): (u32, bool, String) = de(a)?;
                rt.resolve(id, ok, &value);
                ser(&())
            }
            "resolve_open" => {
                let (id, json): (u32, String) = de(a)?;
                rt.resolve_open(id, &json);
                ser(&())
            }
            "click_control" => {
                let (idx,): (usize,) = de(a)?;
                let doc = locked(&self.last).clone().ok_or("the page was not read yet")?;
                ser(&rt.click_control(&doc, idx))
            }
            "type_into" => {
                let (idx, text): (usize, String) = de(a)?;
                let doc = locked(&self.last).clone().ok_or("the page was not read yet")?;
                ser(&rt.type_into(&doc, idx, &text))
            }
            "page_input" => {
                let (node, value): (usize, String) = de(a)?;
                ser(&rt.page_input(node, &value))
            }
            "advance" => de::<(u64,)>(a).and_then(|(ms,)| ser(&rt.advance(ms))),
            "answer_sdk" => {
                let (id, allow): (u32, bool) = de(a)?;
                ser(&rt.answer_sdk(id, allow))
            }
            "console" => ser(&rt.console()),
            "page_notes" => ser(&rt.page_notes()),
            "take" => ser(&rt.take()),
            "drop_request" => {
                let (id,): (u32,) = de(a)?;
                rt.drop_request(id);
                ser(&())
            }
            "refused" => ser(&rt.refused()),
            "sdk_summary" => ser(&rt.sdk_summary()),
            "sdk_info_lines" => ser(&rt.sdk_info_lines()),
            "sdk_revoke" => {
                rt.sdk_revoke();
                ser(&())
            }
            "sdk_embeds" => ser(&rt.sdk_embeds()),
            "sdk_embed_calls" => de::<(u32,)>(a).and_then(|(id,)| ser(&rt.sdk_embed_calls(id))),
            "sdk_embed_data" => de::<(u32,)>(a).and_then(|(id,)| ser(&rt.sdk_embed_data(id))),
            other => Err(format!("TileRt has no call {other}")),
        }
    }
}

/// The UI's end of a running HTML app: every method is a call; a lost core answers the quiet value (an empty page, nothing waiting).
pub struct RemoteTileRt {
    rpc: Rpc,
    obj: u64,
}

impl Drop for RemoteTileRt {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}

impl RemoteTileRt {
    fn ask<T: serde::de::DeserializeOwned>(&self, method: &str, args: Value) -> Option<T> {
        self.rpc.call::<T>("TileRt", self.obj, method, args).ok()
    }
}

impl TileRt for RemoteTileRt {
    fn page_text(&mut self) -> String {
        self.ask("page_text", json!([])).unwrap_or_default()
    }
    fn doc(&mut self) -> Doc {
        self.ask("doc", json!([])).unwrap_or_default()
    }
    fn pump(&mut self) -> bool {
        self.ask("pump", json!([])).unwrap_or(false)
    }
    fn resolve(&mut self, id: u32, ok: bool, value: &str) {
        let _ = self.ask::<()>("resolve", json!([id, ok, value]));
    }
    fn resolve_open(&mut self, id: u32, json: &str) {
        let _ = self.ask::<()>("resolve_open", json!([id, json]));
    }
    fn click_control(&mut self, _doc: &Doc, idx: usize) -> Result<(), String> {
        self.rpc.call::<Result<(), String>>("TileRt", self.obj, "click_control", json!([idx])).map_err(unreached)?
    }
    fn type_into(&mut self, _doc: &Doc, idx: usize, text: &str) -> Result<(), String> {
        self.rpc.call::<Result<(), String>>("TileRt", self.obj, "type_into", json!([idx, text])).map_err(unreached)?
    }
    fn page_input(&mut self, node: usize, value: &str) -> bool {
        self.ask("page_input", json!([node, value])).unwrap_or(false)
    }
    fn advance(&mut self, ms: u64) -> bool {
        self.ask("advance", json!([ms])).unwrap_or(false)
    }
    fn answer_sdk(&mut self, id: u32, allow: bool) -> bool {
        self.ask("answer_sdk", json!([id, allow])).unwrap_or(false)
    }
    fn console(&self) -> Vec<(String, String)> {
        self.ask("console", json!([])).unwrap_or_default()
    }
    fn page_notes(&self) -> Vec<String> {
        self.ask("page_notes", json!([])).unwrap_or_default()
    }
    fn take(&mut self) -> TileTake {
        self.ask("take", json!([])).unwrap_or_default()
    }
    fn drop_request(&mut self, id: u32) {
        let _ = self.ask::<()>("drop_request", json!([id]));
    }
    fn refused(&self) -> Vec<String> {
        self.ask("refused", json!([])).unwrap_or_default()
    }
    fn sdk_summary(&self) -> Option<String> {
        self.ask::<Option<String>>("sdk_summary", json!([])).flatten()
    }
    fn sdk_info_lines(&self) -> Vec<String> {
        self.ask("sdk_info_lines", json!([])).unwrap_or_default()
    }
    fn sdk_revoke(&self) {
        let _ = self.ask::<()>("sdk_revoke", json!([]));
    }
    fn sdk_embeds(&self) -> Vec<TileEmbed> {
        self.ask("sdk_embeds", json!([])).unwrap_or_default()
    }
    fn sdk_embed_calls(&mut self, id: u32) -> Vec<(String, String)> {
        self.ask("sdk_embed_calls", json!([id])).unwrap_or_default()
    }
    fn sdk_embed_data(&mut self, id: u32) -> Option<TileEmbedData> {
        self.ask::<Option<TileEmbedData>>("sdk_embed_data", json!([id])).flatten()
    }
}

// ---------------------------------------------------------------------------------------------------------------- a WebAssembly app

struct WasmTileObj(Mutex<Box<dyn WasmTileRt>>);

fn batch_in(b64: &str) -> Result<Vec<Event>, String> {
    EventBatch::decode(&unb64(b64)?).map(|b| b.events).map_err(|e| format!("the events are not valid: {e:?}"))
}

fn batch_out(events: &[Event]) -> Result<String, String> {
    EventBatch::new(events.to_vec()).encode().map(|b| base64_encode(&b)).map_err(|e| format!("the events cannot be encoded: {e:?}"))
}

impl Served for WasmTileObj {
    fn iface(&self) -> &'static str {
        "WasmTile"
    }
    fn serve(&self, _reg: &Registry<'_>, method: &str, a: Value) -> Result<Value, String> {
        let mut rt = locked(&self.0);
        match method {
            "init" => {
                let (w, h, scale, theme): (i32, i32, u32, String) = de(a)?;
                let theme = batch_in(&theme)?.into_iter().next().ok_or("the theme event is missing")?;
                ser(&rt.init(w, h, scale, theme))
            }
            "event" => {
                let (events,): (String,) = de(a)?;
                ser(&rt.event(&batch_in(&events)?))
            }
            "click" => {
                let (x, y): (i32, i32) = de(a)?;
                ser(&rt.click(x, y))
            }
            "frame" => ser(&rt.frame().and_then(|l| l.encode().map(|b| base64_encode(&b)).map_err(|e| format!("the picture cannot be encoded: {e:?}")))),
            "requests" => ser(&rt.requests()),
            "answer" => {
                let (id, allow): (u32, bool) = de(a)?;
                ser(&rt.answer(id, allow))
            }
            "take_dock_open" => ser(&rt.take_dock_open()),
            "takes_app_events" => ser(&rt.takes_app_events()),
            other => Err(format!("WasmTile has no call {other}")),
        }
    }
}

/// The UI's end of a running WebAssembly app: the events go over in the tile ABI's binary form and the display list comes back in it.
pub struct RemoteWasmTile {
    rpc: Rpc,
    obj: u64,
    abi3: std::sync::OnceLock<bool>,
}

impl Drop for RemoteWasmTile {
    fn drop(&mut self) {
        self.rpc.release(self.obj);
    }
}

impl RemoteWasmTile {
    fn run<T: serde::de::DeserializeOwned>(&self, method: &str, args: Value) -> Result<T, String> {
        self.rpc.call::<Result<T, String>>("WasmTile", self.obj, method, args).map_err(unreached)?
    }
}

impl WasmTileRt for RemoteWasmTile {
    fn init(&mut self, w: i32, h: i32, scale_pct: u32, theme: Event) -> Result<(), String> {
        self.run("init", json!([w, h, scale_pct, batch_out(&[theme])?]))
    }
    fn event(&mut self, events: &[Event]) -> Result<bool, String> {
        if events.is_empty() {
            return Ok(false);
        }
        self.run("event", json!([batch_out(events)?]))
    }
    fn click(&mut self, x: i32, y: i32) -> Result<Option<u32>, String> {
        self.run("click", json!([x, y]))
    }
    fn frame(&mut self) -> Result<DisplayList, String> {
        let text: String = self.run("frame", json!([]))?;
        DisplayList::decode(&unb64(&text)?).map_err(|e| format!("the picture is not valid: {e:?}"))
    }
    fn requests(&self) -> Vec<(u32, String, String)> {
        self.rpc.call("WasmTile", self.obj, "requests", json!([])).unwrap_or_default()
    }
    fn answer(&self, id: u32, allow: bool) -> bool {
        self.rpc.call("WasmTile", self.obj, "answer", json!([id, allow])).unwrap_or(false)
    }
    fn take_dock_open(&self) -> Option<String> {
        self.rpc.call::<Option<String>>("WasmTile", self.obj, "take_dock_open", json!([])).ok().flatten()
    }
    fn takes_app_events(&self) -> bool {
        // a tile's ABI does not change while it runs: asked once
        *self.abi3.get_or_init(|| self.rpc.call("WasmTile", self.obj, "takes_app_events", json!([])).unwrap_or(false))
    }
}
