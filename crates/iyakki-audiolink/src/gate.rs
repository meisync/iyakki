//! Who may send sound where (F-VOICE-38): the consent gate of the device side, shared by the agent and the connector program.
//! See `iyakki-agent::cloudstream` for the three conditions it holds in one place.

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

    pub fn why_not(&self) -> Option<String> {
        match self.verdict() {
            Verdict::Allowed => None,
            Verdict::Off => Some("Sending sound to the cloud machine is off (Settings -> Voice).".into()),
            Verdict::Card(_) => Some("Waiting for you to allow sending your sound to the cloud machine.".into()),
            Verdict::Denied => Some("You said no to sending your sound to the cloud machine.".into()),
            Verdict::Muted => Some("The microphone is muted, so nothing is sent.".into()),
        }
    }
}
