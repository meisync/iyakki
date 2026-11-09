//! Tests of the My IYAKKI state machine with fake audio, a fake speech engine, a fake speaker and a scripted agent.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::voiceassist::*;

#[derive(Default)]
struct World {
    mic_level: f32,
    mic_open: bool,
    opens: u32,
    taken: u32,
    heard: VecDeque<Result<Heard, String>>,
    pcm_lens: Vec<usize>,
    spoken: Vec<String>,
    speaker_left: u32,
    speaker_level: f32,
    stops: u32,
    louder: u32,
    quieter: u32,
    asked: Vec<String>,
    events: VecDeque<AgentEvent>,
    decisions: Vec<(String, bool)>,
    cancels: u32,
    log: Vec<(Who, String)>,
}

type W = Arc<Mutex<World>>;

struct FakeAudio(W);
impl AudioIn for FakeAudio {
    fn open(&mut self) -> Result<(), String> {
        let mut w = self.0.lock().unwrap();
        if !w.mic_open {
            w.opens += 1;
        }
        w.mic_open = true;
        Ok(())
    }
    fn is_open(&self) -> bool {
        self.0.lock().unwrap().mic_open
    }
    fn level(&mut self) -> f32 {
        self.0.lock().unwrap().mic_level
    }
    fn alive(&mut self) -> bool {
        true
    }
    fn take(&mut self) -> Result<Vec<i16>, String> {
        let mut w = self.0.lock().unwrap();
        w.mic_open = false;
        w.taken += 1;
        Ok(vec![1000; 1600])
    }
    fn discard(&mut self) {
        self.0.lock().unwrap().mic_open = false;
    }
}
struct FakeStt(W);
impl Stt for FakeStt {
    fn hear(&mut self, pcm: &[i16]) -> Result<Heard, String> {
        let mut w = self.0.lock().unwrap();
        w.pcm_lens.push(pcm.len());
        w.heard.pop_front().unwrap_or_else(|| Err(crate::voicedesk::NO_SPEECH.into()))
    }
}
struct FakeSpeaker(W);
impl Speaker for FakeSpeaker {
    fn say(&mut self, text: &str) -> Result<(), String> {
        let mut w = self.0.lock().unwrap();
        w.spoken.push(text.to_string());
        w.speaker_left = 3;
        Ok(())
    }
    fn busy(&mut self) -> bool {
        let mut w = self.0.lock().unwrap();
        if w.speaker_left > 0 {
            w.speaker_left -= 1;
            true
        } else {
            false
        }
    }
    fn stop(&mut self) {
        let mut w = self.0.lock().unwrap();
        w.speaker_left = 0;
        w.stops += 1;
    }
    fn level(&self) -> f32 {
        self.0.lock().unwrap().speaker_level
    }
    fn louder(&mut self) {
        self.0.lock().unwrap().louder += 1;
    }
    fn quieter(&mut self) {
        self.0.lock().unwrap().quieter += 1;
    }
}
struct FakeAgent(W);
impl AgentPort for FakeAgent {
    fn ask(&mut self, text: &str) {
        self.0.lock().unwrap().asked.push(text.to_string());
    }
    fn poll(&mut self) -> Option<AgentEvent> {
        self.0.lock().unwrap().events.pop_front()
    }
    fn decide(&mut self, id: &str, yes: bool) {
        self.0.lock().unwrap().decisions.push((id.to_string(), yes));
    }
    fn cancel(&mut self) {
        self.0.lock().unwrap().cancels += 1;
    }
}
struct FakeLog(W);
impl TurnLog for FakeLog {
    fn record(&mut self, who: Who, text: &str) {
        self.0.lock().unwrap().log.push((who, text.to_string()));
    }
}

struct Rig {
    a: Assistant,
    w: W,
    t: u64,
}

fn rig() -> Rig {
    rig_with(Config::default(), false)
}
fn rig_with(cfg: Config, muted: bool) -> Rig {
    let w: W = Arc::new(Mutex::new(World::default()));
    let ports = Ports { audio: Box::new(FakeAudio(w.clone())), stt: Box::new(FakeStt(w.clone())), speaker: Box::new(FakeSpeaker(w.clone())), agent: Box::new(FakeAgent(w.clone())), log: Box::new(FakeLog(w.clone())) };
    Rig { a: Assistant::new(ports, cfg, muted), w, t: 10_000 }
}

