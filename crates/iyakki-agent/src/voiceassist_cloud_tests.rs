// #[cfg(test)] -- this whole file is test code (it is included under #[cfg(test)] from lib.rs); the marker keeps the BOUNDARY check from counting it.
//! My IYAKKI in cloud mode (F-VOICE-38): the whole loop of the state machine with the microphone of a person's WEB PAGE, as a cloud tenant sees it.
//! The page sends 16 kHz mono chunks on the binary channel; `recorders_for` is the one place that turns them into a recording
//! (`browsersource.rs`) and `RecorderAudio` is the assistant's microphone over it. Speech engine, speaker and agent are fakes. Nothing needs a
//! microphone, a speaker or a network.

use std::sync::{Arc, Mutex};

use crate::browsermedia::{BrowserMedia, Kind, Policy, CH_MIC};
use crate::voiceassist::*;
use crate::voiceassist_ports::RecorderAudio;

#[derive(Default)]
struct Seen {
    pcm_lens: Vec<usize>,
    asked: Vec<String>,
    spoken: Vec<String>,
    speaker_left: u32,
}
type S = Arc<Mutex<Seen>>;

struct FakeStt(S);
impl Stt for FakeStt {
    fn hear(&mut self, pcm: &[i16]) -> Result<Heard, String> {
        self.0.lock().unwrap().pcm_lens.push(pcm.len());
        Ok(Heard { text: "what time is it".into(), confidence: Some(0.9) })
    }
}
struct FakeSpeaker(S);
impl Speaker for FakeSpeaker {
    fn say(&mut self, text: &str) -> Result<(), String> {
        let mut s = self.0.lock().unwrap();
        s.spoken.push(text.to_string());
        s.speaker_left = 3;
        Ok(())
    }
    fn busy(&mut self) -> bool {
        let mut s = self.0.lock().unwrap();
        if s.speaker_left > 0 {
            s.speaker_left -= 1;
            true
        } else {
            false
        }
    }
    fn stop(&mut self) {}
    fn level(&self) -> f32 {
        0.0
    }
    fn louder(&mut self) {}
    fn quieter(&mut self) {}
}
struct FakeAgent(S, Arc<Mutex<Vec<AgentEvent>>>);
impl AgentPort for FakeAgent {
    fn ask(&mut self, text: &str) {
        self.0.lock().unwrap().asked.push(text.to_string());
        self.1.lock().unwrap().push(AgentEvent::Final("It is ten past nine.".into()));
    }
    fn poll(&mut self) -> Option<AgentEvent> {
        self.1.lock().unwrap().pop()
    }
    fn decide(&mut self, _: &str, _: bool) {}
    fn cancel(&mut self) {}
}
struct NoLog;
impl TurnLog for NoLog {
    fn record(&mut self, _: Who, _: &str) {}
}

struct Dead;
impl iyakki_proxy::Upstream for Dead {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

/// 50 ms of the page's microphone at 16 kHz: `loud` or a quiet room.
fn chunk(loud: bool) -> Vec<u8> {
    let v: i16 = if loud { 20_000 } else { 200 };
    let mut m = vec![CH_MIC];
    for _ in 0..800 {
        m.extend_from_slice(&v.to_le_bytes());
    }
    m
}

fn world(tag: &str, user: &str, policy: Policy) -> (Assistant, Arc<BrowserMedia>, S, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("iyakki-cloudvoice-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let svc = crate::services::Services::open(&dir.join("svc"), Arc::new(Dead), Arc::new(Dead)).unwrap();
    let media = BrowserMedia::silent(policy);
    crate::browsermedia::register(user, media.clone());
    let seen: S = Arc::new(Mutex::new(Seen::default()));
    let events = Arc::new(Mutex::new(vec![]));
    let ports = Ports { audio: Box::new(RecorderAudio::new(crate::browsersource_svc::recorders_for(user), svc)), stt: Box::new(FakeStt(seen.clone())), speaker: Box::new(FakeSpeaker(seen.clone())), agent: Box::new(FakeAgent(seen.clone(), events)), log: Box::new(NoLog) };
    (Assistant::new(ports, Config::default(), false), media, seen, dir)
}

#[test]
fn a_person_in_a_web_page_talks_to_my_iyakki_through_the_pages_microphone_and_the_loop_runs_to_the_follow_up() {
    let user = "cloudvoice-a";
    let (mut a, media, seen, dir) = world("loop", user, Policy::allow_all());
    // the page's microphone is switched on by the person's own click; the assistant never switches it on
    media.set(Kind::Mic, true).unwrap();
    let mut t = 10_000u64;
    a.tap(t);
    assert_eq!(a.status().state, State::Listening);
    assert!(a.status().mic_open, "the indicator follows the open recording");
    // a quiet room, loud talking, then the pause that ends the request; the page sends a chunk every 50 ms
    let feed = |a: &mut Assistant, media: &BrowserMedia, loud: bool, ms: u64, t: &mut u64| {
        for _ in 0..ms / 50 {
            media.binary(&chunk(loud));
            *t += 50;
            a.tick(*t);
        }
    };
    feed(&mut a, &media, false, 300, &mut t);
    feed(&mut a, &media, true, 900, &mut t);
    feed(&mut a, &media, false, 1800, &mut t);
    // the fake agent answers at once, so the loop has already gone through thinking and speaking
    assert_eq!(seen.lock().unwrap().asked, vec!["what time is it"]);
    let n = seen.lock().unwrap().pcm_lens[0];
    assert!((16_000..=16_000 * 4).contains(&n), "about 3 seconds of 16 kHz sound reached the speech engine, not more: {n}");
    assert!(seen.lock().unwrap().spoken.join(" ").contains("ten past nine"), "the answer was spoken");
    assert_eq!(a.status().state, State::FollowUp, "{:?}", a.status());
    assert!(a.status().mic_open, "the follow-up window listens through the page again");
    // silence for the whole window: back to idle and the recording closed
    feed(&mut a, &media, false, DEFAULT_FOLLOW_MS + 500, &mut t);
    assert_eq!(a.status().state, State::Idle);
    assert!(!a.status().mic_open);
    crate::browsermedia::unregister(user);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_page_whose_microphone_is_not_allowed_gets_a_plain_sentence_and_nothing_is_recorded() {
    let user = "cloudvoice-b";
    let (mut a, media, seen, dir) = world("deny", user, Policy::deny_all());
    a.tap(10_000);
    assert_ne!(a.status().state, State::Listening, "no recording was started");
    assert!(!a.status().mic_open);
    assert!(a.status().note.contains("may not use the microphone"), "{:?}", a.status().note);
    media.binary(&chunk(true));
    assert!(seen.lock().unwrap().pcm_lens.is_empty() && seen.lock().unwrap().asked.is_empty());
    crate::browsermedia::unregister(user);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_page_that_asks_first_shows_its_allow_button_and_the_assistant_waits_for_the_click() {
    let user = "cloudvoice-c";
    let (mut a, media, _, dir) = world("ask", user, Policy::allow_all());
    a.tap(10_000);
    assert!(a.status().note.contains("Allow the microphone in your browser"), "{:?}", a.status().note);
    assert!(media.take_messages().iter().any(|m| m.contains("media_ask")), "the page is asked to show its button");
    assert!(!media.is_on(Kind::Mic));
    crate::browsermedia::unregister(user);
    let _ = std::fs::remove_dir_all(&dir);
}
