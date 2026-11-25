//! TALES: describe a story, choose who it is for, and the model writes it, draws it and reads it aloud. A thin screen over
//! `iyakki_story_kit` (prompt, parsing, pictures, reading pipeline, library): this file only draws and routes keys and clicks.
//!
//! The **story view** fills the left: a cover the shell draws, the story with its pictures in place and the sentence being
//! read lit, and a player bar whose bars move with the real sound. The **form** is a pane on the right (the idea in a
//! box of several lines, who it is for, the kind, length, language, tone, characters, pictures and their style, the voice) with
//! a Create story button that is always in view; on a small screen the pane is a toggle. **Listen again** lists the saved
//! stories. The model gets no tools. Speech comes from Settings → Voice; pictures are small SVG drawings the text model
//! writes (checked and rebuilt from safe parts only) or come from a picture model. With neither, the story is still
//! written, shown and saved.

use iyakki_uikit::safeclamp::SafeClamp;
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use iyakki_chat_model::{ChatBackend, Message, ToolCall};
use iyakki_chatview::chat_view::{self, ChatView, NoPics, TOGGLE_BASE};
use iyakki_chatview::chatpic::PicCache;
use iyakki_chatview::model_picker::{self, Current, ModelList, Pick, TopRow};
use iyakki_piclog::{self as piclog, kind as plk, PicEvent};
use iyakki_scene::{self as scene, CharacterSheet};
use iyakki_story_kit::storyillus::{self as illus, Coverage, Readiness};
use iyakki_story_kit::{self as st, Hints, StoryStore};
use iyakki_story_kit::{Audience, Genre, Length, PictureMode, PictureStyle, Tone};
use iyakki_story_kit::{Block, Chunk, ModelPlan, Story, StoryEntry, StoredPic, StoredVariant, TtsStatus, LANGUAGES};
use iyakki_story_kit::{Status, StoryOptions};
use iyakki_story_model::ModelChoice;
pub type Reader = Arc<dyn iyakki_story_model::StoryReader>;
use iyakki_uikit::fx::{self, mix, with_alpha, CYAN, MAGENTA, PANEL, WELL};
use iyakki_uikit::gfx::{rgb, Face, Gfx, Icon};
use iyakki_uikit::neon::{glow_alphas, glow_bounds, glow_card, glow_card_keep, glow_ring_again, round_button, Glyph, GlowRing};
use iyakki_uikit::textsel::Pane;
use iyakki_uikit::ui::{Event, Key, TextField, CARD_HOVER, DIM, ERR, MUTED, OK, R, TEXT};
use iyakki_uikit::widgets::{menu_list, select_box, select_width, PickEvent, Picker};
use iyakki_voice_model::hostengines;
use iyakki_voice_model::hostvoice::{self, HostVoice};
use iyakki_voice_model::voice::{Highlight, SpeakOpts, SpeechEngine};

pub mod port;
pub use port::{AgentEntry, AgentRunReq, TalesPort, TalesWorker};
use iyakki_uikit::blink::blink;

/// What the screen needs next: nothing, only the lit words and the player bar, or the whole screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Redraw {
    None,
    Live,
    Full,
}

/// What a key or a click did to the screen: stay, or go back to the apps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    Stay,
    Back,
}

/// The tile's colour: lamplight gold.
pub const ACCENT: [u8; 4] = rgb(255, 220, 130);

pub const IDEA_AREA: usize = 63_410;
pub const SEARCH_FIELD: usize = 63_412;
pub const TEXT_PANE: usize = 63_420;
pub const BAR_AREA: usize = 63_421;

/// The word being said: a solid fill of the tile's colour with dark text on it (the sentence around it is a soft wash).
pub const WORD_FILL: [u8; 4] = [255, 220, 130, 240];
pub const WORD_INK: [u8; 4] = [22, 18, 6, 255];
/// Heartbeats a plain click in the story waits for a second click (a double click selects a word) before it starts reading.
pub const READ_CLICK_DELAY: u32 = 14;

// click codes (Event::Tab)
pub const T_OPTIONS: usize = 1;
pub const T_VERSION_MENU: usize = 4;
pub const T_LANG_MENU: usize = 5;
pub const T_KIND_MENU: usize = 6;
pub const T_TONE_MENU: usize = 7;
pub const T_OPTIONS_MENU: usize = 8;
pub const T_HINT: usize = 15; // + 0 length, 1 language, 2 kind, 3 tone, 4 picture style
pub const T_HOW: usize = 130;
pub const T_TURN_ON: usize = 132; // the chip that sets reading aloud up
pub const T_MENU_CLOSE: usize = 131;
pub const T_VERSION_PICK: usize = 500; // + version number (1 is the first)
pub const DIV_AREA: usize = 63_430;
pub const T_STORY_PAGE: usize = 2;
pub const T_LIB_PAGE: usize = 3;
pub const T_AUDIENCE: usize = 10; // + index
pub const T_GENRE: usize = 20;
pub const T_LENGTH: usize = 30;
pub const T_LANG: usize = 40;
pub const T_MODEL: usize = 120;
pub const T_PICMODEL: usize = 121;
pub const T_PICMODE: usize = 140; // + index
pub const T_DRAWMODE: usize = 141;
pub const T_VIDEO: usize = 142;
pub const T_TONE: usize = 60;
pub const T_READ_WHILE: usize = 70;
pub const T_AUTOSAVE: usize = 71;
pub const T_CREATE: usize = 80;
pub const T_QUICK: usize = 90; // + index
pub const T_SUGGEST: usize = 100; // + index
pub const T_PLAY: usize = 200;
pub const T_STOP: usize = 201;
pub const T_PREV: usize = 202;
pub const T_NEXT: usize = 203;
pub const T_REPLAY: usize = 204;
pub const T_VOICE: usize = 205;
pub const T_SPEED: usize = 206;
pub const T_SAVE_AUDIO: usize = 207;
pub const T_FAV: usize = 208;
pub const T_COPY: usize = 209;
pub const T_SAVE: usize = 210;
pub const T_EXPORT: usize = 211;
pub const T_NEW: usize = 212;
pub const T_STOP_WRITING: usize = 213;
pub const T_CANCEL_AUDIO: usize = 214;
pub const T_LIB_OPEN: usize = 300;
pub const T_LIB_LISTEN: usize = 301;
pub const T_LIB_FAV: usize = 302;
pub const T_LIB_DELETE: usize = 303;
pub const T_LIB_EXPORT: usize = 304;
pub const T_PIC_REDO: usize = 400; // + picture index
pub const T_PIC_DELETE: usize = 420; // + picture index
pub const T_COVERAGE: usize = 143;
pub const T_PICS_GO: usize = 215;
pub const T_PICS_SKIP: usize = 216;
pub const T_PICS_CANCEL: usize = 217;
pub const T_PDF: usize = 218;
#[allow(dead_code)] // reserved token id
pub const T_LOOK: usize = 144;
pub const T_QUALITY: usize = 145;
pub const T_STORYMODEL: usize = 146;
pub const T_KEEPLOOK: usize = 147;
pub const T_PICLOG: usize = 148;
pub const T_LOG_COPY: usize = 149;
pub const T_LOG_CLEAR: usize = 150;
pub const T_LOG_VERBOSE: usize = 151;
pub const T_LOG_CLOSE: usize = 152;
pub const T_PIC_INSTALL: usize = 153;
pub const T_TIMEOUT: usize = 154;
pub const T_PERSTYLE: usize = 155;
pub const T_REALISTIC: usize = 156;
pub const T_REAL_INSTALL: usize = 157;
pub const T_KEEP_ONLY: usize = 158;
pub const T_STYLE: usize = 160; // + index of the style
pub const T_CHOICE_INSTALL: usize = 170; // + index of the picture model in the list
pub const T_ALT_PICK: usize = 700; // + scene * 24 + index of the extra picture
pub const T_ALT_DEL: usize = 860; // + scene * 24 + index
pub const T_ALT_MORE: usize = 1010; // + scene
pub const T_LINK: usize = 1100; // + scene * 3 + which of its offered links

