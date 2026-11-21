//! The machine's approval card for a client device's hard asks (CLIENT-SPLIT-DESIGN.md 7b rule 5). EXPERIMENTAL.
//!
//! [`DeviceCard`] is the `CardAsk` the broker of the client endpoint uses behind [`super::ClientConsent`]. It puts one question in the
//! approval store of the shell (`browserconsent::ConsentStore`, the same one the INBOX, the chat and the pop-up over the screen read) for
//! the person the device is signed in as, and waits for the answer there. Only "Allow this time" allows, and only that one use. No answer
//! in time, a full queue or a store that cannot take the question is a refusal; nothing ever approves by itself.

use std::sync::Arc;
use std::time::Duration;

use iyakki_core_api::broker::{CardAnswer, CardAsk, Caller};

use crate::browserconsent::{Answer, ConsentStore};


/// How long the card waits before it counts as "no".
pub const CARD_WAIT: Duration = Duration::from_secs(60);

/// What the card says, in plain words: which device, which person, what for.
pub fn card_text(device: &str, person: &str, capability: &str, label: &str) -> (String, String) {
    let what = match capability {
        "client.camera.offer" => "send its camera to this machine".to_string(),
        "client.microphone.offer" => "send its microphone to this machine".to_string(),
        "clipboard.client" => "put text on this machine's clipboard".to_string(),
        "files.client" => format!("send files between your home folder and the device ({label})"),
        "camera.view" => format!("show you a camera of this machine ({label})"),
        "speaker.play" => "play this machine's sound on the device".to_string(),
        _ => format!("use {capability} ({label})"),
    };
    let device = if device.trim().is_empty() { "A device" } else { device };
    (format!("Allow {device} to {what}?"), format!("{device}, signed in as {person}, asks to {what}. You are asked every time. Allow this time, or Deny. If nobody answers, it is denied."))
}

pub struct DeviceCard {
    services: Arc<crate::services::Services>,
    wait: Duration,
}

impl DeviceCard {
    pub fn new(services: Arc<crate::services::Services>) -> DeviceCard {
        DeviceCard { services, wait: CARD_WAIT }
    }

    pub fn with_wait(mut self, wait: Duration) -> DeviceCard {
        self.wait = wait;
        self
    }

    fn store(&self) -> &ConsentStore {
        &self.services.browser.consent
    }
}

impl CardAsk for DeviceCard {
    fn ask(&self, caller: &Caller, capability: &str, label: &str) -> CardAnswer {
        let (title, text) = card_text(&caller.device, &caller.person, capability, label);
        let Ok(id) = self.store().create_device(&caller.person, &caller.device, &title, &text, crate::clock::now_unix()) else { return CardAnswer::Deny };
        match self.store().wait(&id, self.wait) {
            Some(Answer::Allow) => CardAnswer::Allow,
            _ => CardAnswer::Deny,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_api::broker::CallerKind;

    struct Dead;
    impl iyakki_proxy::Upstream for Dead {
        fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
            Err("offline".into())
        }
    }

    fn who() -> Caller {
        Caller { app: "client".into(), kind: CallerKind::Client { device_id: "dev1".into() }, person: "maya".into(), device: "Maya's phone".into() }
    }

    fn services(tag: &str) -> (Arc<crate::services::Services>, std::path::PathBuf) {
        let d = std::env::temp_dir().join(format!("iyakki-devcard-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        (crate::services::Services::open(&d, Arc::new(Dead), Arc::new(Dead)).unwrap(), d)
    }

    #[test]
    fn the_card_names_the_device_and_the_person_in_plain_words() {
        let (title, text) = card_text("Maya's phone", "maya", "client.camera.offer", "Maya's phone camera");
        assert_eq!(title, "Allow Maya's phone to send its camera to this machine?");
        assert!(text.contains("Maya's phone, signed in as maya, asks to send its camera to this machine."), "{text}");
        assert!(text.contains("every time") && text.contains("denied"));
        assert!(card_text("", "maya", "client.microphone.offer", "").0.starts_with("Allow A device to send its microphone"));
        assert!(card_text("P", "m", "clipboard.client", "").1.contains("clipboard"));
    }

    #[test]
    fn the_question_waits_for_the_person_and_only_allow_this_time_allows() {
        let (svc, dir) = services("answer");
        let card = Arc::new(DeviceCard::new(svc.clone()).with_wait(Duration::from_secs(10)));
        for (answer, expect) in [(Answer::Allow, CardAnswer::Allow), (Answer::Deny, CardAnswer::Deny)] {
            let c = card.clone();
            let t = std::thread::spawn(move || c.ask(&who(), "client.camera.offer", "Maya's phone camera"));
            let req = loop {
                if let Some(r) = svc.browser.consent.pending("maya").into_iter().next() {
                    break r;
                }
                std::thread::sleep(Duration::from_millis(10));
            };
            assert_eq!(req.payload, crate::browserconsent::DEVICE_PAYLOAD);
            assert_eq!(req.domain, "Maya's phone");
            assert!(req.text.contains("signed in as maya") && req.title.contains("send its camera"));
            assert_eq!(req.choices.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), vec!["Allow this time", "Deny"]);
            assert!(svc.browser.consent.pending("sam").is_empty(), "only the person it is for sees it");
            svc.browser.consent.resolve("maya", &req.id, answer).unwrap();
            assert_eq!(t.join().unwrap(), expect);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn nobody_answering_is_a_refusal_and_the_question_is_withdrawn() {
        let (svc, dir) = services("timeout");
        let card = DeviceCard::new(svc.clone()).with_wait(Duration::from_millis(150));
        assert_eq!(card.ask(&who(), "client.microphone.offer", "x"), CardAnswer::Deny, "never approves by itself");
        assert!(svc.browser.consent.pending("maya").is_empty(), "the question went with the timeout");
        // a full queue refuses too
        for i in 0..crate::browserconsent::MAX_PENDING {
            svc.browser.consent.create_device("maya", "d", "t", &format!("q{i}"), 1).unwrap();
        }
        assert_eq!(card.ask(&who(), "client.camera.offer", "x"), CardAnswer::Deny);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_broker_decision_with_this_card_needs_the_grant_and_then_the_answer() {
        use iyakki_core_api::broker::{Call, Denial};
        let (svc, dir) = services("broker");
        let data = dir.join("client");
        std::fs::create_dir_all(&data).unwrap();
        let card = Arc::new(super::super::ClientConsent::new(Some(Arc::new(DeviceCard::new(svc.clone()).with_wait(Duration::from_millis(200))))));
        let (broker, _log) = crate::accessbroker::local(&data, Some(card));
        let caller = who();
        let call = Call { caller: &caller, capability: "client.camera.offer", unit: None, label: "cam" };
        assert_eq!(broker.decide(&call), Err(Denial::NotGranted), "no grant: not even asked");
        assert!(svc.browser.consent.pending("maya").is_empty());
        super::super::grant_device(&data, "dev1", "admin", &["client.camera.offer"], 1).unwrap();
        assert_eq!(broker.decide(&call), Err(Denial::CardDeclined), "granted, asked, nobody answered");
        let _ = std::fs::remove_dir_all(dir);
    }
}