impl Rig {
    fn run(&mut self, ms: u64) {
        let end = self.t + ms;
        while self.t < end {
            self.t += 50;
            self.a.tick(self.t);
        }
    }
    fn level(&self, l: f32) {
        self.w.lock().unwrap().mic_level = l;
    }
    /// The person speaks: a quiet room, loud talking, then the pause that ends it.
    fn talk(&mut self, text: &str, conf: Option<f32>) {
        self.w.lock().unwrap().heard.push_back(Ok(Heard { text: text.into(), confidence: conf }));
        self.level(0.01);
        self.run(300);
        self.level(0.8);
        self.run(900);
        self.level(0.01);
        self.run(1800);
    }
    fn tap_and_talk(&mut self, text: &str, conf: Option<f32>) {
        self.a.tap(self.t);
        self.talk(text, conf);
    }
    fn state(&self) -> State {
        self.a.status().state
    }
    fn asked(&self) -> Vec<String> {
        self.w.lock().unwrap().asked.clone()
    }
    fn spoken(&self) -> Vec<String> {
        self.w.lock().unwrap().spoken.clone()
    }
    fn push(&self, e: AgentEvent) {
        self.w.lock().unwrap().events.push_back(e);
    }
    fn decisions(&self) -> Vec<(String, bool)> {
        self.w.lock().unwrap().decisions.clone()
    }
}

fn approval(id: &str, tool: &str) -> AgentEvent {
    AgentEvent::Approval { id: id.into(), name: tool.into(), summary: "do a thing".into() }
}

#[test]
fn the_whole_loop_with_a_follow_up_window() {
    let mut r = rig();
    assert_eq!(r.state(), State::Idle);
    r.a.tap(r.t);
    assert_eq!(r.state(), State::Listening);
    assert!(r.a.status().mic_open, "the indicator follows the open microphone");
    r.talk("what time is it", Some(0.9));
    assert_eq!(r.state(), State::Thinking);
    assert_eq!(r.asked(), vec!["what time is it"]);
    assert!(!r.a.status().mic_open, "the microphone is closed while the agent thinks");
    r.push(AgentEvent::Final("It is **ten past nine**. Anything else?".into()));
    r.run(100);
    assert_eq!(r.state(), State::Speaking);
    r.run(2000);
    assert_eq!(r.state(), State::FollowUp, "after the answer the follow-up window listens");
    assert!(r.a.status().mic_open);
    let spoken = r.spoken().join(" ");
    assert!(spoken.contains("ten past nine") && !spoken.contains('*'), "spoken without markdown: {spoken}");
    // silence for the whole window: back to idle, microphone closed
    r.level(0.01);
    r.run(DEFAULT_FOLLOW_MS + 500);
    assert_eq!(r.state(), State::Idle);
    assert!(!r.a.status().mic_open);
    let log = r.w.lock().unwrap().log.clone();
    assert_eq!(log[0], (Who::Person, "what time is it".into()));
    assert_eq!(log[1].0, Who::Assistant);
}

#[test]
fn a_follow_up_needs_no_second_tap() {
    let mut r = rig();
    r.tap_and_talk("what is the weather", Some(0.9));
    r.push(AgentEvent::Final("Sunny.".into()));
    r.run(2500);
    assert_eq!(r.state(), State::FollowUp);
    r.talk("and tomorrow please", Some(0.9));
    assert_eq!(r.asked(), vec!["what is the weather", "and tomorrow please"]);
}

#[test]
fn silence_after_a_tap_ends_with_a_note_and_nothing_is_sent() {
    let mut r = rig();
    r.a.tap(r.t);
    r.level(0.01);
    r.run(FIRST_WORD_MS + 500);
    assert_eq!(r.state(), State::Idle);
    assert!(r.a.status().note.contains("did not hear"));
    assert!(r.asked().is_empty());
}

