//! The small kit the page tiles (Settings, Edit, Chat, Runs-like) share, so each tile is only its own state and drawing.
//!
//! * [`App`]: events in, display list out (what every tile is).
//! * [`Source`]: the host's `call(domain, method, json)` import as a trait, so the same tile runs natively against a fake in tests and in the
//!   wasm sandbox against the real service.
//! * [`palette`], [`clip`], [`button`], [`call_json`]: the few drawing and parsing helpers.
//! * [`access`], [`named_button`], [`named_hit`]: put a name, a role and a state into the tile's access tree (ABI 2), so a screen reader hears
//!   the control; a tile that gives none leaves every `Hit` unnamed and the store review lists it.
//! * `export_tile!` (wasm32 only): the six exports of the tile ABI over one [`App`].
pub use iyakki_tile_abi as abi;
use iyakki_tile_abi::{line_height, measure_text, Color, DisplayList, Event, Op, Rect};
use serde_json::Value;

pub trait App {
    /// Returns true when the picture changed.
    fn event(&mut self, e: &Event) -> bool;
    fn frame(&self) -> DisplayList;
}

pub trait Source {
    fn call(&mut self, domain: &str, method: &str, json: &str) -> Result<String, String>;
}

pub struct Palette {
    pub bg: Color,
    pub fg: Color,
    pub mute: Color,
    pub bar: Color,
    pub accent: Color,
    pub bad: Color,
    pub good: Color,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette { bg: Color::rgb(24, 26, 30), fg: Color::rgb(230, 232, 236), mute: Color::rgb(150, 156, 166), bar: Color::rgb(40, 44, 52), accent: Color::rgb(80, 150, 240), bad: Color::rgb(240, 100, 100), good: Color::rgb(90, 200, 130) }
    } else {
        Palette { bg: Color::rgb(245, 245, 245), fg: Color::rgb(30, 30, 30), mute: Color::rgb(100, 100, 100), bar: Color::rgb(225, 228, 232), accent: Color::rgb(40, 110, 220), bad: Color::rgb(200, 40, 40), good: Color::rgb(40, 150, 80) }
    }
}

/// `text` cut to fit `width` pixels at `size`, with "..." when something was cut.
pub fn clip(text: &str, width: i32, size: u32) -> String {
    if measure_text(0, size, text) <= width {
        return text.to_string();
    }
    let mut out = String::new();
    for c in text.chars() {
        let mut t = out.clone();
        t.push(c);
        t.push_str("...");
        if measure_text(0, size, &t) > width {
            break;
        }
        out.push(c);
    }
    out.push_str("...");
    out
}

pub fn text(x: i32, y: i32, size: u32, color: Color, t: &str) -> Op {
    Op::Text { x, y, face: 0, size, color, text: t.into() }
}

/// A rounded button with a centred label, a hit region of this `id` and an access node called by the label (so no shipped tile leaves its buttons unnamed).
pub fn button(l: &mut DisplayList, id: u32, r: Rect, label: &str, p: &Palette) {
    l.push(Op::RoundRect { r, radius: 6, color: p.accent });
    let lw = measure_text(0, 13, label);
    l.push(text(r.x + (r.w - lw) / 2, r.y + (r.h - line_height(13)) / 2, 13, Color::rgb(255, 255, 255), label));
    l.push(Op::Hit { id, r });
    // the screen reader hears the button by its label (an ABI 2 list)
    access(l, id, abi::Role::Button, r, label, "", 0);
}

/// One node of the access tree: what the control is called, what it is, its value and its state (`abi::access_flags`). A text longer than the
/// ABI allows is cut when the list is read, never refused here; a secret field must carry [`abi::access_flags::SECRET`], and its value is
/// then never sent.
pub fn access(l: &mut DisplayList, id: u32, role: abi::Role, r: Rect, name: &str, value: &str, flags: u8) {
    // the ABI refuses a list whose name or value is longer than its limit: cut on a character boundary here, and a secret never carries its value
    let cut = |t: &str| -> String {
        let mut end = t.len().min(abi::limits::MAX_ACCESS_TEXT);
        while !t.is_char_boundary(end) {
            end -= 1;
        }
        t[..end].to_string()
    };
    let value = if flags & abi::access_flags::SECRET != 0 { String::new() } else { cut(value) };
    l.push(Op::Access { id, role, r, name: cut(name), value, flags });
}

/// [`button`] (which now names itself in the access tree); kept for the tiles that already call it by this name.
pub fn named_button(l: &mut DisplayList, id: u32, r: Rect, label: &str, p: &Palette) {
    button(l, id, r, label, p);
}

