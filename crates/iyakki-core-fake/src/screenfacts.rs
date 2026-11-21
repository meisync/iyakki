//! The screens on the fake machine: connectors, the firmware framebuffer's signature and the cards a test sets; every read is counted.

use std::sync::Mutex;

use iyakki_core_api::screenfacts::{Connector, ScreenFacts};

#[derive(Default)]
struct State {
    connectors: Vec<(String, Connector)>,
    fb: Option<String>,
    reads: u32,
}

#[derive(Default)]
pub struct FakeScreenFacts {
    state: Mutex<State>,
}

impl FakeScreenFacts {
    /// Add a connector on a card; `card` is the card's device path as `card_with` will answer it.
    pub fn connector(&self, card: &str, name: &str, connected: bool, modes: &[(u32, u32)]) {
        self.state.lock().unwrap().connectors.push((card.into(), Connector { name: name.into(), connected, enabled: None, modes: modes.to_vec() }));
    }
    pub fn set_fb_signature(&self, s: Option<&str>) {
        self.state.lock().unwrap().fb = s.map(str::to_string);
    }
    /// How many reads the shell made (a read is a call of any method).
    pub fn reads(&self) -> u32 {
        self.state.lock().unwrap().reads
    }
}

impl ScreenFacts for FakeScreenFacts {
    fn connectors(&self) -> Vec<Connector> {
        let mut s = self.state.lock().unwrap();
        s.reads += 1;
        s.connectors.iter().map(|(_, c)| c.clone()).collect()
    }
    fn fb_signature(&self, _fb_path: &str) -> Option<String> {
        let mut s = self.state.lock().unwrap();
        s.reads += 1;
        s.fb.clone()
    }
    fn card_with(&self, at_least: usize) -> Option<String> {
        let mut s = self.state.lock().unwrap();
        s.reads += 1;
        let mut by: std::collections::BTreeMap<String, usize> = Default::default();
        for (card, c) in &s.connectors {
            if c.connected {
                *by.entry(card.clone()).or_default() += 1;
            }
        }
        by.into_iter().find(|(_, n)| *n >= at_least).map(|(c, _)| c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_answers_what_it_was_given_and_counts_the_reads() {
        let f = FakeScreenFacts::default();
        f.connector("/dev/dri/card0", "eDP-1", true, &[(2560, 1600)]);
        f.connector("/dev/dri/card1", "DP-1", true, &[(1920, 1080)]);
        f.connector("/dev/dri/card1", "DP-2", true, &[]);
        f.set_fb_signature(Some("simpledrmdrmfb 1x1 stride 4 32 bpp"));
        assert_eq!(f.signature(), "eDP-1:2560x1600,DP-1:1920x1080,DP-2:?");
        assert_eq!(f.card_with(2), Some("/dev/dri/card1".into()));
        assert_eq!(f.card_with(1), Some("/dev/dri/card0".into()));
        assert!(f.fb_signature("/dev/fb0").is_some());
        assert_eq!(f.reads(), 4);
    }
}
