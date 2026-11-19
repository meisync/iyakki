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
//! The Connector program that carries the offers and starts these loops is outside this crate; everything here is tested against a loopback
//! link with real TLS and the real pinned certificates.

use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::audiostream::{cut, Frame, Jitter, FRAME_SAMPLES};
use crate::cloudstream::{DownlinkAudio, FrameSink, Gate, Uplink};
use iyakki_shared::wakelisten::Mic;

/// Frames kept together before a send (5 frames are 100 ms).
pub const BATCH_FRAMES: usize = 5;
/// Failed sends in a row after which the loop gives up and says so (the person's card is shown again after a new session).
pub const MAX_FAILURES: u32 = 8;
/// The most a certificate offer may carry.
pub const MAX_CERT: usize = 4096;

/// What one step did.
#[derive(Debug, PartialEq, Eq)]
pub enum Step {
    /// This many frames were sent.
    Sent(usize),
    /// Sound is being gathered; nothing to send yet.
    Gathering,
    /// The gate says no (the words say why); the microphone was closed.
    Refused(String),
    /// The link failed this time (the words say why); the sound of this batch is lost, not kept.
    LinkDown(String),
}

/// Where a batch of frames goes. The real one is the pinned TLS link; a test puts its own.
pub trait Link: Send {
    fn send(&mut self, frames: &[Frame]) -> Result<(), String>;
}

/// The pinned TLS link to the cloud machine.
pub struct TlsLink {
    addr: SocketAddr,
    cfg: Arc<rustls::ClientConfig>,
}

impl Link for TlsLink {
    fn send(&mut self, frames: &[Frame]) -> Result<(), String> {
        crate::audiotls::send_frames(self.addr, self.cfg.clone(), frames)
    }
}

pub struct DeviceClient {
    pub up: Uplink,
    mic: Box<dyn Mic>,
    link: Box<dyn Link>,
    failures: u32,
}

impl DeviceClient {
    /// `pinned_cert_der` is the cloud machine's certificate as it came over the authenticated session.
    pub fn new(gate: Gate, mic: Box<dyn Mic>, addr: SocketAddr, pinned_cert_der: &[u8]) -> Result<DeviceClient, String> {
        let link = TlsLink { addr, cfg: crate::audiotls::client_config(pinned_cert_der)? };
        Ok(DeviceClient { up: Uplink::new(gate), mic, link: Box::new(link), failures: 0 })
    }

    /// The same with another link (tests, or a transport that is not the TLS one).
    pub fn with_link(gate: Gate, mic: Box<dyn Mic>, link: Box<dyn Link>) -> DeviceClient {
        DeviceClient { up: Uplink::new(gate), mic, link, failures: 0 }
    }

    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// One turn: read what the microphone has, queue it, send a batch when there is one.
    pub fn step(&mut self) -> Step {
        if let Some(why) = self.up.gate.why_not() {
            // the microphone is closed at the source: nothing is captured that may not be sent
            if self.mic.is_open() {
                self.mic.close();
            }
            let _ = self.up.take_frames();
            return Step::Refused(why);
        }
        if !self.mic.is_open() {
            if let Err(e) = self.mic.open() {
                return Step::Refused(format!("the microphone could not be opened: {e}"));
            }
        }
        let pcm = self.mic.drain();
        if !pcm.is_empty() {
            if let Err(why) = self.up.push_pcm(&pcm) {
                return Step::Refused(why);
            }
        }
        if !self.mic.alive() {
            return Step::Refused("the microphone stopped".into());
        }
        if self.up.queued() < BATCH_FRAMES {
            return Step::Gathering;
        }
        let frames = match self.up.take_frames() {
            Ok(f) => f,
            Err(why) => {
                self.mic.close();
                return Step::Refused(why);
            }
        };
        match self.link.send(&frames) {
            Ok(()) => {
                self.failures = 0;
                Step::Sent(frames.len())
            }
            Err(e) => {
                self.failures += 1;
                Step::LinkDown(e)
            }
        }
    }

