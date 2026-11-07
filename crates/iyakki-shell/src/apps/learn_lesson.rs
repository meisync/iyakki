//! LEARN: the lesson of one day of a plan. Opening a session starts the lesson at once, with no further click: the page shows
//! "What you will learn today" (made by the code from the session's title and notes, so the first frame is never empty) and the
//! teaching streams in under it, section by section, as the assistant writes it. A saved lesson reopens instantly and needs no model.
//! All the writing, drawing, cleaning and saving is `iyakki_agent::studylesson`; this file is the page, the keys and the background run.

use crate::safeclamp::SafeClamp;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::Arc;

use iyakki_agent::story::{self as st, Reader, Status, TtsStatus};
use iyakki_agent::studylesson::{self as sl, Lesson, Parsed};
use iyakki_agent::hostvoice::{self, HostVoice};
use iyakki_agent::hostengines;

use super::learn::*;
use super::AppCx;
use crate::ui::{Event, Key, TextField};

pub(super) const T_L_BACK: usize = 15;
pub(super) const T_L_DONE: usize = 16;
pub(super) const T_L_PREV: usize = 17;
pub(super) const T_L_NEXT: usize = 18;
pub(super) const T_L_ASK: usize = 19;
pub(super) const T_L_SEND: usize = 20;
pub(super) const T_L_SIMPLER: usize = 21;
pub(super) const T_L_DEEPER: usize = 22;
pub(super) const T_L_REGEN: usize = 23;
pub(super) const T_L_READ: usize = 24;
pub(super) const T_L_RETRY: usize = 25;
pub(super) const T_L_CANCEL: usize = 26;
pub(super) const T_L_REGEN_YES: usize = 27;
pub(super) const T_L_REGEN_NO: usize = 28;
pub(super) const T_L_ASK_CANCEL: usize = 29;
/// Tick the chosen session of the plan page / open its lesson (a click on the row itself opens the lesson).
pub(super) const T_TICK: usize = 30;
pub(super) const T_OPEN_LESSON: usize = 31;
/// A click on the heading of section `i` is `T_SEC + i`.
pub(super) const T_SEC: usize = 1000;
pub(super) const FIELD_ID: usize = 61_340;

pub(super) enum Msg {
    Text(Parsed),
    Pic(usize, Vec<u8>),
    Phase(String),
    Done(Result<Lesson, String>),
    Answer(String),
    AskDone(Result<String, String>),
}

pub(super) struct Gen {
    pub rx: Receiver<Msg>,
    pub stop: Arc<AtomicBool>,
    pub asking: bool,
}

pub(super) struct LessonState {
    pub plan: String,
    /// 1-based.
    pub n: usize,
    pub saved: Option<Lesson>,
    /// The text so far while the lesson is being written.
    pub live: Option<Parsed>,
    pub live_pics: Vec<Option<Arc<Vec<u8>>>>,
    /// Picture files of the saved lesson, by file name.
    pub pics: HashMap<String, Arc<Vec<u8>>>,
    pub phase: String,
    pub cur: usize,
    pub jump: bool,
    pub fail: Option<String>,
    pub confirm_regen: bool,
    pub input: Option<TextField>,
    pub asked: String,
    pub answer: Option<String>,
    pub gen: Option<Gen>,
}

impl LessonState {
    pub(super) fn generating(&self) -> bool {
        self.gen.is_some()
    }
    pub(super) fn writing(&self) -> bool {
        self.gen.as_ref().is_some_and(|g| !g.asking)
    }
    /// The sections that are on the page now: the stream while it is written, else the saved lesson.
    pub(super) fn sections(&self) -> &[sl::Section] {
        match (&self.live, &self.saved) {
            (Some(p), _) if self.writing() => &p.sections,
            (_, Some(l)) => &l.sections,
            _ => &[],
        }
    }
    pub(super) fn pictures(&self) -> &[sl::Picture] {
        match (&self.live, &self.saved) {
            (Some(p), _) if self.writing() => &p.pictures,
            (_, Some(l)) => &l.pictures,
            _ => &[],
        }
    }
    /// The bytes of picture `i` of what is on the page.
    pub(super) fn pic_bytes(&self, i: usize) -> Option<Arc<Vec<u8>>> {
        if self.writing() {
            return self.live_pics.get(i).cloned().flatten();
        }
        let f = &self.saved.as_ref()?.pictures.get(i)?.file;
        self.pics.get(f).cloned()
    }
}

