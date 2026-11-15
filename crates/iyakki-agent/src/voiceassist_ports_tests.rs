//! The real microphone port with echo cancellation: a capture made while the assistant spoke comes back without its own voice when the
//! person switched that on, and unchanged when not (F-VOICE-36). The fixture is synthetic (no real room was recorded).

use super::*;
use crate::voicedesk::{Capture, Mic, Recorders};

struct Off;
impl iyakki_proxy::Upstream for Off {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

struct Cap(Vec<i16>);
impl Capture for Cap {
    fn level(&self) -> f32 {
        0.0
    }
    fn elapsed(&self) -> Duration {
        Duration::from_secs(1)
    }
    fn is_full(&self) -> bool {
        false
    }
    fn is_alive(&mut self) -> bool {
        true
    }
    fn stop(self: Box<Self>) -> Result<Vec<i16>, String> {
        Ok(self.0)
    }
}

struct Recs(Vec<i16>);
impl Recorders for Recs {
    fn mics(&self) -> Vec<Mic> {
        vec![Mic { id: "plughw:1,0".into(), label: "USB Audio (USB)".into(), usb: true }]
    }
    fn product(&self) -> String {
        "test".into()
    }
    fn start(&self, _cmd: &str, _max: usize) -> Result<Box<dyn Capture>, String> {
        Ok(Box::new(Cap(self.0.clone())))
    }
}

fn noise(n: usize, seed: u32) -> Vec<f32> {
    let mut s = seed;
    (0..n)
        .map(|_| {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((s >> 8) as f32 / (1u32 << 24) as f32) - 0.5
        })
        .collect()
}

fn rms16(v: &[i16]) -> f32 {
    (v.iter().map(|x| (f32::from(*x) / 32768.0).powi(2)).sum::<f32>() / v.len() as f32).sqrt()
}

fn svc(tag: &str, echo: bool) -> Arc<Services> {
    let dir = std::env::temp_dir().join(format!("iyakki-echoport-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let svc = Services::open(&dir.join("svc"), Arc::new(Off), Arc::new(Off)).unwrap();
    svc.settings.lock().unwrap().update(|s| s.voice.echo_cancel = echo).unwrap();
    svc
}

/// Far end from `lead` samples into the capture, room 60 ms and 140 ms behind it, nobody else speaking.
fn capture_of(played: &[f32], lead: usize, n: usize) -> Vec<i16> {
    let mut mic = vec![0.0f32; n];
    for (k, v) in played.iter().enumerate() {
        for (d, g) in [(960usize, 0.5f32), (2240, -0.2)] {
            if lead + k + d < n {
                mic[lead + k + d] += v * g;
            }
        }
    }
    mic.iter().map(|x| (x.clamp(-1.0, 1.0) * 32767.0) as i16).collect()
}

#[test]
fn with_the_setting_on_a_capture_made_during_the_assistants_speech_comes_back_without_it_and_with_it_off_it_is_untouched() {
    let played = noise(16_000 * 2, 5);
    let lead = 16_000 / 5;
    let raw = capture_of(&played, lead, 16_000 * 3);
    for (tag, on) in [("on", true), ("off", false)] {
        let s = svc(tag, on);
        let mut mic = RecorderAudio::new(Arc::new(Recs(raw.clone())), s.clone());
        mic.open().unwrap();
        // the sound began 200 ms after the capture was taken to start (the margin the port allows for the recorder's start)
        echo_ref().clear();
        echo_ref().add(mic.opened_ms + CAPTURE_START_MARGIN_MS + 200, played.clone());
        let got = mic.take().unwrap();
        assert_eq!(got.len(), raw.len());
        let (a, b) = (16_000, 16_000 * 2);
        if on {
            let drop = 20.0 * (rms16(&raw[a..b]) / rms16(&got[a..b]).max(1e-6)).log10();
            assert!(drop > 15.0, "the assistant's own voice is {drop:.1} dB quieter in what the speech engine gets");
        } else {
            assert_eq!(got, raw, "switched off: the capture is exactly what the microphone heard");
        }
    }
    echo_ref().clear();
    // nothing played: unchanged even when on
    let s = svc("none", true);
    let mut mic = RecorderAudio::new(Arc::new(Recs(raw.clone())), s);
    mic.open().unwrap();
    assert_eq!(mic.take().unwrap(), raw);
}

// ------------------------------------------------------------------------------------------------ speech to text through the backend seam

struct FakeBackend {
    kind: crate::voicebackend::Kind,
    set_up: bool,
    reply: Result<String, String>,
    got: Arc<Mutex<Vec<usize>>>,
}

impl crate::voicebackend::VoiceBackend for FakeBackend {
    fn id(&self) -> &str {
        "fake"
    }
    fn label(&self) -> String {
        "fake".into()
    }
    fn kind(&self) -> crate::voicebackend::Kind {
        self.kind
    }
    fn configured(&self) -> bool {
        self.set_up
    }
    fn transcribe(&self, wav: &[u8], _lang: &str) -> Result<String, String> {
        self.got.lock().unwrap().push(wav.len());
        self.reply.clone()
    }
}

fn stt_with(tag: &str, kind: crate::voicebackend::Kind, set_up: bool, reply: Result<String, String>) -> (ServicesStt, crate::voicebackend::Registry, Arc<Mutex<Vec<usize>>>) {
    let got = Arc::new(Mutex::new(Vec::new()));
    let mut reg = crate::voicebackend::Registry::new();
    reg.register(Arc::new(FakeBackend { kind, set_up, reply, got: got.clone() })).unwrap();
    (ServicesStt { svc: svc(tag, false) }, reg, got)
}

fn loud() -> Vec<i16> {
    vec![4000; 1600]
}

#[test]
fn the_voice_session_hears_through_the_selected_backend_and_never_calls_a_cloud_one_without_agreement() {
    use crate::voicebackend::Kind;
    // on this computer: the backend is called with the recording as a WAV and its words come back
    let (stt, reg, got) = stt_with("stt-local", Kind::OnDevice, true, Ok("turn it up".into()));
    assert_eq!(stt.hear_with(&reg, &loud()).unwrap().text, "turn it up");
    assert_eq!(got.lock().unwrap().len(), 1);
    // silence and an empty recording never reach the backend
    assert!(crate::voicedesk::is_no_words(&stt.hear_with(&reg, &[0; 1600]).unwrap_err()));
    assert!(stt.hear_with(&reg, &[]).is_err());
    assert_eq!(got.lock().unwrap().len(), 1);
    // no words said by the engine is "no speech", a failure stays a failure
    let (stt, reg, _) = stt_with("stt-none", Kind::OnDevice, true, Ok(String::new()));
    assert_eq!(stt.hear_with(&reg, &loud()).unwrap_err(), crate::voicedesk::NO_SPEECH);
    let (stt, reg, _) = stt_with("stt-fail", Kind::OnDevice, true, Err("boom".into()));
    assert_eq!(stt.hear_with(&reg, &loud()).unwrap_err(), "boom");
    // a service on the internet is not called before the person agreed
    let (stt, reg, got) = stt_with("stt-cloud", Kind::Cloud, true, Ok("secret".into()));
    // the automatic choice never picks it at all
    assert!(stt.hear_with(&reg, &loud()).unwrap_err().contains("No speech-to-text is set up"));
    assert!(got.lock().unwrap().is_empty());
    // not set up
    let (stt, reg, got) = stt_with("stt-off", Kind::OnDevice, false, Ok("x".into()));
    assert!(stt.hear_with(&reg, &loud()).unwrap_err().contains("No speech-to-text is set up"));
    assert!(got.lock().unwrap().is_empty());
}