pub const SUGGESTIONS: [&str; 6] = ["A little fox who is afraid of the dark", "The moon lost its silver mitten", "A robot who learns to laugh", "The kindest dragon in the valley", "A rainy day treasure hunt", "A tiny seed that wanted to touch the sky"];
pub const SPEEDS: [f32; 6] = [0.8, 0.9, 1.0, 1.1, 1.25, 1.5];
/// Below this window width the form is a toggle instead of a pane beside the story.
pub const WIDE_PX: f32 = 900.0;

// what the idea text can suggest (indexes into the explicit and dismissed lists)
pub const H_LEN: usize = 0;
pub const H_LANG: usize = 1;
pub const H_GENRE: usize = 2;
pub const H_TONE: usize = 3;
pub const H_STYLE: usize = 4;

/// The one open drop-down, popover or panel of the composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Menu {
    Kind,
    Tone,
    Options,
    /// The language list opened from Options.
    Language,
    /// The versions of the story.
    Version,
    /// The longer text about reading aloud (opened by the hint line's How).
    How,
}

/// An external agent as the chooser shows it.
#[derive(Clone, Debug)]
pub struct AgentRow {
    pub label: String,
    pub chip: String,
    pub ready: bool,
    pub advisor: bool,
}

/// A suggestion found in the idea text, as the chips under the composer show it.
pub struct HintChip {
    pub k: usize,
    pub name: &'static str,
    pub value: String,
    pub said: String,
    /// Used for the story (false: the person chose it explicitly, which wins).
    pub applied: bool,
    pub chosen: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Story,
    Library,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    None,
    Idea,
    Search,
}

pub enum Modal {
    ConfirmDelete(String, String),
    /// What to export: the page, the text, the audio. `Some(id)` exports a saved story from the library.
    Export(Picker, Option<String>),
    /// The model chooser (Ctrl+P or the Model chip).
    Model(TalesPick),
    /// The picture log (the button Picture log, or g).
    PicLog(PicLogView),
    /// A web link to a picture the person may approve for a scene (`scene`, address): nothing is fetched before they say yes.
    FetchLink(usize, String),
}

/// The picture log window: how far it is scrolled (in wrapped lines) and what the last drawing measured.
pub struct PicLogView {
    /// The first line shown; `usize::MAX` is "the end" (the latest events), made real when it is drawn.
    pub top: Cell<usize>,
    /// The view is at the latest events and stays there as new ones come.
    pub at_end: Cell<bool>,
    /// How many wrapped lines the log has and how many fit, as drawn last.
    pub total: Cell<usize>,
    pub rows: Cell<usize>,
    /// Clear log was pressed once and waits for a second press.
    pub confirm_clear: bool,
}

/// The model chooser of the tile: the chat's own list (see model_picker.rs) with "Same as chat" first.
pub struct TalesPick {
    pub list: ModelList,
    /// Choosing the picture model rather than the writing model.
    pub picture: bool,
}

/// The one choice of who writes: our model as the chat's, a provider's model, or an external agent with one of its models.
#[derive(Clone, Debug, PartialEq)]
pub enum Choice {
    SameAsChat,
    Provider(ModelChoice),
    /// The agent and its model (empty: the agent's own default).
    Agent(String, String),
}

pub enum GenMsg {
    Delta(String),
    /// The whole story so far (an external agent sends it that way).
    Snapshot(String),
    /// What an agent has thought so far (when it says).
    Thinking(String),
    /// Who is writing, and a line for the page (the agent was not usable, the fallback).
    Who(String),
    Note(String),
    Done(Result<String, String>),
}

pub struct Gen {
    pub rx: Receiver<GenMsg>,
    pub stop: Arc<AtomicBool>,
}