fn fnv(parts: &[&[u8]]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for p in parts {
        for b in *p {
            h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
        }
        h = (h ^ 0xff).wrapping_mul(0x100000001b3);
    }
    h
}

/// The id a picture is cached under.
pub(super) fn pic_id(plan: &str, n: usize, i: usize, bytes: &[u8]) -> u64 {
    fnv(&[plan.as_bytes(), &n.to_le_bytes(), &i.to_le_bytes(), &bytes.len().to_le_bytes(), &bytes[..bytes.len().min(64)]])
}

impl LearnApp {
    pub(super) fn lesson_state(&self) -> Option<&LessonState> {
        if let View::Lesson(l) = &self.view { Some(l) } else { None }
    }
    pub(super) fn lesson_mut(&mut self) -> Option<&mut LessonState> {
        if let View::Lesson(l) = &mut self.view { Some(l) } else { None }
    }

    /// Open the lesson of session `n` (1-based) of a plan: a saved one is shown at once, otherwise the assistant starts writing it now.
    pub(super) fn open_lesson(&mut self, cx: &AppCx, plan: &str, n: usize) {
        let Some(p) = self.plan(plan) else { return };
        if n == 0 || n > p.sessions.len() {
            return;
        }
        self.stop_lesson();
        self.list.to_top();
        self.confirm_delete = false;
        self.sel_session = n - 1;
        self.pic_cache = Default::default();
        let mut l = LessonState { plan: plan.to_string(), n, saved: None, live: None, live_pics: vec![], pics: HashMap::new(), phase: String::new(), cur: 0, jump: false, fail: None, confirm_regen: false, input: None, asked: String::new(), answer: None, gen: None };
        load_saved(&mut l, cx, &self.user);
        let need = l.saved.is_none();
        self.view = View::Lesson(Box::new(l));
        if need {
            self.start_write(cx, 0);
        }
    }

    /// Stop whatever the lesson page is doing in the background: the writer and the voice.
    pub(super) fn stop_lesson(&mut self) {
        if let View::Lesson(l) = &mut self.view {
            if let Some(g) = &l.gen {
                g.stop.store(true, Ordering::SeqCst);
            }
            l.gen = None;
        }
        if let Some(r) = &self.reader {
            r.stop();
        }
    }

    /// Ask the assistant to write the lesson (again): streams into the page, then pictures, then it is saved.
    pub(super) fn start_write(&mut self, cx: &AppCx, level: i32) {
        let user = self.user.clone();
        let Some(l) = self.lesson_state() else { return };
        if l.generating() {
            return;
        }
        let (pid, n) = (l.plan.clone(), l.n);
        let Some(plan) = self.plan(&pid).cloned() else { return };
        cx.audit.record(&user, "learn.lesson", &format!("plan {pid} day {n} level {level}"));
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let (chat, svc, stop2) = (cx.chat.clone(), cx.services.clone(), stop.clone());
        std::thread::spawn(move || {
            let (t1, t2, t3) = (tx.clone(), tx.clone(), tx.clone());
            let mut on_text = move |p: &Parsed| {
                let _ = t1.send(Msg::Text(p.clone()));
            };
            let mut on_pic = move |i: usize, _ext: &str, b: &[u8]| {
                let _ = t2.send(Msg::Pic(i, b.to_vec()));
            };
            let mut on_phase = move |s: &str| {
                let _ = t3.send(Msg::Phase(s.to_string()));
            };
            let r = sl::generate(chat.as_ref(), &svc, &user, &plan, n, level, &mut sl::Hooks { on_text: &mut on_text, on_pic: &mut on_pic, on_phase: &mut on_phase, stop: &stop2 });
            let _ = tx.send(Msg::Done(r));
        });
        if let Some(l) = self.lesson_mut() {
            l.live = Some(Parsed::default());
            l.live_pics.clear();
            l.fail = None;
            l.confirm_regen = false;
            l.phase = "Waiting for the assistant".into();
            l.cur = 0;
            l.gen = Some(Gen { rx, stop, asking: false });
        }
        self.pic_cache = Default::default();
        self.dirty = true;
    }