/// A hit region with its access node, for a control the tile draws itself.
pub fn named_hit(l: &mut DisplayList, id: u32, role: abi::Role, r: Rect, name: &str, value: &str, flags: u8) {
    l.push(Op::Hit { id, r });
    access(l, id, role, r, name, value, flags);
}

/// One call, its answer read as JSON; a failed call or unreadable answer is one plain sentence.
pub fn call_json(src: &mut dyn Source, domain: &str, method: &str, json: &str) -> Result<Value, String> {
    let t = src.call(domain, method, json)?;
    serde_json::from_str(&t).map_err(|_| "the answer was not readable".to_string())
}

/// What a wasm32 tile needs: the host's `call` import and the six exports over an [`App`].
#[cfg(target_arch = "wasm32")]
pub mod guest {
    #![allow(unsafe_code)]
    use super::{App, Source};
    use iyakki_tile_abi::{DisplayList, EventBatch, ABI_VERSION, ABI_VERSION_EVENTS};
    use std::cell::RefCell;

    #[link(wasm_import_module = "iyakki_tile")]
    extern "C" {
        #[link_name = "call"]
        fn host_call(dom_p: i32, dom_l: i32, meth_p: i32, meth_l: i32, json_p: i32, json_l: i32, out_p: i32, out_cap: i32) -> i32;
    }

    /// The host's `call` as a [`Source`] (answers up to 24 KiB, 384 KiB for `cameras`, 128 KiB for `terminal`: a whole session's screen).
    pub struct HostSource;

    impl Source for HostSource {
        fn call(&mut self, domain: &str, method: &str, json: &str) -> Result<String, String> {
            // a picture of a camera view comes as base64 text, which does not fit the usual 24 KiB (the host refuses what would not fit this)
            let mut out = vec![0u8; match domain {
                "cameras" => 384 * 1024,
                "terminal" => 128 * 1024,
                _ => 24 * 1024,
            }];
            let n = unsafe { host_call(domain.as_ptr() as i32, domain.len() as i32, method.as_ptr() as i32, method.len() as i32, json.as_ptr() as i32, json.len() as i32, out.as_mut_ptr() as i32, out.len() as i32) };
            if n < 0 {
                return Err(match n {
                    -2 => "this tile is not allowed to do that".into(),
                    -5 => "the answer was too large".into(),
                    _ => "the service did not answer".into(),
                });
            }
            out.truncate(n as usize);
            String::from_utf8(out).map_err(|_| "the answer was not text".to_string())
        }
    }

    struct Guest {
        app: Option<Box<dyn App>>,
        input: Vec<u8>,
        frame: Vec<u8>,
    }

