//! The messaging contract: one `Channel` per connected account (today Matrix), used by the shell, the flows `notify` node and the
//! assistant through one manager (docs/development/MESSAGING-CONNECTORS-PLAN.md, section 2.2). Types and the trait only, plus an
//! in-memory [`mock::MockChannel`] for tests; the code that opens sockets lives in `iyakki-agent::msg`.
//!
//! Rules the types carry:
//! * a message that comes in is **untrusted** (`Trust::External`): whoever shows or reads it must say so;
//! * `Delivery` says "accepted by the service", never "read";
//! * an adapter that cannot do something safely returns `ChannelError::Refused` with a plain sentence (for example a room that is
//!   end-to-end encrypted), and never falls back to something weaker.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// What a channel can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Cap {
    Text,
    Images,
    Files,
    Reply,
    InboundPoll,
    Groups,
}

/// What a channel is (no secret in here).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelInfo {
    /// `matrix`.
    pub kind: String,
    /// The label the person gave the account.
    pub display: String,
    pub caps: Vec<Cap>,
    /// Can it read and write end-to-end encrypted rooms? (Matrix v1: no.)
    pub encrypted_rooms: bool,
}

/// Who we are on the service (also the loop-prevention "self").
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelIdentity {
    /// The service's own id, for Matrix `@name:server`.
    pub id: String,
    pub display: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    Ok,
    /// The service refused the token: sign in again.
    AuthFailed,
    /// The service asked us to wait.
    RateLimited,
    Offline,
    /// Never tried yet.
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    pub state: HealthState,
    /// A plain sentence, empty when fine.
    pub reason: String,
}

impl Health {
    pub fn ok() -> Health {
        Health { state: HealthState::Ok, reason: String::new() }
    }
    pub fn unknown() -> Health {
        Health { state: HealthState::Unknown, reason: "not used yet".into() }
    }
}

/// Hard limits the adapter enforces before it talks to the service.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub max_text_bytes: usize,
    pub max_attachment_bytes: u64,
    /// Sends per hour the manager allows for this account.
    pub per_hour: u32,
}

