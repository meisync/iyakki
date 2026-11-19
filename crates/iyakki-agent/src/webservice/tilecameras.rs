//! The `cameras` domain of a browser tile (F-SDK-166): what the UI SDK's `CameraPort` asks of a tile host, mapped onto the core's camera facet
//! (`iyakki_core_api::Cameras`) for the signed-in person and the tile's own id. The core decides everything that matters: which cameras the
//! person may watch, the consent card of a view, the indicator, the limits, the snapshot's folder and the audit lines. The tile only holds a view
//! id and gets pictures back.
//!
//! A tile reaches only the views it opened itself (the host keeps the list). Consent is the person's: a yes to a card (`decide` with `approve`
//! true) is taken from their page by the web service's approval card (`TileBackend::cameras_call`), never from the tile, because a tile that
//! could say yes for itself would make the core's card empty. A no needs nobody.
//!
//! Calls (arguments are a JSON object): `list`, `open {cameras, size, label}` gives the view, `decide {view, call, approve}`, `state {view}`,
//! `set_visible {view, visible}`, `frame {view, camera, after, format}` gives `{seq, width, height, format, data}` (data in base64) or null when
//! nothing is newer, `overlay {view, camera}`, `snapshot {view, camera}` gives the call of the card, `ptz {view, camera, cmd, pan, tilt, zoom}`,
//! `events {view}`, `close {view}`.

use iyakki_core_api::cameras::{CamEvent, Cameras, FrameFormat, FrameSize, PtzCommand, ViewSpec};
use serde_json::{json, Value};
use std::collections::{BTreeMap, BTreeSet};

/// The largest picture a tile is handed, in base64 text (the tile's reply buffer for this domain is 384 KiB).
pub const MAX_FRAME_TEXT: usize = 360 * 1024;

#[derive(Debug, PartialEq)]
pub enum Req {
    List,
    Open(ViewSpec),
    Decide(String, String, bool),
    State(String),
    SetVisible(String, bool),
    Frame(String, String, u64, FrameFormat),
    Overlay(String, String),
    Snapshot(String, String),
    Ptz(String, String, PtzCommand),
    Events(String),
    Close(String),
}

fn text<'a>(v: &'a Value, k: &str, max: usize) -> Result<&'a str, String> {
    match v.get(k).and_then(Value::as_str) {
        Some(s) if !s.is_empty() && s.len() <= max && !s.contains('\0') => Ok(s),
        _ => Err(format!("\"{k}\" is text of 1 to {max} bytes")),
    }
}

fn view(v: &Value) -> Result<String, String> {
    Ok(text(v, "view", 64)?.to_string())
}

pub fn plan(method: &str, args_json: &str) -> Result<Req, String> {
    let v: Value = serde_json::from_str(if args_json.trim().is_empty() { "{}" } else { args_json }).map_err(|_| "the arguments are not JSON".to_string())?;
    if !v.is_object() {
        return Err("the arguments are a JSON object".into());
    }
    Ok(match method {
        "list" => Req::List,
        "open" => {
            let cameras = match v.get("cameras").and_then(Value::as_array) {
                Some(a) if a.len() <= 8 => a.iter().map(|c| c.as_str().map(str::to_string)).collect::<Option<Vec<_>>>().ok_or("\"cameras\" is a list of camera ids")?,
                _ => return Err("\"cameras\" is a list of camera ids".into()),
            };
            let size = match v.get("size").and_then(Value::as_str).unwrap_or("small") {
                "small" => FrameSize::Small,
                "large" => FrameSize::Large,
                _ => return Err("\"size\" is small or large".into()),
            };
            let label = match v.get("label") {
                None | Some(Value::Null) => String::new(),
                Some(Value::String(l)) if l.len() <= 100 => l.clone(),
                _ => return Err("\"label\" is text of at most 100 bytes".into()),
            };
            Req::Open(ViewSpec { cameras, size, audio: false, label })
        }
        "decide" => Req::Decide(view(&v)?, text(&v, "call", 64)?.to_string(), v.get("approve").and_then(Value::as_bool).ok_or("\"approve\" is true or false")?),
        "state" => Req::State(view(&v)?),
        "set_visible" => Req::SetVisible(view(&v)?, v.get("visible").and_then(Value::as_bool).ok_or("\"visible\" is true or false")?),
        "frame" => {
            let format = match v.get("format").and_then(Value::as_str).unwrap_or("jpeg") {
                "jpeg" => FrameFormat::Jpeg,
                "rgba" => FrameFormat::Rgba,
                _ => return Err("\"format\" is jpeg or rgba".into()),
            };
            Req::Frame(view(&v)?, text(&v, "camera", 64)?.to_string(), v.get("after").and_then(Value::as_u64).unwrap_or(0), format)
        }
        "overlay" => Req::Overlay(view(&v)?, text(&v, "camera", 64)?.to_string()),
        "snapshot" => Req::Snapshot(view(&v)?, text(&v, "camera", 64)?.to_string()),
        "ptz" => {
            let cmd: PtzCommand = serde_json::from_value(v.clone()).map_err(|_| "\"cmd\" is move (with pan, tilt, zoom) or stop".to_string())?;
            Req::Ptz(view(&v)?, text(&v, "camera", 64)?.to_string(), cmd.checked()?)
        }
        "events" => Req::Events(view(&v)?),
        "close" => Req::Close(view(&v)?),
        _ => return Err("that is not a call this tile may make".into()),
    })
}

