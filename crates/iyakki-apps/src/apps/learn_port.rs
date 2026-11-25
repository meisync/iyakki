//! What the LEARN tile asks of the rest of the system, answered with the real services (alpha.8 E3; the tile is the crate `iyakki-tile-learn`,
//! its trait is `iyakki_tile_learn::LearnPort`). This file is the only place that names `cx.study()`, the calendar, the PDF export, the chat
//! backend, the voice and the picture decoders for LEARN. It also writes the audit lines and judges the person's rights by the same code the chat's
//! `study_*` tools use, so a plan made in the chat is here and a session ticked here is seen by the chat.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use iyakki_clock::{now_unix, Zone};
use iyakki_shared::hostvoice::HostVoice;
use iyakki_shared::story as st;
use iyakki_shared::story::{Status, TtsStatus};
use iyakki_shared::study::{Plan, Quiz};
use iyakki_shared::studylesson::{self as sl, Lesson};
use iyakki_shared::studypdf::Item;
use iyakki_tile_learn::{LearnPort, LessonWriter, Pictures, WriteEvent};

use super::chatpic::PicCache;
use super::AppCx;

pub(super) type Reader = Arc<dyn iyakki_agent_api::StoryReader>;

/// The reader of the voice and, in tests, a voice that replaces the real engines. It lives as long as the tile, not as long as one call.
#[derive(Default)]
pub struct VoiceSlot {
    reader: Mutex<Option<Reader>>,
    #[cfg(any(test, feature = "testing"))]
    pub test_voice: Mutex<Option<(Arc<dyn st::Synth>, Arc<dyn st::Sink>)>>,
}

impl VoiceSlot {
    fn playing(&self) -> bool {
        self.reader.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().is_some_and(|r| matches!(r.status(), Status::Playing | Status::Buffering | Status::Paused))
    }
}

/// The real port: borrows the person's `AppCx` (and the tile's voice) for one call.
pub struct CxPort<'a> {
    pub cx: &'a AppCx,
    pub voice: &'a VoiceSlot,
}

impl CxPort<'_> {
    /// The speech engine and the speaker, chosen as TALES chooses them.
    fn engines(&self) -> Result<(Arc<dyn st::Synth>, Arc<dyn st::Sink>), String> {
        #[cfg(any(test, feature = "testing"))]
        if let Some(v) = self.voice.test_voice.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
            return Ok(v.clone());
        }
        let cx = self.cx;
        let v = cx.voice_settings();
        let host = HostVoice::current();
        cx.host_tts().ensure();
        let auto = if v.tts.is_none() { cx.host_tts().auto_engine(Some("en")) } else { None };
        let path = std::env::var("PATH").unwrap_or_default();
        match crate::agent().helpers_b7().tts_status(&v, cx.session.policy.voice, &path, &host, auto.as_ref()) {
            TtsStatus::Ready { .. } => {}
            other => return Err(crate::agent().helpers_b7().tts_status_text(&other).replace("The story is", "The lesson is").replace("the story is", "the lesson is").replace("story", "lesson")),
        }
        let engine = iyakki_shared::storyvoice::effective_engine_auto(&v, &host, auto.as_ref());
        let opts = iyakki_shared::voice::SpeakOpts { voice: None, speed: 1.0, lang: Some("en".into()) };
        let browser = engine == Some(iyakki_shared::voice::SpeechEngine::Browser);
        let direct = engine.as_ref().filter(|e| super::speaks_directly(e)).and_then(|e| super::direct_voice(cx.agent.as_ref(), e));
        Ok(if browser {
            host.want_probe();
            (super::story::browser_synth(), super::story::browser_sink(HostVoice::current(), None, Some("en".into()), 1.0))
        } else if let Some(d) = direct {
            (d.synth(), d.sink(None, Some("en"), 1.0))
        } else {
            let play = cx.agent.hostengines_player_command(&v.play_cmd, &path).unwrap_or(v.play_cmd.clone());
            (cx.services.reading_synth(opts), crate::agent().helpers_b7().command_sink(&play))
        })
    }
}

