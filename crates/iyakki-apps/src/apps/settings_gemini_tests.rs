//! Settings -> Models -> Google Gemini (experimental): the setup, the consent card that is "No" until a person says Yes, taking the yes
//! back, and the test request that is sent only after the yes. A fake transport records everything that would leave the computer.

use super::*;
use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
use iyakki_shared::gemini;
use iyakki_proxy::{Upstream, UpstreamResponse};
use std::sync::Mutex;

const KEY: &str = "AIzaSyFAKE-KEY-0123456789abcdef";

/// Records every request; answers like Google would.
#[derive(Default)]
struct Wire(Mutex<Vec<(String, Vec<(String, String)>, String)>>);
impl Upstream for Wire {
    fn request(&self, _m: &str, url: &str, h: &[(String, String)], b: &[u8]) -> Result<UpstreamResponse, String> {
        self.0.lock().unwrap().push((url.into(), h.to_vec(), String::from_utf8_lossy(b).into()));
        Ok(UpstreamResponse { status: 200, body: br#"{"candidates":[{"content":{"parts":[{"text":"OK"}]}}]}"#.to_vec() })
    }
}

fn rig(tag: &str) -> (AppCx, Arc<Wire>) {
    let mut cx = cx_with_admin_store(tag);
    let wire = Arc::new(Wire::default());
    let d = temp_dir(&format!("{tag}-svc"));
    cx.services = services_in(&d, wire.clone());
    cx.services.on_login("root", "rootpass12", true);
    (cx, wire)
}

fn key(a: &mut SettingsApp, cx: &AppCx, k: Key) {
    a.handle(&Event::Key(k), cx);
}

fn typ(a: &mut SettingsApp, cx: &AppCx, s: &str) {
    for c in s.chars() {
        key(a, cx, Key::Char(c));
    }
}

fn settle(a: &mut SettingsApp, cx: &AppCx) {
    for _ in 0..500 {
        a.handle(&Event::Tick, cx);
        if !a.is_busy() {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    panic!("job did not finish");
}

fn status(a: &SettingsApp) -> String {
    a.status().map(|s| s.0.clone()).unwrap_or_default()
}

fn google_requests(w: &Wire) -> usize {
    w.0.lock().unwrap().iter().filter(|r| gemini::is_google_url(&r.0)).count()
}

#[test]
fn setup_stores_the_key_in_the_vault_then_the_card_starts_on_no_and_nothing_leaves_until_a_person_says_yes() {
    let (cx, wire) = rig("gemini-page");
    let mut a = SettingsApp::new(&cx).on(Tab::Models);
    assert!(a.hint().contains("Google Gemini (experimental)"));
    key(&mut a, &cx, Key::Char('g'));
    assert!(a.has_modal());
    typ(&mut a, &cx, KEY);
    key(&mut a, &cx, Key::Enter); // tier: free
    key(&mut a, &cx, Key::Enter); // chat model: the default
    key(&mut a, &cx, Key::Enter); // speech model: the default
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Right); // the assistant uses Gemini: yes
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Right); // speech-to-text uses Gemini: yes
    key(&mut a, &cx, Key::Enter); // save
    assert!(status(&a).contains("Nothing is sent to Google until you say Yes"), "{}", status(&a));
    // the key is in the system vault, and in no setting
    assert_eq!(&*cx.secrets().resolve(gemini::KEY_REF, None).unwrap(), KEY);
    {
        let g = cx.settings().lock().unwrap();
        let s = g.get();
        let p = s.provider(gemini::PROVIDER_ID).expect("provider");
        assert_eq!((p.base_url.as_deref(), p.key_ref.as_deref(), p.default_model.as_deref()), (Some(gemini::OPENAI_BASE), Some(gemini::KEY_REF), Some(gemini::DEFAULT_CHAT_MODEL)));
        assert_eq!(s.chat.as_ref().map(|m| m.provider.as_str()), Some(gemini::PROVIDER_ID));
        assert!(matches!(s.voice.stt, Some(iyakki_shared::voice::SpeechEngine::Gemini { .. })));
        assert!(!serde_json::to_string(s).unwrap().contains(KEY));
    }
    assert!(!std::fs::read_to_string(cx.data_dir().join(gemini::FILE)).unwrap().contains(KEY));
    // the consent card is open, and Enter alone answers No
    assert!(a.has_modal());
    key(&mut a, &cx, Key::Enter);
    assert!(!a.has_modal() && status(&a).contains("nothing is sent to Google"), "{}", status(&a));
    assert_eq!(gemini::consent_state(cx.data_dir(), "root"), gemini::ConsentState::Missing);
    // the test button does not send either: it asks again
    key(&mut a, &cx, Key::Char('t'));
    assert!(a.has_modal(), "the card again");
    key(&mut a, &cx, Key::Esc);
    // the speech engine and the chat are gated at the transport too
    assert!(cx.services.transcribe_samples(&iyakki_shared::voice::tone(300.0, 400)).unwrap_err().contains("nothing was sent to Google"));
    assert_eq!(google_requests(&wire), 0, "nothing reached the wire");
    // Yes: Right then Enter. The test request goes, with the key in a header
    key(&mut a, &cx, Key::Char('C'));
    key(&mut a, &cx, Key::Right);
    key(&mut a, &cx, Key::Enter);
    settle(&mut a, &cx);
    assert!(status(&a).contains("answered: OK"), "{}", status(&a));
    assert!(matches!(gemini::consent_state(cx.data_dir(), "root"), gemini::ConsentState::Given { .. }));
    {
        let w = wire.0.lock().unwrap();
        let sent: Vec<_> = w.iter().filter(|r| gemini::is_google_url(&r.0)).collect();
        assert_eq!(sent.len(), 1);
        assert!(sent[0].0.contains(":generateContent") && !sent[0].0.contains(KEY));
        assert!(sent[0].1.iter().any(|(k, v)| k == "x-goog-api-key" && v == KEY));
        assert!(sent[0].2.contains("Reply with the single word") && !sent[0].2.contains("inline_data"));
    }
    assert!(cx.audit.recent(30).iter().any(|l| l.contains("settings.gemini") && l.contains("consent given (free tier)")));
    assert!(!cx.audit.recent(30).iter().any(|l| l.contains(KEY)));
    // changing the tier asks again
    key(&mut a, &cx, Key::Char('g'));
    key(&mut a, &cx, Key::Enter);
    key(&mut a, &cx, Key::Right); // paid
    for _ in 0..5 {
        key(&mut a, &cx, Key::Enter);
    }
    assert!(a.has_modal(), "the card is shown again for the paid tier");
    assert!(matches!(gemini::consent_state(cx.data_dir(), "root"), gemini::ConsentState::OtherTier { .. }));
    key(&mut a, &cx, Key::Esc);
    let before = google_requests(&wire);
    assert!(cx.services.transcribe_samples(&iyakki_shared::voice::tone(300.0, 400)).is_err());
    assert_eq!(google_requests(&wire), before);
    // taking the yes back (after giving it again)
    key(&mut a, &cx, Key::Char('C'));
    key(&mut a, &cx, Key::Right);
    key(&mut a, &cx, Key::Enter);
    settle(&mut a, &cx);
    key(&mut a, &cx, Key::Char('C'));
    assert!(a.has_modal(), "the question about taking it back");
    key(&mut a, &cx, Key::Char('y'));
    assert_eq!(gemini::consent_state(cx.data_dir(), "root"), gemini::ConsentState::Missing);
    assert!(status(&a).contains("taken back"));
}

#[test]
fn an_ordinary_account_cannot_set_it_up_but_can_answer_for_itself() {
    let (mut cx, _) = rig("gemini-user");
    cx.session.user = "amy".into();
    cx.session.policy.manage_settings = false;
    let mut a = SettingsApp::new(&cx).on(Tab::Models);
    key(&mut a, &cx, Key::Char('g'));
    assert!(!a.has_modal(), "no setup for an ordinary account");
    key(&mut a, &cx, Key::Char('C'));
    assert!(a.has_modal(), "the consent card is each person's own");
}
