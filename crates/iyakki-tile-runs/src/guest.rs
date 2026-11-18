//! The wasm32 build: the six exports of tile ABI 1 over one [`RunsTile`] whose source is the host's `call` import. The only `unsafe` is the
//! one call into that import (the host checks every pointer against this module's memory).
#![allow(unsafe_code)]
use crate::{RunsTile, Source};
use iyakki_tile_abi::{DisplayList, EventBatch, ABI_VERSION};
use std::cell::RefCell;

#[link(wasm_import_module = "iyakki_tile")]
extern "C" {
    #[link_name = "call"]
    fn host_call(dom_p: i32, dom_l: i32, meth_p: i32, meth_l: i32, json_p: i32, json_l: i32, out_p: i32, out_cap: i32) -> i32;
}

struct Host;

impl Source for Host {
    fn call(&mut self, method: &str, json: &str) -> Result<String, String> {
        let dom = "runs";
        let mut out = vec![0u8; 24 * 1024];
        let n = unsafe { host_call(dom.as_ptr() as i32, dom.len() as i32, method.as_ptr() as i32, method.len() as i32, json.as_ptr() as i32, json.len() as i32, out.as_mut_ptr() as i32, out.len() as i32) };
        if n < 0 {
            return Err(match n {
                -2 => "this tile is not allowed to read the runs".into(),
                -5 => "the answer was too large".into(),
                _ => "the service did not answer".into(),
            });
        }
        out.truncate(n as usize);
        String::from_utf8(out).map_err(|_| "the answer was not text".to_string())
    }
}

struct Guest {
    tile: Option<RunsTile<Host>>,
    input: Vec<u8>,
    frame: Vec<u8>,
}

thread_local! {
    static GUEST: RefCell<Guest> = const { RefCell::new(Guest { tile: None, input: Vec::new(), frame: Vec::new() }) };
}

fn with<R>(f: impl FnOnce(&mut Guest) -> R) -> R {
    GUEST.with(|g| f(&mut g.borrow_mut()))
}

fn batch(g: &Guest, ptr: i32, len: i32) -> Option<EventBatch> {
    if ptr as usize != g.input.as_ptr() as usize || len < 0 || len as usize > g.input.len() {
        return None;
    }
    EventBatch::decode(&g.input[..len as usize]).ok()
}

#[no_mangle]
pub extern "C" fn iyakki_tile_abi() -> i32 {
    ABI_VERSION as i32
}

#[no_mangle]
pub extern "C" fn iyakki_alloc(size: i32) -> i32 {
    if !(0..=(iyakki_tile_abi::limits::MAX_EVENT_BYTES as i32)).contains(&size) {
        return 0;
    }
    with(|g| {
        g.input = vec![0; size as usize];
        g.input.as_ptr() as usize as i32
    })
}

#[no_mangle]
pub extern "C" fn iyakki_tile_init(abi: i32, ptr: i32, len: i32) -> i32 {
    if abi != ABI_VERSION as i32 {
        return 1;
    }
    with(|g| match batch(g, ptr, len) {
        Some(b) => {
            let mut t = RunsTile::new(Host);
            for e in &b.events {
                t.event(e);
            }
            g.tile = Some(t);
            g.input = Vec::new();
            0
        }
        None => 2,
    })
}

#[no_mangle]
pub extern "C" fn iyakki_tile_event(ptr: i32, len: i32) -> i32 {
    with(|g| {
        let Some(b) = batch(g, ptr, len) else { return -1 };
        let mut changed = false;
        if let Some(t) = g.tile.as_mut() {
            for e in &b.events {
                changed |= t.event(e);
            }
        }
        g.input = Vec::new();
        i32::from(changed)
    })
}

#[no_mangle]
pub extern "C" fn iyakki_tile_frame() -> i32 {
    with(|g| {
        let list: DisplayList = match g.tile.as_ref() {
            Some(t) => t.frame(),
            None => DisplayList::new(),
        };
        let Ok(bytes) = list.encode() else { return 0 };
        g.frame.clear();
        g.frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        g.frame.extend_from_slice(&bytes);
        g.frame.as_ptr() as usize as i32
    })
}
