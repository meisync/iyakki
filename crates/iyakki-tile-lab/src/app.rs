//! LAB: a model lab for pictures. Three tabs: PICTURE (make a picture with a model, or the same prompt with two models one after
//! the other), VISION (ask a vision model about a picture from a file, the camera, the screen or the clipboard, alone or with two
//! models) and VIDEO (sample a video into frames and ask a vision model about them; video generation is offered only when a
//! provider lists a video model). A LOG at the bottom of every tab records each call.
//!
//! The logic (model lists, caps, frame maths, the log, the backends) is `iyakki_shared::lab`; this file is the state, the keys and
//! the background jobs, `lab_ui.rs` draws it, `lab_pic.rs` prepares pictures. Long work runs on a thread and can be cancelled
//! (a call already sent is not recalled: its answer is dropped). Nothing is written to disk except by Save picture. Nothing is
//! sent to a provider on another computer until the person said yes to that provider in this session.

use iyakki_uikit::safeclamp::SafeClamp;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Instant;

use iyakki_lab_model::{PictureRequest, Sampling, Sees, VisionAnswer, VisionRequest};
use iyakki_lab_model::{Catalogue, LabModel};
use iyakki_lab_model::{self as lab, FrameSource, LogEntry, PictureBackend, VisionBackend};
use iyakki_lab_model::ImageTrace;
use tiny_skia::Pixmap;

use crate::pic::{self as lab_pic, PictureCodec, Prepared};
use iyakki_uikit::multiline::MultiLine;
use iyakki_uikit::widgets::{PickEvent, Picker};
use crate::port::LabPort;
use iyakki_uikit::scrolllog::ScrollLog;
use iyakki_uikit::filepicker::{FilePicker, PathKind, PickOut};
use iyakki_uikit::gfx::Icon;
use iyakki_uikit::ui::{Event, Key, TextField};

pub const ACCENT: [u8; 4] = [190, 140, 255, 255];

// the click codes (Event::Tab)
pub const T_TAB_PICTURE: usize = 1;
pub const T_TAB_VISION: usize = 2;
pub const T_TAB_VIDEO: usize = 3;
pub const T_MODEL: usize = 10;
pub const T_MODEL_B: usize = 11;
pub const T_GENERATE: usize = 12;
pub const T_CANCEL: usize = 13;
pub const T_COMPARE: usize = 14;
pub const T_SAVE: usize = 15;
pub const T_COPY_PROMPT: usize = 16;
pub const T_REFRESH: usize = 17;
pub const T_SEED_REUSE: usize = 18;
pub const T_SEED_NEW: usize = 19;
pub const T_SRC_FILE: usize = 30;
pub const T_SRC_CAMERA: usize = 31;
pub const T_SRC_SCREEN: usize = 32;
pub const T_SRC_CLIP: usize = 33;
pub const T_BROWSE: usize = 34;
pub const T_ASK: usize = 35;
pub const T_ANY_CHAT: usize = 36;
pub const T_COPY_ANSWER: usize = 37;
pub const T_FRAMES_DN: usize = 40;
pub const T_FRAMES_UP: usize = 41;
pub const T_MODE: usize = 42;
pub const T_VIDEO_BROWSE: usize = 43;
pub const T_MEDIA_SETUP: usize = 44;
pub const T_LOG_COPY: usize = 50;
pub const T_LOG_CLEAR: usize = 51;
pub const T_YES: usize = 60;
pub const T_NO: usize = 61;
pub const T_STYLE0: usize = 70;
pub const T_QUICK0: usize = 90;
pub const T_HIST0: usize = 200;

// the areas and fields that take presses
pub const PROMPT_AREA: usize = 63_001;
pub const NEG_AREA: usize = 63_002;
pub const QUESTION_AREA: usize = 63_003;
pub const ANS_AREA_A: usize = 63_011;
pub const ANS_AREA_B: usize = 63_012;
pub const F_SEED: usize = 63_021;
pub const F_W: usize = 63_022;
pub const F_H: usize = 63_023;
pub const F_PATH: usize = 63_024;
pub const F_VPATH: usize = 63_025;
pub const F_EVERY: usize = 63_026;
pub const F_STEPS: usize = 63_027;

/// The looks of TALES, as short style phrases the Picture tab can add to a prompt (the same wording as the TALES looks; copied so
/// this tile does not depend on that code).
pub const STYLES: [(&str, &str); 5] = [
    ("watercolor", "a soft watercolor painting"),
    ("photo", "a realistic photo, 50mm lens, natural light"),
    ("cinematic", "a cinematic film still, 35mm lens, soft light"),
    ("anime", "an anime illustration, clean line art, cel shading"),
    ("pencil", "a detailed pencil sketch, graphite shading"),
];

/// Questions to start from on the Vision tab.
pub const QUICK: [(&str, &str); 4] = [
    ("Describe this picture", "Describe this picture."),
    ("Read the text", "Read all the text in this picture, exactly as written."),
    ("What is wrong with this screenshot?", "What is wrong with this screenshot? Look for errors, broken layout, missing text or anything odd."),
    ("List the objects", "List the objects you can see in this picture."),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Picture,
    Vision,
    Video,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    None,
    Prompt,
    Negative,
    Seed,
    Width,
    Height,
    Steps,
    Question,
    Path,
    VPath,
    Every,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    PicA,
    PicB,
    Vis,
    VisB,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Src {
    Camera,
    Screen,
}

/// What a yes on the card goes on to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Then {
    Picture { compare: bool },
    Vision { compare: bool },
    Video,
}

