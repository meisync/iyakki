//! Which microphone and camera a tile uses: the browser's when the person is in a web browser, the machine's otherwise.
//!
//! The rule is here and nowhere else: a session is a browser session exactly when its browser media is registered for the person
//! (`browsermedia::for_user`, set by the web server when the page connects). VOICE gets its [`Recorders`] from [`recorders_for`]
//! and CAMERA asks [`browser_camera_for`]; neither looks at the hardware itself.
//!
//! The permission prompt is the one the media policy already has: the tenant/role policy decides first (default deny; a denied
//! request is audited as `browser.media.denied`), then the page shows its Allow button and only the person's own click switches the
//! microphone or camera on (`BrowserMedia::request`). A tile never switches a browser source on by itself.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::browsermedia::{for_user, BrowserMedia, Kind, Requested, MIC_RATE};
use crate::voicedesk::{Capture, Mic, Recorders, SystemRecorders};

/// The browser media of this person when they are in a web browser; `None` for a session on the machine itself.
pub fn browser_session(user: &str) -> Option<Arc<BrowserMedia>> {
    for_user(user)
}

/// The microphones for VOICE: the browser's in a browser session, the machine's otherwise.
pub fn recorders_for(user: &str) -> Arc<dyn Recorders> {
    match browser_session(user) {
        Some(m) => Arc::new(BrowserRecorders { media: m }),
        None => Arc::new(SystemRecorders),
    }
}

/// The camera for CAMERA: the browser's in a browser session, `None` (use the machine's) otherwise.
pub fn browser_camera_for(user: &str) -> Option<Arc<BrowserMedia>> {
    browser_session(user)
}

/// The camera for the assistant's camera tools, which are not tied to one person: the browser camera of the page that has it on, else
/// of the page that connected last (`None`: no web page, use the machine's).
pub fn browser_camera_for_desktop() -> Option<Arc<BrowserMedia>> {
    crate::browsermedia::camera_session()
}

/// What to tell the person when a tile could not use a browser source (`None`: it is on and can be used).
pub fn ask_words(kind: Kind, r: Requested) -> Option<String> {
    let what = if kind == Kind::Mic { "microphone" } else { "camera" };
    match r {
        Requested::Already => None,
        Requested::Asked => Some(format!("Allow the {what} in your browser: press the red button at the top of the page, then try again.")),
        Requested::Denied => Some(format!("This session may not use the {what}. Ask the administrator.")),
    }
}

pub struct BrowserRecorders {
    media: Arc<BrowserMedia>,
}

impl Recorders for BrowserRecorders {
    fn mics(&self) -> Vec<Mic> {
        vec![Mic { id: "browser".into(), label: "Browser microphone".into(), usb: false }]
    }
    fn product(&self) -> String {
        "web browser".into()
    }
    fn start(&self, _cmd: &str, max_secs: usize) -> Result<Box<dyn Capture>, String> {
        if let Some(why) = ask_words(Kind::Mic, self.media.request(Kind::Mic)) {
            return Err(why);
        }
        let _ = self.media.take_mic(); // sound from before is not part of this recording
        Ok(Box::new(BrowserMic { media: self.media.clone(), max: Duration::from_secs(max_secs as u64), t0: Instant::now(), inner: Mutex::new(Taken::default()) }))
    }
}

#[derive(Default)]
struct Taken {
    samples: Vec<i16>,
    level: f32,
}

/// A recording of the page's microphone (16 kHz mono, as the page sends it).
pub struct BrowserMic {
    media: Arc<BrowserMedia>,
    max: Duration,
    t0: Instant,
    inner: Mutex<Taken>,
}

impl BrowserMic {
    fn pull(&self) {
        let bytes = self.media.take_mic();
        let mut g = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        if bytes.is_empty() {
            g.level *= 0.85;
            return;
        }
        let mut peak = 0i32;
        for c in bytes.chunks_exact(2) {
            let s = i16::from_le_bytes([c[0], c[1]]);
            peak = peak.max((s as i32).abs());
            g.samples.push(s);
        }
        g.level = (peak as f32 / 32768.0).min(1.0);
        let cap = self.max.as_secs() as usize * MIC_RATE as usize;
        if g.samples.len() > cap {
            g.samples.truncate(cap);
        }
    }
}

