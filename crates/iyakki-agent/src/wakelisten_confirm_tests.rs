//! The second check of the wake phrase and the counts it feeds (F-VOICE-46), with a fake microphone, a fake spotter and a fake speech-to-text.

use super::testing::listener;
use super::*;
use crate::voiceassist::{Heard, Stt};
use crate::wakestats::Count;
use std::collections::VecDeque;

#[derive(Default)]
struct M {
    open: bool,
    frames: VecDeque<Vec<i16>>,
}

#[derive(Clone, Default)]
struct FakeMic(Arc<Mutex<M>>);

impl Mic for FakeMic {
    fn open(&mut self) -> Result<(), String> {
        self.0.lock().unwrap().open = true;
        Ok(())
    }
    fn is_open(&self) -> bool {
        self.0.lock().unwrap().open
    }
    fn drain(&mut self) -> Vec<i16> {
        self.0.lock().unwrap().frames.pop_front().unwrap_or_default()
    }
    fn alive(&mut self) -> bool {
        true
    }
    fn close(&mut self) {
        self.0.lock().unwrap().open = false;
    }
}

/// A speech-to-text that says one thing and remembers what clip it was given.
struct FakeStt {
    says: Result<String, String>,
    got: Arc<Mutex<Vec<usize>>>,
}

impl Stt for FakeStt {
    fn hear(&mut self, pcm: &[i16]) -> Result<Heard, String> {
        self.got.lock().unwrap().push(pcm.len());
        self.says.clone().map(|text| Heard { text, confidence: None })
    }
}

type Counts = Arc<Mutex<Vec<Count>>>;

fn rig(says: Result<String, String>, on: bool) -> (Listener, FakeMic, Arc<Mutex<Vec<usize>>>, Counts) {
    let mic = FakeMic::default();
    let got = Arc::new(Mutex::new(vec![]));
    let counts: Counts = Default::default();
    let c2 = counts.clone();
    let l = listener(Box::new(mic.clone()))
        .with_confirm(Box::new(SttConfirm(Box::new(FakeStt { says, got: got.clone() }))), Box::new(move || on))
        .with_counter(Box::new(move |c| c2.lock().unwrap().push(c)));
    (l, mic, got, counts)
}

const IN: Inputs = Inputs { want: true, paused: false, sens: 1 };

fn loud() -> Vec<i16> {
    let mut v = vec![0i16; FRAME];
    v[10] = 32000;
    v
}

#[test]
fn the_phrase_is_found_as_an_engine_may_spell_it_and_not_in_other_talk() {
    for yes in ["my iyakki", "My IYAKKI.", "my yaki", "Mai Iyaki, what time is it", "hey, my ya key", "myiyakki"] {
        assert!(phrase_in(yes), "{yes}");
    }
    for no in ["", "hello there", "turn the lights on please", "mike hawk", "i like pie", "thank you for watching"] {
        assert!(!phrase_in(no), "{no}");
    }
}

#[test]
fn a_wake_the_second_check_confirms_goes_through_and_both_counts_are_made() {
    let (mut l, mic, got, counts) = rig(Ok("my iyakki".into()), true);
    mic.0.lock().unwrap().frames.push_back(loud());
    let ev = l.step(IN, 0);
    assert!(ev.is_some(), "confirmed");
    assert_eq!(*counts.lock().unwrap(), vec![Count::Heard]);
    assert_eq!(got.lock().unwrap().len(), 1, "the engine heard one clip");
    assert!(!mic.0.lock().unwrap().open, "the microphone closed as before");
}

#[test]
fn a_wake_the_second_check_denies_is_dropped_counted_and_the_listener_goes_on_listening() {
    let (mut l, mic, _got, counts) = rig(Ok("what a lovely day".into()), true);
    mic.0.lock().unwrap().frames.push_back(loud());
    assert!(l.step(IN, 0).is_none(), "no wake");
    assert_eq!(*counts.lock().unwrap(), vec![Count::Heard, Count::Rejected]);
    assert!(mic.0.lock().unwrap().open && l.indicator(), "still listening, sign still on");
    // the next real wake still works (the spotter was re-armed)
    mic.0.lock().unwrap().frames.push_back(loud());
    assert!(l.step(IN, 100).is_none(), "the same engine says no again");
    assert_eq!(counts.lock().unwrap().iter().filter(|c| **c == Count::Rejected).count(), 2);
}

#[test]
fn with_the_check_off_the_engine_is_never_asked_and_with_no_engine_the_wake_is_taken_with_a_note() {
    let (mut l, mic, got, counts) = rig(Ok("nothing".into()), false);
    mic.0.lock().unwrap().frames.push_back(loud());
    assert!(l.step(IN, 0).is_some());
    assert!(got.lock().unwrap().is_empty(), "off means off: no sound goes to the engine");
    assert_eq!(*counts.lock().unwrap(), vec![Count::Heard]);
    let (mut l, mic, _g, _c) = rig(Err("the engine is on another machine you have not agreed to".into()), true);
    mic.0.lock().unwrap().frames.push_back(loud());
    assert!(l.step(IN, 0).is_some(), "taken without the check");
    assert!(l.note().contains("second check") && l.note().contains("not agreed"), "{}", l.note());
}

#[test]
fn the_clip_for_the_check_is_short_in_memory_only_and_wiped_when_the_microphone_closes() {
    let (mut l, mic, got, _c) = rig(Ok("no".into()), true);
    for i in 0..40 {
        mic.0.lock().unwrap().frames.push_back(vec![1i16; FRAME]);
        l.step(IN, i * 100);
    }
    assert!(l.tail.len() <= TAIL_SAMPLES);
    mic.0.lock().unwrap().frames.push_back(loud());
    l.step(IN, 5000);
    assert!(got.lock().unwrap()[0] <= TAIL_SAMPLES, "never more than 2.5 seconds");
    l.step(Inputs { want: false, ..IN }, 6000);
    assert!(l.tail.is_empty(), "wiped with the ring");
}