pub struct AudioJob {
    pub rx: Receiver<Result<String, String>>,
    pub cancel: Arc<AtomicBool>,
    pub done: Arc<AtomicUsize>,
    pub total: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum UKind {
    Hidden,
    Heading,
    Para,
    Partial,
}

/// One unit of the story wrapped to the current width; `lines` holds each line and where it starts in the unit (characters).
pub struct ULay {
    pub text: String,
    pub kind: UKind,
    pub lines: Vec<(String, usize)>,
}

/// What a frame of the story text is drawn with (the same for every line of the frame).
pub struct TextCtx {
    pub area: R,
    pub base_y: f32,
    pub size: f32,
    pub s: f32,
    /// The unit and character range being read, when something is.
    pub hl_range: Option<(usize, (usize, usize))>,
    pub cur_word: Option<(usize, usize)>,
    /// The sentence is lit (Settings → Voice).
    pub lit: bool,
    pub tick: u64,
}

/// Where the word being said sits on its line: characters `c0..c1` of the line at `off` (width `mw`), and the eased fill `ex`, `ew`.
pub struct WordAt {
    pub unit: usize,
    pub li: usize,
    pub c0: usize,
    pub c1: usize,
    pub off: f32,
    pub ex: f32,
    pub ew: f32,
}

/// What the last whole drawing of a story being read aloud left on the screen, so the frames after it can repaint just what
/// moves: the lines of the sentence and word being said, the player bar and the glow around the text card. Everything else on
/// the screen is the same until a whole frame is drawn again (anything unexpected asks for one).
pub struct Anim {
    /// The text area, the top of the first line (scrolled), the text size, the scale, the scroll and the top of every unit.
    pub text: R,
    pub base_y: f32,
    pub size: f32,
    pub s: f32,
    pub scroll: f32,
    pub tops: Vec<f32>,
    /// What was lit.
    pub unit: usize,
    pub range: (usize, usize),
    pub word: Option<(usize, usize)>,
    pub hl: Highlight,
    /// The pictures' slots (with their galleries) on screen: a line next to one is not repainted alone.
    pub slots: Vec<R>,
    /// The glow of the text card, and the strength it was drawn with.
    pub ring: Option<GlowRing>,
    pub ring_alpha: (u8, u8),
    /// The player bar, the rectangle around it and what was under it.
    pub player: R,
    pub bar: (i32, i32, i32, i32),
    pub bar_under: Vec<u8>,
    /// The caret of the idea box blinks: its phase when drawn (a change asks for a whole frame).
    pub caret: bool,
    pub blink: bool,
    /// What else is on the screen, summed up (see `StoryApp::view_sig`).
    pub sig: u64,
    /// Frames painted this way since the whole one.
    pub streak: u32,
}

/// The characters `a..b` of a unit that fall on one line (`first` is where the line starts in the unit, `line` its text): the
/// characters inside the line, and the widths of the text before them and of them.
pub fn span_in_line(g: &mut Gfx, line: &str, first: usize, a: usize, b: usize, sz: f32, face: Face) -> Option<(usize, usize, f32, f32)> {
    let n = line.chars().count();
    let (lo, hi) = (a.max(first), b.min(first + n));
    if lo >= hi {
        return None;
    }
    let pre: String = line.chars().take(lo - first).collect();
    let mid: String = line.chars().skip(lo - first).take(hi - lo).collect();
    Some((lo - first, hi - first, g.text_width(sz, &pre, face), g.text_width(sz, &mid, face)))
}

// ------------------------------------------------------------------------------------------------------------ pictures

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PicState {
    Queued,
    Working,
    Ready,
    Failed(String),
}

/// A revision of the story on the page that is being written: what to go back to if it fails, and the pictures to reuse.
pub struct Rev {
    pub old_story: Story,
    pub old_raw: String,
    pub old_opts: StoryOptions,
    pub old_pics: Vec<Pic>,
    /// The request named a picture style: every picture is drawn again.
    pub new_style: bool,
}

/// One illustration of the story on the page.
#[derive(Clone)]
pub struct Pic {
    pub scene: st::Scene,
    pub state: PicState,
    pub bytes: Option<Arc<Vec<u8>>>,
    pub ext: String,
    pub prompt: String,
    pub mode: PictureMode,
    pub style: PictureStyle,
    /// Changes whenever the picture does (the cache key).
    pub key: u64,
    /// The step in the conversation that is drawing it.
    pub call: Option<String>,
    /// How it was made (`image`, `rich svg`, `composed`, `model svg`, `keywords`), and what was asked and answered.
    pub how: String,
    pub log: String,
    /// What the person asked to change about this picture ("make the sky pink"), used when it is drawn again.
    pub feedback: String,
    /// `WxH` of a picture model's image, kept with the story.
    pub size: String,
    /// How the main picture was made: its style, seed, model, provider and time (ms).
    pub look: illus::Look,
    pub seed: Option<u64>,
    pub provider: String,
    pub model: String,
    pub ms: u64,
    /// The other pictures of this scene (other styles or seeds); the picture above is the one chosen as the main one.
    pub alts: Vec<Alt>,
    /// Web links to a picture that the writer's reply held and that were not fetched: the person may approve one (`Fetch`).
    pub links: Vec<String>,
}

/// One more picture of a scene.
#[derive(Clone)]
pub struct Alt {
    pub look: illus::Look,
    pub seed: Option<u64>,
    pub provider: String,
    pub model: String,
    pub size: String,
    pub ms: u64,
    pub ext: String,
    pub bytes: Arc<Vec<u8>>,
    pub prompt: String,
    /// The cache key of its thumbnail.
    pub key: u64,
}

/// A picture still to be drawn besides the main one of its scene: scene, style, and which picture of that style (from 0).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AltJob {
    pub idx: usize,
    pub look: illus::Look,
    pub n: u32,
}

/// One who can draw: a text backend, whether it is strong enough for a rich SVG, and its name.
pub struct Drawer {
    pub chat: Arc<dyn ChatBackend>,
    pub strong: bool,
    pub who: String,
}

/// A finished picture from the drawing thread.
pub struct PicOut {
    pub bytes: Vec<u8>,
    pub ext: String,
    pub prompt: String,
    pub how: String,
    /// Who made it (the model or agent, or the picture model).
    pub who: String,
    /// Said when the model's own drawing could not be used.
    pub note: Option<String>,
    pub log: String,
    /// The character sheet the thread made for this story, when there was none yet.
    pub sheet: Option<CharacterSheet>,
    /// The setting note the thread made for this story, when there was none yet.
    pub setting: Option<String>,
    pub size: String,
    /// How it was made (set for a picture model's image).
    pub made: Option<(illus::Look, Option<u64>, String, String, u64)>,
    /// Links to pictures that the reply held and nobody fetched.
    pub links: Vec<String>,
}

/// The picture model the server does not have: what to say and what to download.
pub struct MissingModel {
    pub provider: String,
    pub model: String,
    pub size: String,
    pub text: String,
}

/// The offer to download the picture model, and how far the download is.
pub struct Install {
    pub provider: String,
    pub model: String,
    pub size: String,
    pub started: bool,
}

pub struct PicMsg {
    pub gen: u64,
    /// Set when this was one of the extra pictures of a scene, not its main one.
    pub alt: Option<AltJob>,
    /// The picture model is not on the server: nothing was asked of it.
    pub missing: Option<MissingModel>,
    /// This is the answer to an approved link (`fetch_link`), not a drawing: a failure leaves the picture the scene has.
    pub linked: bool,
    pub idx: usize,
    pub res: Result<PicOut, String>,
    /// What the picture log learned in the thread (the request and its answer).
    pub events: Vec<PicEvent>,
}

