//! The real parts behind `iyakki.speech.listen` (F-SDK-16): the microphone and the voice engine of the person's own computer.
//!
//! * [`RecorderCapture`] is [`super::sdkv2_listen::Capture`] over the voice desk's recorders (`iyakki_shared::voicedesk::Recorders`: the
//!   microphones that `arecord` can open, the person's record command aimed at the chosen one, the named capture PCM of `micpcm`). The T2
//!   rule: when the T2 sound card's capture streams are switched on, listening for apps is `Unavailable` (capture there is not proven safe;
//!   with them off the card's microphones are hidden anyway and a USB microphone works).
//! * [`VoiceRecogniser`] is [`super::sdkv2_listen::Recogniser`] over the voice backends (`voicebackend::Registry`: the engine of Settings ->
//!   Voice or the Lemonade Whisper model). It only uses an engine that stays **on this computer**: the sound of an app's listening is never
//!   sent to another machine or to the internet (decision D-SDK-16-02).
//!
//! Nothing here keeps sound: the samples go from the recorder to the engine and are dropped by the caller (`ListenBackend` overwrites them).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iyakki_shared::voicedesk::{pick_mic, record_cmd_for, Recorders, T2Mic};

use super::sdkv2_listen::{Capture, Pcm, Recogniser};
use crate::voicebackend::{Kind, VoiceBackend};

/// Words the person reads when there is no microphone.
const NO_MIC: &str = "Plug in a USB microphone, or use dictation in the chat.";
const T2_OFF_LIMITS: &str = "The built-in microphone of this laptop is not usable by apps yet; use a USB microphone or dictation in the chat.";

pub struct RecorderCapture {
    recorders: Arc<dyn Recorders>,
    /// The record command of Settings -> Voice (`arecord ...`); aimed at the chosen microphone by `record_cmd_for`.
    record_cmd: Arc<dyn Fn() -> String + Send + Sync>,
    /// The microphone the person chose in Settings -> Voice (an ALSA id), or empty for the first one found.
    chosen: Arc<dyn Fn() -> String + Send + Sync>,
    t2: Arc<dyn Fn() -> T2Mic + Send + Sync>,
    /// How often the recording is looked at.
    tick: Duration,
}

impl RecorderCapture {
    pub fn new(recorders: Arc<dyn Recorders>, record_cmd: Arc<dyn Fn() -> String + Send + Sync>, chosen: Arc<dyn Fn() -> String + Send + Sync>, t2: Arc<dyn Fn() -> T2Mic + Send + Sync>) -> RecorderCapture {
        RecorderCapture { recorders, record_cmd, chosen, t2, tick: Duration::from_millis(25) }
    }

    pub fn with_tick(mut self, tick: Duration) -> RecorderCapture {
        self.tick = tick;
        self
    }

    fn mic(&self, device: Option<&str>) -> Result<iyakki_shared::voicedesk::Mic, String> {
        if (self.t2)() == T2Mic::On {
            return Err(T2_OFF_LIMITS.into());
        }
        let mics = self.recorders.mics();
        let want = device.map(str::to_string).unwrap_or_else(|| (self.chosen)());
        // a device an app names must be one of the machine's own microphones; an unknown name is not guessed at
        if let Some(d) = device {
            return mics.iter().find(|m| m.id == d || m.label == d).cloned().ok_or_else(|| format!("there is no microphone called {d:?}. {NO_MIC}"));
        }
        pick_mic(&mics, &want).cloned().ok_or_else(|| NO_MIC.to_string())
    }
}

impl Capture for RecorderCapture {
    fn microphone(&self, device: Option<&str>) -> Result<String, String> {
        self.mic(device).map(|m| m.label)
    }

    fn record(&self, device: Option<&str>, max_seconds: u64, stop: &AtomicBool) -> Result<Pcm, String> {
        let mic = self.mic(device)?;
        let cmd = record_cmd_for(&(self.record_cmd)(), Some(&mic));
        let max = (max_seconds as usize).clamp(1, iyakki_shared::voicedesk::MAX_RECORD_SECS);
        let mut cap = self.recorders.start(&cmd, max)?;
        let limit = Duration::from_secs(max_seconds);
        while !stop.load(Ordering::SeqCst) && !cap.is_full() && cap.elapsed() < limit && cap.is_alive() {
            std::thread::sleep(self.tick);
        }
        cap.stop()
    }
}

/// The voice engine of this computer, looked up each time (the person may change it in Settings).
pub struct VoiceRecogniser {
    backend: Arc<dyn Fn() -> Option<Arc<dyn VoiceBackend>> + Send + Sync>,
}

impl VoiceRecogniser {
    pub fn new(backend: Arc<dyn Fn() -> Option<Arc<dyn VoiceBackend>> + Send + Sync>) -> VoiceRecogniser {
        VoiceRecogniser { backend }
    }

    /// The engine of Settings -> Voice or the automatic Whisper model of the running system.
    pub fn of_services(svc: &Arc<crate::services::Services>) -> VoiceRecogniser {
        let svc = svc.clone();
        VoiceRecogniser::new(Arc::new(move || {
            let registry = crate::voicebackend::Registry::from_services(&svc);
            registry.selected(crate::voicedesk::stt_engine(&svc).as_ref())
        }))
    }

    fn usable(&self) -> Result<Arc<dyn VoiceBackend>, String> {
        let b = (self.backend)().ok_or("Set up speech-to-text in Settings -> Voice, or download a Whisper model in Settings -> Models.")?;
        if !b.configured() {
            return Err("Set up speech-to-text in Settings -> Voice, or download a Whisper model in Settings -> Models.".into());
        }
        if b.kind() != Kind::OnDevice {
            return Err("Apps can only listen through speech-to-text that stays on this computer; choose a local engine in Settings -> Voice.".into());
        }
        Ok(b)
    }
}

impl Recogniser for VoiceRecogniser {
    fn ready(&self) -> Result<(), String> {
        self.usable().map(|_| ())
    }

    fn transcribe(&self, pcm: &[i16], lang: Option<&str>) -> Result<String, String> {
        let b = self.usable()?;
        let _ = lang; // the engines of Settings -> Voice take their language from their own settings
        match crate::voicedesk::transcribe_backend(&*b, pcm) {
            Ok(t) => Ok(t),
            // nothing heard is not a failure: the bridge turns an empty text into a timeout event
            Err(e) if crate::voicedesk::is_no_words(&e) || e == "Nothing was recorded." => Ok(String::new()),
            Err(e) => Err(e),
        }
    }
}

/// The backend of the running system: this computer's microphones and its own voice engine.
pub fn listener_of_services(svc: &Arc<crate::services::Services>) -> Arc<super::sdkv2_listen::ListenBackend> {
    let (s1, s2) = (svc.clone(), svc.clone());
    let capture = RecorderCapture::new(
        Arc::new(crate::voicedesk::SystemRecorders),
        Arc::new(move || s1.voice_settings().record_cmd),
        Arc::new(move || s2.voice_settings().mic),
        Arc::new(|| crate::voicedesk::t2_mic(&crate::voicedesk::proc_root())),
    );
    Arc::new(super::sdkv2_listen::ListenBackend::new(Arc::new(capture), Arc::new(VoiceRecogniser::of_services(svc))))
}

#[cfg(test)]
#[path = "sdkv2_listen_real_tests.rs"]
mod tests;
