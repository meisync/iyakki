//! The JSON command layer between a host and the engine. The wasm exports (`ffi.rs`) are thin wrappers around this, so everything the page
//! can do is testable natively: a host sends one JSON object per command (`{"c":"pointer","kind":"down","x":1,"y":2}`), feeds text and
//! binary messages, and pops [`Out`](crate::engine::Out) records as JSON lines.

use crate::engine::{Config, Engine};
use crate::sign;
use crate::uidraw::{self, UiSurface};
use serde_json::{json, Value};

pub const ENGINE_NAME: &str = "iyakki-client-engine";

#[derive(Default)]
pub struct Host {
    engine: Option<Engine>,
    reply: Vec<u8>,
    ui: UiSurface,
}

fn s<'a>(v: &'a Value, k: &str) -> Option<&'a str> {
    v.get(k)?.as_str()
}

fn n(v: &Value, k: &str) -> Option<f64> {
    v.get(k)?.as_f64().filter(|x| x.is_finite())
}

fn b(v: &Value, k: &str) -> bool {
    v.get(k).and_then(Value::as_bool).unwrap_or(false)
}

impl Host {
    pub fn new() -> Host {
        Host::default()
    }

    /// Start (or restart) the engine. `{"token":"..."|null,"public":"hex","name":"...","device":"..."|null,"client":"..."}`.
    pub fn init(&mut self, json: &str) -> bool {
        let Ok(v) = serde_json::from_str::<Value>(json) else { return false };
        let Some(public) = s(&v, "public").filter(|p| p.len() == 64 && sign::unhex(p).is_some()) else { return false };
        let cfg = Config {
            token: s(&v, "token").filter(|t| !t.is_empty()).map(str::to_string),
            public: public.to_string(),
            name: s(&v, "name").unwrap_or("").to_string(),
            device: s(&v, "device").filter(|d| !d.is_empty()).map(str::to_string),
            client: s(&v, "client").unwrap_or(ENGINE_NAME).chars().take(64).collect(),
        };
        self.engine = Some(Engine::new(cfg));
        true
    }

    pub fn engine(&mut self) -> Option<&mut Engine> {
        self.engine.as_mut()
    }

    pub fn on_text(&mut self, t: &str) {
        if let Some(e) = self.engine.as_mut() {
            e.on_text(t);
        }
    }

    pub fn on_binary(&mut self, m: &[u8]) {
        if let Some(e) = self.engine.as_mut() {
            e.on_binary(m);
        }
    }

    /// The next record for the host as one JSON line, left in [`Host::reply`]; returns its length, or 0 when there is none.
    pub fn next_out(&mut self) -> usize {
        self.reply.clear();
        let Some(o) = self.engine.as_mut().and_then(Engine::pop_out) else { return 0 };
        self.reply = serde_json::to_vec(&o).unwrap_or_default();
        self.reply.len()
    }

    pub fn reply(&self) -> &[u8] {
        &self.reply
    }

    fn put(&mut self, v: Value) -> i32 {
        self.reply = v.to_string().into_bytes();
        self.reply.len() as i32
    }