/// The idea box: the shared multi-line input (grows from three lines, wraps, a caret and a selection, scrolls when long).
pub type IdeaBox = iyakki_uikit::multiline::MultiLine;

pub fn new_idea() -> IdeaBox {
    IdeaBox::new(st::MAX_TOPIC.max(st::MAX_REQUEST))
}

/// "2026-10-15" from seconds since 1970.
pub fn ymd(secs: u64) -> String {
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// What Save audio says with the browser voice.
pub const SAVE_AUDIO_PLAYONLY: &str = "Save audio is not available with this speech engine: it plays the speech itself and cannot write an audio file. Choose one that can (espeak-ng, piper, an endpoint) in Settings → Voice to save audio.";
pub const SAVE_AUDIO_BROWSER: &str = "Save audio is not available with the browser voice: the browser speaks the story itself and no audio file is made on this computer. Choose a speech engine (an endpoint or a local command such as piper) in Settings → Voice to save audio.";

/// A voice name for a chip: the empty name is the browser's own choice.
pub fn voice_label(v: &str) -> String {
    if v.is_empty() { "automatic".into() } else { v.to_string() }
}

pub fn path_env() -> String {
    std::env::var("PATH").unwrap_or_default()
}

pub fn voice_dirs(port: &dyn TalesPort) -> Vec<std::path::PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut v = vec![port.data_dir().join("voices"), "/usr/share/piper-voices".into(), "/usr/local/share/piper".into(), "/usr/share/piper".into(), "/opt/piper".into()];
    if !home.is_empty() {
        v.push(std::path::Path::new(&home).join(".local/share/piper"));
    }
    v
}

/// A pill chip of 28 px (before scaling); draws it and registers the click. Returns the width.
pub fn chip(g: &mut Gfx, x: f32, y: f32, s: f32, label: &str, on: bool, ev: Event) -> f32 {
    let sz = 13.0 * s;
    let w = g.text_width(sz, label, Face::Bold) + sz * 1.9;
    let h = 28.0 * s;
    let r = R::new(x, y, w, h);
    let hot = g.hovered(r);
    let fill = if on { fx::over(WELL, with_alpha(ACCENT, 56)) } else if hot { CARD_HOVER } else { WELL };
    g.rrect(x, y, w, h, h / 2.0, fill);
    g.rrect_outline(x, y, w, h, h / 2.0, if on { 1.5 * s } else { 1.0 }, if on { ACCENT } else if hot { with_alpha(ACCENT, 130) } else { rgb(50, 60, 92) });
    let tc = if on { fx::readable(ACCENT, fill, 4.5) } else { fx::readable(MUTED, fill, 4.5) };
    g.text(x + sz * 0.95, y + (h - g.line_height(sz, Face::Bold)) / 2.0, sz, tc, label, Face::Bold);
    g.hit(r, ev, label);
    w
}

pub fn chip_w(g: &mut Gfx, s: f32, label: &str) -> f32 {
    g.text_width(12.0 * s, label, Face::Bold) + 12.0 * s * 1.4
}

/// Chips in rows that wrap at `w`. With `draw` false nothing is drawn and no click is registered (measuring). Returns the height.
pub fn flow(g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, items: &[(String, bool, Event)], draw: bool) -> f32 {
    let (mut cx, mut cy) = (x, y);
    let h = 28.0 * s;
    let sz = 13.0 * s;
    for (label, on, ev) in items {
        let natural = g.text_width(sz, label, Face::Bold) + sz * 1.9;
        let cw = natural.min(w);
        if cx > x && cx + cw > x + w {
            cx = x;
            cy += h + 8.0 * s;
        }
        if draw {
            // a chip wider than the panel (a long agent name in a narrow window) shows the start of its text, never past the edge
            // (a chip that fits keeps its whole text: fitting it again to `cw - padding` loses to float rounding and ends it with "…")
            let shown = if natural <= w { label.clone() } else { g.fit(sz, label, Face::Bold, (cw - sz * 1.9).max(sz * 2.0)) };
            chip(g, cx, cy, s, &shown, *on, ev.clone());
            if shown != *label {
                // the chip shows the start of a long text, but its name (tooltips, the screen reader, tests) is the whole text
                if let Some(h) = g.hits.last_mut() {
                    h.label = label.clone();
                }
            }
        }
        cx += cw + 8.0 * s;
    }
    cy + h - y
}

pub fn section(g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, title: &str, draw: bool) -> f32 {
    if draw {
        fx::section_title(g, x, y, w, s, title, ACCENT);
    }
    22.0 * s
}

pub fn heart(g: &mut Gfx, cx: f32, cy: f32, size: f32, c: [u8; 4], filled: bool) {
    let r = size * 0.27;
    if filled {
        g.circle(cx - r, cy - r * 0.5, r, c);
        g.circle(cx + r, cy - r * 0.5, r, c);
        g.polygon(&[(cx - r * 1.95, cy - r * 0.2), (cx + r * 1.95, cy - r * 0.2), (cx, cy + size * 0.5)], c);
    } else {
        g.ring(cx - r, cy - r * 0.5, r, (size * 0.09).max(1.2), c);
        g.ring(cx + r, cy - r * 0.5, r, (size * 0.09).max(1.2), c);
        g.line(cx - r * 1.9, cy - r * 0.1, cx, cy + size * 0.5, (size * 0.09).max(1.2), c);
        g.line(cx + r * 1.9, cy - r * 0.1, cx, cy + size * 0.5, (size * 0.09).max(1.2), c);
    }
}

pub fn hash(s: &str) -> u64 {
    s.bytes().fold(1469598103934665603u64, |h, b| (h ^ b as u64).wrapping_mul(1099511628211))
}