    /// Steps until `stop` is set or the link has failed [`MAX_FAILURES`] times in a row. `pause` is the time between steps.
    pub fn run(&mut self, stop: &AtomicBool, pause: Duration) -> Step {
        let mut last = Step::Gathering;
        while !stop.load(Ordering::SeqCst) {
            last = self.step();
            if matches!(last, Step::Refused(_)) || self.failures >= MAX_FAILURES {
                break;
            }
            std::thread::sleep(if matches!(last, Step::LinkDown(_)) { pause * (1 + self.failures.min(10)) } else { pause });
        }
        self.mic.close();
        last
    }
}

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

/// The device's side of the answer: frames in, sound out, in order, with a short wait for late ones.
pub struct AnswerPlayer {
    jitter: Jitter,
    pub heard: Vec<i16>,
}

impl AnswerPlayer {
    pub fn new() -> AnswerPlayer {
        AnswerPlayer { jitter: Jitter::new(3), heard: Vec::new() }
    }

    /// Take one connection's frames; what arrives is played in order (here: gathered for the speaker to take).
    pub fn receive(&mut self, listener: &TcpListener, cfg: Arc<rustls::ServerConfig>) -> Result<u32, String> {
        let jitter = &mut self.jitter;
        let heard = &mut self.heard;
        let n = crate::audiotls::receive_frames(listener, cfg, &mut |f| {
            jitter.push(f);
            while let Some(p) = jitter.pop() {
                heard.extend(p);
            }
        })?;
        heard.extend(jitter.drain());
        Ok(n)
    }

    /// The sound gathered so far, for the speaker; taken once.
    pub fn take(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.heard)
    }
}

impl Default for AnswerPlayer {
    fn default() -> Self {
        AnswerPlayer::new()
    }
}

/// A certificate on its way to the other end inside the authenticated session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertOffer {
    pub machine: String,
    pub cert_der: Vec<u8>,
}

impl CertOffer {
    /// The record the session carries: `{"machine": "...", "cert": "<base64 of the DER>"}`.
    pub fn to_json(&self) -> String {
        serde_json::json!({"machine": self.machine, "cert": iyakki_shared::b64::base64_encode(&self.cert_der)}).to_string()
    }

    /// Read a record from the session. A machine name with control characters, an empty or oversized certificate, or text that is not a
    /// certificate at all is refused (the check is the DER sequence's own length, not a guess).
    pub fn from_json(text: &str) -> Result<CertOffer, String> {
        let v: serde_json::Value = serde_json::from_str(text.get(..MAX_CERT * 2).unwrap_or(text)).map_err(|_| "the certificate offer is not valid".to_string())?;
        let machine = v["machine"].as_str().unwrap_or("").to_string();
        if machine.is_empty() || machine.len() > 60 || machine.chars().any(char::is_control) {
            return Err("the certificate offer names no machine".into());
        }
        let der = v["cert"].as_str().and_then(iyakki_shared::b64::b64_decode).ok_or("the certificate offer has no certificate")?;
        if der.len() < 64 || der.len() > MAX_CERT || der[0] != 0x30 || !der_length_matches(&der) {
            return Err("that is not a certificate".into());
        }
        Ok(CertOffer { machine, cert_der: der })
    }
}

/// A DER SEQUENCE whose own length is the length of the bytes.
fn der_length_matches(d: &[u8]) -> bool {
    let (len, header) = match d.get(1) {
        Some(b) if *b < 0x80 => (*b as usize, 2),
        Some(0x81) => (d.get(2).copied().unwrap_or(0) as usize, 3),
        Some(0x82) => (((d.get(2).copied().unwrap_or(0) as usize) << 8) | d.get(3).copied().unwrap_or(0) as usize, 4),
        _ => return false,
    };
    header + len == d.len()
}

/// Samples in a second of sound at the stream's rate.
pub const SAMPLES_PER_SECOND: usize = FRAME_SAMPLES * 50;

#[cfg(test)]
#[path = "cloudvoice_tests.rs"]
mod tests;
