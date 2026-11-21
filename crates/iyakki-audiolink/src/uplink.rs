//! The device side of the stream: microphone sound in, numbered frames out (F-VOICE-38, F-VOICE-61).

use std::net::SocketAddr;
use std::sync::Arc;

use crate::audiostream::{cut, Frame, SendQueue, FRAME_SAMPLES};
use crate::gate::Gate;

/// The most the device keeps waiting for a slow link.
pub const QUEUE_MS: u32 = 200;

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

    /// How many frames wait to be sent.
    pub fn queued(&self) -> usize {
        self.queue.len()
    }

    /// What is queued, taken out for sending. The gate is checked again: a mute or a switch-off that came after the sound was queued gives
    /// nothing and empties the queue.
    pub fn take_frames(&mut self) -> Result<Vec<Frame>, String> {
        if let Some(why) = self.gate.why_not() {
            while self.queue.pop().is_some() {}
            return Err(why);
        }
        let mut frames = Vec::new();
        while let Some(f) = self.queue.pop() {
            frames.push(f);
        }
        Ok(frames)
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