#[test]
fn a_second_tap_ends_the_request_early() {
    let mut r = rig();
    r.a.tap(r.t);
    r.w.lock().unwrap().heard.push_back(Ok(Heard { text: "open the calendar".into(), confidence: Some(0.9) }));
    r.level(0.01);
    r.run(300);
    r.level(0.8);
    r.run(500);
    r.a.tap(r.t);
    assert_eq!(r.asked(), vec!["open the calendar"]);
}

#[test]
fn control_words_are_handled_locally_and_never_reach_the_agent() {
    // stop
    let mut r = rig();
    r.tap_and_talk("stop", Some(0.9));
    assert!(r.asked().is_empty());
    assert_eq!(r.state(), State::Idle);
    // cancel
    let mut r = rig();
    r.tap_and_talk("never mind", Some(0.9));
    assert!(r.asked().is_empty());
    assert_eq!(r.state(), State::Idle);
    // louder and quieter
    let mut r = rig();
    r.tap_and_talk("louder", Some(0.9));
    assert_eq!(r.w.lock().unwrap().louder, 1);
    assert!(r.asked().is_empty());
    let mut r = rig();
    r.tap_and_talk("quieter", Some(0.9));
    assert_eq!(r.w.lock().unwrap().quieter, 1);
    // that's all
    let mut r = rig();
    r.tap_and_talk("That's all.", Some(0.9));
    r.run(1500);
    assert!(r.asked().is_empty());
    assert_eq!(r.state(), State::Idle, "no follow-up window after that's all");
    // mute: the microphone closes and stays closed
    let mut r = rig();
    r.tap_and_talk("my IYAKKI, mute", Some(0.9));
    assert!(r.asked().is_empty());
    assert!(r.a.is_muted());
    assert_eq!(r.state(), State::Muted);
    assert!(!r.a.status().mic_open);
}

#[test]
fn repeat_says_the_last_answer_again_without_asking_the_agent() {
    let mut r = rig();
    r.tap_and_talk("tell me a fact", Some(0.9));
    r.push(AgentEvent::Final("Honey never spoils.".into()));
    r.run(2500);
    let before = r.spoken().len();
    r.talk("say that again", Some(0.9));
    r.run(1500);
    assert_eq!(r.asked().len(), 1, "repeat is local");
    assert!(r.spoken().len() > before);
    assert!(r.spoken().last().unwrap().contains("Honey never spoils"));
}

#[test]
fn a_stop_during_speech_stops_it_and_cancels_the_run() {
    let mut r = rig();
    r.tap_and_talk("tell me a long story", Some(0.9));
    r.push(AgentEvent::Final("Once upon a time. There was a king. He had a hat.".into()));
    r.run(200);
    assert_eq!(r.state(), State::Speaking);
    r.a.stop();
    assert_eq!(r.state(), State::Idle);
    assert!(r.w.lock().unwrap().stops > 0);
    assert!(!r.a.status().mic_open);
}

#[test]
fn unsure_transcripts_are_read_back_before_anything_runs() {
    // low confidence
    let mut r = rig();
    r.tap_and_talk("delete my photos", Some(0.3));
    assert!(r.asked().is_empty(), "nothing runs on a transcript the engine is unsure of");
    r.run(300);
    assert!(r.spoken().iter().any(|s| s.starts_with("Did you say: delete my photos")));
    assert_eq!(r.state(), State::Confirming);
    r.talk("yes", Some(0.9));
    assert_eq!(r.asked(), vec!["delete my photos"]);
    // a very short transcript
    let mut r = rig();
    r.tap_and_talk("weather", Some(0.95));
    assert!(r.asked().is_empty());
    r.run(300);
    assert!(r.spoken()[0].contains("Did you say: weather"));
    r.talk("no", Some(0.9));
    assert!(r.asked().is_empty(), "a no sends nothing");
    r.run(1500);
    assert_eq!(r.state(), State::Idle);
    // a tap or a key answers it too
    let mut r = rig();
    r.tap_and_talk("hmm", None);
    assert!(r.a.confirm(true));
    assert_eq!(r.asked(), vec!["hmm"]);
    // something else said instead is a new request
    let mut r = rig();
    r.tap_and_talk("weather", Some(0.95));
    r.run(300);
    r.talk("what is the weather today", Some(0.9));
    assert_eq!(r.asked(), vec!["what is the weather today"]);
}

