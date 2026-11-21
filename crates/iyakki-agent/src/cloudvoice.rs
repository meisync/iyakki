//! My IYAKKI on a cloud machine, the working parts on both ends (F-VOICE-61; the gate and the frames are in `cloudstream`, the pinned TLS link in
//! `audiotls`). Three things are written here:
//!
//! * [`DeviceClient`]: the device end. It reads the microphone, puts the sound through the [`Uplink`] (which refuses everything the gate refuses:
//!   switch off, no Allow pressed in this session, mute), sends the frames over the pinned link every few frames and backs off when the link
//!   is down. Mute or a "no" closes the microphone itself, so nothing is captured that cannot be sent.
//! * [`CloudEnd`]: the cloud machine's end. It accepts the device's frames into the [`DownlinkAudio`] the assistant listens to, and sends the
//!   assistant's answer sound back as frames to the device over the link in the other direction (the device has its own certificate for it).
//! * [`CertOffer`]: how a certificate reaches the other end: as part of the authenticated session of the Connector, a small record with a size
//!   limit, never from the audio link itself.
//!
//! The device end, the gate, the frames and the pinned link live in `iyakki-audiolink` (one small crate for this agent and the connector program
//! `iyakki-connect`, which carries the offers and starts the device loop); they are re-exported here. Everything is tested against a loopback
//! link with real TLS and the real pinned certificates.

use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;

use crate::audiostream::{cut, Frame, FRAME_SAMPLES};
use crate::cloudstream::{DownlinkAudio, FrameSink};

pub use iyakki_audiolink::certoffer::{CertOffer, MAX_CERT};
pub use iyakki_audiolink::device::{AnswerPlayer, DeviceClient, Link, Step, TlsLink, BATCH_FRAMES, MAX_FAILURES};

/// The cloud machine's end.
pub struct CloudEnd {
    /// What My IYAKKI on the cloud machine listens to.
    pub audio: DownlinkAudio,
    sink: FrameSink,
}

impl CloudEnd {
    pub fn new() -> CloudEnd {
        let (audio, sink) = DownlinkAudio::new();
        CloudEnd { audio, sink }
    }

    /// The handle a receiving thread gives frames to.
    pub fn sink(&self) -> FrameSink {
        self.sink.clone()
    }

    /// Take the device's frames from one connection into the assistant's microphone. Returns how many frames came.
    pub fn serve_one(&self, listener: &TcpListener, cfg: Arc<rustls::ServerConfig>) -> Result<u32, String> {
        let sink = self.sink.clone();
        crate::audiotls::receive_frames(listener, cfg, &mut |f: Frame| {
            let _ = sink.send(f);
        })
    }
}

impl Default for CloudEnd {
    fn default() -> Self {
        CloudEnd::new()
    }
}

/// The assistant's answer as sound (16 kHz mono) back to the device, over the link in the other direction: the device listens with its own
/// certificate, which the cloud machine got over the session. Returns the number of frames sent.
pub fn send_answer(pcm: &[i16], device: SocketAddr, pinned_device_cert_der: &[u8]) -> Result<usize, String> {
    if pcm.is_empty() {
        return Ok(0);
    }
    let frames = cut(pcm, 0, 0);
    let n = frames.len();
    crate::audiotls::send_frames(device, crate::audiotls::client_config(pinned_device_cert_der)?, &frames)?;
    Ok(n)
}


