//! The wasm32 build of the Rust counter: the six exports of tile ABI 1 (docs/development/TILE-ABI.md section 1) over one [`Counter`]
//! kept in thread-local state, with no `unsafe` and no host imports.
//!
//! The host asks for a buffer with `iyakki_alloc`, writes an event batch into it and calls `iyakki_tile_event` with the same pointer: the
//! batch is read back from the buffer this module handed out, never from an arbitrary address, so a wrong pointer is refused (-1)
//! instead of read. The frame is kept as `[u32 LE length][display list]` until the next call.
use crate::Counter;
use iyakki_tile_abi::{DisplayList, EventBatch, ABI_VERSION};
use std::cell::RefCell;

struct Guest {
    counter: Counter,
    input: Vec<u8>,
    frame: Vec<u8>,
}

thread_local! {
    static GUEST: RefCell<Guest> = RefCell::new(Guest { counter: Counter::new(), input: Vec::new(), frame: Vec::new() });
}

fn with<R>(f: impl FnOnce(&mut Guest) -> R) -> R {
    GUEST.with(|g| f(&mut g.borrow_mut()))
}

/// The batch the host wrote into the buffer handed out last, if `ptr` and `len` are that buffer.
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

/// A writable buffer of `size` bytes for the next event batch, or 0 when the size is not sane (the batch limit is 64 KiB).
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
            g.counter = Counter::new();
            for e in &b.events {
                g.counter.event(e);
            }
            // the input heap is recycled at the end of init
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
        for e in &b.events {
            changed |= g.counter.event(e);
        }
        g.input = Vec::new();
        i32::from(changed)
    })
}

/// A pointer to `[u32 LE length][display list]`; 0 when the list cannot be encoded.
#[no_mangle]
pub extern "C" fn iyakki_tile_frame() -> i32 {
    with(|g| {
        let list: DisplayList = g.counter.frame();
        let Ok(bytes) = list.encode() else { return 0 };
        g.frame.clear();
        g.frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        g.frame.extend_from_slice(&bytes);
        g.frame.as_ptr() as usize as i32
    })
}