    /// Send the question typed on the page to the assistant; the answer streams in under the lesson and is saved with it.
    fn start_ask(&mut self, cx: &AppCx) {
        let user = self.user.clone();
        let Some(l) = self.lesson_state() else { return };
        let Some(q) = l.input.as_ref().map(|f| f.value.trim().to_string()).filter(|q| !q.is_empty()) else { return };
        let (Some(lesson), false) = (l.saved.clone(), l.generating()) else { return };
        let Some(plan) = self.plan(&l.plan).cloned() else { return };
        cx.audit.record(&user, "learn.ask", &format!("plan {} day {} question {} chars", l.plan, l.n, q.chars().count()));
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let (chat, svc, stop2, q2) = (cx.chat.clone(), cx.services.clone(), stop.clone(), q.clone());
        std::thread::spawn(move || {
            let t = tx.clone();
            let r = sl::ask(chat.as_ref(), &svc, &user, &plan, &lesson, &q2, &stop2, &mut |a| {
                let _ = t.send(Msg::Answer(a.to_string()));
            });
            let _ = tx.send(Msg::AskDone(r));
        });
        if let Some(l) = self.lesson_mut() {
            l.input = None;
            l.asked = q;
            l.answer = Some(String::new());
            l.fail = None;
            l.phase = "Asking the assistant".into();
            l.gen = Some(Gen { rx, stop, asking: true });
        }
        self.dirty = true;
    }