    /// Run one command. Returns the length of the reply left in [`Host::reply`] (0: nothing to read), or -1 for a command that was not
    /// understood or refused. Passwords and seeds are only ever read here, never echoed in a reply.
    pub fn command(&mut self, json: &str) -> i32 {
        self.reply.clear();
        let Ok(v) = serde_json::from_str::<Value>(json) else { return -1 };
        let Some(c) = s(&v, "c") else { return -1 };
        // commands that need no engine
        match c {
            "public_from_seed" => {
                return match s(&v, "seed").and_then(sign::unhex).and_then(|b| <[u8; 32]>::try_from(b).ok()) {
                    Some(seed) => self.put(json!({ "public": sign::hex(&sign::public_from_seed(&seed)) })),
                    None => -1,
                }
            }
            "version" => return self.put(json!({ "engine": ENGINE_NAME, "version": env!("CARGO_PKG_VERSION"), "protocol": iyakki_core_api::client::PROTOCOL_VERSION })),
            c if c.starts_with("ui_") => return self.ui_command(c, &v),
            _ => {}
        }
        let Some(e) = self.engine.as_mut() else { return -1 };
        if c == "state" {
            let (w, h) = e.screen();
            let st = json!({ "phase": e.phase(), "reconnect": e.wants_reconnect(), "device": e.device(), "last_seq": e.last_seq(), "screen": [w, h] });
            return self.put(st);
        }
        let int = |k: &str| n(&v, k).map(|x| x.round() as i32);
        match c {
            "connected" => e.connected(),
            "closed" => e.closed(),
            "bye" => e.bye(),
            "sign_in" => {
                if !e.sign_in(s(&v, "user").unwrap_or(""), s(&v, "password").unwrap_or("")) {
                    return -1;
                }
            }
            "begin_sign" => {
                let (Some(ts), Some(nonce)) = (n(&v, "ts"), s(&v, "nonce")) else { return -1 };
                return match e.begin_sign(ts as u64, nonce) {
                    Some(text) => self.put(json!({ "text": text })),
                    None => -1,
                };
            }
            "finish_sign" => {
                if !e.finish_sign(s(&v, "sig").unwrap_or("")) {
                    return -1;
                }
            }
            "sign_seed" => {
                let (Some(ts), Some(nonce), Some(seed)) = (n(&v, "ts"), s(&v, "nonce"), s(&v, "seed")) else { return -1 };
                if !e.sign_with_seed(seed, ts as u64, nonce) {
                    return -1;
                }
            }
            "call" => {
                let (Some(id), Some(cap)) = (n(&v, "id"), s(&v, "capability")) else { return -1 };
                e.call(id as u64, cap, s(&v, "unit"));
            }
            "ping" => e.ping(n(&v, "ts").unwrap_or(0.0).max(0.0) as u64),
            "size" => {
                let (Some(w), Some(h)) = (int("w"), int("h")) else { return -1 };
                e.size(w.max(0) as u32, h.max(0) as u32);
            }
            "pointer" => {
                let (Some(k), Some(x), Some(y)) = (s(&v, "kind"), int("x"), int("y")) else { return -1 };
                e.pointer(k, x, y);
            }
            "map" => {
                let (Some(x), Some(y), Some(w), Some(h)) = (n(&v, "x"), n(&v, "y"), n(&v, "w"), n(&v, "h")) else { return -1 };
                return match e.map_point(x, y, w, h) {
                    Some((px, py)) => self.put(json!({ "x": px, "y": py })),
                    None => -1,
                };
            }
            "wheel" => e.wheel(int("d").unwrap_or(0)),
            "shift" => e.shift(b(&v, "on")),
            "key" => e.key(s(&v, "key").unwrap_or(""), s(&v, "code").unwrap_or(""), b(&v, "ctrl"), b(&v, "alt"), b(&v, "shift")),
            "text" => e.text(s(&v, "s").unwrap_or("")),
            "full" => e.full(),
            "audio_open" => {
                if !e.audio_open(s(&v, "side").unwrap_or("client")) {
                    return -1;
                }
            }
            "audio_close" => {
                if !e.audio_close() {
                    return -1;
                }
            }
            "clip_put" => {
                if !e.clip_put(s(&v, "text").unwrap_or(""), b(&v, "confirmed")) {
                    return -1;
                }
            }
            "clip_get" => {
                if !e.clip_get(b(&v, "confirmed")) {
                    return -1;
                }
            }
            "camera_list" => {
                if !e.camera_list() {
                    return -1;
                }
            }
            "camera_open" => {
                if !e.camera_open(s(&v, "unit").unwrap_or("")) {
                    return -1;
                }
            }
            "camera_close" => {
                if !e.camera_close(s(&v, "unit").unwrap_or("")) {
                    return -1;
                }
            }
            "app_open" => {
                let (Some(app), Some(w), Some(h)) = (s(&v, "app"), int("w"), int("h")) else { return -1 };
                if !e.app_open(app, w.max(0) as u32, h.max(0) as u32) {
                    return -1;
                }
            }
            "app_input" => {
                if !e.app_input(s(&v, "lines").unwrap_or("")) {
                    return -1;
                }
            }
            "app_close" => {
                if !e.app_close() {
                    return -1;
                }
            }
            "offer_open" => {
                if !e.offer_open(s(&v, "kind").unwrap_or("")) {
                    return -1;
                }
            }
            "offer_close" => {
                if !e.offer_close(s(&v, "kind").unwrap_or("")) {
                    return -1;
                }
            }
            _ => return -1,
        }
        0
    }

