//! Person detection: a hook, not a detector.
//!
//! This installation has no person detector. A heuristic on shapes or motion would only guess, and a notice that says "person" when it
//! was a dog, a shadow or a curtain is worse than no notice. So the default implementation, [`NotAvailable`], says so, and events carry
//! "motion" only. Nothing here sends a picture anywhere.
//!
//! The route for the real thing is the data-science toolkit (`docs/development/DATA-SCIENCE-ADDON-PLAN.md`): an isolated job on the
//! person's own k3s cluster receives ONE picture and returns boxes. The agent, not the job, decides when to ask (only for an event that
//! already passed the motion detector, only with the visible area of the picture, privacy masks already black), and only after the
//! person turned person detection on. The request and the answer are plain JSON, versioned, and checked on the way back:
//!
//! ```text
//! request  {"v":1,"task":"person","min_confidence":0.5,
//!           "image":{"format":"jpeg","width":640,"height":360,"data_b64":"..."}}
//! answer   {"v":1,"boxes":[{"x":0.41,"y":0.20,"w":0.12,"h":0.55,"confidence":0.91,"label":"person"}]}
//! ```
//!
//! Box coordinates are fractions of the picture (0.0 to 1.0, `x`/`y` the top left corner). The answer may hold at most 50 boxes; boxes
//! outside the picture, with a confidence outside 0 to 1, or that are not numbers make the whole answer invalid. Boxes below
//! `min_confidence` or labelled anything but "person" are ignored. An error answer is `{"v":1,"error":"words"}`.
//!
//! Not built, and not run here: the job itself, the transport to it, the cluster. [`FakePersonDetector`] stands in for tests.

use std::sync::Mutex;

pub const MAX_BOXES: usize = 50;
pub const REQUEST_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq)]
pub struct PersonBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub confidence: f32,
}

/// A picture handed to a detector: JPEG bytes of the visible area (masks already black) and their size.
pub struct Shot<'a> {
    pub width: u32,
    pub height: u32,
    pub jpeg: &'a [u8],
}