impl Capture for BrowserMic {
    fn level(&self) -> f32 {
        self.pull();
        self.inner.lock().unwrap_or_else(|e| e.into_inner()).level
    }
    fn elapsed(&self) -> Duration {
        self.t0.elapsed()
    }
    fn is_full(&self) -> bool {
        self.t0.elapsed() >= self.max
    }
    fn is_alive(&mut self) -> bool {
        self.pull();
        self.media.is_on(Kind::Mic)
    }
    fn drain(&mut self) -> Vec<i16> {
        self.pull();
        std::mem::take(&mut self.inner.lock().unwrap_or_else(|e| e.into_inner()).samples)
    }
    fn stop(self: Box<Self>) -> Result<Vec<i16>, String> {
        self.pull();
        let v = std::mem::take(&mut self.inner.lock().unwrap_or_else(|e| e.into_inner()).samples);
        Ok(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browsermedia::{Policy, CH_MIC};

    fn media(p: Policy) -> (Arc<BrowserMedia>, Arc<Mutex<Vec<String>>>) {
        let log = Arc::new(Mutex::new(Vec::new()));
        let l = log.clone();
        (BrowserMedia::new(p, Arc::new(move |a, d| l.lock().unwrap().push(format!("{a} {d}"))), None), log)
    }

    fn pcm(v: &[i16]) -> Vec<u8> {
        let mut m = vec![CH_MIC];
        for s in v {
            m.extend(s.to_le_bytes());
        }
        m
    }

    #[test]
    fn a_session_is_a_browser_session_exactly_when_its_media_is_registered() {
        let (m, _) = media(Policy::allow_all());
        assert!(browser_session("bs-user-a").is_none() && browser_camera_for("bs-user-a").is_none());
        let local = recorders_for("bs-user-a");
        assert_ne!(local.product(), "web browser", "the machine's microphones for a local session");
        crate::browsermedia::register("bs-user-a", m);
        assert_eq!(recorders_for("bs-user-a").product(), "web browser");
        assert!(browser_camera_for("bs-user-a").is_some());
        crate::browsermedia::unregister("bs-user-a");
        assert!(browser_session("bs-user-a").is_none());
    }

    #[test]
    fn the_microphone_asks_first_and_records_only_after_the_persons_click() {
        let (m, log) = media(Policy::allow_all());
        let r = BrowserRecorders { media: m.clone() };
        assert_eq!(r.mics().len(), 1);
        let e = r.start("arecord", 300).err().unwrap();
        assert!(e.contains("Allow the microphone in your browser"), "{e}");
        assert!(m.take_messages().iter().any(|s| s.contains("media_ask") && s.contains("\"mic\"")), "the page is asked to show its Allow button");
        assert!(!m.is_on(Kind::Mic), "a tile never switches it on");
        m.set(Kind::Mic, true).unwrap(); // the person's click in the page
        let mut cap = r.start("arecord", 300).unwrap();
        m.binary(&pcm(&[100, -200, 300]));
        assert!(cap.is_alive());
        assert!(cap.level() > 0.0);
        m.binary(&pcm(&[1, 2]));
        assert_eq!(cap.stop().unwrap(), vec![100, -200, 300, 1, 2]);
        let l = log.lock().unwrap().clone();
        assert!(l.contains(&"browser.media.asked mic".to_string()) && l.contains(&"browser.media.on mic".to_string()));
    }

    #[test]
    fn a_denied_microphone_is_refused_and_audited() {
        let (m, log) = media(Policy::deny_all());
        let e = BrowserRecorders { media: m.clone() }.start("", 300).err().unwrap();
        assert!(e.contains("may not use the microphone"), "{e}");
        assert!(log.lock().unwrap().contains(&"browser.media.denied mic".to_string()));
        assert!(m.take_messages().iter().all(|s| !s.contains("media_ask")), "no Allow button for what the policy forbids");
    }

    #[test]
    fn the_recording_ends_when_the_page_switches_the_microphone_off() {
        let (m, _) = media(Policy::allow_all());
        m.set(Kind::Mic, true).unwrap();
        let mut cap = BrowserRecorders { media: m.clone() }.start("", 300).unwrap();
        m.binary(&pcm(&[5; 10]));
        assert!(cap.level() > 0.0); // the tile looks every tick, which also takes the sound
        m.set(Kind::Mic, false).unwrap();
        assert!(!cap.is_alive(), "the tile sees that the source is gone");
        assert_eq!(cap.stop().unwrap().len(), 10, "what was heard before is kept");
    }

    #[test]
    fn a_recording_never_exceeds_its_maximum() {
        let (m, _) = media(Policy::allow_all());
        m.set(Kind::Mic, true).unwrap();
        let mut cap = BrowserRecorders { media: m.clone() }.start("", 1).unwrap();
        for _ in 0..3 {
            m.binary(&pcm(&vec![1i16; 16_000]));
            cap.is_alive();
        }
        assert_eq!(cap.stop().unwrap().len(), 16_000);
    }
}