/// The cloud machine's listener thread (F-VOICE-61): it holds a certificate made for this link, waits for devices on `bind`, takes their frames
/// into the [`DownlinkAudio`] My IYAKKI listens to, and hands the certificate and its advertised address to a device that asks inside the
/// authenticated session ([`CloudListener::offer`]). Frames that arrive while the switch is off are dropped (fail closed), and a switch that
/// is off makes no offer at all. The thread ends when the listener is dropped.
pub struct CloudListener {
    id: crate::audiotls::Identity,
    advertise: String,
    machine: String,
    enabled: Arc<dyn Fn() -> bool + Send + Sync>,
    audio: std::sync::Mutex<Option<DownlinkAudio>>,
    ret: std::sync::Mutex<Option<(SocketAddr, Vec<u8>)>>,
    stop: Arc<std::sync::atomic::AtomicBool>,
    wake: SocketAddr,
    served: Arc<std::sync::atomic::AtomicU32>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl CloudListener {
    /// Bind `bind` (`host:port`, port 0 for any), advertise `advertise` (`host:port`; empty means the address actually bound) and start the thread.
    pub fn start(bind: &str, advertise: &str, machine: &str, enabled: Arc<dyn Fn() -> bool + Send + Sync>) -> Result<CloudListener, String> {
        let id = crate::audiotls::Identity::make()?;
        let cfg = crate::audiotls::server_config(&id)?;
        let listener = TcpListener::bind(bind).map_err(|e| format!("could not listen for the audio link: {e}"))?;
        let wake = listener.local_addr().map_err(|e| e.to_string())?;
        let (audio, sink) = DownlinkAudio::new();
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let served = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let (stop2, on2, served2) = (stop.clone(), enabled.clone(), served.clone());
        let thread = std::thread::spawn(move || {
            while !stop2.load(std::sync::atomic::Ordering::SeqCst) {
                let r = crate::audiotls::receive_frames(&listener, cfg.clone(), &mut |f: Frame| {
                    if on2() {
                        let _ = sink.send(f);
                    }
                });
                served2.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if r.is_err() {
                    // a peer that did not speak TLS, or a cut stream: the next device is accepted; a failing accept does not spin
                    std::thread::sleep(std::time::Duration::from_millis(20));
                }
            }
        });
        let advertise = if advertise.is_empty() { wake.to_string() } else { advertise.to_string() };
        Ok(CloudListener { id, advertise, machine: machine.chars().filter(|c| !c.is_control()).take(60).collect(), enabled, audio: std::sync::Mutex::new(Some(audio)), ret: std::sync::Mutex::new(None), stop, wake, served, thread: Some(thread) })
    }

    /// Connections the thread has finished with (one per batch a device sends), so a test can wait for one without sleeping.
    pub fn served(&self) -> u32 {
        self.served.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Where the thread listens now.
    pub fn local_addr(&self) -> SocketAddr {
        self.wake
    }

    /// The reply to the device's call inside the session: `(address, certificate offer as JSON)`. Refused while the switch is off.
    pub fn offer(&self) -> Result<(String, String), String> {
        if !(self.enabled)() {
            return Err("My IYAKKI's cloud audio stream is off on this machine (Settings, Voice)".into());
        }
        Ok((self.advertise.clone(), CertOffer { machine: self.machine.clone(), cert_der: self.id.cert_der.clone() }.to_json()))
    }

    /// The device's own offer for the answer's sound (its address and certificate, got inside the authenticated session). Refused while the
    /// switch is off, and an address that is not `host:port` or a certificate that is not one is refused too.
    pub fn set_return(&self, device_addr: &str, offer_json: &str) -> Result<(), String> {
        if !(self.enabled)() {
            return Err("My IYAKKI's cloud audio stream is off on this machine (Settings, Voice)".into());
        }
        let addr: SocketAddr = device_addr.parse().map_err(|_| "the device's audio address is not host:port".to_string())?;
        let offer = CertOffer::from_json(offer_json)?;
        *self.ret.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some((addr, offer.cert_der));
        Ok(())
    }

    /// Where the answer's sound goes: `None` while the switch is off or the device has not offered a return link.
    pub fn return_target(&self) -> Option<(SocketAddr, Vec<u8>)> {
        if !(self.enabled)() {
            return None;
        }
        self.ret.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    /// The microphone My IYAKKI listens to; handed out once.
    pub fn take_audio(&self) -> Option<DownlinkAudio> {
        self.audio.lock().unwrap_or_else(std::sync::PoisonError::into_inner).take()
    }
}

impl Drop for CloudListener {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        // wake the accept (a plain connect: the handshake fails and the thread sees the flag)
        let _ = std::net::TcpStream::connect_timeout(&self.wake, std::time::Duration::from_secs(1));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// My IYAKKI's mouth in a cloud session: the sentence is made into sound by `synth` (a WAV), turned into 16 kHz mono and sent to the device
/// over the pinned link in the other direction ([`send_answer`]). Nothing plays on the cloud machine. No return link is an error the
/// assistant says in words. One sentence at a time on a helper thread.
pub struct CloudSpeaker {
    synth: Arc<dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync>,
    target: Arc<dyn Fn() -> Option<(SocketAddr, Vec<u8>)> + Send + Sync>,
    busy: Arc<std::sync::atomic::AtomicBool>,
    stop: Arc<std::sync::atomic::AtomicBool>,
}

impl CloudSpeaker {
    pub fn new(synth: Arc<dyn Fn(&str) -> Result<Vec<u8>, String> + Send + Sync>, target: Arc<dyn Fn() -> Option<(SocketAddr, Vec<u8>)> + Send + Sync>) -> CloudSpeaker {
        CloudSpeaker { synth, target, busy: Arc::new(std::sync::atomic::AtomicBool::new(false)), stop: Arc::new(std::sync::atomic::AtomicBool::new(false)) }
    }
}

impl crate::voiceassist::Speaker for CloudSpeaker {
    fn say(&mut self, text: &str) -> Result<(), String> {
        use std::sync::atomic::Ordering;
        let (addr, cert) = (self.target)().ok_or_else(|| "The device has not offered a link for My IYAKKI's voice yet.".to_string())?;
        self.stop.store(true, Ordering::SeqCst);
        let (busy, stop) = (Arc::new(std::sync::atomic::AtomicBool::new(true)), Arc::new(std::sync::atomic::AtomicBool::new(false)));
        self.busy = busy.clone();
        self.stop = stop.clone();
        let (synth, text) = (self.synth.clone(), text.to_string());
        std::thread::spawn(move || {
            if let Ok(wav) = synth(&text) {
                if let Ok(w) = crate::voice::parse_wav(&wav) {
                    if !stop.load(Ordering::SeqCst) {
                        let _ = send_answer(&crate::voice::to_mono_16k(&w), addr, &cert);
                    }
                }
            }
            busy.store(false, Ordering::SeqCst);
        });
        Ok(())
    }
    fn busy(&mut self) -> bool {
        self.busy.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn stop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
    }
    fn level(&self) -> f32 {
        // the sound plays on the device, not here: nothing of it reaches this machine's microphone
        0.0
    }
    fn louder(&mut self) {}
    fn quieter(&mut self) {}
}

/// Samples in a second of sound at the stream's rate.
pub const SAMPLES_PER_SECOND: usize = FRAME_SAMPLES * 50;

#[cfg(test)]
#[path = "cloudvoice_tests.rs"]
mod tests;
