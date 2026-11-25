//! What the LEARN tile asks of the rest of the system (alpha.8 E3, the worked example of docs/development/APP-DEVELOPMENT-PROCEDURE.md section 10).
//!
//! The tile holds no `Services`, no `AppCx`, no file path, no chat backend and no voice: it asks a [`LearnPort`]. The real port is in
//! `crates/iyakki-apps/src/apps/learn_port.rs` (the only place that names the services; it also writes the audit lines and judges the
//! person's rights); the tests of this crate use [`crate::fake::FakePort`].

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use iyakki_study_model::study::{Plan, Quiz};
use iyakki_study_model::studylesson::{Lesson, Parsed};
use iyakki_study_model::studypdf::Item;

/// What a lesson run tells the page while it works (sent in order; the last call of `generate` returns the result).
#[derive(Clone, Debug)]
pub enum WriteEvent {
    /// The text so far.
    Text(Parsed),
    /// Picture `i` is finished: its bytes.
    Pic(usize, Vec<u8>),
    /// A plain sentence about what is being done now.
    Phase(String),
}

/// Writes lessons and answers questions with the assistant. One object serves the background threads of the page, so it is `Send + Sync`
/// and holds everything it needs (the chat backend, the study files, the picture model).
pub trait LessonWriter: Send + Sync {
    /// Write, draw and save the lesson of session `n` (1-based). `stop` is set when the person cancels. `Err` is the sentence to show.
    fn generate(&self, user: &str, plan: &Plan, n: usize, level: i32, on: &mut dyn FnMut(WriteEvent), stop: &AtomicBool) -> Result<Lesson, String>;
    /// Answer a question about a saved lesson, streaming the text so far into `on_text`; the exchange is saved with the lesson.
    fn ask(&self, user: &str, plan: &Plan, lesson: &Lesson, question: &str, stop: &AtomicBool, on_text: &mut dyn FnMut(&str)) -> Result<String, String>;
}

/// Draws a picture of a lesson (png, jpeg, gif, webp or a checked svg) from its bytes: the decoders are not the tile's.
pub trait Pictures {
    /// Forget what was drawn (a new lesson is opened or written).
    fn clear(&mut self);
    /// Draw picture `id` as wide as `max_w` allows at scale `s`; the pixels, or the sentence to show.
    fn draw(&mut self, g: &mut iyakki_uikit::gfx::Gfx, id: u64, bytes: &[u8], max_w: f32, s: f32) -> Result<Arc<tiny_skia::Pixmap>, String>;
}

/// Everything the tile asks. Every call that changes something is the person's own act, done by the core under their account; `Err` is
/// the sentence to show in the tile.
pub trait LearnPort {
    fn plans(&self, user: &str) -> Vec<Plan>;
    fn quizzes(&self, user: &str) -> Vec<Quiz>;
    /// Keep the answers of one pass through a quiz (the port stamps the time).
    fn record_attempt(&self, user: &str, quiz: &str, picks: &[Option<usize>]) -> Result<(), String>;
    /// Report (or take back the report of) question `q` of a quiz.
    fn flag_question(&self, user: &str, quiz: &str, q: usize, flagged: bool) -> Result<(), String>;
    /// Tick (or untick) session `n` (1-based) of a plan.
    fn set_done(&self, user: &str, plan: &str, n: usize, done: bool) -> Result<(), String>;
    fn delete_quiz(&self, user: &str, id: &str) -> Result<(), String>;
    /// Delete a plan and the calendar events it made.
    fn delete_plan(&self, user: &str, id: &str) -> Result<(), String>;
    /// Put the plan's sessions in the calendar: how many events were made.
    fn to_calendar(&self, user: &str, id: &str) -> Result<usize, String>;
    /// Take the plan's events out of the calendar: how many were removed.
    fn calendar_remove(&self, user: &str, id: &str) -> Result<usize, String>;
    /// The saved lesson of session `n` (1-based), when there is one.
    fn lesson(&self, user: &str, plan: &str, n: usize) -> Option<Lesson>;
    /// The bytes of one picture file of a saved lesson.
    fn lesson_image(&self, user: &str, plan: &str, n: usize, file: &str) -> Option<Vec<u8>>;
    /// Write the item as a PDF in the person's Downloads, under their file rules; the sentence to show. The audit line is the port's.
    fn export_pdf(&self, user: &str, item: &Item) -> Result<String, String>;
    /// The writer of lessons (clone it into a thread).
    fn writer(&self) -> Arc<dyn LessonWriter>;
    /// Record what the person did (`learn.lesson`, `learn.ask`, `learn.read`).
    fn audit(&self, user: &str, action: &str, detail: &str);
    /// Start reading `text` aloud (as TALES reads) or say in a sentence why that is not possible; how many parts will be read.
    fn read_aloud(&self, text: &str) -> Result<usize, String>;
    /// Is the voice reading now (playing, buffering or paused)?
    fn reading(&self) -> bool;
    /// Stop reading.
    fn stop_reading(&self);
}