/// The cover made by the shell: a gradient picked from the title, a moon, a few stars and the title in a band at the bottom.
/// Also the placeholder of a picture that is still being made.
pub fn cover(g: &mut Gfx, r: R, s: f32, title: &str) {
    const PAL: [([u8; 4], [u8; 4]); 6] = [
        (rgb(58, 40, 120), rgb(250, 140, 120)),
        (rgb(16, 70, 110), rgb(90, 220, 200)),
        (rgb(90, 30, 100), rgb(255, 120, 170)),
        (rgb(20, 40, 90), rgb(120, 150, 255)),
        (rgb(30, 80, 70), rgb(240, 210, 110)),
        (rgb(70, 30, 60), rgb(255, 170, 90)),
    ];
    let h = hash(title);
    let (a, b) = PAL[(h % PAL.len() as u64) as usize];
    let pad = (5.0 * s).max(3.0);
    let rad = (r.w.min(r.h) * 0.09).max(6.0);
    g.rrect(r.x - pad, r.y - pad, r.w + 2.0 * pad, r.h + 2.0 * pad, rad + pad, PANEL);
    g.rrect_outline(r.x - pad, r.y - pad, r.w + 2.0 * pad, r.h + 2.0 * pad, rad + pad, 1.2 * s, with_alpha(ACCENT, 120));
    g.gradient_rect(r.x, r.y, r.w, r.h, (r.x, r.y), (r.x + r.w, r.y + r.h), &[(0.0, a), (1.0, b)]);
    // the moon and some stars
    let (mx, my) = (r.x + r.w * (0.3 + 0.4 * ((h >> 8) % 100) as f32 / 100.0), r.y + r.h * 0.28);
    let u = r.w.min(r.h);
    g.radial_glow(mx, my, u * 0.34, with_alpha(rgb(255, 245, 210), 90));
    g.circle(mx, my, u * 0.09, rgb(255, 246, 214));
    g.circle(mx + u * 0.03, my - u * 0.015, u * 0.075, mix(a, b, 0.25));
    for k in 0..7u64 {
        let hk = hash(&format!("{title}{k}"));
        let (sx, sy) = (r.x + r.w * ((hk % 90) as f32 + 5.0) / 100.0, r.y + r.h * (((hk >> 8) % 55) as f32 + 4.0) / 100.0);
        g.circle(sx, sy, (u * 0.012).max(1.0), with_alpha(TEXT, 200));
    }
    // hills
    g.polygon(&[(r.x, r.y + r.h * 0.72), (r.x + r.w * 0.35, r.y + r.h * 0.6), (r.x + r.w * 0.7, r.y + r.h * 0.74), (r.x + r.w, r.y + r.h * 0.64), (r.x + r.w, r.y + r.h), (r.x, r.y + r.h)], with_alpha(rgb(10, 12, 28), 170));
    // the title
    if r.w >= 70.0 && r.h >= 50.0 {
        let sz = (u * 0.105).clamp(10.0, 20.0);
        let lines = g.wrap(sz, title, Face::Bold, r.w - 14.0);
        let lh = g.line_height(sz, Face::Bold);
        let n = lines.len().min(3);
        let top = r.y + r.h - 8.0 - n as f32 * lh;
        g.rrect(r.x + 4.0, top - 5.0, r.w - 8.0, n as f32 * lh + 10.0, 6.0, [6, 8, 20, 150]);
        for (i, l) in lines.iter().take(n).enumerate() {
            let l = if i + 1 == n && lines.len() > n { g.fit(sz, &format!("{l}…"), Face::Bold, r.w - 14.0) } else { l.clone() };
            let lw = g.text_width(sz, &l, Face::Bold);
            g.text(r.x + (r.w - lw) / 2.0, top + i as f32 * lh, sz, TEXT, &l, Face::Bold);
        }
    } else {
        let ch: String = title.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        let sz = u * 0.5;
        let w = g.text_width(sz, &ch, Face::Bold);
        g.text(r.x + (r.w - w) / 2.0, r.y + (r.h - g.line_height(sz, Face::Bold)) / 2.0, sz, TEXT, &ch, Face::Bold);
    }
    // round the corners again over the gradient: the piece of each corner outside a quarter circle takes the frame's colour
    for (cx, cy, sx, sy) in [(r.x, r.y, 1.0f32, 1.0f32), (r.x + r.w, r.y, -1.0, 1.0), (r.x + r.w, r.y + r.h, -1.0, -1.0), (r.x, r.y + r.h, 1.0, -1.0)] {
        let (ax, ay) = (cx + sx * rad, cy + sy * rad);
        let mut pts = vec![(cx, cy)];
        for k in 0..=8 {
            let t = k as f32 / 8.0 * std::f32::consts::FRAC_PI_2;
            pts.push((ax - sx * rad * t.cos(), ay - sy * rad * t.sin()));
        }
        g.polygon(&pts, PANEL);
    }
}

/// Options remembered by earlier versions: there is no field for the characters any more (they go in the idea) and the pictures are
/// automatic or off.
pub fn normalize_options(o: &mut StoryOptions) {
    o.characters = String::new();
    if o.pictures != PictureMode::Off {
        o.pictures = PictureMode::Auto;
    }
}