#[test]
fn a_yes_that_answers_the_assistants_question_needs_no_read_back() {
    let mut r = rig();
    r.tap_and_talk("remind me about the dentist", Some(0.9));
    r.push(AgentEvent::Final("Shall I put it in your calendar?".into()));
    r.run(2500);
    r.talk("yes", Some(0.9));
    assert_eq!(r.asked().last().unwrap(), "yes");
}

#[test]
fn an_ordinary_approval_can_be_given_by_voice() {
    let mut r = rig();
    r.tap_and_talk("set a timer for ten minutes", Some(0.9));
    r.push(approval("c1", "calendar_add"));
    r.run(200);
    assert_eq!(r.a.status().approval.as_ref().map(|a| a.hard), Some(false));
    r.run(1500);
    assert!(r.spoken().iter().any(|s| s.contains("do a thing") && s.contains("yes")));
    assert_eq!(r.state(), State::AwaitApproval);
    r.talk("yes, do it", Some(0.9));
    assert_eq!(r.decisions(), vec![("c1".to_string(), true)]);
    assert_eq!(r.state(), State::Thinking);
    assert!(r.a.status().approval.is_none());
}

#[test]
fn a_spoken_no_denies_even_a_hard_ask_tool() {
    let mut r = rig();
    r.tap_and_talk("remove the vpn", Some(0.9));
    r.push(approval("c1", "vpn_delete"));
    r.run(1800);
    r.talk("no", Some(0.9));
    assert_eq!(r.decisions(), vec![("c1".to_string(), false)]);
}

#[test]
fn a_spoken_yes_never_approves_the_hard_ask_list() {
    // the hard rule, for a spread of tools on the list (and the word list for unknown names)
    for tool in ["vpn_delete", "store_install", "message_send", "share_create", "driver_install", "k8s_delete", "dictation_wording_set", "voice_assistant_set", "some_new_delete_thing", "something_sendmail"] {
        assert!(is_hard(tool), "{tool} must be hard");
        let mut r = rig();
        r.tap_and_talk("please do it now", Some(0.9));
        r.push(approval("c9", tool));
        r.run(1800);
        assert_eq!(r.state(), State::AwaitApproval, "{tool}");
        // a spoken yes in any way it could arrive: a stray tap-to-talk, a barge-in, the follow-up
        r.a.tap(r.t);
        r.run(100);
        r.talk("yes, do it", Some(0.99));
        assert!(r.decisions().is_empty(), "{tool}: a spoken yes must not approve: {:?}", r.decisions());
        assert!(r.a.status().approval.is_some());
        // a tap or a key does
        assert!(!r.a.approve("wrong-id", true));
        assert!(r.a.approve("c9", true));
        assert_eq!(r.decisions(), vec![("c9".to_string(), true)]);
    }
}

#[test]
fn a_hard_ask_that_is_listening_still_refuses_a_spoken_yes() {
    // barge in over the question, then say yes: the microphone is open, the answer is still refused
    let mut r = rig();
    r.tap_and_talk("do the thing", Some(0.9));
    r.push(approval("h1", "store_install"));
    r.run(100);
    assert_eq!(r.state(), State::Speaking);
    r.level(0.9);
    r.run(300);
    assert_eq!(r.state(), State::AwaitApproval, "interrupted, listening for an answer");
    r.w.lock().unwrap().heard.push_back(Ok(Heard { text: "yes do it".into(), confidence: Some(0.99) }));
    r.level(0.01);
    r.run(100);
    r.level(0.8);
    r.run(800);
    r.level(0.01);
    r.run(1800);
    assert!(r.decisions().is_empty());
    assert!(r.spoken().iter().any(|s| s == SAID_TAP_ONLY));
    assert!(r.a.approve("h1", false));
    assert_eq!(r.decisions(), vec![("h1".to_string(), false)]);
}

#[test]
fn an_unclear_answer_twice_denies_an_ordinary_ask() {
    let mut r = rig();
    r.tap_and_talk("do something useful", Some(0.9));
    r.push(approval("c2", "calendar_add"));
    r.run(1800);
    r.talk("banana", Some(0.9));
    r.run(1500);
    assert!(r.spoken().iter().any(|s| s.contains("yes or no")));
    r.talk("banana again", Some(0.9));
    assert_eq!(r.decisions(), vec![("c2".to_string(), false)]);
}

