//! The JSON command layer between a host and the engine. The wasm exports (`ffi.rs`) are thin wrappers around this, so everything the page
//! can do is testable natively: a host sends one JSON object per command (`{"c":"pointer","kind":"down","x":1,"y":2}`), feeds text and
//! binary messages, and pops [`Out`](crate::engine::Out) records as JSON lines.

use crate::engine::{Config, Engine};
use crate::sign;
use serde_json::{json, Value};

pub const ENGINE_NAME: &str = "iyakki-client-engine";

#[derive(Default)]
pub struct Host {
    engine: Option<Engine>,
    reply: Vec<u8>,
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
