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

use std::sync::mpsc::{channel, Receiver, Sender};

use crate::audiostream::{Frame, Jitter};

pub use iyakki_audiolink::gate::{Gate, Verdict};
pub use iyakki_audiolink::uplink::{Uplink, QUEUE_MS};

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