pub enum Modal {
    /// Pictures or questions would go to a provider on another computer the person has not agreed to yet in this session.
    Consent { providers: Vec<(String, &'static str)>, then: Then },
    /// The camera or the screen is private: asked before every use.
    Source(Src),
    Pick { slot: Slot, picker: Picker, items: Vec<(String, String)> },
    File { video: bool, picker: Box<FilePicker> },
}

/// A decoded picture the model made.
pub struct Item {
    pub id: u64,
    pub provider: String,
    pub model: String,
    pub remote: bool,
    pub prompt: String,
    pub negative: String,
    pub seed: u64,
    pub asked: (u32, u32),
    pub size: (u32, u32),
    pub steps: Option<u32>,
    pub ms: u64,
    pub bytes: Vec<u8>,
    pub ext: &'static str,
    /// What is drawn (at most 1024 on a side) and a small copy for the list.
    pub view: Arc<Pixmap>,
    pub thumb: Arc<Pixmap>,
}

impl Item {
    pub fn weight(&self) -> usize {
        self.bytes.len() + self.view.data().len() + self.thumb.data().len()
    }
}

/// What the result pane shows for one run of a picture model.
pub enum Shown {
    Item(u64),
    Failed { label: String, error: String },
}

/// One model's answer to a picture question.
pub struct Answer {
    pub label: String,
    pub place: String,
    pub text: String,
    pub error: Option<String>,
    pub ms: u64,
    pub tokens: Option<(u64, u64)>,
    pub done: bool,
    pub streamed: bool,
    pub note: String,
    pub view: ScrollLog,
}

impl Answer {
    pub fn new(m: &LabModel, id: usize) -> Answer {
        let view = ScrollLog { pane: iyakki_uikit::textsel::Pane::new(id), ..ScrollLog::default() };
        Answer { label: format!("{} on {}", m.model, m.provider), place: m.runs_on(), text: String::new(), error: None, ms: 0, tokens: None, done: false, streamed: false, note: if m.sees == Sees::Unknown { "Not marked as a vision model: it may not see the picture.".into() } else { String::new() }, view }
    }

    pub fn set_text(&mut self, t: &str) {
        self.text = t.to_string();
        let keep = self.view.scroll;
        self.view.lines = t.split('\n').map(|l| (l.to_string(), [0, 0, 0, 0])).collect();
        self.view.scroll = keep;
    }
}

/// A frame taken from a video, for the strip.
pub struct FrameView {
    pub time: f64,
    pub thumb: Arc<Pixmap>,
}

pub enum Msg {
    Progress(String),
    Picture { req: Box<PictureRequest>, remote: bool, res: Result<(Vec<u8>, &'static str, Pixmap), String>, trace: Box<ImageTrace> },
    Text { slot: usize, piece: String },
    Answer { slot: usize, res: Result<VisionAnswer, String>, ms: u64 },
    Frames(Vec<FrameView>),
    Source(Result<Prepared, String>),
    Refreshed,
    Log(Box<LogEntry>),
    Failed(String),
    Done,
}

struct Job {
    rx: Receiver<Msg>,
    cancel: Arc<AtomicBool>,
    what: &'static str,
    started: Instant,
    progress: String,
}

/// What a screen of the tile answers to an event: stay, or go back to the launcher.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Stay,
    Back,
}

/// The things the tile uses that tests replace.
#[derive(Clone)]
pub struct Rig {
    /// How a picture file is read (the shell's decoders).
    pub codec: Arc<dyn PictureCodec>,
    pub picture: Arc<dyn PictureBackend>,
    pub vision: Arc<dyn VisionBackend>,
    pub frames: Arc<dyn FrameSource>,
    /// One camera frame as JPEG bytes, in memory only.
    pub camera: Arc<dyn Fn() -> Result<Vec<u8>, String> + Send + Sync>,
    pub has_camera: Arc<dyn Fn() -> bool + Send + Sync>,
    /// The screen as (width, height, RGBA).
    pub screen: Arc<dyn Fn() -> Result<(u32, u32, Vec<u8>), String> + Send + Sync>,
    /// The person's home folder (Save puts pictures in `Pictures` under it).
    pub home: PathBuf,
    /// Where files may be opened from, when it is not the account's own home (tests); `None`: the account's usual folders.
    pub roots_home: Option<PathBuf>,
}

pub struct LabApp {
    pub tab: Tab,
    pub rig: Rig,
    pub cat: Catalogue,
    pub focus: Focus,
    pub modal: Option<Modal>,
    pub status: Option<(String, bool)>,
    // picture
    pub prompt: MultiLine,
    pub negative: MultiLine,
    pub seed: TextField,
    pub width: TextField,
    pub height: TextField,
    pub steps: TextField,
    pub pic_a: Option<(String, String)>,
    pub pic_b: Option<(String, String)>,
    pub history: Vec<Item>,
    pub shown: Vec<Shown>,
    next_id: u64,
    // vision
    pub question: MultiLine,
    pub path: TextField,
    pub any_chat: bool,
    pub vis_a: Option<(String, String)>,
    pub vis_b: Option<(String, String)>,
    pub input: Option<Prepared>,
    pub answers: Vec<Answer>,
    // video
    pub vpath: TextField,
    pub nframes: usize,
    pub even: bool,
    pub every: TextField,
    pub frames: Vec<FrameView>,
    pub ffmpeg: Result<(), String>,
    // jobs, the log and the session's approvals
    job: Option<Job>,
    pub log: lab::Log,
    pub logv: ScrollLog,
    approved: HashSet<String>,
    pub has_camera: bool,
    request: Option<(String, String)>,
    pub ticks: u64,
    dirty: bool,
    /// How far the controls are scrolled (pixels) and how much more there is.
    pub col_scroll: f32,
    pub col_max: f32,
    /// Where the pointer was at the last drawing or move (the wheel scrolls what is under it).
    pub mouse: (i32, i32),
}

fn new_box(max_rows: usize) -> MultiLine {
    let mut m = MultiLine::new(lab::MAX_TEXT);
    m.min_rows = 2;
    m.max_rows = max_rows;
    m
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

impl LabApp {
    /// The tile over any port, with the rig the port gives.
    pub fn with_port(port: &dyn LabPort) -> LabApp {
        LabApp::with_port_rig(port, port.rig())
    }

    pub fn with_port_rig(port: &dyn LabPort, rig: Rig) -> LabApp {
        let mut path = TextField::new("path of a picture", false);
        path.max = 1024;
        let mut vpath = TextField::new("path of a video", false);
        vpath.max = 1024;
        let mut seed = TextField::new("random", false);
        seed.max = 12;
        let mut width = TextField::new("width", false);
        width.max = 5;
        let mut height = TextField::new("height", false);
        height.max = 5;
        let mut steps = TextField::new("model's own", false);
        steps.max = 3;
        let mut every = TextField::new("seconds", false);
        every.max = 5;
        every.set("5");
        let mut a = LabApp {
            tab: Tab::Picture,
            rig,
            cat: Catalogue::default(),
            focus: Focus::Prompt,
            modal: None,
            status: None,
            prompt: new_box(6),
            negative: new_box(3),
            seed,
            width,
            height,
            steps,
            pic_a: None,
            pic_b: None,
            history: Vec::new(),
            shown: Vec::new(),
            next_id: 1,
            question: new_box(5),
            path,
            any_chat: false,
            vis_a: None,
            vis_b: None,
            input: None,
            answers: Vec::new(),
            vpath,
            nframes: 8,
            even: true,
            every,
            frames: Vec::new(),
            ffmpeg: Ok(()),
            job: None,
            log: lab::Log::default(),
            logv: ScrollLog { nowrap: false, ..ScrollLog::default() },
            approved: HashSet::new(),
            has_camera: false,
            request: None,
            ticks: 0,
            dirty: true,
            col_scroll: 0.0,
            col_max: 0.0,
            mouse: (0, 0),
        };
        a.reload(port);
        a
    }

    // ------------------------------------------------------------------------------------------------ what exists

    /// Read the model lists again (from the settings; no network) and keep the choices that still exist.
    pub fn reload(&mut self, port: &dyn LabPort) {
        self.cat = port.catalogue();
        let keep = |cur: &Option<(String, String)>, list: &[LabModel]| cur.clone().filter(|c| list.iter().any(|m| (m.provider.as_str(), m.model.as_str()) == (c.0.as_str(), c.1.as_str()))).or_else(|| list.first().map(|m| (m.provider.clone(), m.model.clone())));
        let before = self.pic_a.clone();
        self.pic_a = keep(&self.pic_a, &self.cat.pictures);
        self.pic_b = self.pic_b.clone().filter(|c| self.cat.pictures.iter().any(|m| (m.provider.as_str(), m.model.as_str()) == (c.0.as_str(), c.1.as_str()))).or_else(|| self.cat.pictures.iter().find(|m| Some((&m.provider, &m.model)) != self.pic_a.as_ref().map(|a| (&a.0, &a.1))).map(|m| (m.provider.clone(), m.model.clone())));
        let vis = self.cat.vision_choices(self.any_chat);
        self.vis_a = keep(&self.vis_a, &vis);
        self.vis_b = self.vis_b.clone().filter(|c| vis.iter().any(|m| (m.provider.as_str(), m.model.as_str()) == (c.0.as_str(), c.1.as_str()))).or_else(|| vis.iter().find(|m| Some((&m.provider, &m.model)) != self.vis_a.as_ref().map(|a| (&a.0, &a.1))).map(|m| (m.provider.clone(), m.model.clone())));
        if before != self.pic_a || self.width.value.is_empty() {
            self.fill_defaults();
        }
        self.ffmpeg = self.rig.frames.available();
        self.has_camera = (self.rig.has_camera)();
        self.dirty = true;
    }

    pub fn pic_model(&self, which: Slot) -> Option<&LabModel> {
        let k = if which == Slot::PicB { &self.pic_b } else { &self.pic_a };
        k.as_ref().and_then(|(p, m)| self.cat.find_picture(p, m))
    }

    pub fn vis_model(&self, which: Slot) -> Option<LabModel> {
        let k = if which == Slot::VisB { &self.vis_b } else { &self.vis_a };
        let (p, m) = k.as_ref()?;
        self.cat.vision_choices(true).into_iter().find(|x| (&x.provider, &x.model) == (p, m))
    }

    /// Size from the model's own list (and its steps, which the tile only shows: the server decides them).
    pub fn fill_defaults(&mut self) {
        let (w, h) = match self.pic_model(Slot::PicA).and_then(|m| m.defaults) {
            Some(d) => (d.width.unwrap_or(1024), d.height.unwrap_or(1024)),
            None => (1024, 1024),
        };
        self.width.set(&w.to_string());
        self.height.set(&h.to_string());
        let steps = self.pic_model(Slot::PicA).and_then(|m| m.defaults.and_then(|d| d.steps).filter(|_| m.takes_steps()));
        self.steps.set(&steps.map_or(String::new(), |s| s.to_string()));
    }

    /// The line about steps for the model chosen: its own (from its list) in the box, changeable when the server takes them.
    pub fn steps_text(&self) -> String {
        match self.pic_model(Slot::PicA) {
            Some(m) if m.takes_steps() => match m.defaults.and_then(|d| d.steps) {
                Some(s) => format!("Steps start at the model's own {s}; more steps take longer (1 to {})", lab::MAX_STEPS),
                None => "Steps: empty uses the server's own (its list does not say how many)".into(),
            },
            Some(_) => "Steps: this server chooses its own".into(),
            None => String::new(),
        }
    }

    pub fn tab(&self) -> Tab {
        self.tab
    }

    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    pub fn status_text(&self) -> Option<(&str, bool)> {
        self.status.as_ref().map(|(t, ok)| (t.as_str(), *ok))
    }

    pub fn log_text(&self) -> String {
        self.log.text()
    }

    pub fn progress_text(&self) -> Option<String> {
        self.job.as_ref().map(|j| format!("{} {}s", if j.progress.is_empty() { j.what.to_string() } else { j.progress.clone() }, j.started.elapsed().as_secs()))
    }

    pub fn say(&mut self, t: impl Into<String>, ok: bool) {
        self.status = Some((t.into(), ok));
        self.dirty = true;
    }

    pub fn push_log(&mut self, e: LogEntry) {
        self.log.push(e);
        self.sync_log();
    }

    pub fn sync_log(&mut self) {
        let z = iyakki_clock::Zone::system();
        self.logv.lines = self.log.entries().map(|e| (e.line(&z), if e.error.is_empty() { [0, 0, 0, 0] } else { iyakki_uikit::ui::ERR })).collect();
        self.logv.scroll = 0;
    }

    pub fn now(&self) -> u64 {
        iyakki_clock::now_unix().max(0) as u64
    }

    // ------------------------------------------------------------------------------------------------ the approval

    /// The providers of these models that are on another computer and not yet approved in this session.
    pub fn unapproved(&self, models: &[&LabModel]) -> Vec<(String, &'static str)> {
        let mut v: Vec<(String, &'static str)> = Vec::new();
        for m in models.iter().filter(|m| m.remote && !self.approved.contains(&m.provider)) {
            if !v.iter().any(|(p, _)| *p == m.provider) {
                v.push((m.provider.clone(), m.place));
            }
        }
        v
    }

    pub fn approved_for(&self, provider: &str) -> bool {
        self.approved.contains(provider)
    }

    pub fn ask_consent(&mut self, models: &[&LabModel], then: Then) -> bool {
        let need = self.unapproved(models);
        if need.is_empty() {
            return false;
        }
        self.modal = Some(Modal::Consent { providers: need, then });
        self.dirty = true;
        true
    }

    /// The person approved: remember the providers for this session (memory only) and carry on.
    pub fn consent_yes(&mut self, port: &dyn LabPort) {
        let Some(Modal::Consent { providers, then }) = self.modal.take() else { return };
        for (p, _) in &providers {
            port.audit("lab.remote_ok", p);
            self.approved.insert(p.clone());
        }
        match then {
            Then::Picture { compare } => self.run_picture(port, compare),
            Then::Vision { compare } => self.run_vision(port, compare),
            Then::Video => self.run_video(port),
        }
    }

    // ------------------------------------------------------------------------------------------------ jobs

    pub fn start(&mut self, what: &'static str, f: impl FnOnce(Sender<Msg>, Arc<AtomicBool>) + Send + 'static) {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let c2 = cancel.clone();
        let tx2 = tx.clone();
        let spawned = std::thread::Builder::new().name("iyakki-lab".into()).spawn(move || {
            f(tx, c2);
        });
        if spawned.is_err() {
            let _ = tx2.send(Msg::Failed("Could not start the work (out of threads).".into()));
        }
        self.job = Some(Job { rx, cancel, what, started: Instant::now(), progress: String::new() });
        self.status = None;
        self.dirty = true;
    }

    /// Stop waiting for the running job. A call already sent is not recalled; its answer is dropped.
    pub fn cancel(&mut self) {
        if let Some(j) = self.job.take() {
            j.cancel.store(true, Ordering::SeqCst);
            for a in self.answers.iter_mut().filter(|a| !a.done) {
                a.done = true;
                a.error = Some("Cancelled.".into());
            }
            let now = self.now();
            let tab = match self.tab {
                Tab::Picture => "picture",
                Tab::Vision => "vision",
                Tab::Video => "video",
            };
            self.push_log(LogEntry { ts: now, tab: tab.into(), provider: "-".into(), model: j.what.into(), request: "stopped by the person".into(), notes: vec![format!("cancelled after {}s; a request already sent is not recalled", j.started.elapsed().as_secs())], ..Default::default() });
            self.say("Cancelled. A request already sent may still finish on the server; its answer is dropped.", true);
        }
    }

    pub fn on_tick(&mut self, port: &dyn LabPort) {
        self.ticks += 1;
        let mut msgs = Vec::new();
        let mut gone = false;
        if let Some(j) = &self.job {
            loop {
                match j.rx.try_recv() {
                    Ok(m) => msgs.push(m),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        gone = true;
                        break;
                    }
                }
            }
        }
        for m in msgs {
            self.apply(m, port);
        }
        if gone && self.job.is_some() {
            self.job = None;
        }
        if self.ticks % 150 == 0 && self.job.is_none() && self.modal.is_none() {
            self.reload(port);
        }
        self.logv.on_tick();
        if self.job.is_some() {
            self.dirty = true;
        }
    }

    pub fn apply(&mut self, m: Msg, port: &dyn LabPort) {
        match m {
            Msg::Progress(t) => {
                if let Some(j) = self.job.as_mut() {
                    j.progress = t;
                }
            }
            Msg::Log(e) => self.push_log(*e),
            Msg::Picture { req, remote, res, trace } => self.picture_done(*req, remote, res, *trace),
            Msg::Text { slot, piece } => {
                if let Some(a) = self.answers.get_mut(slot) {
                    let t = format!("{}{}", a.text, piece);
                    a.set_text(&t);
                    a.streamed = true;
                }
            }
            Msg::Answer { slot, res, ms } => {
                if let Some(a) = self.answers.get_mut(slot) {
                    a.done = true;
                    a.ms = ms;
                    match res {
                        Ok(r) => {
                            a.tokens = r.tokens;
                            a.streamed = r.streamed;
                            let t = r.text.clone();
                            a.set_text(&t);
                        }
                        Err(e) => a.error = Some(e),
                    }
                }
            }
            Msg::Frames(f) => self.frames = f,
            Msg::Source(r) => {
                match r {
                    Ok(p) => {
                        let line = p.sent_line();
                        let src = p.source.clone();
                        self.input = Some(p);
                        self.answers.clear();
                        self.say(format!("Picture ready from {src}. {line}."), true);
                    }
                    Err(e) => self.say(e, false),
                }
                self.job = None;
            }
            Msg::Refreshed => {
                self.reload(port);
                self.job = None;
                self.say("Model lists read again from the servers.", true);
            }
            Msg::Failed(e) => {
                self.say(e, false);
                self.job = None;
            }
            Msg::Done => {
                self.job = None;
                if self.status.is_none() {
                    self.dirty = true;
                }
            }
        }
        self.dirty = true;
    }

    // ------------------------------------------------------------------------------------------------ picture

    pub fn run_picture(&mut self, port: &dyn LabPort, compare: bool) {
        if self.job.is_some() {
            return;
        }
        let prompt = lab::clean_text(&self.prompt.text);
        if prompt.is_empty() {
            self.say("Type what the picture should show first.", false);
            self.focus = Focus::Prompt;
            return;
        }
        let Some(a) = self.pic_model(Slot::PicA).cloned() else {
            self.say("No picture model is available. Models are installed in Settings \u{2192} Models (Lemonade: SD-Turbo, SDXL; or a provider that draws).", false);
            return;
        };
        let mut models = vec![a];
        if compare {
            match self.pic_model(Slot::PicB).cloned() {
                Some(b) if (&b.provider, &b.model) != (&models[0].provider, &models[0].model) => models.push(b),
                Some(_) => {
                    self.say("Pick two different models to compare.", false);
                    return;
                }
                None => {
                    self.say("Compare needs a second picture model; none other is available.", false);
                    return;
                }
            }
        }
        let refs: Vec<&LabModel> = models.iter().collect();
        if self.ask_consent(&refs, Then::Picture { compare }) {
            return;
        }
        let (w, h) = (lab::bound_side(&self.width.value, 1024), lab::bound_side(&self.height.value, 1024));
        self.width.set(&w.to_string());
        self.height.set(&h.to_string());
        let seed = lab::parse_seed(&self.seed.value).unwrap_or_else(|| lab::fresh_seed(now_ms()));
        let neg = lab::clean_text(&self.negative.text);
        let steps = lab::bound_steps(&self.steps.value);
        if let Some(s) = steps {
            self.steps.set(&s.to_string());
        }
        let reqs: Vec<(PictureRequest, bool)> = models.iter().map(|m| (PictureRequest { provider: m.provider.clone(), model: m.model.clone(), prompt: prompt.clone(), negative: (m.takes_negative() && !neg.is_empty()).then(|| neg.clone()), width: w, height: h, steps: steps.filter(|_| m.takes_steps()), seed }, m.remote)).collect();
        port.audit("lab.picture", &models.iter().map(|m| format!("{} on {}", m.model, m.provider)).collect::<Vec<_>>().join(" then "));
        self.shown.clear();
        let backend = self.rig.picture.clone();
        let codec = self.rig.codec.clone();
        let now = self.now();
        let total = reqs.len();
        self.start("Drawing", move |tx, cancel| {
            for (n, (req, remote)) in reqs.into_iter().enumerate() {
                if cancel.load(Ordering::SeqCst) {
                    break;
                }
                let _ = tx.send(Msg::Progress(format!("Drawing {} of {} with {} on {}, one after the other\u{2026}", n + 1, total, req.model, req.provider)));
                let (res, trace) = backend.draw(&req);
                let entry = lab::picture_entry(&req, &trace, &res, now);
                let _ = tx.send(Msg::Log(Box::new(entry)));
                // decoded here, off the screen's thread; a picture too big or not a picture is refused with the reason
                let decoded = res.and_then(|(b, ext)| {
                    if b.len() > lab::MAX_RESULT_BYTES {
                        return Err(format!("The picture is too large to keep ({} MB; the limit is {} MB).", b.len() >> 20, lab::MAX_RESULT_BYTES >> 20));
                    }
                    let pm = codec.decode(&b).map_err(|e| format!("The server answered, but not with a picture IYAKKI can show ({e})."))?;
                    Ok((b, ext, pm))
                });
                if tx.send(Msg::Picture { req: Box::new(req), remote, res: decoded, trace: Box::new(trace) }).is_err() {
                    return;
                }
            }
            let _ = tx.send(Msg::Done);
        });
    }

    pub fn picture_done(&mut self, req: PictureRequest, remote: bool, res: Result<(Vec<u8>, &'static str, Pixmap), String>, trace: ImageTrace) {
        let label = format!("{} on {}", req.model, req.provider);
        match res {
            Ok((bytes, ext, pm)) => {
                let id = self.next_id;
                self.next_id += 1;
                // the size the picture really has (its header), not the one asked for
                let size = lab_pic::header_size(&bytes).unwrap_or((pm.width(), pm.height()));
                let item = Item {
                    id,
                    provider: req.provider,
                    model: req.model,
                    remote,
                    prompt: req.prompt,
                    negative: req.negative.unwrap_or_default(),
                    seed: req.seed,
                    asked: (req.width, req.height),
                    size,
                    steps: trace.steps,
                    ms: trace.ms,
                    bytes,
                    ext,
                    view: lab_pic::thumb(&pm, 1024),
                    thumb: lab_pic::thumb(&pm, 96),
                };
                self.history.push(item);
                self.enforce_caps();
                self.shown.push(Shown::Item(id));
                self.say(format!("Done: {label}."), true);
            }
            Err(e) => {
                self.shown.push(Shown::Failed { label, error: e.clone() });
                self.say(e, false);
            }
        }
    }

    /// Drop the oldest results until the history fits its caps (count and memory); the newest is always kept.
    pub fn enforce_caps(&mut self) {
        let sizes: Vec<usize> = self.history.iter().map(Item::weight).collect();
        let n = lab::over_cap(&sizes);
        if n > 0 {
            self.history.drain(..n);
        }
    }

    pub fn item(&self, id: u64) -> Option<&Item> {
        self.history.iter().find(|i| i.id == id)
    }

    /// The picture being looked at: the first one in the result pane, or the newest.
    pub fn current_item(&self) -> Option<&Item> {
        self.shown.iter().find_map(|s| if let Shown::Item(id) = s { self.item(*id) } else { None }).or_else(|| self.history.last())
    }

    pub fn save_picture(&mut self, port: &dyn LabPort) {
        let Some(it) = self.current_item() else {
            self.say("There is no picture to save yet.", false);
            return;
        };
        let (bytes, ext, stem) = (it.bytes.clone(), it.ext, format!("lab-{}-{}", it.model.chars().filter(|c| c.is_alphanumeric()).take(20).collect::<String>(), it.seed));
        let name = iyakki_uikit::tilekit::clean_name(&stem, ext);
        let dir = self.rig.home.join("Pictures");
        match port.save_file(&dir, &name, &bytes) {
            Ok(p) => {
                port.audit("lab.picture.save", &p.display().to_string());
                self.say(format!("Saved as {}", p.display()), true);
            }
            Err(e) => self.say(e, false),
        }
    }

    pub fn copy_prompt(&mut self) {
        let t = self.current_item().map(|i| i.prompt.clone()).filter(|p| !p.is_empty()).unwrap_or_else(|| self.prompt.text.clone());
        if t.trim().is_empty() {
            self.say("There is no prompt to copy.", false);
        } else {
            iyakki_uikit::ui::clipboard::copy(&t);
            self.say("Prompt copied.", true);
        }
    }

    // ------------------------------------------------------------------------------------------------ vision

    pub fn run_vision(&mut self, port: &dyn LabPort, compare: bool) {
        if self.job.is_some() {
            return;
        }
        let Some(input) = self.input.clone() else {
            self.say("Choose a picture first: a file, the camera, the screen or the clipboard.", false);
            return;
        };
        let question = lab::clean_text(&self.question.text);
        if question.is_empty() {
            self.say("Type a question about the picture (or press one of the quick questions).", false);
            self.focus = Focus::Question;
            return;
        }
        let Some(a) = self.vis_model(Slot::Vis) else {
            self.say("No vision model is available. Settings \u{2192} Models lists them; a vision model (for example a Gemma 3 or Qwen-VL) can be installed there, or tick \"any chat model\" to try one that is not marked.", false);
            return;
        };
        let mut models = vec![a];
        if compare {
            match self.vis_model(Slot::VisB) {
                Some(b) if (&b.provider, &b.model) != (&models[0].provider, &models[0].model) => models.push(b),
                Some(_) => {
                    self.say("Pick two different models to compare.", false);
                    return;
                }
                None => {
                    self.say("Compare needs a second model; none other is available.", false);
                    return;
                }
            }
        }
        // a model that cannot see pictures at all is refused here, before anything is sent
        let now = self.now();
        for m in &models {
            if let Err(why) = port.check_vision(&self.cat, &m.provider, &m.model) {
                self.push_log(LogEntry { ts: now, tab: "vision".into(), provider: m.provider.clone(), model: m.model.clone(), request: "not sent".into(), error: why.clone(), notes: vec!["refused before sending".into()], ..Default::default() });
                self.answers = vec![Answer::new(m, ANS_AREA_A)];
                self.answers[0].done = true;
                self.answers[0].error = Some(why.clone());
                self.say(why, false);
                return;
            }
        }
        let refs: Vec<&LabModel> = models.iter().collect();
        if self.ask_consent(&refs, Then::Vision { compare }) {
            return;
        }
        port.audit("lab.vision", &models.iter().map(|m| format!("{} on {}", m.model, m.provider)).collect::<Vec<_>>().join(" then "));
        self.answers = models.iter().enumerate().map(|(i, m)| Answer::new(m, if i == 0 { ANS_AREA_A } else { ANS_AREA_B })).collect();
        let reqs: Vec<VisionRequest> = models.iter().map(|m| VisionRequest { provider: m.provider.clone(), model: m.model.clone(), question: question.clone(), images: vec![input.png.clone()] }).collect();
        let backend = self.rig.vision.clone();
        let total = reqs.len();
        self.start("Asking", move |tx, cancel| ask_all(&*backend, reqs, total, &tx, &cancel, now));
    }

    // ------------------------------------------------------------------------------------------------ inputs

    pub fn read_path(&mut self, port: &dyn LabPort, input: &str) -> Result<PathBuf, String> {
        port.read_path(self.rig.roots_home.clone(), input)
    }

    pub fn open_picture(&mut self, port: &dyn LabPort) {
        if self.job.is_some() {
            return;
        }
        let text = self.path.value.trim().to_string();
        self.focus = Focus::None;
        if text.is_empty() {
            self.say("Type the path of a picture, or press Browse.", false);
            return;
        }
        let real = match self.read_path(port, &text) {
            Ok(r) => r,
            Err(e) => {
                self.say(e, false);
                return;
            }
        };
        match std::fs::metadata(&real) {
            Ok(m) if m.len() > lab::MAX_INPUT_FILE_BYTES => {
                self.say(format!("That picture is too large to open ({} MB; the limit is {} MB).", m.len() >> 20, lab::MAX_INPUT_FILE_BYTES >> 20), false);
                return;
            }
            Ok(m) if !m.is_file() => {
                self.say("That is not a file.", false);
                return;
            }
            Err(e) => {
                self.say(format!("Could not read that file: {e}"), false);
                return;
            }
            _ => {}
        }
        port.audit("lab.open", &real.to_string_lossy());
        let name = real.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let now = self.now();
        let codec = self.rig.codec.clone();
        self.start("Opening the picture", move |tx, _| {
            let t = Instant::now();
            let r = std::fs::read(&real).map_err(|e| format!("Could not read that file: {e}")).and_then(|b| lab_pic::prepare(codec.as_ref(), &b, &format!("file {name}")));
            let _ = tx.send(Msg::Log(Box::new(source_entry("file", &r, t, now))));
            let _ = tx.send(Msg::Source(r));
        });
    }

    pub fn take_source(&mut self, port: &dyn LabPort, src: Src) {
        if self.job.is_some() {
            return;
        }
        let rig = self.rig.clone();
        let now = self.now();
        port.audit(if src == Src::Camera { "lab.camera" } else { "lab.screen" }, "one picture");
        match src {
            Src::Camera => self.start("Taking one picture with the camera", move |tx, _| {
                let t = Instant::now();
                let r = (rig.camera)().and_then(|j| lab_pic::prepare(rig.codec.as_ref(), &j, "the camera"));
                let _ = tx.send(Msg::Log(Box::new(source_entry("camera", &r, t, now))));
                let _ = tx.send(Msg::Source(r));
            }),
            Src::Screen => self.start("Taking a picture of the screen", move |tx, _| {
                let t = Instant::now();
                let r = (rig.screen)().and_then(|(w, h, rgba)| lab_pic::prepare_rgba(w, h, rgba, "the screen"));
                let _ = tx.send(Msg::Log(Box::new(source_entry("screen", &r, t, now))));
                let _ = tx.send(Msg::Source(r));
            }),
        }
    }

    pub fn from_clipboard(&mut self) {
        if self.job.is_some() {
            return;
        }
        match iyakki_uikit::ui::clipboard::latest_image() {
            Some((w, h, rgba)) => {
                let now = self.now();
                self.start("Preparing the clipboard picture", move |tx, _| {
                    let t = Instant::now();
                    let r = lab_pic::prepare_rgba(w, h, rgba, "the clipboard");
                    let _ = tx.send(Msg::Log(Box::new(source_entry("clipboard", &r, t, now))));
                    let _ = tx.send(Msg::Source(r));
                });
            }
            None => self.say("The clipboard holds no picture. Copy one first (in the browser, or a screenshot).", false),
        }
    }

    // ------------------------------------------------------------------------------------------------ video

    pub fn run_video(&mut self, port: &dyn LabPort) {
        if self.job.is_some() {
            return;
        }
        if let Err(e) = self.rig.frames.available() {
            self.ffmpeg = Err(e.clone());
            self.say(e, false);
            return;
        }
        let text = self.vpath.value.trim().to_string();
        if text.is_empty() {
            self.say("Type the path of a video, or press Browse.", false);
            self.focus = Focus::VPath;
            return;
        }
        let real = match self.read_path(port, &text) {
            Ok(r) => r.to_string_lossy().to_string(),
            Err(e) => {
                self.say(e, false);
                return;
            }
        };
        let question = lab::clean_text(&self.question.text);
        if question.is_empty() {
            self.say("Type a question about the video.", false);
            self.focus = Focus::Question;
            return;
        }
        let Some(m) = self.vis_model(Slot::Vis) else {
            self.say("No vision model is available. Settings \u{2192} Models lists them; tick \"any chat model\" to try one that is not marked.", false);
            return;
        };
        if let Err(why) = port.check_vision(&self.cat, &m.provider, &m.model) {
            self.say(why, false);
            return;
        }
        if self.ask_consent(&[&m], Then::Video) {
            return;
        }
        let mode = if self.even { Sampling::Even } else { Sampling::EveryK(self.every.value.trim().parse::<f64>().unwrap_or(5.0)) };
        let n = self.nframes.clamp_safe(lab::MIN_FRAMES, lab::MAX_FRAMES);
        port.audit("lab.video", &format!("{n} frames with {} on {}", m.model, m.provider));
        self.frames.clear();
        self.answers = vec![Answer::new(&m, ANS_AREA_A)];
        let (src, vision) = (self.rig.frames.clone(), self.rig.vision.clone());
        let now = self.now();
        let req0 = VisionRequest { provider: m.provider.clone(), model: m.model.clone(), question, images: vec![] };
        self.start("Taking frames", move |tx, cancel| {
            let t0 = Instant::now();
            let fail = |tx: &Sender<Msg>, e: String| {
                let _ = tx.send(Msg::Log(Box::new(lab::video_entry(&real, 0, 0, t0.elapsed().as_millis() as u64, &e, now))));
                let _ = tx.send(Msg::Failed(e));
            };
            let info = match src.info(&real) {
                Ok(i) => i,
                Err(e) => return fail(&tx, e),
            };
            let times = match lab::sample_times(info.duration, n, mode) {
                Ok(t) => t,
                Err(e) => return fail(&tx, e),
            };
            let mut progress = |d: usize, total: usize| {
                let _ = tx.send(Msg::Progress(format!("Frame {d} of {total}\u{2026}")));
            };
            let frames = match lab::extract(&*src, &real, &info, &times, &cancel, &mut progress) {
                Ok(f) => f,
                Err(e) => return fail(&tx, e),
            };
            let bytes: u64 = frames.iter().map(|f| f.png.len() as u64).sum();
            let _ = tx.send(Msg::Log(Box::new(lab::video_entry(&real, frames.len(), bytes, t0.elapsed().as_millis() as u64, "", now))));
            let views: Vec<FrameView> = frames.iter().filter_map(|f| decode_thumb(&f.png).map(|thumb| FrameView { time: f.time, thumb })).collect();
            let _ = tx.send(Msg::Frames(views));
            let req = VisionRequest { question: lab::frames_question(&req0.question, &times), images: frames.into_iter().map(|f| f.png).collect(), ..req0 };
            ask_all(&*vision, vec![req], 1, &tx, &cancel, now);
        });
    }

    // ------------------------------------------------------------------------------------------------ picking

    pub fn open_picker(&mut self, slot: Slot) {
        let (title, items): (&str, Vec<(String, String)>) = match slot {
            Slot::PicA | Slot::PicB => ("Picture model", self.cat.pictures.iter().map(|m| (m.label(), format!("{}\u{1}{}", m.provider, m.model))).collect()),
            Slot::Vis | Slot::VisB => ("Vision model", self.cat.vision_choices(self.any_chat).iter().map(|m| (m.label(), format!("{}\u{1}{}", m.provider, m.model))).collect()),
        };
        if items.is_empty() {
            self.say(if matches!(slot, Slot::PicA | Slot::PicB) { "No picture model is available. Install one in Settings \u{2192} Models." } else { "No vision model is available. Install one in Settings \u{2192} Models." }, false);
            return;
        }
        let picker = Picker::new(title, items.iter().map(|(l, _)| l.clone()).collect());
        self.modal = Some(Modal::Pick { slot, picker, items });
    }

    pub fn picked(&mut self, slot: Slot, key: &str) {
        let Some((p, m)) = key.split_once('\u{1}') else { return };
        let v = Some((p.to_string(), m.to_string()));
        match slot {
            Slot::PicA => {
                self.pic_a = v;
                self.fill_defaults();
            }
            Slot::PicB => self.pic_b = v,
            Slot::Vis => self.vis_a = v,
            Slot::VisB => self.vis_b = v,
        }
    }

    pub fn open_file_picker(&mut self, video: bool) {
        let (exts, title, start): (Vec<String>, &str, String) = if video {
            (["mp4", "mkv", "webm", "mov", "avi", "m4v", "mpg", "mpeg", "ogv"].iter().map(|s| s.to_string()).collect(), "Choose a video", self.vpath.value.clone())
        } else {
            (["png", "jpg", "jpeg", "gif", "webp"].iter().map(|s| s.to_string()).collect(), "Choose a picture", self.path.value.clone())
        };
        let start = if start.trim().is_empty() { self.rig.home.join(if video { "Videos" } else { "Pictures" }).display().to_string() } else { start };
        self.modal = Some(Modal::File { video, picker: Box::new(FilePicker::new(PathKind::File, &exts, title, &start, iyakki_uikit::filepicker::current_scope())) });
    }

    pub fn set_any_chat(&mut self, on: bool, port: &dyn LabPort) {
        self.any_chat = on;
        self.reload(port);
    }

    // ------------------------------------------------------------------------------------------------ clicks and keys

    pub fn click(&mut self, n: usize, port: &dyn LabPort) {
        match n {
            T_TAB_PICTURE => self.switch(Tab::Picture),
            T_TAB_VISION => self.switch(Tab::Vision),
            T_TAB_VIDEO => self.switch(Tab::Video),
            T_MODEL => self.open_picker(if self.tab == Tab::Picture { Slot::PicA } else { Slot::Vis }),
            T_MODEL_B => self.open_picker(if self.tab == Tab::Picture { Slot::PicB } else { Slot::VisB }),
            T_GENERATE => self.run_picture(port, false),
            T_COMPARE => {
                if self.tab == Tab::Picture {
                    self.run_picture(port, true)
                } else {
                    self.run_vision(port, true)
                }
            }
            T_CANCEL => self.cancel(),
            T_SAVE => self.save_picture(port),
            T_COPY_PROMPT => self.copy_prompt(),
            T_SEED_REUSE => {
                if let Some(s) = self.current_item().map(|i| i.seed) {
                    self.seed.set(&s.to_string());
                }
            }
            T_SEED_NEW => self.seed.set(&lab::fresh_seed(now_ms()).to_string()),
            T_REFRESH => self.refresh_lists(port),
            T_SRC_FILE => {
                self.focus = Focus::Path;
                if !self.path.value.trim().is_empty() {
                    self.open_picture(port);
                }
            }
            T_SRC_CAMERA => {
                if self.has_camera {
                    self.modal = Some(Modal::Source(Src::Camera));
                } else {
                    self.say("No camera is connected (plug in a USB camera), or your account may not use the camera.", false);
                }
            }
            T_SRC_SCREEN => self.modal = Some(Modal::Source(Src::Screen)),
            T_SRC_CLIP => self.from_clipboard(),
            T_BROWSE => self.open_file_picker(false),
            T_VIDEO_BROWSE => self.open_file_picker(true),
            T_ASK => {
                if self.tab == Tab::Video {
                    self.run_video(port)
                } else {
                    self.run_vision(port, false)
                }
            }
            T_ANY_CHAT => self.set_any_chat(!self.any_chat, port),
            T_COPY_ANSWER => {
                let t = self.answers.iter().map(|a| format!("{}\n{}", a.label, a.text)).collect::<Vec<_>>().join("\n\n");
                if t.trim().is_empty() {
                    self.say("There is no answer to copy yet.", false);
                } else {
                    iyakki_uikit::ui::clipboard::copy(&t);
                    self.say("Answer copied.", true);
                }
            }
            T_FRAMES_DN => self.nframes = self.nframes.saturating_sub(1).max(lab::MIN_FRAMES),
            T_FRAMES_UP => self.nframes = (self.nframes + 1).min(lab::MAX_FRAMES),
            T_MODE => self.even = !self.even,
            T_MEDIA_SETUP => self.request = Some(("quickinstall".into(), "ffmpeg".into())),
            T_LOG_COPY => {
                if self.log.is_empty() {
                    self.say("The log is empty.", false);
                } else {
                    iyakki_uikit::ui::clipboard::copy(&self.log.text());
                    self.say("Log copied.", true);
                }
            }
            T_LOG_CLEAR => {
                self.log.clear();
                self.sync_log();
            }
            n if (T_STYLE0..T_STYLE0 + STYLES.len()).contains(&n) => {
                let phrase = STYLES[n - T_STYLE0].1;
                let t = self.prompt.text.trim_end().to_string();
                self.prompt.set(&if t.is_empty() { phrase.to_string() } else { format!("{t}, {phrase}") });
                self.focus = Focus::Prompt;
            }
            n if (T_QUICK0..T_QUICK0 + QUICK.len()).contains(&n) => {
                self.question.set(QUICK[n - T_QUICK0].1);
                self.focus = Focus::Question;
            }
            n if n >= T_HIST0 && n < T_HIST0 + lab::HISTORY_MAX_ITEMS => {
                // newest first in the list
                let i = self.history.len().checked_sub(1 + (n - T_HIST0));
                if let Some(id) = i.and_then(|i| self.history.get(i)).map(|it| it.id) {
                    self.shown = vec![Shown::Item(id)];
                }
            }
            _ => {}
        }
        self.dirty = true;
    }

    pub fn refresh_lists(&mut self, port: &dyn LabPort) {
        if self.job.is_some() {
            return;
        }
        let refresh = port.refresh_lemonade();
        let ids = port.lemonade_ids();
        if ids.is_empty() {
            self.reload(port);
            self.say("Model lists read again (no Lemonade server is set up to ask).", true);
            return;
        }
        self.start("Reading the model lists", move |tx, _| {
            for id in ids {
                refresh(&id);
            }
            let _ = tx.send(Msg::Refreshed);
        });
    }

    pub fn switch(&mut self, t: Tab) {
        if self.tab != t {
            self.tab = t;
            self.col_scroll = 0.0;
            self.focus = match t {
                Tab::Picture => Focus::Prompt,
                Tab::Vision | Tab::Video => Focus::None,
            };
            self.ffmpeg = self.rig.frames.available();
        }
    }

    pub fn paste(&mut self, t: &str) {
        match self.focus {
            Focus::Prompt => {
                self.prompt.insert(t);
            }
            Focus::Negative => {
                self.negative.insert(t);
            }
            Focus::Question => {
                self.question.insert(t);
            }
            Focus::Path => {
                self.path.paste(t);
            }
            Focus::VPath => {
                self.vpath.paste(t);
            }
            Focus::Seed => {
                self.seed.paste(t);
            }
            Focus::Width => {
                self.width.paste(t);
            }
            Focus::Height => {
                self.height.paste(t);
            }
            Focus::Steps => {
                self.steps.paste(t);
            }
            Focus::Every => {
                self.every.paste(t);
            }
            Focus::None => {
                if self.tab == Tab::Picture {
                    self.prompt.insert(t);
                    self.focus = Focus::Prompt;
                } else {
                    self.question.insert(t);
                    self.focus = Focus::Question;
                }
            }
        }
        self.dirty = true;
    }

    pub fn boxed(&mut self) -> Option<&mut MultiLine> {
        match self.focus {
            Focus::Prompt => Some(&mut self.prompt),
            Focus::Negative => Some(&mut self.negative),
            Focus::Question => Some(&mut self.question),
            _ => None,
        }
    }

    pub fn field(&mut self) -> Option<&mut TextField> {
        match self.focus {
            Focus::Seed => Some(&mut self.seed),
            Focus::Width => Some(&mut self.width),
            Focus::Height => Some(&mut self.height),
            Focus::Steps => Some(&mut self.steps),
            Focus::Path => Some(&mut self.path),
            Focus::VPath => Some(&mut self.vpath),
            Focus::Every => Some(&mut self.every),
            _ => None,
        }
    }

    /// The boxes and fields the keyboard can move between on this tab.
    pub fn order(&self) -> Vec<Focus> {
        match self.tab {
            Tab::Picture => {
                let mut v = vec![Focus::Prompt];
                if self.pic_model(Slot::PicA).is_some_and(|m| m.takes_negative()) {
                    v.push(Focus::Negative);
                }
                v.extend([Focus::Width, Focus::Height]);
                if self.pic_model(Slot::PicA).is_some_and(|m| m.takes_steps()) {
                    v.push(Focus::Steps);
                }
                v.push(Focus::Seed);
                v
            }
            Tab::Vision => vec![Focus::Path, Focus::Question],
            Tab::Video => vec![Focus::VPath, Focus::Every, Focus::Question],
        }
    }

    pub fn key(&mut self, k: &Key, port: &dyn LabPort) -> Flow {
        match k {
            Key::F(2) => {
                self.switch(Tab::Picture);
                return Flow::Stay;
            }
            Key::F(3) => {
                self.switch(Tab::Vision);
                return Flow::Stay;
            }
            Key::F(4) => {
                self.switch(Tab::Video);
                return Flow::Stay;
            }
            Key::Ctrl('\n' | '\r') => {
                match self.tab {
                    Tab::Picture => self.run_picture(port, false),
                    Tab::Vision => self.run_vision(port, false),
                    Tab::Video => self.run_video(port),
                }
                return Flow::Stay;
            }
            Key::Esc => {
                if self.job.is_some() {
                    self.cancel();
                } else if self.focus != Focus::None {
                    self.focus = Focus::None;
                } else {
                    return Flow::Back;
                }
                return Flow::Stay;
            }
            Key::Tab | Key::BackTab => {
                let o = self.order();
                let at = o.iter().position(|f| *f == self.focus);
                let n = o.len();
                self.focus = match (at, matches!(k, Key::Tab)) {
                    (Some(i), true) => o[(i + 1) % n],
                    (Some(i), false) => o[(i + n - 1) % n],
                    (None, true) => o[0],
                    (None, false) => o[n - 1],
                };
                return Flow::Stay;
            }
            _ => {}
        }
        if self.field().is_some() {
            match k {
                Key::Enter => match self.focus {
                    Focus::Path => self.open_picture(port),
                    Focus::VPath => self.focus = Focus::Question,
                    _ => self.focus = Focus::None,
                },
                other => {
                    if let Some(f) = self.field() {
                        f.handle(other);
                    }
                }
            }
            return Flow::Stay;
        }
        if let Some(b) = self.boxed() {
            match k {
                Key::Ctrl('a' | 'A') => b.select_all(),
                Key::Ctrl('c' | 'C') => {
                    if let Some(t) = b.selected_text() {
                        iyakki_uikit::ui::clipboard::copy(&t);
                    }
                }
                Key::Ctrl('x' | 'X') => {
                    if let Some(t) = b.selected_text() {
                        iyakki_uikit::ui::clipboard::copy(&t);
                        b.key(&Key::Delete);
                    }
                }
                other => {
                    b.key(other);
                }
            }
            return Flow::Stay;
        }
        // nothing has the keyboard: single keys are shortcuts
        match (k, self.tab) {
            (Key::Char('p'), _) => self.switch(Tab::Picture),
            (Key::Char('v'), _) => self.switch(Tab::Vision),
            (Key::Char('d'), _) => self.switch(Tab::Video),
            (Key::Char('m'), Tab::Picture | Tab::Vision | Tab::Video) => self.click(T_MODEL, port),
            (Key::Char('c'), Tab::Picture | Tab::Vision) => self.click(T_COMPARE, port),
            (Key::Char('g') | Key::Enter, Tab::Picture) => self.run_picture(port, false),
            (Key::Char('s'), Tab::Picture) => self.save_picture(port),
            (Key::Char('r') | Key::Enter, Tab::Vision) => self.run_vision(port, false),
            (Key::Char('r') | Key::Enter, Tab::Video) => self.run_video(port),
            (Key::Char('f'), Tab::Vision) => self.click(T_SRC_FILE, port),
            (Key::Char('b'), Tab::Vision) => self.click(T_BROWSE, port),
            (Key::Char('b'), Tab::Video) => self.click(T_VIDEO_BROWSE, port),
            (Key::Char('k'), Tab::Vision) => self.click(T_SRC_CAMERA, port),
            (Key::Char('x'), Tab::Vision) => self.click(T_SRC_SCREEN, port),
            (Key::Char('l'), Tab::Vision) => self.click(T_SRC_CLIP, port),
            (Key::Char('e') | Key::Tab, _) => self.focus = self.order()[0],
            (Key::Char('+' | '='), Tab::Video) => self.click(T_FRAMES_UP, port),
            (Key::Char('-'), Tab::Video) => self.click(T_FRAMES_DN, port),
            (Key::Up | Key::PageUp, _) => self.col_scroll = (self.col_scroll - 60.0).max(0.0),
            (Key::Down | Key::PageDown, _) => self.col_scroll = (self.col_scroll + 60.0).min(self.col_max),
            _ => {}
        }
        self.dirty = true;
        Flow::Stay
    }

    pub fn modal_event(&mut self, ev: &Event, port: &dyn LabPort) {
        match self.modal.take() {
            Some(Modal::Consent { providers, then }) => {
                let yes = matches!(ev, Event::Key(Key::Char('y' | 'Y')) | Event::Tab(T_YES));
                let no = matches!(ev, Event::Key(Key::Esc | Key::Char('n' | 'N')) | Event::Tab(T_NO));
                self.modal = Some(Modal::Consent { providers, then });
                if yes {
                    self.consent_yes(port);
                } else if no {
                    self.modal = None;
                    self.say("Nothing was sent. Choose a model on this computer, or say yes to use that one.", true);
                }
            }
            Some(Modal::Source(src)) => {
                let yes = matches!(ev, Event::Key(Key::Char('y' | 'Y')) | Event::Tab(T_YES));
                let no = matches!(ev, Event::Key(Key::Esc | Key::Char('n' | 'N')) | Event::Tab(T_NO));
                if yes {
                    self.take_source(port, src);
                } else if !no {
                    self.modal = Some(Modal::Source(src));
                }
            }
            Some(Modal::Pick { slot, mut picker, items }) => match picker.handle_event(ev) {
                PickEvent::Chosen(label) => {
                    if let Some((_, key)) = items.iter().find(|(l, _)| *l == label) {
                        let key = key.clone();
                        self.picked(slot, &key);
                    }
                }
                PickEvent::Cancel => {}
                PickEvent::None => self.modal = Some(Modal::Pick { slot, picker, items }),
            },
            Some(Modal::File { video, mut picker }) => match picker.handle_event(ev) {
                PickOut::Chosen(p) => {
                    let text = p.display().to_string();
                    if video {
                        self.vpath.set(&text);
                    } else {
                        self.path.set(&text);
                        self.open_picture(port);
                    }
                }
                PickOut::Cancel => {}
                PickOut::Stay => self.modal = Some(Modal::File { video, picker }),
            },
            None => {}
        }
        self.dirty = true;
    }

    pub fn animating(&self) -> bool {
        self.job.is_some()
    }
}

/// Ask each request in turn (one after the other), streaming the text into its slot.
fn ask_all(backend: &dyn VisionBackend, reqs: Vec<VisionRequest>, total: usize, tx: &Sender<Msg>, cancel: &Arc<AtomicBool>, now: u64) {
    for (slot, req) in reqs.into_iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            break;
        }
        let _ = tx.send(Msg::Progress(format!("Asking {} of {}: {} on {}\u{2026}", slot + 1, total, req.model, req.provider)));
        let (res, trace) = backend.ask(&req, cancel, &mut |piece| {
            let _ = tx.send(Msg::Text { slot, piece: piece.to_string() });
        });
        let _ = tx.send(Msg::Log(Box::new(lab::vision_entry(&req, &trace, &res, now))));
        if tx.send(Msg::Answer { slot, res, ms: trace.ms }).is_err() {
            return;
        }
    }
    let _ = tx.send(Msg::Done);
}

/// The log line for getting a picture in (a file, the camera, the screen, the clipboard).
fn source_entry(what: &str, r: &Result<Prepared, String>, started: Instant, now: u64) -> LogEntry {
    let mut e = LogEntry { ts: now, tab: "vision".into(), provider: "this computer".into(), model: what.into(), request: "get one picture".into(), ms: started.elapsed().as_millis() as u64, ..Default::default() };
    match r {
        Ok(p) => {
            e.bytes = p.png.len() as u64;
            e.request = format!("one picture {}x{}", p.w, p.h);
            if p.scaled() {
                e.notes.push(format!("downscaled from {}x{} ({}) to {}x{}", p.from.0, p.from.1, lab::bytes_text(p.from_bytes), p.w, p.h));
            }
        }
        Err(err) => e.error = err.clone(),
    }
    e
}

/// A small copy of a frame's PNG for the strip.
fn decode_thumb(png: &[u8]) -> Option<Arc<Pixmap>> {
    Pixmap::decode_png(png).ok().map(|p| lab_pic::thumb(&p, 160))
}

impl LabApp {
    pub fn title(&self) -> &'static str {
        "Lab"
    }

    pub fn icon(&self) -> Icon {
        Icon::Lab
    }

    pub fn accent(&self) -> [u8; 4] {
        ACCENT
    }

    pub fn hint(&self) -> String {
        match self.tab {
            Tab::Picture => "F2 Picture · F3 Vision · F4 Video · Ctrl+Enter generate · Tab next field · Esc cancel or back".into(),
            Tab::Vision => "F2 Picture · F3 Vision · F4 Video · Ctrl+Enter ask · f file · k camera · x screen · l clipboard · Esc cancel or back".into(),
            Tab::Video => "F2 Picture · F3 Vision · F4 Video · Ctrl+Enter take frames and ask · + - frames · Esc cancel or back".into(),
        }
    }

    pub fn help(&self) -> Vec<String> {
        [
            "F2 / F3 / F4: Picture / Vision / Video (or p / v / d when no box has the keyboard); Tab moves between the boxes",
            "Picture: type a prompt, choose the model (m), size and seed; Ctrl+Enter or g makes the picture; c compares two models, one after the other; s saves it to Pictures",
            "Vision: choose a picture (f file, b browse, k camera, x screen, l clipboard), type or pick a question, Ctrl+Enter asks; c asks a second model the same thing",
            "Video: choose a video file, the number of frames (+ and -) and how they are spread; the frames are sent to a vision model. It needs ffmpeg, which you install yourself",
            "Esc cancels a running job; a request already sent may still finish on the server and its answer is dropped",
            "A model on another computer is used only after you say yes to that provider (once per session); the Run button area says where each run goes",
            "The log at the bottom lists every call (no secrets). Copy log and Clear log are beside it. Nothing is saved to disk except Save picture",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    pub fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }

    pub fn wants_redraw(&mut self) -> bool {
        if self.animating() {
            self.ticks % 3 == 0
        } else {
            std::mem::take(&mut self.dirty)
        }
    }
}

impl LabApp {
    pub fn handle_with(&mut self, ev: &Event, port: &dyn LabPort) -> Flow {
        if matches!(ev, Event::Tick) {
            self.on_tick(port);
            return Flow::Stay;
        }
        if self.modal.is_some() {
            self.modal_event(ev, port);
            return Flow::Stay;
        }
        // selection and Ctrl+C in the log and the answers when no box has the keyboard
        if self.focus == Focus::None || matches!(ev, Event::AreaAt(..) | Event::MouseUp(..)) {
            let mut used = self.logv.handle(ev);
            for a in self.answers.iter_mut() {
                used |= a.view.handle(ev);
            }
            if used && !matches!(ev, Event::MouseUp(..)) {
                return Flow::Stay;
            }
        }
        match ev {
            Event::Key(k) => return self.key(k, port),
            Event::Tab(n) => {
                // these keep or take the keyboard themselves
                let keeps = *n == T_SRC_FILE || (T_STYLE0..T_STYLE0 + STYLES.len()).contains(n) || (T_QUICK0..T_QUICK0 + QUICK.len()).contains(n);
                if !keeps {
                    self.focus = Focus::None;
                }
                self.click(*n, port);
            }
            Event::Paste(t) => self.paste(t),
            Event::MouseMove(x, y) => self.mouse = (*x, *y),
            Event::AreaAt(PROMPT_AREA, x, y) => {
                self.focus = Focus::Prompt;
                self.prompt.press(*x, *y);
            }
            Event::AreaAt(NEG_AREA, x, y) => {
                self.focus = Focus::Negative;
                self.negative.press(*x, *y);
            }
            Event::AreaAt(QUESTION_AREA, x, y) => {
                self.focus = Focus::Question;
                self.question.press(*x, *y);
            }
            Event::FocusAt(id, x) => {
                let (f, which) = match *id {
                    F_SEED => (Focus::Seed, &mut self.seed),
                    F_W => (Focus::Width, &mut self.width),
                    F_H => (Focus::Height, &mut self.height),
                    F_STEPS => (Focus::Steps, &mut self.steps),
                    F_PATH => (Focus::Path, &mut self.path),
                    F_VPATH => (Focus::VPath, &mut self.vpath),
                    F_EVERY => (Focus::Every, &mut self.every),
                    _ => return Flow::Stay,
                };
                which.click(*x);
                self.focus = f;
            }
            Event::Scroll(d) => {
                let taken = match self.focus {
                    Focus::Prompt => self.prompt.wheel(*d),
                    Focus::Negative => self.negative.wheel(*d),
                    Focus::Question => self.question.wheel(*d),
                    _ => false,
                };
                if !taken {
                    let (mx, my) = self.mouse;
                    if self.logv.pane.over(mx, my) {
                        self.logv.scroll_by(*d);
                    } else if let Some(a) = self.answers.iter_mut().find(|a| a.view.pane.over(mx, my)) {
                        a.view.scroll_by(*d);
                    } else {
                        self.col_scroll = (self.col_scroll - *d as f32 * 40.0).clamp_safe(0.0, self.col_max);
                    }
                }
            }
            _ => {}
        }
        self.dirty = true;
        Flow::Stay
    }

    pub fn render_with(&mut self, g: &mut iyakki_uikit::gfx::Gfx, area: iyakki_uikit::ui::R, s: f32, port: &dyn LabPort, tick: u64) {
        self.draw(g, area, s, port, tick);
    }
}