    /// Take what the background run has sent so far (called every frame).
    pub(super) fn pump_lesson(&mut self, cx: &AppCx) {
        let user = self.user.clone();
        let Some(l) = self.lesson_mut() else { return };
        let Some(g) = &l.gen else { return };
        let mut finished: Option<Result<(), String>> = None;
        let mut asked_done: Option<Result<String, String>> = None;
        let mut done: Option<Result<Lesson, String>> = None;
        loop {
            match g.rx.try_recv() {
                Ok(Msg::Text(p)) => {
                    l.live = Some(p);
                    l.phase = "Writing the lesson".into();
                }
                Ok(Msg::Pic(i, b)) => {
                    if l.live_pics.len() <= i {
                        l.live_pics.resize(i + 1, None);
                    }
                    l.live_pics[i] = Some(Arc::new(b));
                }
                Ok(Msg::Phase(s)) => l.phase = s,
                Ok(Msg::Answer(a)) => l.answer = Some(a),
                Ok(Msg::Done(r)) => done = Some(r),
                Ok(Msg::AskDone(r)) => asked_done = Some(r),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    finished = Some(Err("The writer stopped without finishing.".into()));
                    break;
                }
            }
        }
        let asking = g.asking;
        if done.is_none() && asked_done.is_none() && finished.is_none() {
            self.dirty = true;
            return;
        }
        l.gen = None;
        l.live = None;
        l.answer = None;
        let err = match (done, asked_done, finished) {
            (Some(Ok(lesson)), _, _) => {
                l.saved = Some(lesson);
                l.cur = 0;
                None
            }
            (Some(Err(e)), _, _) => Some(e),
            (_, Some(Ok(_)), _) => None,
            (_, Some(Err(e)), _) => Some(e),
            (_, _, Some(Err(e))) => Some(e),
            _ => None,
        };
        // what is on disk is the truth: the lesson, its pictures and its questions
        load_saved(l, cx, &user);
        l.fail = err.map(|e| if asking { format!("The question could not be answered: {e}") } else { e });
        l.live_pics.clear();
        l.phase.clear();
        self.pic_cache = Default::default();
        self.dirty = true;
    }

    pub(super) fn cancel_run(&mut self) {
        if let Some(l) = self.lesson_mut() {
            if let Some(g) = &l.gen {
                g.stop.store(true, Ordering::SeqCst);
            }
        }
    }

    fn lesson_done_toggle(&mut self, cx: &AppCx) {
        let Some(l) = self.lesson_state() else { return };
        let (pid, n) = (l.plan.clone(), l.n);
        let Some(done) = self.plan(&pid).and_then(|p| p.sessions.get(n - 1)).map(|s| s.done) else { return };
        match cx.services.study.set_done(&self.user, &pid, n, !done) {
            Ok(_) => self.say(if done { "Day marked as not done." } else { "Day marked done." }, true),
            Err(e) => self.say(&e, false),
        }
        self.reload(cx);
    }

    /// Move to another day of the same plan from the lesson (`d` = +1 / -1) is not offered: sections move, days are chosen in the plan.
    fn section_step(&mut self, d: i32) {
        let Some(l) = self.lesson_mut() else { return };
        let n = l.sections().len();
        if n == 0 {
            return;
        }
        l.cur = (l.cur as i32 + d).clamp_safe(0, n as i32 - 1) as usize;
        l.jump = true;
    }

    fn regenerate(&mut self, cx: &AppCx, level: Option<i32>) {
        let Some(l) = self.lesson_state() else { return };
        if l.generating() {
            return;
        }
        let lv = level.unwrap_or_else(|| l.saved.as_ref().map_or(0, |s| s.level));
        self.start_write(cx, lv);
    }

    /// The keys of the lesson page.
    pub(super) fn lesson_key(&mut self, k: &Key, cx: &AppCx) {
        let Some(l) = self.lesson_state() else { return };
        // typing a question
        if l.input.is_some() {
            match k {
                Key::Enter => self.start_ask(cx),
                other => {
                    if let Some(f) = self.lesson_mut().and_then(|l| l.input.as_mut()) {
                        f.handle(other);
                    }
                }
            }
            return;
        }
        let (confirm, generating, failed, has) = (l.confirm_regen, l.generating(), l.fail.is_some(), l.saved.is_some());
        if confirm {
            match k {
                Key::Char('y') | Key::Enter => self.regenerate(cx, None),
                Key::Char('n') => {
                    if let Some(l) = self.lesson_mut() {
                        l.confirm_regen = false;
                    }
                }
                _ => {}
            }
            return;
        }
        match k {
            Key::Left => self.section_step(-1),
            Key::Right => self.section_step(1),
            Key::Char(' ') => self.lesson_done_toggle(cx),
            Key::Enter if failed && !generating => self.regenerate(cx, None),
            Key::Enter => self.section_step(1),
            Key::Char('c') if generating => self.cancel_run(),
            Key::Char('q') | Key::Char('?') if has && !generating => self.begin_ask(),
            Key::Char('s') if !generating => self.regenerate(cx, Some(-1)),
            Key::Char('d') if !generating => self.regenerate(cx, Some(1)),
            Key::Char('g') if has && !generating => self.ask_regen(),
            Key::Char('r') => self.read_aloud(cx),
            other => {
                self.list.key(other);
            }
        }
    }

    pub(super) fn begin_ask(&mut self) {
        if let Some(l) = self.lesson_mut() {
            if l.saved.is_some() && !l.generating() {
                let mut f = TextField::new("Ask a question about this lesson", false);
                f.max = sl::MAX_QUESTION;
                l.input = Some(f);
                l.confirm_regen = false;
            }
        }
    }

    pub(super) fn ask_regen(&mut self) {
        if let Some(l) = self.lesson_mut() {
            if l.saved.is_some() && !l.generating() {
                l.confirm_regen = true;
            }
        }
    }

    /// The tile events of the lesson page. True when it was one of them.
    pub(super) fn lesson_event(&mut self, ev: &Event, cx: &AppCx) -> bool {
        match ev {
            Event::Tab(T_L_BACK) => {
                self.leave_lesson();
            }
            Event::Tab(T_L_DONE) => self.lesson_done_toggle(cx),
            Event::Tab(T_L_PREV) => self.section_step(-1),
            Event::Tab(T_L_NEXT) => self.section_step(1),
            Event::Tab(T_L_ASK) => self.begin_ask(),
            Event::Tab(T_L_SEND) => self.start_ask(cx),
            Event::Tab(T_L_ASK_CANCEL) => {
                if let Some(l) = self.lesson_mut() {
                    l.input = None;
                }
            }
            Event::Tab(T_L_SIMPLER) => self.regenerate(cx, Some(-1)),
            Event::Tab(T_L_DEEPER) => self.regenerate(cx, Some(1)),
            Event::Tab(T_L_REGEN) => self.ask_regen(),
            Event::Tab(T_L_REGEN_YES) => {
                if self.lesson_state().is_some_and(|l| l.confirm_regen) {
                    self.regenerate(cx, None);
                }
            }
            Event::Tab(T_L_REGEN_NO) => {
                if let Some(l) = self.lesson_mut() {
                    l.confirm_regen = false;
                }
            }
            Event::Tab(T_L_READ) => self.read_aloud(cx),
            Event::Tab(T_L_RETRY) => self.regenerate(cx, None),
            Event::Tab(T_L_CANCEL) => self.cancel_run(),
            Event::Tab(n) if *n >= T_SEC && *n < T_SEC + 100 => {
                if let Some(l) = self.lesson_mut() {
                    let i = n - T_SEC;
                    if i < l.sections().len() {
                        l.cur = i;
                    }
                }
            }
            Event::Paste(t) => {
                if let Some(f) = self.lesson_mut().and_then(|l| l.input.as_mut()) {
                    f.paste(t);
                }
            }
            _ => return false,
        }
        true
    }

    /// Back to the plan the lesson belongs to (the run is stopped).
    pub(super) fn leave_lesson(&mut self) {
        let Some(l) = self.lesson_state() else { return };
        let (pid, n) = (l.plan.clone(), l.n);
        self.stop_lesson();
        self.view = View::Plan(pid);
        self.sel_session = n - 1;
        self.list.to_top();
    }

    /// Read the lesson aloud, or stop reading. Uses the same reader, engines and speaker as TALES; says plainly when no voice is set up.
    pub(super) fn read_aloud(&mut self, cx: &AppCx) {
        if self.reader.as_ref().is_some_and(|r| matches!(r.status(), Status::Playing | Status::Buffering | Status::Paused)) {
            if let Some(r) = &self.reader {
                r.stop();
            }
            return;
        }
        let Some(text) = self.lesson_state().and_then(|l| l.saved.as_ref().map(|s| s.plain())) else {
            self.say("There is no lesson to read yet.", false);
            return;
        };
        let chunks = st::sentence_chunks(&text, st::MAX_CHUNK_CHARS);
        if chunks.is_empty() {
            return;
        }
        let (synth, sink): (Arc<dyn st::Synth>, Arc<dyn st::Sink>) = match self.voice(cx) {
            Ok(x) => x,
            Err(why) => {
                self.say(&why, false);
                return;
            }
        };
        cx.audit.record(&self.user, "learn.read", &format!("{} parts", chunks.len()));
        let r = self.reader.get_or_insert_with(Reader::new);
        r.start(chunks, 0, false, "learn", synth, sink);
        self.say("Reading the lesson aloud. Press r or the button to stop.", true);
    }

    /// The speech engine and the speaker, chosen as TALES chooses them.
    fn voice(&self, cx: &AppCx) -> Result<(Arc<dyn st::Synth>, Arc<dyn st::Sink>), String> {
        #[cfg(test)]
        if let Some(v) = &self.test_voice {
            return Ok(v.clone());
        }
        let v = cx.services.voice_settings();
        let host = HostVoice::current();
        cx.services.host_tts.ensure();
        let auto = if v.tts.is_none() { cx.services.host_tts.auto_engine(Some("en")) } else { None };
        let path = std::env::var("PATH").unwrap_or_default();
        match st::tts_status_auto(&v, cx.session.policy.voice, &path, &host, auto.as_ref()) {
            TtsStatus::Ready { .. } => {}
            other => return Err(st::status_text(&other).replace("The story is", "The lesson is").replace("the story is", "the lesson is").replace("story", "lesson")),
        }
        let engine = st::effective_engine_auto(&v, &host, auto.as_ref());
        let opts = iyakki_agent::voice::SpeakOpts { voice: None, speed: 1.0, lang: Some("en".into()) };
        let browser = engine == Some(iyakki_agent::voice::SpeechEngine::Browser);
        let direct = engine.as_ref().filter(|e| hostengines::speaks_directly(e)).and_then(hostengines::Spec::from_setting);
        Ok(if browser {
            host.want_probe();
            (Arc::new(hostvoice::BrowserSynth), Arc::new(hostvoice::BrowserSink { hv: HostVoice::current(), voice: None, lang: Some("en".into()), rate: 1.0 }))
        } else if let Some(spec) = direct {
            (Arc::new(hostengines::DirectSynth), Arc::new(hostengines::DirectSink { spec, lang: Some("en".into()), speed: 1.0 }))
        } else {
            let play = hostengines::player_command(&v.play_cmd, &path).unwrap_or(v.play_cmd.clone());
            (Arc::new(st::ServicesSynth { svc: cx.services.clone(), opts }), Arc::new(st::CommandSink { play_cmd: play }))
        })
    }

    pub(super) fn reading(&self) -> bool {
        self.reader.as_ref().is_some_and(|r| matches!(r.status(), Status::Playing | Status::Buffering | Status::Paused))
    }
}

/// Read the saved lesson and its picture files from disk into the state.
fn load_saved(l: &mut LessonState, cx: &AppCx, user: &str) {
    l.saved = cx.services.study.lesson(user, &l.plan, l.n);
    l.pics.clear();
    if let Some(s) = &l.saved {
        for p in &s.pictures {
            if let Some(b) = cx.services.study.lesson_image(user, &l.plan, l.n, &p.file) {
                l.pics.insert(p.file.clone(), Arc::new(b));
            }
        }
    }
}