    thread_local! {
        static GUEST: RefCell<Guest> = const { RefCell::new(Guest { app: None, input: Vec::new(), frame: Vec::new() }) };
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

    /// The ABI this tile speaks: 1, or 3 when it handles app events (feature `app-events`).
    pub const TILE_ABI: u32 = if cfg!(feature = "app-events") { ABI_VERSION_EVENTS } else { ABI_VERSION };

    pub fn abi() -> i32 {
        TILE_ABI as i32
    }

    pub fn alloc(size: i32) -> i32 {
        if !(0..=(iyakki_tile_abi::limits::MAX_EVENT_BYTES as i32)).contains(&size) {
            return 0;
        }
        with(|g| {
            g.input = vec![0; size as usize];
            g.input.as_ptr() as usize as i32
        })
    }

    pub fn init(abi: i32, ptr: i32, len: i32, make: fn(HostSource) -> Box<dyn App>) -> i32 {
        if abi != TILE_ABI as i32 {
            return 1;
        }
        with(|g| match batch(g, ptr, len) {
            Some(b) => {
                let mut app = make(HostSource);
                for e in &b.events {
                    app.event(e);
                }
                g.app = Some(app);
                g.input = Vec::new();
                0
            }
            None => 2,
        })
    }

    pub fn event(ptr: i32, len: i32) -> i32 {
        with(|g| {
            let Some(b) = batch(g, ptr, len) else { return -1 };
            let mut changed = false;
            if let Some(a) = g.app.as_mut() {
                for e in &b.events {
                    changed |= a.event(e);
                }
            }
            g.input = Vec::new();
            i32::from(changed)
        })
    }

    pub fn frame() -> i32 {
        with(|g| {
            let list: DisplayList = g.app.as_ref().map(|a| a.frame()).unwrap_or_default();
            let Ok(bytes) = list.encode() else { return 0 };
            g.frame.clear();
            g.frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            g.frame.extend_from_slice(&bytes);
            g.frame.as_ptr() as usize as i32
        })
    }
}

/// The six exports of tile ABI 1 over the app `$make` builds from the host source: `export_tile!(|src| Box::new(MyTile::new(src)))`.
#[macro_export]
macro_rules! export_tile {
    ($make:expr) => {
        #[no_mangle]
        pub extern "C" fn iyakki_tile_abi() -> i32 {
            $crate::guest::abi()
        }
        #[no_mangle]
        pub extern "C" fn iyakki_alloc(size: i32) -> i32 {
            $crate::guest::alloc(size)
        }
        #[no_mangle]
        pub extern "C" fn iyakki_tile_init(abi: i32, ptr: i32, len: i32) -> i32 {
            $crate::guest::init(abi, ptr, len, $make)
        }
        #[no_mangle]
        pub extern "C" fn iyakki_tile_event(ptr: i32, len: i32) -> i32 {
            $crate::guest::event(ptr, len)
        }
        #[no_mangle]
        pub extern "C" fn iyakki_tile_frame() -> i32 {
            $crate::guest::frame()
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake(Result<String, String>);
    impl Source for Fake {
        fn call(&mut self, _: &str, _: &str, _: &str) -> Result<String, String> {
            self.0.clone()
        }
    }

    #[test]
    fn clipping_fits_the_width_and_says_when_it_cut() {
        assert_eq!(clip("short", 200, 13), "short");
        let c = clip(&"x".repeat(100), 100, 13);
        assert!(c.ends_with("...") && measure_text(0, 13, &c) <= 100, "{c}");
        assert_eq!(clip("anything", 1, 13), "...");
    }

    #[test]
    fn a_json_call_is_a_value_or_one_plain_sentence() {
        assert_eq!(call_json(&mut Fake(Ok("{\"a\":1}".into())), "d", "m", "{}").unwrap()["a"], 1);
        assert_eq!(call_json(&mut Fake(Ok("nope".into())), "d", "m", "{}").unwrap_err(), "the answer was not readable");
        assert_eq!(call_json(&mut Fake(Err("down".into())), "d", "m", "{}").unwrap_err(), "down");
    }

    #[test]
    fn a_button_has_its_label_centred_and_a_hit_region() {
        let mut l = DisplayList::new();
        button(&mut l, 7, Rect::new(10, 10, 80, 24), "Go", &palette(false));
        assert!(l.ops.iter().any(|o| matches!(o, Op::Hit { id: 7, .. })) && l.ops.iter().any(|o| matches!(o, Op::Text { text, .. } if text == "Go")));
        assert_ne!(palette(true).bg, palette(false).bg);
    }

    #[test]
    fn a_named_control_reaches_the_access_tree_and_leaves_no_hit_unnamed_and_a_secret_carries_no_value() {
        let mut l = DisplayList::new();
        named_button(&mut l, 1, Rect::new(10, 10, 80, 24), "Save", &palette(false));
        named_hit(&mut l, 2, abi::Role::Toggle, Rect::new(10, 50, 80, 24), "Dark mode", "on", abi::access_flags::CHECKED);
        named_hit(&mut l, 3, abi::Role::Field, Rect::new(10, 90, 80, 24), "Password", "hunter2", abi::access_flags::SECRET);
        l.push(Op::Hit { id: 4, r: Rect::new(10, 130, 80, 24) });
        let tree = l.access_tree();
        assert_eq!(tree.iter().map(|n| (n.id, n.name.as_str())).collect::<Vec<_>>(), [(1, "Save"), (2, "Dark mode"), (3, "Password")]);
        assert_eq!(tree[1].flags, abi::access_flags::CHECKED);
        assert_eq!(tree[2].value, "", "a secret field never carries its text");
        assert_eq!(l.unnamed_hits(), vec![4], "only the hit nobody named is listed");
        // a long name or value is cut, never refused, and the list still encodes
        let mut long = DisplayList::new();
        named_hit(&mut long, 9, abi::Role::Text, Rect::new(0, 0, 10, 10), &"é".repeat(200), &"x".repeat(500), 0);
        assert!(long.encode().is_ok() && long.access_tree()[0].name.len() <= 120);
        // the list survives the wire in the version that carries access
        let bytes = l.encode().unwrap();
        assert_eq!(DisplayList::decode(&bytes).unwrap().access_tree().len(), 3);
    }
}