impl Req {
    fn view(&self) -> Option<&str> {
        match self {
            Req::Decide(s, ..) | Req::State(s) | Req::SetVisible(s, _) | Req::Frame(s, ..) | Req::Overlay(s, _) | Req::Snapshot(s, _) | Req::Ptz(s, ..) | Req::Events(s) | Req::Close(s) => Some(s),
            _ => None,
        }
    }
}

/// What the host remembers for one tile: the views it opened, and the cards it was shown (call id to the words of the card).
#[derive(Default)]
pub struct Own {
    pub views: BTreeSet<String>,
    pub cards: BTreeMap<String, String>,
}

/// The words of the person's card for a yes (the core's summary names cameras, which are device text: cut and cleaned).
pub fn card_words(tile: &str, summary: &str) -> String {
    let clean = |s: &str, n: usize| s.chars().filter(|c| !c.is_control()).take(n).collect::<String>();
    format!("Let {} show a camera: {}", clean(tile, 40), clean(summary, 200))
}

fn json_of<T: serde::Serialize>(v: &T) -> Result<String, String> {
    serde_json::to_string(v).map_err(|_| "the answer was not readable".to_string())
}

/// Run a planned request. A view the tile did not open is the core's own "no such camera view" sentence. `Decide` with `approve` true must have
/// been approved by the person before this is called; this function does not ask.
pub fn run(c: &dyn Cameras, user: &str, app: &str, own: &mut Own, req: &Req) -> Result<String, String> {
    if let Some(s) = req.view() {
        if !own.views.contains(s) {
            return Err("no such camera view".into());
        }
    }
    let ok = || json!({"ok": true}).to_string();
    Ok(match req {
        Req::List => json_of(&c.list(user, app))?,
        Req::Open(spec) => {
            let info = c.open(user, app, spec)?;
            own.views.insert(info.id.clone());
            json_of(&info)?
        }
        Req::Decide(s, call, approve) => {
            c.decide(user, s, call, *approve)?;
            own.cards.remove(call);
            ok()
        }
        Req::State(s) => json_of(&c.state(user, s)?)?,
        Req::SetVisible(s, on) => {
            c.set_visible(user, s, *on)?;
            ok()
        }
        Req::Frame(s, cam, after, fmt) => match c.frame(user, s, cam, *after, *fmt)? {
            None => Value::Null.to_string(),
            Some(f) => {
                let data = iyakki_shared::b64::base64_encode(&f.data);
                if data.len() > MAX_FRAME_TEXT {
                    return Err("that picture is too big for an app of this kind; open the view small".into());
                }
                json!({"seq": f.seq, "width": f.width, "height": f.height, "format": match f.format { FrameFormat::Jpeg => "jpeg", FrameFormat::Rgba => "rgba" }, "data": data}).to_string()
            }
        },
        Req::Overlay(s, cam) => json_of(&c.overlay(user, s, cam)?)?,
        Req::Snapshot(s, cam) => json!({"call": c.snapshot(user, s, cam)?}).to_string(),
        Req::Ptz(s, cam, cmd) => json!({"call": c.ptz(user, s, cam, *cmd)?}).to_string(),
        Req::Events(s) => {
            let events = c.events(user, s);
            for e in &events {
                if let CamEvent::Approval { call, summary, .. } = e {
                    if own.cards.len() < 64 {
                        own.cards.insert(call.clone(), card_words(app, summary));
                    }
                }
            }
            json_of(&events)?
        }
        Req::Close(s) => {
            c.close(user, s);
            own.views.remove(s);
            ok()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::NoCameras;
    use iyakki_core_fake::{FakeCam, FakeCameras};

    #[test]
    fn open_and_the_other_calls_are_checked_before_the_core_is_asked() {
        let Req::Open(s) = plan("open", r#"{"cameras":["front","yard"],"size":"large","label":"Panel"}"#).unwrap() else { panic!() };
        assert_eq!((s.cameras.len(), s.size, s.audio, s.label.as_str()), (2, FrameSize::Large, false, "Panel"));
        for bad in ["{}", r#"{"cameras":"front"}"#, r#"{"cameras":[1]}"#, r#"{"cameras":["a"],"size":"huge"}"#, r#"{"cameras":["a"],"label":5}"#, "[1]", "nope"] {
            assert!(plan("open", bad).is_err(), "{bad}");
        }
        assert_eq!(plan("frame", r#"{"view":"v1","camera":"front"}"#).unwrap(), Req::Frame("v1".into(), "front".into(), 0, FrameFormat::Jpeg));
        assert_eq!(plan("ptz", r#"{"view":"v1","camera":"front","cmd":"move","pan":5,"tilt":-0.5,"zoom":0}"#).unwrap(), Req::Ptz("v1".into(), "front".into(), PtzCommand::Move { pan: 1.0, tilt: -0.5, zoom: 0.0 }));
        assert_eq!(plan("ptz", r#"{"view":"v1","camera":"front","cmd":"stop"}"#).unwrap(), Req::Ptz("v1".into(), "front".into(), PtzCommand::Stop));
        for (m, a) in [("frame", r#"{"view":"v1","camera":"a","format":"png"}"#), ("decide", r#"{"view":"v","call":"c"}"#), ("state", "{}"), ("ptz", r#"{"view":"v","camera":"a","cmd":"spin"}"#), ("set_enabled", "{}"), ("record", r#"{"view":"v"}"#), ("snapshot", r#"{"view":"v"}"#)] {
            assert!(plan(m, a).is_err(), "{m} {a}");
        }
    }

    #[test]
    fn a_core_with_no_camera_views_answers_with_its_sentence_and_a_made_up_view_is_not_the_tiles() {
        let mut own = Own::default();
        let e = run(&NoCameras, "u", "tile", &mut own, &plan("open", r#"{"cameras":["a"]}"#).unwrap()).unwrap_err();
        assert!(e.contains("no camera views"), "{e}");
        assert_eq!(run(&NoCameras, "u", "tile", &mut own, &Req::State("v-1".into())).unwrap_err(), "no such camera view");
        assert_eq!(run(&NoCameras, "u", "tile", &mut own, &Req::List).unwrap(), "[]");
    }

    #[test]
    fn a_tile_watches_a_camera_only_after_the_card_and_never_reaches_the_view_of_another() {
        let core = FakeCameras::new();
        core.add(FakeCam::new("front", "Front door", "amy").with_ptz());
        core.push_solid("front", 4, 2, [10, 20, 30]);
        let mut own = Own::default();
        let call = |own: &mut Own, who: &str, m: &str, a: &str| plan(m, a).and_then(|r| run(&core, who, "tile-a", own, &r));
        assert!(call(&mut own, "bob", "open", r#"{"cameras":["front"]}"#).unwrap_err().contains("not on its list"), "the core's words reach the tile");
        let v: Value = serde_json::from_str(&call(&mut own, "amy", "open", r#"{"cameras":["front"]}"#).unwrap()).unwrap();
        let id = v["id"].as_str().unwrap().to_string();
        let arg = |extra: &str| format!(r#"{{"view":"{id}"{extra}}}"#);
        assert_eq!(call(&mut own, "amy", "frame", &arg(r#","camera":"front""#)).unwrap(), "null", "nothing is pulled before the card");
        assert_eq!(core.pulled("front"), 0);
        let ev: Vec<Value> = serde_json::from_str(&call(&mut own, "amy", "events", &arg("")).unwrap()).unwrap();
        let card = ev.iter().find(|e| e["type"] == "approval").expect("the core asks first");
        let c = card["call"].as_str().unwrap().to_string();
        assert!(own.cards[&c].starts_with("Let tile-a show a camera: tile-a wants to show 1 camera: Front door"), "{:?}", own.cards);
        call(&mut own, "amy", "decide", &arg(&format!(r#","call":"{c}","approve":true"#))).unwrap();
        let st: Value = serde_json::from_str(&call(&mut own, "amy", "state", &arg("")).unwrap()).unwrap();
        assert_eq!(st["indicator"], true);
        let f: Value = serde_json::from_str(&call(&mut own, "amy", "frame", &arg(r#","camera":"front","format":"rgba""#)).unwrap()).unwrap();
        assert_eq!((f["width"].as_u64(), f["format"].as_str()), (Some(4), Some("rgba")));
        assert_eq!(iyakki_shared::b64::b64_decode(f["data"].as_str().unwrap()).unwrap().len(), 32);
        call(&mut own, "amy", "set_visible", &arg(r#","visible":false"#)).unwrap();
        // a snapshot and a move each ask again
        let s: Value = serde_json::from_str(&call(&mut own, "amy", "snapshot", &arg(r#","camera":"front""#)).unwrap()).unwrap();
        assert!(s["call"].is_string());
        // another person, or another tile (its own list), does not reach the view
        assert!(call(&mut own, "bob", "state", &arg("")).is_err());
        let mut other = Own::default();
        assert_eq!(call(&mut other, "amy", "state", &arg("")).unwrap_err(), "no such camera view");
        call(&mut own, "amy", "close", &arg("")).unwrap();
        assert!(call(&mut own, "amy", "state", &arg("")).is_err(), "closed");
    }

    #[test]
    fn the_card_words_are_cleaned_and_short() {
        let w = card_words("tile\u{7}", &format!("tile wants to show {}", "x".repeat(400)));
        assert!(w.starts_with("Let tile show a camera: tile wants to show xxx") && w.len() < 300 && !w.contains('\u{7}'), "{w}");
    }
}