    /// The display-list commands (F-SDK-22); they need no connection. `ui_load` reads and draws a list; `ui_hit` finds the action at a
    /// place; `ui_line` makes an input line for the SDK's session. A refused list leaves the old picture and answers `{"error":...}` with the reply's length as a negative number.
    fn ui_command(&mut self, c: &str, v: &Value) -> i32 {
        match c {
            "ui_load" => {
                let scale = n(v, "scale").unwrap_or(1.0) as f32;
                if let Err(e) = self.ui.load(s(v, "text").unwrap_or(""), scale) {
                    // the reason is in the reply; the length comes back negative so a host can tell a refusal from an answer
                    return -self.put(json!({ "error": e }));
                }
                self.ui_reply()
            }
            // the glyph atlas (F-SDK-28): the page draws the glyphs the module lacks and hands them in as alpha bitmaps (hex), then asks for a redraw
            "ui_needs" => {
                let list: Vec<Value> = self.ui.needs().into_iter().map(|(c, size, style)| json!({ "ch": c.to_string(), "size": size, "style": style })).collect();
                self.put(json!({ "glyphs": list, "have": self.ui.glyphs() }))
            }
            "ui_glyph" => {
                let (Some(ch), Some(size), Some(adv), Some(w), Some(h)) = (s(v, "ch").and_then(|t| t.chars().next()), n(v, "size"), n(v, "adv"), n(v, "w"), n(v, "h")) else { return -1 };
                let style = match s(v, "style") {
                    Some("bold") => iyakki_ui_sdk::canvas::TextStyle::Bold,
                    Some("mono") => iyakki_ui_sdk::canvas::TextStyle::Mono,
                    _ => iyakki_ui_sdk::canvas::TextStyle::Regular,
                };
                let hex = s(v, "alpha").unwrap_or("");
                let alpha: Option<Vec<u8>> = (hex.len() % 2 == 0).then(|| (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()).collect::<Option<Vec<u8>>>()).flatten();
                let Some(alpha) = alpha else { return -1 };
                match self.ui.put_glyph(ch, size as u16, style, adv as f32, w as u16, h as u16, n(v, "ox").unwrap_or(0.0) as i16, n(v, "oy").unwrap_or(0.0) as i16, alpha) {
                    Ok(()) => self.put(json!({ "ok": true })),
                    Err(e) => -self.put(json!({ "error": e })),
                }
            }
            "ui_redraw" => {
                if !self.ui.redraw() {
                    return -self.put(json!({ "error": "no display list is loaded" }));
                }
                self.ui_reply()
            }
            "ui_hit" => {
                let (Some(x), Some(y)) = (n(v, "x"), n(v, "y")) else { return -1 };
                let k = self.ui.scale().max(f32::MIN_POSITIVE) as f64;
                match self.ui.action_at((x / k) as f32, (y / k) as f32) {
                    Some(a) => self.put(json!({ "action": a, "line": format!("action {a}") })),
                    None => self.put(json!({ "action": null })),
                }
            }
            "ui_line" => {
                let line = match s(v, "kind") {
                    Some("pointer") => {
                        let (Some(x), Some(y)) = (n(v, "x"), n(v, "y")) else { return -1 };
                        let k = self.ui.scale().max(f32::MIN_POSITIVE) as f64;
                        uidraw::pointer_line(n(v, "id").unwrap_or(0.0).max(0.0) as u32, s(v, "phase").unwrap_or(""), (x / k) as f32, (y / k) as f32, n(v, "t").unwrap_or(0.0).max(0.0) as u64)
                    }
                    Some("key") => uidraw::key_line(s(v, "name").unwrap_or(""), s(v, "char")),
                    Some("text") => Some(uidraw::text_line(s(v, "s").unwrap_or(""))),
                    Some("action") => n(v, "n").map(|a| format!("action {}", a.max(0.0) as u32)),
                    Some("wheel") => n(v, "n").map(|a| format!("wheel {}", a as i32)),
                    Some("resize") => n(v, "w").zip(n(v, "h")).map(|(w, h)| format!("resize {w} {h}")),
                    Some("tick") => n(v, "t").map(|t| format!("tick {}", t.max(0.0) as u64)),
                    _ => None,
                };
                match line {
                    Some(l) => self.put(json!({ "line": l })),
                    None => -1,
                }
            }
            _ => -1,
        }
    }

    /// The reply of `ui_load` and `ui_redraw`: the picture size, the accessibility tree and the text runs the page still has to draw.
    fn ui_reply(&mut self) -> i32 {
                let k = self.ui.scale();
                let list = self.ui.list();
                let a11y: Vec<Value> = list
                    .map(|l| {
                        l.ops
                            .iter()
                            .filter_map(|o| match o {
                                iyakki_ui_sdk::list::Op::Hit(r, h) => Some(json!({ "name": h.name, "role": uidraw::role_name(h.role), "action": h.action, "x": r.x * k, "y": r.y * k, "w": r.w * k, "h": r.h * k })),
                                _ => None,
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                let texts: Vec<Value> = self.ui.texts().iter().map(|t| json!({ "x": t.x, "y": t.y, "size": t.size, "color": t.color, "style": uidraw::style_name(t.style), "text": t.text, "clip": t.clip })).collect();
                let (_, w, h, ver) = self.ui.framebuffer();
                let (lw, lh) = list.map(|l| (l.width, l.height)).unwrap_or((0.0, 0.0));
                self.put(json!({ "w": w, "h": h, "version": ver, "lw": lw, "lh": lh, "scale": k, "a11y": a11y, "texts": texts }))
    }

    /// The display-list picture: `(pointer, width, height, version)`, like [`Host::framebuffer`].
    pub fn ui_framebuffer(&self) -> (*const u8, usize, usize, u64) {
        let (px, w, h, v) = self.ui.framebuffer();
        (px.as_ptr(), w, h, v)
    }

    /// Where the framebuffer is: `(pointer, width, height, version)` for the host to wrap in an `ImageData`. Valid until the next call
    /// that can change the picture.
    pub fn framebuffer(&self) -> (*const u8, usize, usize, u64) {
        match self.engine.as_ref() {
            Some(e) => {
                let (px, w, h, v) = e.framebuffer();
                (px.as_ptr(), w, h, v)
            }
            None => (std::ptr::null(), 0, 0, 0),
        }
    }
}