#[test]
fn no_answer_in_time_denies_an_ordinary_ask_but_keeps_a_hard_card_waiting() {
    let mut r = rig();
    r.tap_and_talk("do something useful", Some(0.9));
    r.push(approval("c3", "calendar_add"));
    r.run(1800);
    r.level(0.01);
    r.run(APPROVAL_WAIT_MS * 3);
    assert_eq!(r.decisions(), vec![("c3".to_string(), false)], "silence is never a yes");
    let mut r = rig();
    r.tap_and_talk("do something useful", Some(0.9));
    r.push(approval("c4", "store_install"));
    r.run(60_000);
    assert!(r.decisions().is_empty());
    assert_eq!(r.state(), State::AwaitApproval, "the card waits for the person");
    assert!(!r.a.status().mic_open, "after two unanswered questions the microphone closes and only a tap answers");
}

#[test]
fn speaking_over_the_answer_stops_it_but_its_own_echo_does_not() {
    let mut r = rig();
    r.tap_and_talk("tell me a story", Some(0.9));
    r.push(AgentEvent::Final("One. Two. Three. Four. Five.".into()));
    r.run(100);
    assert_eq!(r.state(), State::Speaking);
    // the assistant is loud and the microphone hears the same loudness: echo
    r.w.lock().unwrap().speaker_level = 0.3;
    r.level(0.3);
    r.run(600);
    assert_eq!(r.state(), State::Speaking, "echo of its own voice is not a barge-in");
    // a person talks over it, clearly louder than the echo
    r.level(0.95);
    r.run(300);
    assert_eq!(r.state(), State::Listening, "barge-in");
    assert!(r.w.lock().unwrap().stops > 0);
    assert!(r.a.status().mic_open);
    // a click shorter than the barge-in time does not count
    let mut r = rig();
    r.tap_and_talk("tell me a story", Some(0.9));
    r.push(AgentEvent::Final("One. Two. Three. Four. Five.".into()));
    r.run(100);
    r.level(0.95);
    r.run(100);
    r.level(0.01);
    r.run(200);
    assert_eq!(r.state(), State::Speaking);
}

#[test]
fn barge_in_off_keeps_the_microphone_closed_while_speaking() {
    let mut r = rig_with(Config { barge_in: false, ..Config::default() }, false);
    r.tap_and_talk("tell me a story", Some(0.9));
    r.push(AgentEvent::Final("One. Two.".into()));
    r.run(100);
    assert_eq!(r.state(), State::Speaking);
    assert!(!r.a.status().mic_open);
}

#[test]
fn a_tap_while_speaking_interrupts_and_listens() {
    let mut r = rig();
    r.tap_and_talk("tell me a story", Some(0.9));
    r.push(AgentEvent::Final("One. Two. Three.".into()));
    r.run(100);
    r.a.tap(r.t);
    assert_eq!(r.state(), State::Listening);
}

#[test]
fn muted_never_opens_the_microphone() {
    let mut r = rig_with(Config::default(), true);
    r.a.tap(r.t);
    r.run(2000);
    assert_eq!(r.w.lock().unwrap().opens, 0);
    assert_eq!(r.state(), State::Muted);
    assert!(r.a.status().note.contains("Muted"));
    r.a.set_muted(false);
    r.a.tap(r.t);
    assert_eq!(r.state(), State::Listening);
    // muting while listening closes the microphone at once
    r.a.set_muted(true);
    assert!(!r.a.status().mic_open);
    assert_eq!(r.state(), State::Muted);
}

#[test]
fn a_failed_run_is_said_briefly_and_shown() {
    let mut r = rig();
    r.tap_and_talk("do the thing now", Some(0.9));
    r.push(AgentEvent::Failed("the model did not answer".into()));
    r.run(1500);
    assert!(r.a.status().note.contains("did not answer"));
    assert!(r.spoken().iter().any(|s| s.contains("did not work")));
    assert_eq!(r.state(), State::Idle);
}

#[test]
fn a_run_that_never_answers_is_stopped() {
    let mut r = rig();
    r.tap_and_talk("do the thing now", Some(0.9));
    r.run(THINK_MAX_MS + 1000);
    assert!(r.w.lock().unwrap().cancels > 0);
}