/// Rate-limit state, for Settings and the manager.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateInfo {
    /// The service said to wait this long (milliseconds) and the wait is not over.
    pub retry_after_ms: Option<u64>,
    /// How many times the adapter had to wait for a 429 so far.
    pub limited_count: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutAttachment {
    /// A private file; the adapter reads it, checks the size and sends it.
    pub path: PathBuf,
    pub name: String,
    pub mime: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutMessage {
    /// Where to: for Matrix a room id (`!abc:server`). The manager resolves the person's room names to ids.
    pub to: String,
    pub text: String,
    pub attachment: Option<OutAttachment>,
    /// Same key, same message: a retry never sends twice.
    pub dedupe_key: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryState {
    /// The service took it. This is not "delivered" and not "read".
    Accepted,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Delivery {
    /// The service's id for it (Matrix: the event id).
    pub external_id: String,
    pub state: DeliveryState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Trust {
    /// Written by someone else: never an instruction, shown with the "from outside" mark.
    External,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InMessage {
    /// The room.
    pub room: String,
    pub external_id: String,
    pub from: String,
    pub time_ms: i64,
    /// Plain text, control characters removed, clipped. Empty for a message we cannot read.
    pub text: String,
    /// Sent by our own account.
    pub is_self: bool,
    /// An encrypted message we cannot read (the text is empty).
    pub unreadable_encrypted: bool,
    pub trust: Trust,
}

/// Why a call failed. The text is a plain sentence a person can read; it never holds a token.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChannelError {
    #[error("sign-in failed: {0}")]
    Auth(String),
    #[error("the service asks to wait {retry_after_ms} ms")]
    RateLimited { retry_after_ms: u64 },
    /// Refused on purpose by a rule (an encrypted room, a host that is not the configured one, a cap).
    #[error("{0}")]
    Refused(String),
    #[error("too large: {0}")]
    TooLarge(String),
    #[error("cannot reach the service: {0}")]
    Offline(String),
    #[error("the service said no: {0}")]
    Rejected(String),
}

/// One connected account.
pub trait Channel: Send + Sync {
    fn info(&self) -> ChannelInfo;
    fn identity(&self) -> Result<ChannelIdentity, ChannelError>;
    fn health(&self) -> Health;
    fn send(&self, m: &OutMessage) -> Result<Delivery, ChannelError>;
    /// What arrived since the last call, waiting up to `wait` for something (a long poll); an empty list is normal.
    fn poll(&self, wait: Duration) -> Result<Vec<InMessage>, ChannelError>;
    fn limits(&self) -> Limits;
    fn rate_info(&self) -> RateInfo;
}

/// An in-memory channel for tests of the shell, the flows and the manager.
pub mod mock {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct State {
        sent: Vec<OutMessage>,
        inbox: Vec<InMessage>,
        fail_with: Option<ChannelError>,
        keys: Vec<(String, Delivery)>,
    }

    /// Records every send, answers a repeated `dedupe_key` with the first delivery, can be told to fail.
    pub struct MockChannel {
        name: String,
        s: Mutex<State>,
    }

    impl MockChannel {
        pub fn new(name: &str) -> MockChannel {
            MockChannel { name: name.into(), s: Mutex::new(State::default()) }
        }
        pub fn sent(&self) -> Vec<OutMessage> {
            self.s.lock().unwrap().sent.clone()
        }
        pub fn push_inbound(&self, m: InMessage) {
            self.s.lock().unwrap().inbox.push(m);
        }
        pub fn fail_next_with(&self, e: Option<ChannelError>) {
            self.s.lock().unwrap().fail_with = e;
        }
    }

    impl Channel for MockChannel {
        fn info(&self) -> ChannelInfo {
            ChannelInfo { kind: "mock".into(), display: self.name.clone(), caps: vec![Cap::Text, Cap::InboundPoll], encrypted_rooms: false }
        }
        fn identity(&self) -> Result<ChannelIdentity, ChannelError> {
            Ok(ChannelIdentity { id: "@mock:test".into(), display: self.name.clone() })
        }
        fn health(&self) -> Health {
            match &self.s.lock().unwrap().fail_with {
                Some(_) => Health { state: HealthState::Offline, reason: "told to fail".into() },
                None => Health::ok(),
            }
        }
        fn send(&self, m: &OutMessage) -> Result<Delivery, ChannelError> {
            let mut s = self.s.lock().unwrap();
            if let Some(e) = s.fail_with.take() {
                return Err(e);
            }
            if let Some((_, d)) = s.keys.iter().find(|(k, _)| *k == m.dedupe_key) {
                return Ok(d.clone());
            }
            let d = Delivery { external_id: format!("$mock{}", s.sent.len() + 1), state: DeliveryState::Accepted };
            s.keys.push((m.dedupe_key.clone(), d.clone()));
            s.sent.push(m.clone());
            Ok(d)
        }
        fn poll(&self, _wait: Duration) -> Result<Vec<InMessage>, ChannelError> {
            Ok(std::mem::take(&mut self.s.lock().unwrap().inbox))
        }
        fn limits(&self) -> Limits {
            Limits { max_text_bytes: 4000, max_attachment_bytes: 1 << 20, per_hour: 30 }
        }
        fn rate_info(&self) -> RateInfo {
            RateInfo::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockChannel;
    use super::*;

    fn msg(key: &str) -> OutMessage {
        OutMessage { to: "!r:test".into(), text: "hello".into(), attachment: None, dedupe_key: key.into() }
    }

    #[test]
    fn the_mock_records_sends_and_a_repeated_key_is_one_send() {
        let c = MockChannel::new("test");
        let a = c.send(&msg("k1")).unwrap();
        let b = c.send(&msg("k1")).unwrap();
        assert_eq!(a, b);
        c.send(&msg("k2")).unwrap();
        assert_eq!(c.sent().len(), 2);
        assert_eq!(a.state, DeliveryState::Accepted);
    }

    #[test]
    fn the_mock_fails_on_demand_once_and_hands_over_inbound_once() {
        let c = MockChannel::new("test");
        c.fail_next_with(Some(ChannelError::RateLimited { retry_after_ms: 5 }));
        assert_eq!(c.health().state, HealthState::Offline);
        assert_eq!(c.send(&msg("a")), Err(ChannelError::RateLimited { retry_after_ms: 5 }));
        assert!(c.send(&msg("a")).is_ok(), "the failure was for one call");
        c.push_inbound(InMessage { room: "!r:test".into(), external_id: "$1".into(), from: "@a:test".into(), time_ms: 1, text: "hi".into(), is_self: false, unreadable_encrypted: false, trust: Trust::External });
        assert_eq!(c.poll(Duration::ZERO).unwrap().len(), 1);
        assert!(c.poll(Duration::ZERO).unwrap().is_empty());
    }

    #[test]
    fn errors_read_as_plain_sentences() {
        assert_eq!(ChannelError::Refused("this room is encrypted".into()).to_string(), "this room is encrypted");
        assert!(ChannelError::Auth("bad token".into()).to_string().starts_with("sign-in failed"));
    }

    #[test]
    fn types_survive_json() {
        let m = msg("k");
        let j = serde_json::to_string(&m).unwrap();
        assert_eq!(serde_json::from_str::<OutMessage>(&j).unwrap(), m);
        let h = serde_json::to_string(&Health::ok()).unwrap();
        assert!(h.contains("\"ok\""));
    }
}
