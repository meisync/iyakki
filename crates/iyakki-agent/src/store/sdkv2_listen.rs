//! What `iyakki.speech.listen` does on the machine (F-SDK-16): the backend behind the bridge's consent card. The bridge (`sdkv2.rs`) has
//! already asked the person, limited the uses and written the audit line; this backend only does the work.
//!
//! * [`Capture`] is the microphone (a named capture device; a USB or non-T2 one) and [`Recogniser`] is the voice engine. Both are
//!   traits so the whole path is tested with fakes; the real ones are wired by the shell when a microphone and an engine exist.
//! * `speech.available` answers the microphone's name, or `Unavailable` with a fix: on a T2 laptop (capture is not proven safe
//!   there), without a microphone, or without an engine. Nobody is asked to allow what cannot work.
//! * `speech.listen` records on a thread of its own for at most the asked seconds, hands the recording to the engine, **throws the
//!   sound away** (overwritten, then dropped) and reports the text only, through the sink the bridge gave ([`ListenBackend::set_sink`]).
//!   The app never gets sound, and the text is not logged here.
//! * `speech.cancel` stops the recording at once and reports nothing (the bridge already told the app it was cancelled).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

use super::sdkv2_backends::{Backend, Who};
use super::tilesdk::{code, SdkError};

/// 16 kHz mono signed 16-bit samples, the form the voice engine reads.
pub type Pcm = Vec<i16>;

pub trait Capture: Send + Sync {
    /// The microphone's name for the person's card, or why there is none.
    fn microphone(&self, device: Option<&str>) -> Result<String, String>;
    /// Records until `max_seconds` have passed or `stop` is set (then returns what it has).
    fn record(&self, device: Option<&str>, max_seconds: u64, stop: &AtomicBool) -> Result<Pcm, String>;
}

pub trait Recogniser: Send + Sync {
    /// `Ok` when an engine (and its voice pack) is installed and can run, else what to do.
    fn ready(&self) -> Result<(), String>;
    /// The words in `pcm`; an empty text means nothing could be made out.
    fn transcribe(&self, pcm: &[i16], lang: Option<&str>) -> Result<String, String>;
}

/// What the bridge gives the backend to deliver the text: the handle and the text, or the error the app should see.
pub type Sink = Arc<dyn Fn(u32, Result<String, SdkError>) + Send + Sync>;

type Active = Arc<Mutex<Option<(u32, Arc<AtomicBool>)>>>;

pub struct ListenBackend {
    capture: Arc<dyn Capture>,
    engine: Arc<dyn Recogniser>,
    /// Capture on this computer is not proven safe (T2 laptops): always `Unavailable`.
    blocked: Option<&'static str>,
    sink: Mutex<Option<Sink>>,
    /// The handle that is recording and the flag that stops it.
    active: Active,
}

impl ListenBackend {
    pub fn new(capture: Arc<dyn Capture>, engine: Arc<dyn Recogniser>) -> ListenBackend {
        ListenBackend { capture, engine, blocked: None, sink: Mutex::new(None), active: Arc::new(Mutex::new(None)) }
    }

    /// A computer where capture is not allowed (the T2 laptops: the microphone path there is not proven safe). The line is the fix.
    pub fn blocked(mut self, why: &'static str) -> ListenBackend {
        self.blocked = Some(why);
        self
    }

    pub fn set_sink(&self, sink: Sink) {
        *self.sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(sink);
    }

    /// True while a recording or recognition is running.
    pub fn busy(&self) -> bool {
        self.active.lock().unwrap_or_else(std::sync::PoisonError::into_inner).is_some()
    }

    fn unavailable(what: &str, fix: &str) -> SdkError {
        SdkError::new(code::UNAVAILABLE, what.to_string()).with("fix", fix)
    }

    fn available(&self, device: Option<&str>) -> Result<String, SdkError> {
        if let Some(why) = self.blocked {
            return Err(Self::unavailable("listening is not available on this computer", why));
        }
        let name = self.capture.microphone(device).map_err(|e| Self::unavailable("there is no microphone to listen through", &e))?;
        self.engine.ready().map_err(|e| Self::unavailable("no voice engine is set up for listening", &e))?;
        Ok(name)
    }

    fn start(&self, id: u32, lang: Option<String>, secs: u64, device: Option<String>) -> Result<(), SdkError> {
        let Some(sink) = self.sink.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone() else {
            return Err(Self::unavailable("listening is not connected to the app yet", "Try again after the app has restarted."));
        };
        let stop = Arc::new(AtomicBool::new(false));
        {
            let mut a = self.active.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if a.is_some() {
                return Err(SdkError::new(code::BUSY, "the microphone is already in use"));
            }
            *a = Some((id, stop.clone()));
        }
        let (capture, engine) = (self.capture.clone(), self.engine.clone());
        let active_flag = stop.clone();
        let slot = self.active.clone();
        std::thread::Builder::new()
            .name("iyakki-listen".into())
            .spawn(move || {
                let result = capture.record(device.as_deref(), secs, &active_flag).map_err(|e| Self::unavailable("the microphone could not be used", &e)).and_then(|mut pcm| {
                    let cancelled = active_flag.load(Ordering::SeqCst);
                    let text = if cancelled { Ok(String::new()) } else { engine.transcribe(&pcm, lang.as_deref()).map_err(|e| Self::unavailable("the voice engine failed", &e)) };
                    // the sound is thrown away: overwritten, then dropped
                    pcm.iter_mut().for_each(|s| *s = 0);
                    drop(pcm);
                    text.map(|t| (t, cancelled))
                });
                let mut a = slot.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                if a.as_ref().is_some_and(|(i, _)| *i == id) {
                    *a = None;
                }
                drop(a);
                match result {
                    Ok((_, true)) => {}
                    Ok((t, false)) if t.trim().is_empty() => sink(id, Err(SdkError::new(code::TIMEOUT, "the microphone heard nothing it could use in time"))),
                    Ok((t, false)) => sink(id, Ok(t)),
                    Err(e) => sink(id, Err(e)),
                }
            })
            .map_err(|_| SdkError::new(code::BUSY, "too many things are running; try again in a moment"))?;
        Ok(())
    }
}

impl Backend for ListenBackend {
    fn call(&self, _: &Who, op: &str, a: &Value) -> Result<Value, SdkError> {
        let device = a.get("device").and_then(Value::as_str).filter(|d| !d.is_empty());
        match op {
            "speech.available" => Ok(json!({"microphone": self.available(device)?})),
            "speech.listen" => {
                self.available(device)?;
                let id = a.get("id").and_then(Value::as_u64).ok_or_else(|| SdkError::bad("id", "a handle"))? as u32;
                let secs = a.get("max_seconds").and_then(Value::as_u64).unwrap_or(15).clamp(1, 60);
                let lang = a.get("lang").and_then(Value::as_str).map(str::to_string);
                self.start(id, lang, secs, device.map(str::to_string))?;
                Ok(json!({}))
            }
            "speech.cancel" => {
                if let Some((_, stop)) = self.active.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
                    stop.store(true, Ordering::SeqCst);
                }
                Ok(json!({}))
            }
            _ => Err(SdkError::new(code::UNAVAILABLE, "this backend only listens")),
        }
    }
}