#[test]
fn nothing_is_written_to_disk() {
    // the machine and the real ports touch no file; the samples go microphone -> speech engine -> dropped
    for (name, src) in [("voiceassist.rs", include_str!("voiceassist.rs")), ("voiceassist_ports.rs", include_str!("voiceassist_ports.rs"))] {
        for bad in ["std::fs", "File::create", "File::open", "OpenOptions", "write_all", "encode_wav", "create_dir", "tempfile", "temp_dir"] {
            assert!(!src.contains(bad), "{name} must not touch files ({bad})");
        }
    }
    // and the samples of a whole conversation are only ever handed to the speech engine
    let mut r = rig();
    r.tap_and_talk("what time is it", Some(0.9));
    assert_eq!(r.w.lock().unwrap().pcm_lens.len(), 1);
    assert!(!r.a.status().mic_open);
}

#[test]
fn spoken_form_is_short_plain_and_points_to_the_screen() {
    let l = spoken_form("# Title\n\nSome *bold* text with a [link](https://a.b/c). See https://x.y now.\n```\ncode\n```\n");
    let j = l.join(" ");
    assert!(!j.contains('*') && !j.contains('#') && !j.contains("http") && !j.contains("code"), "{j}");
    assert!(j.contains(SAID_SCREEN), "a code block is left on the screen");
    let long = "This is a sentence about nothing in particular. ".repeat(40);
    let l = spoken_form(&long);
    assert!(l.last().unwrap() == SAID_SCREEN);
    assert!(l.iter().map(|s| s.len()).sum::<usize>() < SPOKEN_CHARS + 120);
    assert_eq!(spoken_form("   "), vec!["Done."]);
}

#[test]
fn words_are_matched_whole_not_inside_requests() {
    assert_eq!(normalize("Hey, My IYAKKI: That's all!"), "thats all");
    assert_eq!(control(&normalize("Stop.")), Some(Control::Stop));
    assert_eq!(control(&normalize("stop the music")), None, "a request, not a control");
    assert_eq!(control(&normalize("mute")), Some(Control::Mute));
    assert!(is_yes(&normalize("Yes, do it!")) && is_no(&normalize("No, don't.")));
    assert!(!is_yes("yes but delete everything"));
}

#[test]
fn the_voice_session_prompt_asks_for_spoken_style() {
    for w in ["short", "no markdown", "screen", "clarifying", "speech recognition"] {
        assert!(SESSION_PROMPT.contains(w), "{w}");
    }
    assert!(!THREAD_TITLE.is_empty());
}

#[test]
fn the_worker_follows_taps_and_the_mute_setting() {
    use crate::voiceassist_ports::{spawn, Cmd, Live};
    use std::sync::atomic::{AtomicBool, Ordering};
    let w: W = Arc::new(Mutex::new(World::default()));
    let ports = Ports { audio: Box::new(FakeAudio(w.clone())), stt: Box::new(FakeStt(w.clone())), speaker: Box::new(FakeSpeaker(w.clone())), agent: Box::new(FakeAgent(w.clone())), log: Box::new(FakeLog(w.clone())) };
    let muted = Arc::new(AtomicBool::new(false));
    let m2 = muted.clone();
    let worker = spawn(ports, Config::default(), Box::new(move || Live { muted: m2.load(Ordering::SeqCst), follow_ms: 8000, pause_ms: 1500 }), 5);
    let wait = |f: &dyn Fn(&Status) -> bool| {
        for _ in 0..400 {
            if f(&worker.status()) {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        false
    };
    worker.send(Cmd::Tap);
    assert!(wait(&|s| s.state == State::Listening && s.mic_open), "a tap opens the microphone");
    // the chat tool or Settings mutes: the worker closes the microphone by itself
    muted.store(true, Ordering::SeqCst);
    assert!(wait(&|s| s.muted && !s.mic_open && s.state == State::Muted), "muting closes the microphone");
    assert!(!w.lock().unwrap().mic_open);
    drop(worker);
    assert!(!w.lock().unwrap().mic_open, "dropping the worker closes the microphone");
}
