//! My IYAKKI on a cloud machine: who may send sound where (F-VOICE-38; transport in `audiotls`, frames in `audiostream`).
//!
//! The microphone is on the person's own device, so on a cloud machine sound has to leave that device. Three things must be true before one frame
//! does, and the [`Gate`] holds all three in one place:
//!
//! 1. The Settings switch (`voice.my_iyakki_cloud_stream`, off by default; the chat tool `voice_cloud_stream_set` changes it and asks every time).
//! 2. The person pressed Allow on the consent card IN THIS SESSION. The card names the cloud machine and says that microphone sound will leave the
//!    device. Only [`Gate::press`] answers it, and only the screen calls that (a tap or a key); there is no function that takes words, so a spoken
//!    yes cannot approve it.
//! 3. The microphone is not muted. Mute stops the stream at the source: nothing is queued while muted.
//!
//! [`Uplink`] is the device side (microphone sound into frames, over the pinned TLS link); [`DownlinkAudio`] is the cloud-machine side (a
//! `voiceassist::AudioIn` that hands the frames it received to My IYAKKI as if they were a local microphone).

use std::net::SocketAddr;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use crate::audiostream::{cut, Frame, Jitter, SendQueue, FRAME_SAMPLES};

/// The most the device keeps waiting for a slow link.
pub const QUEUE_MS: u32 = 200;

/// Where the gate stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The Settings switch is off: nothing leaves, no card is shown.
    Off,
    /// The card is waiting for a tap or a key; its text.
    Card(String),
    Allowed,
    Denied,
    Muted,
}

#[derive(Clone, Debug)]
pub struct Gate {
    enabled: bool,
    muted: bool,
    machine: String,
    answer: Option<bool>,
}

impl Gate {
    pub fn new(enabled: bool, machine: &str) -> Gate {
        Gate { enabled, muted: false, machine: machine.chars().filter(|c| !c.is_control()).take(60).collect(), answer: None }
    }

    /// The Settings switch changed. Turning it off forgets the answer: the next time asks again.
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
        if !on {
            self.answer = None;
        }
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// A new session begins: the card is asked again whatever was answered before.
    pub fn new_session(&mut self) {
        self.answer = None;
    }

    pub fn card_text(&self) -> String {
        format!("Send your microphone sound to the cloud machine {}? While this is on, what you say leaves this device, over a secure link, so My IYAKKI there can hear you. Nothing is kept. Mute stops it at once. A tap or a key answers; saying yes does not.", self.machine)
    }

    pub fn verdict(&self) -> Verdict {
        if !self.enabled {
            Verdict::Off
        } else if self.muted {
            Verdict::Muted
        } else {
            match self.answer {
                None => Verdict::Card(self.card_text()),
                Some(true) => Verdict::Allowed,
                Some(false) => Verdict::Denied,
            }
        }
    }

    /// The person pressed Allow (true) or Deny (false) on the card. Only the screen's tap or key calls this.
    pub fn press(&mut self, yes: bool) {
        if self.enabled && self.answer.is_none() {
            self.answer = Some(yes);
        }
    }

    fn why_not(&self) -> Option<String> {
        match self.verdict() {
            Verdict::Allowed => None,
            Verdict::Off => Some("Sending sound to the cloud machine is off (Settings -> Voice).".into()),
            Verdict::Card(_) => Some("Waiting for you to allow sending your sound to the cloud machine.".into()),
            Verdict::Denied => Some("You said no to sending your sound to the cloud machine.".into()),
            Verdict::Muted => Some("The microphone is muted, so nothing is sent.".into()),
        }
    }
}

impl crate::services::Services {
    fn gate(&self) -> std::sync::MutexGuard<'_, Gate> {
        self.cloud_gate.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The Voice tile calls this on every beat: the gate follows the Settings switch (a changed switch starts a new gate, which asks again) and the mute.
    pub fn cloud_stream_sync(&self) {
        let v = self.voice_settings();
        let mut g = self.gate();
        if v.my_cloud_stream != (g.verdict() != Verdict::Off) {
            *g = Gate::new(v.my_cloud_stream, &crate::devname::label(self.dir()));
        }
        g.set_muted(v.my_muted);
    }

