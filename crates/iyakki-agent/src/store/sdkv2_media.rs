//! `iyakki.media` and `iyakki.audio` (SDK 1.2): host-side playback control. The app starts, pauses, resumes and stops what the *host* plays; no picture and
//! no sound ever reaches the page. The person's volume and mute always apply (the playback backend plays through the same sound path as the rest of
//! IYAKKI), the speaker is shared by whatever the sound path allows (`Busy` with the holder otherwise), and an app has one audio stream at a time:
//! a new `audio.play` replaces the old one.

use serde_json::{json, Value};

use super::sdkv2::{items_of, path_ok, req_str, req_uint, unknown, with_ignored, Sdk2, WAV_BASE64_CHARS};
use super::tilesdk::{code, SdkError};

pub const MEDIA_ITEMS: usize = 500;

fn bad_id() -> SdkError {
    SdkError::bad("id", "not a playback this app started")
}

impl Sdk2 {
    pub(super) fn media(&self, op: &str, a: &Value) -> Result<Value, SdkError> {
        match op {
            "media.library" => {
                let ignored = unknown(a, &["kind"]);
                let kind = match a.get("kind") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(k)) if k == "audio" || k == "video" => Some(k.clone()),
                    Some(_) => return Err(SdkError::bad("kind", "audio or video")),
                };
                let app = self.app().to_string();
                let lines = vec![format!("“{app}” would like to see the titles of your music and videos (not the files). You are asked once; you can take this back later.")];
                self.ask("media.library", Some("media.library"), "An app wants to see your media list", lines, Box::new(move |s| {
                    let r = s.ba("media.library", &s.be.playback, &json!({"kind": kind}))?;
                    Ok(with_ignored(items_of(r, MEDIA_ITEMS), ignored))
                }))
            }
            "media.play" => self.media_play(a),
            "media.pause" | "media.resume" | "media.stop" => {
                let ignored = unknown(a, &["id"]);
                let id = self.own_media(a)?;
                let r = self.ba(op, &self.be.playback, &json!({"id": id}))?;
                if op == "media.stop" {
                    self.forget_media(id);
                }
                Ok(with_ignored(json!({"id": id, "state": r.get("state").cloned().unwrap_or_else(|| json!(op.trim_start_matches("media.")))}), ignored))
            }
            "media.state" => {
                let ignored = unknown(a, &["id"]);
                let id = match a.get("id") {
                    None | Some(Value::Null) => None,
                    Some(_) => Some(self.own_media(a)?),
                };
                let r = self.ba("media.state", &self.be.playback, &json!({"id": id}))?;
                Ok(with_ignored(json!({"id": id, "state": r.get("state").cloned().unwrap_or(json!("stopped")), "position": r.get("position").cloned().unwrap_or(json!(0))}), ignored))
            }
            "media.queue.add" => {
                let ignored = unknown(a, &["id"]);
                let id = req_str(a, "id", 120)?.to_string();
                let app = self.app().to_string();
                self.ask("media.queue.add", None, "An app wants to add to the play queue", vec![format!("“{app}” would like to add an item from your media list to the play queue.")], Box::new(move |s| {
                    let r = s.ba("media.queue.add", &s.be.playback, &json!({"id": id}))?;
                    Ok(with_ignored(json!({"queued": r.get("queued").cloned().unwrap_or(json!(1))}), ignored))
                }))
            }
            _ => {
                let ignored = unknown(a, &[]);
                let r = self.ba("media.queue.list", &self.be.playback, &json!({}))?;
                Ok(with_ignored(items_of(r, MEDIA_ITEMS), ignored))
            }
        }
    }

    fn media_play(&self, a: &Value) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["id", "file", "start"]);
        let (id, file) = (a.get("id").filter(|v| !v.is_null()), a.get("file").filter(|v| !v.is_null()));
        let start = super::sdkv2::opt_uint(a, "start", 0, 86_400)?.unwrap_or(0);
        let app = self.app().to_string();
        match (id, file) {
            (Some(_), None) => {
                let id = req_str(a, "id", 120)?.to_string();
                let r = self.ba("media.play", &self.be.playback, &json!({"id": id, "start": start}))?;
                self.media_started(&r, ignored)
            }
            (None, Some(_)) => {
                let f = req_str(a, "file", 300)?.to_string();
                if f.is_empty() || f.chars().any(|c| c.is_control()) {
                    return Err(SdkError::bad("file", "a file name"));
                }
                let inside = self.ba("media.in_folders", &self.be.playback, &json!({"file": f}))?;
                let run = move |s: &Sdk2| {
                    let r = s.ba("media.play", &s.be.playback, &json!({"file": f, "start": start}))?;
                    s.media_started(&r, ignored)
                };
                if inside.get("inside").and_then(Value::as_bool) == Some(true) {
                    return run(self);
                }
                let lines = vec![format!("“{app}” would like to play a file that is not in your media folders."), "You are asked every time for such a file.".into()];
                self.ask("media.play", None, "An app wants to play a file", lines, Box::new(run))
            }
            _ => Err(SdkError::bad("id", "give either id (from media.library) or file, not both and not neither")),
        }
    }

    fn media_started(&self, r: &Value, ignored: Vec<String>) -> Result<Value, SdkError> {
        let id = r.get("id").and_then(Value::as_u64).ok_or_else(|| SdkError::new(code::FAILED, "the player gave no playback"))? as u32;
        self.media_add(id);
        Ok(with_ignored(json!({"id": id}), ignored))
    }

    fn own_media(&self, a: &Value) -> Result<u32, SdkError> {
        let id = req_uint(a, "id")? as u32;
        if !self.media_has(id) {
            return Err(bad_id());
        }
        Ok(id)
    }

    // ---- audio

    pub(super) fn audio(&self, op: &str, a: &Value) -> Result<Value, SdkError> {
        match op {
            "audio.play" => self.audio_play(a),
            "audio.outputs" => {
                let ignored = unknown(a, &[]);
                let r = self.ba("audio.outputs", &self.be.playback, &json!({}))?;
                Ok(with_ignored(items_of(r, 50), ignored))
            }
            _ => {
                let ignored = unknown(a, &["id", "volume"]);
                let id = req_uint(a, "id")? as u32;
                if self.audio_id() != Some(id) {
                    return Err(bad_id());
                }
                let mut args = json!({"id": id});
                if op == "audio.setVolume" {
                    let v = a.get("volume").and_then(Value::as_f64).filter(|v| (0.0..=1.0).contains(v)).ok_or_else(|| SdkError::bad("volume", "a number from 0 to 1"))?;
                    args["volume"] = json!(v);
                }
                self.ba(op, &self.be.playback, &args)?;
                if op == "audio.stop" {
                    self.set_audio(None);
                }
                Ok(with_ignored(json!({"id": id}), ignored))
            }
        }
    }

    fn audio_play(&self, a: &Value) -> Result<Value, SdkError> {
        let ignored = unknown(a, &["file", "wav", "url", "volume", "loop", "channel"]);
        if a.get("url").is_some_and(|v| !v.is_null()) {
            return Err(SdkError::new(code::UNSUPPORTED, "audio from an internet address is not available in SDK 1.2: bundle the sound in the app (file) or send it as wav").with("op", "audio.play").with("namespace", "audio").with("sdk", iyakki_core_api::sdkbridge::SDK_VERSION));
        }
        let src = match (a.get("file").filter(|v| !v.is_null()), a.get("wav").filter(|v| !v.is_null())) {
            (Some(_), None) => {
                let f = req_str(a, "file", 100)?;
                let lower = f.to_ascii_lowercase();
                if !path_ok(f) || !(lower.ends_with(".wav") || lower.ends_with(".mp3")) {
                    return Err(SdkError::bad("file", "a .wav or .mp3 file inside the app"));
                }
                json!({"file": f})
            }
            (None, Some(_)) => {
                let w = req_str(a, "wav", WAV_BASE64_CHARS)?;
                if w.is_empty() || !w.chars().all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c)) {
                    return Err(SdkError::bad("wav", "base64 text of a WAV file up to 2 MB"));
                }
                json!({"wav": w})
            }
            _ => return Err(SdkError::bad("file", "give either file (inside the app) or wav (base64), not both and not neither")),
        };
        let volume = match a.get("volume") {
            None | Some(Value::Null) => 1.0,
            Some(v) => v.as_f64().filter(|v| (0.0..=1.0).contains(v)).ok_or_else(|| SdkError::bad("volume", "a number from 0 to 1"))?,
        };
        let looped = match a.get("loop") {
            None | Some(Value::Null) => false,
            Some(v) => v.as_bool().ok_or_else(|| SdkError::bad("loop", "true or false"))?,
        };
        let channel = match a.get("channel").and_then(Value::as_str) {
            None => "music",
            Some(c @ ("cue" | "music" | "speech")) => c,
            Some(_) => return Err(SdkError::bad("channel", "cue, music or speech")),
        }
        .to_string();
        let app = self.app().to_string();
        let lines = vec![format!("“{app}” would like to play sound on this computer. Your volume and mute settings always apply."), "You are asked once; you can take this back later.".into()];
        self.ask("audio.play", Some("audio.play"), "An app wants to play sound", lines, Box::new(move |s| {
            if let Some(old) = s.audio_id() {
                let _ = s.ba("audio.stop", &s.be.playback, &json!({"id": old}));
                s.set_audio(None);
            }
            let r = s.ba("audio.play", &s.be.playback, &json!({"src": src, "volume": volume, "loop": looped, "channel": channel, "app": s.app()}))?;
            let id = r.get("id").and_then(Value::as_u64).ok_or_else(|| SdkError::new(code::FAILED, "the sound path gave no stream"))? as u32;
            s.set_audio(Some(id));
            Ok(with_ignored(json!({"id": id}), ignored))
        }))
    }
}
