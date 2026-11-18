//! The media port: what an SDK app is handed so that it can embed an audio or video player (see [`crate::audioview::AudioView`]). The port is
//! the core's `media` facet seen from the app: the person's own media files, one session at a time, control and events. The app never gets
//! a byte of the sound or a path: the core decodes, the sound goes through the core's mixer (so volume, ducking and the sound doctor apply),
//! and the app only draws state, position and a level. [`MemMedia`] is the in-memory port for tests and the browser preview: a virtual
//! clock the test moves, and the same rules as the core (a remote source waits for the person, a live stream cannot be moved, one session).

use std::cell::RefCell;
use std::collections::VecDeque;
use std::rc::Rc;

/// Where the sound comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A file of the person's home folder, by the name [`MediaPort::list_home`] gave.
    Home(String),
    /// An http(s) address (the person is asked first).
    Stream(String),
    /// A station of the person's radio list, by name (the person is asked first).
    Station(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub size: u64,
    pub video: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayState {
    Idle,
    AwaitingApproval,
    Opening,
    Ready,
    Playing,
    Paused,
    Buffering,
    Ended,
    Failed,
}

impl PlayState {
    pub fn word(self) -> &'static str {
        match self {
            PlayState::Idle => "stopped",
            PlayState::AwaitingApproval => "waiting for your answer",
            PlayState::Opening => "opening",
            PlayState::Ready => "ready",
            PlayState::Playing => "playing",
            PlayState::Paused => "paused",
            PlayState::Buffering => "buffering",
            PlayState::Ended => "ended",
            PlayState::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TrackKind {
    Audio,
    Video,
    Caption,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub id: u32,
    pub kind: TrackKind,
    pub label: String,
    pub lang: String,
    pub selected: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Info {
    pub title: String,
    pub duration_ms: Option<u64>,
    pub seekable: bool,
    pub has_video: bool,
    pub codec: String,
    pub tracks: Vec<Track>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// The person must allow a remote source (the sentence says what).
    Approval(String),
    Opened(Info),
    State(PlayState),
    Position(u64),
    /// 0 to 1000.
    Level(u32),
    Error(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaError {
    /// The core said no, in a sentence for the person.
    Refused(String),
    /// This host has no media player (a preview, a core without one).
    Unavailable,
}

impl MediaError {
    pub fn sentence(&self) -> String {
        match self {
            MediaError::Refused(s) => s.clone(),
            MediaError::Unavailable => "The media player is not available here.".into(),
        }
    }
}

pub trait MediaPort {
    /// The sound (and video) files of the person's home folder.
    fn list_home(&mut self) -> Result<Vec<Item>, MediaError>;
    /// Open `source` in this app's one session (an earlier one is closed). `Ok(true)`: the person must allow it first.
    fn open(&mut self, source: &Source, title: &str, autoplay: bool) -> Result<bool, MediaError>;
    fn decide(&mut self, allow: bool) -> Result<(), MediaError>;
    fn play(&mut self) -> Result<(), MediaError>;
    fn pause(&mut self) -> Result<(), MediaError>;
    fn seek(&mut self, ms: u64) -> Result<(), MediaError>;
    /// Speed in thousandths (1000 is normal; 500 to 2000).
    fn set_rate(&mut self, permille: u32) -> Result<(), MediaError>;
    /// This player's own volume, 0 to 100 percent.
    fn set_volume(&mut self, percent: u32) -> Result<(), MediaError>;
    fn set_mute(&mut self, mute: bool) -> Result<(), MediaError>;
    fn select_track(&mut self, id: u32) -> Result<(), MediaError>;
    /// The events since the last call.
    fn poll(&mut self) -> Vec<Event>;
    /// The picture of a video that is due now when its `seq` is above `after` (RGBA, scaled by the core to a size that fits the display
    /// list); `None`: nothing new, or the source has no picture. A sound-only port keeps the default.
    fn frame(&mut self, _after: u64) -> Result<Option<crate::canvas::Image>, MediaError> {
        Ok(None)
    }
    /// Stop the sound and close the session.
    fn close(&mut self);
}

#[derive(Default)]
struct Mem {
    files: Vec<(String, Item, Option<u64>)>,
    streams: Vec<(String, Option<u64>)>,
    open: bool,
    info: Option<Info>,
    state: Option<PlayState>,
    pos: u64,
    rate: u32,
    volume: u32,
    muted: bool,
    autoplay: bool,
    out: VecDeque<Event>,
    calls: Vec<String>,
    off: bool,
    refuse: Option<String>,
    level: u32,
    /// Name -> (pictures a second, width, height) of the files that have a picture.
    video: Vec<(String, u32, u32, u32)>,
    cur_video: Option<(u32, u32, u32)>,
}

/// An in-memory player. Clone it before handing it to a view: the clone is the test's handle (`advance` moves the clock).
#[derive(Clone, Default)]
pub struct MemMedia {
    m: Rc<RefCell<Mem>>,
}

impl MemMedia {
    pub fn new() -> MemMedia {
        let m = MemMedia::default();
        m.m.borrow_mut().rate = 1000;
        m.m.borrow_mut().volume = 100;
        m
    }
    /// A port that has no player at all.
    pub fn unavailable() -> MemMedia {
        let m = MemMedia::new();
        m.m.borrow_mut().off = true;
        m
    }
    pub fn put_file(&self, name: &str, duration_ms: u64, video: bool) {
        self.m.borrow_mut().files.push((name.into(), Item { name: name.into(), size: duration_ms * 16, video }, Some(duration_ms)));
    }
    /// A video file: it has a picture, `fps` of them a second, `w` x `h` pixels; picture `n` is a plain field whose red is `n`.
    pub fn put_video(&self, name: &str, duration_ms: u64, fps: u32, w: u32, h: u32) {
        self.put_file(name, duration_ms, true);
        self.m.borrow_mut().video.push((name.into(), fps, w, h));
    }
    /// A stream address; `None`: a live stream.
    pub fn put_stream(&self, url: &str, duration_ms: Option<u64>) {
        self.m.borrow_mut().streams.push((url.into(), duration_ms));
    }
    /// The next `open` is refused with this sentence.
    pub fn refuse_next(&self, why: &str) {
        self.m.borrow_mut().refuse = Some(why.into());
    }
    /// The level the meter reports while playing (0 to 1000).
    pub fn set_level(&self, l: u32) {
        self.m.borrow_mut().level = l;
    }
    /// Move the clock: a playing session advances by `ms` at its rate, reports position and level, and ends at its end.
    pub fn advance(&self, ms: u64) {
        let mut g = self.m.borrow_mut();
        if g.state != Some(PlayState::Playing) {
            return;
        }
        g.pos += ms * u64::from(g.rate) / 1000;
        let level = if g.muted { 0 } else { g.level * g.volume / 100 };
        g.out.push_back(Event::Level(level));
        let dur = g.info.as_ref().and_then(|i| i.duration_ms);
        if let Some(d) = dur {
            if g.pos >= d {
                g.pos = d;
                g.out.push_back(Event::Position(d));
                g.state = Some(PlayState::Ended);
                g.out.push_back(Event::State(PlayState::Ended));
                return;
            }
        }
        let p = g.pos;
        g.out.push_back(Event::Position(p));
    }
    /// The calls the view made, in order (`open home a.mp3`, `play`, `seek 5000`, ...).
    pub fn calls(&self) -> Vec<String> {
        self.m.borrow().calls.clone()
    }
    pub fn position(&self) -> u64 {
        self.m.borrow().pos
    }
    pub fn volume(&self) -> (u32, bool) {
        let g = self.m.borrow();
        (g.volume, g.muted)
    }
    pub fn rate(&self) -> u32 {
        self.m.borrow().rate
    }
    /// Tests: the open file gains a second audio track and a caption track (the core announces them with the file).
    pub fn add_tracks_for_test(&self) {
        let mut g = self.m.borrow_mut();
        if let Some(i) = g.info.as_mut() {
            if i.tracks.len() < 3 {
                i.tracks.push(Track { id: 1, kind: TrackKind::Audio, label: "Commentary".into(), lang: "en".into(), selected: false });
                i.tracks.push(Track { id: 2, kind: TrackKind::Caption, label: "English".into(), lang: "en".into(), selected: false });
                let info = i.clone();
                g.out.push_back(Event::Opened(info));
            }
        }
    }
    pub fn is_open(&self) -> bool {
        self.m.borrow().open
    }
}

fn off(g: &Mem) -> Result<(), MediaError> {
    if g.off { Err(MediaError::Unavailable) } else { Ok(()) }
}

impl MediaPort for MemMedia {
    fn list_home(&mut self) -> Result<Vec<Item>, MediaError> {
        let g = self.m.borrow();
        off(&g)?;
        Ok(g.files.iter().map(|(_, i, _)| i.clone()).collect())
    }

    fn open(&mut self, source: &Source, title: &str, autoplay: bool) -> Result<bool, MediaError> {
        let mut g = self.m.borrow_mut();
        off(&g)?;
        if let Some(why) = g.refuse.take() {
            return Err(MediaError::Refused(why));
        }
        let (label, duration, remote) = match source {
            Source::Home(n) => {
                let Some((_, _, d)) = g.files.iter().find(|(k, _, _)| k == n).cloned() else { return Err(MediaError::Refused("That file is not in your home folder.".into())) };
                g.calls.push(format!("open home {n}"));
                (n.rsplit('/').next().unwrap_or(n).to_string(), d, false)
            }
            Source::Stream(u) => {
                let d = g.streams.iter().find(|(k, _)| k == u).and_then(|(_, d)| *d);
                g.calls.push(format!("open stream {u}"));
                (u.clone(), d, true)
            }
            Source::Station(n) => {
                g.calls.push(format!("open station {n}"));
                (n.clone(), None, true)
            }
        };
        g.open = true;
        g.pos = 0;
        g.autoplay = autoplay;
        g.out.clear();
        let vid = match source {
            Source::Home(n) => g.video.iter().find(|(k, ..)| k == n).map(|(_, f, w, h)| (*f, *w, *h)),
            _ => None,
        };
        g.cur_video = vid;
        let mut tracks = vec![Track { id: 0, kind: TrackKind::Audio, label: "Audio".into(), lang: String::new(), selected: true }];
        if vid.is_some() {
            tracks.push(Track { id: 1, kind: TrackKind::Video, label: "Video".into(), lang: String::new(), selected: true });
        }
        g.info = Some(Info { title: if title.is_empty() { label } else { title.into() }, duration_ms: duration, seekable: duration.is_some(), has_video: vid.is_some(), codec: "MP3".into(), tracks });
        if remote {
            g.state = Some(PlayState::AwaitingApproval);
            g.out.push_back(Event::State(PlayState::AwaitingApproval));
            g.out.push_back(Event::Approval("Play this from the internet".into()));
        } else {
            start(&mut g);
        }
        Ok(remote)
    }

    fn decide(&mut self, allow: bool) -> Result<(), MediaError> {
        let mut g = self.m.borrow_mut();
        g.calls.push(format!("decide {allow}"));
        if g.state != Some(PlayState::AwaitingApproval) {
            return Err(MediaError::Refused("Nothing is waiting for your answer.".into()));
        }
        if allow {
            start(&mut g);
        } else {
            g.state = Some(PlayState::Failed);
            g.out.push_back(Event::Error("You did not allow that source.".into()));
            g.out.push_back(Event::State(PlayState::Failed));
        }
        Ok(())
    }

    fn play(&mut self) -> Result<(), MediaError> {
        let mut g = self.m.borrow_mut();
        g.calls.push("play".into());
        match g.state {
            Some(PlayState::Ready | PlayState::Paused | PlayState::Ended) => {
                if g.state == Some(PlayState::Ended) {
                    g.pos = 0;
                }
                g.state = Some(PlayState::Playing);
                g.out.push_back(Event::State(PlayState::Playing));
                Ok(())
            }
            Some(PlayState::Playing) => Ok(()),
            Some(PlayState::AwaitingApproval) => Err(MediaError::Refused("Allow the source first.".into())),
            _ => Err(MediaError::Refused("Nothing is open.".into())),
        }
    }

    fn pause(&mut self) -> Result<(), MediaError> {
        let mut g = self.m.borrow_mut();
        g.calls.push("pause".into());
        if g.state == Some(PlayState::Playing) {
            g.state = Some(PlayState::Paused);
            g.out.push_back(Event::State(PlayState::Paused));
        }
        Ok(())
    }

    fn seek(&mut self, ms: u64) -> Result<(), MediaError> {
        let mut g = self.m.borrow_mut();
        g.calls.push(format!("seek {ms}"));
        let Some(d) = g.info.as_ref().and_then(|i| i.duration_ms) else { return Err(MediaError::Refused("A live stream cannot be moved.".into())) };
        g.pos = ms.min(d);
        let p = g.pos;
        g.out.push_back(Event::Position(p));
        Ok(())
    }

    fn set_rate(&mut self, permille: u32) -> Result<(), MediaError> {
        let mut g = self.m.borrow_mut();
        g.calls.push(format!("rate {permille}"));
        if !(500..=2000).contains(&permille) {
            return Err(MediaError::Refused("The speed is 0.5x to 2x.".into()));
        }
        g.rate = permille;
        Ok(())
    }

    fn set_volume(&mut self, percent: u32) -> Result<(), MediaError> {
        let mut g = self.m.borrow_mut();
        g.calls.push(format!("volume {percent}"));
        if percent > 100 {
            return Err(MediaError::Refused("The volume is 0 to 100 percent.".into()));
        }
        g.volume = percent;
        Ok(())
    }

    fn set_mute(&mut self, mute: bool) -> Result<(), MediaError> {
        let mut g = self.m.borrow_mut();
        g.calls.push(format!("mute {mute}"));
        g.muted = mute;
        Ok(())
    }

    fn select_track(&mut self, id: u32) -> Result<(), MediaError> {
        let mut g = self.m.borrow_mut();
        g.calls.push(format!("track {id}"));
        let Some(i) = g.info.as_mut() else { return Err(MediaError::Refused("Nothing is open.".into())) };
        let Some(kind) = i.tracks.iter().find(|t| t.id == id).map(|t| t.kind) else { return Err(MediaError::Refused("That track does not exist.".into())) };
        for t in i.tracks.iter_mut().filter(|t| t.kind == kind) {
            t.selected = t.id == id;
        }
        Ok(())
    }

    fn poll(&mut self) -> Vec<Event> {
        self.m.borrow_mut().out.drain(..).collect()
    }

    fn frame(&mut self, after: u64) -> Result<Option<crate::canvas::Image>, MediaError> {
        let mut g = self.m.borrow_mut();
        off(&g)?;
        g.calls.push("frame".into());
        let Some((fps, w, h)) = g.cur_video else { return Ok(None) };
        let n = g.pos * u64::from(fps) / 1000;
        let seq = n + 1;
        if seq <= after {
            return Ok(None);
        }
        let data: Vec<u8> = (0..w * h).flat_map(|_| [(n % 256) as u8, 40, 80, 255]).collect();
        Ok(Some(crate::canvas::Image { format: crate::canvas::ImageFormat::Rgba, width: w, height: h, seq, data: std::sync::Arc::new(data) }))
    }

    fn close(&mut self) {
        let mut g = self.m.borrow_mut();
        g.calls.push("close".into());
        g.open = false;
        g.state = None;
        g.info = None;
        g.out.clear();
    }
}

fn start(g: &mut Mem) {
    g.state = Some(PlayState::Opening);
    g.out.push_back(Event::State(PlayState::Opening));
    if let Some(i) = g.info.clone() {
        g.out.push_back(Event::Opened(i));
    }
    g.state = Some(PlayState::Ready);
    g.out.push_back(Event::State(PlayState::Ready));
    if g.autoplay {
        g.state = Some(PlayState::Playing);
        g.out.push_back(Event::State(PlayState::Playing));
    }
}