    /// Where the gate stands.
    pub fn cloud_stream_verdict(&self) -> Verdict {
        self.gate().verdict()
    }

    /// The text of the consent card while it is waiting for an answer.
    pub fn cloud_stream_card(&self) -> Option<String> {
        match self.gate().verdict() {
            Verdict::Card(t) => Some(t),
            _ => None,
        }
    }

    /// The person pressed Allow (true) or Do not allow (false) on the card: a tap or a key on the screen, nothing else calls this.
    pub fn cloud_stream_press(&self, yes: bool) {
        self.gate().press(yes);
    }
}

/// The device side: sound in, numbered frames out, over the pinned TLS link.
pub struct Uplink {
    pub gate: Gate,
    queue: SendQueue,
    seq: u32,
    ms: u32,
}

impl Uplink {
    pub fn new(gate: Gate) -> Uplink {
        Uplink { gate, queue: SendQueue::new(QUEUE_MS), seq: 0, ms: 0 }
    }

    /// Microphone sound (16 kHz mono). Refused, and not kept, unless the gate allows it.
    pub fn push_pcm(&mut self, pcm: &[i16]) -> Result<usize, String> {
        if let Some(why) = self.gate.why_not() {
            return Err(why);
        }
        let frames = cut(pcm, self.seq, self.ms);
        self.seq = self.seq.wrapping_add(frames.len() as u32);
        self.ms = self.ms.wrapping_add((frames.len() * FRAME_SAMPLES * 1000 / crate::audiostream::RATE as usize) as u32);
        let n = frames.len();
        for f in frames {
            self.queue.push(f);
        }
        Ok(n)
    }

    pub fn dropped(&self) -> u32 {
        self.queue.dropped
    }

    /// Send what is queued over the pinned TLS link. The gate is checked again: a mute or a switch-off that came after the sound was queued sends
    /// nothing and empties the queue.
    pub fn flush(&mut self, addr: SocketAddr, cfg: Arc<rustls::ClientConfig>) -> Result<usize, String> {
        if let Some(why) = self.gate.why_not() {
            while self.queue.pop().is_some() {}
            return Err(why);
        }
        let mut frames = Vec::new();
        while let Some(f) = self.queue.pop() {
            frames.push(f);
        }
        crate::audiotls::send_frames(addr, cfg, &frames)?;
        Ok(frames.len())
    }
}

/// The cloud-machine side: frames in, a microphone for My IYAKKI out.
pub struct DownlinkAudio {
    rx: Receiver<Frame>,
    jitter: Jitter,
    heard: Vec<i16>,
    open: bool,
    level: f32,
}

/// The handle the receiving thread uses to hand frames over.
pub type FrameSink = Sender<Frame>;

impl DownlinkAudio {
    pub fn new() -> (DownlinkAudio, FrameSink) {
        let (tx, rx) = channel();
        (DownlinkAudio { rx, jitter: Jitter::new(3), heard: vec![], open: false, level: 0.0 }, tx)
    }

    fn pump(&mut self) {
        while let Ok(f) = self.rx.try_recv() {
            self.jitter.push(f);
        }
        while let Some(p) = self.jitter.pop() {
            self.level = p.iter().map(|s| (*s as f32).abs()).fold(0.0, f32::max) / 32768.0;
            if self.open {
                self.heard.extend(p);
            }
        }
    }
}

impl crate::voiceassist::AudioIn for DownlinkAudio {
    fn open(&mut self) -> Result<(), String> {
        self.heard.clear();
        self.open = true;
        Ok(())
    }
    fn is_open(&self) -> bool {
        self.open
    }
    fn level(&mut self) -> f32 {
        self.pump();
        self.level
    }
    fn alive(&mut self) -> bool {
        self.open
    }
    fn take(&mut self) -> Result<Vec<i16>, String> {
        self.pump();
        self.heard.extend(self.jitter.drain());
        self.open = false;
        Ok(std::mem::take(&mut self.heard))
    }
    fn discard(&mut self) {
        self.open = false;
        self.heard.clear();
        let _ = self.jitter.drain();
    }
}

#[cfg(test)]
#[path = "cloudstream_tests.rs"]
mod tests;