pub struct StoryApp {
    /// The core the tile was last given (to reword a failure when the drive is gone).
    /// Rewords a failure when the drive it names is gone.
    pub reword: Arc<dyn Fn(&str) -> String + Send + Sync>,
    pub page: Page,
    pub focus: Focus,
    pub opts: StoryOptions,
    pub idea: IdeaBox,
    pub search: TextField,
    pub store: StoryStore,
    pub prefs: st::Prefs,
    pub tts: TtsStatus,
    pub quick: Vec<st::QuickEngine>,
    pub voices: Vec<String>,
    pub takes_speed: bool,
    /// The engine that reads is the browser's (or window's) own voice: no audio is made here, so no audio can be saved.
    pub browser_voice: bool,
    /// The engine of this computer speaks by itself (spd-say, espeak, say, SAPI): the reader hands it the text of each piece.
    pub direct: Option<std::sync::Arc<dyn iyakki_story_model::DirectVoice>>,
    /// The engine that reads cannot write an audio file (the browser voice, spd-say), so no audio can be saved.
    pub no_audio_file: bool,
    /// Something to open in another app: Settings → Voice.
    pub request: Option<(String, String)>,
    /// The chip "Turn on reading aloud" was pressed and the engines of this computer are being looked for.
    pub turning_on: bool,
    pub image_models: Vec<(String, String)>,
    // the story on the page
    pub raw: String,
    pub story: Story,
    pub story_opts: StoryOptions,
    pub saved: Option<StoryEntry>,
    pub gen: Option<Gen>,
    pub error: Option<String>,
    pub note: Option<(String, bool)>,
    // pictures
    pub pics: Vec<Pic>,
    /// The character sheet of this story (made once, used by every picture, saved with the story).
    pub sheet: Option<CharacterSheet>,
    pub pic_gen: u64,
    /// The clock the tile stamps what it saves and logs with (seconds since 1970; a manual clock in tests).
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    /// Picture log events not yet in the story's folder (the story is not saved yet).
    pub pic_pending: Vec<PicEvent>,
    /// The extra pictures still to be drawn (scene by scene, style by style), the one being drawn, and each scene's seed.
    pub alt_jobs: std::collections::VecDeque<AltJob>,
    pub alt_running: Option<usize>,
    pub scene_seed: HashMap<usize, u64>,
    /// A job estimated to take longer than this many seconds asks first (tests change it).
    pub confirm_secs: f64,
    /// The gallery of the scenes as last drawn: where each thumbnail is and what a click on it does.
    pub gal_hits: Vec<(R, usize, String)>,
    /// The picture model that is not downloaded, with the Install button.
    pub install: Option<Install>,
    /// When each picture being drawn was asked for, and from which provider and model.
    pub pic_started: HashMap<usize, Instant>,
    pub pic_target: Option<(String, String)>,
    pub health_tx: Sender<Result<(String, Vec<String>), String>>,
    pub health_rx: Receiver<Result<(String, Vec<String>), String>>,
    pub health_busy: bool,
    pub health_last: Option<Instant>,
    /// What the server said about itself while a picture is being drawn.
    pub server_state: String,
    pub last_wait_log: Option<Instant>,
    /// The line about the picture being drawn now (time so far, server state); shown when there is no other notice.
    pub wait_text: Option<String>,
    /// Tests shorten the time limit and the polling and logging intervals.
    pub test_timing: Option<(Duration, Duration, Duration)>,
    /// How many times each picture was asked for (`attempt` in the log).
    pub pic_attempts: HashMap<usize, u32>,
    pub pic_tx: Sender<PicMsg>,
    pub pic_rx: Receiver<PicMsg>,
    pub pic_cache: PicCache,
    // reading
    pub reader: Reader,
    pub chunks: Vec<Chunk>,
    pub chunked_units: usize,
    pub live: bool,
    pub saved_audio: Option<Arc<st::PrebuiltSynth>>,
    pub level: f32,
    pub audio_job: Option<AudioJob>,
    // scrolling and selection
    pub cscroll: f32,
    pub cmax: f32,
    pub tscroll: f32,
    pub tmax: f32,
    pub manual_unit: Option<usize>,
    pub stick_bottom: bool,
    pub pane: Pane,
    /// What is lit while reading (Settings → Voice), as read at the last frame.
    pub highlight: Highlight,
    /// The text area as last drawn, and for each line given to the pane its unit and first character: turns a click into a place.
    pub text_area: R,
    pub line_map: Vec<(usize, usize)>,
    /// A plain click in the text, waiting for its release, then out the double-click time: it starts reading from there.
    pub click_read: Option<(f32, f32)>,
    pub pending_read: Option<(f32, f32, u32)>,
    /// The word highlight as drawn: (unit, line, x, width), eased toward the word being said.
    pub word_vis: Option<(usize, usize, f32, f32)>,
    /// Where the sentence and the word being said were drawn last (screen pixels), and the top of every line given to the pane.
    pub word_rect: Option<R>,
    pub sentence_rects: Vec<R>,
    pub line_tops: Vec<f32>,
    pub lay: Vec<ULay>,
    pub lay_key: (i32, i32),
    /// What the last whole frame left for the partial ones while the story is read aloud (see [`Anim`]).
    pub anim: Option<Anim>,
    /// The rectangles the last partial frame painted.
    pub live_out: Vec<(i32, i32, i32, i32)>,
    /// The shell's heartbeat count minus this app's, to know the phase of the caret between whole frames.
    pub tick_off: u64,
    /// Pictures scaled to the size they are drawn at: (picture key, slot width, slot height, scale in thousandths, pixels). A few are kept.
    pub pic_scaled: Vec<(u64, i32, i32, i32, tiny_skia::Pixmap)>,
    // the composer
    pub pane_open: bool,
    pub menu: Option<Menu>,
    pub menu_sel: usize,
    pub panel_hover: bool,
    pub anchors: [R; 7],
    /// The person chose length, language, kind or tone themselves (a remembered choice counts): the text never overrides it.
    pub explicit: [bool; 4],
    /// The words of a suggestion the person removed with its chip.
    pub dismissed: [Option<String>; 5],
    /// Auto picked image models and the first one failed: the rest are drawn as SVG (said once).
    pub pic_auto: bool,
    /// What the page says about an agent that could not write (kept beside the saved note).
    pub agent_note: Option<String>,
    // the conversation beside the story
    pub convo: Vec<Message>,
    pub view: ChatView,
    pub tool_ok: HashMap<String, bool>,
    pub scroll_px: f32,
    pub max_off: f32,
    pub thinking: String,
    pub think_since: Option<Instant>,
    pub step_seq: usize,
    /// The version in use and how many there are (1 when the story has not been revised).
    pub ver: u32,
    pub ver_count: u32,
    /// A correction sent while the story was being written: applied when the writing has stopped.
    pub queued: Option<String>,
    /// A revision under way: what to go back to, and what to reuse.
    pub rev: Option<Rev>,
    /// The step that is writing the story.
    pub write_call: Option<ToolCall>,
    /// Width of the right-hand panel (unscaled), set by dragging its divider.
    pub panel_w: f32,
    pub drag_o: Option<f32>,
    pub div_rect: R,
    pub area_rect: R,
    pub scale: f32,
    pub wide: bool,
    // library
    pub lib: Vec<StoryEntry>,
    pub lib_sel: usize,
    pub lib_top: usize,
    pub modal: Option<Modal>,
    pub host_save: Option<(String, Vec<u8>)>,
    pub ticks: u64,
    /// The word that was lit at the last heartbeat, to repaint the moment it changes instead of at the slow cadence.
    pub seen_word: Option<(usize, usize)>,
    /// The word last looked at between heartbeats (see `App::poll_redraw_due`).
    pub polled_word: Option<(usize, usize)>,
    /// (heartbeat, whether a frame is due) so both redraw questions of one heartbeat get the same answer.
    pub frame_due_at: (u64, bool),
    pub dirty: bool,
    /// Where the form's fields were drawn last (relative to the content top), to scroll them into view.
    pub bar_rect: Option<R>,
    /// The buttons on the pictures (Draw again, Try again), drawn after the text area is registered so they are on top of it.
    pub pic_buttons: Vec<(R, &'static str, usize, [u8; 4])>,
    /// The pictures are made but waiting for the person's yes (Pictures: ask me each time).
    pub held: bool,
    /// The one line about where and when the story happens, kept so the pictures look alike.
    pub setting: Option<String>,
    /// Tests: the machine's memory instead of the real one.
    #[cfg(any(test, feature = "testing"))]
    pub test_memory: Option<iyakki_sysinfo_model::Memory>,
    pub idea_geo: Option<(f32, f32)>,
    pub last_status: Status,
    /// A story was just asked for and may start reading by itself once there is something to read.
    pub auto_pending: bool,
    /// The name of the model that wrote (or is writing) the story on the page, for the header and status line.
    pub run_model: String,
    /// Tests: a backend per chosen model, instead of the real provider.
    #[cfg(any(test, feature = "testing"))]
    pub test_backend: Option<Arc<dyn Fn(&ModelChoice) -> Arc<dyn ChatBackend> + Send + Sync>>,
    /// Tests: change who draws.
    #[cfg(any(test, feature = "testing"))]
    pub test_drawers: Option<Arc<dyn Fn(Vec<Drawer>) -> Vec<Drawer> + Send + Sync>>,
}

/// The browser voice as the reader's synthesiser and sink (the page speaks; nothing is made here).
pub fn browser_synth() -> Arc<dyn st::Synth> {
    Arc::new(iyakki_voice_model::hostvoice::BrowserSynth)
}

pub fn browser_sink(hv: Arc<HostVoice>, voice: Option<String>, lang: Option<String>, rate: f32) -> Arc<dyn st::Sink> {
    Arc::new(iyakki_voice_model::hostvoice::BrowserSink { hv, voice, lang, rate })
}


mod state;
mod gallery;
mod hints;
mod input;
mod panels;
mod pictures;
mod reading;
mod view;
mod write;

pub type AgentRun = (AgentRunReq, String, String, bool);

impl StoryApp {
    /// A frame is due at this heartbeat while reading: every third one (the level bars and the glow, about 10 a second), and at
    /// once at any heartbeat when the lit word changed since the last one, so a word is on screen within one heartbeat (33 ms) of
    /// the clock reaching it, not within three (100 ms: at fast speech a word is only 150 ms long).
    pub fn reading_frame_due(&mut self) -> bool {
        let word = if matches!(self.reader.status(), Status::Playing) { self.reader.snapshot().word } else { None };
        if self.frame_due_at.0 == self.ticks && word == self.seen_word {
            return self.frame_due_at.1;
        }
        let new_tick = self.frame_due_at.0 != self.ticks;
        let due = (new_tick && self.ticks % 3 == 0) || word != self.seen_word;
        self.seen_word = word;
        self.frame_due_at = (self.ticks, due);
        due
    }
}

impl StoryApp {
    pub fn keeps_screen_on(&self) -> bool {
        matches!(self.reader.status(), Status::Playing | Status::Buffering)
    }
    pub fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }
    pub fn title(&self) -> &'static str {
        "Tales"
    }
    pub fn icon(&self) -> Icon {
        Icon::Tales
    }
    pub fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    pub fn hint(&self) -> String {
        match self.page {
            Page::Story => "Enter send · Ctrl+J new line · Ctrl+P model or agent · Ctrl+O options · Space play · Ctrl+S save · e export · d PDF · l saved stories · Esc back".into(),
            Page::Library => "↑↓ choose · Enter open · Space listen · f favourite · d delete · / search".into(),
        }
    }
    pub fn help(&self) -> Vec<String> {
        [
            "The story is above, the idea box and its controls below (on a small window, press o or click Idea & options to show them)",
            "Idea box: type and press Enter or Send to write the story; Ctrl+J for a new line. Say it in words too: a 5-minute funny story in Tamil about a fox (what was understood shows as chips you can remove)",
            "Model ▾ or Ctrl+P (Ctrl+G too): the one chooser of who writes, as in the chat: same as chat (default), a model of a provider, or an external agent (Claude Code, GitHub Copilot) with one of its models; an agent gets no tools. Kind and Tone are drop-downs; Options ▾ (Ctrl+O) has length, language, characters, pictures, reading and saving",
            "Pictures: Auto (images when a picture model is set up, else SVG drawings), Off, Drawings (SVG) or Image model; one per scene, drawn while you read; Draw again redoes one",
            "Space or p · play and pause      s · stop      r · replay      ← → or [ ] · the previous and next part      click the bar · jump",
            "v · next voice      + − · faster, slower      w · save the audio      Ctrl+S · save the story into Stories/<title>/",
            "e · export to the computer: a page with the pictures (.html), the text (.md) or the audio",
            "f · favourite      c or Ctrl+C · copy (the selection, or the whole story)      n · a new story      l · saved stories",
            "Up Down PageUp PageDown · scroll the text (with Options open: the options)",
            "Listen again: ↑↓ choose · Enter open · Space listen · f favourite · d delete · e export · / search",
            "Esc · leave a field, then back to the apps (reading stops)",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    pub fn handle_with(&mut self, ev: &Event, port: &dyn TalesPort) -> Flow {
        if matches!(ev, Event::Tick) {
            self.ticks += 1;
            self.poll(port);
            // a plain click in the text that waited out the double-click time starts reading from there
            if let Some((x, y, n)) = self.pending_read {
                if n > 1 {
                    self.pending_read = Some((x, y, n - 1));
                } else {
                    self.pending_read = None;
                    self.read_from_point(x, y, port);
                }
            }
            return Flow::Stay;
        }
        self.dirty = true;
        if self.page == Page::Story && self.modal.is_none() {
            match ev {
                Event::AreaAt(TEXT_PANE, x, y) => {
                    self.pending_read = None;
                    self.click_read = (!iyakki_uikit::ui::dragging() && iyakki_uikit::ui::click_count() == 1 && !iyakki_uikit::ui::shift_held()).then_some((*x, *y));
                }
                Event::MouseUp(..) => {
                    if let Some((x, y)) = self.click_read.take() {
                        if !self.pane.has_selection() && self.can_read() && !self.chunks.is_empty() {
                            self.pending_read = Some((x, y, READ_CLICK_DELAY));
                        }
                    }
                }
                _ => {}
            }
        }
        if matches!(self.modal, Some(Modal::ConfirmDelete(..) | Modal::FetchLink(..))) && !matches!(ev, Event::Key(_)) {
            return Flow::Stay;
        }
        if matches!(self.modal, Some(Modal::Model(_))) && !matches!(ev, Event::Key(_)) {
            self.model_event(ev, port);
            return Flow::Stay;
        }
        if let Some(Modal::Export(p, id)) = self.modal.as_mut() {
            if !matches!(ev, Event::Key(_)) {
                let id = id.clone();
                match p.handle_event(ev) {
                    PickEvent::Chosen(label) => {
                        self.modal = None;
                        let what = if label.starts_with("Page with every") { 3 } else if label.starts_with("Page") { 0 } else if label.starts_with("Text") { 1 } else { 2 };
                        self.do_export(what, id);
                    }
                    PickEvent::Cancel => self.modal = None,
                    PickEvent::None => {}
                }
                return Flow::Stay;
            }
        }
        if self.page == Page::Story && self.modal.is_none() && self.pane.handle(ev) {
            // a press in the text takes the keyboard away from any field
            if matches!(ev, Event::AreaAt(..)) {
                self.focus = Focus::None;
            }
            return Flow::Stay;
        }
        match ev {
            Event::AreaAt(DIV_AREA, x, _) => {
                // the divider between the story and the panel: the pointer is where its centre should sit
                if !iyakki_uikit::ui::dragging() {
                    self.drag_o = Some(self.div_rect.x);
                }
                let (area, s) = (self.area_rect, self.scale);
                let pos = self.drag_o.unwrap_or(self.div_rect.x) + *x;
                self.panel_w = (((area.x + area.w - pos) / s).clamp_safe(270.0, (area.w / s * 0.6).max(270.0))).round();
            }
            Event::Tab(n) if *n >= TOGGLE_BASE => self.view.toggle_index(*n - TOGGLE_BASE),
            Event::AreaAt(IDEA_AREA, x, y) => {
                self.set_focus(Focus::Idea);
                self.idea.press(*x, *y);
            }
            Event::FocusAt(id, x) => {
                match *id {
                    SEARCH_FIELD => {
                        self.set_focus(Focus::Search);
                        self.search.click(*x);
                    }
                    _ => {}
                }
            }
            Event::Paste(t) => {
                let was = self.focus;
                match self.focus {
                    Focus::Idea => {
                        self.idea.insert(t);
                    }
                    Focus::Search => {
                        self.search.paste(t);
                    }
                    Focus::None => {}
                }
                self.sync_fields();
                if was == Focus::Search {
                    self.reload_library();
                }
            }
            Event::Scroll(d) if matches!(self.modal, Some(Modal::PicLog(_))) => self.log_scroll(-*d as i64 * 3),
            Event::Scroll(d) if self.idea.wheel(*d) => {}
            Event::Scroll(d) => self.scroll_by(-*d as f32 * 48.0),
            Event::Row(i) => {
                self.lib_sel = (*i).min(self.lib.len().saturating_sub(1));
                self.focus = if self.focus == Focus::Search { Focus::None } else { self.focus };
            }
            Event::Tab(n) => {
                // a click on a button takes the keyboard from a field (an idea chip puts it back in the idea box)
                let keeps = (T_SUGGEST..T_SUGGEST + SUGGESTIONS.len()).contains(n) || (T_HINT..T_HINT + 5).contains(n) || matches!(*n, T_OPTIONS_MENU | T_KIND_MENU | T_TONE_MENU | T_LANG_MENU | T_VERSION_MENU | T_HOW | T_MENU_CLOSE);
                if self.focus != Focus::None && !keeps {
                    self.focus = Focus::None;
                }
                self.click(*n, port);
            }
            Event::AreaAt(BAR_AREA, x, _) => {
                if let (Some(bar), true) = (self.bar_rect, !self.chunks.is_empty()) {
                    let f = (*x / bar.w.max(1.0)).clamp(0.0, 1.0);
                    let idx = (f * self.chunks.len() as f32) as usize;
                    self.seek(idx, port);
                }
            }
            Event::Key(k) => return self.key(k, port),
            _ => {}
        }
        Flow::Stay
    }

    pub fn wants_redraw(&mut self) -> bool {
        if self.animating() {
            self.reading_frame_due()
        } else {
            std::mem::take(&mut self.dirty)
        }
    }

    pub fn idle_redraw_ms(&self) -> u64 {
        // nothing is read or made: only a caret blinks (every 500 ms) and the clock moves
        if self.animating() {
            100
        } else {
            250
        }
    }

    pub fn wants_fast_poll(&self) -> bool {
        self.highlight.word() && matches!(self.reader.status(), Status::Playing)
    }

    pub fn poll_redraw_due(&mut self) -> bool {
        let word = self.reader.snapshot().word;
        let changed = word != self.polled_word;
        self.polled_word = word;
        changed
    }

    pub fn redraw_kind(&mut self) -> Redraw {
        if !self.animating() {
            return if std::mem::take(&mut self.dirty) { Redraw::Full } else { Redraw::None };
        }
        if !self.reading_frame_due() {
            return Redraw::None;
        }
        // reading aloud with nothing else going on: only the lit words, the glow and the player bar move
        if !std::mem::take(&mut self.dirty) && self.anim.is_some() && self.partial_possible() {
            return Redraw::Live;
        }
        Redraw::Full
    }

    pub fn live_draw(&mut self, g: &mut Gfx) -> Option<(i32, i32, i32, i32)> {
        let rects = self.live_animate(g)?;
        let (x0, y0) = (rects.iter().map(|r| r.0).min()?, rects.iter().map(|r| r.1).min()?);
        let (x1, y1) = (rects.iter().map(|r| r.0 + r.2).max()?, rects.iter().map(|r| r.1 + r.3).max()?);
        self.live_out = rects;
        Some((x0, y0, x1 - x0, y1 - y0))
    }

    pub fn live_rects(&self) -> Vec<(i32, i32, i32, i32)> {
        self.live_out.clone()
    }

    pub fn live_full_every_ms(&self) -> u64 {
        // the shell still draws the whole screen now and then, as a safety net
        1500
    }

    pub fn take_host_save(&mut self) -> Option<(String, Vec<u8>)> {
        self.host_save.take()
    }

    pub fn context_items(&mut self, x: i32, y: i32) -> Vec<iyakki_uikit::ctxmenu::Item> {
        if self.page != Page::Story {
            return Vec::new();
        }
        self.pane.context_items(x, y)
    }

    pub fn render_with(&mut self, g: &mut Gfx, area: R, s: f32, port: &dyn TalesPort, tick: u64) {
        match self.page {
            Page::Story => self.story_screen(g, area, s, port, tick),
            Page::Library => self.library_page(g, area, s, tick),
        }
    }
}

#[cfg(test)]
mod fake_tests;
