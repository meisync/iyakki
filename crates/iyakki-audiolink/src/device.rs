//! The device's end of "My IYAKKI" in cloud mode (F-VOICE-61): the microphone through the consent gate, into frames, over the pinned TLS link.
//! Both the agent and the connector program run this loop; the link and the microphone are traits, so a test stands in for each.

use std::net::{SocketAddr, TcpListener};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::audiostream::{Frame, Jitter};
use crate::gate::Gate;
use crate::uplink::Uplink;
use crate::Mic;

/// Frames kept together before a send (5 frames are 100 ms).
pub const BATCH_FRAMES: usize = 5;
/// Failed sends in a row after which the loop gives up and says so (the person's card is shown again after a new session).
pub const MAX_FAILURES: u32 = 8;

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

impl TlsLink {
    /// A link to `addr` that trusts only the certificate `pinned_cert_der`.
    pub fn new(addr: SocketAddr, pinned_cert_der: &[u8]) -> Result<TlsLink, String> {
        Ok(TlsLink { addr, cfg: crate::audiotls::client_config(pinned_cert_der)? })
    }
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
