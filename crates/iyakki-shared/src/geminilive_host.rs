//! Types moved out of `iyakki_agent::geminilive_host` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.



/// What the shell draws.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunState {
    /// Connecting (the consent gate, the key and the network are being checked).
    Connecting,
    /// The microphone is open (the red sign is on).
    Open,
    /// It could not start: the one line for the person (the local voice answers instead).
    Fallback(String),
    /// It ended: why, in one line.
    Ended(String),
}

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// The id of the speaker question's card.
pub const SPEAKER_CARD: &str = "voice-speaker";

/// The approval card, the same text the chat shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    pub id: String,
    pub name: String,
    pub summary: String,
    pub detail: String,
}

/// The person's screen. Implemented by the shell's board (the chat's approval card); a fake in tests. `answer` returns only what the person
/// pressed.
pub trait Screen: Send + Sync {
    /// The person is at the screen (a hard-ask card can be answered).
    fn present(&self) -> bool;
    fn show(&self, card: &Card);
    /// `Some(true)` Allow, `Some(false)` Deny, `None` not yet.
    fn answer(&self, id: &str) -> Option<bool>;
    /// The card is no longer wanted (cancelled or timed out).
    fn withdraw(&self, id: &str);
}

struct Entry {
    card: Card,
    answer: Option<bool>,
}

/// The cards waiting for the person. Implements the dispatcher's [`Screen`].
#[derive(Default)]
pub struct Board {
    present: AtomicBool,
    items: Mutex<Vec<Entry>>,
}

impl Board {
    pub fn new() -> Arc<Board> {
        Arc::new(Board::default())
    }

    /// The person is at the screen (the shell sets it while a person is signed in and the screen is not locked).
    pub fn set_present(&self, on: bool) {
        self.present.store(on, Ordering::SeqCst);
    }

    /// The cards that wait for a press.
    pub fn pending(&self) -> Vec<Card> {
        self.items.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().filter(|e| e.answer.is_none()).map(|e| e.card.clone()).collect()
    }

    /// The person pressed Allow or Deny on the card. Only the shell's button handler calls this.
    pub fn press(&self, id: &str, allow: bool) -> bool {
        let mut v = self.items.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        match v.iter_mut().find(|e| e.card.id == id && e.answer.is_none()) {
            Some(e) => {
                e.answer = Some(allow);
                true
            }
            None => false,
        }
    }

    /// A card of our own (the speaker question).
    pub fn ask(&self, card: Card) {
        self.show(&card);
    }

    pub fn clear(&self) {
        self.items.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    }
}

impl Screen for Board {
    fn present(&self) -> bool {
        self.present.load(Ordering::SeqCst)
    }
    fn show(&self, card: &Card) {
        let mut v = self.items.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        v.retain(|e| e.card.id != card.id);
        v.push(Entry { card: card.clone(), answer: None });
    }
    fn answer(&self, id: &str) -> Option<bool> {
        self.items.lock().unwrap_or_else(std::sync::PoisonError::into_inner).iter().find(|e| e.card.id == id).and_then(|e| e.answer)
    }
    fn withdraw(&self, id: &str) {
        self.items.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|e| e.card.id != id);
    }
}

/// A session on its thread: the state the shell draws, "I am done talking", and the stop.
pub struct Running {
    state: Arc<Mutex<RunState>>,
    done_talking: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
    stop: Arc<dyn Fn() + Send + Sync>,
}

impl Running {
    /// Made by the core when it starts the session thread (`stop` ends the live session at once).
    pub fn new(state: Arc<Mutex<RunState>>, done_talking: Arc<AtomicBool>, join: Option<std::thread::JoinHandle<()>>, stop: Arc<dyn Fn() + Send + Sync>) -> Running {
        Running { state, done_talking, join, stop }
    }

    pub fn state(&self) -> RunState {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub fn finished(&self) -> bool {
        self.join.as_ref().is_none_or(std::thread::JoinHandle::is_finished)
    }

    /// "I am done talking": hand the turn over; the session ends after the answer.
    pub fn finish_turn(&self) {
        self.done_talking.store(true, Ordering::SeqCst);
    }

    /// End it now (Esc, the Stop button, the chat tool): the microphone closes at the thread's next step.
    pub fn stop(&self) {
        (self.stop)();
    }

    /// Waits for the thread (tests, sign-out).
    pub fn join(mut self) {
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}