pub trait PersonDetector: Send + Sync {
    /// `Ok` when the detector can be asked; the error says why not, in words for the person.
    fn available(&self) -> Result<(), String>;
    /// Boxes of people in the picture (empty: none found).
    fn detect(&self, shot: &Shot<'_>, min_confidence: f32) -> Result<Vec<PersonBox>, String>;
}

/// The default: there is no person detector.
pub struct NotAvailable;

pub const NOT_AVAILABLE: &str = "Person detection is not available on this computer. Motion is detected locally; recognising a person needs a model that runs as an isolated job on your own cluster (see the data-science add-on), which is not set up.";

impl PersonDetector for NotAvailable {
    fn available(&self) -> Result<(), String> {
        Err(NOT_AVAILABLE.into())
    }
    fn detect(&self, _: &Shot<'_>, _: f32) -> Result<Vec<PersonBox>, String> {
        Err(NOT_AVAILABLE.into())
    }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 with padding (the same code as the agent's `story::base64_encode`; this crate has no `iyakki-*` dependency for it).
fn base64_encode(data: &[u8]) -> String {
    let mut o = String::with_capacity(data.len().div_ceil(3) * 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                o.push(B64[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                o.push('=');
            }
        }
    }
    o
}

/// The request for the job, as JSON text.
pub fn request_json(shot: &Shot<'_>, min_confidence: f32) -> String {
    format!(
        "{{\"v\":{REQUEST_VERSION},\"task\":\"person\",\"min_confidence\":{:.3},\"image\":{{\"format\":\"jpeg\",\"width\":{},\"height\":{},\"data_b64\":\"{}\"}}}}",
        min_confidence.clamp(0.0, 1.0),
        shot.width,
        shot.height,
        base64_encode(shot.jpeg)
    )
}

/// Check and read the job's answer.
pub fn parse_answer(text: &str, min_confidence: f32) -> Result<Vec<PersonBox>, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|_| "the person detector's answer is not JSON".to_string())?;
    if v.get("v").and_then(|x| x.as_u64()) != Some(REQUEST_VERSION as u64) {
        return Err("the person detector answered in a version this computer does not know".into());
    }
    if let Some(e) = v.get("error").and_then(|x| x.as_str()) {
        return Err(format!("the person detector said: {}", e.chars().filter(|c| !c.is_control()).take(200).collect::<String>()));
    }
    let boxes = v.get("boxes").and_then(|b| b.as_array()).ok_or("the person detector's answer has no list of boxes")?;
    if boxes.len() > MAX_BOXES {
        return Err("the person detector returned too many boxes".into());
    }
    let mut out = Vec::new();
    for b in boxes {
        let num = |k: &str| b.get(k).and_then(|x| x.as_f64()).filter(|n| n.is_finite()).map(|n| n as f32);
        let (Some(x), Some(y), Some(w), Some(h), Some(c)) = (num("x"), num("y"), num("w"), num("h"), num("confidence")) else {
            return Err("a box in the answer is missing a number".into());
        };
        if x < 0.0 || y < 0.0 || w <= 0.0 || h <= 0.0 || x + w > 1.0001 || y + h > 1.0001 || !(0.0..=1.0).contains(&c) {
            return Err("a box in the answer lies outside the picture".into());
        }
        if b.get("label").and_then(|l| l.as_str()).is_some_and(|l| l != "person") || c < min_confidence {
            continue;
        }
        out.push(PersonBox { x, y, w, h, confidence: c });
    }
    Ok(out)
}

/// A scripted detector for tests: answers in turn with what it was given, and remembers what it was asked.
pub struct FakePersonDetector {
    answers: Mutex<Vec<Result<Vec<PersonBox>, String>>>,
    pub asked: Mutex<Vec<(u32, u32, usize)>>,
}

impl FakePersonDetector {
    pub fn new(answers: Vec<Result<Vec<PersonBox>, String>>) -> FakePersonDetector {
        FakePersonDetector { answers: Mutex::new(answers), asked: Mutex::new(vec![]) }
    }
}

impl PersonDetector for FakePersonDetector {
    fn available(&self) -> Result<(), String> {
        Ok(())
    }
    fn detect(&self, shot: &Shot<'_>, _: f32) -> Result<Vec<PersonBox>, String> {
        self.asked.lock().unwrap_or_else(|e| e.into_inner()).push((shot.width, shot.height, shot.jpeg.len()));
        let mut a = self.answers.lock().unwrap_or_else(|e| e.into_inner());
        if a.is_empty() { Ok(vec![]) } else { a.remove(0) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_says_it_is_not_available_and_never_guesses() {
        let d = NotAvailable;
        assert!(d.available().unwrap_err().contains("not available"));
        let shot = Shot { width: 8, height: 8, jpeg: &[0xFF, 0xD8] };
        assert!(d.detect(&shot, 0.5).is_err(), "no heuristic stands in for a model");
    }

    #[test]
    fn the_request_is_versioned_json_with_the_picture_in_base64() {
        let shot = Shot { width: 640, height: 360, jpeg: &[0xFF, 0xD8, 0xFF, 0xD9] };
        let t = request_json(&shot, 0.5);
        let v: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert_eq!((v["v"].as_u64(), v["task"].as_str()), (Some(1), Some("person")));
        assert_eq!((v["image"]["width"].as_u64(), v["image"]["height"].as_u64(), v["image"]["format"].as_str()), (Some(640), Some(360), Some("jpeg")));
        assert_eq!(v["image"]["data_b64"].as_str(), Some("/9j/2Q=="));
        assert!((v["min_confidence"].as_f64().unwrap() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn answers_are_checked_before_they_are_believed() {
        let ok = r#"{"v":1,"boxes":[{"x":0.4,"y":0.2,"w":0.1,"h":0.5,"confidence":0.9,"label":"person"},{"x":0.1,"y":0.1,"w":0.1,"h":0.1,"confidence":0.3,"label":"person"},{"x":0.5,"y":0.5,"w":0.1,"h":0.1,"confidence":0.99,"label":"dog"}]}"#;
        let b = parse_answer(ok, 0.5).unwrap();
        assert_eq!(b.len(), 1, "low confidence and other labels are ignored");
        assert!((b[0].confidence - 0.9).abs() < 1e-6);
        assert!(parse_answer(r#"{"v":1,"boxes":[]}"#, 0.5).unwrap().is_empty());
        for bad in [
            "not json",
            r#"{"v":2,"boxes":[]}"#,
            r#"{"v":1}"#,
            r#"{"v":1,"boxes":[{"x":0.9,"y":0.2,"w":0.3,"h":0.5,"confidence":0.9}]}"#,
            r#"{"v":1,"boxes":[{"x":-0.1,"y":0.2,"w":0.3,"h":0.5,"confidence":0.9}]}"#,
            r#"{"v":1,"boxes":[{"x":0.1,"y":0.2,"w":0.3,"h":0.5,"confidence":1.5}]}"#,
            r#"{"v":1,"boxes":[{"x":0.1,"y":0.2,"w":0.3,"h":0.5}]}"#,
            r#"{"v":1,"boxes":[{"x":"a","y":0.2,"w":0.3,"h":0.5,"confidence":0.9}]}"#,
        ] {
            assert!(parse_answer(bad, 0.5).is_err(), "{bad}");
        }
        let many = format!("{{\"v\":1,\"boxes\":[{}]}}", vec![r#"{"x":0.1,"y":0.1,"w":0.1,"h":0.1,"confidence":0.9}"#; 51].join(","));
        assert!(parse_answer(&many, 0.5).unwrap_err().contains("too many"));
        let e = parse_answer("{\"v\":1,\"error\":\"model\\nnot loaded\"}", 0.5).unwrap_err();
        assert!(e.contains("model") && !e.contains('\n'));
    }

    #[test]
    fn the_fake_detector_answers_in_turn_and_remembers_what_it_was_asked() {
        let f = FakePersonDetector::new(vec![Ok(vec![PersonBox { x: 0.1, y: 0.1, w: 0.2, h: 0.6, confidence: 0.8 }]), Err("busy".into())]);
        let shot = Shot { width: 160, height: 90, jpeg: &[1, 2, 3] };
        assert_eq!(f.detect(&shot, 0.5).unwrap().len(), 1);
        assert_eq!(f.detect(&shot, 0.5).unwrap_err(), "busy");
        assert!(f.detect(&shot, 0.5).unwrap().is_empty());
        assert_eq!(f.asked.lock().unwrap().len(), 3);
    }
}