impl LearnPort for CxPort<'_> {
    fn plans(&self, user: &str) -> Vec<Plan> {
        self.cx.study().plans(user)
    }
    fn quizzes(&self, user: &str) -> Vec<Quiz> {
        self.cx.study().quizzes(user)
    }
    fn record_attempt(&self, user: &str, quiz: &str, picks: &[Option<usize>]) -> Result<(), String> {
        self.cx.study().record_attempt(user, quiz, picks, now_unix()).map(|_| ())
    }
    fn flag_question(&self, user: &str, quiz: &str, q: usize, flagged: bool) -> Result<(), String> {
        self.cx.study().flag_question(user, quiz, q, flagged)
    }
    fn set_done(&self, user: &str, plan: &str, n: usize, done: bool) -> Result<(), String> {
        self.cx.study().set_done(user, plan, n, done).map(|_| ())
    }
    fn delete_quiz(&self, user: &str, id: &str) -> Result<(), String> {
        self.cx.study().delete_quiz(user, id).map(|_| ())
    }
    fn delete_plan(&self, user: &str, id: &str) -> Result<(), String> {
        self.cx.study_calendar().delete_plan(user, id).map(|_| ())
    }
    fn to_calendar(&self, user: &str, id: &str) -> Result<usize, String> {
        self.cx.study_calendar().to_calendar(user, id, &Zone::system())
    }
    fn calendar_remove(&self, user: &str, id: &str) -> Result<usize, String> {
        self.cx.study_calendar().calendar_remove(user, id)
    }
    fn lesson(&self, user: &str, plan: &str, n: usize) -> Option<Lesson> {
        self.cx.study().lesson(user, plan, n)
    }
    fn lesson_image(&self, user: &str, plan: &str, n: usize, file: &str) -> Option<Vec<u8>> {
        self.cx.study().lesson_image(user, plan, n, file)
    }
    fn export_pdf(&self, user: &str, item: &Item) -> Result<String, String> {
        let cx = self.cx;
        let m = cx.b5().study_pdf_export(&cx.session.policy, user, cx.data_dir(), item, true, None)?;
        cx.audit.record(&cx.session.user, "learn.pdf", &format!("{item:?}"));
        Ok(m)
    }
    fn writer(&self) -> Arc<dyn LessonWriter> {
        Arc::new(RealWriter { chat: self.cx.chat.clone(), svc: self.cx.services.clone() })
    }
    fn audit(&self, user: &str, action: &str, detail: &str) {
        self.cx.audit.record(user, action, detail);
    }
    fn read_aloud(&self, text: &str) -> Result<usize, String> {
        let chunks = st::sentence_chunks(text, st::MAX_CHUNK_CHARS);
        if chunks.is_empty() {
            return Err("There is no lesson to read yet.".into());
        }
        let (synth, sink) = self.engines()?;
        let n = chunks.len();
        let mut slot = self.voice.reader.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let r = slot.get_or_insert_with(crate::new_reader);
        r.start(chunks, 0, false, "learn", synth, sink);
        Ok(n)
    }
    fn reading(&self) -> bool {
        self.voice.playing()
    }
    fn stop_reading(&self) {
        if let Some(r) = self.voice.reader.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref() {
            r.stop();
        }
    }
}

/// The lesson writer of the installation: the chat backend writes and answers, the study files keep the lesson, the picture model draws.
pub struct RealWriter {
    chat: Arc<dyn iyakki_shared::chat::ChatBackend>,
    svc: Arc<dyn iyakki_agent_api::ServicesHandle>,
}

impl LessonWriter for RealWriter {
    fn generate(&self, user: &str, plan: &Plan, n: usize, level: i32, on: &mut dyn FnMut(WriteEvent), stop: &AtomicBool) -> Result<Lesson, String> {
        let on = std::cell::RefCell::new(on);
        let mut on_text = |p: &sl::Parsed| (on.borrow_mut())(WriteEvent::Text(p.clone()));
        let mut on_pic = |i: usize, _ext: &str, b: &[u8]| (on.borrow_mut())(WriteEvent::Pic(i, b.to_vec()));
        let mut on_phase = |s: &str| (on.borrow_mut())(WriteEvent::Phase(s.to_string()));
        sl::generate(self.chat.as_ref(), &*self.svc.lesson_host(), user, plan, n, level, &mut sl::Hooks { on_text: &mut on_text, on_pic: &mut on_pic, on_phase: &mut on_phase, stop })
    }
    fn ask(&self, user: &str, plan: &Plan, lesson: &Lesson, question: &str, stop: &AtomicBool, on_text: &mut dyn FnMut(&str)) -> Result<String, String> {
        sl::ask(self.chat.as_ref(), &*self.svc.lesson_host(), user, plan, lesson, question, stop, on_text)
    }
}

/// The pictures of a lesson, decoded and drawn by the shell's picture cache (png, jpeg, gif, webp and checked svg).
#[derive(Default)]
pub struct ShellPictures(pub Arc<Mutex<PicCache>>);

impl Pictures for ShellPictures {
    fn clear(&mut self) {
        *self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = PicCache::default();
    }
    fn draw(&mut self, g: &mut crate::gfx::Gfx, id: u64, bytes: &[u8], max_w: f32, s: f32) -> Result<Arc<tiny_skia::Pixmap>, String> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).draw_bytes(g, id, bytes, max_w, s).map(|r| r.pm)
    }
}
