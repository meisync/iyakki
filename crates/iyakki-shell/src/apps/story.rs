//! TALES: describe a story, choose who it is for, and the model writes it, draws it and reads it aloud. A thin screen over
//! `iyakki_agent::story` (prompt, parsing, pictures, reading pipeline, library): this file only draws and routes keys and clicks.
//!
//! The **story view** fills the left: a cover the shell draws, the story with its pictures in place and the sentence being
//! read lit, and a player bar whose bars move with the real sound. The **form** is a pane on the right (the idea in a
//! box of several lines, who it is for, the kind, length, language, tone, characters, pictures and their style, the voice) with
//! a Create story button that is always in view; on a small screen the pane is a toggle. **Listen again** lists the saved
//! stories. The model gets no tools. Speech comes from Settings → Voice; pictures are small SVG drawings the text model
//! writes (checked and rebuilt from safe parts only) or come from a picture model. With neither, the story is still
//! written, shown and saved.

use crate::safeclamp::SafeClamp;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::cell::Cell;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use iyakki_agent::chat::{ChatBackend, Message, ToolCall};
use iyakki_agent::settings::ModelChoice;
use iyakki_agent::hostengines;
use iyakki_agent::hostvoice::{self, HostVoice};
use iyakki_agent::voice::{Highlight, SpeechEngine};
use iyakki_agent::scene::{self, CharacterSheet};
use iyakki_agent::story::{self as st, Hints, ModelPlan, Audience, Block, Chunk, Genre, Length, PictureMode, PictureStyle, Reader, Status, Story, StoryEntry, StoryOptions, StoryStore, StoredPic, StoredVariant, Tone, TtsStatus, LANGUAGES};
use iyakki_agent::piclog::{self, kind as plk, PicEvent};
use iyakki_agent::storyillus::{self as illus, Coverage, Readiness};
use iyakki_agent::voice::SpeakOpts;

use super::chat_view::{self, ChatView, NoPics, TOGGLE_BASE};
use super::chatpic::PicCache;
use super::model_picker::{self, Current, ModelList, Pick, TopRow};
use super::neon::{glow_card, round_button, Glyph};
use super::widgets::{menu_list, select_box, select_width, PickEvent, Picker};
use super::{blink, App, AppCx, Flow};
use crate::fx::{self, mix, with_alpha, CYAN, MAGENTA, PANEL, WELL};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::textsel::Pane;
use crate::ui::{Event, Key, TextField, CARD_HOVER, DIM, ERR, MUTED, OK, R, TEXT};

/// The tile's colour: lamplight gold.
pub const ACCENT: [u8; 4] = rgb(255, 220, 130);

const IDEA_AREA: usize = 63_410;
const SEARCH_FIELD: usize = 63_412;
const TEXT_PANE: usize = 63_420;
const BAR_AREA: usize = 63_421;

/// The word being said: a solid fill of the tile's colour with dark text on it (the sentence around it is a soft wash).
const WORD_FILL: [u8; 4] = [255, 220, 130, 240];
const WORD_INK: [u8; 4] = [22, 18, 6, 255];
/// Heartbeats a plain click in the story waits for a second click (a double click selects a word) before it starts reading.
const READ_CLICK_DELAY: u32 = 14;

// click codes (Event::Tab)
const T_OPTIONS: usize = 1;
const T_VERSION_MENU: usize = 4;
const T_LANG_MENU: usize = 5;
const T_KIND_MENU: usize = 6;
const T_TONE_MENU: usize = 7;
const T_OPTIONS_MENU: usize = 8;
const T_HINT: usize = 15; // + 0 length, 1 language, 2 kind, 3 tone, 4 picture style
const T_HOW: usize = 130;
const T_TURN_ON: usize = 132; // the chip that sets reading aloud up
const T_MENU_CLOSE: usize = 131;
const T_VERSION_PICK: usize = 500; // + version number (1 is the first)
const DIV_AREA: usize = 63_430;
const T_STORY_PAGE: usize = 2;
const T_LIB_PAGE: usize = 3;
const T_AUDIENCE: usize = 10; // + index
const T_GENRE: usize = 20;
const T_LENGTH: usize = 30;
const T_LANG: usize = 40;
const T_MODEL: usize = 120;
const T_PICMODEL: usize = 121;
const T_PICMODE: usize = 140; // + index
const T_DRAWMODE: usize = 141;
const T_VIDEO: usize = 142;
const T_TONE: usize = 60;
const T_READ_WHILE: usize = 70;
const T_AUTOSAVE: usize = 71;
const T_CREATE: usize = 80;
const T_QUICK: usize = 90; // + index
const T_SUGGEST: usize = 100; // + index
const T_PLAY: usize = 200;
const T_STOP: usize = 201;
const T_PREV: usize = 202;
const T_NEXT: usize = 203;
const T_REPLAY: usize = 204;
const T_VOICE: usize = 205;
const T_SPEED: usize = 206;
const T_SAVE_AUDIO: usize = 207;
const T_FAV: usize = 208;
const T_COPY: usize = 209;
const T_SAVE: usize = 210;
const T_EXPORT: usize = 211;
const T_NEW: usize = 212;
const T_STOP_WRITING: usize = 213;
const T_CANCEL_AUDIO: usize = 214;
const T_LIB_OPEN: usize = 300;
const T_LIB_LISTEN: usize = 301;
const T_LIB_FAV: usize = 302;
const T_LIB_DELETE: usize = 303;
const T_LIB_EXPORT: usize = 304;
const T_PIC_REDO: usize = 400; // + picture index
const T_PIC_DELETE: usize = 420; // + picture index
const T_COVERAGE: usize = 143;
const T_PICS_GO: usize = 215;
const T_PICS_SKIP: usize = 216;
const T_PICS_CANCEL: usize = 217;
const T_LOOK: usize = 144;
const T_QUALITY: usize = 145;
const T_STORYMODEL: usize = 146;
const T_KEEPLOOK: usize = 147;
const T_PICLOG: usize = 148;
const T_LOG_COPY: usize = 149;
const T_LOG_CLEAR: usize = 150;
const T_LOG_VERBOSE: usize = 151;
const T_LOG_CLOSE: usize = 152;
const T_PIC_INSTALL: usize = 153;
const T_TIMEOUT: usize = 154;
const T_PERSTYLE: usize = 155;
const T_REALISTIC: usize = 156;
const T_REAL_INSTALL: usize = 157;
const T_KEEP_ONLY: usize = 158;
const T_STYLE: usize = 160; // + index of the style
const T_CHOICE_INSTALL: usize = 170; // + index of the picture model in the list
const T_ALT_PICK: usize = 700; // + scene * 24 + index of the extra picture
const T_ALT_DEL: usize = 860; // + scene * 24 + index
const T_ALT_MORE: usize = 1010; // + scene

const SUGGESTIONS: [&str; 6] = ["A little fox who is afraid of the dark", "The moon lost its silver mitten", "A robot who learns to laugh", "The kindest dragon in the valley", "A rainy day treasure hunt", "A tiny seed that wanted to touch the sky"];
const SPEEDS: [f32; 6] = [0.8, 0.9, 1.0, 1.1, 1.25, 1.5];
/// Below this window width the form is a toggle instead of a pane beside the story.
const WIDE_PX: f32 = 900.0;

// what the idea text can suggest (indexes into the explicit and dismissed lists)
const H_LEN: usize = 0;
const H_LANG: usize = 1;
const H_GENRE: usize = 2;
const H_TONE: usize = 3;
const H_STYLE: usize = 4;

/// The one open drop-down, popover or panel of the composer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
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
struct AgentRow {
    label: String,
    chip: String,
    ready: bool,
    advisor: bool,
}

/// A suggestion found in the idea text, as the chips under the composer show it.
struct HintChip {
    k: usize,
    name: &'static str,
    value: String,
    said: String,
    /// Used for the story (false: the person chose it explicitly, which wins).
    applied: bool,
    chosen: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Story,
    Library,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Focus {
    None,
    Idea,
    Search,
}

enum Modal {
    ConfirmDelete(String, String),
    /// What to export: the page, the text, the audio. `Some(id)` exports a saved story from the library.
    Export(Picker, Option<String>),
    /// The model chooser (Ctrl+P or the Model chip).
    Model(TalesPick),
    /// The picture log (the button Picture log, or g).
    PicLog(PicLogView),
}

/// The picture log window: how far it is scrolled (in wrapped lines) and what the last drawing measured.
struct PicLogView {
    /// The first line shown; `usize::MAX` is "the end" (the latest events), made real when it is drawn.
    top: Cell<usize>,
    /// The view is at the latest events and stays there as new ones come.
    at_end: Cell<bool>,
    /// How many wrapped lines the log has and how many fit, as drawn last.
    total: Cell<usize>,
    rows: Cell<usize>,
    /// Clear log was pressed once and waits for a second press.
    confirm_clear: bool,
}

/// The model chooser of the tile: the chat's own list (see model_picker.rs) with "Same as chat" first.
struct TalesPick {
    list: ModelList,
    /// Choosing the picture model rather than the writing model.
    picture: bool,
}

/// The one choice of who writes: our model as the chat's, a provider's model, or an external agent with one of its models.
#[derive(Clone, Debug, PartialEq)]
enum Choice {
    SameAsChat,
    Provider(ModelChoice),
    /// The agent and its model (empty: the agent's own default).
    Agent(String, String),
}

enum GenMsg {
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

struct Gen {
    rx: Receiver<GenMsg>,
    stop: Arc<AtomicBool>,
}

struct AudioJob {
    rx: Receiver<Result<String, String>>,
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    total: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum UKind {
    Hidden,
    Heading,
    Para,
    Partial,
}

/// One unit of the story wrapped to the current width; `lines` holds each line and where it starts in the unit (characters).
struct ULay {
    text: String,
    kind: UKind,
    lines: Vec<(String, usize)>,
}

// ------------------------------------------------------------------------------------------------------------ pictures

#[derive(Clone, Debug, PartialEq, Eq)]
enum PicState {
    Queued,
    Working,
    Ready,
    Failed(String),
}

/// A revision of the story on the page that is being written: what to go back to if it fails, and the pictures to reuse.
struct Rev {
    old_story: Story,
    old_raw: String,
    old_opts: StoryOptions,
    old_pics: Vec<Pic>,
    /// The request named a picture style: every picture is drawn again.
    new_style: bool,
}

/// One illustration of the story on the page.
#[derive(Clone)]
struct Pic {
    scene: st::Scene,
    state: PicState,
    bytes: Option<Arc<Vec<u8>>>,
    ext: String,
    prompt: String,
    mode: PictureMode,
    style: PictureStyle,
    /// Changes whenever the picture does (the cache key).
    key: u64,
    /// The step in the conversation that is drawing it.
    call: Option<String>,
    /// How it was made (`image`, `rich svg`, `composed`, `model svg`, `keywords`), and what was asked and answered.
    how: String,
    log: String,
    /// What the person asked to change about this picture ("make the sky pink"), used when it is drawn again.
    feedback: String,
    /// `WxH` of a picture model's image, kept with the story.
    size: String,
    /// How the main picture was made: its style, seed, model, provider and time (ms).
    look: illus::Look,
    seed: Option<u64>,
    provider: String,
    model: String,
    ms: u64,
    /// The other pictures of this scene (other styles or seeds); the picture above is the one chosen as the main one.
    alts: Vec<Alt>,
}

/// One more picture of a scene.
#[derive(Clone)]
struct Alt {
    look: illus::Look,
    seed: Option<u64>,
    provider: String,
    model: String,
    size: String,
    ms: u64,
    ext: String,
    bytes: Arc<Vec<u8>>,
    prompt: String,
    /// The cache key of its thumbnail.
    key: u64,
}

/// A picture still to be drawn besides the main one of its scene: scene, style, and which picture of that style (from 0).
#[derive(Clone, Copy, Debug, PartialEq)]
struct AltJob {
    idx: usize,
    look: illus::Look,
    n: u32,
}

/// One who can draw: a text backend, whether it is strong enough for a rich SVG, and its name.
struct Drawer {
    chat: Arc<dyn ChatBackend>,
    strong: bool,
    who: String,
}

/// A finished picture from the drawing thread.
struct PicOut {
    bytes: Vec<u8>,
    ext: String,
    prompt: String,
    how: String,
    /// Who made it (the model or agent, or the picture model).
    who: String,
    /// Said when the model's own drawing could not be used.
    note: Option<String>,
    log: String,
    /// The character sheet the thread made for this story, when there was none yet.
    sheet: Option<CharacterSheet>,
    /// The setting note the thread made for this story, when there was none yet.
    setting: Option<String>,
    size: String,
    /// How it was made (set for a picture model's image).
    made: Option<(illus::Look, Option<u64>, String, String, u64)>,
}

/// The picture model the server does not have: what to say and what to download.
struct MissingModel {
    provider: String,
    model: String,
    size: String,
    text: String,
}

/// The offer to download the picture model, and how far the download is.
struct Install {
    provider: String,
    model: String,
    size: String,
    started: bool,
}

struct PicMsg {
    gen: u64,
    /// Set when this was one of the extra pictures of a scene, not its main one.
    alt: Option<AltJob>,
    /// The picture model is not on the server: nothing was asked of it.
    missing: Option<MissingModel>,
    idx: usize,
    res: Result<PicOut, String>,
    /// What the picture log learned in the thread (the request and its answer).
    events: Vec<PicEvent>,
}

/// The idea box: the shared multi-line input (grows from three lines, wraps, a caret and a selection, scrolls when long).
type IdeaBox = super::multiline::MultiLine;

fn new_idea() -> IdeaBox {
    IdeaBox::new(st::MAX_TOPIC.max(st::MAX_REQUEST))
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// "2026-10-15" from seconds since 1970.
fn ymd(secs: u64) -> String {
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
const SAVE_AUDIO_PLAYONLY: &str = "Save audio is not available with this speech engine: it plays the speech itself and cannot write an audio file. Choose one that can (espeak-ng, piper, an endpoint) in Settings → Voice to save audio.";
const SAVE_AUDIO_BROWSER: &str = "Save audio is not available with the browser voice: the browser speaks the story itself and no audio file is made on this computer. Choose a speech engine (an endpoint or a local command such as piper) in Settings → Voice to save audio.";

/// A voice name for a chip: the empty name is the browser's own choice.
fn voice_label(v: &str) -> String {
    if v.is_empty() { "automatic".into() } else { v.to_string() }
}

fn path_env() -> String {
    std::env::var("PATH").unwrap_or_default()
}

fn voice_dirs(cx: &AppCx) -> Vec<std::path::PathBuf> {
    let home = std::env::var("HOME").unwrap_or_default();
    let mut v = vec![cx.services.dir().join("voices"), "/usr/share/piper-voices".into(), "/usr/local/share/piper".into(), "/usr/share/piper".into(), "/opt/piper".into()];
    if !home.is_empty() {
        v.push(std::path::Path::new(&home).join(".local/share/piper"));
    }
    v
}

/// A pill chip of 28 px (before scaling); draws it and registers the click. Returns the width.
fn chip(g: &mut Gfx, x: f32, y: f32, s: f32, label: &str, on: bool, ev: Event) -> f32 {
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

fn chip_w(g: &mut Gfx, s: f32, label: &str) -> f32 {
    g.text_width(12.0 * s, label, Face::Bold) + 12.0 * s * 1.4
}

/// Chips in rows that wrap at `w`. With `draw` false nothing is drawn and no click is registered (measuring). Returns the height.
fn flow(g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, items: &[(String, bool, Event)], draw: bool) -> f32 {
    let (mut cx, mut cy) = (x, y);
    let h = 28.0 * s;
    let sz = 13.0 * s;
    for (label, on, ev) in items {
        let cw = (g.text_width(sz, label, Face::Bold) + sz * 1.9).min(w);
        if cx > x && cx + cw > x + w {
            cx = x;
            cy += h + 8.0 * s;
        }
        if draw {
            chip(g, cx, cy, s, label, *on, ev.clone());
        }
        cx += cw + 8.0 * s;
    }
    cy + h - y
}

fn section(g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, title: &str, draw: bool) -> f32 {
    if draw {
        fx::section_title(g, x, y, w, s, title, ACCENT);
    }
    22.0 * s
}

fn heart(g: &mut Gfx, cx: f32, cy: f32, size: f32, c: [u8; 4], filled: bool) {
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

fn hash(s: &str) -> u64 {
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
fn normalize_options(o: &mut StoryOptions) {
    o.characters = String::new();
    if o.pictures != PictureMode::Off {
        o.pictures = PictureMode::Auto;
    }
}

pub struct StoryApp {
    page: Page,
    focus: Focus,
    opts: StoryOptions,
    idea: IdeaBox,
    search: TextField,
    store: StoryStore,
    prefs: st::Prefs,
    tts: TtsStatus,
    quick: Vec<st::QuickEngine>,
    voices: Vec<String>,
    takes_speed: bool,
    /// The engine that reads is the browser's (or window's) own voice: no audio is made here, so no audio can be saved.
    browser_voice: bool,
    /// The engine of this computer speaks by itself (spd-say, espeak, say, SAPI): the reader hands it the text of each piece.
    direct: Option<hostengines::Spec>,
    /// The engine that reads cannot write an audio file (the browser voice, spd-say), so no audio can be saved.
    no_audio_file: bool,
    /// Something to open in another app: Settings → Voice.
    request: Option<(String, String)>,
    /// The chip "Turn on reading aloud" was pressed and the engines of this computer are being looked for.
    turning_on: bool,
    image_models: Vec<(String, String)>,
    // the story on the page
    raw: String,
    story: Story,
    story_opts: StoryOptions,
    saved: Option<StoryEntry>,
    gen: Option<Gen>,
    error: Option<String>,
    note: Option<(String, bool)>,
    // pictures
    pics: Vec<Pic>,
    /// The character sheet of this story (made once, used by every picture, saved with the story).
    sheet: Option<CharacterSheet>,
    pic_gen: u64,
    /// Picture log events not yet in the story's folder (the story is not saved yet).
    pic_pending: Vec<PicEvent>,
    /// The extra pictures still to be drawn (scene by scene, style by style), the one being drawn, and each scene's seed.
    alt_jobs: std::collections::VecDeque<AltJob>,
    alt_running: Option<usize>,
    scene_seed: HashMap<usize, u64>,
    /// A job estimated to take longer than this many seconds asks first (tests change it).
    confirm_secs: f64,
    /// The gallery of the scenes as last drawn: where each thumbnail is and what a click on it does.
    gal_hits: Vec<(R, usize, String)>,
    /// The picture model that is not downloaded, with the Install button.
    install: Option<Install>,
    /// When each picture being drawn was asked for, and from which provider and model.
    pic_started: HashMap<usize, Instant>,
    pic_target: Option<(String, String)>,
    health_tx: Sender<Result<(String, Vec<String>), String>>,
    health_rx: Receiver<Result<(String, Vec<String>), String>>,
    health_busy: bool,
    health_last: Option<Instant>,
    /// What the server said about itself while a picture is being drawn.
    server_state: String,
    last_wait_log: Option<Instant>,
    /// The line about the picture being drawn now (time so far, server state); shown when there is no other notice.
    wait_text: Option<String>,
    /// Tests shorten the time limit and the polling and logging intervals.
    test_timing: Option<(Duration, Duration, Duration)>,
    /// How many times each picture was asked for (`attempt` in the log).
    pic_attempts: HashMap<usize, u32>,
    pic_tx: Sender<PicMsg>,
    pic_rx: Receiver<PicMsg>,
    pic_cache: PicCache,
    // reading
    reader: Reader,
    chunks: Vec<Chunk>,
    chunked_units: usize,
    live: bool,
    saved_audio: Option<Arc<st::PrebuiltSynth>>,
    level: f32,
    audio_job: Option<AudioJob>,
    // scrolling and selection
    cscroll: f32,
    cmax: f32,
    tscroll: f32,
    tmax: f32,
    manual_unit: Option<usize>,
    stick_bottom: bool,
    pane: Pane,
    /// What is lit while reading (Settings → Voice), as read at the last frame.
    highlight: Highlight,
    /// The text area as last drawn, and for each line given to the pane its unit and first character: turns a click into a place.
    text_area: R,
    line_map: Vec<(usize, usize)>,
    /// A plain click in the text, waiting for its release, then out the double-click time: it starts reading from there.
    click_read: Option<(f32, f32)>,
    pending_read: Option<(f32, f32, u32)>,
    /// The word highlight as drawn: (unit, line, x, width), eased toward the word being said.
    word_vis: Option<(usize, usize, f32, f32)>,
    /// Where the sentence and the word being said were drawn last (screen pixels), and the top of every line given to the pane.
    word_rect: Option<R>,
    sentence_rects: Vec<R>,
    line_tops: Vec<f32>,
    lay: Vec<ULay>,
    lay_key: (i32, i32),
    // the composer
    pane_open: bool,
    menu: Option<Menu>,
    menu_sel: usize,
    panel_hover: bool,
    anchors: [R; 7],
    /// The person chose length, language, kind or tone themselves (a remembered choice counts): the text never overrides it.
    explicit: [bool; 4],
    /// The words of a suggestion the person removed with its chip.
    dismissed: [Option<String>; 5],
    /// Auto picked image models and the first one failed: the rest are drawn as SVG (said once).
    pic_auto: bool,
    /// What the page says about an agent that could not write (kept beside the saved note).
    agent_note: Option<String>,
    // the conversation beside the story
    convo: Vec<Message>,
    view: ChatView,
    tool_ok: HashMap<String, bool>,
    scroll_px: f32,
    max_off: f32,
    thinking: String,
    think_since: Option<Instant>,
    step_seq: usize,
    /// The version in use and how many there are (1 when the story has not been revised).
    ver: u32,
    ver_count: u32,
    /// A correction sent while the story was being written: applied when the writing has stopped.
    queued: Option<String>,
    /// A revision under way: what to go back to, and what to reuse.
    rev: Option<Rev>,
    /// The step that is writing the story.
    write_call: Option<ToolCall>,
    /// Width of the right-hand panel (unscaled), set by dragging its divider.
    panel_w: f32,
    drag_o: Option<f32>,
    div_rect: R,
    area_rect: R,
    scale: f32,
    wide: bool,
    // library
    lib: Vec<StoryEntry>,
    lib_sel: usize,
    lib_top: usize,
    modal: Option<Modal>,
    host_save: Option<(String, Vec<u8>)>,
    ticks: u64,
    dirty: bool,
    /// Where the form's fields were drawn last (relative to the content top), to scroll them into view.
    bar_rect: Option<R>,
    /// The buttons on the pictures (Draw again, Try again), drawn after the text area is registered so they are on top of it.
    pic_buttons: Vec<(R, &'static str, usize, [u8; 4])>,
    /// The pictures are made but waiting for the person's yes (Pictures: ask me each time).
    held: bool,
    /// The one line about where and when the story happens, kept so the pictures look alike.
    setting: Option<String>,
    /// Tests: the machine's memory instead of the real one.
    #[cfg(test)]
    test_memory: Option<iyakki_agent::sysinfo::Memory>,
    idea_geo: Option<(f32, f32)>,
    last_status: Status,
    /// A story was just asked for and may start reading by itself once there is something to read.
    auto_pending: bool,
    /// The name of the model that wrote (or is writing) the story on the page, for the header and status line.
    run_model: String,
    /// Tests: a backend per chosen model, instead of the real provider.
    #[cfg(test)]
    test_backend: Option<Arc<dyn Fn(&ModelChoice) -> Arc<dyn ChatBackend> + Send + Sync>>,
    /// Tests: change who draws.
    #[cfg(test)]
    test_drawers: Option<Arc<dyn Fn(Vec<Drawer>) -> Vec<Drawer> + Send + Sync>>,
}

impl StoryApp {
    pub fn new(cx: &AppCx) -> StoryApp {
        let store = StoryStore::open(&cx.session.policy, &cx.session.user, cx.services.dir());
        let prefs = store.load_prefs();
        let mut opts = prefs.options.clone();
        opts.topic = String::new();
        normalize_options(&mut opts);
        let mut search = TextField::new("search your stories", false);
        search.max = 80;
        let (pic_tx, pic_rx) = channel();
        let (health_tx, health_rx) = channel();
        let mut app = StoryApp {
            page: Page::Story,
            focus: Focus::Idea,
            opts,
            idea: new_idea(),
            search,
            store,
            prefs,
            tts: TtsStatus::NoEngine,
            quick: Vec::new(),
            voices: Vec::new(),
            takes_speed: false,
            browser_voice: false,
            direct: None,
            no_audio_file: false,
            request: None,
            turning_on: false,
            image_models: Vec::new(),
            raw: String::new(),
            story: Story::default(),
            story_opts: StoryOptions::default(),
            saved: None,
            gen: None,
            error: None,
            note: None,
            pics: Vec::new(),
            sheet: None,
            pic_gen: 0,
            pic_pending: Vec::new(),
            alt_jobs: std::collections::VecDeque::new(),
            alt_running: None,
            scene_seed: HashMap::new(),
            confirm_secs: illus::CONFIRM_SECS,
            gal_hits: Vec::new(),
            install: None,
            pic_started: HashMap::new(),
            pic_target: None,
            health_tx,
            health_rx,
            health_busy: false,
            health_last: None,
            server_state: String::new(),
            last_wait_log: None,
            wait_text: None,
            test_timing: None,
            pic_attempts: HashMap::new(),
            pic_tx,
            pic_rx,
            pic_cache: PicCache::default(),
            reader: Reader::new(),
            chunks: Vec::new(),
            chunked_units: 0,
            live: false,
            saved_audio: None,
            level: 0.0,
            audio_job: None,
            cscroll: 0.0,
            cmax: 0.0,
            tscroll: 0.0,
            tmax: 0.0,
            manual_unit: None,
            stick_bottom: true,
            pane: Pane::new(TEXT_PANE),
            highlight: Highlight::default(),
            text_area: R::new(0.0, 0.0, 0.0, 0.0),
            line_map: Vec::new(),
            click_read: None,
            pending_read: None,
            word_vis: None,
            word_rect: None,
            sentence_rects: Vec::new(),
            line_tops: Vec::new(),
            lay: Vec::new(),
            lay_key: (0, 0),
            pane_open: true,
            menu: None,
            menu_sel: 0,
            panel_hover: false,
            anchors: [R::new(0.0, 0.0, 0.0, 0.0); 7],
            explicit: [false; 4],
            dismissed: [None, None, None, None, None],
            pic_auto: false,
            agent_note: None,
            convo: Vec::new(),
            view: ChatView::new(),
            tool_ok: HashMap::new(),
            scroll_px: 0.0,
            max_off: 0.0,
            thinking: String::new(),
            think_since: None,
            step_seq: 0,
            ver: 1,
            ver_count: 1,
            queued: None,
            rev: None,
            write_call: None,
            panel_w: 380.0,
            drag_o: None,
            div_rect: R::new(0.0, 0.0, 0.0, 0.0),
            area_rect: R::new(0.0, 0.0, 0.0, 0.0),
            scale: 1.0,
            wide: true,
            lib: Vec::new(),
            lib_sel: 0,
            lib_top: 0,
            modal: None,
            host_save: None,
            ticks: 0,
            dirty: true,
            bar_rect: None,
            pic_buttons: Vec::new(),
            held: false,
            setting: None,
            #[cfg(test)]
            test_memory: None,
            idea_geo: None,
            last_status: Status::Idle,
            auto_pending: false,
            run_model: String::new(),
            #[cfg(test)]
            test_backend: None,
            #[cfg(test)]
            test_drawers: None,
        };
        let d = StoryOptions::default();
        app.explicit = [app.opts.length != d.length, app.opts.language != d.language, app.opts.genre != d.genre, app.opts.tone != d.tone];
        app.refresh_tts(cx);
        app
    }

    // ------------------------------------------------------------------------------------------------ small helpers

    pub fn page_name(&self) -> &'static str {
        match self.page {
            Page::Story => "story",
            Page::Library => "library",
        }
    }
    pub fn tts_status(&self) -> &TtsStatus {
        &self.tts
    }
    pub fn story_text(&self) -> String {
        self.story.plain()
    }
    pub fn is_writing(&self) -> bool {
        self.gen.is_some()
    }
    pub fn reader_status(&self) -> Status {
        self.reader.status()
    }
    pub fn error_text(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn note_text(&self) -> Option<&str> {
        self.note.as_ref().map(|n| n.0.as_str())
    }
    pub fn focus_name(&self) -> &'static str {
        match self.focus {
            Focus::None => "none",
            Focus::Idea => "idea",
            Focus::Search => "search",
        }
    }
    pub fn saved_id(&self) -> Option<String> {
        self.saved.as_ref().map(|e| e.id.clone())
    }
    pub fn library_ids(&self) -> Vec<String> {
        self.lib.iter().map(|e| e.id.clone()).collect()
    }
    pub fn options(&self) -> &StoryOptions {
        &self.opts
    }
    pub fn take_pending_save(&mut self) -> Option<(String, Vec<u8>)> {
        self.host_save.take()
    }

    fn refresh_tts(&mut self, cx: &AppCx) {
        let v = cx.services.voice_settings();
        let host = HostVoice::current();
        // the engines of this computer are looked for once, in the background, the first time somebody might read
        cx.services.host_tts.ensure();
        let lang = hostvoice::lang_tag(&self.story_opts.language);
        let auto = if v.tts.is_none() { cx.services.host_tts.auto_engine(Some(lang)) } else { None };
        let engine = st::effective_engine_auto(&v, &host, auto.as_ref());
        self.browser_voice = engine == Some(SpeechEngine::Browser);
        if self.browser_voice {
            // the window front end checks its engine only when somebody wants to read
            host.want_probe();
        }
        self.direct = engine.as_ref().filter(|e| hostengines::speaks_directly(e)).and_then(hostengines::Spec::from_setting);
        self.no_audio_file = self.browser_voice || engine.as_ref().is_some_and(|e| !hostengines::can_save_audio(e));
        self.tts = st::tts_status_auto(&v, cx.session.policy.voice, &path_env(), &host, auto.as_ref());
        self.voices = if self.browser_voice {
            self.browser_voices(&host)
        } else if matches!(engine, Some(SpeechEngine::Host { .. })) {
            self.host_voices(cx, &engine, lang)
        } else {
            engine.as_ref().map(st::voice_choices).unwrap_or_default()
        };
        self.takes_speed = engine.as_ref().is_some_and(st::engine_takes_speed);
        self.image_models = cx.services.image_models();
        self.quick = if matches!(self.tts, TtsStatus::NoEngine) && cx.session.policy.manage_settings {
            let providers = cx.services.settings.lock().map(|s| s.get().providers.clone()).unwrap_or_default();
            st::quick_engines(&providers, &path_env(), &voice_dirs(cx))
        } else {
            Vec::new()
        };
    }

    /// "automatic" (the engine picks a voice for the story's language) and the voices of the engine of this computer that speak it,
    /// or all of them when none does.
    fn host_voices(&self, cx: &AppCx, engine: &Option<SpeechEngine>, lang: &str) -> Vec<String> {
        let Some(SpeechEngine::Host { engine: id, .. }) = engine else { return Vec::new() };
        let Some(report) = cx.services.host_tts.report() else { return vec![String::new()] };
        let Some(e) = hostengines::Kind::from_id(id).and_then(|k| report.get(k)) else { return vec![String::new()] };
        let mut mine: Vec<String> = e.voices.iter().filter(|v| v.fit(lang) > 0).map(|v| v.id.clone()).collect();
        if mine.is_empty() {
            mine = e.voices.iter().map(|v| v.id.clone()).collect();
        }
        mine.truncate(60);
        std::iter::once(String::new()).chain(mine).collect()
    }

    /// "automatic" (the browser picks a voice for the story's language) and the browser's voices for that language, or all of them
    /// when none speaks it.
    fn browser_voices(&self, host: &HostVoice) -> Vec<String> {
        let all = host.voices();
        let mut mine = hostvoice::voices_for(&all, hostvoice::lang_tag(&self.opts.language));
        if mine.is_empty() {
            mine = all;
        }
        std::iter::once(String::new()).chain(mine.into_iter().map(|v| v.name)).collect()
    }

    fn can_read(&self) -> bool {
        matches!(self.tts, TtsStatus::Ready { .. }) || self.saved_audio.is_some()
    }

    fn sync_fields(&mut self) {
        self.opts.topic = self.idea.text.clone();
    }

    fn set_focus(&mut self, f: Focus) {
        self.focus = f;
        // the idea box is only reachable with the panel in view
        if f == Focus::Idea && !self.wide {
            self.pane_open = true;
        }
    }

    fn voice_key(&self) -> String {
        if self.saved_audio.is_some() {
            return "saved".into();
        }
        format!("{}{}@{:.2}", if self.browser_voice { "browser:" } else if self.direct.is_some() { "direct:" } else { "" }, self.prefs.voice.clone().unwrap_or_default(), self.prefs.speed)
    }

    fn save_prefs(&mut self) {
        self.prefs.options = self.opts.clone();
        self.prefs.options.topic.clear();
        let _ = self.store.save_prefs(&self.prefs);
    }

    fn go(&mut self, p: Page, cx: &AppCx) {
        self.page = p;
        self.modal = None;
        self.menu = None;
        self.focus = Focus::None;
        if p == Page::Library {
            self.reload_library();
        }
        self.refresh_tts(cx);
    }

    /// Show the form and put the keyboard in the idea box.
    fn open_form(&mut self, cx: &AppCx) {
        self.page = Page::Story;
        self.modal = None;
        self.pane_open = true;
        self.refresh_tts(cx);
        self.set_focus(Focus::Idea);
    }

    // ------------------------------------------------------------------------------------------------ writing

    // ------------------------------------------------------------------------------------------------ the model

    /// Every (provider, model) of the enabled providers: the same list the chat's model chooser starts with.
    fn text_models(cx: &AppCx) -> Vec<(String, String)> {
        let st = cx.services.settings.lock().unwrap();
        st.get().providers.iter().filter(|p| p.enabled).flat_map(|p| p.models.iter().map(move |m| (p.id.clone(), m.clone()))).collect()
    }

    fn chat_choice(cx: &AppCx) -> Option<ModelChoice> {
        cx.services.settings.lock().unwrap().get().chat.clone()
    }

    fn model_plan(&self, cx: &AppCx) -> ModelPlan {
        st::plan_model(self.prefs.model.as_ref(), &Self::text_models(cx), "model")
    }

    /// The picture model to use, if one was chosen and is still there, and a note when it is not.
    fn picture_plan(&self, cx: &AppCx) -> ModelPlan {
        let found = if self.image_models.is_empty() { cx.services.image_models() } else { self.image_models.clone() };
        st::plan_model(self.prefs.picture_model.as_ref(), &found, "picture model")
    }

    /// How a model is named on the page: `qwen3 · home`.
    fn model_name(c: &ModelChoice) -> String {
        format!("{} · {}", c.model, c.provider)
    }

    /// What the Model chip says, and the note to show when a remembered choice could not be used.
    fn model_label(&self, cx: &AppCx) -> (String, Option<String>) {
        let chat = Self::chat_choice(cx).map_or("no chat model yet".to_string(), |c| Self::model_name(&c));
        match self.model_plan(cx) {
            ModelPlan::Chosen(c) => (Self::model_name(&c), None),
            ModelPlan::SameAsChat { note } => (format!("same as chat ({chat})"), note),
        }
    }

    fn picture_label(&self, cx: &AppCx) -> (String, Option<String>) {
        match self.picture_plan(cx) {
            ModelPlan::Chosen(c) => (Self::model_name(&c), None),
            ModelPlan::SameAsChat { note } => ("automatic".to_string(), note),
        }
    }

    /// The backend that writes: the chosen model, or the chat's.
    fn backend(&self, cx: &AppCx) -> Arc<dyn ChatBackend> {
        match self.model_plan(cx) {
            ModelPlan::Chosen(c) => {
                #[cfg(test)]
                if let Some(f) = &self.test_backend {
                    return f(&c);
                }
                cx.services.chat_backend_for(c)
            }
            ModelPlan::SameAsChat { .. } => cx.chat.clone(),
        }
    }

    /// The one choice of who writes, from the remembered preferences: a chosen agent that is still set up wins, then a model of
    /// a provider that is still there, else the chat's model. (Older versions kept the model and the agent as two choices;
    /// both fields are still read, and the agent is what answered then too.)
    fn choice(&self, cx: &AppCx) -> Choice {
        if let Some(id) = self.agent_id(cx) {
            return Choice::Agent(id, self.prefs.agent_model.clone());
        }
        match self.model_plan(cx) {
            ModelPlan::Chosen(c) => Choice::Provider(c),
            ModelPlan::SameAsChat { .. } => Choice::SameAsChat,
        }
    }

    /// The name of an agent's model as the chooser shows it: `Sonnet`, `Default`, or the id that was typed.
    fn agent_model_name(&self, cx: &AppCx, id: &str, model: &str) -> String {
        if model.is_empty() {
            return "Default".into();
        }
        let cfg = cx.services.agents_cfg();
        cfg.agent(id).and_then(|a| cx.services.ext_status.models_for(a.as_ref()).into_iter().find(|m| m.id == model)).map_or(model.to_string(), |m| m.label)
    }

    /// What the Model chip says: `Model: Sonnet · Claude Code`, `Model: qwen3:8b · nvidia_ollama` or `Model: same as chat`.
    fn chip_text(&self, cx: &AppCx) -> String {
        let t = match self.choice(cx) {
            Choice::SameAsChat => "same as chat".to_string(),
            Choice::Provider(c) => Self::model_name(&c),
            Choice::Agent(id, model) => format!("{} · {}", self.agent_model_name(cx, &id, &model), self.agent_label(cx).unwrap_or(id).replace(" (agent)", "")),
        };
        let t = if t.chars().count() > 36 { format!("{}…", t.chars().take(35).collect::<String>()) } else { t };
        format!("Model: {t}")
    }

    /// An agent as the chooser lists it (for the notes after choosing it).
    fn agent_row(&self, cx: &AppCx, id: &str) -> Option<AgentRow> {
        let cfg = cx.services.agents_cfg();
        let e = iyakki_agent::extagent::entries(&cfg, &cx.services.ext_status).into_iter().find(|e| e.id == id)?;
        let advisor = cfg.agent(id).is_some_and(|a| a.mode() == iyakki_agent::extagent::Mode::Advisor);
        Some(AgentRow { label: e.label.replace(" (agent)", ""), chip: e.chip, ready: e.ready, advisor })
    }

    /// Open the model chooser (Ctrl+P, Ctrl+G or the chip): the chat's list with "Same as chat" first, and the agents. With
    /// `picture`, the picture models only. Anyone may choose here: it changes only this tile's own preference.
    fn open_model_pick(&mut self, picture: bool, cx: &AppCx) {
        if self.gen.is_some() {
            self.note = Some(("Wait until the story is written, then choose the model.".into(), false));
            return;
        }
        let plain = if picture { if self.image_models.is_empty() { cx.services.image_models() } else { self.image_models.clone() } } else { Self::text_models(cx) };
        let (top, current, title, hint) = if picture {
            let cur = self.prefs.picture_model.clone();
            (
                TopRow { label: "Automatic".into(), note: "the first picture model found".into() },
                Current { same: cur.is_none(), plain: cur.map(|c| (c.provider, c.model)), agent: None },
                "Choose the picture model",
                "Enter or click chooses · Esc closes · only for pictures made by a picture model.",
            )
        } else {
            let chat = Self::chat_choice(cx).map_or("no chat model yet".to_string(), |c| Self::model_name(&c));
            let current = match self.choice(cx) {
                Choice::SameAsChat => Current { same: true, ..Default::default() },
                Choice::Provider(c) => Current { plain: Some((c.provider, c.model)), ..Default::default() },
                Choice::Agent(id, model) => Current { agent: Some((id, model)), ..Default::default() },
            };
            (
                TopRow { label: "Same as chat".into(), note: chat },
                current,
                "Choose the model",
                "Enter or click chooses · Esc closes · only for TALES; the chat keeps its own model · Settings → Models adds providers, Settings → Agents sets up Claude Code and others.",
            )
        };
        let list = model_picker::build(cx, plain, Some(top), !picture, current, title, hint);
        // the first line is always there; with nothing after it there is nothing to choose
        let Some(list) = list.filter(|l| l.items.len() > 1) else {
            self.note = Some((if picture { "No picture model is available. Settings → Models: connect a server with a model such as FLUX or Stable Diffusion." } else { "No models yet. An administrator can open Settings → Models, add a provider and press t to fetch its model list." }.into(), false));
            return;
        };
        self.focus = Focus::None;
        self.note = None;
        self.menu = None;
        self.modal = Some(Modal::Model(TalesPick { list, picture }));
    }

    /// Make the picked line the choice and remember it.
    fn choose_model(&mut self, pick: Pick, picture: bool, cx: &AppCx) {
        match pick {
            Pick::Top | Pick::Provider(..) => {
                let choice = match pick {
                    Pick::Provider(p, m) => Some(ModelChoice { provider: p, model: m }),
                    _ => None,
                };
                if picture {
                    self.prefs.picture_model = choice.clone();
                } else {
                    // a provider's model (or "same as chat") replaces an agent: one choice
                    self.prefs.model = choice.clone();
                    if self.prefs.agent.take().is_some() {
                        cx.audit.record(&cx.session.user, "story.agent", "own model");
                    }
                    self.prefs.agent_model = String::new();
                }
                let _ = self.store.save_prefs(&self.prefs);
                cx.audit.record(&cx.session.user, if picture { "story.picture_model" } else { "story.model" }, &choice.map_or("same as chat".to_string(), |c| format!("{} {}", c.provider, c.model)));
                self.set_focus(Focus::Idea);
            }
            // pictures are never made by an agent
            Pick::Agent(_) | Pick::AgentModel { .. } if picture => {}
            Pick::Agent(id) => {
                let keep = if self.prefs.agent.as_deref() == Some(id.as_str()) { self.prefs.agent_model.clone() } else { String::new() };
                self.choose_agent(&id, keep, false, cx);
            }
            Pick::AgentModel { agent, model, custom } => self.choose_agent(&agent, model, custom, cx),
        }
    }

    /// An agent (and its model; empty = its own default) now writes the stories. An agent that is not ready is still chosen: the
    /// exact fix is said, and our own model writes until it is ready.
    fn choose_agent(&mut self, id: &str, model: String, custom: bool, cx: &AppCx) {
        let Some(row) = self.agent_row(cx, id) else { return };
        if cx.session.policy.cli != iyakki_sec::policy::CliLevel::Full {
            self.note = Some(("Your account may not run commands, so external agents are off for it.".into(), false));
            return;
        }
        if custom {
            model_picker::remember_custom(cx, id, &model);
        }
        self.prefs.agent = Some(id.to_string());
        self.prefs.agent_model = model;
        let _ = self.store.save_prefs(&self.prefs);
        cx.audit.record(&cx.session.user, "story.agent", id);
        cx.audit.record(&cx.session.user, "story.agent_model", &format!("{id} {}", self.prefs.agent_model));
        self.note = if !row.ready {
            Some((format!("{} is {}: the story is written with our own model until it is ready (Settings → Agents).", row.label, row.chip), false))
        } else if row.advisor {
            Some((format!("{} is an advisor here (read-only): it can only write the story as text, which is all a story needs. It cannot save files; TALES saves the story.", row.label), true))
        } else {
            None
        };
        self.set_focus(Focus::Idea);
    }

    /// A key, click or paste in the chooser.
    fn model_event(&mut self, ev: &Event, cx: &AppCx) {
        let Some(Modal::Model(m)) = self.modal.as_mut() else { return };
        let picture = m.picture;
        match model_picker::key(&mut m.list, ev) {
            model_picker::Out::Stay => {}
            model_picker::Out::Close => {
                self.modal = None;
                self.set_focus(Focus::Idea);
            }
            model_picker::Out::Bad(e) => self.note = Some((e, false)),
            model_picker::Out::Pick(p) => {
                self.modal = None;
                self.choose_model(p, picture, cx);
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ variants and the gallery

    /// Queue the extra pictures of the scenes in `idxs`: one per further style and per further picture of a style, scene by
    /// scene, style by style. The first picture of the first style is the scene's main picture and is not queued here.
    fn queue_alts(&mut self, idxs: &[usize]) {
        let pic = self.story_opts.pic.clone();
        let (styles, count) = (pic.styles_list(), pic.count());
        for &idx in idxs {
            if self.pics.get(idx).is_none_or(|p| p.mode != PictureMode::ImageModel) {
                continue;
            }
            for (si, look) in styles.iter().enumerate() {
                for n in 0..count {
                    if si == 0 && n == 0 {
                        continue;
                    }
                    self.alt_jobs.push_back(AltJob { idx, look: *look, n });
                }
            }
        }
    }

    /// How many pictures the job makes for `scenes` scenes and the estimated seconds in all (0.0 when the model's steps are not
    /// known, so no estimate is made).
    fn job_totals(&self, cx: &AppCx, scenes: usize) -> (usize, f64) {
        let pic = self.pic_now();
        let cpu_only = !iyakki_agent::aiprofile::this_machine(0).capable();
        let mut total = 0.0;
        for look in pic.styles_list() {
            match self.resolved_look(&pic, look, cx).and_then(|r| r.steps.map(|st| illus::est_secs(r.width, r.height, st, cpu_only))) {
                Some(each) => total += each * pic.count() as f64 * scenes as f64,
                None => return (illus::job_estimate(scenes, pic.styles_list().len(), pic.count(), 0.0).0, 0.0),
            }
        }
        (illus::job_estimate(scenes, pic.styles_list().len(), pic.count(), 0.0).0, total)
    }

    /// The total line for the job, before it starts.
    fn job_line(&self, cx: &AppCx, scenes: usize) -> Option<String> {
        let (n, total) = self.job_totals(cx, scenes);
        let pic = self.pic_now();
        (total > 0.0 && n > 0).then(|| illus::job_text(scenes, pic.styles_list().len(), pic.count(), total / n as f64))
    }

    /// Make extra picture `k` of scene `idx` its main picture (the reader, the cover and the export use the main one); the old
    /// main picture takes its place in the gallery.
    fn choose_main(&mut self, idx: usize, k: usize) {
        let Some(p) = self.pics.get_mut(idx) else { return };
        if k >= p.alts.len() || p.state != PicState::Ready {
            return;
        }
        let a = p.alts[k].clone();
        let old = Alt { look: p.look, seed: p.seed, provider: p.provider.clone(), model: p.model.clone(), size: p.size.clone(), ms: p.ms, ext: p.ext.clone(), bytes: p.bytes.clone().unwrap_or_default(), prompt: p.prompt.clone(), key: p.key };
        (p.look, p.seed, p.provider, p.model, p.ms) = (a.look, a.seed, a.provider.clone(), a.model.clone(), a.ms);
        (p.size, p.ext, p.prompt, p.key, p.bytes) = (a.size.clone(), a.ext.clone(), a.prompt.clone(), a.key, Some(a.bytes.clone()));
        p.how = format!("images via {}", a.model);
        p.alts[k] = old;
        if self.saved.is_some() {
            self.persist_pictures();
        }
        self.note = Some((format!("The {} picture is now the main one of scene {}.", a.look.label(), idx + 1), true));
        self.dirty = true;
    }

    fn delete_alt(&mut self, idx: usize, k: usize) {
        if self.pics.get(idx).is_none_or(|p| k >= p.alts.len()) {
            return;
        }
        self.pics[idx].alts.remove(k);
        if self.saved.is_some() {
            self.persist_pictures();
        }
        self.note = Some(("That picture was deleted; the others are as they were.".into(), true));
        self.dirty = true;
    }

    /// Remove every picture that is not the chosen main one of its scene.
    fn keep_only_chosen(&mut self) {
        let n: usize = self.pics.iter().map(|p| p.alts.len()).sum();
        for p in &mut self.pics {
            p.alts.clear();
        }
        self.alt_jobs.clear();
        if self.saved.is_some() {
            self.persist_pictures();
        }
        self.note = Some((format!("Kept only the chosen picture of each scene ({n} removed)."), true));
        self.dirty = true;
    }

    /// Draw one more picture of scene `idx` in the next style (a new seed), without redrawing the others.
    fn more_alt(&mut self, idx: usize, cx: &AppCx) {
        let Some(p) = self.pics.get(idx) else { return };
        if p.state != PicState::Ready || p.mode != PictureMode::ImageModel {
            return;
        }
        let styles = self.story_opts.pic.styles_list();
        let look = styles[(p.alts.len() + 1) % styles.len()];
        let n = (p.alts.iter().filter(|a| a.look == look).count() + usize::from(p.look == look)) as u32;
        self.alt_jobs.push_back(AltJob { idx, look, n });
        self.held = false;
        self.pump_pictures(cx);
        self.dirty = true;
    }

    /// The height of the gallery under the picture of scene `pi` (0 when the scene has only its main picture).
    fn gallery_h(&self, pi: usize, slot_w: f32, s: f32) -> f32 {
        let n = self.pics.get(pi).map_or(0, |p| p.alts.len());
        if n == 0 {
            return 0.0;
        }
        let per_row = ((slot_w / (118.0 * s)) as usize).max(1);
        ((n + 1).div_ceil(per_row)) as f32 * 104.0 * s + 34.0 * s
    }

    /// The row of thumbnails under a scene's picture: the main one (outlined) and the others with their style, model and time.
    /// A click on another picture makes it the main one.
    fn draw_gallery(&mut self, g: &mut Gfx, slot: R, s: f32, idx: usize) {
        let h = self.gallery_h(idx, slot.w, s);
        if h <= 0.0 {
            return;
        }
        let (tw, th) = (110.0 * s, 62.0 * s);
        let per_row = ((slot.w / (118.0 * s)) as usize).max(1);
        let items: Vec<(Option<usize>, illus::Look, String, u64, u64, Arc<Vec<u8>>)> = {
            let p = &self.pics[idx];
            let mut v = vec![(None, p.look, p.model.clone(), p.ms, p.key, p.bytes.clone().unwrap_or_default())];
            v.extend(p.alts.iter().enumerate().map(|(k, a)| (Some(k), a.look, a.model.clone(), a.ms, a.key, a.bytes.clone())));
            v
        };
        let y0 = slot.y + slot.h + 6.0 * s;
        for (i, (alt, look, model, ms, key, bytes)) in items.iter().enumerate() {
            let r = R::new(slot.x + (i % per_row) as f32 * 118.0 * s, y0 + (i / per_row) as f32 * 104.0 * s, tw, th);
            g.rrect(r.x, r.y, r.w, r.h, 8.0 * s, rgb(14, 18, 36));
            if let Ok(rd) = self.pic_cache.draw_bytes(g, *key, bytes, tw, s) {
                let (pw, ph) = (rd.pm.width() as f32, rd.pm.height() as f32);
                let k = (tw / pw).min(th / ph);
                let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
                g.pm.draw_pixmap(0, 0, rd.pm.as_ref().as_ref(), &paint, tiny_skia::Transform::from_translate(r.x + (tw - pw * k) / 2.0, r.y + (th - ph * k) / 2.0).pre_scale(k, k), None);
            }
            g.rrect_outline(r.x, r.y, r.w, r.h, 8.0 * s, if alt.is_none() { 2.2 * s } else { 1.0 }, if alt.is_none() { ACCENT } else { with_alpha(MUTED, 120) });
            let cap = format!("{}{}", if alt.is_none() { "main \u{b7} " } else { "" }, look.label());
            let cap = g.fit(11.0 * s, &cap, Face::Bold, tw);
            g.text(r.x, r.y + th + 2.0 * s, 11.0 * s, if alt.is_none() { ACCENT } else { TEXT }, &cap, Face::Bold);
            let sub = g.fit(10.0 * s, &format!("{model} \u{b7} {} s", ms / 1000), Face::Sans, tw);
            g.text(r.x, r.y + th + 2.0 * s + g.line_height(11.0 * s, Face::Bold), 10.0 * s, MUTED, &sub, Face::Sans);
            if let Some(k) = alt {
                self.gal_hits.push((r, T_ALT_PICK + idx * 24 + k, format!("Make the {} picture the main one", look.label())));
                let x = R::new(r.x + r.w - 18.0 * s, r.y + 2.0 * s, 16.0 * s, 16.0 * s);
                g.rrect(x.x, x.y, x.w, x.h, 8.0 * s, [6, 8, 20, 210]);
                g.text(x.x + 4.5 * s, x.y + 1.0 * s, 11.0 * s, ERR, "x", Face::Bold);
                self.gal_hits.push((x, T_ALT_DEL + idx * 24 + k, format!("Delete this {} picture", look.label())));
            }
        }
        let styles = self.story_opts.pic.styles_list();
        let next = styles[(self.pics[idx].alts.len() + 1) % styles.len()];
        let rows = items.len().div_ceil(per_row);
        let by = y0 + rows as f32 * 104.0 * s - 2.0 * s;
        if self.gen.is_none() && self.alt_running.is_none() {
            self.gal_hits.push((R::new(slot.x, by, 0.0, 0.0), T_ALT_MORE + idx, format!("Draw another in {}", next.label())));
        }
    }

    /// The picture log window: the events of the story, newest at the bottom, scrolling, with Copy log, Clear log and Verbose.
    fn draw_pic_log(&self, g: &mut Gfx, area: R, s: f32) {
        let Some(Modal::PicLog(v)) = &self.modal else { return };
        let p = fx::modal(g, area, s, 920.0 * s, area.h - 24.0 * s, ACCENT, false);
        let events = self.pic_log_events();
        let top = fx::modal_title(g, p, s, &format!("Picture log · {}", self.story_name()), ACCENT);
        let sub = format!("{} event{} · stays on this computer (it holds your story text) · the key is never written · g, Esc or Close leaves · Up, Down, PageUp, PageDown scroll", events.len(), if events.len() == 1 { "" } else { "s" });
        let sub = g.fit(12.0 * s, &sub, Face::Sans, p.w - 52.0 * s);
        g.text(p.x + 26.0 * s, top - 2.0 * s, 12.0 * s, MUTED, &sub, Face::Sans);
        let (bsz, by) = (13.0 * s, top + 20.0 * s);
        let mut bx = p.x + 26.0 * s;
        let verbose = self.pic_now().verbose;
        let clear = if v.confirm_clear { "Press again to clear" } else { "Clear log" };
        for (label, code, col) in [("Copy log".to_string(), T_LOG_COPY, ACCENT), (clear.to_string(), T_LOG_CLEAR, if v.confirm_clear { ERR } else { MUTED }), (format!("Verbose: {}", if verbose { "on" } else { "off" }), T_LOG_VERBOSE, if verbose { ACCENT } else { MUTED }), ("Close".to_string(), T_LOG_CLOSE, MUTED)] {
            bx += g.button(bx, by, bsz, &label, col, Event::Tab(code)) + 8.0 * s;
        }
        let inner = R::new(p.x + 26.0 * s, by + bsz * 2.1 + 12.0 * s, p.w - 52.0 * s - 10.0 * s, p.y + p.h - (by + bsz * 2.1 + 12.0 * s) - 16.0 * s);
        let (sz, face) = (11.5 * s, Face::Mono);
        let lh = g.line_height(sz, face);
        let text = if events.is_empty() { "Nothing yet. Every picture request, skip, cap and fall back to SVG is written here.".to_string() } else { piclog::log_text(&events) };
        let mut lines: Vec<String> = Vec::new();
        for l in text.split('\n') {
            if l.is_empty() {
                lines.push(String::new());
            } else {
                lines.extend(g.wrap(sz, l, face, inner.w));
            }
        }
        let rows = ((inner.h / lh) as usize).max(1);
        let max = lines.len().saturating_sub(rows);
        let at = if v.at_end.get() { max } else { v.top.get().min(max) };
        v.top.set(at);
        v.total.set(lines.len());
        v.rows.set(rows);
        g.clipped(inner, |g| {
            for (i, l) in lines.iter().skip(at).take(rows + 1).enumerate() {
                g.text(inner.x, inner.y + i as f32 * lh, sz, if l.starts_with("20") { TEXT } else { MUTED }, l, face);
            }
        });
        fx::scrollbar(g, R::new(p.x + p.w - 22.0 * s, inner.y, 6.0 * s, inner.h), s, at, rows, lines.len(), ACCENT);
    }

    fn draw_model_pick(&self, g: &mut Gfx, area: R, s: f32, cx: &AppCx) {
        let Some(Modal::Model(m)) = &self.modal else { return };
        model_picker::draw(g, area, s, cx, &m.list);
    }

    /// Send: a new story when there is none on the page, a correction of it when there is. While a story is being written a
    /// correction stops it where it is and is applied next.
    fn submit(&mut self, cx: &AppCx) {
        self.sync_fields();
        let text = self.idea.text.trim().to_string();
        if self.gen.is_some() {
            if text.is_empty() {
                self.note = Some(("Type what to change: the story stops where it is and your change is applied.".into(), false));
                return;
            }
            self.queued = Some(text);
            self.idea.set("");
            self.sync_fields();
            self.stop_writing();
            self.note = Some(("Stopping here; your change is applied next.".into(), true));
            return;
        }
        if self.story.blocks.is_empty() {
            self.start_create(cx);
        } else {
            self.start_revision(text, cx);
        }
    }

    /// A step of the conversation: shown as a dot on the timeline of the panel until `step_done` gives its result.
    fn step(&mut self, name: &str, input: &str) -> ToolCall {
        self.step_seq += 1;
        let c = ToolCall { id: format!("tales-{}", self.step_seq), name: name.into(), args: serde_json::json!({ "prompt": input }) };
        self.convo.push(Message::assistant_calls("", vec![c.clone()]));
        c
    }

    fn step_done(&mut self, c: &ToolCall, ok: bool, text: &str) {
        self.tool_ok.insert(c.id.clone(), ok);
        self.convo.push(Message::tool_result(c, if ok { text.to_string() } else { format!("Failed: {text}") }));
    }

    /// Several drawing steps at once (one message), each the step of one picture.
    fn push_draw_calls(&mut self, idxs: &[usize]) {
        let mut calls = Vec::new();
        for &i in idxs {
            self.step_seq += 1;
            let id = format!("tales-{}", self.step_seq);
            let title = self.pics[i].scene.title.clone();
            calls.push(ToolCall { id: id.clone(), name: "Draw".into(), args: serde_json::json!({ "prompt": format!("picture {}: {}", i + 1, title) }) });
            self.pics[i].call = Some(id);
        }
        if !calls.is_empty() {
            self.convo.push(Message::assistant_calls("", calls));
        }
    }

    /// The result of the drawing step of picture `idx`.
    fn pic_result(&mut self, idx: usize, ok: bool, text: &str) {
        if let Some(id) = self.pics.get_mut(idx).and_then(|p| p.call.take()) {
            self.step_done(&ToolCall { id, name: "Draw".into(), args: serde_json::Value::Null }, ok, text);
        }
    }

    /// What the person asked for so far (the user messages of the conversation), oldest first.
    fn user_requests(&self) -> Vec<String> {
        self.convo.iter().filter(|m| m.role == iyakki_agent::chat::Role::User).map(|m| m.content.clone()).collect()
    }

    /// The line of the "Plan" step: what was understood.
    fn plan_text(&self, o: &StoryOptions, cx: &AppCx) -> String {
        let pictures = match o.pictures {
            PictureMode::Off => "off".to_string(),
            PictureMode::ImageModel => format!("images via {}", self.picture_name(cx)),
            _ => format!("{}, {}", match o.drawing {
                st::DrawingMode::Auto => "drawings (a strong model draws SVG or an image itself, a small one uses the built-in cartoon)",
                st::DrawingMode::Image => "an image model, else drawings",
                st::DrawingMode::Rich => "rich SVG by the model",
                st::DrawingMode::Cartoon => "cartoon (built-in)",
            }, o.style.label().to_lowercase()),
        };
        format!("For {} · {} · {} · {} · {}. Pictures: {pictures}.", o.audience.label(), o.genre.label(), o.length.label(), o.language, o.tone.label())
    }

    fn picture_name(&self, cx: &AppCx) -> String {
        match self.picture_plan(cx) {
            ModelPlan::Chosen(c) => c.model,
            ModelPlan::SameAsChat { .. } => self.image_models.first().map_or("the picture model".to_string(), |m| m.1.clone()),
        }
    }

    fn start_create(&mut self, cx: &AppCx) {
        if self.gen.is_some() {
            return;
        }
        self.sync_fields();
        let base = match self.opts.validated() {
            Ok(o) => o,
            Err(e) => {
                self.note = Some((e, false));
                self.error = None;
                return;
            }
        };
        // what the text says fills the options the person did not choose; the pictures follow what is available
        self.refresh_tts(cx);
        let mut o = self.with_hints(&base);
        o.pictures = base.pictures.resolve(!self.image_models.is_empty());
        o.style = self.style_for(&base.topic, o.audience, o.genre);
        let msgs = match st::messages(&o) {
            Ok(m) => m,
            Err(e) => {
                self.note = Some((e, false));
                return;
            }
        };
        self.reader.stop();
        self.reset_story();
        self.pic_auto = base.pictures == PictureMode::Auto && o.pictures == PictureMode::ImageModel && o.drawing != st::DrawingMode::Image;
        self.story_opts = o.clone();
        self.opts = base;
        self.menu = None;
        self.save_prefs();
        self.ver = 1;
        self.ver_count = 1;
        self.convo.push(Message::user(o.topic.clone()));
        let plan = self.step("Plan", &o.topic);
        let text = self.plan_text(&o, cx);
        self.step_done(&plan, true, &text);
        self.launch(cx, msgs, "story.create");
        self.idea.set("");
        self.sync_fields();
        self.dismissed = Default::default();
    }

    /// The look of the pictures: a style named in the text (unless its chip was removed), else the one for the kind of story.
    fn style_for(&self, text: &str, audience: Audience, genre: Genre) -> PictureStyle {
        match st::parse_hints(text).style {
            Some((s, said)) if self.dismissed[H_STYLE].as_deref() != Some(said.as_str()) => s,
            _ => st::auto_style(audience, genre),
        }
    }

    /// Start the model (or the agent) on `msgs`: the story streams into the page. `what` is the audit line.
    fn launch(&mut self, cx: &AppCx, msgs: Vec<Message>, what: &str) {
        let (label, note) = self.model_label(cx);
        let own_name = match self.model_plan(cx) {
            ModelPlan::Chosen(c) => Self::model_name(&c),
            ModelPlan::SameAsChat { .. } => Self::chat_choice(cx).map_or(label, |c| Self::model_name(&c)),
        };
        self.note = note.map(|n| (n, false));
        // an external agent writes it when one is chosen and usable; otherwise our own model, with the reason
        let ext = match self.agent_request(cx, &msgs) {
            Ok(x) => x,
            Err(why) => {
                let n = format!("{why} Writing it with {own_name} instead.");
                self.agent_note = Some(n.clone());
                self.note = Some((n, false));
                None
            }
        };
        self.run_model = ext.as_ref().map_or(own_name.clone(), |e| e.2.clone());
        let o = &self.story_opts;
        cx.audit.record(&cx.session.user, what, &format!("{} {} {} {} pictures {} topic {} chars model {}", o.audience.id(), o.genre.id(), o.length.id(), o.language, o.pictures.id(), o.topic.chars().count(), if ext.is_some() { format!("agent {}", self.prefs.agent.clone().unwrap_or_default()) } else { self.run_model_for_audit(cx) }));
        let write = self.step("Write", &format!("{} {} story in {}", o.length.label(), o.genre.label().to_lowercase(), o.language));
        self.write_call = Some(write);
        self.thinking.clear();
        self.think_since = Some(Instant::now());
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let (chat, stop2) = (self.backend(cx), stop.clone());
        std::thread::spawn(move || {
            fn own(chat: &Arc<dyn ChatBackend>, msgs: &[Message], tx: &Sender<GenMsg>, stop: &Arc<AtomicBool>) -> Result<String, String> {
                // no tools are ever passed: the model can only write
                chat.complete_tools_stream(msgs, &[], &mut |piece| {
                    if !piece.is_empty() {
                        let _ = tx.send(GenMsg::Delta(piece.to_string()));
                    }
                    !stop.load(Ordering::SeqCst)
                })
                .map(|t| t.text)
            }
            let r = match ext {
                None => own(&chat, &msgs, &tx, &stop2),
                Some((req, label, _shown, advisor)) => {
                    let h = iyakki_agent::extagent::run::start(req);
                    let (txs, txt) = (tx.clone(), tx.clone());
                    let got = st::follow_agent_run(
                        &h,
                        &|| stop2.load(Ordering::SeqCst),
                        &mut |t| {
                            let _ = txs.send(GenMsg::Snapshot(t.to_string()));
                        },
                        &mut |t| {
                            let _ = txt.send(GenMsg::Thinking(t.to_string()));
                        },
                    );
                    match got {
                        Ok(t) if !t.trim().is_empty() => Ok(t),
                        other if stop2.load(Ordering::SeqCst) => other.map(|_| String::new()),
                        other => {
                            let why = other.err().unwrap_or_else(|| "it sent no story".into());
                            let who = if advisor { format!("{label} runs as an advisor here (read-only) and could not write the story: {why}") } else { format!("{label} could not write the story: {why}") };
                            let _ = tx.send(GenMsg::Snapshot(String::new()));
                            let _ = tx.send(GenMsg::Note(format!("{who} Writing it with {own_name} instead.")));
                            let _ = tx.send(GenMsg::Who(own_name.clone()));
                            own(&chat, &msgs, &tx, &stop2)
                        }
                    }
                }
            };
            let _ = tx.send(GenMsg::Done(r));
        });
        self.gen = Some(Gen { rx, stop });
        self.auto_pending = true;
        self.page = Page::Story;
        self.focus = Focus::None;
        self.pane_open = false;
        self.stick_bottom = true;
        self.scroll_px = 0.0;
    }

    /// Correct the story on the page: the model gets the story as data with the requests so far and writes it again, keeping
    /// what the request does not touch; only the pictures whose scene changed are drawn again; the result is a new version.
    fn start_revision(&mut self, text: String, cx: &AppCx) {
        if self.gen.is_some() {
            return;
        }
        if text.trim().is_empty() {
            self.note = Some(("Say what to change, for example \"make it shorter\" or \"change the fox's name to Beeru\".".into(), false));
            return;
        }
        if let Some(req) = st::parse_picture_request(&text) {
            return self.redraw_by_request(req, &text, cx);
        }
        self.refresh_tts(cx);
        let earlier = self.user_requests();
        let mut o = self.story_opts.clone();
        let h = st::parse_revision_hints(&text);
        if let Some((l, _)) = h.length {
            o.length = l;
        }
        if let Some((l, _)) = h.language {
            o.language = l;
        }
        if let Some((g, _)) = h.genre {
            o.genre = g;
        }
        if let Some((t, _)) = h.tone {
            o.tone = t;
        }
        let new_style = h.style.is_some();
        if let Some((s, _)) = h.style {
            o.style = s;
        } else if h.genre.is_some() && self.story_opts.style == st::auto_style(self.story_opts.audience, self.story_opts.genre) {
            o.style = st::auto_style(o.audience, o.genre);
        }
        o.pictures = self.opts.pictures.resolve(!self.image_models.is_empty());
        let msgs = match st::revision_messages(&o, &self.story, &earlier, &text) {
            Ok(m) => m,
            Err(e) => {
                self.note = Some((e, false));
                return;
            }
        };
        self.reader.stop();
        self.pic_auto = self.opts.pictures == PictureMode::Auto && o.pictures == PictureMode::ImageModel && o.drawing != st::DrawingMode::Image;
        let rev = Rev { old_story: std::mem::take(&mut self.story), old_raw: std::mem::take(&mut self.raw), old_opts: std::mem::replace(&mut self.story_opts, o.clone()), old_pics: std::mem::take(&mut self.pics), new_style };
        self.rev = Some(rev);
        self.clear_view();
        self.pic_gen += 1;
        self.error = None;
        self.note = None;
        self.agent_note = None;
        self.convo.push(Message::user(text.clone()));
        let plan = self.step("Plan", &text);
        let plan_text = format!("Revise version {}: keep what the request does not touch. {}", self.ver, self.plan_text(&o, cx));
        self.step_done(&plan, true, &plan_text);
        self.launch(cx, msgs, "story.revise");
        self.idea.set("");
        self.sync_fields();
        self.dismissed = Default::default();
    }

    /// "Redraw picture 2 as a comic": only that picture is made again; the story text stays.
    fn redraw_by_request(&mut self, req: st::PictureRequest, text: &str, cx: &AppCx) {
        let idx = req.index - 1;
        if self.pics.is_empty() {
            self.note = Some(("There are no pictures to redraw (they are switched off, see Options).".into(), false));
            return;
        }
        if idx >= self.pics.len() {
            self.note = Some((format!("There is no picture {}: this story has {}.", req.index, self.pics.len()), false));
            return;
        }
        self.convo.push(Message::user(text.to_string()));
        if let Some(style) = req.style {
            self.pics[idx].style = style;
        }
        let fb = st::picture_feedback(text);
        if !fb.is_empty() {
            self.pics[idx].feedback = fb;
        }
        self.pics[idx].state = PicState::Queued;
        self.push_draw_calls(&[idx]);
        self.idea.set("");
        self.sync_fields();
        self.note = None;
        self.pump_pictures(cx);
        self.persist_chat();
    }

    /// The revision has been written (or stopped, or failed): keep it as the next version and match the pictures to it.
    fn finish_revision(&mut self, rev: Rev, stopped: bool, cx: &AppCx) {
        let write = self.write_call.take();
        if self.story.blocks.is_empty() {
            // nothing came back: the story stays as it was
            let why = self.error.take().unwrap_or_else(|| "the model returned no story".into());
            self.story = rev.old_story;
            self.raw = rev.old_raw;
            self.story_opts = rev.old_opts;
            self.pics = rev.old_pics;
            self.chunks = st::chunks_for(&self.story.units(), 0, st::MAX_CHUNK_CHARS);
            self.chunked_units = self.story.units().len();
            self.note = Some((format!("The story was not changed: {why}"), false));
            if let Some(c) = write {
                self.step_done(&c, false, &format!("the story was not changed: {why}"));
            }
            return;
        }
        let before = self.pics.len();
        let _ = before;
        self.reconcile_pictures(rev.old_pics, rev.new_style, cx);
        // a saved story keeps the earlier text as a version of its own; an unsaved one only moves on
        let mut saved_as: Option<String> = None;
        match self.saved.clone() {
            Some(e) => match self.store.save_revision(&e.id, &self.story, &self.story_opts) {
                Ok(n) => {
                    self.ver = n;
                    self.ver_count = n.max(self.ver_count);
                    if let Ok(d) = self.store.load(&e.id) {
                        self.saved = Some(d.entry);
                    }
                    saved_as = Some(format!("Saved as version {n} in Stories/{}/; version {} is kept.", e.id, n - 1));
                    cx.audit.record(&cx.session.user, "story.version", &format!("{} v{n}", e.id));
                }
                Err(err) => self.note = Some((format!("The new version could not be saved: {err}"), false)),
            },
            None => {
                self.ver += 1;
                self.ver_count = self.ver_count.max(self.ver);
            }
        }
        if self.saved.is_some() {
            self.persist_pictures();
        }
        let words = self.story.words();
        if let Some(c) = write {
            self.step_done(&c, true, &format!("Wrote \"{}\": {words} words{}. This is version {}.", self.display_title(), if stopped { " (stopped early)" } else { "" }, self.ver));
        }
        self.convo.push(Message::assistant(format!("Version {} is on the left{}. Keep telling me what to change; the earlier versions stay under \"Version\".", self.ver, if stopped { " (stopped where it was)" } else { "" })));
        if let Some(n) = saved_as {
            self.note = Some((format!("{}{n}", self.agent_note.take().map(|a| format!("{a} ")).unwrap_or_default()), true));
        }
    }

    /// Give the story the pictures that still fit (same scene text), draw the others, and say which in the conversation.
    fn reconcile_pictures(&mut self, old: Vec<Pic>, all_new: bool, cx: &AppCx) {
        self.pics.clear();
        self.pic_gen += 1;
        self.held = false;
        self.alt_jobs.clear();
        self.alt_running = None;
        if self.story_opts.pictures == PictureMode::Off {
            return;
        }
        let o = self.story_opts.clone();
        let title = if self.story.title.is_empty() { st::title_from_topic(&o.topic) } else { self.story.title.clone() };
        let scenes = illus::pick_scenes(st::scenes(&self.story, st::MAX_PICTURES), o.coverage);
        let old_scenes: Vec<st::Scene> = old.iter().map(|p| if p.state == PicState::Ready { p.scene.clone() } else { st::Scene { text: String::new(), ..p.scene.clone() } }).collect();
        let reuse = if all_new { vec![None; scenes.len()] } else { st::reuse_pictures(&old_scenes, &scenes) };
        let mut kept = 0;
        let mut fresh = Vec::new();
        for (sc, r) in scenes.into_iter().zip(reuse) {
            match r {
                Some(j) => {
                    let mut p = old[j].clone();
                    p.scene = sc;
                    p.call = None;
                    kept += 1;
                    self.pics.push(p);
                }
                None => {
                    let prompt = match o.pictures {
                        PictureMode::ImageModel => st::image_prompt(&title, &o, &sc),
                        _ => format!("SVG drawing: {}", st::image_prompt(&title, &o, &sc)),
                    };
                    fresh.push(self.pics.len());
                    self.pics.push(Pic { scene: sc, state: PicState::Queued, bytes: None, ext: "svg".into(), prompt, mode: o.pictures, style: o.style, key: self.pic_gen, call: None, how: String::new(), log: String::new(), feedback: String::new(), size: String::new(), look: o.pic.styles_list()[0], seed: None, provider: String::new(), model: String::new(), ms: 0, alts: Vec::new() });
                }
            }
        }
        if kept > 0 {
            let c = self.step("Keep", &format!("{kept} picture{}", if kept == 1 { "" } else { "s" }));
            self.step_done(&c, true, &format!("{kept} picture{} still fit{} the new text and {} kept; {} drawn again.", if kept == 1 { "" } else { "s" }, if kept == 1 { "s" } else { "" }, if kept == 1 { "is" } else { "are" }, fresh.len()));
        }
        self.queue_alts(&fresh);
        let job = if o.pictures == PictureMode::ImageModel { self.job_line(cx, fresh.len()) } else { None };
        let (_, secs) = self.job_totals(cx, fresh.len());
        let long = o.pictures == PictureMode::ImageModel && secs > self.confirm_secs && !fresh.is_empty();
        if (o.coverage == Coverage::Ask || long) && !fresh.is_empty() {
            // nothing is drawn until the person says yes: the text is complete and readable meanwhile
            self.held = true;
            let n = fresh.len();
            let total = job.unwrap_or_else(|| self.timing_line(cx, n));
            self.convo.push(Message::assistant(format!("I can draw {n} picture{} for this story. {total} Press \"Make the pictures\" to start, or \"No pictures\" to cancel.", if n == 1 { "" } else { "s" })));
            return;
        }
        self.push_draw_calls(&fresh);
        self.pump_pictures(cx);
    }

    /// Choose version `n` of the saved story again: its text comes back, and the pictures are matched to it.
    fn switch_version(&mut self, n: u32, cx: &AppCx) {
        if self.gen.is_some() {
            self.note = Some(("Wait until the story is written, or stop it, then choose a version.".into(), false));
            return;
        }
        let Some(id) = self.saved_id() else {
            self.note = Some(("Save the story first: versions are kept with a saved story.".into(), false));
            return;
        };
        if n == self.ver {
            return;
        }
        let doc = match self.store.select_version(&id, n) {
            Ok(d) => d,
            Err(e) => {
                self.note = Some((e, false));
                return;
            }
        };
        self.reader.stop();
        let old = std::mem::take(&mut self.pics);
        self.clear_view();
        self.story = doc.story.clone();
        self.raw = doc.story.markdown();
        self.story_opts = self.options_of(&doc.entry);
        self.story_opts.pictures = self.opts.pictures.resolve(!self.image_models.is_empty());
        self.story_opts.style = old.first().map_or_else(|| st::auto_style(self.story_opts.audience, self.story_opts.genre), |p| p.style);
        self.saved = Some(doc.entry.clone());
        self.ver = n;
        self.chunks = st::chunks_for(&self.story.units(), 0, st::MAX_CHUNK_CHARS);
        self.chunked_units = self.story.units().len();
        self.pic_auto = self.opts.pictures == PictureMode::Auto && self.story_opts.pictures == PictureMode::ImageModel && self.story_opts.drawing != st::DrawingMode::Image;
        self.reconcile_pictures(old, false, cx);
        self.persist_pictures();
        self.convo.push(Message::assistant(format!("Back to version {n} of {}. Your later versions are still there.", self.ver_count)));
        self.persist_chat();
        self.menu = None;
        cx.audit.record(&cx.session.user, "story.version", &format!("{id} switched to v{n}"));
        self.note = Some((format!("Version {n} is in use again."), true));
    }

    /// The options a saved story was made with (from its header).
    fn options_of(&self, e: &StoryEntry) -> StoryOptions {
        StoryOptions {
            topic: e.topic.clone(),
            audience: Audience::from_id(&e.audience).unwrap_or(Audience::Kids),
            genre: Genre::from_id(&e.genre).unwrap_or(Genre::Bedtime),
            length: Length::from_id(&e.length).unwrap_or(Length::Short),
            language: if e.language.is_empty() { "English".into() } else { e.language.clone() },
            tone: Tone::from_id(&e.tone).unwrap_or(Tone::Calm),
            characters: e.characters.clone(),
            pictures: PictureMode::Off,
            style: PictureStyle::default(),
            drawing: st::DrawingMode::default(),
            video: false,
            coverage: self.opts.coverage,
            pic: self.store.load_picture_opts(&e.id).unwrap_or_else(|| illus::PictureOpts { seed: None, ..self.opts.pic.clone() }),
        }
    }

    /// Keep the conversation beside the saved story.
    fn persist_chat(&mut self) {
        if let Some(id) = self.saved_id() {
            let _ = self.store.save_chat(&id, &st::StoryChat { version: self.ver, messages: self.convo.clone() });
        }
    }

    /// A new story: the page, the pictures and the conversation start empty.
    fn new_story(&mut self, cx: &AppCx) {
        if self.gen.is_some() {
            self.stop_writing();
        }
        self.queued = None;
        self.reader.stop();
        self.reset_story();
        self.ver = 1;
        self.ver_count = 1;
        self.open_form(cx);
    }

    fn run_model_for_audit(&self, cx: &AppCx) -> String {
        match self.model_plan(cx) {
            ModelPlan::Chosen(c) => format!("{} {}", c.provider, c.model),
            ModelPlan::SameAsChat { .. } => "same as chat".to_string(),
        }
    }

    /// Empty the story view (the text, the reading, the layout) without touching the conversation or the saved story.
    fn clear_view(&mut self) {
        self.raw.clear();
        self.story = Story::default();
        self.chunks.clear();
        self.chunked_units = 0;
        self.live = false;
        self.saved_audio = None;
        self.auto_pending = false;
        self.tscroll = 0.0;
        self.manual_unit = None;
        self.pane.clear();
        self.lay.clear();
        self.pics.clear();
        self.sheet = None;
        self.setting = None;
        self.held = false;
        self.pic_gen += 1;
        if let Some(j) = self.audio_job.take() {
            j.cancel.store(true, Ordering::SeqCst);
        }
    }

    fn reset_story(&mut self) {
        self.run_model.clear();
        self.agent_note = None;
        self.saved = None;
        self.error = None;
        self.note = None;
        self.convo.clear();
        self.tool_ok.clear();
        self.write_call = None;
        self.rev = None;
        self.thinking.clear();
        self.scroll_px = 0.0;
        self.pic_pending.clear();
        self.pic_attempts.clear();
        self.clear_view();
    }

    fn stop_writing(&mut self) {
        if let Some(g) = &self.gen {
            g.stop.store(true, Ordering::SeqCst);
        }
    }

    /// Take what the model has sent so far.
    fn poll_writing(&mut self, cx: &AppCx) {
        let mut finished: Option<Result<String, String>> = None;
        if let Some(g) = &self.gen {
            while let Ok(m) = g.rx.try_recv() {
                match m {
                    GenMsg::Delta(t) => self.raw.push_str(&t),
                    GenMsg::Snapshot(t) => self.raw = t,
                    GenMsg::Thinking(t) => self.thinking = t,
                    GenMsg::Who(w) => self.run_model = w,
                    GenMsg::Note(n) => {
                        self.agent_note = Some(n.clone());
                        self.note = Some((n, false));
                    }
                    GenMsg::Done(r) => {
                        finished = Some(r);
                        break;
                    }
                }
            }
        } else {
            return;
        }
        let stopped = self.gen.as_ref().is_some_and(|g| g.stop.load(Ordering::SeqCst));
        if let Some(r) = finished {
            self.gen = None;
            match r {
                Ok(full) => {
                    if !full.trim().is_empty() {
                        self.raw = full;
                    }
                }
                Err(e) if self.raw.trim().is_empty() => self.error = Some(e),
                Err(e) => self.error = Some(format!("The model stopped early: {e}")),
            }
            self.story = st::parse_story(&self.raw, true);
            if self.story.blocks.is_empty() && self.error.is_none() {
                self.error = Some("The model returned no story. Try again, or choose another model (Settings → Models).".into());
            }
            self.thinking.clear();
            self.think_since = None;
            self.feed_reader(true);
            match self.rev.take() {
                Some(rev) => self.finish_revision(rev, stopped, cx),
                None => {
                    if !self.story.blocks.is_empty() {
                        self.start_pictures(cx);
                        if self.prefs.auto_save {
                            self.save_story(cx, if stopped { "Stopped early. " } else { "" });
                        } else {
                            self.note = Some((format!("{}{}The story is not saved yet. Press Save story to keep it.", self.agent_note.take().map(|n| format!("{n} ")).unwrap_or_default(), if stopped { "Stopped early. " } else { "" }), true));
                        }
                    }
                    if let Some(c) = self.write_call.take() {
                        if self.story.blocks.is_empty() {
                            let why = self.error.clone().unwrap_or_else(|| "no story came back".into());
                            self.step_done(&c, false, &why);
                        } else {
                            let t = format!("Wrote \"{}\": {} words{}.", self.display_title(), self.story.words(), if stopped { " (stopped early)" } else { "" });
                            self.step_done(&c, true, &t);
                            self.convo.push(Message::assistant("Version 1 is on the left. Tell me what to change: shorter, a different name, funnier, another language, or \"redraw picture 2 as a comic\"."));
                        }
                    }
                }
            }
            self.persist_chat();
            if let Some(q) = self.queued.take() {
                self.idea.set(&q);
                self.sync_fields();
                self.submit(cx);
            }
        } else {
            self.story = st::parse_story(&self.raw, false);
            self.feed_reader(false);
        }
    }

    /// Give the reader the paragraphs that are finished; start it by itself when "read while writing" is on.
    fn feed_reader(&mut self, done: bool) {
        let units = self.story.units();
        if units.len() <= self.chunked_units && !(done && self.live) {
            return;
        }
        let fresh = st::chunks_for(&units, self.chunked_units, st::MAX_CHUNK_CHARS);
        self.chunked_units = units.len();
        if fresh.is_empty() && !done {
            return;
        }
        if self.live {
            if !fresh.is_empty() {
                self.chunks.extend(fresh.clone());
                self.reader.append(fresh);
            }
            if done {
                self.reader.close_input();
            }
        } else {
            self.chunks.extend(fresh);
        }
    }

    fn maybe_autostart(&mut self, cx: &AppCx) {
        if !self.auto_pending {
            return;
        }
        if self.live || !self.prefs.read_while_writing || !matches!(self.tts, TtsStatus::Ready { .. }) {
            self.auto_pending = false;
        } else if !self.chunks.is_empty() {
            self.auto_pending = false;
            self.start_reading(0, cx);
        } else if self.gen.is_none() {
            self.auto_pending = false;
        }
    }

    // ------------------------------------------------------------------------------------------------ saving

    /// Save the story into `Stories/<title>/` with what is ready of its pictures and its audio, and show it in Listen again.
    fn save_story(&mut self, cx: &AppCx, prefix: &str) {
        if self.story.blocks.is_empty() {
            self.note = Some(("There is no story to save yet.".into(), false));
            return;
        }
        let entry = match self.saved.clone() {
            Some(e) => e,
            None => match self.store.save(&self.story, &self.story_opts, now_secs()) {
                Ok(e) => {
                    cx.audit.record(&cx.session.user, "story.save", &e.id);
                    // a story that was revised before it was saved starts its kept versions from this text
                    self.ver = 1;
                    self.ver_count = 1;
                    e
                }
                Err(e) => {
                    self.note = Some((format!("The story could not be saved: {e}"), false));
                    return;
                }
            },
        };
        self.saved = Some(entry.clone());
        let mut parts = vec!["text".to_string()];
        let n = self.persist_pictures();
        if n > 0 {
            parts.push(format!("{n} picture{}", if n == 1 { "" } else { "s" }));
        }
        if self.persist_audio_if_ready() {
            parts.push("audio".into());
        }
        if let Ok(d) = self.store.load(&entry.id) {
            self.saved = Some(d.entry);
        }
        let said = self.agent_note.take().map(|n| format!("{n} ")).unwrap_or_default();
        self.note = Some((format!("{said}{prefix}Saved in Stories/{}/ ({}).", entry.id, parts.join(", ")), true));
        self.persist_chat();
    }

    /// The other pictures of a scene as the store keeps them.
    fn stored_variants(p: &Pic) -> Vec<StoredVariant> {
        p.alts.iter().map(|a| StoredVariant { ext: a.ext.clone(), bytes: a.bytes.as_ref().clone(), style: a.look.id().into(), seed: a.seed, provider: a.provider.clone(), model: a.model.clone(), size: a.size.clone(), ms: a.ms, prompt: a.prompt.clone() }).collect()
    }

    /// Keep the pictures that are ready next to the saved story (their places are those of the saved text).
    fn persist_pictures(&mut self) -> usize {
        let Some(id) = self.saved_id() else { return 0 };
        // a story the model gave no title is saved with one, which shifts every unit by one
        let shift = usize::from(self.story.title.is_empty());
        let list: Vec<StoredPic> = self
            .pics
            .iter()
            .filter(|p| p.state == PicState::Ready)
            .filter_map(|p| {
                Some(StoredPic {
                    variants: Self::stored_variants(p),
                    look: p.look.id().into(),
                    seed: p.seed,
                    provider: p.provider.clone(),
                    model: p.model.clone(),
                    ms: p.ms,
                    size: p.size.clone(),
                    source: p.how.clone(),
                    scene_text: p.scene.text.clone(),
                    unit: p.scene.place_before + shift,
                    title: p.scene.title.clone(),
                    prompt: p.prompt.clone(),
                    style: p.style.id().into(),
                    mode: p.mode.id().into(),
                    ext: p.ext.clone(),
                    bytes: p.bytes.as_ref()?.as_ref().clone(),
                })
            })
            .collect();
        let mut log = String::new();
        for (i, p) in self.pics.iter().enumerate().filter(|(_, p)| !p.log.is_empty()) {
            log.push_str(&format!("######## Picture {} ({}): {}\nstyle {} · made as: {}\n{}\n", i + 1, p.scene.title, p.how, p.style.id(), p.ext, p.log));
        }
        self.flush_pic_log();
        let _ = self.store.save_picture_opts(&id, &self.story_opts.pic);
        let _ = self.store.save_notes(&id, self.sheet.as_ref().map(|c| c.to_json()).as_deref(), &log);
        if let Some(t) = &self.setting {
            let _ = self.store.save_setting(&id, t);
        }
        match self.store.save_pictures(&id, &list) {
            Ok(()) => list.len(),
            Err(e) => {
                self.note = Some((format!("The pictures could not be saved: {e}"), false));
                0
            }
        }
    }

    /// Save the audio when every piece of the story has already been spoken (nothing is made just for this).
    fn persist_audio_if_ready(&mut self) -> bool {
        let Some(id) = self.saved_id() else { return false };
        if !self.live || self.gen.is_some() {
            return false;
        }
        if self.browser_voice || self.direct.is_some() {
            return false; // the browser (or the engine) said it aloud; there is nothing to keep
        }
        let (chunks, wavs) = self.reader.sounds();
        if chunks.is_empty() || chunks.len() != self.chunks.len() || wavs.iter().any(|w| w.is_none()) {
            return false;
        }
        let all: Vec<Arc<Vec<u8>>> = wavs.into_iter().flatten().collect();
        match st::assemble_audio_timed(&chunks, &all).and_then(|(wav, starts)| self.store.save_audio_indexed(&id, &wav, &chunks, &starts)) {
            Ok(_) => true,
            Err(_) => false,
        }
    }

    // ------------------------------------------------------------------------------------------------ pictures

    /// Plan the illustrations of the finished story (one per scene) and start making them one after another in the
    /// background; reading never waits for them.
    fn start_pictures(&mut self, cx: &AppCx) {
        self.reconcile_pictures(Vec::new(), true, cx);
    }

    fn pump_pictures(&mut self, cx: &AppCx) {
        if self.held || self.alt_running.is_some() || self.pics.iter().any(|p| p.state == PicState::Working) {
            return;
        }
        // the main pictures first; then the extra ones (other styles and seeds), scene by scene
        let mut alt: Option<AltJob> = None;
        let idx = match self.pics.iter().position(|p| p.state == PicState::Queued) {
            Some(i) => i,
            None => {
                while let Some(j) = self.alt_jobs.pop_front() {
                    if self.pics.get(j.idx).is_some_and(|p| p.state == PicState::Ready && p.mode == PictureMode::ImageModel) {
                        alt = Some(j);
                        break;
                    }
                }
                let Some(j) = alt else { return };
                j.idx
            }
        };
        if self.pics[idx].mode == PictureMode::ImageModel {
            if let Some(why) = self.low_memory(cx) {
                // no picture job starts while the machine is short of memory; the text is untouched and Try again works later
                let mut evs = Vec::new();
                for i in 0..self.pics.len() {
                    if self.pics[i].state == PicState::Queued && self.pics[i].mode == PictureMode::ImageModel {
                        self.pics[i].state = PicState::Failed(why.clone());
                        self.pic_result(i, false, &why);
                        evs.push(self.pic_decision(plk::SKIPPED, i, &format!("skipped: free memory low. {why}")));
                    }
                }
                if alt.is_some() {
                    evs.push(self.pic_decision(plk::SKIPPED, idx, &format!("skipped (another picture of this scene): free memory low. {why}")));
                    self.alt_jobs.clear();
                }
                self.log_pic_events(evs);
                self.note = Some((why, false));
                self.dirty = true;
                return;
            }
        }
        if alt.is_none() {
            self.pics[idx].state = PicState::Working;
        } else {
            self.alt_running = Some(idx);
        }
        let (gen, tx) = (self.pic_gen, self.pic_tx.clone());
        let title = if self.story.title.is_empty() { st::title_from_topic(&self.story_opts.topic) } else { self.story.title.clone() };
        let (mut o, scene, mode) = (self.story_opts.clone(), self.pics[idx].scene.clone(), self.pics[idx].mode);
        o.style = self.pics[idx].style;
        let (look, variant_n) = alt.map_or((self.pics[idx].look, 0), |j| (j.look, j.n));
        o.pic.look = look;
        let svc = cx.services.clone();
        let drawers = self.drawers(cx);
        let workdir = cx.services.dir().join("tales-agent");
        let (story_text, known_sheet, feedback, known_setting) = (self.story.plain(), self.sheet.clone(), if alt.is_some() { String::new() } else { self.pics[idx].feedback.clone() }, self.setting.clone());
        // the model that draws is named (the chosen one, else the first found) so its name is kept with the picture
        let resolved = self.resolved_look(&o.pic, look, cx);
        let picture_model = resolved.as_ref().map(|r| (r.provider.clone(), r.model.clone()));
        let family = illus::Family::of(&resolved.as_ref().map_or(String::new(), |r| r.model.clone()));
        // a new seed for every picture, or the story's own when it keeps one look (made with the first picture and kept)
        let attempt = {
            let n = self.pic_attempts.entry(idx).or_insert(0);
            *n += 1;
            *n
        };
        // one seed family per scene: every style of it starts from the same seed, each further picture of a style from another
        let base = match self.scene_seed.get(&idx).copied().filter(|_| alt.is_some()) {
            Some(b) => b,
            None => {
                let b = illus::seed_for(&o.pic, illus::fresh_seed(idx as u64 * 31 + attempt as u64));
                self.scene_seed.insert(idx, b);
                b
            }
        };
        let seed = illus::PictureOpts::variant_seed(base, variant_n);
        let (rsteps, rsize, rguid) = resolved.as_ref().map_or((None, None, None), |r| (r.steps, Some((r.width, r.height)), r.guidance));
        let realistic = o.pic.is_realistic();
        if o.pic.keep_look && self.story_opts.pic.seed.is_none() {
            self.story_opts.pic.seed = Some(base);
            if let Some(id) = self.saved_id() {
                let _ = self.store.save_picture_opts(&id, &self.story_opts.pic);
            }
        }
        let story_name = title.clone();
        self.pic_started.insert(idx, Instant::now());
        self.pic_target = picture_model.clone();
        self.last_wait_log = None;
        cx.audit.record(&cx.session.user, "story.picture", &format!("{} scene {}", mode.id(), idx + 1));
        std::thread::spawn(move || {
            // the character sheet is made once per story, by the first drawer, and kept
            let mut sheet = known_sheet;
            let mut made_sheet = None;
            let mut made_setting: Option<String> = None;
            let mut need_sheet = |drawer: &Drawer| -> CharacterSheet {
                if let Some(s) = &sheet {
                    return s.clone();
                }
                let s = scene::sheet::character_sheet(drawer.chat.as_ref(), &o, &story_text);
                made_sheet = Some(s.clone());
                sheet = Some(s.clone());
                s
            };
            let mut events: Vec<PicEvent> = Vec::new();
            let mut missing: Option<MissingModel> = None;
            let res: Result<PicOut, String> = match mode {
                PictureMode::ImageModel => 'img: {
                    // ask the server now whether the model is on it: a call for a model that is not downloaded would start a
                    // download of several GB inside the call, with no progress and no end in sight
                    if let Some((p, m)) = &picture_model {
                        if svc.provider_is_lemonade(p) {
                            let size = match svc.lemonade_live_model(p, m) {
                                Ok(Some(info)) if !info.downloaded => Some(info.size_text()),
                                Ok(None) => Some("size unknown".to_string()),
                                _ => None,
                            };
                            if let Some(size) = size {
                                let text = illus::not_downloaded_text(m, p, &size);
                                let mut ev = piclog::decision_event(now_secs(), plk::NOT_DOWNLOADED, &story_name, idx + 1, &scene.title, &format!("{text} Nothing was asked of the server."));
                                (ev.provider, ev.model, ev.attempt) = (p.clone(), m.clone(), attempt);
                                events.push(ev);
                                missing = Some(MissingModel { provider: p.clone(), model: m.clone(), size, text: text.clone() });
                                break 'img Err(text);
                            }
                        }
                    }
                    let sh = need_sheet(&drawers[0]);
                    // the story model writes the setting note once and a short visual brief for this scene; when it cannot,
                    // the scene's own text is used, so a picture is never lost to this step
                    let setting = known_setting.clone().or_else(|| {
                        let n = drawers[0].chat.complete(&illus::setting_messages(&title, &o, &story_text)).ok().and_then(|t| illus::clean_description(&t));
                        made_setting = n.clone();
                        n
                    });
                    let brief = drawers[0].chat.complete(&illus::visual_messages_for(&title, &o, &scene, &sh, setting.as_deref().unwrap_or(""), Some(family))).ok().and_then(|t| illus::clean_description(&t));
                    let mut desc = brief.unwrap_or_else(|| scene.text.chars().take(300).collect());
                    if !feedback.trim().is_empty() {
                        desc.push_str(&format!(" Change requested by the person: {}.", feedback.trim()));
                    }
                    let built = illus::compose(&o, &scene, Some(&desc), &sh, setting.as_deref().unwrap_or(""), Some(family));
                    let prompt = built.prompt.clone();
                    let (fw, fh) = family.fallback_size();
                    let params = iyakki_agent::providers::ImageParams { seed: Some(seed), negative: illus::API_TAKES_NEGATIVE_PROMPT.then(|| built.negative.clone()), verbose: o.pic.verbose, steps: rsteps, size: rsize, guidance: rguid };
                    let (r, trace) = svc.generate_image_traced(picture_model.as_ref().map(|(p, m)| (p.as_str(), m.as_str())), &prompt, &format!("{fw}x{fh}"), &params);
                    // what the picture model sends is judged by its first bytes, never by its claimed type: size caps, a pixel
                    // cap from the header, a real decode, metadata stripped
                    let mut rejected = None;
                    let out = match r {
                        Ok((_, _, bytes, _)) => match scene::reply::validate(bytes, o.video, "the picture model") {
                            Ok(f) => {
                                let size = scene::reply::dimensions(&f.bytes, f.ext).map(|(w, h)| format!("{w}x{h}")).unwrap_or_default();
                                let log = format!("=== image model ===\nPROMPT\n{prompt}\nNEGATIVE (sent: {})\n{}\nSEED {seed}\nRESULT: {} accepted\n", built.negative_sent, built.negative, f.ext);
                                Ok(PicOut { bytes: f.bytes, ext: f.ext.to_string(), prompt: prompt.clone(), how: "image".into(), who: picture_model.as_ref().map_or("the picture model".into(), |m| m.1.clone()), note: None, log, sheet: None, setting: None, size, made: Some((look, Some(seed), trace.provider.clone(), trace.model.clone(), trace.ms)) })
                            }
                            Err(e) => {
                                rejected = Some(e.clone());
                                Err(e)
                            }
                        },
                        Err(e) => Err(e),
                    };
                    let info = piclog::RequestInfo { ts: now_secs(), story: &story_name, picture: idx + 1, scene: &scene.title, attempt, prompt: &prompt, negative: &built.negative, negative_sent: built.negative_sent, verbose: o.pic.verbose, style: look.id(), variant: variant_n + 1, realistic, guidance: rguid };
                    events.push(piclog::request_event(&trace, &info, rejected.as_deref(), ""));
                    out
                }
                _ => {
                    // the writer draws (a rich SVG when it is strong, else it picks a scene for the built-in cartoon); when the
                    // chosen agent cannot be reached the person's own model takes over
                    let mut last = "no drawing".to_string();
                    let mut out = None;
                    for d in &drawers {
                        let sh = need_sheet(d);
                        match scene::draw_picture(d.chat.as_ref(), &title, &o, &scene, &sh, &scene::DrawCfg { strong: d.strong, story_text: &story_text, feedback: &feedback, workdir: Some(workdir.as_path()), allow_video: o.video }) {
                            Ok(x) => {
                                out = Some(PicOut { bytes: x.bytes, ext: x.ext.into(), prompt: x.prompt, how: x.how.into(), who: d.who.clone(), note: x.note, log: x.log, sheet: None, setting: None, size: String::new(), made: None });
                                break;
                            }
                            Err(e) => last = format!("{}: {e}", d.who),
                        }
                    }
                    out.ok_or(last)
                }
            };
            let res = res.map(|mut r| {
                r.sheet = made_sheet.take();
                r.setting = made_setting.take();
                r
            });
            let _ = tx.send(PicMsg { gen, alt, idx, res, events, missing });
        });
    }

    /// Who draws: the external agent that was chosen to write (when it is usable), then the writing model, each with whether
    /// it is strong enough to draw a rich SVG by itself. The agent is tried first and the model is the fallback.
    fn drawers(&self, cx: &AppCx) -> Vec<Drawer> {
        let mut v = Vec::new();
        if self.prefs.agent.is_some() {
            if let Ok(Some((req, label, _, _))) = self.agent_request(cx, &[]) {
                let chat: Arc<dyn ChatBackend> = Arc::new(scene::agentchat::AgentChat { agent: req.agent, workspace: req.workspace, model: req.model, key: req.key, audit: req.audit, on_usage: req.on_usage, exe: req.exe });
                v.push(Drawer { chat, strong: true, who: label });
            }
        }
        let model = match self.model_plan(cx) {
            ModelPlan::Chosen(c) => c.model,
            ModelPlan::SameAsChat { .. } => Self::chat_choice(cx).map_or(String::new(), |c| c.model),
        };
        v.push(Drawer { chat: self.backend(cx), strong: scene::rich::is_strong(&model, false), who: if model.is_empty() { "the chat model".into() } else { model } });
        #[cfg(test)]
        if let Some(f) = &self.test_drawers {
            return f(v);
        }
        v
    }

    fn poll_pictures(&mut self, cx: &AppCx) {
        self.poll_install(cx);
        self.update_wait(cx);
        let mut changed = false;
        while let Ok(mut m) = self.pic_rx.try_recv() {
            // the log keeps what was asked even when the answer comes after a cancel
            let evs = std::mem::take(&mut m.events);
            self.log_pic_events(evs);
            if m.gen != self.pic_gen || m.idx >= self.pics.len() {
                continue;
            }
            self.pic_started.remove(&m.idx);
            if m.missing.is_some() {
                self.alt_running = None;
                self.alt_jobs.clear();
            }
            if let Some(miss) = m.missing.take() {
                for i in 0..self.pics.len() {
                    if (m.alt.is_none() && i == m.idx) || (self.pics[i].state == PicState::Queued && self.pics[i].mode == PictureMode::ImageModel) {
                        self.pics[i].state = PicState::Failed(miss.text.clone());
                        self.pic_result(i, false, &miss.text);
                    }
                }
                self.note = Some((miss.text.clone(), false));
                self.install = Some(Install { provider: miss.provider, model: miss.model, size: miss.size, started: false });
                changed = true;
                continue;
            }
            // one more picture of a scene (another style or seed): it goes into the scene's gallery, never over the main one
            if let Some(job) = m.alt {
                self.alt_running = None;
                match m.res {
                    Ok(out) => match self.cap_error_alt(out.bytes.len() as u64) {
                        Ok(()) => {
                            let (look, seed, provider, model, ms) = out.made.clone().unwrap_or((job.look, None, String::new(), out.who.clone(), 0));
                            let key = hash(&format!("{}{}{}{}", self.pic_gen, m.idx, job.look.id(), out.bytes.len())) ^ out.bytes.len() as u64;
                            self.pics[m.idx].alts.push(Alt { look, seed, provider, model, size: out.size.clone(), ms, ext: out.ext.clone(), prompt: out.prompt.clone(), key, bytes: Arc::new(out.bytes) });
                            self.note = Some((format!("Another picture of scene {} is ready ({}).", m.idx + 1, look.label()), true));
                        }
                        Err(e) => {
                            // the limit: nothing that exists is dropped; the rest of the job stops, and it says so
                            let left = self.alt_jobs.len();
                            self.alt_jobs.clear();
                            let ev = self.pic_decision(plk::NOT_KEPT, m.idx, &format!("not kept: {e}. The {left} pictures still to be drawn were stopped."));
                            self.log_pic_events(vec![ev]);
                            self.note = Some((format!("Stopped: {e}. Your pictures are all still there; {left} more were not drawn."), false));
                        }
                    },
                    Err(e) => self.note = Some((format!("Another picture of scene {} could not be drawn: {e}", m.idx + 1), false)),
                }
                changed = true;
                continue;
            }
            match m.res {
                Ok(out) => {
                    if let Err(e) = self.cap_error(m.idx, out.bytes.len() as u64) {
                        let why = format!("Not kept: {e}.");
                        let ev = self.pic_decision(plk::NOT_KEPT, m.idx, &format!("not kept: {e}"));
                        self.log_pic_events(vec![ev]);
                        self.pics[m.idx].state = PicState::Failed(why.clone());
                        self.pic_result(m.idx, false, &why);
                        changed = true;
                        continue;
                    }
                    if self.setting.is_none() && out.setting.is_some() {
                        self.setting = out.setting.clone();
                        if let Some(id) = self.saved_id() {
                            let _ = self.store.save_setting(&id, self.setting.as_deref().unwrap_or(""));
                        }
                    }
                    if self.sheet.is_none() {
                        self.sheet = out.sheet.clone();
                    }
                    let how = match out.how.as_str() {
                        "image" => format!("images via {}", out.who),
                        "rich svg" => format!("rich SVG drawing by {}", out.who),
                        "image reply" => format!("image by {}", out.who),
                        "video reply" => format!("video clip by {}", out.who),
                        "composed" => "cartoon (built-in)".to_string(),
                        "model svg" => "an SVG drawing".to_string(),
                        _ => "built-in sketch".to_string(),
                    };
                    let said = out.note.clone().map_or(format!("Picture {} is ready ({how}).", m.idx + 1), |n| format!("Picture {} is ready ({how}). {n}", m.idx + 1));
                    let p = &mut self.pics[m.idx];
                    p.key = p.key.wrapping_add(1).wrapping_mul(1_000_003) ^ out.bytes.len() as u64;
                    p.bytes = Some(Arc::new(out.bytes));
                    p.ext = out.ext;
                    p.prompt = out.prompt;
                    p.how = how;
                    p.size = out.size;
                    if let Some((look, seed, provider, model, ms)) = out.made {
                        (p.look, p.seed, p.provider, p.model, p.ms) = (look, seed, provider, model, ms);
                    }
                    p.log = out.log;
                    p.state = PicState::Ready;
                    self.pic_result(m.idx, true, &said);
                }
                Err(e) if self.pic_auto && self.pics[m.idx].mode == PictureMode::ImageModel => {
                    // images are preferred; when they do not work this scene and the rest are drawn as SVG, said once
                    self.pic_auto = false;
                    self.alt_jobs.clear();
                    self.pic_result(m.idx, false, &format!("image generation did not work: {e}"));
                    let mut ev = self.pic_decision(plk::SVG_FALLBACK, m.idx, "this picture and the rest are drawn as SVG");
                    ev.svg_fallback = format!("image generation did not work: {e}");
                    self.log_pic_events(vec![ev]);
                    let title = if self.story.title.is_empty() { st::title_from_topic(&self.story_opts.topic) } else { self.story.title.clone() };
                    let mut again = Vec::new();
                    for i in 0..self.pics.len() {
                        if i == m.idx || self.pics[i].state == PicState::Queued {
                            let mut o = self.story_opts.clone();
                            o.style = self.pics[i].style;
                            let q = &mut self.pics[i];
                            q.mode = PictureMode::Drawings;
                            q.state = PicState::Queued;
                            q.prompt = format!("SVG drawing: {}", st::image_prompt(&title, &o, &q.scene));
                            q.call = None;
                            again.push(i);
                        }
                    }
                    self.push_draw_calls(&again);
                    self.note = Some((format!("Image generation did not work ({e}), so the remaining pictures are drawn as SVG instead."), false));
                }
                Err(e) => {
                    self.pics[m.idx].state = PicState::Failed(e.clone());
                    self.pic_result(m.idx, false, &e);
                }
            }
            changed = true;
        }
        if changed {
            // a story that is saved keeps its pictures up to date
            if self.saved.is_some() {
                self.persist_pictures();
            }
            self.persist_chat();
            self.pump_pictures(cx);
            self.dirty = true;
        }
    }

    fn redo_picture(&mut self, idx: usize, cx: &AppCx) {
        let Some(p) = self.pics.get_mut(idx) else { return };
        if p.state == PicState::Working {
            return;
        }
        p.state = PicState::Queued;
        self.held = false;
        self.push_draw_calls(&[idx]);
        self.pump_pictures(cx);
    }

    /// Stop the picture job: the one being made is dropped when it comes back, the waiting ones are not started. The text and
    /// the finished pictures stay; a cancelled picture can be tried again.
    fn cancel_pictures(&mut self) {
        self.pic_gen += 1;
        self.held = false;
        self.alt_jobs.clear();
        self.alt_running = None;
        let mut evs = Vec::new();
        for i in 0..self.pics.len() {
            if matches!(self.pics[i].state, PicState::Working | PicState::Queued) {
                evs.push(self.pic_decision(plk::SKIPPED, i, if self.pics[i].state == PicState::Working { "cancelled while it was being drawn (an answer that comes later is only logged)" } else { "cancelled before it started" }));
                self.pics[i].state = PicState::Failed("cancelled".into());
                self.pic_result(i, false, "cancelled");
            }
        }
        self.log_pic_events(evs);
        if self.saved.is_some() {
            self.persist_pictures();
        }
        self.persist_chat();
        self.note = Some(("The pictures were cancelled; the story is as it was.".into(), true));
        self.dirty = true;
    }

    /// Remove one picture from the story (and from its folder when saved).
    fn delete_picture(&mut self, idx: usize) {
        if idx >= self.pics.len() {
            return;
        }
        if self.pics.iter().any(|p| p.state == PicState::Working) {
            self.note = Some(("Wait for the picture being drawn, or press Cancel pictures, then delete.".into(), false));
            return;
        }
        self.pics.remove(idx);
        if self.saved.is_some() {
            self.persist_pictures();
        }
        self.note = Some(("The picture was deleted.".into(), true));
        self.dirty = true;
    }

    // ------------------------------------------------------------------------------------------------ waiting, time limit, install

    /// The time limit for one picture, the pause between health calls and between "waiting" lines of the log.
    fn timing(&self) -> (Duration, Duration, Duration) {
        self.test_timing.unwrap_or((Duration::from_secs(60 * self.pic_now().limit_min() as u64), Duration::from_secs(5), Duration::from_secs(30)))
    }

    fn secs_text(d: Duration) -> String {
        let s = d.as_secs();
        if s >= 120 { format!("{} min {} s", s / 60, s % 60) } else { format!("{s} s") }
    }

    /// While a picture is being drawn: how long it has taken, what the server says (asked every few seconds), a line in the log
    /// now and then, and the time limit.
    fn update_wait(&mut self, cx: &AppCx) {
        while let Ok(r) = self.health_rx.try_recv() {
            self.health_busy = false;
            let target = self.pic_target.as_ref().map_or(String::new(), |t| t.1.clone());
            self.server_state = match r {
                Ok((text, loaded)) => format!("server: {text}; {target} is {}", if loaded.iter().any(|m| *m == target) { "loaded" } else { "not loaded yet (loading, or downloading)" }),
                Err(e) => format!("server: its health call failed ({e})"),
            };
        }
        let Some(idx) = self.pics.iter().position(|p| p.state == PicState::Working).or(self.alt_running) else {
            self.wait_text = None;
            self.server_state.clear();
            return;
        };
        let Some(t0) = self.pic_started.get(&idx).copied() else { return };
        let el = t0.elapsed();
        let (limit, every, log_every) = self.timing();
        if el >= limit {
            self.time_out(idx, el, limit);
            return;
        }
        if let Some((p, _)) = self.pic_target.clone().filter(|(p, _)| cx.services.provider_is_lemonade(p)) {
            if !self.health_busy && self.health_last.is_none_or(|l| l.elapsed() >= every) {
                self.health_busy = true;
                self.health_last = Some(Instant::now());
                let (svc, tx) = (cx.services.clone(), self.health_tx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(svc.lemonade_health(&p));
                });
            }
        }
        let state = if self.server_state.is_empty() { String::new() } else { format!(" ({})", self.server_state) };
        let text = format!("Drawing picture {}: {} so far. The model is loading or generating; on a CPU this can take minutes. The limit is {}; Cancel pictures stops it{state}.", idx + 1, Self::secs_text(el), Self::secs_text(limit));
        self.wait_text = Some(text);
        if self.last_wait_log.is_none_or(|l| l.elapsed() >= log_every) && el >= log_every.min(Duration::from_secs(30)) {
            self.last_wait_log = Some(Instant::now());
            let mut ev = self.pic_decision(plk::WAITING, idx, &format!("waiting {}{}", Self::secs_text(el), if self.server_state.is_empty() { String::new() } else { format!(" · {}", self.server_state) }));
            if let Some((p, m)) = &self.pic_target {
                (ev.provider, ev.model) = (p.clone(), m.clone());
            }
            self.log_pic_events(vec![ev]);
        }
        self.dirty = true;
    }

    /// The picture took longer than the limit: stop waiting for it (an answer that comes later is dropped, and only logged).
    fn time_out(&mut self, idx: usize, el: Duration, limit: Duration) {
        let why = format!("Stopped after {} (the time limit for a picture is {}). The server may still be working on it; try again, choose a smaller picture model, or raise the limit in Options.", Self::secs_text(el), Self::secs_text(limit));
        let mut evs = vec![self.pic_decision(plk::TIMEOUT, idx, &format!("timeout after {}: {}", Self::secs_text(el), if self.server_state.is_empty() { "no server state" } else { &self.server_state }))];
        if let Some((p, m)) = &self.pic_target {
            (evs[0].provider, evs[0].model) = (p.clone(), m.clone());
        }
        self.pic_gen += 1;
        self.held = false;
        self.alt_jobs.clear();
        self.alt_running = None;
        for i in 0..self.pics.len() {
            if matches!(self.pics[i].state, PicState::Working | PicState::Queued) {
                self.pics[i].state = PicState::Failed(why.clone());
                self.pic_result(i, false, &why);
            }
        }
        self.pic_started.clear();
        self.log_pic_events(std::mem::take(&mut evs));
        if self.saved.is_some() {
            self.persist_pictures();
        }
        self.persist_chat();
        self.note = Some((why, false));
        self.dirty = true;
    }

    /// Start the download of the picture model that is not on the server (the same download as Settings → Models, with progress).
    fn start_install(&mut self, cx: &AppCx) {
        let Some((p, m, size)) = self.install.as_ref().filter(|i| !i.started).map(|i| (i.provider.clone(), i.model.clone(), i.size.clone())) else { return };
        match cx.services.pull_model(&p, &m) {
            Ok(()) => {
                if let Some(i) = self.install.as_mut() {
                    i.started = true;
                }
                let mut ev = piclog::decision_event(now_secs(), plk::PULL, &self.story_name(), 0, "", &format!("pull started: {m} on {p} ({size})"));
                (ev.provider, ev.model) = (p, m);
                self.log_pic_events(vec![ev]);
                cx.audit.record(&cx.session.user, "story.picture_install", &self.pic_log_name());
            }
            Err(e) => self.note = Some((format!("The download could not start: {e}"), false)),
        }
    }

    /// Follow the download: progress as a note, then, when it is there, the waiting pictures are drawn.
    fn poll_install(&mut self, cx: &AppCx) {
        let Some((p, m, started)) = self.install.as_ref().map(|i| (i.provider.clone(), i.model.clone(), i.started)) else { return };
        if !started {
            return;
        }
        let Some(st) = cx.services.pull_status().into_iter().find(|s| s.provider == p && s.model == m) else { return };
        let mut log = |a: &mut StoryApp, d: String| {
            let mut ev = piclog::decision_event(now_secs(), plk::PULL, &a.story_name(), 0, "", &d);
            (ev.provider, ev.model) = (p.clone(), m.clone());
            a.log_pic_events(vec![ev]);
        };
        if let Some(e) = &st.error {
            self.install = self.install.take().map(|i| Install { started: false, ..i });
            log(self, format!("pull failed: {e}"));
            self.note = Some((format!("The download of {m} failed: {e}"), false));
        } else if st.done {
            self.install = None;
            self.image_models = cx.services.image_models();
            log(self, "pull finished".into());
            self.note = Some((format!("{m} is installed. Drawing the pictures now."), true));
            let again: Vec<usize> = (0..self.pics.len()).filter(|&i| self.pics[i].mode == PictureMode::ImageModel && matches!(&self.pics[i].state, PicState::Failed(w) if w.contains("is not downloaded"))).collect();
            for &i in &again {
                self.pics[i].state = PicState::Queued;
            }
            self.held = false;
            self.push_draw_calls(&again);
            self.pump_pictures(cx);
        } else {
            let pct = if st.total > 0 { format!(" {}%", st.completed * 100 / st.total) } else { String::new() };
            self.note = Some((format!("Downloading {m}{pct}: {}", st.describe()), true));
        }
        self.dirty = true;
    }

    /// The picture choices in force: the story's when one is on the page, else the defaults.
    fn pic_now(&self) -> illus::PictureOpts {
        if self.story.is_empty() && self.gen.is_none() { self.opts.pic.clone() } else { self.story_opts.pic.clone() }
    }

    /// The picture models that are there, as (provider, model).
    fn found_images(&self, cx: &AppCx) -> Vec<(String, String)> {
        if self.image_models.is_empty() { cx.services.image_models() } else { self.image_models.clone() }
    }

    /// The picture model that `pic` leads to (the story's own choice, Quality, the selected one) with its size and steps.
    fn resolved_for(&self, pic: &illus::PictureOpts, cx: &AppCx) -> Option<illus::Resolved> {
        self.resolved_look(pic, pic.styles_list()[0], cx)
    }

    /// [`Self::resolved_for`] for drawing in `look` (the Photorealistic style takes the most realistic installed model).
    fn resolved_look(&self, pic: &illus::PictureOpts, look: illus::Look, cx: &AppCx) -> Option<illus::Resolved> {
        let cands = cx.services.picture_choices(&self.found_images(cx));
        let avail = self.memory_now().filter(|m| m.total > 0).map(|m| m.available >> 20);
        illus::resolve_for(pic, look, self.prefs.picture_model.as_ref(), &cands, avail)
    }

    /// Change a picture choice for the defaults and for the story on the page, and keep it (with the story when it is saved).
    fn set_pic(&mut self, f: impl Fn(&mut illus::PictureOpts)) {
        f(&mut self.opts.pic);
        self.opts.pic.seed = None;
        if !self.story.is_empty() {
            f(&mut self.story_opts.pic);
            if !self.story_opts.pic.keep_look {
                self.story_opts.pic.seed = None;
            }
            if let Some(id) = self.saved_id() {
                let _ = self.store.save_picture_opts(&id, &self.story_opts.pic);
            }
        }
        self.save_prefs();
        self.dirty = true;
    }

    /// The next picture model for this story: automatic, then each picture model of every provider, round and round.
    fn cycle_story_model(&mut self, cx: &AppCx) {
        let found: Vec<(String, String)> = cx.services.picture_choices(&self.found_images(cx)).into_iter().map(|c| (c.provider, c.model)).collect();
        let cur = self.pic_now().model;
        let at = cur.and_then(|c| found.iter().position(|(p, m)| *p == c.provider && *m == c.model));
        let next = match at {
            None if self.pic_now().model.is_none() => found.first(),
            None => None,
            Some(i) => found.get(i + 1),
        };
        let next = next.map(|(p, m)| ModelChoice { provider: p.clone(), model: m.clone() });
        self.set_pic(move |o| o.model = next.clone());
    }

    /// Note picture log events; they go to the story's folder as soon as the story is saved.
    fn log_pic_events(&mut self, evs: Vec<PicEvent>) {
        if evs.is_empty() {
            return;
        }
        self.pic_pending.extend(evs);
        self.flush_pic_log();
    }

    fn flush_pic_log(&mut self) {
        let Some(id) = self.saved_id() else {
            let n = self.pic_pending.len();
            if n > 500 {
                self.pic_pending.drain(..n - 500);
            }
            return;
        };
        if !self.pic_pending.is_empty() && self.store.append_picture_log(&id, &self.pic_pending).is_ok() {
            self.pic_pending.clear();
        }
    }

    fn pic_log_name(&self) -> String {
        self.install.as_ref().map_or(String::new(), |i| format!("{} {}", i.provider, i.model))
    }

    fn story_name(&self) -> String {
        if self.story.title.is_empty() { st::title_from_topic(&self.story_opts.topic) } else { self.story.title.clone() }
    }

    /// A decision about picture `idx` for the log (skipped, not kept, cancelled).
    fn pic_decision(&self, kind: &str, idx: usize, decision: &str) -> PicEvent {
        piclog::decision_event(now_secs(), kind, &self.story_name(), idx + 1, self.pics.get(idx).map_or("", |p| p.scene.title.as_str()), decision)
    }

    /// Every picture log event of the story on the page, oldest first.
    fn pic_log_events(&self) -> Vec<PicEvent> {
        let mut v = self.saved_id().map(|id| self.store.load_picture_log(&id)).unwrap_or_default();
        v.extend(self.pic_pending.iter().cloned());
        v
    }

    fn open_pic_log(&mut self) {
        self.menu = None;
        self.focus = Focus::None;
        self.modal = Some(Modal::PicLog(PicLogView { top: Cell::new(usize::MAX), at_end: Cell::new(true), total: Cell::new(0), rows: Cell::new(0), confirm_clear: false }));
    }

    /// Scroll the picture log by `d` wrapped lines (negative: towards the oldest).
    fn log_scroll(&mut self, d: i64) {
        if let Some(Modal::PicLog(v)) = &self.modal {
            let max = v.total.get().saturating_sub(v.rows.get());
            let at = v.top.get().min(max) as i64;
            let to = (at + d).clamp(0, max as i64) as usize;
            v.top.set(to);
            v.at_end.set(to == max);
            self.dirty = true;
        }
    }

    /// The machine's memory now (tests set their own).
    fn memory_now(&self) -> Option<iyakki_agent::sysinfo::Memory> {
        #[cfg(test)]
        return self.test_memory;
        #[cfg(not(test))]
        Some(iyakki_agent::sysinfo::memory(&iyakki_agent::sysinfo::SysRoots::real()))
    }

    /// The picture model that would be used: the chosen one, else the first found, with its Lemonade entry when it has one.
    fn picture_info(&self, cx: &AppCx) -> Option<iyakki_agent::lemonade::Model> {
        let r = self.resolved_for(&self.pic_now(), cx)?;
        cx.services.lemonade_model(&r.provider, &r.model)
    }

    /// Whether the picture step can run now, and why not.
    fn readiness(&self, cx: &AppCx) -> Readiness {
        let found = self.found_images(cx);
        // the memory of this computer matters only for a model that loads on this computer
        let remote = self.resolved_for(&self.pic_now(), cx).is_some_and(|r| r.remote);
        illus::readiness(&found, &cx.services.lemonade_images_missing(), self.picture_info(cx).as_ref(), if remote { None } else { self.memory_now() }.as_ref())
    }

    /// The sentence about memory when a picture job must wait, else `None`.
    fn low_memory(&self, cx: &AppCx) -> Option<String> {
        match self.readiness(cx) {
            r @ Readiness::LowMemory { .. } => illus::reason(&r),
            _ => None,
        }
    }

    /// The honest timing line for `n` pictures with the picture model that would be used.
    fn timing_line(&self, cx: &AppCx, n: usize) -> String {
        let info = self.picture_info(cx);
        let name = self.resolved_for(&self.pic_now(), cx).map_or("The picture model".to_string(), |r| r.model);
        let cpu_only = !iyakki_agent::aiprofile::this_machine(0).capable();
        illus::timing_text(&name, info.and_then(|m| m.image_defaults), n, cpu_only)
    }

    /// May a picture of `new` bytes be kept (the story's cap and the cap over all stories)?
    fn cap_error(&self, idx: usize, new: u64) -> Result<(), String> {
        // every picture of the story counts, the extra ones of a scene too (the one being replaced does not)
        let story: u64 = self.story_pic_bytes(Some(idx));
        let all = if self.saved.is_some() { self.store.pictures_bytes().max(story) } else { story };
        illus::check_caps(story, all, new)
    }

    /// May one more picture of `new` bytes be kept beside the others?
    fn cap_error_alt(&self, new: u64) -> Result<(), String> {
        let story = self.story_pic_bytes(None);
        let all = if self.saved.is_some() { self.store.pictures_bytes().max(story) } else { story };
        illus::check_caps(story, all, new)
    }

    /// The bytes of the pictures of the story on the page: main and extra ones, without the main one of `skip`.
    fn story_pic_bytes(&self, skip: Option<usize>) -> u64 {
        self.pics
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let main = if Some(i) != skip && p.state == PicState::Ready { p.bytes.as_ref().map_or(0, |b| b.len() as u64) } else { 0 };
                main + p.alts.iter().map(|a| a.bytes.len() as u64).sum::<u64>()
            })
            .sum()
    }

    // ------------------------------------------------------------------------------------------------ reading

    fn synth(&self, cx: &AppCx) -> Arc<dyn st::Synth> {
        match &self.saved_audio {
            Some(p) => p.clone(),
            None if self.browser_voice => Arc::new(hostvoice::BrowserSynth),
            None if self.direct.is_some() => Arc::new(hostengines::DirectSynth),
            None => self.file_synth(cx),
        }
    }

    /// The engine writing audio files (what Save audio uses, also for an engine that speaks by itself when reading).
    fn file_synth(&self, cx: &AppCx) -> Arc<dyn st::Synth> {
        Arc::new(st::ServicesSynth { svc: cx.services.clone(), opts: SpeakOpts { voice: self.prefs.voice.clone(), speed: self.prefs.speed, lang: Some(hostvoice::lang_tag(&self.story_opts.language).to_string()) } })
    }

    fn start_reading(&mut self, from: usize, cx: &AppCx) {
        if !self.can_read() {
            self.note = Some((st::status_text(&self.tts), false));
            return;
        }
        if self.chunks.is_empty() {
            self.chunks = st::chunks_for(&self.story.units(), 0, st::MAX_CHUNK_CHARS);
            self.chunked_units = self.story.units().len();
        }
        if self.chunks.is_empty() {
            return;
        }
        cx.audit.record(&cx.session.user, "story.read", &format!("{} parts", self.chunks.len()));
        self.highlight = cx.services.voice_settings().highlight;
        self.reader.set_words(self.highlight.word());
        self.reader.set_speed(self.prefs.speed);
        let sink: Arc<dyn st::Sink> = if self.browser_voice && self.saved_audio.is_none() {
            Arc::new(hostvoice::BrowserSink { hv: HostVoice::current(), voice: self.prefs.voice.clone().filter(|v| !v.is_empty()), lang: Some(hostvoice::lang_tag(&self.story_opts.language).to_string()), rate: self.prefs.speed })
        } else if let (Some(spec), true) = (self.direct.clone(), self.saved_audio.is_none()) {
            Arc::new(hostengines::DirectSink { spec: hostengines::Spec { voice: self.prefs.voice.clone().filter(|v| !v.is_empty()).or(spec.voice.clone()), ..spec }, lang: Some(hostvoice::lang_tag(&self.story_opts.language).to_string()), speed: self.prefs.speed })
        } else {
            let play_cmd = cx.services.voice_settings().play_cmd;
            Arc::new(st::CommandSink { play_cmd: hostengines::player_command(&play_cmd, &path_env()).unwrap_or(play_cmd) })
        };
        self.reader.start(self.chunks.clone(), from, self.gen.is_some(), &self.voice_key(), self.synth(cx), sink);
        self.live = true;
        self.manual_unit = None;
        self.note = None;
    }

    /// A click in the text at (`xr`, `yr`) relative to the text area: read from the sentence under it.
    fn read_from_point(&mut self, xr: f32, yr: f32, cx: &AppCx) {
        let Some((line, ch)) = self.pane.locate(xr, yr) else { return };
        let Some(&(unit, start)) = self.line_map.get(line) else { return };
        self.read_from(unit, start + ch, cx);
    }

    /// Start reading at the sentence that holds character `pos` of unit `unit`. A piece holds several sentences, so the piece
    /// is cut in two at that sentence (not with saved audio, whose sound is made per piece).
    fn read_from(&mut self, unit: usize, pos: usize, cx: &AppCx) {
        if !self.can_read() {
            self.note = Some((st::status_text(&self.tts), false));
            return;
        }
        if self.chunks.is_empty() {
            self.chunks = st::chunks_for(&self.story.units(), 0, st::MAX_CHUNK_CHARS);
            self.chunked_units = self.story.units().len();
        }
        let Some(mut idx) = self.chunks.iter().position(|c| c.unit == unit && c.start <= pos && pos < c.end).or_else(|| self.chunks.iter().position(|c| c.unit == unit && c.start >= pos)).or_else(|| self.chunks.iter().rposition(|c| c.unit == unit)) else { return };
        if self.saved_audio.is_none() {
            let units = self.story.units();
            if let Some(text) = units.get(unit) {
                let chars: Vec<char> = text.chars().collect();
                let c = self.chunks[idx].clone();
                let sentences = st::sentence_ranges(text);
                if let Some(k) = sentences.iter().position(|(a, b)| *a <= pos && pos < *b).or_else(|| sentences.iter().position(|(a, _)| *a >= pos)) {
                    let (sa, _) = sentences[k];
                    if sa > c.start && sa < c.end && k > 0 {
                        let before_end = sentences[k - 1].1.min(sa);
                        let cut = |a: usize, b: usize| Chunk { unit, start: a, end: b, text: chars[a..b.min(chars.len())].iter().collect() };
                        let (left, right) = (cut(c.start, before_end), cut(sa, c.end.min(chars.len())));
                        if !left.text.trim().is_empty() && !right.text.trim().is_empty() {
                            self.chunks.splice(idx..=idx, [left, right]);
                            idx += 1;
                        }
                    }
                }
            }
        }
        self.start_reading(idx, cx);
    }

    fn play_pause(&mut self, cx: &AppCx) {
        match self.reader.status() {
            Status::Playing | Status::Buffering => self.reader.pause(),
            Status::Paused => self.reader.resume(),
            _ => {
                if self.live && self.reader.snapshot().chunks > 0 && !matches!(self.reader.status(), Status::Done | Status::Failed(_)) {
                    self.reader.seek(0);
                } else {
                    self.start_reading(0, cx);
                }
            }
        }
    }

    fn seek(&mut self, chunk: usize, cx: &AppCx) {
        if self.chunks.is_empty() {
            return;
        }
        let chunk = chunk.min(self.chunks.len() - 1);
        if self.live {
            self.reader.seek(chunk);
            self.manual_unit = None;
        } else {
            self.start_reading(chunk, cx);
        }
    }

    fn step_unit(&mut self, d: i32, cx: &AppCx) {
        if self.chunks.is_empty() {
            return;
        }
        let cur = self.reader.snapshot().chunk.min(self.chunks.len() - 1);
        let unit = self.chunks[cur].unit;
        let first_of = |u: usize| self.chunks.iter().position(|c| c.unit == u);
        let target = if d > 0 {
            self.chunks.iter().position(|c| c.unit > unit)
        } else if first_of(unit) != Some(cur) {
            first_of(unit)
        } else {
            self.chunks[..cur].last().and_then(|c| first_of(c.unit))
        };
        if let Some(t) = target {
            self.seek(t, cx);
        } else if d < 0 {
            self.seek(0, cx);
        }
    }

    fn stop_reading(&mut self) {
        self.reader.stop();
    }

    fn cycle_voice(&mut self, cx: &AppCx) {
        if self.voices.len() < 2 {
            return;
        }
        let cur = self.prefs.voice.clone().unwrap_or_else(|| self.voices[0].clone());
        let i = self.voices.iter().position(|v| *v == cur).unwrap_or(0);
        self.prefs.voice = Some(self.voices[(i + 1) % self.voices.len()].clone());
        self.after_voice_change(cx);
    }

    fn cycle_speed(&mut self, up: bool, cx: &AppCx) {
        let i = SPEEDS.iter().position(|v| (*v - self.prefs.speed).abs() < 0.01).unwrap_or(2);
        let n = SPEEDS.len();
        self.prefs.speed = SPEEDS[if up { (i + 1) % n } else { (i + n - 1) % n }];
        self.after_voice_change(cx);
    }

    fn after_voice_change(&mut self, cx: &AppCx) {
        // the saved sound has one voice: a different choice means the engine speaks again
        self.saved_audio = None;
        self.save_prefs();
        if self.live && matches!(self.reader.status(), Status::Playing | Status::Buffering | Status::Paused) {
            let cur = self.reader.snapshot().chunk;
            self.start_reading(cur, cx);
        } else {
            self.live = false;
        }
    }

    fn save_audio(&mut self, cx: &AppCx) {
        if self.gen.is_some() {
            self.note = Some(("Wait until the story is finished.".into(), false));
            return;
        }
        if self.no_audio_file && self.saved_audio.is_none() {
            self.note = Some((if self.browser_voice { SAVE_AUDIO_BROWSER } else { SAVE_AUDIO_PLAYONLY }.into(), false));
            return;
        }
        if !matches!(self.tts, TtsStatus::Ready { .. }) {
            self.note = Some((st::status_text(&self.tts), false));
            return;
        }
        if self.saved.is_none() {
            self.save_story(cx, "");
        }
        let Some(e) = self.saved.clone() else { return };
        if self.audio_job.is_some() {
            return;
        }
        if self.chunks.is_empty() {
            self.chunks = st::chunks_for(&self.story.units(), 0, st::MAX_CHUNK_CHARS);
        }
        let (chunks, have) = {
            let (c, w) = self.reader.sounds();
            if self.live && self.direct.is_none() && c.len() == self.chunks.len() && c.iter().zip(&self.chunks).all(|(a, b)| a.text == b.text) { (c, w) } else { (self.chunks.clone(), vec![None; self.chunks.len()]) }
        };
        let (tx, rx) = channel();
        let (cancel, done) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        let (c2, d2) = (cancel.clone(), done.clone());
        let synth = if self.saved_audio.is_none() { self.file_synth(cx) } else { self.synth(cx) };
        let store = self.store.clone();
        let id = e.id.clone();
        let total = chunks.len();
        cx.audit.record(&cx.session.user, "story.audio", &format!("{total} parts"));
        std::thread::spawn(move || {
            let r = st::render_audio(&chunks, &have, synth.as_ref(), &c2, &d2).and_then(|(wav, starts)| store.save_audio_indexed(&id, &wav, &chunks, &starts).map(|p| p.display().to_string()));
            let _ = tx.send(r);
        });
        self.audio_job = Some(AudioJob { rx, cancel, done, total });
        self.note = None;
    }

    // ------------------------------------------------------------------------------------------------ the library

    fn reload_library(&mut self) {
        self.lib = if self.search.value.trim().is_empty() { self.store.list() } else { self.store.search(&self.search.value) };
        self.lib_sel = self.lib_sel.min(self.lib.len().saturating_sub(1));
    }

    fn open_saved(&mut self, id: &str, listen: bool, cx: &AppCx) {
        let doc = match self.store.load(id) {
            Ok(d) => d,
            Err(e) => {
                self.note = Some((e, false));
                return;
            }
        };
        self.stop_writing();
        self.gen = None;
        self.reader.stop();
        self.reset_story();
        self.story = doc.story.clone();
        self.raw = doc.story.markdown();
        self.story_opts = self.options_of(&doc.entry);
        self.saved = Some(doc.entry.clone());
        // the conversation that was kept with it, so it can go on (or one made up from the idea for stories saved without)
        self.ver_count = self.store.versions(id).max(1);
        match self.store.load_chat(id) {
            Some(c) if !c.messages.is_empty() => {
                self.convo = c.messages;
                self.ver = c.version.clamp_safe(1, self.ver_count);
            }
            _ => {
                self.ver = self.ver_count;
                self.convo = vec![Message::user(doc.entry.topic.clone()), Message::assistant(format!("Opened from Listen again (version {}). Tell me what to change.", self.ver))];
            }
        }
        self.step_seq = self.convo.len() + 1;
        self.tool_ok = self.convo.iter().filter(|m| m.role == iyakki_agent::chat::Role::Tool).filter_map(|m| m.tool_call_id.clone().map(|id| (id, !m.content.starts_with("Failed:")))).collect();
        // the pictures that were kept, in their places
        self.sheet = self.store.load_character(id);
        self.setting = self.store.load_setting(id);
        for p in self.store.load_pictures(id) {
            let mode = PictureMode::from_id(&p.mode).unwrap_or(PictureMode::Drawings);
            let style = PictureStyle::from_id(&p.style).unwrap_or_default();
            let key = hash(&format!("{id}{}{}", p.title, p.bytes.len()));
            let alts: Vec<Alt> = p.variants.iter().map(|v| Alt { look: illus::Look::from_id(&v.style).unwrap_or_default(), seed: v.seed, provider: v.provider.clone(), model: v.model.clone(), size: v.size.clone(), ms: v.ms, ext: v.ext.clone(), key: hash(&format!("{id}{}{}{}", p.title, v.style, v.bytes.len())), bytes: Arc::new(v.bytes.clone()), prompt: v.prompt.clone() }).collect();
            self.pics.push(Pic { scene: st::Scene { place_before: p.unit, title: p.title, text: p.scene_text }, state: PicState::Ready, bytes: Some(Arc::new(p.bytes)), ext: p.ext, prompt: p.prompt, mode, style, key, call: None, how: String::new(), log: String::new(), feedback: String::new(), size: p.size, look: illus::Look::from_id(&p.look).unwrap_or_default(), seed: p.seed, provider: p.provider.clone(), model: p.model.clone(), ms: p.ms, alts });
        }
        self.story_opts.style = self.pics.first().map_or_else(|| st::auto_style(self.story_opts.audience, self.story_opts.genre), |p| p.style);
        self.chunks = st::chunks_for(&self.story.units(), 0, st::MAX_CHUNK_CHARS);
        self.chunked_units = self.story.units().len();
        self.saved_audio = if doc.entry.has_audio {
            self.store.load_audio_pieces(id).ok().filter(|p| p.len() == self.chunks.len() && p.iter().zip(&self.chunks).all(|(a, b)| a.0.text == b.text)).map(|p| Arc::new(st::PrebuiltSynth::new(&p)))
        } else {
            None
        };
        self.page = Page::Story;
        self.focus = Focus::None;
        self.pane_open = false;
        self.refresh_tts(cx);
        if listen {
            self.start_reading(0, cx);
        }
    }

    fn toggle_fav(&mut self, id: &str) {
        let now = self.store.load(id).map(|d| d.entry.favourite).unwrap_or(false);
        match self.store.set_favourite(id, !now) {
            Ok(()) => {
                if let Some(s) = self.saved.as_mut().filter(|s| s.id == id) {
                    s.favourite = !now;
                }
                self.reload_library();
                self.note = Some((if now { "Removed from favourites.".into() } else { "Added to favourites.".into() }, true));
            }
            Err(e) => self.note = Some((e, false)),
        }
    }

    fn delete_saved(&mut self, id: &str, cx: &AppCx) {
        match self.store.delete(id) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "story.delete", id);
                if self.saved.as_ref().is_some_and(|s| s.id == id) {
                    self.saved = None;
                    self.saved_audio = None;
                }
                self.reload_library();
                self.note = Some(("Story deleted.".into(), true));
            }
            Err(e) => self.note = Some((e, false)),
        }
    }

    /// Ask what to export (the page with its pictures, the text, the audio) and hand the choice to the shell's save dialog.
    fn ask_export(&mut self, id: Option<String>) {
        if id.is_none() && self.story.blocks.is_empty() {
            self.note = Some(("There is no story to export yet.".into(), false));
            return;
        }
        let mut items = vec!["Page with pictures (.html)".to_string(), "Text (.md)".to_string(), "Audio (.wav)".to_string()];
        let has_alts = match &id {
            Some(i) => self.store.load_pictures(i).iter().any(|p| !p.variants.is_empty()),
            None => self.pics.iter().any(|p| !p.alts.is_empty()),
        };
        if has_alts {
            items.push("Page with every picture variant (.html)".to_string());
        }
        self.modal = Some(Modal::Export(Picker::new("Export to the computer", items), id));
    }

    /// What the export of `id` (or of the story on the page) needs: the story, its options and its pictures.
    fn export_source(&self, id: Option<&str>) -> Result<(Story, StoryOptions, Vec<StoredPic>, Option<String>), String> {
        match id {
            Some(id) => {
                let d = self.store.load(id)?;
                let o = StoryOptions {
                    topic: d.entry.topic.clone(),
                    audience: Audience::from_id(&d.entry.audience).unwrap_or(Audience::Kids),
                    genre: Genre::from_id(&d.entry.genre).unwrap_or(Genre::Bedtime),
                    language: if d.entry.language.is_empty() { "English".into() } else { d.entry.language.clone() },
                    ..StoryOptions::default()
                };
                Ok((d.story, o, self.store.load_pictures(id), Some(id.to_string())))
            }
            None => {
                let shift = usize::from(self.story.title.is_empty());
                let pics: Vec<StoredPic> = self
                    .pics
                    .iter()
                    .filter(|p| p.state == PicState::Ready)
                    .filter_map(|p| Some(StoredPic { variants: Self::stored_variants(p), look: p.look.id().into(), seed: p.seed, provider: p.provider.clone(), model: p.model.clone(), ms: p.ms, size: p.size.clone(), source: p.how.clone(), scene_text: p.scene.text.clone(), unit: p.scene.place_before + shift * 0, title: p.scene.title.clone(), prompt: p.prompt.clone(), style: p.style.id().into(), mode: p.mode.id().into(), ext: p.ext.clone(), bytes: p.bytes.as_ref()?.as_ref().clone() }))
                    .collect();
                Ok((self.story.clone(), self.story_opts.clone(), pics, self.saved_id()))
            }
        }
    }

    fn do_export(&mut self, what: usize, id: Option<String>) {
        let (story, opts, pics, saved) = match self.export_source(id.as_deref()) {
            Ok(x) => x,
            Err(e) => {
                self.note = Some((e, false));
                return;
            }
        };
        let name = saved.clone().unwrap_or_else(|| {
            let s = st::slug(&story.title);
            if s.is_empty() { "story".into() } else { s }
        });
        match what {
            0 => self.host_save = Some((format!("{name}.html"), st::export_html(&story, &opts, &pics).into_bytes())),
            3 => self.host_save = Some((format!("{name}-all-pictures.html"), st::export_html_with(&story, &opts, &pics, true).into_bytes())),
            1 => {
                // a saved story goes out as it is stored (with its picture lines); one not saved yet, as it is on the screen
                let md = saved.as_deref().and_then(|i| self.store.raw_markdown(i).ok()).unwrap_or_else(|| story.markdown());
                self.host_save = Some((format!("{name}.md"), md.into_bytes()));
            }
            _ => match saved.as_deref().and_then(|i| self.store.audio_path(i)) {
                Some(p) => match std::fs::read(&p) {
                    Ok(b) => self.host_save = Some((format!("{name}.{}", p.extension().map_or("wav".into(), |e| e.to_string_lossy().into_owned())), b)),
                    Err(e) => self.note = Some((format!("could not read the audio: {e}"), false)),
                },
                None => self.note = Some(("This story has no saved audio yet. Use Save audio first.".into(), false)),
            },
        }
    }

    fn copy_story(&mut self) {
        let t = self.pane.selected_text().unwrap_or_else(|| self.story.plain());
        if !t.is_empty() {
            crate::ui::clipboard::copy(&t);
        }
    }

    fn apply_quick(&mut self, i: usize, cx: &AppCx) {
        let Some(q) = self.quick.get(i).cloned() else { return };
        let r = cx.services.settings.lock().map_err(|e| e.to_string()).and_then(|mut s| s.update(|x| x.voice.tts = Some(q.engine.clone())).map_err(|e| e.to_string()));
        match r {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "story.engine", &q.label);
                self.note = Some((format!("{} is now the voice for Text to speech (Settings → Voice).", q.label.trim_start_matches("Use ")), true));
            }
            Err(e) => self.note = Some((format!("Could not change the voice settings: {e}"), false)),
        }
        self.refresh_tts(cx);
    }

    // ------------------------------------------------------------------------------------------------ background results

    fn poll(&mut self, cx: &AppCx) {
        self.poll_turn_on(cx);
        self.poll_writing(cx);
        self.poll_pictures(cx);
        self.maybe_autostart(cx);
        if let Some(j) = &self.audio_job {
            if let Ok(r) = j.rx.try_recv() {
                self.audio_job = None;
                match r {
                    Ok(p) => {
                        self.note = Some((format!("Audio saved: {p}"), true));
                        if let Some(e) = self.saved.clone() {
                            if let Ok(d) = self.store.load(&e.id) {
                                self.saved = Some(d.entry);
                            }
                        }
                    }
                    Err(e) if e == "cancelled" => self.note = Some(("Saving the audio was cancelled.".into(), false)),
                    Err(e) => self.note = Some((format!("The audio could not be saved: {e}"), false)),
                }
            }
        }
        if self.ticks % 45 == 0 {
            self.refresh_tts(cx);
        }
        let target = if matches!(self.reader.status(), Status::Playing) { self.reader.level() } else { 0.0 };
        self.level = if target > self.level { target } else { self.level * 0.8 };
        let now = self.reader.status();
        if now != self.last_status {
            self.last_status = now;
            self.dirty = true;
        }
        if let Status::Failed(m) = self.reader.status() {
            if self.error.as_deref() != Some(m.as_str()) {
                self.error = Some(m);
            }
        }
    }

    fn animating(&self) -> bool {
        self.gen.is_some() || self.audio_job.is_some() || self.level > 0.01 || self.pics.iter().any(|p| matches!(p.state, PicState::Working | PicState::Queued)) || self.alt_running.is_some() || !self.alt_jobs.is_empty() || matches!(self.reader.status(), Status::Playing | Status::Buffering)
    }

    // ------------------------------------------------------------------------------------------------ clicks and keys

    fn click(&mut self, n: usize, cx: &AppCx) {
        match n {
            T_OPTIONS => {
                if self.wide {
                    self.set_focus(Focus::Idea);
                } else {
                    self.pane_open = !self.pane_open;
                    self.focus = if self.pane_open { Focus::Idea } else { Focus::None };
                }
            }
            T_STORY_PAGE => self.go(Page::Story, cx),
            T_LIB_PAGE => self.go(Page::Library, cx),
            n if (T_AUDIENCE..T_AUDIENCE + Audience::ALL.len()).contains(&n) => {
                self.opts.audience = Audience::ALL[n - T_AUDIENCE];
                self.save_prefs();
            }
            n if (T_GENRE..T_GENRE + Genre::ALL.len()).contains(&n) => {
                self.opts.genre = Genre::ALL[n - T_GENRE];
                self.explicit[H_GENRE] = true;
                self.save_prefs();
                if self.menu == Some(Menu::Kind) {
                    self.close_menu();
                }
            }
            n if (T_LENGTH..T_LENGTH + Length::ALL.len()).contains(&n) => {
                self.opts.length = Length::ALL[n - T_LENGTH];
                self.explicit[H_LEN] = true;
                self.save_prefs();
            }
            n if (T_LANG..T_LANG + LANGUAGES.len()).contains(&n) => {
                self.opts.language = LANGUAGES[n - T_LANG].to_string();
                self.explicit[H_LANG] = true;
                self.save_prefs();
                if self.menu == Some(Menu::Language) {
                    self.close_menu();
                }
            }
            n if (T_TONE..T_TONE + Tone::ALL.len()).contains(&n) => {
                self.opts.tone = Tone::ALL[n - T_TONE];
                self.explicit[H_TONE] = true;
                self.save_prefs();
                if self.menu == Some(Menu::Tone) {
                    self.close_menu();
                }
            }
            T_KIND_MENU => self.toggle_menu(Menu::Kind, cx),
            T_TONE_MENU => self.toggle_menu(Menu::Tone, cx),
            T_OPTIONS_MENU => self.toggle_menu(Menu::Options, cx),
            T_LANG_MENU => self.toggle_menu(Menu::Language, cx),
            T_VERSION_MENU => self.toggle_menu(Menu::Version, cx),
            n if (T_VERSION_PICK + 1..T_VERSION_PICK + 60).contains(&n) => {
                self.menu = None;
                self.switch_version((n - T_VERSION_PICK) as u32, cx);
            }
            T_HOW => self.toggle_menu(Menu::How, cx),
            T_TURN_ON => self.turn_on_reading(cx),
            T_MENU_CLOSE => self.close_menu(),
            n if (T_HINT..T_HINT + 5).contains(&n) => {
                // the person removed a suggestion: it is not used (and not shown) until the words change
                let k = n - T_HINT;
                if let Some(c) = self.hint_chips().into_iter().find(|c| c.k == k) {
                    self.dismissed[k] = Some(c.said);
                }
                self.set_focus(Focus::Idea);
            }
            // the one switch for pictures: off, or automatic (images when a picture model exists, else SVG drawings)
            T_PICMODE => {
                self.opts.pictures = if self.opts.pictures == PictureMode::Off { PictureMode::Auto } else { PictureMode::Off };
                self.save_prefs();
            }
            // which scenes get a picture: every chapter, the cover only, or ask each time (chosen before Send, kept with the story)
            T_COVERAGE => {
                self.opts.coverage = self.opts.coverage.next();
                self.save_prefs();
            }
            n if (T_STYLE..T_STYLE + illus::Look::ALL.len()).contains(&n) => {
                let l = illus::Look::ALL[n - T_STYLE];
                self.set_pic(move |o| o.toggle_style(l));
            }
            T_PERSTYLE => self.set_pic(|o| o.next_count()),
            T_REALISTIC => {
                let on = !self.pic_now().is_realistic();
                self.set_pic(move |o| o.set_realistic(on));
            }
            T_REAL_INSTALL => {
                let pic = self.pic_now();
                let cands = cx.services.picture_choices(&self.found_images(cx));
                if let Some((p, m, size)) = illus::realism_info(&pic, self.prefs.picture_model.as_ref(), &cands, None).install {
                    self.install = Some(Install { provider: p, model: m, size, started: false });
                    self.start_install(cx);
                }
            }
            n if (T_CHOICE_INSTALL..T_CHOICE_INSTALL + 10).contains(&n) => {
                let cands = cx.services.picture_choices(&self.found_images(cx));
                if let Some(c) = cands.get(n - T_CHOICE_INSTALL).filter(|c| !c.downloaded) {
                    let size = c.line().split(", ").nth(1).map_or("size unknown".to_string(), str::to_string);
                    self.install = Some(Install { provider: c.provider.clone(), model: c.model.clone(), size, started: false });
                    self.start_install(cx);
                }
            }
            T_KEEP_ONLY => self.keep_only_chosen(),
            n if (T_ALT_PICK..T_ALT_PICK + 6 * 24).contains(&n) => self.choose_main((n - T_ALT_PICK) / 24, (n - T_ALT_PICK) % 24),
            n if (T_ALT_DEL..T_ALT_DEL + 6 * 24).contains(&n) => self.delete_alt((n - T_ALT_DEL) / 24, (n - T_ALT_DEL) % 24),
            n if (T_ALT_MORE..T_ALT_MORE + 6).contains(&n) => self.more_alt(n - T_ALT_MORE, cx),
            T_QUALITY => self.set_pic(|o| o.quality = o.quality.next()),
            T_KEEPLOOK => self.set_pic(|o| o.keep_look = !o.keep_look),
            T_STORYMODEL => self.cycle_story_model(cx),
            T_PICLOG => self.open_pic_log(),
            T_PIC_INSTALL => self.start_install(cx),
            T_TIMEOUT => self.set_pic(|o| o.timeout_min = match o.limit_min() { 5 => 15, 15 => 30, 30 => 60, _ => 5 }),
            T_LOG_VERBOSE => self.set_pic(|o| o.verbose = !o.verbose),
            T_LOG_CLOSE => self.modal = None,
            T_LOG_COPY => {
                let t = piclog::log_text(&self.pic_log_events());
                if t.is_empty() {
                    self.note = Some(("The picture log is empty.".into(), false));
                } else {
                    crate::ui::clipboard::copy(&t);
                    self.note = Some(("The picture log is copied.".into(), true));
                }
            }
            T_LOG_CLEAR => {
                if let Some(Modal::PicLog(v)) = self.modal.as_mut() {
                    if v.confirm_clear {
                        v.confirm_clear = false;
                        v.top.set(0);
                        if let Some(id) = self.saved_id() {
                            self.store.clear_picture_log(&id);
                        }
                        self.pic_pending.clear();
                        self.note = Some(("The picture log was cleared.".into(), true));
                    } else {
                        v.confirm_clear = true;
                    }
                }
            }
            T_PICS_GO => {
                self.held = false;
                let queued: Vec<usize> = (0..self.pics.len()).filter(|&i| self.pics[i].state == PicState::Queued).collect();
                self.push_draw_calls(&queued);
                self.pump_pictures(cx);
            }
            T_PICS_SKIP => {
                self.held = false;
                self.alt_jobs.clear();
                self.pics.retain(|p| p.state == PicState::Ready);
                self.pic_gen += 1;
                self.persist_pictures();
                self.note = Some(("No pictures for this story.".into(), true));
            }
            T_PICS_CANCEL => self.cancel_pictures(),
            // how drawings are made: automatic (by the model), a rich SVG by the model, or the built-in cartoon
            T_DRAWMODE => {
                let all = st::DrawingMode::ALL;
                let at = all.iter().position(|m| *m == self.opts.drawing).unwrap_or(0);
                self.opts.drawing = all[(at + 1) % all.len()];
                self.save_prefs();
            }
            // short video clips as pictures: off unless the person allows them (they cost time and money)
            T_VIDEO => {
                self.opts.video = !self.opts.video;
                self.save_prefs();
            }
            T_READ_WHILE => {
                self.prefs.read_while_writing = !self.prefs.read_while_writing;
                self.save_prefs();
            }
            T_AUTOSAVE => {
                self.prefs.auto_save = !self.prefs.auto_save;
                self.save_prefs();
                if self.prefs.auto_save && self.saved.is_none() && self.gen.is_none() && !self.story.blocks.is_empty() {
                    self.save_story(cx, "");
                }
            }
            T_CREATE => self.submit(cx),
            T_MODEL => self.open_model_pick(false, cx),
            T_PICMODEL => self.open_model_pick(true, cx),
            n if (T_QUICK..T_QUICK + 6).contains(&n) => self.apply_quick(n - T_QUICK, cx),
            n if (T_SUGGEST..T_SUGGEST + SUGGESTIONS.len()).contains(&n) => {
                self.idea.set(SUGGESTIONS[n - T_SUGGEST]);
                self.sync_fields();
                self.menu = None;
                self.set_focus(Focus::Idea);
            }
            T_PLAY => self.play_pause(cx),
            T_STOP => self.stop_reading(),
            T_PREV => self.step_unit(-1, cx),
            T_NEXT => self.step_unit(1, cx),
            T_REPLAY => self.seek(0, cx),
            T_VOICE => self.cycle_voice(cx),
            T_SPEED => self.cycle_speed(true, cx),
            T_SAVE_AUDIO => self.save_audio(cx),
            T_FAV => {
                if let Some(id) = self.saved_id() {
                    self.toggle_fav(&id);
                } else {
                    self.note = Some(("Save the story first, then mark it as a favourite.".into(), false));
                }
            }
            T_COPY => self.copy_story(),
            T_SAVE => self.save_story(cx, ""),
            T_EXPORT => self.ask_export(None),
            T_NEW => self.new_story(cx),
            T_STOP_WRITING => self.stop_writing(),
            T_CANCEL_AUDIO => {
                if let Some(j) = &self.audio_job {
                    j.cancel.store(true, Ordering::SeqCst);
                }
            }
            T_LIB_OPEN | T_LIB_LISTEN => {
                if let Some(id) = self.lib.get(self.lib_sel).map(|e| e.id.clone()) {
                    self.open_saved(&id, n == T_LIB_LISTEN, cx);
                }
            }
            T_LIB_FAV => {
                if let Some(id) = self.lib.get(self.lib_sel).map(|e| e.id.clone()) {
                    self.toggle_fav(&id);
                }
            }
            T_LIB_DELETE => {
                if let Some(e) = self.lib.get(self.lib_sel) {
                    self.modal = Some(Modal::ConfirmDelete(e.id.clone(), e.title.clone()));
                }
            }
            T_LIB_EXPORT => {
                let id = self.lib.get(self.lib_sel).map(|e| e.id.clone());
                if id.is_some() {
                    self.ask_export(id);
                }
            }
            n if (T_PIC_REDO..T_PIC_REDO + 16).contains(&n) => self.redo_picture(n - T_PIC_REDO, cx),
            n if (T_PIC_DELETE..T_PIC_DELETE + 16).contains(&n) => self.delete_picture(n - T_PIC_DELETE),
            _ => {}
        }
    }

    fn next_focus(&mut self, back: bool) {
        let order: &[Focus] = match self.page {
            Page::Story => &[Focus::Idea, Focus::None],
            Page::Library => &[Focus::Search, Focus::None],
        };
        let i = order.iter().position(|f| *f == self.focus).unwrap_or(order.len() - 1);
        let n = order.len();
        self.set_focus(order[if back { (i + n - 1) % n } else { (i + 1) % n }]);
    }

    fn scroll_by(&mut self, delta: f32) {
        match self.page {
            Page::Story if self.menu == Some(Menu::Options) => self.cscroll = (self.cscroll + delta).clamp_safe(0.0, self.cmax),
            Page::Story => {
                self.tscroll = (self.tscroll + delta).clamp_safe(0.0, self.tmax);
                self.stick_bottom = self.tscroll >= self.tmax - 2.0;
                self.manual_unit = Some(self.reader.snapshot().unit);
            }
            Page::Library => {
                let d = if delta > 0.0 { 1 } else { -1 };
                self.lib_sel = (self.lib_sel as i32 + d).clamp_safe(0, self.lib.len().saturating_sub(1) as i32) as usize;
            }
        }
    }

    fn key(&mut self, k: &Key, cx: &AppCx) -> Flow {
        if matches!(self.modal, Some(Modal::PicLog(_))) {
            let page = self.modal.as_ref().map_or(1, |m| if let Modal::PicLog(v) = m { v.rows.get().max(2) as i64 - 1 } else { 1 });
            match k {
                Key::Esc | Key::Char('q') | Key::Char('g') => self.modal = None,
                Key::Up => self.log_scroll(-1),
                Key::Down => self.log_scroll(1),
                Key::PageUp => self.log_scroll(-page),
                Key::PageDown => self.log_scroll(page),
                Key::Home => self.log_scroll(i64::MIN / 2),
                Key::End => self.log_scroll(i64::MAX / 2),
                Key::Char('c') | Key::Ctrl('c') => self.click(T_LOG_COPY, cx),
                Key::Char('x') => self.click(T_LOG_CLEAR, cx),
                Key::Char('v') => self.click(T_LOG_VERBOSE, cx),
                _ => {}
            }
            if !matches!(k, Key::Char('x')) {
                if let Some(Modal::PicLog(v)) = self.modal.as_mut() {
                    v.confirm_clear = false;
                }
            }
            return Flow::Stay;
        }
        if let Some(Modal::ConfirmDelete(id, _)) = &self.modal {
            let id = id.clone();
            if *k == Key::Char('y') {
                self.delete_saved(&id, cx);
            }
            self.modal = None;
            return Flow::Stay;
        }
        if matches!(self.modal, Some(Modal::Model(_))) {
            self.model_event(&Event::Key(k.clone()), cx);
            return Flow::Stay;
        }
        if self.page == Page::Story && self.modal.is_none() {
            if let Some(m) = self.menu {
                if k == &Key::Esc {
                    self.close_menu();
                    return Flow::Stay;
                }
                if matches!(m, Menu::Kind | Menu::Tone | Menu::Language | Menu::Version) {
                    let n = match m {
                        Menu::Kind => Genre::ALL.len(),
                        Menu::Tone => Tone::ALL.len(),
                        Menu::Language => LANGUAGES.len(),
                        Menu::Version => self.ver_count as usize,
                        _ => 0,
                    };
                    match k {
                        Key::Up => self.menu_sel = self.menu_sel.saturating_sub(1),
                        Key::Down => self.menu_sel = (self.menu_sel + 1).min(n.saturating_sub(1)),
                        Key::Home => self.menu_sel = 0,
                        Key::End => self.menu_sel = n.saturating_sub(1),
                        Key::Enter => {
                            let code = match m {
                                Menu::Kind => T_GENRE + self.menu_sel,
                                Menu::Tone => T_TONE + self.menu_sel,
                                Menu::Language => T_LANG + self.menu_sel,
                                Menu::Version => T_VERSION_PICK + self.menu_sel + 1,
                                _ => T_MENU_CLOSE,
                            };
                            self.click(code, cx);
                        }
                        _ => {}
                    }
                    return Flow::Stay;
                }
                if m == Menu::How {
                    return Flow::Stay;
                }
            }
        }
        if matches!(k, Key::Ctrl('p') | Key::Ctrl('P')) && self.page == Page::Story {
            self.open_model_pick(false, cx);
            return Flow::Stay;
        }
        if self.page == Page::Story && matches!(k, Key::Ctrl('g') | Key::Ctrl('G')) {
            // the agents are in the same list now: Ctrl+G opens it too
            self.open_model_pick(false, cx);
            return Flow::Stay;
        }
        if self.page == Page::Story && matches!(k, Key::Ctrl('o') | Key::Ctrl('O')) {
            self.toggle_menu(Menu::Options, cx);
            return Flow::Stay;
        }
        if matches!(self.modal, Some(Modal::Export(..))) {
            let ev = Event::Key(k.clone());
            if let Some(Modal::Export(p, id)) = self.modal.as_mut() {
                let r = p.handle_event(&ev);
                let id = id.clone();
                match r {
                    PickEvent::Chosen(label) => {
                        self.modal = None;
                        let what = if label.starts_with("Page with every") { 3 } else if label.starts_with("Page") { 0 } else if label.starts_with("Text") { 1 } else { 2 };
                        self.do_export(what, id);
                    }
                    PickEvent::Cancel => self.modal = None,
                    PickEvent::None => {}
                }
            }
            return Flow::Stay;
        }
        if self.focus != Focus::None {
            // Ctrl+N (or Ctrl+Enter) creates from anywhere in the form
            if matches!(k, Key::Ctrl('n') | Key::Ctrl('N') | Key::Ctrl('\n') | Key::Ctrl('\r')) && self.page == Page::Story {
                self.submit(cx);
                return Flow::Stay;
            }
            match k {
                Key::Esc => self.focus = Focus::None,
                Key::Tab => self.next_focus(false),
                Key::BackTab => self.next_focus(true),
                // Enter sends, as in the chat; Ctrl+J makes a new line
                Key::Ctrl('j') | Key::Ctrl('J') if self.focus == Focus::Idea => {
                    self.idea.key(&Key::Enter);
                    self.sync_fields();
                }
                Key::Enter => match self.focus {
                    Focus::Search => {
                        if let Some(id) = self.lib.get(self.lib_sel).map(|e| e.id.clone()) {
                            self.open_saved(&id, false, cx);
                        }
                    }
                    _ => self.submit(cx),
                },
                Key::Up | Key::Down | Key::PageUp | Key::PageDown if self.page == Page::Library => {
                    let d = if matches!(k, Key::Up | Key::PageUp) { -1 } else { 1 };
                    self.lib_sel = (self.lib_sel as i32 + d).clamp_safe(0, self.lib.len().saturating_sub(1) as i32) as usize;
                }
                Key::PageUp => self.cscroll = (self.cscroll - 120.0).max(0.0),
                Key::PageDown => self.cscroll = (self.cscroll + 120.0).min(self.cmax),
                _ => {
                    let was = self.focus;
                    match self.focus {
                        Focus::Idea => {
                            self.idea.key(k);
                        }
                        Focus::Search => {
                            self.search.handle(k);
                        }
                        Focus::None => {}
                    }
                    self.sync_fields();
                    if was == Focus::Search {
                        self.reload_library();
                    }
                }
            }
            return Flow::Stay;
        }
        let ch = if let Key::Char(c) = k { Some(*c) } else { None };
        match k {
            Key::Esc => return Flow::Back,
            Key::Tab => {
                self.next_focus(false);
                return Flow::Stay;
            }
            Key::BackTab => {
                self.next_focus(true);
                return Flow::Stay;
            }
            Key::Ctrl('a') | Key::Ctrl('A') if self.page == Page::Story => {
                self.pane.select_all();
                return Flow::Stay;
            }
            Key::Ctrl('c') | Key::Ctrl('C') if self.page == Page::Story => {
                self.copy_story();
                return Flow::Stay;
            }
            Key::Ctrl('s') | Key::Ctrl('S') if self.page == Page::Story => {
                self.save_story(cx, "");
                return Flow::Stay;
            }
            Key::Ctrl('n') | Key::Ctrl('N') if self.page == Page::Story => {
                self.submit(cx);
                return Flow::Stay;
            }
            _ => {}
        }
        match ch {
            Some('1') => {
                self.open_form(cx);
                return Flow::Stay;
            }
            Some('2') => return self.go_ret(Page::Story, cx),
            Some('3') => return self.go_ret(Page::Library, cx),
            _ => {}
        }
        match self.page {
            Page::Story => match (k, ch) {
                (_, Some(' ')) | (_, Some('p')) => self.play_pause(cx),
                (_, Some('s')) => self.stop_reading(),
                (_, Some('r')) => self.seek(0, cx),
                (Key::Left, _) | (_, Some('[')) => self.step_unit(-1, cx),
                (Key::Right, _) | (_, Some(']')) => self.step_unit(1, cx),
                (_, Some('f')) => self.click(T_FAV, cx),
                (_, Some('c')) => self.copy_story(),
                (_, Some('e')) => self.ask_export(None),
                (_, Some('w')) => self.save_audio(cx),
                (_, Some('v')) => self.cycle_voice(cx),
                (_, Some('+')) | (_, Some('=')) => self.cycle_speed(true, cx),
                (_, Some('-')) | (_, Some('_')) => self.cycle_speed(false, cx),
                (_, Some('n')) => self.open_form(cx),
                (_, Some('l')) => self.go(Page::Library, cx),
                (_, Some('o')) => self.click(T_OPTIONS, cx),
                (_, Some('g')) => self.click(T_PICLOG, cx),
                (Key::Up, _) => self.scroll_by(-40.0),
                (Key::Down, _) => self.scroll_by(40.0),
                (Key::PageUp, _) => self.scroll_by(-240.0),
                (Key::PageDown, _) => self.scroll_by(240.0),
                (Key::Home, _) => self.scroll_by(-1e6),
                (Key::End, _) => self.scroll_by(1e6),
                _ => {}
            },
            Page::Library => match (k, ch) {
                (Key::Up, _) => self.lib_sel = self.lib_sel.saturating_sub(1),
                (Key::Down, _) => self.lib_sel = (self.lib_sel + 1).min(self.lib.len().saturating_sub(1)),
                (Key::Enter, _) => self.click(T_LIB_OPEN, cx),
                (_, Some(' ')) | (_, Some('l')) => self.click(T_LIB_LISTEN, cx),
                (_, Some('f')) => self.click(T_LIB_FAV, cx),
                (_, Some('d')) => self.click(T_LIB_DELETE, cx),
                (_, Some('e')) => self.click(T_LIB_EXPORT, cx),
                (_, Some('n')) => self.open_form(cx),
                (_, Some('/')) => self.set_focus(Focus::Search),
                _ => {}
            },
        }
        Flow::Stay
    }

    fn go_ret(&mut self, p: Page, cx: &AppCx) -> Flow {
        self.go(p, cx);
        Flow::Stay
    }
}

// ---------------------------------------------------------------------------------------------------------------- the composer's logic

type AgentRun = (iyakki_agent::extagent::run::RunRequest, String, String, bool);

impl StoryApp {
    /// The options with what the idea text says filled in, except what the person chose (that always wins) or removed.
    fn with_hints(&self, base: &StoryOptions) -> StoryOptions {
        let mut o = base.clone();
        let h: Hints = st::parse_hints(&base.topic);
        let ok = |k: usize, said: &str| !self.explicit[k] && self.dismissed[k].as_deref() != Some(said);
        if let Some((l, said)) = h.length {
            if ok(H_LEN, &said) {
                o.length = l;
            }
        }
        if let Some((l, said)) = h.language {
            if ok(H_LANG, &said) {
                o.language = l;
            }
        }
        if let Some((g, said)) = h.genre {
            if ok(H_GENRE, &said) {
                o.genre = g;
            }
        }
        if let Some((t, said)) = h.tone {
            if ok(H_TONE, &said) {
                o.tone = t;
            }
        }
        o
    }

    /// The options as they stand now, with the text's suggestions (what Send would use, pictures still as chosen).
    fn shown_options(&self) -> StoryOptions {
        let mut b = self.opts.clone();
        b.topic = self.idea.text.clone();
        self.with_hints(&b)
    }

    /// The suggestions in the idea text that have not been removed, for the chips under the composer.
    fn hint_chips(&self) -> Vec<HintChip> {
        let h = st::parse_hints(&self.idea.text);
        let mut v = Vec::new();
        let mut add = |k: usize, name: &'static str, value: String, said: String, chosen: String| {
            if self.dismissed[k].as_deref() != Some(said.as_str()) {
                v.push(HintChip { k, name, value, said, applied: self.explicit.get(k).map_or(true, |e| !e), chosen });
            }
        };
        if let Some((l, said)) = h.length {
            add(H_LEN, "Length", l.label().to_string(), said, self.opts.length.label().to_string());
        }
        if let Some((l, said)) = h.language {
            add(H_LANG, "Language", l, said, self.opts.language.clone());
        }
        if let Some((g, said)) = h.genre {
            add(H_GENRE, "Kind", g.label().to_string(), said, self.opts.genre.label().to_string());
        }
        if let Some((t, said)) = h.tone {
            add(H_TONE, "Tone", t.label().to_string(), said, self.opts.tone.label().to_string());
        }
        if let Some((st, said)) = h.style {
            add(H_STYLE, "Picture style", st.label().to_string(), said, String::new());
        }
        v
    }

    // ---- the agent

    /// The agent that will write, if one is chosen and still configured.
    fn agent_id(&self, cx: &AppCx) -> Option<String> {
        self.prefs.agent.clone().filter(|id| cx.services.agents_cfg().agent(id).is_some())
    }

    fn agent_label(&self, cx: &AppCx) -> Option<String> {
        let id = self.agent_id(cx)?;
        cx.services.agents_cfg().agent(&id).map(|a| a.label())
    }

    /// The run of an external agent for these messages: `Ok(None)` when our own model writes; `Err` says why a chosen agent
    /// cannot be used (the story is then written by our own model). The request denies every tool and works in an empty folder.
    fn agent_request(&self, cx: &AppCx, msgs: &[iyakki_agent::chat::Message]) -> Result<Option<AgentRun>, String> {
        let Some(id) = self.prefs.agent.clone() else { return Ok(None) };
        let cfg = cx.services.agents_cfg();
        let Some(agent) = cfg.agent(&id) else { return Err("The agent you chose is not set up any more (Settings → Agents).".into()) };
        let label = agent.label();
        if cx.session.policy.cli != iyakki_sec::policy::CliLevel::Full {
            return Err(format!("{label} is off for your account (it may not run commands)."));
        }
        match cx.services.ext_status.get(&id) {
            Some(st) if !st.ready() => return Err(format!("{label} is not ready: {}", st.fix)),
            Some(_) => {}
            None => cx.services.ext_status.refresh(cfg.clone()),
        }
        let advisor = agent.mode() == iyakki_agent::extagent::Mode::Advisor;
        let (_, key_ref) = cfg.cfg_of(&id).unwrap_or_default();
        let key = if key_ref.is_empty() { None } else { Some(cx.services.secrets.resolve(&key_ref, None).map_err(|e| format!("The API key `{key_ref}` of {label} could not be read from the vault: {e}."))?) };
        let workspace = cx.services.dir().join("tales-agent");
        std::fs::create_dir_all(&workspace).map_err(|e| format!("{label} has no folder to work in: {e}."))?;
        let (system, prompt) = (msgs.first().map(|m| m.content.clone()).unwrap_or_default(), msgs.get(1).map(|m| m.content.clone()).unwrap_or_default());
        let (svc, user, audit, user2) = (cx.services.clone(), cx.session.user.clone(), cx.audit.clone(), cx.session.user.clone());
        let model = self.prefs.agent_model.clone();
        let shown = if model.is_empty() { label.clone() } else { format!("{label} · {model}") };
        let req = iyakki_agent::extagent::run::RunRequest {
            agent: Arc::from(agent),
            workspace,
            prompt,
            system: Arc::new(move || system.clone()),
            model,
            resume: None,
            tools: Arc::new(iyakki_agent::extagent::NoTools),
            limits: iyakki_agent::agent::RunLimits { max_seconds: 900, max_steps: 1, ..Default::default() },
            allow_auto: false,
            key,
            audit: Arc::new(move |k, d| audit.record(&user2, k, d)),
            exe: std::env::current_exe().unwrap_or_default(),
            on_session: Arc::new(|_| {}),
            on_usage: Arc::new(move |l, m, i, o| svc.usage.record(&user, l, m, i, o)),
            no_tools: true,
        };
        Ok(Some((req, label, shown, advisor)))
    }

    // ---- menus

    fn close_menu(&mut self) {
        if self.menu == Some(Menu::Language) {
            self.menu = Some(Menu::Options);
            return;
        }
        self.menu = None;
        if self.page == Page::Story && self.modal.is_none() {
            self.set_focus(Focus::Idea);
        }
    }

    /// Open one of the composer's menus, or close it when it is the one open.
    fn toggle_menu(&mut self, m: Menu, cx: &AppCx) {
        if self.menu == Some(m) {
            self.close_menu();
            return;
        }
        match m {
            Menu::Kind | Menu::Tone => {
                let o = self.shown_options();
                self.menu_sel = if m == Menu::Kind { Genre::ALL.iter().position(|g| *g == o.genre) } else { Tone::ALL.iter().position(|t| *t == o.tone) }.unwrap_or(0);
                self.menu = Some(m);
            }
            Menu::Options => {
                self.refresh_tts(cx);
                self.cscroll = 0.0;
                self.menu = Some(m);
            }
            Menu::Language => {
                self.menu_sel = LANGUAGES.iter().position(|l| *l == self.shown_options().language).unwrap_or(0);
                self.menu = Some(m);
            }
            Menu::Version => {
                if self.ver_count < 2 {
                    return;
                }
                self.menu_sel = (self.ver as usize).saturating_sub(1);
                self.menu = Some(m);
            }
            Menu::How => {
                self.refresh_tts(cx);
                self.menu = Some(m);
            }
        }
        if self.menu.is_some() && !matches!(self.menu, Some(Menu::Options)) && self.focus != Focus::None {
            self.focus = Focus::None;
        }
    }

    /// What the Pictures chip near the composer says: what will really be done.
    fn pictures_status(&self, cx: &AppCx) -> String {
        let picture_name = || match self.picture_plan(cx) {
            ModelPlan::Chosen(c) => c.model,
            ModelPlan::SameAsChat { .. } => self.image_models.first().map_or("the picture model".to_string(), |m| m.1.clone()),
        };
        match self.opts.pictures.resolve(!self.image_models.is_empty()) {
            PictureMode::Off => "Pictures: off".into(),
            PictureMode::ImageModel if self.image_models.is_empty() && self.prefs.picture_model.is_none() => "Pictures: images (no picture model set up)".into(),
            PictureMode::ImageModel => format!("Pictures: images via {}", self.resolved_for(&self.pic_now(), cx).map_or_else(picture_name, |r| r.model)),
            _ => {
                let (rich, who) = self.draws_rich(cx);
                if rich { format!("Pictures: rich SVG by {who}") } else { "Pictures: cartoon (built-in)".into() }
            }
        }
    }

    /// Whether drawings are made as rich SVG (a strong writer, or the person said so) and by whom; else the built-in cartoon.
    fn draws_rich(&self, cx: &AppCx) -> (bool, String) {
        let agent = self.agent_label(cx);
        let model = match self.model_plan(cx) {
            ModelPlan::Chosen(c) => c.model,
            ModelPlan::SameAsChat { .. } => Self::chat_choice(cx).map_or(String::new(), |c| c.model),
        };
        let who = agent.clone().unwrap_or(model.clone());
        let rich = match self.opts.drawing {
            st::DrawingMode::Rich => true,
            st::DrawingMode::Cartoon => false,
            st::DrawingMode::Auto | st::DrawingMode::Image => scene::rich::is_strong(&model, agent.is_some()),
        };
        (rich, who)
    }

    /// The line above the controls: who writes.
    fn writer_line(&self, cx: &AppCx) -> String {
        match self.agent_label(cx) {
            Some(l) => {
                let m = if self.prefs.agent_model.is_empty() { "default model".to_string() } else { self.prefs.agent_model.clone() };
                let mode = self.agent_id(cx).and_then(|id| cx.services.agents_cfg().agent(&id)).map_or("", |a| if a.mode() == iyakki_agent::extagent::Mode::Advisor { " · advisor, text only" } else { " · no tools" });
                format!("agent: {l} · {m}{mode}")
            }
            None => format!("model: {}", self.model_label(cx).0),
        }
    }
}

impl Drop for StoryApp {
    fn drop(&mut self) {
        // leaving the tile ends the reading and the writing: nothing keeps running unseen
        self.reader.stop();
        if let Some(g) = &self.gen {
            g.stop.store(true, Ordering::SeqCst);
        }
        if let Some(j) = &self.audio_job {
            j.cancel.store(true, Ordering::SeqCst);
        }
        self.pic_gen += 1;
    }
}

// ---------------------------------------------------------------------------------------------------------------- drawing

impl StoryApp {
    /// The page tabs, and on a narrow window the buttons that stand in for the form pane. Returns the height.
    fn tabs(&mut self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32) -> f32 {
        let items = vec![("Story".to_string(), Event::Tab(T_STORY_PAGE)), ("Listen again".to_string(), Event::Tab(T_LIB_PAGE))];
        let sel = match self.page {
            Page::Story => 0,
            Page::Library => 1,
        };
        let tw = fx::tab_strip(g, x, y, s, w, &items, sel, ACCENT);
        let th = 14.0 * s * 2.1;
        // narrow: the composer is a toggle (it sits below the story when shown)
        let right = x + w;
        if !self.wide && self.page == Page::Story {
            let sz = 14.0 * s;
            let label = if self.pane_open { "Hide the idea box" } else { "Idea & options" };
            let ow = g.text_width(sz, label, Face::Bold) + sz * 1.9;
            if right - ow > x + tw + 8.0 * s {
                chip(g, right - ow, y + (th - 28.0 * s) / 2.0, s, label, self.pane_open, Event::Tab(T_OPTIONS));
            }
            return th;
        }
        // what is going on, on the right
        let (label, col) = if self.gen.is_some() {
            (format!("writing{}", ".".repeat(1 + (self.ticks / 10 % 3) as usize)), MAGENTA)
        } else if matches!(self.reader.status(), Status::Playing) {
            ("reading aloud".to_string(), CYAN)
        } else if matches!(self.reader.status(), Status::Paused) {
            ("paused".to_string(), MUTED)
        } else {
            (String::new(), MUTED)
        };
        if !label.is_empty() {
            let lw = g.text_width(13.0 * s, &label, Face::Bold);
            if tw + lw + 24.0 * s < w {
                g.circle(x + w - lw - 14.0 * s, y + 15.0 * s, 4.0 * s, col);
                g.text(x + w - lw, y + 15.0 * s - g.line_height(13.0 * s, Face::Bold) / 2.0, 13.0 * s, col, &label, Face::Bold);
            }
        }
        th
    }

    fn banner(&mut self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32) -> f32 {
        let shown = self.note.clone().or_else(|| self.wait_text.clone().map(|t| (t, true)));
        if let Some((m, ok)) = &shown {
            let m = if *ok { m.clone() } else { iyakki_storage::loss::reword(m) };
            let lines = g.wrap(13.0 * s, &m, Face::Sans, w - 24.0 * s);
            let lh = g.line_height(13.0 * s, Face::Sans);
            let h = lines.len().min(3) as f32 * lh + 12.0 * s;
            g.rrect(x, y, w, h, 10.0 * s, if *ok { fx::over(WELL, with_alpha(OK, 30)) } else { fx::over(WELL, with_alpha(ERR, 34)) });
            for (i, l) in lines.iter().take(3).enumerate() {
                g.text(x + 12.0 * s, y + 6.0 * s + i as f32 * lh, 13.0 * s, if *ok { OK } else { ERR }, l, Face::Sans);
            }
            return h + 8.0 * s;
        }
        0.0
    }

    // ---------------------------------------------------------------------------------------------- the form pane

    /// The idea box: draws it (when `draw`) and registers its press area. Returns its height (with the notice under it).
    fn idea_box(&mut self, g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, draw: bool, tick: u64) -> f32 {
        let focused = self.focus == Focus::Idea && self.modal.is_none();
        let ph = "Describe your story, e.g. a 5-minute funny story in Tamil about a fox who is afraid of the dark";
        let h = self.idea.render(g, x, y, w, s, 15.0 * s, 9.0 * s, 40.0 * s, focused, tick, ph, ACCENT, IDEA_AREA, "idea", false, draw);
        self.idea_geo = self.idea.geo;
        h
    }

    /// The one line under the player about reading aloud, and the colour to draw it in; `None` when a voice is ready.
    /// No voice is ready and the person can fix it: the chip "Turn on reading aloud" is shown instead of a message.
    fn chip_wanted(&self) -> bool {
        self.saved_audio.is_none() && matches!(self.tts, TtsStatus::NoEngine | TtsStatus::HostUnavailable(_) | TtsStatus::BrowserUnavailable(_))
    }

    /// The chip: look for the speech engines of this computer again; when one is found it is used (nothing to choose). When none
    /// is, the desktop window offers the browser voice helper, and every other screen goes to Settings → Voice.
    fn turn_on_reading(&mut self, cx: &AppCx) {
        if self.turning_on {
            return;
        }
        cx.audit.record(&cx.session.user, "story.turn_on_reading", "");
        if !cx.services.host_tts.is_enabled() {
            self.open_voice_settings("Reading aloud is set up in Settings → Voice.");
            return;
        }
        self.turning_on = true;
        self.note = Some(("Looking for a speech engine on this computer…".into(), true));
        cx.services.host_tts.recheck();
    }

    fn open_voice_settings(&mut self, why: &str) {
        self.note = Some((why.to_string(), true));
        self.request = Some(("settings".into(), "voice".into()));
    }

    fn poll_turn_on(&mut self, cx: &AppCx) {
        if !self.turning_on || cx.services.host_tts.scanning() {
            return;
        }
        self.turning_on = false;
        self.refresh_tts(cx);
        if matches!(self.tts, TtsStatus::Ready { .. }) {
            self.note = Some((format!("{} Press play.", st::status_text(&self.tts)), true));
            return;
        }
        let host = HostVoice::current();
        if host.front() == hostvoice::Front::Window && !host.helper_active() {
            match cx.services.host_tts.helper_open(host, None) {
                Ok(_) => self.note = Some(("No speech engine was found on this computer. The voice helper is opening in your browser: allow sound there, then press play. (Settings → Voice has more choices.)".into(), true)),
                Err(e) => self.open_voice_settings(&format!("No speech engine was found, and the voice helper could not start: {e}")),
            }
            return;
        }
        self.open_voice_settings("No speech engine was found on this computer. Settings → Voice shows what is missing and how to get it.");
    }

    fn read_hint(&self) -> Option<(String, [u8; 4])> {
        match &self.tts {
            TtsStatus::Ready { .. } => None,
            _ if self.saved_audio.is_some() => None,
            TtsStatus::NoEngine => Some(("Reading aloud: open IYAKKI in a browser for browser voice, or set up Settings → Voice".into(), DIM)),
            other => Some((st::status_text(other), fx::AMBER)),
        }
    }

    // ---------------------------------------------------------------------------------------------- the composer

    /// The composer, in the manner of the AI IDE's chat: the writer line, one row of controls (who it is for, kind and tone as
    /// drop-downs, Model, Agent, Options), the idea box that grows with Send beside it, and a row of chips (what Pictures will do
    /// and the suggestions found in the text). Draws it (when `draw`) with its top left at (`x`, `y0`) and registers the clicks.
    /// Returns its height.
    #[allow(clippy::too_many_arguments)]
    fn composer_body(&mut self, g: &mut Gfx, x: f32, y0: f32, w: f32, s: f32, cx: &AppCx, tick: u64, draw: bool, bounds: R) -> f32 {
        enum Ctl {
            Label(String),
            Aud(usize),
            Kind(String),
            Tone(String),
            Model(String),
            Opts,
        }
        let m = super::ide_chrome::cur();
        let side = 12.0 * s;
        let (x0, w) = (x + side, w - 2.0 * side);
        let th = super::ide_chrome::tool(s) + 2.0 * s;
        let fsz = m.font_small * s + 1.0 * s;
        let mut y = y0 + 6.0 * s;
        // who writes (and Stop while it is writing)
        if draw {
            let line = self.writer_line(cx);
            let stop_w = if self.gen.is_some() { g.text_width(fsz, "Stop", Face::Bold) + 16.0 * s } else { 0.0 };
            let t = g.fit(13.0 * s, &line, Face::Sans, w - stop_w - 6.0 * s);
            g.text(x0, y + 1.0 * s, 13.0 * s, super::ide_chrome::dim(), &t, Face::Sans);
            if self.gen.is_some() {
                super::ide_chrome::text_btn(g, x0 + w - stop_w, y - 3.0 * s, s, "Stop", ERR, true, Event::Tab(T_STOP_WRITING), "Stop", Some("Stop writing: what is written so far is kept (Send a correction to steer it)"), bounds);
            }
        }
        y += 18.0 * s;
        // the controls, left to right, wrapping when the row is full
        let shown = self.shown_options();
        let chip = self.chip_text(cx);
        let ctls: Vec<Ctl> = {
            let mut v = vec![Ctl::Label("Who is it for".into())];
            v.extend((0..Audience::ALL.len()).map(Ctl::Aud));
            v.push(Ctl::Kind(format!("Kind: {}", shown.genre.label())));
            v.push(Ctl::Tone(format!("Tone: {}", shown.tone.label())));
            v.push(Ctl::Model(chip.clone()));
            v.push(Ctl::Opts);
            v
        };
        let btn_w = |g: &mut Gfx, l: &str| g.text_width(fsz, l, Face::Bold) + 16.0 * s;
        let (mut cxp, mut cy) = (x0, y);
        for c in &ctls {
            let cw = match c {
                Ctl::Label(l) => g.text_width(fsz, l, Face::Sans) + 10.0 * s,
                Ctl::Aud(i) => btn_w(g, Audience::ALL[*i].label()),
                Ctl::Kind(l) | Ctl::Tone(l) => select_width(g, s, l),
                Ctl::Model(l) => btn_w(g, &format!("{l} ▾")),
                Ctl::Opts => btn_w(g, "Options ▾"),
            };
            if cxp > x0 && cxp + cw > x0 + w {
                cxp = x0;
                cy += th + 2.0 * s;
            }
            if draw {
                let ty = cy + 1.0 * s;
                match c {
                    Ctl::Label(l) => {
                        g.text(cxp, ty + (th - 2.0 * s - g.line_height(fsz, Face::Sans)) / 2.0, fsz, super::ide_chrome::dim(), l, Face::Sans);
                    }
                    Ctl::Aud(i) => {
                        let a = Audience::ALL[*i];
                        let on = self.opts.audience == a;
                        let r = R::new(cxp, ty, cw, super::ide_chrome::tool(s));
                        if on {
                            g.rrect(r.x, r.y, r.w, r.h, 4.0 * s, with_alpha(ACCENT, 46));
                            g.rrect_outline(r.x, r.y, r.w, r.h, 4.0 * s, 1.2 * s, ACCENT);
                        }
                        super::ide_chrome::text_btn(g, cxp, ty, s, a.label(), if on { ACCENT } else { MUTED }, true, Event::Tab(T_AUDIENCE + i), a.label(), Some("Who the story is for (remembered)"), bounds);
                    }
                    Ctl::Kind(l) => self.anchors[0] = select_box(g, cxp, ty, cw, s, l, self.menu == Some(Menu::Kind), ACCENT, Event::Tab(T_KIND_MENU), "Kind of story"),
                    Ctl::Tone(l) => self.anchors[1] = select_box(g, cxp, ty, cw, s, l, self.menu == Some(Menu::Tone), ACCENT, Event::Tab(T_TONE_MENU), "Tone"),
                    Ctl::Model(l) => {
                        self.anchors[4] = R::new(cxp, ty, cw, super::ide_chrome::tool(s));
                        let label = format!("{l} ▾");
                        let on = self.agent_id(cx).is_some();
                        super::ide_chrome::text_btn(g, cxp, ty, s, &label, if on { rgb(255, 196, 94) } else { ACCENT }, true, Event::Tab(T_MODEL), &label, Some("Choose who writes the story: same as chat, a model of a provider, or an agent such as Claude Code with one of its models (Ctrl+P); an agent gets no tools"), bounds);
                    }
                    Ctl::Opts => {
                        self.anchors[2] = R::new(cxp, ty, cw, super::ide_chrome::tool(s));
                        super::ide_chrome::text_btn(g, cxp, ty, s, "Options ▾", if self.menu == Some(Menu::Options) { ACCENT } else { MUTED }, true, Event::Tab(T_OPTIONS_MENU), "Options ▾", Some("Length, language, characters, pictures, reading and saving (Ctrl+O)"), bounds);
                    }
                }
            }
            cxp += cw + 2.0 * s;
        }
        y = cy + th + 4.0 * s;
        // the idea box and Send
        let send_w = 64.0 * s;
        let ih = self.idea_box(g, x0, y, w - send_w - 8.0 * s, s, draw, tick);
        if draw {
            let bs = 13.0 * s;
            let by = y + (self.idea.box_h - bs * 2.1) / 2.0;
            if self.gen.is_some() {
                g.button_off(x0 + w - send_w + 8.0 * s, by, bs, "Send");
            } else {
                g.button(x0 + w - send_w + 8.0 * s, by, bs, "Send", ACCENT, Event::Tab(T_CREATE));
            }
        }
        y += ih + 6.0 * s;
        // the chips: what the pictures will be, and what was understood from the text
        let mut items: Vec<(String, bool, Event)> = vec![(self.pictures_status(cx), false, Event::Tab(T_OPTIONS_MENU))];
        for c in self.hint_chips() {
            let label = if c.applied { format!("{}: {} ×", c.name, c.value) } else { format!("\"{}\" ignored, {} is {} ×", c.said, c.name, c.chosen) };
            items.push((label, c.applied, Event::Tab(T_HINT + c.k)));
        }
        y += flow(g, x0, y, w, s, &items, draw) + 8.0 * s;
        y - y0
    }

    /// The height of the composer in a panel `w` wide and `avail` high: the idea box shows fewer lines (three at least when there
    /// is room, two or one when it is short) when room is short.
    fn composer_height(&mut self, g: &mut Gfx, w: f32, avail: f32, s: f32, cx: &AppCx) -> f32 {
        for rows in (1..=super::multiline::MAX_ROWS).rev() {
            self.idea.max_rows = rows;
            self.idea.min_rows = rows.min(super::multiline::MIN_ROWS);
            let h = self.composer_body(g, 0.0, 0.0, w, s, cx, 0, false, R::new(0.0, 0.0, w, avail));
            if h <= avail * 0.62 || rows == 1 {
                return h;
            }
        }
        0.0
    }

    /// The panel on the right, like the AI IDE's chat: a header, the conversation (what was understood, the steps of writing
    /// and drawing, the person's corrections) with the same renderer as the chat, and the composer at the bottom.
    fn panel(&mut self, g: &mut Gfx, r: R, s: f32, cx: &AppCx, tick: u64, area: R) {
        let m = super::ide_chrome::cur();
        g.rect(r.x, r.y, r.w, r.h, crate::ui::BG_BOTTOM);
        g.rect(r.x, r.y, 1.0, r.h, [255, 255, 255, 22]);
        let head_h = (m.head_h * s).max(30.0 * s);
        let title = if self.story.blocks.is_empty() { "Create a story".to_string() } else { self.display_title() };
        let tsz = m.font_ui * s;
        let hide_w = if self.wide { 0.0 } else { g.text_width(m.font_small * s + s, "Hide", Face::Bold) + 22.0 * s };
        let t = g.fit(tsz, &title, Face::Bold, (r.w - 24.0 * s - hide_w).max(40.0 * s));
        g.text(r.x + 12.0 * s, r.y + (head_h - g.line_height(tsz, Face::Bold)) / 2.0, tsz, TEXT, &t, Face::Bold);
        if !self.wide {
            super::ide_chrome::text_btn(g, r.x + r.w - hide_w - 4.0 * s, r.y + (head_h - super::ide_chrome::tool(s)) / 2.0, s, "Hide", MUTED, true, Event::Tab(T_OPTIONS), "Hide the idea box", Some("Back to the story"), area);
        }
        g.rect(r.x + 1.0, r.y + head_h, r.w - 1.0, 1.0, [255, 255, 255, 14]);
        let comp_h = self.composer_height(g, r.w, r.h - head_h, s, cx);
        let comp_y = r.y + r.h - comp_h;
        g.rect(r.x + 1.0, comp_y - 2.0 * s, r.w - 1.0, comp_h + 2.0 * s, crate::ui::BG_BOTTOM);
        g.rect(r.x + 1.0, comp_y - 2.0 * s, r.w - 1.0, 1.0, [255, 255, 255, 14]);
        // the conversation, in the chat's own renderer
        let log = R::new(r.x + 12.0 * s, r.y + head_h + 6.0 * s, r.w - 24.0 * s, (comp_y - 6.0 * s - (r.y + head_h + 6.0 * s)).max(30.0 * s));
        if g.hovered(r) {
            self.panel_hover = true;
        }
        let busy = self.gen.is_some() || self.pics.iter().any(|p| matches!(p.state, PicState::Working | PicState::Queued));
        let think_secs = self.think_since.map_or(0, |t| t.elapsed().as_secs());
        chat_view::set_mono_boost(1.0);
        let total = {
            let bctx = chat_view::Ctx { tool_ok: &self.tool_ok, approval: None, busy, open: &Default::default(), live: "", thinking: &self.thinking, think_secs };
            self.view.prepare(g, &self.convo, &bctx, log.w, s, &mut NoPics)
        };
        self.max_off = (total - log.h).max(0.0);
        self.scroll_px = self.scroll_px.clamp_safe(0.0, self.max_off);
        if total <= 0.0 {
            let lines = ["Describe a story and press Send.", "Say it in words if you like: a 5-minute funny story in Tamil about a fox who is afraid of the dark.", "Then tell me what to change: shorter, a different name, funnier, another language, or redraw picture 2 as a comic."];
            let mut y = log.y + 8.0 * s;
            for (i, l) in lines.iter().enumerate() {
                for w in g.wrap(if i == 0 { 15.5 * s } else { 13.5 * s }, l, Face::Sans, log.w) {
                    g.text(log.x, y, if i == 0 { 15.5 * s } else { 13.5 * s }, if i == 0 { TEXT } else { MUTED }, &w, Face::Sans);
                    y += if i == 0 { 22.0 * s } else { 19.0 * s };
                }
                y += 8.0 * s;
            }
        }
        let (scroll_px, view) = (self.scroll_px, &self.view);
        g.clipped(log, |g| view.draw(g, log, scroll_px, tick, s, ACCENT, &|k| Event::Key(k)));
        self.composer_body(g, r.x, comp_y, r.w, s, cx, tick, true, area);
    }

    /// The Options popover: the little that is not always on show.
    #[allow(clippy::too_many_arguments)]
    fn options_content(&mut self, g: &mut Gfx, x: f32, y0: f32, w: f32, s: f32, cx: &AppCx, draw: bool) -> f32 {
        let gap = 12.0 * s;
        let mut y = y0;
        y += section(g, x, y, w, s, "Or pick an idea", draw);
        let items: Vec<(String, bool, Event)> = SUGGESTIONS.iter().enumerate().map(|(i, t)| (t.to_string(), self.idea.text == *t, Event::Tab(T_SUGGEST + i))).collect();
        y += flow(g, x, y, w, s, &items, draw) + gap;
        y += section(g, x, y, w, s, "How long", draw);
        let items: Vec<_> = Length::ALL.iter().enumerate().map(|(i, a)| (a.label().to_string(), self.opts.length == *a, Event::Tab(T_LENGTH + i))).collect();
        y += flow(g, x, y, w, s, &items, draw) + gap;
        // the one manual control for the language (the idea text can say it too)
        y += section(g, x, y, w, s, "Language", draw);
        let label = format!("Language: {}", self.shown_options().language);
        let lw = select_width(g, s, "Language: Portuguese").max(select_width(g, s, &label));
        if draw {
            self.anchors[5] = select_box(g, x, y, lw, s, &label, self.menu == Some(Menu::Language), ACCENT, Event::Tab(T_LANG_MENU), "Language");
        }
        y += super::ide_chrome::tool(s) + gap;
        y += section(g, x, y, w, s, "Pictures", draw);
        let images = !self.image_models.is_empty();
        let mut chips: Vec<(String, bool, Event)> = Vec::new();
        let mut real_lines: Vec<(String, [u8; 4])> = Vec::new();
        if self.opts.pictures != PictureMode::Off && images {
            // the one visible choice for real-looking pictures, with the model and server it will use
            let pic = self.pic_now();
            let on = pic.is_realistic();
            chips.push((format!("{} Realistic pictures (photo style, people look real)", if on { "\u{2611}" } else { "\u{2610}" }), on, Event::Tab(T_REALISTIC)));
            let cands = cx.services.picture_choices(&self.found_images(cx));
            let info = illus::realism_info(&pic, self.prefs.picture_model.as_ref(), &cands, self.memory_now().filter(|m| m.total > 0).map(|m| m.available >> 20).as_ref().copied());
            real_lines.push((format!("{}{}", info.uses, if on { "" } else { " (when ticked)" }), MUTED));
            if let Some(w) = &info.warning {
                real_lines.push((w.clone(), fx::AMBER));
            }
            if let Some((p, m, size)) = &info.install {
                chips.push((format!("Install {m} on {p} ({size})"), false, Event::Tab(T_REAL_INSTALL)));
            }
        }
        chips.push(("Pictures off".to_string(), self.opts.pictures == PictureMode::Off, Event::Tab(T_PICMODE)));
        let mut pnote = None;
        if images && self.opts.pictures != PictureMode::Off {
            let (pl, pn) = self.picture_label(cx);
            chips.push((format!("Picture model: {pl}"), self.prefs.picture_model.is_some(), Event::Tab(T_PICMODEL)));
            pnote = pn;
        }
        if self.opts.pictures != PictureMode::Off {
            chips.push((format!("Pictures: {}", self.opts.coverage.label()), self.opts.coverage != Coverage::Chapters, Event::Tab(T_COVERAGE)));
            if images {
                let pic = self.pic_now();
                let styles = pic.styles_list();
                for (i, l) in illus::Look::ALL.iter().enumerate() {
                    let on = styles.contains(l);
                    chips.push((format!("Style {}: {}", if on { "\u{2611}" } else { "\u{2610}" }, l.label()), on, Event::Tab(T_STYLE + i)));
                }
                chips.push((format!("Pictures per style: {}", pic.count()), pic.count() > 1, Event::Tab(T_PERSTYLE)));
                chips.push((format!("Quality: {}", pic.quality.label()), pic.quality != illus::Quality::Selected, Event::Tab(T_QUALITY)));
                let choices = cx.services.picture_choices(&self.found_images(cx));
                let mname = pic.model.as_ref().map_or("automatic".to_string(), |c| {
                    let state = choices.iter().find(|x| x.provider == c.provider && x.model == c.model).map_or(String::new(), |x| format!(" ({}{})", if x.downloaded { "installed" } else { "not downloaded" }, x.size_gb.map_or(String::new(), |g| format!(", {g:.1} GB"))));
                    format!("{}{state}", Self::model_name(c))
                });
                chips.push((format!("Model for this story: {mname}"), pic.model.is_some(), Event::Tab(T_STORYMODEL)));
                chips.push((format!("Time limit per picture: {} min", pic.limit_min()), pic.limit_min() != 15, Event::Tab(T_TIMEOUT)));
                chips.push((format!("Same look for all pictures: {}", if pic.keep_look { "on" } else { "off" }), pic.keep_look, Event::Tab(T_KEEPLOOK)));
            }
            chips.push(("Picture log (g)".to_string(), false, Event::Tab(T_PICLOG)));
            chips.push((format!("Drawing: {}", self.opts.drawing.label()), self.opts.drawing != st::DrawingMode::Auto, Event::Tab(T_DRAWMODE)));
            chips.push((format!("Allow short video clips: {}", if self.opts.video { "on" } else { "off" }), self.opts.video, Event::Tab(T_VIDEO)));
        }
        y += flow(g, x, y, w, s, &chips, draw) + 6.0 * s;
        for (t, col) in &real_lines {
            for l in g.wrap(12.0 * s, t, Face::Sans, w) {
                if draw {
                    g.text(x, y, 12.0 * s, *col, &l, Face::Sans);
                }
                y += g.line_height(12.0 * s, Face::Sans);
            }
        }
        // the picture models that are not installed, with their size and an Install button
        if images && self.opts.pictures != PictureMode::Off {
            let missing: Vec<(String, bool, Event)> = cx.services.picture_choices(&self.found_images(cx)).iter().enumerate().filter(|(_, c)| !c.downloaded).take(10).map(|(i, c)| (format!("Install {} on {} ({})", c.model, c.provider, c.line().split(", ").nth(1).unwrap_or("size unknown")), false, Event::Tab(T_CHOICE_INSTALL + i))).collect();
            if !missing.is_empty() {
                y += 4.0 * s + flow(g, x, y, w, s, &missing, draw) + 2.0 * s;
            }
        }
        let hint = match (self.opts.pictures, images) {
            (PictureMode::Off, _) => "No pictures: the story is text only.".to_string(),
            (_, true) => "Pictures are automatic: each scene is drawn as a real image by the picture model; if that fails the rest are drawn as SVG. The style follows the kind of story (say \"comic style\" or \"pixel art\" in your idea to change it).".to_string(),
            (_, false) => "Pictures are automatic: no picture model is set up (Settings → Models), so the writer draws each scene: a strong model or agent (Claude, GPT class) draws a rich SVG itself, a small model picks a scene for the built-in cartoon (Drawing ▸ Auto, Rich SVG or Built-in cartoon). Realistic or photo pictures need a picture model; the built-in drawings are cartoons. The style follows the kind of story (say \"comic style\" or \"pixel art\" in your idea to change it).".to_string(),
        };
        let mut hint = pnote.unwrap_or(hint);
        // one plain line about the picture model: why it is not used, or what it costs in time (an estimate, said so)
        if self.opts.pictures != PictureMode::Off {
            let r = self.readiness(cx);
            match illus::reason(&r) {
                Some(why) => hint = format!("{why} {hint}"),
                None if images => {
                    let using = self.resolved_for(&self.pic_now(), cx).map_or(String::new(), |r| format!("Using {}. ", r.summary()));
                    let list: Vec<String> = cx.services.picture_choices(&self.found_images(cx)).iter().map(|c| c.line()).collect();
                    let missing = self.resolved_for(&self.pic_now(), cx).filter(|r| !r.downloaded).map_or(String::new(), |r| format!("{} ", illus::not_downloaded_text(&r.model, &r.provider, "see the list"))) ;
                    let job = self.job_line(cx, 1).filter(|_| self.pic_now().styles_list().len() > 1 || self.pic_now().count() > 1).map_or(String::new(), |j| format!(" For one scene: {j}"));
                    hint = format!("{missing}{using}{} {hint}{job} {} Picture models: {}.", self.timing_line(cx, 1), illus::advice(cx.services.gpu_lemonade().as_deref()), list.join(" \u{b7} "));
                }
                None => {}
            }
        }
        let lh = g.line_height(12.0 * s, Face::Sans);
        for l in g.wrap(12.0 * s, &hint, Face::Sans, w) {
            if draw {
                g.text(x, y, 12.0 * s, MUTED, &l, Face::Sans);
            }
            y += lh;
        }
        y += gap;
        y += section(g, x, y, w, s, "Reading and saving", draw);
        let rw = [("Read while it is written".to_string(), self.prefs.read_while_writing, Event::Tab(T_READ_WHILE)), ("Save automatically".to_string(), self.prefs.auto_save, Event::Tab(T_AUTOSAVE))];
        y += flow(g, x, y, w, s, &rw, draw) + 4.0 * s;
        y - y0
    }

    /// The Options popover next to the panel, scrolling inside when it is taller than the room.
    fn options_popover(&mut self, g: &mut Gfx, area: R, panel: R, s: f32, cx: &AppCx) {
        let w = (area.w - 16.0 * s).min(560.0 * s);
        let pad = 16.0 * s;
        let content_h = self.options_content(g, 0.0, 0.0, w - 2.0 * pad - 8.0 * s, s, cx, false);
        let head = 36.0 * s;
        let h = (content_h + head + 12.0 * s).min(area.h - 16.0 * s);
        let x = (panel.x - w - 6.0 * s).max(area.x + 8.0 * s);
        let anchor = self.anchors[2];
        let y = (anchor.y + anchor.h - h).clamp_safe(area.y + 8.0 * s, (area.y + area.h - h - 8.0 * s).max(area.y + 8.0 * s));
        let r = R::new(x, y, w, h);
        g.hit(area, Event::Tab(T_MENU_CLOSE), "close the menu");
        g.rrect(r.x, r.y, r.w, r.h, 14.0 * s, rgb(16, 20, 36));
        g.rrect_outline(r.x, r.y, r.w, r.h, 14.0 * s, 1.5 * s, ACCENT);
        // swallow presses on the panel itself
        g.hit(r, Event::Tick, "options");
        fx::section_title(g, r.x + pad, r.y + 11.0 * s, r.w - 2.0 * pad - 70.0 * s, s, "Options", ACCENT);
        let sz = 13.0 * s;
        let cw = g.text_width(sz, "Close", Face::Bold) + sz * 1.7;
        g.button(r.x + r.w - cw - pad, r.y + 5.0 * s, sz, "Close", MUTED, Event::Tab(T_MENU_CLOSE));
        let vp = R::new(r.x + pad, r.y + head, r.w - 2.0 * pad - 8.0 * s, r.h - head - 8.0 * s);
        self.cmax = (content_h - vp.h).max(0.0);
        self.cscroll = self.cscroll.clamp_safe(0.0, self.cmax);
        let top = vp.y - self.cscroll;
        g.clipped(vp, |g| {
            self.options_content(g, vp.x, top, vp.w, s, cx, true);
        });
        if self.cmax > 0.0 {
            fx::scrollbar(g, R::new(r.x + r.w - 10.0 * s, vp.y + 4.0 * s, 6.0 * s, vp.h - 8.0 * s), s, (self.cscroll / (28.0 * s)) as usize, (vp.h / (28.0 * s)) as usize, (content_h / (28.0 * s)) as usize, ACCENT);
        }
    }

    /// The menus that hang from a chip of the composer, and the reading-aloud panel.
    fn draw_menus(&mut self, g: &mut Gfx, area: R, panel: R, s: f32, cx: &AppCx) {
        let Some(m) = self.menu else { return };
        match m {
            Menu::Kind => {
                let rows: Vec<(String, bool, Event)> = Genre::ALL.iter().enumerate().map(|(i, a)| (a.label().to_string(), i == self.menu_sel, Event::Tab(T_GENRE + i))).collect();
                menu_list(g, self.anchors[0], area, s, &rows, Event::Tab(T_MENU_CLOSE), ACCENT);
            }
            Menu::Tone => {
                let rows: Vec<(String, bool, Event)> = Tone::ALL.iter().enumerate().map(|(i, a)| (a.label().to_string(), i == self.menu_sel, Event::Tab(T_TONE + i))).collect();
                menu_list(g, self.anchors[1], area, s, &rows, Event::Tab(T_MENU_CLOSE), ACCENT);
            }
            Menu::Language => {
                self.options_popover(g, area, panel, s, cx);
                let rows: Vec<(String, bool, Event)> = LANGUAGES.iter().enumerate().map(|(i, a)| (a.to_string(), i == self.menu_sel, Event::Tab(T_LANG + i))).collect();
                menu_list(g, self.anchors[5], area, s, &rows, Event::Tab(T_MENU_CLOSE), ACCENT);
            }
            Menu::Version => {
                let rows: Vec<(String, bool, Event)> = (1..=self.ver_count).map(|k| (format!("Version {k}{}", if k == self.ver { " (in use)" } else { "" }), k as usize == self.menu_sel + 1, Event::Tab(T_VERSION_PICK + k as usize))).collect();
                menu_list(g, self.anchors[6], area, s, &rows, Event::Tab(T_MENU_CLOSE), ACCENT);
            }
            Menu::Options => self.options_popover(g, area, panel, s, cx),
            Menu::How => self.how_panel(g, area, s, cx),
        }
    }

    /// The longer text about reading aloud, in a small panel.
    fn how_panel(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx) {
        let w = 640.0 * s;
        let p = fx::modal(g, area, s, w, 400.0 * s, ACCENT, false);
        let top = fx::modal_title(g, p, s, "Reading aloud", ACCENT);
        let inner = R::new(p.x + 26.0 * s, top + 4.0 * s, p.w - 52.0 * s, p.y + p.h - top - 62.0 * s);
        let mut paras: Vec<(String, [u8; 4])> = vec![(st::status_text(&self.tts), if matches!(self.tts, TtsStatus::Ready { .. }) { OK } else { fx::AMBER })];
        if matches!(self.tts, TtsStatus::NoEngine) {
            paras.push((st::NO_ENGINE_HELP.to_string(), MUTED));
        }
        let mut y = inner.y - self.cscroll;
        let lh = g.line_height(13.0 * s, Face::Sans);
        let mut total = 0.0;
        let quick: Vec<(String, bool, Event)> = if matches!(self.tts, TtsStatus::NoEngine) && cx.session.policy.manage_settings { self.quick.iter().enumerate().take(6).map(|(i, q)| (q.label.clone(), false, Event::Tab(T_QUICK + i))).collect() } else { Vec::new() };
        g.clipped(inner, |g| {
            for (text, col) in &paras {
                for l in g.wrap(13.0 * s, text, Face::Sans, inner.w) {
                    g.text(inner.x, y, 13.0 * s, *col, &l, Face::Sans);
                    y += lh;
                    total += lh;
                }
                y += 8.0 * s;
                total += 8.0 * s;
            }
            if !quick.is_empty() {
                let h = flow(g, inner.x, y, inner.w, s, &quick, true);
                total += h;
            }
        });
        self.cmax = (total - inner.h).max(0.0);
        self.cscroll = self.cscroll.clamp_safe(0.0, self.cmax);
        let sz = 14.0 * s;
        let cw = g.text_width(sz, "Close", Face::Bold) + sz * 1.7;
        g.button(p.x + p.w - 26.0 * s - cw, p.y + p.h - 48.0 * s, sz, "Close", MUTED, Event::Tab(T_MENU_CLOSE));
    }

    // ---------------------------------------------------------------------------------------------- the story view

    fn ensure_layout(&mut self, g: &mut Gfx, size: f32, width: f32) {
        let key = ((size * 100.0) as i32, width as i32);
        if key != self.lay_key {
            self.lay.clear();
            self.lay_key = key;
        }
        let mut units: Vec<(String, UKind)> = Vec::new();
        if !self.story.title.is_empty() {
            units.push((self.story.title.clone(), UKind::Hidden));
        }
        for b in &self.story.blocks {
            units.push((b.text().to_string(), if matches!(b, Block::Heading(_)) { UKind::Heading } else { UKind::Para }));
        }
        if !self.story.partial.trim().is_empty() {
            units.push((self.story.partial.clone(), UKind::Partial));
        }
        self.lay.truncate(units.len());
        for (i, (text, kind)) in units.into_iter().enumerate() {
            if self.lay.get(i).is_some_and(|l| l.text == text && l.kind == kind) {
                continue;
            }
            let (sz, face) = if kind == UKind::Heading { (size * 1.18, Face::Bold) } else { (size, Face::Sans) };
            let lines = if kind == UKind::Hidden { Vec::new() } else { g.wrap(sz, &text, face, width) };
            let chars: Vec<char> = text.chars().collect();
            let mut pos = 0usize;
            let mut with_start = Vec::with_capacity(lines.len());
            for l in lines {
                while pos < chars.len() && chars[pos].is_whitespace() {
                    pos += 1;
                }
                let n = l.chars().count();
                with_start.push((l, pos));
                pos += n;
            }
            let lay = ULay { text, kind, lines: with_start };
            if i < self.lay.len() {
                self.lay[i] = lay;
            } else {
                self.lay.push(lay);
            }
        }
    }

    fn display_title(&self) -> String {
        if !self.story.title.is_empty() {
            self.story.title.clone()
        } else if let Some(e) = &self.saved {
            e.title.clone()
        } else if self.gen.is_none() && !self.story.blocks.is_empty() {
            self.story_opts.topic.chars().take(60).collect()
        } else {
            String::new()
        }
    }

    fn hero(&mut self, g: &mut Gfx, r: R, s: f32, snap_unit: Option<usize>) {
        let hh = r.h;
        let shown_title = self.display_title();
        cover(g, R::new(r.x + 4.0 * s, r.y + 4.0 * s, hh - 8.0 * s, hh - 8.0 * s), s, if shown_title.is_empty() { "Your story" } else { &shown_title });
        let rx = r.x + hh + 14.0 * s;
        let rw = r.w - hh - 14.0 * s;
        let title = if shown_title.is_empty() { "Writing your story…".to_string() } else { shown_title };
        let tsz = if hh >= 110.0 * s { 24.0 * s } else { 20.0 * s };
        let lines = g.wrap(tsz, &title, Face::Bold, rw);
        let lh = g.line_height(tsz, Face::Bold);
        let nl = if hh >= 110.0 * s { 2 } else { 1 };
        let tc = if snap_unit == Some(0) && !self.story.title.is_empty() { ACCENT } else { TEXT };
        for (i, l) in lines.iter().take(nl).enumerate() {
            let l = if i + 1 == nl && lines.len() > nl { g.fit(tsz, &format!("{l}…"), Face::Bold, rw) } else { l.clone() };
            g.text(rx, r.y + 4.0 * s + i as f32 * lh, tsz, tc, &l, Face::Bold);
        }
        let ty = r.y + 4.0 * s + lines.len().min(nl) as f32 * lh + 4.0 * s;
        let o = &self.story_opts;
        let state = if self.saved.is_some() { " · saved" } else if self.gen.is_none() { " · not saved" } else { "" };
        let by = if self.run_model.is_empty() { String::new() } else { format!(" · {}", self.run_model) };
        let version = if self.ver_count > 1 { format!(" · version {}", self.ver) } else { String::new() };
        let meta = format!("{} · {} · {} · {} · {} words{version}{by}{state}", o.audience.label(), o.genre.label(), o.length.id(), o.language, self.story.words());
        let meta = g.fit(13.0 * s, &meta, Face::Sans, rw);
        g.text(rx, ty, 13.0 * s, if self.saved.is_some() { fx::readable(OK, PANEL, 4.5) } else { MUTED }, &meta, Face::Sans);
        // the toolbar along the bottom of the hero, as many buttons as fit
        let by = r.y + hh - 14.0 * s * 2.1 - 4.0 * s;
        let mut bx = rx;
        let mut btns: Vec<(String, usize, [u8; 4], bool)> = Vec::new();
        if self.gen.is_some() {
            btns.push(("Stop writing".into(), T_STOP_WRITING, ERR, true));
        } else {
            if self.saved.is_some() {
                btns.push(("Saved ✓".into(), T_SAVE, OK, false));
            } else {
                btns.push(("Save story".into(), T_SAVE, ACCENT, true));
            }
            if self.held {
                btns.push(("Make the pictures".into(), T_PICS_GO, ACCENT, true));
                btns.push(("No pictures".into(), T_PICS_SKIP, MUTED, true));
            } else if self.pics.iter().any(|p| matches!(p.state, PicState::Working | PicState::Queued)) || self.alt_running.is_some() || !self.alt_jobs.is_empty() {
                let done = self.pics.iter().filter(|p| matches!(p.state, PicState::Ready | PicState::Failed(_))).count();
                btns.push((format!("Cancel pictures {done}/{}", self.pics.len()), T_PICS_CANCEL, ERR, true));
            }
            if let Some(j) = &self.audio_job {
                btns.push((format!("Cancel audio {}/{}", j.done.load(Ordering::SeqCst), j.total), T_CANCEL_AUDIO, ERR, true));
            } else {
                let off = self.no_audio_file && self.saved_audio.is_none();
                btns.push((if self.saved.as_ref().is_some_and(|e| e.has_audio) { "Save audio again".into() } else { "Save audio".into() }, T_SAVE_AUDIO, if off { DIM } else { MUTED }, true));
            }
            if self.ver_count > 1 && self.saved.is_some() {
                btns.push((format!("Version {} ▾", self.ver), T_VERSION_MENU, ACCENT, true));
            }
            if let Some(i) = &self.install {
                btns.push((if i.started { format!("Downloading {}…", i.model) } else { format!("Install {} ({})", i.model, i.size) }, T_PIC_INSTALL, ACCENT, !i.started));
            }
            if self.pics.iter().any(|p| !p.alts.is_empty()) {
                btns.push(("Keep only the chosen".into(), T_KEEP_ONLY, MUTED, true));
            }
            if !self.pics.is_empty() {
                btns.push(("Picture log (g)".into(), T_PICLOG, MUTED, true));
            }
            btns.push(("Export…".into(), T_EXPORT, MUTED, true));
            if let Some(e) = &self.saved {
                btns.push((if e.favourite { "Unfavourite".into() } else { "Favourite".into() }, T_FAV, MUTED, true));
            }
            btns.push(("Copy".into(), T_COPY, MUTED, true));
            btns.push(("New story".into(), T_NEW, MUTED, true));
        }
        for (label, code, col, active) in btns {
            let bsz = 13.0 * s;
            let bw = g.text_width(bsz, &label, Face::Bold) + bsz * 1.7;
            if bx + bw > rx + rw {
                continue;
            }
            if code == T_VERSION_MENU {
                self.anchors[6] = R::new(bx, by, bw, bsz * 2.1);
            }
            if active {
                g.button(bx, by, bsz, &label, col, Event::Tab(code));
            } else {
                // "Saved": shown lit, and still a button (it saves again what has changed since)
                let rr = R::new(bx, by, bw, bsz * 2.1);
                let hot = g.hovered(rr);
                let tc = fx::pill(g, rr, bsz / 14.0, col, hot, true);
                g.text(bx + bsz * 0.85, by + (bsz * 2.1 - g.line_height(bsz, Face::Bold)) / 2.0, bsz, tc, &label, Face::Bold);
                g.hit(rr, Event::Tab(code), &label);
            }
            bx += bw + 8.0 * s;
        }
    }

    /// The size of the slot of a picture inside a text column of width `w`.
    fn pic_slot(w: f32, s: f32) -> (f32, f32) {
        let pw = w.min(520.0 * s);
        (pw, pw * 0.6)
    }

    /// Draw one picture (or its placeholder) in `slot`; the buttons to draw it again sit in its corner.
    fn draw_pic(&mut self, g: &mut Gfx, slot: R, s: f32, idx: usize, tick: u64) {
        let (state, title, bytes, key) = {
            let p = &self.pics[idx];
            (p.state.clone(), p.scene.title.clone(), p.bytes.clone(), p.key)
        };
        let video = matches!(self.pics[idx].ext.as_str(), "mp4" | "webm");
        match (&state, bytes) {
            // a video clip is kept with the story; this view has no video player, so the slot says so and where it is
            (PicState::Ready, Some(b)) if video => {
                cover(g, R::new(slot.x + 4.0 * s, slot.y + 4.0 * s, slot.w - 8.0 * s, slot.h - 8.0 * s), s, &title);
                let label = format!("Video clip ({} KB, {}): saved in the story's pics folder", b.len() / 1024, self.pics[idx].ext);
                let t = g.fit(12.0 * s, &label, Face::Bold, slot.w - 24.0 * s);
                let tw = g.text_width(12.0 * s, &t, Face::Bold);
                g.rrect(slot.x + (slot.w - tw) / 2.0 - 8.0 * s, slot.y + slot.h / 2.0 - 11.0 * s, tw + 16.0 * s, 22.0 * s, 11.0 * s, [6, 8, 20, 200]);
                g.text(slot.x + (slot.w - tw) / 2.0, slot.y + slot.h / 2.0 - 11.0 * s + (22.0 * s - g.line_height(12.0 * s, Face::Bold)) / 2.0, 12.0 * s, CYAN, &t, Face::Bold);
            }
            (PicState::Ready, Some(b)) => {
                g.rrect(slot.x, slot.y, slot.w, slot.h, 12.0 * s, rgb(14, 18, 36));
                match self.pic_cache.draw_bytes(g, key, &b, slot.w, s) {
                    Ok(rd) => {
                        let (pw, ph) = (rd.pm.width() as f32, rd.pm.height() as f32);
                        let k = (slot.w / pw).min(slot.h / ph);
                        let (dw, dh) = (pw * k, ph * k);
                        let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
                        g.pm.draw_pixmap(0, 0, rd.pm.as_ref().as_ref(), &paint, tiny_skia::Transform::from_translate(slot.x + (slot.w - dw) / 2.0, slot.y + (slot.h - dh) / 2.0).pre_scale(k, k), None);
                    }
                    Err(e) => {
                        let m = g.fit(12.0 * s, &format!("The picture cannot be shown: {e}"), Face::Sans, slot.w - 16.0 * s);
                        g.text(slot.x + 8.0 * s, slot.y + slot.h / 2.0, 12.0 * s, ERR, &m, Face::Sans);
                    }
                }
                g.rrect_outline(slot.x, slot.y, slot.w, slot.h, 12.0 * s, 1.0, with_alpha(ACCENT, 90));
            }
            _ => {
                cover(g, R::new(slot.x + 4.0 * s, slot.y + 4.0 * s, slot.w - 8.0 * s, slot.h - 8.0 * s), s, &title);
                let (label, col) = match &state {
                    PicState::Failed(e) => (format!("Could not draw this one: {e}"), ERR),
                    PicState::Working => (format!("Drawing{}", ".".repeat(1 + (tick / 10 % 3) as usize)), CYAN),
                    _ => ("Waiting to be drawn".to_string(), MUTED),
                };
                let t = g.fit(12.0 * s, &label, Face::Bold, slot.w - 24.0 * s);
                let tw = g.text_width(12.0 * s, &t, Face::Bold);
                g.rrect(slot.x + (slot.w - tw) / 2.0 - 8.0 * s, slot.y + 10.0 * s, tw + 16.0 * s, 22.0 * s, 11.0 * s, [6, 8, 20, 190]);
                g.text(slot.x + (slot.w - tw) / 2.0, slot.y + 10.0 * s + (22.0 * s - g.line_height(12.0 * s, Face::Bold)) / 2.0, 12.0 * s, col, &t, Face::Bold);
            }
        }
        // draw this one again
        if !matches!(state, PicState::Working) && self.gen.is_none() && idx < 16 {
            let (label, col) = if matches!(state, PicState::Failed(_)) { ("Try again", ACCENT) } else { ("Draw again", MUTED) };
            let bsz = 12.0 * s;
            let bw = g.text_width(bsz, label, Face::Bold) + bsz * 1.7;
            self.pic_buttons.push((R::new(slot.x + slot.w - bw - 8.0 * s, slot.y + slot.h - bsz * 2.1 - 8.0 * s, bw, bsz * 2.1), label, idx, col));
            let dw = g.text_width(bsz, "Delete", Face::Bold) + bsz * 1.7;
            self.pic_buttons.push((R::new(slot.x + slot.w - bw - dw - 14.0 * s, slot.y + slot.h - bsz * 2.1 - 8.0 * s, dw, bsz * 2.1), "Delete", idx + 100, ERR));
        }
        self.draw_gallery(g, slot, s, idx);
    }

    fn story_view(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        let m = if area.w < 700.0 * s { 14.0 * s } else { 20.0 * s };
        let x = area.x + m;
        let w = area.w - 2.0 * m;
        let mut y = area.y + 8.0 * s;
        y += self.tabs(g, x, y, w, s) + 10.0 * s;
        let empty = self.story.is_empty() && self.gen.is_none();
        if empty {
            let r = R::new(area.x, y, area.w, area.y + area.h - y);
            if let Some(e) = &self.error {
                fx::empty_state(g, r, s, Icon::Tales, "The story could not be written", e, ACCENT);
            } else {
                let hint = if self.pane_open || self.wide { "Describe it in the box below and press Send." } else { "Open Idea & options, describe it and press Send." };
                fx::empty_state(g, r, s, Icon::Tales, "Your story will appear here", hint, ACCENT);
            }
            return;
        }
        let snap = self.reader.snapshot();
        let reading = matches!(snap.status, Status::Playing | Status::Paused) || (matches!(snap.status, Status::Buffering) && self.live);
        let ph = if area.h < 500.0 { 58.0 * s } else { 70.0 * s };
        // one short hint line under the player when no voice is ready, with How for the longer text
        let chip_on = self.chip_wanted();
        let hint_h = if chip_on { 34.0 * s } else if self.read_hint().is_some() { 20.0 * s } else { 0.0 };
        let player = R::new(x, area.y + area.h - ph - 8.0 * s - hint_h, w, ph);
        if chip_on {
            // one actionable chip instead of a message: it looks for a speech engine here, offers the browser voice helper
            // and otherwise opens Settings → Voice
            let fs = 12.0 * s;
            let ty = player.y + player.h + 3.0 * s;
            let label = if self.turning_on { "Looking for a voice…" } else { "Turn on reading aloud" };
            let cw = (g.text_width(13.0 * s, label, Face::Bold) + 13.0 * s * 1.9).min(w);
            chip(g, x + 4.0 * s, ty, s, label, self.turning_on, Event::Tab(T_TURN_ON));
            let hw = g.text_width(fs, "How", Face::Bold) + fs * 1.4;
            let room = w - cw - hw - 28.0 * s;
            let mut tx = x + 4.0 * s + cw + 10.0 * s;
            if room > 70.0 * s {
                let t = g.fit(fs, "Text only for now", Face::Sans, room);
                g.text(tx, ty + (28.0 * s - g.line_height(fs, Face::Sans)) / 2.0, fs, fx::AMBER, &t, Face::Sans);
                tx += g.text_width(fs, &t, Face::Sans) + 8.0 * s;
            }
            let r = R::new(tx, ty, hw, 28.0 * s);
            let hot = g.hovered(r);
            g.text(tx + fs * 0.2, ty + (r.h - g.line_height(fs, Face::Bold)) / 2.0, fs, if hot { ACCENT } else { fx::readable(CYAN, fx::PANEL, 4.5) }, "How", Face::Bold);
            g.rect(tx + fs * 0.2, ty + r.h - 7.0 * s, hw - fs * 0.4, 1.0, if hot { ACCENT } else { with_alpha(CYAN, 120) });
            g.hit(r, Event::Tab(T_HOW), "How");
        } else if let Some((text, col)) = self.read_hint() {
            let fs = 12.0 * s;
            let hw = g.text_width(fs, "How", Face::Bold) + fs * 1.4;
            let ty = player.y + player.h + 2.0 * s;
            let t = g.fit(fs, &text, Face::Sans, (w - hw - 12.0 * s).max(40.0 * s));
            g.text(x + 4.0 * s, ty + (hint_h - 2.0 * s - g.line_height(fs, Face::Sans)) / 2.0, fs, col, &t, Face::Sans);
            let tx = x + 4.0 * s + g.text_width(fs, &t, Face::Sans) + 8.0 * s;
            let r = R::new(tx, ty, hw, hint_h - 2.0 * s);
            let hot = g.hovered(r);
            g.text(tx + fs * 0.2, ty + (r.h - g.line_height(fs, Face::Bold)) / 2.0, fs, if hot { ACCENT } else { fx::readable(CYAN, fx::PANEL, 4.5) }, "How", Face::Bold);
            g.rect(tx + fs * 0.2, ty + r.h - 2.0 * s, hw - fs * 0.4, 1.0, if hot { ACCENT } else { with_alpha(CYAN, 120) });
            g.hit(r, Event::Tab(T_HOW), "How");
        }
        let hh = (area.h * 0.19).clamp_safe(72.0 * s, 150.0 * s);
        let hero = R::new(x, y, w, hh);
        self.hero(g, hero, s, if reading { Some(snap.unit) } else { None });
        y += hh + 10.0 * s;
        let card = R::new(x, y, w, (player.y - 8.0 * s - y).max(40.0 * s));
        let glow = if matches!(snap.status, Status::Playing) { 0.3 + 0.7 * self.level } else { 0.15 };
        glow_card(g, card, s, ACCENT, MAGENTA, glow);
        let pad = 18.0 * s;
        let mut ty = card.y + 12.0 * s;
        let tw = card.w - 2.0 * pad - 12.0 * s;
        let nh = self.banner(g, card.x + pad, ty, tw, s);
        ty += nh;
        let area_txt = R::new(card.x + pad, ty, tw, (card.y + card.h - ty - 12.0 * s).max(20.0));
        let size = 16.0 * s;
        self.ensure_layout(g, size, area_txt.w);
        let lh = g.line_height(size, Face::Sans);
        // heights: the units, with a picture slot before each unit that has one
        let gap = 0.7 * lh;
        let (slot_w, slot_h) = Self::pic_slot(area_txt.w, s);
        let mut tops = Vec::with_capacity(self.lay.len());
        let mut slots: Vec<(usize, f32)> = Vec::new(); // (picture index, y offset)
        let mut total = 0.0;
        for (i, l) in self.lay.iter().enumerate() {
            if l.kind == UKind::Hidden {
                tops.push(total);
                continue;
            }
            if l.kind == UKind::Heading {
                total += gap * 0.5;
            }
            for (pi, p) in self.pics.iter().enumerate() {
                if p.scene.place_before == i {
                    slots.push((pi, total));
                    total += slot_h + self.gallery_h(pi, slot_w, s) + gap;
                }
            }
            tops.push(total);
            let hl = if l.kind == UKind::Heading { g.line_height(size * 1.18, Face::Bold) } else { lh };
            total += l.lines.len() as f32 * hl + if l.kind == UKind::Heading { gap * 0.6 } else { gap };
        }
        if self.gen.is_some() {
            total += lh;
        }
        self.tmax = (total - area_txt.h).max(0.0);
        // follow what is being read, or the end of what is being written
        let hl_range = if reading { Some((snap.unit, snap.range)) } else { None };
        // the word being said (inside the sentence), when the setting asks for it
        let cur_word = if reading && self.highlight.word() { snap.word } else { None };
        if let Some((u, (a, _))) = hl_range {
            if self.manual_unit != Some(u) {
                if let Some(l) = self.lay.get(u) {
                    // follow the word when there is one (a long sentence wraps over several lines), else the sentence
                    let a = cur_word.map_or(a, |w| w.0);
                    let k = l.lines.iter().rposition(|(_, st0)| *st0 <= a).unwrap_or(0);
                    let target = (tops[u] + k as f32 * lh - area_txt.h * 0.3).clamp_safe(0.0, self.tmax);
                    self.tscroll += (target - self.tscroll) * 0.25;
                    if (target - self.tscroll).abs() < 1.0 {
                        self.tscroll = target;
                    }
                }
            }
        } else if self.stick_bottom && self.gen.is_some() {
            self.tscroll = self.tmax;
        }
        self.tscroll = self.tscroll.clamp_safe(0.0, self.tmax);
        let scroll = self.tscroll;
        let lay = std::mem::take(&mut self.lay);
        self.pane.begin(area_txt);
        self.text_area = area_txt;
        self.line_map.clear();
        self.line_tops.clear();
        self.word_rect = None;
        self.sentence_rects.clear();
        if cur_word.is_none() {
            self.word_vis = None;
        }
        self.pic_buttons.clear();
        self.gal_hits.clear();
        let mut ypos = area_txt.y - scroll;
        let base_y = area_txt.y - scroll;
        g.clipped(area_txt, |g| {
            for (pi, off) in &slots {
                let slot = R::new(area_txt.x, base_y + off, slot_w, slot_h);
                if slot.y + slot.h >= area_txt.y && slot.y <= area_txt.y + area_txt.h {
                    self.draw_pic(g, slot, s, *pi, tick);
                }
            }
            for (i, l) in lay.iter().enumerate() {
                if l.kind == UKind::Hidden {
                    continue;
                }
                let (sz, face) = if l.kind == UKind::Heading { (size * 1.18, Face::Bold) } else { (size, Face::Sans) };
                let hl = g.line_height(sz, face);
                ypos = base_y + tops[i];
                let current = hl_range.filter(|(u, _)| *u == i);
                let lit = self.highlight.sentence();
                let base = if l.kind == UKind::Heading { ACCENT } else if l.kind == UKind::Partial { MUTED } else if reading && lit && current.is_none() { MUTED } else { TEXT };
                if current.is_some() && lit {
                    // a soft bar along the paragraph being read
                    g.rrect(area_txt.x - 10.0 * s, ypos - 2.0 * s, 4.0 * s, l.lines.len() as f32 * hl + 4.0 * s, 2.0 * s, ACCENT);
                }
                for (li, (line, ls)) in l.lines.iter().enumerate() {
                    let ly = ypos + li as f32 * hl;
                    if ly + hl >= area_txt.y - hl && ly <= area_txt.y + area_txt.h + hl {
                        // the characters of this line that are inside the range (sentence, then word), as pixels
                        let n = line.chars().count();
                        let span_of = |g: &mut Gfx, a: usize, b: usize| -> Option<(usize, usize, f32, f32)> {
                            let (lo, hi) = (a.max(*ls), b.min(ls + n));
                            if lo >= hi {
                                return None;
                            }
                            let pre: String = line.chars().take(lo - ls).collect();
                            let mid: String = line.chars().skip(lo - ls).take(hi - lo).collect();
                            Some((lo - ls, hi - ls, g.text_width(sz, &pre, face), g.text_width(sz, &mid, face)))
                        };
                        if let (true, Some((_, (a, b)))) = (lit, current) {
                            if let Some((_, _, off, mw)) = span_of(g, a, b) {
                                g.rrect(area_txt.x + off - 3.0 * s, ly - 1.0, mw + 6.0 * s, hl + 2.0, 5.0 * s, with_alpha(ACCENT, 70));
                                self.sentence_rects.push(R::new(area_txt.x + off - 3.0 * s, ly - 1.0, mw + 6.0 * s, hl + 2.0));
                            }
                        }
                        // the word being said: a solid rounded fill under dark text, easing from the word before it
                        let mut word_at: Option<(usize, usize, f32, f32)> = None;
                        if let (Some((_, _)), Some((wa, wb))) = (current, cur_word) {
                            if let Some((c0, c1, off, mw)) = span_of(g, wa, wb) {
                                let (ex, ew) = match self.word_vis {
                                    Some((u, l0, x, w)) if u == i && l0 == li => (x + (off - x) * 0.6, w + (mw - w) * 0.6),
                                    _ => (off, mw),
                                };
                                self.word_vis = Some((i, li, ex, ew));
                                g.rrect(area_txt.x + ex - 3.0 * s, ly - 1.0, ew + 6.0 * s, hl + 2.0, 5.0 * s, WORD_FILL);
                                self.word_rect = Some(R::new(area_txt.x + ex - 3.0 * s, ly - 1.0, ew + 6.0 * s, hl + 2.0));
                                word_at = Some((c0, c1, off, mw));
                            }
                        }
                        let shown = if l.kind == UKind::Partial && li + 1 == l.lines.len() { format!("{line}{}", ".".repeat(1 + (tick / 10 % 3) as usize)) } else { line.clone() };
                        match word_at {
                            Some((c0, c1, off, _)) => {
                                // three runs, so the word can be dark on its fill: before, the word, after
                                let chars: Vec<char> = shown.chars().collect();
                                let (c1, c0) = (c1.min(chars.len()), c0.min(chars.len()));
                                let (a, m, z): (String, String, String) = (chars[..c0].iter().collect(), chars[c0..c1].iter().collect(), chars[c1..].iter().collect());
                                g.text(area_txt.x, ly, sz, base, &a, face);
                                g.text(area_txt.x + off, ly, sz, WORD_INK, &m, face);
                                let zx = g.text_width(sz, &format!("{a}{m}"), face);
                                g.text(area_txt.x + zx, ly, sz, base, &z, face);
                            }
                            None => {
                                g.text(area_txt.x, ly, sz, base, &shown, face);
                            }
                        }
                    }
                    self.line_map.push((i, *ls));
                    self.line_tops.push(ly);
                    self.pane.line(g, area_txt.x, ly, hl, sz, face, line, i as u32, i as u32);
                }
            }
            if self.gen.is_some() && lay.is_empty() {
                g.text(area_txt.x, area_txt.y, size, MUTED, &format!("The storyteller is thinking{}", ".".repeat(1 + (tick / 10 % 3) as usize)), Face::Sans);
            }
        });
        let _ = ypos;
        self.lay = lay;
        self.pane.end(g, ACCENT);
        // the buttons of the pictures go on top of the text area's press region
        let gal = std::mem::take(&mut self.gal_hits);
        g.clipped(area_txt, |g| {
            for (r, code, label) in &gal {
                if *code >= T_ALT_MORE {
                    g.button(r.x, r.y, 12.0 * s, label, ACCENT, Event::Tab(*code));
                } else {
                    g.hit(*r, Event::Tab(*code), label);
                }
            }
        });
        let btns = std::mem::take(&mut self.pic_buttons);
        g.clipped(area_txt, |g| {
            for (r, label, idx, col) in &btns {
                g.button(r.x, r.y, 12.0 * s, label, *col, Event::Tab(if *idx >= 100 { T_PIC_DELETE + idx - 100 } else { T_PIC_REDO + idx }));
            }
        });
        if let Some(e) = &self.error {
            // the failure, under the text (or alone)
            let es = g.fit(13.0 * s, e, Face::Sans, area_txt.w);
            g.rrect(area_txt.x - 6.0 * s, area_txt.y + area_txt.h - 24.0 * s, area_txt.w + 12.0 * s, 24.0 * s, 8.0 * s, fx::over(PANEL, with_alpha(ERR, 60)));
            g.text(area_txt.x, area_txt.y + area_txt.h - 20.0 * s, 13.0 * s, ERR, &es, Face::Sans);
        }
        if self.tmax > 0.0 {
            fx::scrollbar(g, R::new(card.x + card.w - 14.0 * s, area_txt.y, 6.0 * s, area_txt.h), s, (self.tscroll / lh) as usize, (area_txt.h / lh) as usize, (total / lh) as usize, ACCENT);
        }
        self.player_bar(g, player, s, &snap, tick);
    }

    fn player_bar(&mut self, g: &mut Gfx, r: R, s: f32, snap: &st::Snapshot, tick: u64) {
        let active = matches!(snap.status, Status::Playing);
        glow_card(g, r, s, CYAN, MAGENTA, if active { 0.3 + 0.7 * self.level } else { 0.1 });
        let d = (r.h * 0.5).max(30.0 * s);
        let cy = r.y + r.h / 2.0;
        let mut x = r.x + 16.0 * s;
        let btn = |g: &mut Gfx, x: f32, dd: f32, glyph: Glyph, lit: Option<[u8; 4]>, code: usize, label: &str| {
            let rr = R::new(x, cy - dd / 2.0, dd, dd);
            round_button(g, rr, s, glyph, lit);
            g.hit(rr, Event::Tab(code), label);
        };
        btn(g, x, d, Glyph::Previous, None, T_PREV, "Previous part");
        x += d + 8.0 * s;
        // play / pause is the big one
        let pd = d + 12.0 * s;
        let rr = R::new(x, cy - pd / 2.0, pd, pd);
        let lit = if active { MAGENTA } else { CYAN };
        round_button(g, rr, s, Glyph::Play, Some(lit));
        if matches!(snap.status, Status::Playing | Status::Buffering) {
            // cover the triangle with two bars
            g.circle(rr.x + rr.w / 2.0, rr.y + rr.h / 2.0, rr.w * 0.3, fx::over(PANEL, with_alpha(lit, 120)));
            let k = rr.w * 0.17;
            g.rect(rr.x + rr.w / 2.0 - k * 0.95, rr.y + rr.h / 2.0 - k, k * 0.7, k * 2.0, TEXT);
            g.rect(rr.x + rr.w / 2.0 + k * 0.25, rr.y + rr.h / 2.0 - k, k * 0.7, k * 2.0, TEXT);
        }
        g.hit(rr, Event::Tab(T_PLAY), if matches!(snap.status, Status::Playing | Status::Buffering) { "Pause" } else { "Play" });
        x += pd + 8.0 * s;
        btn(g, x, d, Glyph::Stop, None, T_STOP, "Stop");
        x += d + 8.0 * s;
        btn(g, x, d, Glyph::Next, None, T_NEXT, "Next part");
        x += d + 14.0 * s;
        // replay as a small text button when it fits
        let rs = 12.0 * s;
        let rw = g.text_width(rs, "Replay", Face::Bold) + rs * 1.7;
        let right_min = 250.0 * s;
        if x + rw + right_min < r.x + r.w {
            g.button(x, cy - rs * 1.05, rs, "Replay", MUTED, Event::Tab(T_REPLAY));
            x += rw + 12.0 * s;
        }
        // the right side: voice and speed chips (when the engine has them), the level bars
        let mut rx = r.x + r.w - 14.0 * s;
        let eng = matches!(self.tts, TtsStatus::Ready { .. }) && self.saved_audio.is_none();
        if self.saved_audio.is_some() {
            let w = chip_w(g, s, "saved audio");
            rx -= w;
            fx::chip(g, rx, cy - 12.0 * s, s, "saved audio", OK, false);
            rx -= 10.0 * s;
        }
        if eng && self.takes_speed {
            let label = format!("{:.2}x", self.prefs.speed).replace(".00x", ".0x");
            let w = g.text_width(13.0 * s, &label, Face::Bold) + 13.0 * s * 1.9;
            if rx - w > x + 220.0 * s {
                rx -= w;
                chip(g, rx, cy - 14.0 * s, s, &label, false, Event::Tab(T_SPEED));
                rx -= 8.0 * s;
            }
        }
        if eng && self.voices.len() > 1 {
            let v = self.prefs.voice.clone().unwrap_or_else(|| self.voices[0].clone());
            let label = format!("Voice: {}", voice_label(&v));
            let w = g.text_width(13.0 * s, &label, Face::Bold) + 13.0 * s * 1.9;
            if rx - w > x + 220.0 * s {
                rx -= w;
                chip(g, rx, cy - 14.0 * s, s, &label, false, Event::Tab(T_VOICE));
                rx -= 8.0 * s;
            }
        } else if eng && !self.voices.is_empty() {
            let label = format!("Voice: {}", voice_label(&self.voices[0]));
            let w = g.text_width(12.0 * s, &label, Face::Sans);
            if rx - w > x + 220.0 * s {
                rx -= w;
                g.text(rx, cy - g.line_height(12.0 * s, Face::Sans) / 2.0, 12.0 * s, DIM, &label, Face::Sans);
                rx -= 10.0 * s;
            }
        }
        // waveform
        let bars = 18usize;
        let ww = (r.w * 0.11).clamp_safe(60.0 * s, 150.0 * s);
        let wavex = rx - ww;
        if wavex > x + 160.0 * s {
            let maxh = r.h * 0.6;
            for i in 0..bars {
                let t = i as f32 / (bars - 1) as f32;
                let bw = ww / bars as f32;
                let wob = (tick as f32 * 0.35 + i as f32 * 0.9).sin() * 0.5 + 0.5;
                let lvl = match snap.status {
                    Status::Playing => (self.level * (0.35 + 0.65 * wob)).max(0.06),
                    Status::Buffering => 0.08 + 0.12 * ((tick as f32 * 0.12 + i as f32 * 0.5).sin() * 0.5 + 0.5),
                    _ => 0.05,
                };
                let hgt = (lvl * maxh).max(3.0 * s);
                let c = mix(CYAN, MAGENTA, t);
                g.rrect(wavex + i as f32 * bw, cy - hgt / 2.0, bw - 2.0 * s, hgt, 2.0 * s, if matches!(snap.status, Status::Playing | Status::Buffering) { c } else { with_alpha(c, 90) });
            }
            rx = wavex - 14.0 * s;
        }
        // progress and what is going on
        let px = x;
        let pw = (rx - px).max(40.0 * s);
        let (txt, col) = self.status_line(snap);
        let sz = 13.0 * s;
        let txt = g.fit(sz, &txt, Face::Sans, pw);
        let th = g.line_height(sz, Face::Sans);
        g.text(px, cy - th - 6.0 * s, sz, col, &txt, Face::Sans);
        let bar = R::new(px, cy + 4.0 * s, pw, 8.0 * s);
        fx::progress_bar(g, bar, snap.fraction, if active { MAGENTA } else { CYAN }, s);
        g.hit(R::new(bar.x, bar.y - 8.0 * s, bar.w, bar.h + 16.0 * s), Event::Area(BAR_AREA), "progress");
        self.bar_rect = Some(bar);
    }

    fn status_line(&self, snap: &st::Snapshot) -> (String, [u8; 4]) {
        if let Status::Failed(m) = &snap.status {
            return (m.clone(), ERR);
        }
        if let Some(j) = &self.audio_job {
            return (format!("Saving the audio… {} of {}", j.done.load(Ordering::SeqCst), j.total), MAGENTA);
        }
        let more = if self.gen.is_some() { " (still writing)" } else { "" };
        match snap.status {
            // where the lit word's times come from, said quietly (native, aligned, from the voice, estimated)
            Status::Playing if self.highlight.word() && snap.word.is_some() => (format!("Reading part {} of {}{more} · words {}", snap.chunk + 1, snap.chunks, snap.timing.label()), CYAN),
            Status::Playing => (format!("Reading part {} of {}{more}", snap.chunk + 1, snap.chunks), CYAN),
            Status::Buffering if self.live => (format!("Getting the voice ready… part {} of {}{more}", snap.chunk + 1, snap.chunks.max(1)), MUTED),
            Status::Paused => (format!("Paused at part {} of {}", snap.chunk + 1, snap.chunks), MUTED),
            Status::Done => ("Finished. Press play to listen again.".into(), OK),
            _ if self.gen.is_some() => (if self.run_model.is_empty() { "Writing the story…".into() } else { format!("Writing the story with {}…", self.run_model) }, MAGENTA),
            _ if self.can_read() => (if self.saved_audio.is_some() { "Saved audio is ready. Press play.".into() } else { "Press play to read it aloud.".into() }, MUTED),
            _ => ("Text only: reading aloud is off. Use Turn on reading aloud under the player.".into(), fx::AMBER),
        }
    }

    /// The story page: the story view on the left and the conversation panel on the right (below 900 px the panel is a toggle).
    fn story_screen(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        let hl = cx.services.voice_settings().highlight;
        if hl != self.highlight {
            self.highlight = hl;
            self.reader.set_words(hl.word());
        }
        self.wide = area.w >= WIDE_PX;
        self.area_rect = area;
        self.scale = s;
        self.panel_hover = false;
        let show = self.wide || self.pane_open;
        let pw = (self.panel_w * s).clamp_safe(270.0 * s, area.w * if self.wide { 0.6 } else { 0.65 });
        let (main, panel) = if show { (R::new(area.x, area.y, area.w - pw, area.h), R::new(area.x + area.w - pw, area.y, pw, area.h)) } else { (area, R::new(area.x + area.w, area.y, 0.0, area.h)) };
        self.story_view(g, main, s, tick);
        if show {
            self.panel(g, panel, s, cx, tick, area);
            // the divider, to drag the panel wider or narrower
            let dw = (10.0 * s).max(9.0);
            let d = R::new(panel.x - dw / 2.0, area.y, dw, area.h);
            self.div_rect = d;
            let hot = g.hovered(d) || self.drag_o.is_some() && crate::ui::dragging();
            if hot {
                g.rrect(d.x + d.w / 2.0 - 1.5 * s, d.y + 8.0 * s, 3.0 * s, d.h - 16.0 * s, 1.5 * s, with_alpha(ACCENT, 190));
            }
            g.hit(d, Event::Area(DIV_AREA), "divider");
        }
        // the export menu
        if let Some(Modal::Export(p, _)) = &self.modal {
            p.render(g, area, s, ACCENT);
        }
        self.draw_menus(g, area, panel, s, cx);
        self.draw_model_pick(g, area, s, cx);
        self.draw_pic_log(g, area, s);
    }

    // ---------------------------------------------------------------------------------------------- library page

    fn library_page(&mut self, g: &mut Gfx, area: R, s: f32, tick: u64) {
        let m = if area.w < 700.0 * s { 14.0 * s } else { 24.0 * s };
        let x = area.x + m;
        let w = area.w - 2.0 * m;
        let mut y = area.y + 8.0 * s;
        y += self.tabs(g, x, y, w, s) + 12.0 * s;
        let fr = R::new(x, y, w.min(520.0 * s), 38.0 * s);
        self.search.render(g, fr, self.focus == Focus::Search && self.modal.is_none(), s, blink(tick), Face::Sans, SEARCH_FIELD);
        let count = format!("{} {}", self.lib.len(), if self.lib.len() == 1 { "story" } else { "stories" });
        g.text(fr.x + fr.w + 14.0 * s, fr.y + (fr.h - g.line_height(13.0 * s, Face::Sans)) / 2.0, 13.0 * s, MUTED, &count, Face::Sans);
        y += fr.h + 10.0 * s;
        let nh = self.banner(g, x, y, w, s);
        y += nh;
        if self.lib.is_empty() {
            let r = R::new(area.x, y, area.w, area.y + area.h - y);
            if self.search.value.trim().is_empty() {
                fx::empty_state(g, r, s, Icon::Tales, "No saved stories yet", "Stories you create are kept here, with their pictures and audio, so you can listen again.", ACCENT);
            } else {
                fx::empty_state(g, r, s, Icon::Tales, "Nothing matches", "Try fewer words, or clear the search.", ACCENT);
            }
        } else {
            let rh = 60.0 * s;
            let fit = (((area.y + area.h - y - 8.0 * s) / (rh + 6.0 * s)).floor() as usize).max(1);
            self.lib_top = self.lib_top.min(self.lib.len().saturating_sub(1));
            if self.lib_sel < self.lib_top {
                self.lib_top = self.lib_sel;
            } else if self.lib_sel >= self.lib_top + fit {
                self.lib_top = self.lib_sel + 1 - fit;
            }
            let rows = self.lib.clone();
            for (i, e) in rows.iter().enumerate().skip(self.lib_top).take(fit) {
                let rect = R::new(x, y, w - 12.0 * s, rh);
                let sel = i == self.lib_sel;
                let hot = g.hovered(rect);
                fx::list_row(g, rect, s, sel, hot, ACCENT);
                g.hit(rect, Event::Row(i), &e.title);
                let cv = R::new(rect.x + 8.0 * s, rect.y + 7.0 * s, rh - 14.0 * s, rh - 14.0 * s);
                cover(g, cv, s * 0.6, &e.title);
                let tx = cv.x + cv.w + 14.0 * s;
                // the actions of the chosen row on the right; the title gets the rest
                let mut right = rect.x + rect.w - 10.0 * s;
                if sel {
                    let mut acts: Vec<(&str, usize, [u8; 4])> = vec![("Listen", T_LIB_LISTEN, ACCENT), ("Open", T_LIB_OPEN, MUTED), (if e.favourite { "Unfavourite" } else { "Favourite" }, T_LIB_FAV, MUTED), ("Export…", T_LIB_EXPORT, MUTED), ("Delete", T_LIB_DELETE, ERR)];
                    let bsz = 12.0 * s;
                    let mut keep = Vec::new();
                    let mut used = 0.0;
                    let room = rect.w * 0.52;
                    for a in acts.drain(..) {
                        let bw = g.text_width(bsz, a.0, Face::Bold) + bsz * 1.7 + 6.0 * s;
                        if used + bw <= room {
                            used += bw;
                            keep.push((a, bw));
                        }
                    }
                    for (a, bw) in keep.into_iter().rev() {
                        right -= bw;
                        g.button(right, rect.y + (rect.h - bsz * 2.1) / 2.0, bsz, a.0, a.2, Event::Tab(a.1));
                    }
                    right -= 6.0 * s;
                } else {
                    if e.has_audio {
                        let cw = chip_w(g, s, "audio");
                        right -= cw;
                        fx::chip(g, right, rect.y + (rect.h - 22.0 * s) / 2.0, s, "audio", OK, false);
                        right -= 8.0 * s;
                    }
                    if e.pictures > 0 {
                        let cw = chip_w(g, s, "pictures");
                        right -= cw;
                        fx::chip(g, right, rect.y + (rect.h - 22.0 * s) / 2.0, s, "pictures", CYAN, false);
                        right -= 8.0 * s;
                    }
                    if e.favourite {
                        right -= 20.0 * s;
                        heart(g, right + 10.0 * s, rect.y + rect.h / 2.0 - 2.0 * s, 16.0 * s, MAGENTA, true);
                    }
                }
                let tw = (right - tx - 10.0 * s).max(30.0);
                let t = g.fit(16.0 * s, &e.title, Face::Bold, tw);
                if e.favourite && sel {
                    heart(g, tx + 6.0 * s, rect.y + 17.0 * s, 13.0 * s, MAGENTA, true);
                    let t2 = g.fit(16.0 * s, &e.title, Face::Bold, tw - 18.0 * s);
                    g.text(tx + 18.0 * s, rect.y + 8.0 * s, 16.0 * s, TEXT, &t2, Face::Bold);
                } else {
                    g.text(tx, rect.y + 8.0 * s, 16.0 * s, TEXT, &t, Face::Bold);
                }
                let aud = Audience::from_id(&e.audience).map_or("", |a| a.label());
                let gen = Genre::from_id(&e.genre).map_or("", |a| a.label());
                let meta = format!("{aud} · {gen} · {} · {} words · {}", e.language, e.words, ymd(e.created));
                let meta = g.fit(12.0 * s, &meta, Face::Sans, tw);
                g.text(tx, rect.y + 34.0 * s, 12.0 * s, fx::readable(MUTED, rgb(30, 38, 68), 4.5), &meta, Face::Sans);
                y += rh + 6.0 * s;
            }
            if self.lib.len() > fit {
                fx::scrollbar(g, R::new(x + w - 6.0 * s, area.y + 90.0 * s, 6.0 * s, area.h - 100.0 * s), s, self.lib_top, fit, self.lib.len(), ACCENT);
            }
        }
        match &self.modal {
            Some(Modal::ConfirmDelete(_, title)) => {
                let shown = g.fit(19.0 * s, title, Face::Bold, 300.0 * s);
                super::widgets::render_confirm(g, area, s, &format!("Delete \"{shown}\" and everything saved with it?"), "Delete");
            }
            Some(Modal::Export(p, _)) => p.render(g, area, s, ACCENT),
            Some(Modal::Model(_)) | Some(Modal::PicLog(_)) | None => {}
        }
    }
}

impl App for StoryApp {
    fn keeps_screen_on(&self) -> bool {
        matches!(self.reader.status(), Status::Playing | Status::Buffering)
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }
    fn title(&self) -> &'static str {
        "Tales"
    }
    fn icon(&self) -> Icon {
        Icon::Tales
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        match self.page {
            Page::Story => "Enter send · Ctrl+J new line · Ctrl+P model or agent · Ctrl+O options · Space play · Ctrl+S save · e export · l saved stories · Esc back".into(),
            Page::Library => "↑↓ choose · Enter open · Space listen · f favourite · d delete · / search".into(),
        }
    }
    fn help(&self) -> Vec<String> {
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

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if matches!(ev, Event::Tick) {
            self.ticks += 1;
            self.poll(cx);
            // a plain click in the text that waited out the double-click time starts reading from there
            if let Some((x, y, n)) = self.pending_read {
                if n > 1 {
                    self.pending_read = Some((x, y, n - 1));
                } else {
                    self.pending_read = None;
                    self.read_from_point(x, y, cx);
                }
            }
            return Flow::Stay;
        }
        self.dirty = true;
        if self.page == Page::Story && self.modal.is_none() {
            match ev {
                Event::AreaAt(TEXT_PANE, x, y) => {
                    self.pending_read = None;
                    self.click_read = (!crate::ui::dragging() && crate::ui::click_count() == 1 && !crate::ui::shift_held()).then_some((*x, *y));
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
        if matches!(self.modal, Some(Modal::ConfirmDelete(..))) && !matches!(ev, Event::Key(_)) {
            return Flow::Stay;
        }
        if matches!(self.modal, Some(Modal::Model(_))) && !matches!(ev, Event::Key(_)) {
            self.model_event(ev, cx);
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
                if !crate::ui::dragging() {
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
                self.click(*n, cx);
            }
            Event::AreaAt(BAR_AREA, x, _) => {
                if let (Some(bar), true) = (self.bar_rect, !self.chunks.is_empty()) {
                    let f = (*x / bar.w.max(1.0)).clamp(0.0, 1.0);
                    let idx = (f * self.chunks.len() as f32) as usize;
                    self.seek(idx, cx);
                }
            }
            Event::Key(k) => return self.key(k, cx),
            _ => {}
        }
        Flow::Stay
    }

    fn wants_redraw(&mut self) -> bool {
        if self.animating() {
            self.ticks % 3 == 0
        } else {
            std::mem::take(&mut self.dirty)
        }
    }

    fn take_host_save(&mut self) -> Option<(String, Vec<u8>)> {
        self.host_save.take()
    }

    fn context_items(&mut self, x: i32, y: i32) -> Vec<crate::ctxmenu::Item> {
        if self.page != Page::Story {
            return Vec::new();
        }
        self.pane.context_items(x, y)
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        match self.page {
            Page::Story => self.story_screen(g, area, s, cx, tick),
            Page::Library => self.library_page(g, area, s, tick),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::{cx_with_backend, services_in, temp_dir, Offline};
    use iyakki_agent::chat::{ChatBackend, Message, ModelTurn, Role as ChatRole, ToolSpec};
    use iyakki_agent::voice::{encode_wav, tone, SpeechEngine};
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Duration;

    const STORY: &str = "# The Brave Little Fox\n\nOnce upon a time a small fox lived near the river. She was afraid of the dark.\n\nOne night the moon was hiding. \"Do not worry,\" said an owl. The fox listened carefully.\n\nShe walked home with a quiet heart, and she slept well.\n";

    /// A model that records what it was asked and streams a story in small pieces.
    struct Teller {
        seen: Mutex<Vec<(Vec<Message>, usize)>>,
        text: String,
        fail: Option<String>,
        delay_ms: u64,
        /// What the model answers when asked for a drawing.
        svg: Mutex<String>,
        /// What it answers when asked to choose a scene for the built-in cartoon.
        scene: Mutex<String>,
        /// When set, every drawing request fails with this.
        draw_error: Mutex<Option<String>>,
        /// Stories to answer with, one per request (then `text` again).
        queue: Mutex<Vec<String>>,
    }

    const SVG: &str = "<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 400 240'><defs><linearGradient id='sky' x1='0' y1='0' x2='0' y2='1'><stop offset='0' stop-color='#223366'/><stop offset='1' stop-color='#88aadd'/></linearGradient></defs><rect width='400' height='240' fill='url(#sky)'/><circle cx='300' cy='60' r='28' fill='#ffeeaa'/><ellipse cx='120' cy='190' rx='90' ry='40' fill='#336644'/></svg>";

    const SCENE: &str = "{\"setting\":\"meadow\",\"time\":\"day\",\"mood\":\"happy\",\"objects\":[{\"kind\":\"hero\",\"x\":30},{\"kind\":\"tree\",\"x\":85,\"size\":\"large\"}]}";

    impl Teller {
        fn new(text: &str) -> Arc<Teller> {
            Arc::new(Teller { seen: Mutex::new(vec![]), text: text.into(), fail: None, delay_ms: 0, svg: Mutex::new(SVG.into()), scene: Mutex::new(SCENE.into()), draw_error: Mutex::new(None), queue: Mutex::new(vec![]) })
        }
        fn with(text: &str, fail: Option<&str>, delay_ms: u64) -> Arc<Teller> {
            Arc::new(Teller { seen: Mutex::new(vec![]), text: text.into(), fail: fail.map(str::to_string), delay_ms, svg: Mutex::new(SVG.into()), scene: Mutex::new(SCENE.into()), draw_error: Mutex::new(None), queue: Mutex::new(vec![]) })
        }
        /// The next requests for a story are answered with these texts, in order.
        fn queue(&self, texts: &[&str]) {
            *self.queue.lock().unwrap() = texts.iter().map(|t| t.to_string()).collect();
        }
        /// The requests that asked for a story.
        fn story_calls(&self) -> Vec<(Vec<Message>, usize)> {
            self.seen.lock().unwrap().iter().filter(|c| c.0[0].content.contains("IYAKKI Tales")).cloned().collect()
        }
        /// The requests that asked for a drawing.
        fn drawing_calls(&self) -> Vec<(Vec<Message>, usize)> {
            self.seen.lock().unwrap().iter().filter(|c| c.0[0].content.contains("small illustration") || c.0[0].content.contains("children's-book illustrator")).cloned().collect()
        }
    }

    struct Shared(Arc<Teller>);
    impl ChatBackend for Shared {
        fn complete(&self, h: &[Message]) -> Result<String, String> {
            self.0.seen.lock().unwrap().push((h.to_vec(), 0));
            if h[0].content.contains("picture brief") {
                return Ok("A small fox sits by a quiet river as the moon rises.".into());
            }
            if h[0].content.contains("where and when a story happens") {
                return Ok("a river meadow at dusk, soft blue light".into());
            }
            if h[0].content.contains("small illustration") || h[0].content.contains("children's-book illustrator") {
                if let Some(e) = self.0.draw_error.lock().unwrap().clone() {
                    return Err(e);
                }
                return Ok(if h[0].content.contains("scene composer") { self.0.scene.lock().unwrap().clone() } else { self.0.svg.lock().unwrap().clone() });
            }
            self.0.fail.clone().map_or(Ok(self.0.text.clone()), Err)
        }
        fn complete_tools_stream(&self, h: &[Message], tools: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
            self.0.seen.lock().unwrap().push((h.to_vec(), tools.len()));
            if let Some(e) = &self.0.fail {
                return Err(e.clone());
            }
            let full = {
                let mut q = self.0.queue.lock().unwrap();
                if q.is_empty() { self.0.text.clone() } else { q.remove(0) }
            };
            let chars: Vec<char> = full.chars().collect();
            let mut sent = String::new();
            for piece in chars.chunks(23) {
                let p: String = piece.iter().collect();
                if self.0.delay_ms > 0 {
                    std::thread::sleep(Duration::from_millis(self.0.delay_ms));
                }
                sent.push_str(&p);
                if !on_text(&p) {
                    break;
                }
            }
            Ok(ModelTurn { text: sent, calls: vec![] })
        }
    }

    fn make(tag: &str, teller: &Arc<Teller>) -> (StoryApp, AppCx, PathBuf) {
        make_up(tag, teller, Arc::new(Offline))
    }

    fn make_up(tag: &str, teller: &Arc<Teller>, up: Arc<dyn iyakki_agent::iyakki_proxy::Upstream>) -> (StoryApp, AppCx, PathBuf) {
        let (mut cx, d) = cx_with_backend(tag, Shared(teller.clone()));
        cx.services = services_in(&temp_dir(&format!("{tag}-svc")), up);
        let home = d.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let home = std::fs::canonicalize(home).unwrap();
        cx.session.policy.fs_read = vec![home.to_string_lossy().into_owned()];
        cx.session.policy.fs_write = vec![home.to_string_lossy().into_owned()];
        let mut app = StoryApp::new(&cx);
        app.store = StoryStore::open_at(&cx.session.policy, "tester", cx.services.dir(), home.clone());
        app.prefs = app.store.load_prefs();
        (app, cx, home)
    }

    fn typ(a: &mut StoryApp, cx: &AppCx, s: &str) {
        for c in s.chars() {
            a.handle(&Event::Key(Key::Char(c)), cx);
        }
    }

    fn key(a: &mut StoryApp, cx: &AppCx, k: Key) -> Flow {
        a.handle(&Event::Key(k), cx)
    }

    fn click(a: &mut StoryApp, cx: &AppCx, code: usize) {
        a.handle(&Event::Tab(code), cx);
    }

    /// Click the control named `label` of the last drawing.
    fn click_hit(a: &mut StoryApp, cx: &AppCx, g: &Gfx, label: &str) {
        let ev = g.hits.iter().rev().find(|h| h.label == label).unwrap_or_else(|| panic!("no control named {label}: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>())).ev.clone();
        a.handle(&ev, cx);
    }

    /// Run the heartbeat until `f` holds (or panic after ~8 s).
    fn until(a: &mut StoryApp, cx: &AppCx, what: &str, f: impl Fn(&mut StoryApp) -> bool) {
        // a wall-clock wait: what is waited for runs processes, whose speed depends on the load of the machine
        let deadline = Instant::now() + Duration::from_secs(60);
        while Instant::now() < deadline {
            a.handle(&Event::Tick, cx);
            if f(a) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("timed out waiting for: {what}; story page {:?}, error {:?}, reader {:?}, pictures {:?}, note {:?}", a.page_name(), a.error, a.reader.status(), a.pics.iter().map(|p| p.state.clone()).collect::<Vec<_>>(), a.note);
    }

    fn write_story(a: &mut StoryApp, cx: &AppCx, topic: &str) {
        a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), cx);
        typ(a, cx, topic);
        key(a, cx, Key::Ctrl('n'));
        until(a, cx, "the story to be written", |a| !a.is_writing());
    }

    fn draw(a: &mut StoryApp, cx: &AppCx, w: u32, h: u32, tick: u64) -> (Gfx, R) {
        let s = crate::shell::scale_for(w, h);
        draw_at(a, cx, w, h, s, tick)
    }

    fn draw_at(a: &mut StoryApp, cx: &AppCx, w: u32, h: u32, s: f32, tick: u64) -> (Gfx, R) {
        let mut g = Gfx::new(w, h);
        g.clear(rgb(9, 11, 20));
        let top = 46.0 * s;
        let area = R::new(0.0, top, w as f32, h as f32 - top);
        a.render(&mut g, area, s, cx, tick);
        (g, area)
    }

    /// A speech engine that is a script: it logs what it was asked to say, then prints a short WAV (or fails when `fail` exists).
    struct Engine {
        dir: PathBuf,
    }
    impl Engine {
        fn install(cx: &AppCx, dir: &Path, play: &str) -> Engine {
            std::fs::create_dir_all(dir).unwrap();
            std::fs::write(dir.join("tone.wav"), encode_wav(&tone(440.0, 120), 16_000)).unwrap();
            let script = dir.join("say.sh");
            std::fs::write(&script, format!("#!/bin/sh\ncat >> '{d}/said.log'\necho >> '{d}/said.log'\nif [ -f '{d}/fail' ]; then echo 'engine exploded' >&2; exit 3; fi\ncat '{d}/tone.wav'\n", d = dir.display())).unwrap();
            std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
            cx.services
                .settings
                .lock()
                .unwrap()
                .update(|s| {
                    s.voice.tts = Some(SpeechEngine::Command { command: script.display().to_string() });
                    s.voice.play_cmd = play.to_string();
                })
                .unwrap();
            Engine { dir: dir.to_path_buf() }
        }
        fn said(&self) -> Vec<String> {
            std::fs::read_to_string(self.dir.join("said.log")).unwrap_or_default().lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
        }
    }

    fn logging_player(dir: &Path) -> String {
        logging_player_for(dir, "0.12")
    }

    /// A speaker that "plays" for `secs` seconds and then logs one line.
    fn logging_player_for(dir: &Path, secs: &str) -> String {
        std::fs::create_dir_all(dir).unwrap();
        format!("sh -c \"cat >/dev/null; sleep {secs}; echo p >> {}/played.log\"", dir.display())
    }

    /// A speaker that plays piece N (N pieces have been played before it) once the file `go.N` exists in `dir`.
    fn gated_player(dir: &Path) -> String {
        std::fs::create_dir_all(dir).unwrap();
        let script = dir.join("gated.sh");
        std::fs::write(&script, format!("#!/bin/sh\ncat >/dev/null\nn=$(cat '{d}/played.log' 2>/dev/null | wc -l)\nwhile [ ! -f '{d}/go.'$n ]; do sleep 0.02; done\necho p >> '{d}/played.log'\n", d = dir.display())).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        script.display().to_string()
    }

    fn played(dir: &Path) -> usize {
        std::fs::read_to_string(dir.join("played.log")).map_or(0, |t| t.lines().count())
    }

    // ------------------------------------------------------------------ writing

    #[test]
    fn a_topic_with_options_is_written_by_the_model_without_tools_and_the_story_is_saved() {
        let t = Teller::new(STORY);
        let (mut a, cx, home) = make("tales-write", &t);
        assert_eq!((a.page_name(), a.focus_name()), ("story", "idea"), "ready to type");
        // pick options by clicking chips
        click(&mut a, &cx, T_AUDIENCE + 1);
        click(&mut a, &cx, T_GENRE + 3);
        click(&mut a, &cx, T_LENGTH + 1);
        click(&mut a, &cx, T_LANG + 2);
        click(&mut a, &cx, T_TONE + 3);
        click(&mut a, &cx, T_SUGGEST); // an idea from the list fills the topic
        assert_eq!(a.options().topic, SUGGESTIONS[0]);
        assert_eq!(a.focus_name(), "idea");
        // an empty topic is refused with a hint, and nothing is asked of the model
        a.idea.set("");
        key(&mut a, &cx, Key::Ctrl('n'));
        assert!(a.note_text().unwrap().contains("Type a topic"), "{:?}", a.note_text());
        assert!(t.seen.lock().unwrap().is_empty(), "nothing was asked of the model");
        write_story(&mut a, &cx, "a fox and the moon with Mala and her goat");
        assert_eq!(a.page_name(), "story");
        let seen = t.story_calls();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].1, 0, "no tools are ever offered");
        let (sys, user) = (&seen[0].0[0], &seen[0].0[1]);
        assert_eq!((sys.role, user.role), (ChatRole::System, ChatRole::User));
        assert!(sys.content.contains("SAFETY (teenagers)") && sys.content.contains("YOU HAVE NO TOOLS") && sys.content.contains("in Hindi"), "{}", sys.content);
        assert!(user.content.contains("a fox and the moon") && user.content.contains("Mala and her goat") && user.content.contains("science-fiction") == false);
        assert!(sys.content.contains("science-fiction"), "the genre chip reached the prompt");
        assert!(a.story_text().contains("Once upon a time") && a.story_text().starts_with("The Brave Little Fox"));
        // it was saved in the account's folder, with the options, and listed
        let id = a.saved_id().expect("saved");
        assert_eq!(id, "the-brave-little-fox");
        let md = std::fs::read_to_string(home.join("Stories/the-brave-little-fox/story.md")).unwrap();
        assert!(md.starts_with("---\ntitle: The Brave Little Fox") && md.contains("audience: teen") && md.contains("language: Hindi") && md.contains("# The Brave Little Fox"), "{md}");
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("story.create") && log.contains("teen scifi medium Hindi"), "{log}");
        assert!(!log.contains("a fox and the moon"), "the topic text is not logged");
        // the choices are remembered for the next visit, the topic is not
        assert_eq!(a.store.load_prefs().options.audience, Audience::Teen);
        assert_eq!(a.store.load_prefs().options.language, "Hindi");
        assert!(a.store.load_prefs().options.topic.is_empty(), "the topic itself is not kept");
    }

    #[test]
    fn the_story_appears_as_it_is_written_paragraph_by_paragraph() {
        let t = Teller::with(STORY, None, 60);
        let (mut a, cx, _) = make("tales-stream", &t);
        let _ = draw(&mut a, &cx, 1280, 720, 0); // the first draw loads the fonts: not while the clock runs
        a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx);
        typ(&mut a, &cx, "a fox");
        key(&mut a, &cx, Key::Ctrl('n'));
        assert!(a.is_writing());
        let (mut max_blocks, mut saw_partial, mut counts) = (0, false, vec![]);
        for _ in 0..600u64 {
            a.handle(&Event::Tick, &cx);
            if a.story.blocks.len() > max_blocks {
                max_blocks = a.story.blocks.len();
                counts.push(a.story.blocks.len());
            }
            saw_partial |= !a.story.partial.is_empty();
            if !a.is_writing() {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!a.is_writing());
        assert!(saw_partial, "a paragraph in progress is shown before it is finished");
        assert_eq!(counts, vec![1, 2, 3], "the paragraphs were completed one after another: {counts:?}");
        assert_eq!(a.story.title, "The Brave Little Fox");
        assert!(a.saved_id().is_some());
    }

    #[test]
    fn the_paragraph_in_progress_is_drawn_after_the_finished_ones_with_moving_dots() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-partial", &t);
        a.story = st::parse_story("# The Fox\n\nOne finished paragraph here.\n\nHalf a sente", false);
        assert_eq!((a.story.blocks.len(), a.story.partial.as_str()), (1, "Half a sente"));
        let (_tx, rx) = channel();
        a.gen = Some(Gen { rx, stop: Arc::new(AtomicBool::new(false)) });
        a.go(Page::Story, &cx);
        let (g1, area) = draw(&mut a, &cx, 1280, 720, 0);
        let (g2, _) = draw(&mut a, &cx, 1280, 720, 10);
        assert_ne!(g1.rgba(), g2.rgba(), "the dots move");
        assert_layout(&g1, area, "story page while writing");
        assert!(g1.hits.iter().any(|h| h.label == "Stop writing"), "and the person can stop it");
        a.gen = None;
    }

    #[test]
    fn a_failing_model_says_why_and_a_story_stopped_early_is_still_kept() {
        let t = Teller::with(STORY, Some("No chat model configured. Open Settings → Models and add a provider."), 0);
        let (mut a, cx, _) = make("tales-fail", &t);
        write_story(&mut a, &cx, "a fox");
        assert!(a.error_text().unwrap().contains("Settings → Models"), "{:?}", a.error_text());
        assert!(a.saved_id().is_none() && a.library_ids().is_empty());
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Send"), "the composer is there to try again");
        // stopped by the person after the first paragraph: what was written is kept
        let t2 = Teller::with(STORY, None, 15);
        let (mut b, cx2, home) = make("tales-stop", &t2);
        b.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx2);
        typ(&mut b, &cx2, "a fox");
        key(&mut b, &cx2, Key::Ctrl('n'));
        until(&mut b, &cx2, "a paragraph", |a| !a.story.blocks.is_empty());
        click(&mut b, &cx2, T_STOP_WRITING);
        until(&mut b, &cx2, "the end", |a| !a.is_writing());
        let id = b.saved_id().expect("kept");
        assert!(b.note_text().unwrap().contains("Stopped early"), "{:?}", b.note_text());
        let md = std::fs::read_to_string(home.join(format!("Stories/{id}/story.md"))).unwrap();
        assert!(md.contains("Once upon a time") && !md.contains("slept well"), "{md}");
    }

    // ------------------------------------------------------------------ reading aloud

    #[test]
    fn without_a_speech_engine_the_story_is_still_written_and_the_page_says_how_to_turn_reading_on() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-notts", &t);
        assert_eq!(*a.tts_status(), TtsStatus::NoEngine);
        let help = st::status_text(a.tts_status());
        assert!(help.contains("Settings → Voice") && help.contains("/v1/audio/speech") && help.contains("piper"), "{help}");
        write_story(&mut a, &cx, "a fox");
        assert!(a.saved_id().is_some(), "written and saved as text");
        assert!(a.story_text().contains("slept well"));
        click(&mut a, &cx, T_PLAY);
        assert_eq!(a.note_text(), Some(help.as_str()), "pressing play explains it");
        assert_eq!(a.reader_status(), Status::Idle);
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Play"), "the player is drawn all the same");
        // an account that may not use the speaker is told that instead
        let mut cx2 = cx.clone();
        cx2.session.policy.voice = false;
        a.refresh_tts(&cx2);
        assert_eq!(*a.tts_status(), TtsStatus::NotAllowed);
        assert!(st::status_text(a.tts_status()).contains("not enabled for your account"));
        // a missing speaker command
        let engine_dir = temp_dir("tales-notts-eng");
        let _e = Engine::install(&cx, &engine_dir, "nosuchplayer-xyz -q");
        a.refresh_tts(&cx);
        assert_eq!(*a.tts_status(), TtsStatus::NoPlayer("nosuchplayer-xyz".into()));
    }

    #[test]
    fn one_click_turns_on_a_local_piper_when_it_is_installed() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-quick", &t);
        // nothing to offer on this machine's settings: the page says so instead of showing a button
        a.quick.clear();
        a.quick.push(st::QuickEngine { label: "Use piper (en_US-test)".into(), engine: SpeechEngine::Command { command: "piper --model /v/en.onnx --output_file {out}".into() } });
        click(&mut a, &cx, T_QUICK);
        assert!(matches!(cx.services.voice_settings().tts, Some(SpeechEngine::Command { ref command }) if command.starts_with("piper")), "{:?}", cx.services.voice_settings().tts);
        assert!(a.note_text().unwrap().contains("piper (en_US-test)"), "{:?}", a.note_text());
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("story.engine"));
        // the button is only drawn for administrators
        let mut c2 = cx.clone();
        c2.session.policy.manage_settings = false;
        cx.services.settings.lock().unwrap().update(|s| s.voice.tts = None).unwrap();
        a.refresh_tts(&c2);
        assert!(a.quick.is_empty(), "no one-click buttons for someone who cannot change Settings");
    }

    #[test]
    fn the_story_is_spoken_in_order_while_it_is_written_and_the_page_follows_along() {
        let t = Teller::with(STORY, None, 15);
        let (mut a, cx, _) = make("tales-read", &t);
        let _ = draw(&mut a, &cx, 1280, 720, 0);
        let dir = temp_dir("tales-read-eng");
        // a speaker that holds each piece until the test has looked at it (the file go.N lets piece N end): what the page shows
        // while a piece plays is looked at while it plays, however slow or fast the machine is
        let player = gated_player(&dir.join("p"));
        let eng = Engine::install(&cx, &dir, &player);
        a.refresh_tts(&cx);
        assert!(matches!(a.tts_status(), TtsStatus::Ready { .. }));
        assert!(a.prefs.read_while_writing, "reading while writing is on by default");
        a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx);
        typ(&mut a, &cx, "a fox");
        key(&mut a, &cx, Key::Ctrl('n'));
        // reading starts before the model has finished
        until(&mut a, &cx, "reading to start", |a| a.live && matches!(a.reader_status(), Status::Playing | Status::Buffering | Status::Done));
        let early = a.is_writing();
        let mut highlighted = std::collections::BTreeSet::new();
        let mut released = std::collections::BTreeSet::new();
        // the wait is for the reading to end (a state), with a wall-clock limit that only matters when something is wrong
        let deadline = Instant::now() + Duration::from_secs(300);
        let mut tick = 0u64;
        loop {
            assert!(Instant::now() < deadline, "the reading did not end: {:?}, writing {}", a.reader_status(), a.is_writing());
            a.handle(&Event::Tick, &cx);
            let snap = a.reader.snapshot();
            if matches!(snap.status, Status::Playing) && !released.contains(&snap.chunk) {
                // this piece plays now and is held: the page lights its paragraph and offers Pause
                highlighted.insert(snap.unit);
                let (g, _) = draw(&mut a, &cx, 1280, 720, tick);
                assert_eq!(a.reader_status(), Status::Playing, "held, so still playing");
                assert!(g.hits.iter().any(|h| h.label == "Pause"), "the play button turns into pause while it plays");
                released.insert(snap.chunk);
                std::fs::write(dir.join("p").join(format!("go.{}", snap.chunk)), "").unwrap();
            }
            if a.reader_status() == Status::Done && !a.is_writing() {
                break;
            }
            tick += 1;
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(a.reader_status(), Status::Done);
        assert!(early || a.chunks.len() > 3, "(started while writing: {early})");
        // the engine was asked for the sentences in story order, each once, title first
        let said = eng.said();
        assert_eq!(said[0], "The Brave Little Fox");
        let spoken = said.join(" ");
        let (i1, i2, i3) = (spoken.find("Once upon a time").unwrap(), spoken.find("One night").unwrap(), spoken.find("She walked home").unwrap());
        assert!(i1 < i2 && i2 < i3, "in order: {said:?}");
        assert_eq!(said.len(), a.chunks.len(), "one request per piece");
        assert_eq!(played(&dir.join("p")), a.chunks.len(), "and every piece was played");
        assert!(highlighted.len() >= 2, "the highlight moved along the paragraphs: {highlighted:?}");
        assert_eq!(a.reader.snapshot().fraction, 1.0);
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("story.read"));
    }

    // ------------------------------------------------------------------ the word being said

    /// A speaker whose voice reports the word it is on (like the browser voice's boundary events); it never ends by itself.
    struct WordSink {
        at: Arc<Mutex<Option<(usize, usize)>>>,
    }
    struct WordChild {
        at: Arc<Mutex<Option<(usize, usize)>>>,
        until: Instant,
    }
    impl st::Playing for WordChild {
        fn is_playing(&mut self) -> bool {
            Instant::now() < self.until
        }
        fn stop(&mut self) {}
        fn spoken_word(&mut self) -> Option<(usize, usize)> {
            *self.at.lock().unwrap()
        }
    }
    impl st::Sink for WordSink {
        fn play(&self, _wav: &[u8]) -> Result<Box<dyn st::Playing>, String> {
            Ok(Box::new(WordChild { at: self.at.clone(), until: Instant::now() + Duration::from_secs(3600) }))
        }
    }
    struct ToneSynth;
    impl st::Synth for ToneSynth {
        fn synth(&self, _text: &str) -> Result<Vec<u8>, String> {
            // long, so that the timing model moves slowly and the voice's own word is what the page shows
            Ok(encode_wav(&tone(440.0, 60_000), 16_000))
        }
    }

    /// A written story whose chunk `chunk` is being read, with the voice on word `word` (None: no word reported yet).
    /// The time of the reader in these tests: it only moves when a test moves it (milliseconds).
    type Moved = Arc<std::sync::atomic::AtomicU32>;

    fn reading_app(tag: &str, chunk: usize, word: Option<usize>) -> (StoryApp, AppCx, Arc<Mutex<Option<(usize, usize)>>>, Moved) {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make(tag, &t);
        // the word timing runs on a clock the test holds: how long the machine takes to draw or to start processes cannot
        // move the word that is lit
        let time: Moved = Arc::new(std::sync::atomic::AtomicU32::new(1000));
        let t2 = time.clone();
        a.reader.set_time_source(Arc::new(move || t2.load(std::sync::atomic::Ordering::SeqCst) as f32));
        write_story(&mut a, &cx, "a fox");
        a.chunks = st::chunks_for(&a.story.units(), 0, st::MAX_CHUNK_CHARS);
        a.chunked_units = a.story.units().len();
        let at = Arc::new(Mutex::new(None));
        a.reader.start(a.chunks.clone(), chunk, false, "t", Arc::new(ToneSynth), Arc::new(WordSink { at: at.clone() }));
        a.live = true;
        a.manual_unit = None;
        until(&mut a, &cx, "playing", |a| a.reader_status() == Status::Playing);
        if let Some(k) = word {
            let n = iyakki_agent::wordtime::words_of(&a.chunks[chunk].text).len();
            *at.lock().unwrap() = Some((k.min(n - 1), n));
            until(&mut a, &cx, "an exact word", |a| a.reader.snapshot().word_exact);
        }
        (a, cx, at, time)
    }

    fn word_text(a: &StoryApp) -> String {
        let snap = a.reader.snapshot();
        let (u, (x, y)) = (snap.unit, snap.word.expect("a word"));
        a.story.units()[u].chars().skip(x).take(y - x).collect()
    }

    #[test]
    fn the_sentence_and_the_word_being_said_are_lit_inside_the_text_at_every_screen_size() {
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            // the chunk of the second paragraph (several sentences, wrapping on a small screen), each of its words in turn
            let (mut a, cx, at, _time) = reading_app(&format!("tales-word-{w}"), 2, Some(0));
            let n = iyakki_agent::wordtime::words_of(&a.chunks[2].text).len();
            assert!(n > 12, "{n}");
            let mut lines_seen = std::collections::BTreeSet::new();
            for k in [0, 5, 10, n - 1] {
                *at.lock().unwrap() = Some((k, n));
                let (c0, sp) = (a.chunks[2].start, iyakki_agent::wordtime::words_of(&a.chunks[2].text)[k]);
                until(&mut a, &cx, "the word", |a| a.reader.snapshot().word == Some((c0 + sp.start, c0 + sp.end)));
                let want = {
                    let c = &a.chunks[2];
                    let sp = iyakki_agent::wordtime::words_of(&c.text)[k];
                    c.text.chars().skip(sp.start).take(sp.end - sp.start).collect::<String>()
                };
                assert_eq!(word_text(&a), want, "{w}x{h} word {k}");
                // a few frames, so that the scroll follows and the easing settles
                let mut g = draw(&mut a, &cx, w, h, 0).0;
                for t in 1..10 {
                    g = draw(&mut a, &cx, w, h, t).0;
                }
                let s = crate::shell::scale_for(w, h);
                let word = a.word_rect.unwrap_or_else(|| panic!("{w}x{h}: word {k} ({want}) was not drawn"));
                let area = a.text_area;
                assert!(word.x >= area.x - 4.0 * s && word.x + word.w <= area.x + area.w + 4.0 * s, "{w}x{h} word {k}: {word:?} not inside {area:?}");
                assert!(word.y >= area.y - 2.0 && word.y + word.h <= area.y + area.h + 2.0, "{w}x{h} word {k}: {word:?} not inside {area:?} (the page follows the word)");
                assert!(!a.sentence_rects.is_empty());
                assert!(a.sentence_rects.iter().any(|r| word.x >= r.x - 0.6 && word.x + word.w <= r.x + r.w + 0.6 && (word.y - r.y).abs() < 0.6), "{w}x{h} word {k}: not inside the sentence {:?} / {word:?}", a.sentence_rects);
                // a solid gold fill (not a wash) at the left edge of the highlight, where no glyph is drawn (the right edge when the
                // word starts the line: the left padding of the fill is outside the text area, which clips it)
                let (lx, rx) = (word.x + 1.5 * s, word.x + word.w - 1.5 * s);
                let px = g.pm.pixel((if lx >= area.x + 0.5 { lx } else { rx }) as u32, (word.y + word.h / 2.0) as u32).expect("pixel");
                assert!(px.red() > 200 && px.green() > 170 && px.blue() > 80, "{w}x{h} word {k}: {px:?}");
                lines_seen.insert((word.y * 10.0) as i32);
            }
            assert!(lines_seen.len() >= 2, "{w}x{h}: the word moved over several lines {lines_seen:?}");
            a.reader.stop();
        }
    }

    #[test]
    fn the_highlight_setting_chooses_sentence_and_word_sentence_only_or_nothing() {
        use iyakki_agent::voice::Highlight;
        let (mut a, cx, _at, _time) = reading_app("tales-hl-setting", 2, Some(2));
        let set = |h: Highlight| cx.services.settings.lock().unwrap().update(|s| s.voice.highlight = h).unwrap();
        for t in 0..10 {
            let _ = draw(&mut a, &cx, 1280, 720, t);
        }
        assert!(a.word_rect.is_some() && !a.sentence_rects.is_empty(), "the default is sentence and word");
        set(Highlight::Sentence);
        let _ = draw(&mut a, &cx, 1280, 720, 2);
        assert!(a.word_rect.is_none() && !a.sentence_rects.is_empty(), "sentence only");
        set(Highlight::Off);
        let (g, _) = draw(&mut a, &cx, 1280, 720, 3);
        assert!(a.word_rect.is_none() && a.sentence_rects.is_empty(), "off");
        // and still reading, with the page following along
        assert_eq!(a.reader_status(), Status::Playing);
        assert!(g.hits.iter().any(|h| h.label == "Pause"));
        a.reader.stop();
        set(Highlight::SentenceAndWord);
        let _ = draw(&mut a, &cx, 1280, 720, 4);
        assert!(a.word_rect.is_none() && a.sentence_rects.is_empty(), "nothing is lit when nothing is read");
    }

    #[test]
    fn pausing_freezes_the_word_on_the_page_and_stopping_clears_it() {
        let (mut a, cx, _at, time) = reading_app("tales-hl-pause", 2, Some(5));
        for t in 0..25 {
            let _ = draw(&mut a, &cx, 1280, 720, t);
        }
        let word = a.reader.snapshot().word;
        assert!(a.word_rect.is_some());
        click(&mut a, &cx, T_PLAY);
        assert_eq!(a.reader_status(), Status::Paused);
        assert_eq!(a.reader.snapshot().word, word, "the word stays the one that was lit");
        // (the fill eases from the word before it, so let it arrive)
        for t in 100..115 {
            let _ = draw(&mut a, &cx, 1280, 720, t);
        }
        let lit = a.word_rect.expect("lit");
        for t in 2..8 {
            // time passes (the real clock is not what moves the word): a paused word stays where it is
            time.fetch_add(30_000, std::sync::atomic::Ordering::SeqCst);
            let _ = draw(&mut a, &cx, 1280, 720, t);
            assert_eq!(a.word_rect, Some(lit), "the paused word does not move");
        }
        click(&mut a, &cx, T_STOP);
        let _ = draw(&mut a, &cx, 1280, 720, 9);
        assert!(a.word_rect.is_none() && a.sentence_rects.is_empty(), "stop clears the highlight");
    }

    #[test]
    fn frames_are_asked_for_only_while_the_story_is_being_read() {
        let (mut a, cx, _at, _time) = reading_app("tales-frames", 2, Some(1));
        let _ = a.wants_redraw();
        let mut n = 0;
        for _ in 0..30 {
            a.handle(&Event::Tick, &cx);
            if a.wants_redraw() {
                n += 1;
            }
        }
        assert!((8..=12).contains(&n), "about every third heartbeat while reading, got {n}");
        a.reader.pause();
        for _ in 0..40 {
            // (the level bars settle for a moment)
            a.handle(&Event::Tick, &cx);
            let _ = a.wants_redraw();
        }
        let idle = (0..30).filter(|_| {
            a.handle(&Event::Tick, &cx);
            a.wants_redraw()
        });
        assert_eq!(idle.count(), 0, "paused: nothing moves, no frames");
        a.reader.stop();
        for _ in 0..40 {
            a.handle(&Event::Tick, &cx);
            let _ = a.wants_redraw();
        }
        let idle = (0..30).filter(|_| {
            a.handle(&Event::Tick, &cx);
            a.wants_redraw()
        });
        assert_eq!(idle.count(), 0, "stopped: no frames");
    }

    /// The screen point of character `pos` of unit `unit` in the last drawing.
    fn point_of(a: &StoryApp, g: &mut Gfx, s: f32, unit: usize, pos: usize) -> (f32, f32) {
        let l = (0..a.line_map.len()).rev().find(|i| a.line_map[*i].0 == unit && a.line_map[*i].1 <= pos).expect("line");
        let start = a.line_map[l].1;
        let text: String = a.lay[unit].lines.iter().find(|(_, st0)| *st0 == start).map(|(t, _)| t.clone()).unwrap();
        let prefix: String = text.chars().take(pos - start).collect();
        let x = a.text_area.x + g.text_width(16.0 * s, &prefix, Face::Sans) + 3.0 * s;
        (x, a.line_tops[l] + g.line_height(16.0 * s, Face::Sans) / 2.0)
    }

    #[test]
    fn a_click_on_a_sentence_reads_from_that_sentence_but_a_double_click_or_a_drag_does_not() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-click-read", &t);
        let dir = temp_dir("tales-click-eng");
        let eng = Engine::install(&cx, &dir, &logging_player_for(&dir.join("p"), "0.2"));
        a.refresh_tts(&cx);
        a.prefs.read_while_writing = false;
        write_story(&mut a, &cx, "a fox");
        // the end of the story is in view (the first paragraph has a picture slot above it)
        a.tscroll = 1.0e6;
        let _ = draw(&mut a, &cx, 1280, 720, 0);
        let (mut g, _) = draw(&mut a, &cx, 1280, 720, 1);
        let s = crate::shell::scale_for(1280, 720);
        // the third sentence of the second paragraph
        let unit = a.story.units().iter().position(|u| u.contains("The fox listened carefully.")).unwrap();
        let pos = a.story.units()[unit].chars().count() - "The fox listened carefully.".chars().count();
        let before = a.chunks.len();
        assert_eq!(a.reader_status(), Status::Idle);
        let (x, y) = point_of(&a, &mut g, s, unit, pos + 2);
        let press = |a: &mut StoryApp, cx: &AppCx, x: f32, y: f32, clicks: u8, drag: bool| {
            crate::ui::set_click_count(clicks);
            crate::ui::set_dragging(drag);
            a.handle(&Event::AreaAt(TEXT_PANE, x - a.text_area.x, y - a.text_area.y), cx);
            crate::ui::set_dragging(false);
            crate::ui::set_click_count(1);
        };
        let release = |a: &mut StoryApp, cx: &AppCx| {
            a.handle(&Event::MouseUp(0, 0), cx);
        };
        // a double click (the word is selected) never starts reading
        press(&mut a, &cx, x, y, 2, false);
        release(&mut a, &cx);
        for _ in 0..40 {
            a.handle(&Event::Tick, &cx);
        }
        assert_eq!(a.reader_status(), Status::Idle);
        // neither does a drag
        press(&mut a, &cx, x, y, 1, false);
        press(&mut a, &cx, x + 30.0, y, 1, true);
        release(&mut a, &cx);
        for _ in 0..40 {
            a.handle(&Event::Tick, &cx);
        }
        assert_eq!(a.reader_status(), Status::Idle);
        a.pane.clear();
        // a plain click waits out the double-click time, then reads from the start of the sentence it is in
        press(&mut a, &cx, x, y, 1, false);
        release(&mut a, &cx);
        for _ in 0..5 {
            a.handle(&Event::Tick, &cx);
        }
        assert_eq!(a.reader_status(), Status::Idle, "still waiting for a possible second click");
        for _ in 0..20 {
            a.handle(&Event::Tick, &cx);
        }
        until(&mut a, &cx, "reading from the click", |a| matches!(a.reader_status(), Status::Playing | Status::Buffering | Status::Done));
        let snap = a.reader.snapshot();
        assert_eq!(snap.unit, unit);
        assert_eq!(snap.range.0, pos, "the piece starts at the clicked sentence");
        assert!(a.chunks.len() > before.max(1) - 1);
        until(&mut a, &cx, "the engine to be asked", |_| !eng.said().is_empty());
        assert_eq!(eng.said()[0], "The fox listened carefully.", "the first thing said is the clicked sentence: {:?}", eng.said());
        a.reader.stop();
    }

    #[test]
    fn play_pause_stop_and_replay_end_the_speaker_process() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-ctl", &t);
        let dir = temp_dir("tales-ctl-eng");
        let pid = dir.join("pid");
        let _eng = Engine::install(&cx, &dir, &format!("sh -c \"echo $$ > {}; cat >/dev/null; sleep 30\"", pid.display()));
        a.refresh_tts(&cx);
        a.prefs.read_while_writing = false;
        write_story(&mut a, &cx, "a fox");
        assert_eq!(a.reader_status(), Status::Idle, "nothing is read until asked");
        let alive = |p: &Path| -> Option<u32> {
            let n: u32 = std::fs::read_to_string(p).ok()?.trim().parse().ok()?;
            Path::new(&format!("/proc/{n}")).exists().then_some(n)
        };
        let gone = |a: &mut StoryApp, cx: &AppCx, n: u32| until(a, cx, "the speaker process to end", |_| !Path::new(&format!("/proc/{n}")).exists());
        click(&mut a, &cx, T_PLAY);
        until(&mut a, &cx, "the first piece to play", |a| alive(&pid).is_some() && a.reader_status() == Status::Playing);
        let first = alive(&pid).unwrap();
        // pause ends the process; resume starts a new one for the same piece
        click(&mut a, &cx, T_PLAY);
        gone(&mut a, &cx, first);
        assert_eq!(a.reader_status(), Status::Paused);
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Play"));
        click(&mut a, &cx, T_PLAY);
        until(&mut a, &cx, "a new process", |a| alive(&pid).is_some_and(|n| n != first) && a.reader_status() == Status::Playing);
        let second = alive(&pid).unwrap();
        // stop kills it and the position is forgotten
        click(&mut a, &cx, T_STOP);
        gone(&mut a, &cx, second);
        assert_eq!(a.reader_status(), Status::Idle);
        // next part, then replay from the start, then leaving the tile ends the sound too
        click(&mut a, &cx, T_PLAY);
        until(&mut a, &cx, "playing", |_| alive(&pid).is_some_and(|n| n != second));
        let third = alive(&pid).unwrap();
        click(&mut a, &cx, T_NEXT);
        gone(&mut a, &cx, third);
        until(&mut a, &cx, "the next part", |a| a.reader.snapshot().unit > 0 && alive(&pid).is_some_and(|n| n != third));
        assert!(a.reader.snapshot().unit >= 1);
        click(&mut a, &cx, T_REPLAY);
        until(&mut a, &cx, "replay from the start", |a| a.reader.snapshot().chunk == 0 && a.reader_status() == Status::Playing);
        let last = alive(&pid);
        drop(a);
        if let Some(n) = last {
            for _ in 0..200 {
                if !Path::new(&format!("/proc/{n}")).exists() {
                    return;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            panic!("the speaker process {n} survived leaving the tile");
        }
    }

    #[test]
    fn a_speech_engine_that_fails_is_reported_on_the_page() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-err", &t);
        let dir = temp_dir("tales-err-eng");
        let _eng = Engine::install(&cx, &dir, &logging_player(&dir.join("p")));
        std::fs::write(dir.join("fail"), "").unwrap();
        a.refresh_tts(&cx);
        a.prefs.read_while_writing = false;
        write_story(&mut a, &cx, "a fox");
        click(&mut a, &cx, T_PLAY);
        until(&mut a, &cx, "the failure", |a| matches!(a.reader_status(), Status::Failed(_)));
        let e = a.error_text().unwrap().to_string();
        assert!(e.contains("Could not make the speech for part 1") && e.contains("engine exploded"), "{e}");
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Play"), "retry is one click away");
        assert_eq!(played(&dir.join("p")), 0);
        // fixing the engine and pressing replay works
        std::fs::remove_file(dir.join("fail")).unwrap();
        click(&mut a, &cx, T_REPLAY);
        until(&mut a, &cx, "done", |a| a.reader_status() == Status::Done);
        assert!(played(&dir.join("p")) > 0);
    }

    #[test]
    fn voice_and_speed_are_chosen_from_the_engine_and_remembered() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-voice", &t);
        cx.services
            .settings
            .lock()
            .unwrap()
            .update(|s| {
                s.voice.tts = Some(SpeechEngine::Http { url: "https://api.openai.com/v1/audio/speech".into(), model: "tts-1".into(), key_ref: None, voice: Some("nova".into()), language: None });
                s.voice.play_cmd = "sh -c true".into();
            })
            .unwrap();
        a.refresh_tts(&cx);
        assert_eq!(a.voices[0], "nova");
        assert!(a.voices.len() >= 6 && a.takes_speed);
        click(&mut a, &cx, T_VOICE);
        assert_eq!(a.prefs.voice.as_deref(), Some("alloy"));
        click(&mut a, &cx, T_SPEED);
        assert!((a.prefs.speed - 1.1).abs() < 0.001);
        assert_eq!(a.store.load_prefs().voice.as_deref(), Some("alloy"), "remembered for the next visit");
        assert!((a.store.load_prefs().speed - 1.1).abs() < 0.001);
        write_story_nofocus(&mut a, &cx);
        assert!(a.saved_id().is_some());
        // a command engine without {voice} or {speed} has neither chip
        cx.services.settings.lock().unwrap().update(|s| s.voice.tts = Some(SpeechEngine::Command { command: "piper --output_file {out}".into() })).unwrap();
        a.refresh_tts(&cx);
        assert!(a.voices.is_empty() && !a.takes_speed);
    }

    fn write_story_nofocus(a: &mut StoryApp, cx: &AppCx) {
        a.open_form(cx);
        a.idea.set("a fox");
        a.sync_fields();
        a.start_create(cx);
        until(a, cx, "the story", |a| !a.is_writing());
    }

    // ------------------------------------------------------------------ audio files and the library

    #[test]
    fn audio_is_saved_next_to_the_story_and_heard_again_later_without_a_speech_engine() {
        let t = Teller::new(STORY);
        let (mut a, cx, home) = make("tales-audio", &t);
        let dir = temp_dir("tales-audio-eng");
        let eng = Engine::install(&cx, &dir, &logging_player(&dir.join("p")));
        a.refresh_tts(&cx);
        a.prefs.read_while_writing = false;
        write_story(&mut a, &cx, "a fox");
        let id = a.saved_id().unwrap();
        click(&mut a, &cx, T_SAVE_AUDIO);
        assert!(a.audio_job.is_some());
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label.starts_with("Cancel audio")), "progress and a cancel button while it is made");
        until(&mut a, &cx, "the audio file", |a| a.audio_job.is_none());
        assert!(a.note_text().unwrap().starts_with("Audio saved: "), "{:?}", a.note_text());
        let wav = home.join(format!("Stories/{id}/audio.wav"));
        assert!(wav.is_file() && std::fs::read(&wav).unwrap().starts_with(b"RIFF"));
        assert!(home.join(format!("Stories/{id}/audio.json")).is_file());
        assert_eq!(eng.said().len(), a.chunks.len(), "every piece was made once");
        // share the text and the audio through the host-files save dialog
        click(&mut a, &cx, T_EXPORT);
        key(&mut a, &cx, Key::Down); // the second line of the menu: the text
        key(&mut a, &cx, Key::Enter);
        let (name, bytes) = a.take_host_save().unwrap();
        assert_eq!(name, format!("{id}.md"));
        assert!(String::from_utf8(bytes).unwrap().contains("# The Brave Little Fox"));
        click(&mut a, &cx, T_EXPORT);
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Enter);
        let (name, bytes) = a.take_host_save().unwrap();
        assert_eq!((name.as_str(), &bytes[..4]), ((format!("{id}.wav")).as_str(), &b"RIFF"[..]));
        assert!(a.take_host_save().is_none(), "taken once");
        // later: no engine at all, the saved sound is played piece by piece with the page following
        cx.services.settings.lock().unwrap().update(|s| s.voice.tts = None).unwrap();
        let mut b = StoryApp::new(&cx);
        b.store = StoryStore::open_at(&cx.session.policy, "tester", cx.services.dir(), home.clone());
        b.go(Page::Library, &cx);
        assert_eq!(*b.tts_status(), TtsStatus::NoEngine);
        assert_eq!(b.library_ids(), vec![id.clone()]);
        let before = eng.said().len();
        click(&mut b, &cx, T_LIB_LISTEN);
        assert_eq!(b.page_name(), "story");
        assert!(b.saved_audio.is_some() && b.can_read());
        until(&mut b, &cx, "the saved audio to play out", |b| b.reader_status() == Status::Done);
        assert_eq!(played(&dir.join("p")), b.chunks.len(), "every piece of the saved audio was played, once");
        assert_eq!(eng.said().len(), before, "the engine was not asked again");
        let (g, _) = draw(&mut b, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Play"));
    }

    #[test]
    fn the_library_lists_searches_opens_favourites_and_deletes_with_a_question_first() {
        let t = Teller::new(STORY);
        let (mut a, cx, home) = make("tales-lib", &t);
        write_story(&mut a, &cx, "a fox");
        let first = a.saved_id().unwrap();
        a.open_form(&cx);
        a.idea.set("owls");
        a.sync_fields();
        a.start_create(&cx);
        until(&mut a, &cx, "the second story", |a| !a.is_writing());
        let second = a.saved_id().unwrap();
        assert_ne!(first, second, "the same title does not overwrite");
        key(&mut a, &cx, Key::Char('l'));
        assert_eq!(a.page_name(), "library");
        assert_eq!(a.library_ids().len(), 2);
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Listen") && g.hits.iter().any(|h| h.label == "Delete"), "the chosen row has its actions");
        // search
        key(&mut a, &cx, Key::Char('/'));
        assert_eq!(a.focus_name(), "search");
        typ(&mut a, &cx, "zebra");
        assert!(a.library_ids().is_empty());
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().all(|h| h.label != "Listen"));
        for _ in 0..5 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "owl");
        assert_eq!(a.library_ids().len(), 2, "the word is in the story text");
        key(&mut a, &cx, Key::Esc);
        assert_eq!(a.focus_name(), "none");
        // favourite
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Char('f'));
        let chosen = a.library_ids()[1].clone();
        assert!(a.store.load(&chosen).unwrap().entry.favourite);
        // delete asks first; anything but y keeps it
        key(&mut a, &cx, Key::Char('d'));
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Cancel"), "a confirmation dialog is up");
        key(&mut a, &cx, Key::Char('n'));
        assert_eq!(a.library_ids().len(), 2, "n did not delete");
        key(&mut a, &cx, Key::Char('d'));
        key(&mut a, &cx, Key::Char('y'));
        assert_eq!(a.library_ids().len(), 1);
        assert!(!home.join(format!("Stories/{chosen}")).exists());
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("story.delete"));
        // open: the saved text is on the page and the list survives a new visit
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.page_name(), "story");
        assert!(a.story_text().contains("Once upon a time"));
        let b = {
            let mut b = StoryApp::new(&cx);
            b.store = StoryStore::open_at(&cx.session.policy, "tester", cx.services.dir(), home.clone());
            b.go(Page::Library, &cx);
            b
        };
        assert_eq!(b.library_ids().len(), 1);
    }

    #[test]
    fn a_story_cannot_be_saved_outside_what_the_account_may_write_and_is_still_shown() {
        let t = Teller::new(STORY);
        let (mut a, mut cx, home) = make("tales-scope", &t);
        cx.session.policy.fs_write.clear();
        a.store = StoryStore::open_at(&cx.session.policy, "tester", cx.services.dir(), home.clone());
        write_story(&mut a, &cx, "a fox");
        assert!(a.story_text().contains("slept well"), "the story is on the page");
        assert!(a.saved_id().is_none());
        assert!(a.note_text().unwrap().contains("could not be saved") && a.note_text().unwrap().contains("may not save"), "{:?}", a.note_text());
        assert!(!home.join("Stories").exists());
        click(&mut a, &cx, T_SAVE_AUDIO);
        assert!(a.audio_job.is_none());
    }

    // ------------------------------------------------------------------ selection, focus, drawing

    #[test]
    fn the_story_text_can_be_selected_and_copied() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-copy", &t);
        write_story(&mut a, &cx, "a fox");
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        let pane = g.hits.iter().find(|h| h.label == "text area").expect("the text is selectable").rect;
        crate::ui::set_click_count(3);
        crate::ui::set_dragging(false);
        a.handle(&Event::AreaAt(TEXT_PANE, 20.0, 6.0), &cx); // a triple click on the first line
        crate::ui::set_click_count(1);
        let _ = draw(&mut a, &cx, 1280, 720, 1);
        key(&mut a, &cx, Key::Ctrl('c'));
        let copied = crate::ui::clipboard::latest().unwrap_or_default();
        assert!(copied.starts_with("Once upon a time a small fox"), "{copied:?}");
        assert!(!copied.contains("One night"), "just the paragraph: {copied:?}");
        // select all and copy the lot (the title is the hero, the paragraphs are the text)
        key(&mut a, &cx, Key::Ctrl('a'));
        let _ = draw(&mut a, &cx, 1280, 720, 2);
        key(&mut a, &cx, Key::Ctrl('c'));
        let all = crate::ui::clipboard::latest().unwrap_or_default();
        assert!(all.contains("Once upon a time") && all.contains("slept well"), "{all:?}");
        // the right-click menu offers Copy over the text
        let items = a.context_items((pane.x + 10.0) as i32, (pane.y + 10.0) as i32);
        assert!(items.iter().any(|i| i.label == "Copy"));
        // a press in the text takes the keyboard away from a field
        a.focus = Focus::Search;
        a.handle(&Event::AreaAt(TEXT_PANE, 5.0, 5.0), &cx);
        assert_eq!(a.focus_name(), "none");
    }

    #[test]
    fn exactly_one_thing_owns_the_keyboard_and_shows_a_caret() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-focus", &t);
        let carets = |a: &mut StoryApp| {
            let mut best = 0;
            for tick in [0u64, 16] {
                crate::ui::take_carets();
                crate::ui::take_focus_rings();
                let _ = draw(a, &cx, 1280, 720, tick);
                let (c, r) = (crate::ui::take_carets(), crate::ui::take_focus_rings());
                assert!(c <= 1 && r <= 1, "{c} carets and {r} focus frames on the {} page", a.page_name());
                best = best.max(c);
            }
            best
        };
        assert_eq!(a.focus_name(), "idea");
        assert_eq!(carets(&mut a), 1, "the topic field");
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.focus_name(), "none", "with Options closed there is only the idea box");
        assert_eq!(carets(&mut a), 0, "no field, no caret");
        key(&mut a, &cx, Key::BackTab);
        assert_eq!(a.focus_name(), "idea");
        // with Options open the keyboard still has the idea box and "no field" only
        click(&mut a, &cx, T_OPTIONS_MENU);
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.focus_name(), "none");
        key(&mut a, &cx, Key::BackTab);
        assert_eq!(a.focus_name(), "idea");
        key(&mut a, &cx, Key::Esc);
        assert_eq!((a.menu, a.focus_name()), (None, "idea"), "Esc closes Options and the keyboard goes back to the idea box");
        key(&mut a, &cx, Key::Esc);
        assert_eq!(a.focus_name(), "none", "the next Esc leaves the field");
        assert!(matches!(key(&mut a, &cx, Key::Esc), Flow::Back), "the second leaves the tile");
        // typing goes to the owner only: a letter with no field is a shortcut, not text
        a.go(Page::Story, &cx);
        assert_eq!(a.focus_name(), "none");
        assert_eq!(carets(&mut a), 0);
        a.go(Page::Library, &cx);
        a.handle(&Event::FocusAt(SEARCH_FIELD, 4.0), &cx);
        assert_eq!(carets(&mut a), 1, "the search field");
        a.open_form(&cx);
        a.idea.set("");
        typ(&mut a, &cx, "hi");
        assert_eq!(a.idea.text, "hi");
        key(&mut a, &cx, Key::Esc);
        typ(&mut a, &cx, "xyz");
        assert_eq!(a.idea.text, "hi", "with no field focused, letters are not typed into one");
        // a click on a chip takes the keyboard from the field; an idea chip puts the caret back in the topic
        a.set_focus(Focus::Idea);
        click(&mut a, &cx, T_GENRE);
        assert_eq!(a.focus_name(), "none");
        click(&mut a, &cx, T_SUGGEST + 1);
        assert_eq!(a.focus_name(), "idea");
        // a pasted text goes into the focused field only
        a.handle(&Event::Paste(" pasted".into()), &cx);
        assert!(a.idea.text.ends_with(" pasted") || a.idea.text.contains("pasted"));
    }

    /// No two click regions overlap (except a row and the buttons on it), and all are inside the window.
    fn assert_layout(g: &Gfx, area: R, what: &str) {
        let (w, h) = (g.width() as f32, g.height() as f32);
        for hit in &g.hits {
            let r = hit.rect;
            assert!(r.x >= -0.5 && r.y >= area.y - 0.5 && r.x + r.w <= w + 0.5 && r.y + r.h <= h + 0.5, "{what}: {:?} is outside the window", (&hit.label, r.x, r.y, r.w, r.h));
            // a button cut by the edge of a scrolled area (a picture's button, a block of the conversation) may show only a sliver
            let cut = matches!(hit.label.as_str(), "Draw again" | "Try again" | "toggle");
            assert!(cut || (r.w >= 8.0 && r.h >= 8.0), "{what}: {:?} is too small to hit", (&hit.label, r.w, r.h));
        }
        for (i, a) in g.hits.iter().enumerate() {
            for b in &g.hits[i + 1..] {
                if matches!(a.ev, Event::Row(_)) || matches!(b.ev, Event::Row(_)) {
                    continue;
                }
                // buttons sit on the pictures, which sit in the text area
                if a.label == "text area" || b.label == "text area" {
                    continue;
                }
                // the clipboard button belongs to the field it sits in
                if (a.label == "field" && b.label == "clipboard") || (a.label == "clipboard" && b.label == "field") {
                    continue;
                }
                let (ra, rb) = (a.rect, b.rect);
                let ox = (ra.x + ra.w).min(rb.x + rb.w) - ra.x.max(rb.x);
                let oy = (ra.y + ra.h).min(rb.y + rb.h) - ra.y.max(rb.y);
                assert!(ox <= 1.5 || oy <= 1.5, "{what}: {:?} and {:?} overlap ({ox:.0} x {oy:.0})", (&a.label, ra.x, ra.y, ra.w, ra.h), (&b.label, rb.x, rb.y, rb.w, rb.h));
            }
        }
    }

    fn colours(g: &Gfx) -> usize {
        let mut seen = std::collections::HashSet::new();
        for px in g.rgba().chunks_exact(4).step_by(37) {
            seen.insert([px[0], px[1], px[2]]);
        }
        seen.len()
    }

    #[test]
    fn every_page_fits_at_the_screen_sizes_with_no_overlap() {
        let long = STORY.replace("\n\nShe walked", &"\n\nAnother long paragraph about the river, the reeds and the night wind that went on for quite a while without stopping.".repeat(8));
        let t = Teller::new(&long);
        let (mut a, cx, _) = make("tales-geo", &t);
        let dir = temp_dir("tales-geo-eng");
        let _eng = Engine::install(&cx, &dir, "sh -c \"cat >/dev/null; sleep 30\"");
        a.refresh_tts(&cx);
        a.prefs.read_while_writing = false;
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the pictures", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
        assert!(a.pics.len() >= 3, "the long story has pictures: {}", a.pics.len());
        // a second and a third story for the list
        for topic in ["owls", "a robot"] {
            a.open_form(&cx);
            a.idea.set(topic);
            a.sync_fields();
            a.start_create(&cx);
            until(&mut a, &cx, "a story", |a| !a.is_writing());
            until(&mut a, &cx, "its pictures", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
        }
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            for text_scale in [1.0f32, 1.35] {
                let s = crate::shell::scale_for(w, h) * text_scale;
                let wide = w as f32 >= WIDE_PX;
                // the story with its composer below it; narrow, the composer is a toggle
                for pane_open in if wide { vec![true] } else { vec![false, true] } {
                    a.go(Page::Story, &cx);
                    a.pane_open = pane_open;
                    click(&mut a, &cx, T_PLAY);
                    until(&mut a, &cx, "playing", |a| a.reader_status() == Status::Playing);
                    for tick in [0u64, 7] {
                        let (g, area) = draw_at(&mut a, &cx, w, h, s, tick);
                        let what = format!("story page at {w}x{h} (text x{text_scale}, composer open {pane_open})");
                        assert_layout(&g, area, &what);
                        assert!(colours(&g) > 20, "{what} is not blank");
                        for needed in ["Pause", "Stop", "Previous part", "Next part", "progress", "text area"] {
                            assert!(g.hits.iter().any(|h| h.label == needed), "{what}: the {needed} control is there");
                        }
                        // the text area stays above the player
                        let text = g.hits.iter().find(|h| h.label == "text area").unwrap().rect;
                        let bar = g.hits.iter().find(|h| h.label == "progress").unwrap().rect;
                        assert!(text.y + text.h <= bar.y, "{what}: text ends above the player");
                        assert!(g.hits.iter().any(|h| h.label == "Save story" || h.label == "Saved ✓"), "{what}: the toolbar has the save button");
                    }
                    let (g, area) = draw_at(&mut a, &cx, w, h, s, 3);
                    if pane_open {
                        for needed in ["idea", "Send", "Kids", "Teens", "Adults", "Kind of story", "Tone", "Options ▾"] {
                            assert!(g.hits.iter().any(|h| h.label == *needed), "the {needed} control of the composer is there at {w}x{h} x{text_scale}");
                        }
                        assert!(g.hits.iter().any(|h| h.label.starts_with("Model: ") && h.label.ends_with(" ▾")), "the Model chip at {w}x{h}");
                        assert!(!g.hits.iter().any(|h| h.label.starts_with("Agent: ")), "no separate Agent chip at {w}x{h}");
                        // one group of buttons only: no length, language, tone or picture chips on the panel
                        for gone in ["Medium · 5 min", "Hindi", "Mysterious", "Drawings (SVG)", "Read while it is written", "Save automatically"] {
                            assert!(!g.hits.iter().any(|h| h.label == gone), "{gone} is behind Options at {w}x{h}");
                        }
                        // the composer is under the story (or beside it when wide), in view, and Send is inside the window
                        let idea = g.hits.iter().find(|h| h.label == "idea").unwrap().rect;
                        let send = g.hits.iter().find(|h| h.label == "Send").unwrap().rect;
                        let text = g.hits.iter().find(|h| h.label == "text area").unwrap().rect;
                        assert!((idea.x >= text.x + text.w || idea.y >= text.y + text.h) && idea.y >= area.y && send.y + send.h <= h as f32, "{w}x{h}: idea {idea:?} send {send:?} text {text:?} area {area:?} open {pane_open}");
                        assert!(idea.x + idea.w <= send.x, "Send is beside the box at {w}x{h}");
                        // fonts: the chips use the same sizes as the chat's controls
                        assert!(idea.h >= 38.0 * s * 0.99, "the box is a readable height at {w}x{h}");
                    }
                    click(&mut a, &cx, T_STOP);
                }
                a.go(Page::Library, &cx);
                for tick in [0u64, 7] {
                    let (g, area) = draw_at(&mut a, &cx, w, h, s, tick);
                    assert_layout(&g, area, &format!("library at {w}x{h} (text x{text_scale})"));
                }
            }
        }
        // the toggle on a small screen: Hide the idea box / Idea & options
        a.go(Page::Story, &cx);
        a.pane_open = true;
        let (g, _) = draw(&mut a, &cx, 800, 480, 0);
        assert!(g.hits.iter().any(|h| h.label == "Hide the idea box"), "the composer can be hidden");
        a.pane_open = false;
        let (g, _) = draw(&mut a, &cx, 800, 480, 0);
        assert!(g.hits.iter().any(|h| h.label == "Idea & options"), "and shown again");
        assert!(!g.hits.iter().any(|h| h.label == "Send"));
        click(&mut a, &cx, T_OPTIONS);
        assert!(a.pane_open && a.focus_name() == "idea");
        a.stop_reading();
    }

    #[test]
    fn the_options_popover_holds_the_rest_scrolls_and_fits_at_every_size() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-opts", &t);
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            for text_scale in [1.0f32, 1.35] {
                let s = crate::shell::scale_for(w, h) * text_scale;
                a.go(Page::Story, &cx);
                a.pane_open = true;
                a.menu = None;
                let (g, _) = draw_at(&mut a, &cx, w, h, s, 0);
                click_hit(&mut a, &cx, &g, "Options ▾");
                assert_eq!(a.menu, Some(Menu::Options));
                a.cscroll = 0.0;
                let mut seen: std::collections::HashSet<String> = Default::default();
                for step in 0..30 {
                    a.cscroll = step as f32 * 60.0 * s;
                    let (g, area) = draw_at(&mut a, &cx, w, h, s, 1);
                    let panel = g.hits.iter().find(|x| x.label == "options").expect("the popover").rect;
                    assert!(panel.x >= area.x && panel.y >= area.y && panel.x + panel.w <= w as f32 && panel.y + panel.h <= h as f32, "popover inside the window at {w}x{h} x{text_scale}: {panel:?}");
                    assert!(g.hits.iter().any(|x| x.label == "Close"), "Close is in view");
                    for x in &g.hits {
                        seen.insert(x.label.clone());
                    }
                    if a.cscroll >= a.cmax {
                        break;
                    }
                }
                for needed in ["Medium · 5 min", "Language", "Pictures off", "Read while it is written", "Save automatically"] {
                    assert!(seen.contains(needed), "{needed} is reachable in Options at {w}x{h} x{text_scale}");
                }
                // what the owner took away is not there: no language chips, no characters box, no picture chips or style chips
                for gone in ["field", "Hindi", "Auto", "Drawings (SVG)", "Image model", "Storybook watercolor", "Comic", "Pixel art", "Night sky"] {
                    assert!(!seen.contains(gone), "{gone} is not a control of Options at {w}x{h}");
                }
                assert!(!seen.contains("Picture model: automatic"), "no picture model chip while no image model is set up");
                // Esc closes it
                key(&mut a, &cx, Key::Esc);
                assert_eq!(a.menu, None);
                assert_eq!(a.focus_name(), "idea");
            }
        }
        // the one language control: a drop-down in Options whose list opens over it and returns to Options
        a.go(Page::Story, &cx);
        a.menu = None;
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        click_hit(&mut a, &cx, &g, "Options ▾");
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        click_hit(&mut a, &cx, &g, "Language");
        assert_eq!(a.menu, Some(Menu::Language));
        let (g, area) = draw(&mut a, &cx, 1280, 720, 1);
        for l in ["English", "Tamil", "Hindi", "Japanese"] {
            let r = g.hits.iter().rev().find(|x| x.label == l).unwrap_or_else(|| panic!("{l} in the list")).rect;
            assert!(r.y >= area.y - 1.0 && r.y + r.h <= 721.0, "{l} inside the window: {r:?}");
        }
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.options().language, "Tamil");
        assert_eq!(a.menu, Some(Menu::Options), "the choice closes the list and Options is there again");
        key(&mut a, &cx, Key::Esc);
        assert_eq!(a.menu, None);
        assert!(a.explicit[H_LANG]);
        assert_eq!(a.store.load_prefs().options.language, "Tamil", "remembered");
        a.opts.language = "English".into();
        a.explicit[H_LANG] = false;
        // a click outside closes it, a choice inside keeps it open and is remembered and explicit
        a.go(Page::Story, &cx);
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        click_hit(&mut a, &cx, &g, "Options ▾");
        click(&mut a, &cx, T_LENGTH + 2);
        assert_eq!(a.menu, Some(Menu::Options));
        assert_eq!(a.options().length, Length::Long);
        assert!(a.explicit[H_LEN]);
        click(&mut a, &cx, T_MENU_CLOSE);
        assert_eq!(a.menu, None);
        key(&mut a, &cx, Key::Ctrl('o'));
        assert_eq!(a.menu, Some(Menu::Options), "Ctrl+O opens it");
        key(&mut a, &cx, Key::Ctrl('o'));
        assert_eq!(a.menu, None);
    }

    #[test]
    fn the_cover_is_made_by_the_shell_from_the_title() {
        let mut g = Gfx::new(300, 300);
        g.clear(rgb(9, 11, 20));
        cover(&mut g, R::new(20.0, 20.0, 200.0, 200.0), 1.0, "The Brave Little Fox");
        let a = g.rgba().to_vec();
        let px = |g: &Gfx, x: usize, y: usize| {
            let o = (y * 300 + x) * 4;
            [g.rgba()[o], g.rgba()[o + 1], g.rgba()[o + 2]]
        };
        assert_ne!(px(&g, 40, 40), px(&g, 200, 200), "a gradient, not a flat fill");
        assert_eq!(px(&g, 21, 21), [8, 11, 26], "the corners are rounded off");
        let mut g2 = Gfx::new(300, 300);
        g2.clear(rgb(9, 11, 20));
        cover(&mut g2, R::new(20.0, 20.0, 200.0, 200.0), 1.0, "A Totally Different Tale");
        assert_ne!(a, g2.rgba().to_vec(), "another title, another cover");
        // tiny covers (library rows) show an initial instead of an unreadable title
        let mut g3 = Gfx::new(100, 100);
        cover(&mut g3, R::new(10.0, 10.0, 40.0, 40.0), 0.6, "Zebra");
        assert!(colours(&g3) > 5);
    }

    #[test]
    fn the_tile_is_in_the_media_group_for_roles_that_have_apps_and_the_model_has_no_tools() {
        use iyakki_sec::policy::{App as PApp, Role};
        assert!(Role::Admin.template().apps.contains(&PApp::Tales) && Role::Operator.template().apps.contains(&PApp::Tales) && Role::User.template().apps.contains(&PApp::Tales));
        assert!(!Role::Guest.template().apps.contains(&PApp::Tales));
        assert_eq!(crate::launcher::Group::of(PApp::Tales), Some(crate::launcher::Group::Home));
        assert_eq!(PApp::Tales.label(), "TALES");
        let t = Teller::new(STORY);
        let (a, _, _) = make("tales-meta", &t);
        assert_eq!((a.title(), a.icon(), a.accent()), ("Tales", Icon::Tales, ACCENT));
        assert!(a.help().iter().any(|l| l.contains("Space")) && a.hint().contains("Enter send") && a.hint().contains("Ctrl+P model or agent"));
    }

    // ------------------------------------------------------------------ the form, pictures and saving

    fn press(a: &mut StoryApp, cx: &AppCx, x: f32, y: f32) {
        a.handle(&Event::AreaAt(IDEA_AREA, x, y), cx);
    }

    #[test]
    fn the_idea_box_has_several_lines_grows_selects_and_places_the_caret() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-idea", &t);
        assert_eq!(a.focus_name(), "idea", "ready to type");
        typ(&mut a, &cx, "first line");
        key(&mut a, &cx, Key::Ctrl('j'));
        typ(&mut a, &cx, "second line");
        assert_eq!(a.idea.text, "first line\nsecond line", "Ctrl+J makes a new line (Enter sends)");
        assert_eq!(a.options().topic, a.idea.text, "the options follow the box");
        let (_g, _) = draw(&mut a, &cx, 1280, 720, 0);
        key(&mut a, &cx, Key::Up);
        typ(&mut a, &cx, "X");
        assert_eq!(a.idea.text, "first lineX\nsecond line", "Up keeps the column, clipped to the shorter line");
        key(&mut a, &cx, Key::Home);
        typ(&mut a, &cx, ">");
        key(&mut a, &cx, Key::End);
        key(&mut a, &cx, Key::Backspace);
        assert_eq!(a.idea.text, ">first line\nsecond line");
        // select all and replace
        key(&mut a, &cx, Key::Ctrl('a'));
        assert_eq!(a.idea.selected_text().as_deref(), Some(">first line\nsecond line"));
        key(&mut a, &cx, Key::Ctrl('c'));
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some(">first line\nsecond line"));
        typ(&mut a, &cx, "new");
        assert_eq!(a.idea.text, "new");
        // paste keeps line breaks; the length is capped
        a.handle(&Event::Paste("\nand\r\nmore".into()), &cx);
        assert_eq!(a.idea.text, "new\nand\nmore");
        a.idea.set(&"x".repeat(st::MAX_TOPIC + 500));
        assert_eq!(a.idea.len(), st::MAX_TOPIC);
        assert!(!a.idea.key(&Key::Char('y')), "no room for more");
        // three lines at least, growing with the text (up to ten lines in the tall panel)
        a.idea.set("short");
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        let small = g.hits.iter().find(|h| h.label == "idea").unwrap().rect.h;
        a.idea.set(&"a longer idea that wraps over a few lines of the box ".repeat(6));
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        let big = g.hits.iter().find(|h| h.label == "idea").unwrap().rect.h;
        assert!(big > small, "the box grew: {small} to {big}");
        let (g2, _) = {
            a.idea.set(&"w ".repeat(150));
            draw(&mut a, &cx, 1280, 720, 0)
        };
        let capped = g2.hits.iter().find(|h| h.label == "idea").unwrap().rect.h;
        assert!(a.idea.rows() <= 10 && capped <= big * 1.0 + 10.0 * 30.0, "at most ten lines");
        // a press places the caret, a drag selects, a triple click selects all
        a.idea.set("one\ntwo words\nthree");
        let _ = draw(&mut a, &cx, 1280, 720, 0);
        let (pad, lh) = a.idea_geo.unwrap();
        press(&mut a, &cx, pad, pad + lh * 0.5);
        assert_eq!(a.idea.cur, 0);
        press(&mut a, &cx, pad + 5000.0, pad + lh * 1.5);
        assert_eq!(a.idea.cur, "one\ntwo words".chars().count(), "the end of the second line");
        crate::ui::set_dragging(true);
        press(&mut a, &cx, pad + 5000.0, pad + lh * 2.5);
        crate::ui::set_dragging(false);
        assert_eq!(a.idea.selected_text().as_deref(), Some("\nthree"));
        crate::ui::set_click_count(3);
        press(&mut a, &cx, pad + 2.0, pad + 2.0);
        crate::ui::set_click_count(1);
        assert_eq!(a.idea.selected_text().as_deref(), Some("one\ntwo words\nthree"));
        // cut puts it on the clipboard and removes it
        key(&mut a, &cx, Key::Ctrl('x'));
        assert!(a.idea.text.is_empty() && crate::ui::clipboard::latest().as_deref() == Some("one\ntwo words\nthree"));
    }

    #[test]
    fn each_scene_gets_a_drawing_made_in_the_background_and_kept_with_the_story() {
        let t = Teller::new(STORY);
        let (mut a, cx, home) = make("tales-pics", &t);
        write_story(&mut a, &cx, "a fox");
        assert_eq!(a.pics.len(), 2, "three paragraphs, two scenes");
        until(&mut a, &cx, "the drawings", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
        // the model was asked for each, with no tools, with the style, the safety words for children and the scene
        let calls = t.drawing_calls();
        assert_eq!(calls.len(), 2);
        for (msgs, tools) in &calls {
            assert_eq!(*tools, 0);
            assert!(msgs[0].content.contains(PictureStyle::Watercolor.phrase()) && msgs[0].content.contains("small children") && msgs[0].content.contains("YOU HAVE NO TOOLS"));
            assert!(msgs[1].content.contains("The Brave Little Fox"));
        }
        assert!(calls[0].0[1].content.contains("Once upon a time") && calls[1].0[1].content.contains("She walked home"));
        // each is placed before the paragraph it shows
        assert_eq!(a.pics.iter().map(|p| p.scene.place_before).collect::<Vec<_>>(), vec![1, 2], "the first paragraph, then the other two");
        assert!(a.pics.iter().all(|p| p.ext == "svg" && p.bytes.as_ref().is_some_and(|b| b.starts_with(b"<svg")) && p.prompt.contains("watercolor")));
        // drawn in the page, and the page lays out the same story around them
        let before = a.pic_cache.rasterised;
        let (g, area) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(a.pic_cache.rasterised > before, "the picture was drawn");
        assert_layout(&g, area, "story with pictures");
        let (_, _) = draw(&mut a, &cx, 1280, 720, 1);
        assert_eq!(a.pic_cache.rasterised, a.pic_cache.rasterised.max(before + 1));
        // kept in the story folder with the prompt, and the text refers to them
        let id = a.saved_id().unwrap();
        let dir = home.join(format!("Stories/{id}"));
        assert!(dir.join("pics/01.svg").is_file() && dir.join("pics/02.svg").is_file());
        let index = std::fs::read_to_string(dir.join("pics/index.json")).unwrap();
        assert!(index.contains("watercolor") && index.contains("SVG drawing") && index.contains("Once upon a time"), "{index}");
        let md = std::fs::read_to_string(dir.join("story.md")).unwrap();
        assert!(md.contains("![The Brave Little Fox, part 1](pics/01.svg)") && md.contains("![The Brave Little Fox, part 2](pics/02.svg)"), "{md}");
        assert!(md.find("pics/01.svg").unwrap() < md.find("Once upon a time").unwrap());
        assert_eq!(a.store.load(&id).unwrap().entry.pictures, 2);
        // one picture again: only that one changes
        let first = a.pics[0].bytes.clone();
        let second = a.pics[1].bytes.clone();
        *t.scene.lock().unwrap() = SCENE.replace("\"x\":85", "\"x\":15").replace("meadow", "snow");
        click(&mut a, &cx, T_PIC_REDO + 1);
        assert!(matches!(a.pics[1].state, PicState::Queued | PicState::Working));
        until(&mut a, &cx, "the new drawing", |a| a.pics[1].state == PicState::Ready && a.pics[1].bytes != second);
        assert_eq!(a.pics[0].bytes, first, "the other picture is untouched");
        assert_eq!(std::fs::read(dir.join("pics/02.svg")).unwrap(), a.pics[1].bytes.as_ref().unwrap().to_vec(), "and the saved one follows");
        // what was asked and answered is kept beside the story, with the character sheet every picture used
        let log = std::fs::read_to_string(dir.join("pictures.log")).unwrap();
        assert!(log.contains("Picture 1") && log.contains("REQUEST") && log.contains("REPLY") && log.contains("scene composer"), "{log}");
        let sheet = std::fs::read_to_string(dir.join("character.json")).unwrap();
        assert!(sheet.contains("\"gender\"") && a.sheet.is_some(), "{sheet}");
        assert!(a.pics.iter().all(|p| p.how == "cartoon (built-in)"), "the status says what drew it: {:?}", a.pics.iter().map(|p| &p.how).collect::<Vec<_>>());
        assert_eq!(t.drawing_calls().len(), 3);
    }

    #[test]
    fn a_drawing_that_cannot_be_used_leaves_a_placeholder_and_can_be_tried_again() {
        let t = Teller::new(STORY);
        *t.draw_error.lock().unwrap() = Some("connection refused".into());
        let (mut a, cx, _) = make("tales-badpic", &t);
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the failures", |a| a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
        assert_eq!(t.drawing_calls().len(), 6, "each picture: the scene twice, then the plain drawing");
        let PicState::Failed(why) = a.pics[0].state.clone() else { unreachable!() };
        assert!(why.contains("connection refused"), "{why}");
        // the story is all there, the placeholder says why, and the way to try again is one click
        assert!(a.story_text().contains("slept well"));
        let (g, area) = draw(&mut a, &cx, 1280, 720, 0);
        assert_layout(&g, area, "placeholders");
        // the pictures are in the text, which scrolls: the way to try again is there
        let mut found = false;
        for k in 0..40 {
            a.tscroll = k as f32 * 25.0;
            let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
            found |= g.hits.iter().any(|h| h.label == "Try again");
        }
        assert!(found, "Try again is reachable by scrolling the story");
        a.tscroll = 0.0;
        // a model that cooperates now: one retry
        *t.draw_error.lock().unwrap() = None;
        click(&mut a, &cx, T_PIC_REDO);
        until(&mut a, &cx, "the retry", |a| a.pics[0].state == PicState::Ready);
        assert!(matches!(a.pics[1].state, PicState::Failed(_)), "the other one still waits for its turn");
        // pictures that are switched off cost nothing
        let t2 = Teller::new(STORY);
        let (mut b, cx2, home) = make("tales-nopic", &t2);
        click(&mut b, &cx2, T_PICMODE);
        assert_eq!(b.options().pictures, PictureMode::Off);
        write_story(&mut b, &cx2, "a fox");
        assert!(b.pics.is_empty() && t2.drawing_calls().is_empty());
        let md = std::fs::read_to_string(home.join(format!("Stories/{}/story.md", b.saved_id().unwrap()))).unwrap();
        assert!(!md.contains("!["));
    }

    #[test]
    fn save_story_is_a_clear_button_with_a_saved_state_and_an_automatic_option() {
        let t = Teller::new(STORY);
        let (mut a, cx, home) = make("tales-save", &t);
        assert!(a.prefs.auto_save, "saving by itself is the default");
        click(&mut a, &cx, T_AUTOSAVE);
        assert!(!a.prefs.auto_save);
        assert!(!a.store.load_prefs().auto_save, "and the choice is remembered");
        write_story(&mut a, &cx, "a fox");
        assert!(a.saved_id().is_none() && a.store.list().is_empty(), "nothing is saved unasked");
        let _ = &home;
        assert!(a.note_text().unwrap().contains("not saved yet"), "{:?}", a.note_text());
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Save story") && !g.hits.iter().any(|h| h.label == "Saved ✓"));
        until(&mut a, &cx, "the drawings", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
        // the button: text and the pictures that are ready go into Stories/<title>/
        click(&mut a, &cx, T_SAVE);
        let id = a.saved_id().expect("saved");
        let dir = home.join(format!("Stories/{id}"));
        assert!(dir.join("story.md").is_file() && dir.join("pics/01.svg").is_file(), "text and pictures");
        assert!(a.note_text().unwrap().contains(&format!("Saved in Stories/{id}/")) && a.note_text().unwrap().contains("2 pictures"), "{:?}", a.note_text());
        let (g, area) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Saved ✓") && !g.hits.iter().any(|h| h.label == "Save story"), "the state shows");
        assert_layout(&g, area, "saved");
        // it is in Listen again, with its pictures
        key(&mut a, &cx, Key::Char('l'));
        assert_eq!(a.library_ids(), vec![id.clone()]);
        assert_eq!(a.lib[0].pictures, 2);
        // switching saving on saves a story that is waiting
        let t2 = Teller::new(STORY);
        let (mut b, cx2, home2) = make("tales-save2", &t2);
        click(&mut b, &cx2, T_AUTOSAVE);
        write_story(&mut b, &cx2, "a fox");
        assert!(b.saved_id().is_none());
        click(&mut b, &cx2, T_AUTOSAVE);
        assert!(b.saved_id().is_some() && home2.join("Stories").is_dir(), "switching it on saves what is waiting");
    }

    #[test]
    fn save_story_keeps_the_audio_too_when_it_has_been_spoken() {
        let t = Teller::new(STORY);
        let (mut a, cx, home) = make("tales-saveaudio", &t);
        let dir = temp_dir("tales-saveaudio-eng");
        let _eng = Engine::install(&cx, &dir, &logging_player_for(&dir.join("p"), "0.02"));
        a.refresh_tts(&cx);
        a.prefs.auto_save = false;
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "all of it spoken", |a| a.reader_status() == Status::Done);
        click(&mut a, &cx, T_SAVE);
        let id = a.saved_id().unwrap();
        assert!(home.join(format!("Stories/{id}/audio.wav")).is_file() && home.join(format!("Stories/{id}/audio.json")).is_file());
        assert!(a.note_text().unwrap().contains("audio"), "{:?}", a.note_text());
        assert!(a.store.load(&id).unwrap().entry.has_audio);
    }

    #[test]
    fn export_makes_one_html_page_with_the_pictures_inside_or_the_text() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-export", &t);
        a.prefs.auto_save = false;
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the drawings", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
        // an unsaved story can be exported too: the page is made from what is on screen
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Export…"));
        click(&mut a, &cx, T_EXPORT);
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label.starts_with("Page with pictures")), "the menu is up");
        key(&mut a, &cx, Key::Enter);
        let (name, bytes) = a.take_host_save().expect("handed to the host-files save dialog");
        assert_eq!(name, "the-brave-little-fox.html");
        let html = String::from_utf8(bytes).unwrap();
        assert!(html.starts_with("<!doctype html>") && html.contains("<h1>The Brave Little Fox</h1>"));
        assert_eq!(html.matches("data:image/svg+xml;base64,").count(), 2, "both pictures are inside the file");
        assert!(!html.contains("<script") && !html.contains("src=\"http") && !html.contains("<link"), "self-contained, no script");
        assert!(html.find("<img").unwrap() < html.find("Once upon a time").unwrap());
        // Esc closes the menu without exporting
        click(&mut a, &cx, T_EXPORT);
        key(&mut a, &cx, Key::Esc);
        assert!(a.take_host_save().is_none() && a.modal.is_none());
        // a saved story from Listen again exports the same way, from what is on disk
        click(&mut a, &cx, T_SAVE);
        let id = a.saved_id().unwrap();
        a.go(Page::Library, &cx);
        click(&mut a, &cx, T_LIB_EXPORT);
        key(&mut a, &cx, Key::Enter);
        let (name, bytes) = a.take_host_save().unwrap();
        assert_eq!(name, format!("{id}.html"));
        let html = String::from_utf8(bytes).unwrap();
        assert_eq!(html.matches("data:image/svg+xml;base64,").count(), 2);
        // the text
        click(&mut a, &cx, T_LIB_EXPORT);
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Enter);
        let (name, bytes) = a.take_host_save().unwrap();
        assert_eq!(name, format!("{id}.md"));
        assert!(String::from_utf8(bytes).unwrap().contains("![The Brave Little Fox, part 1](pics/01.svg)"), "the Markdown refers to the pictures");
    }

    #[test]
    fn a_saved_story_opens_with_its_pictures_where_they_were() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-reopen", &t);
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the drawings", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
        let id = a.saved_id().unwrap();
        let places: Vec<usize> = a.pics.iter().map(|p| p.scene.place_before).collect();
        a.go(Page::Library, &cx);
        click(&mut a, &cx, T_LIB_OPEN);
        assert_eq!(a.page_name(), "story");
        assert_eq!(a.saved_id().as_deref(), Some(id.as_str()));
        assert_eq!(a.pics.len(), 2);
        assert!(a.pics.iter().all(|p| p.state == PicState::Ready));
        assert_eq!(a.pics.iter().map(|p| p.scene.place_before).collect::<Vec<_>>(), places, "in the same places");
        assert!(a.pics[0].prompt.contains("watercolor"));
        let (g, area) = draw(&mut a, &cx, 1280, 720, 0);
        assert_layout(&g, area, "a saved story with pictures");
        assert!(a.pic_cache.rasterised >= 1);
    }

    // ------------------------------------------------------------------ the model

    fn add_providers(cx: &AppCx, models: &[&str]) {
        let mut a = iyakki_agent::providers::ProviderCfg::new("home", iyakki_agent::providers::Kind::Ollama);
        a.models = models.iter().map(|m| m.to_string()).collect();
        let mut b = iyakki_agent::providers::ProviderCfg::new("off", iyakki_agent::providers::Kind::Ollama);
        b.models = vec!["hidden".into()];
        b.enabled = false;
        let mut st = cx.services.settings.lock().unwrap();
        st.upsert_provider(a).unwrap();
        st.upsert_provider(b).unwrap();
    }

    /// Draw the form scrolled step by step until a hit whose label starts with `prefix` shows; its label.
    fn find_in_form(a: &mut StoryApp, cx: &AppCx, w: u32, h: u32, prefix: &str) -> Option<String> {
        a.pane_open = true;
        a.menu = Some(Menu::Options);
        for step in 0..30 {
            a.cscroll = step as f32 * 80.0;
            let (g, _) = draw(a, cx, w, h, 1);
            if let Some(x) = g.hits.iter().find(|x| x.label.starts_with(prefix)) {
                return Some(x.label.clone());
            }
        }
        None
    }

    /// Make the tile use `other` for any chosen model and remember which (provider, model) pairs were asked for.
    fn record_choices(a: &mut StoryApp, other: &Arc<Teller>) -> Arc<Mutex<Vec<(String, String)>>> {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let (log, other) = (asked.clone(), other.clone());
        a.test_backend = Some(Arc::new(move |c: &ModelChoice| {
            log.lock().unwrap().push((c.provider.clone(), c.model.clone()));
            Arc::new(Shared(other.clone())) as Arc<dyn ChatBackend>
        }));
        asked
    }

    #[test]
    fn the_model_chosen_in_tales_writes_the_story_without_tools_and_is_remembered_and_chat_is_unchanged() {
        let chat_model = Teller::new("# Wrong\n\nThe chat model must not be asked.");
        let tales_model = Teller::new(STORY);
        let (mut a, mut cx, home) = make("tales-model", &chat_model);
        cx.session.policy.manage_settings = false; // not an administrator: may still choose here
        add_providers(&cx, &["llama3", "qwen3"]);
        cx.services.settings.lock().unwrap().set_chat(Some(ModelChoice { provider: "home".into(), model: "llama3".into() })).unwrap();
        let asked = record_choices(&mut a, &tales_model);
        // by default: same as chat, and the chip says so
        assert_eq!(a.model_label(&cx).0, "same as chat (llama3 · home)");
        // Ctrl+P: the default line, then the models of the enabled providers only
        key(&mut a, &cx, Key::Ctrl('p'));
        let Some(Modal::Model(m)) = &a.modal else { panic!("the chooser is open") };
        assert_eq!(m.list.items[1..3], [("home".to_string(), "llama3".to_string()), ("home".to_string(), "qwen3".to_string())], "the same list as the chat's, behind the first line");
        assert_eq!(m.list.rows[0], crate::apps::model_picker::MRow::Top);
        assert_eq!(m.list.sel, 0, "same as chat is highlighted");
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Enter);
        assert!(a.modal.is_none());
        assert_eq!(a.prefs.model, Some(ModelChoice { provider: "home".into(), model: "qwen3".into() }));
        assert_eq!(a.store.load_prefs().model, a.prefs.model, "remembered for the next visit");
        let chat = cx.services.settings.lock().unwrap().get().chat.clone().unwrap();
        assert_eq!(chat.model, "llama3", "the chat model was not changed by a non-administrator");
        assert!(cx.audit.recent(10).iter().any(|l| l.contains("story.model") && l.contains("qwen3")));
        // the story is written by the chosen model, with no tools, and the page says which
        write_story(&mut a, &cx, "a fox");
        assert_eq!(asked.lock().unwrap().first(), Some(&("home".to_string(), "qwen3".to_string())));
        assert_eq!(chat_model.story_calls().len(), 0, "the chat model was not asked");
        let calls = tales_model.story_calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].1, 0, "still no tools");
        assert_eq!(a.run_model, "qwen3 · home");
        let (_tx, rx) = channel();
        a.gen = Some(Gen { rx, stop: Arc::new(AtomicBool::new(false)) });
        assert_eq!(a.status_line(&a.reader.snapshot()).0, "Writing the story with qwen3 · home…");
        a.gen = None;
        assert!(cx.audit.recent(20).iter().any(|l| l.contains("story.create") && l.contains("model home qwen3")));
        // a new visit finds the choice again
        let mut b = StoryApp::new(&cx);
        b.store = StoryStore::open_at(&cx.session.policy, "tester", cx.services.dir(), home.clone());
        b.prefs = b.store.load_prefs();
        assert_eq!(b.model_label(&cx).0, "qwen3 · home");
        // going back to "same as chat" is one more choice
        key(&mut a, &cx, Key::Ctrl('p'));
        key(&mut a, &cx, Key::Home);
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.prefs.model, None);
        assert_eq!(a.store.load_prefs().model, None);
    }

    #[test]
    fn a_choice_whose_provider_was_removed_falls_back_to_the_chat_model_with_a_note_and_works_again_when_it_returns() {
        let chat_model = Teller::new(STORY);
        let tales_model = Teller::new("# Wrong\n\nNot this one.");
        let (mut a, cx, _) = make("tales-gone", &chat_model);
        add_providers(&cx, &["llama3", "qwen3"]);
        let mut base = iyakki_agent::providers::ProviderCfg::new("base", iyakki_agent::providers::Kind::Ollama);
        base.models = vec!["small".into()];
        cx.services.settings.lock().unwrap().upsert_provider(base).unwrap();
        cx.services.settings.lock().unwrap().set_chat(Some(ModelChoice { provider: "base".into(), model: "small".into() })).unwrap();
        let asked = record_choices(&mut a, &tales_model);
        a.prefs.model = Some(ModelChoice { provider: "home".into(), model: "qwen3".into() });
        cx.services.settings.lock().unwrap().remove_provider("home").unwrap_or(());
        let (label, note) = a.model_label(&cx);
        assert!(label.starts_with("same as chat"), "{label}");
        assert!(note.as_deref().is_some_and(|n| n.contains("qwen3") && n.contains("not available")), "{note:?}");
        // the page shows the note, and the story is written by the chat model
        assert_eq!(a.writer_line(&cx), "model: same as chat (small · base)");
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Model: same as chat ▾"));
        write_story(&mut a, &cx, "a fox");
        assert!(asked.lock().unwrap().is_empty(), "no backend was made for the missing model");
        assert_eq!(chat_model.story_calls().len(), 1);
        assert_eq!(a.prefs.model.as_ref().map(|c| c.model.as_str()), Some("qwen3"), "the choice is kept, not erased");
        // the provider comes back: the choice is used again
        add_providers(&cx, &["llama3", "qwen3"]);
        assert_eq!(a.model_label(&cx), ("qwen3 · home".to_string(), None));
    }

    #[test]
    fn the_picture_model_is_chosen_separately_and_only_offered_for_a_picture_model() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-picmodel", &t);
        // with no picture model anywhere there is nothing to choose: no chip in Options
        a.opts.pictures = PictureMode::Auto;
        a.image_models.clear();
        assert_eq!(find_in_form(&mut a, &cx, 1920, 1080, "Picture model"), None, "not offered when images are not available");
        // with one it is offered, unless the pictures are off
        a.image_models = vec![("home".into(), "flux".into())];
        assert_eq!(find_in_form(&mut a, &cx, 1920, 1080, "Picture model").as_deref(), Some("Picture model: automatic"));
        a.opts.pictures = PictureMode::Off;
        assert_eq!(find_in_form(&mut a, &cx, 1920, 1080, "Picture model"), None, "pictures off: no picture model to choose");
        a.opts.pictures = PictureMode::Auto;
        // with no picture model anywhere the chooser says so and stays closed
        a.image_models.clear();
        click(&mut a, &cx, T_PICMODEL);
        assert!(a.modal.is_none() || matches!(a.modal, Some(Modal::Model(_))));
        // with one, it is chosen, remembered and handed to the picture request
        a.modal = None;
        a.image_models = vec![("home".into(), "flux".into()), ("home".into(), "sdxl".into())];
        add_providers(&cx, &["flux", "sdxl"]);
        click(&mut a, &cx, T_PICMODEL);
        {
            let Some(Modal::Model(m)) = &a.modal else { panic!("the picture chooser is open") };
            assert!(m.picture && m.list.title == "Choose the picture model", "{}", m.list.title);
            assert_eq!(m.list.items.len(), 3, "Automatic and the two picture models; no agents");
            assert_eq!(m.list.sel, 0);
        }
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.prefs.picture_model, Some(ModelChoice { provider: "home".into(), model: "sdxl".into() }));
        assert_eq!(a.store.load_prefs().picture_model, a.prefs.picture_model);
        assert_eq!(a.picture_plan(&cx), ModelPlan::Chosen(ModelChoice { provider: "home".into(), model: "sdxl".into() }));
        assert_eq!(a.picture_label(&cx).0, "sdxl · home");
    }

    #[test]
    fn the_model_chooser_fits_and_works_by_click_and_key_at_every_screen_size() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-pickgeo", &t);
        let names: Vec<String> = (0..40).map(|i| format!("model-{i:02}-with-a-fairly-long-name:latest")).collect();
        add_providers(&cx, &names.iter().map(String::as_str).collect::<Vec<_>>());
        let _asked = record_choices(&mut a, &t);
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            for text_scale in [1.0f32, 1.35] {
                let s = crate::shell::scale_for(w, h) * text_scale;
                a.go(Page::Story, &cx);
                a.pane_open = true;
                // the composer with the Model chip: Send is always in view
                let (g, _) = draw_at(&mut a, &cx, w, h, s, 1);
                assert!(g.hits.iter().any(|x| x.label == "Send"), "Send in view at {w}x{h} x{text_scale}");
                assert!(g.hits.iter().any(|x| x.label.starts_with("Model: ")), "the Model chip at {w}x{h} x{text_scale}");
                // the chooser: opened by Ctrl+P, every visible line inside the screen, the highlighted line in view
                a.cscroll = 0.0;
                a.focus = Focus::None;
                key(&mut a, &cx, Key::Ctrl('p'));
                assert!(matches!(a.modal, Some(Modal::Model(_))), "Ctrl+P opens at {w}x{h}");
                for step in [Key::Down, Key::PageDown, Key::End, Key::PageUp, Key::Home, Key::Down] {
                    key(&mut a, &cx, step);
                    let (g, area) = draw_at(&mut a, &cx, w, h, s, 2);
                    assert_layout(&g, area, &format!("chooser at {w}x{h} x{text_scale}"));
                    let rows: Vec<_> = g.hits.iter().filter(|x| x.label == "model").collect();
                    assert!(rows.len() >= 3, "{} lines at {w}x{h}", rows.len());
                    let Some(Modal::Model(m)) = &a.modal else { panic!() };
                    assert!(rows.iter().any(|r| matches!(r.ev, Event::Row(n) if n == m.list.sel)), "the highlighted line is in view at {w}x{h} (sel {})", m.list.sel);
                    assert!(!g.hits.iter().any(|x| x.label == "Send"), "the chooser covers the composer");
                    for r in &rows {
                        assert!(r.rect.y >= area.y && r.rect.y + r.rect.h <= area.y + area.h, "line inside the window at {w}x{h}: {:?}", r.rect);
                    }
                }
                // a click on a line chooses it
                let (g, _) = draw_at(&mut a, &cx, w, h, s, 3);
                let target = g.hits.iter().filter(|x| x.label == "model").filter_map(|x| if let Event::Row(n) = x.ev { Some(n) } else { None }).last().unwrap();
                a.handle(&Event::Row(target), &cx);
                assert!(a.modal.is_none());
                assert_eq!(a.prefs.model.as_ref().map(|c| c.model.clone()), Some(names[target - 1].clone()), "line {target} at {w}x{h}");
                // Esc closes without changing; the chip's click opens it too
                click(&mut a, &cx, T_MODEL);
                assert!(matches!(a.modal, Some(Modal::Model(_))));
                key(&mut a, &cx, Key::Home);
                key(&mut a, &cx, Key::Esc);
                assert!(a.modal.is_none());
                assert_eq!(a.prefs.model.as_ref().map(|c| c.model.clone()), Some(names[target - 1].clone()), "Esc changes nothing");
                a.prefs.model = None;
            }
        }
    }

    /// The row of `(agent, model)` / provider model in the open chooser.
    fn pick_row(a: &StoryApp, f: impl Fn(&crate::apps::model_picker::MRow, &(String, String)) -> bool) -> usize {
        let Some(Modal::Model(m)) = &a.modal else { panic!("the chooser is open") };
        m.list.rows.iter().zip(&m.list.items).position(|(r, i)| f(r, i)).expect("the row is in the list")
    }

    #[test]
    fn one_chooser_picks_a_provider_model_an_agent_model_a_custom_id_or_same_as_chat_and_replaces_the_other() {
        use crate::apps::model_picker::MRow;
        let t = Teller::new(STORY);
        let (mut a, mut cx, _) = make("tales-onechoice", &t);
        cx.session.policy.manage_settings = true;
        add_providers(&cx, &["qwen3:8b", "llama3"]);
        let dir = temp_dir("tales-onechoice-bin");
        set_agents(&cx, Some(&fake_claude(&dir, false)), None);
        assert_eq!(a.chip_text(&cx), "Model: same as chat");
        // a provider's model, found by its row; the chip says model and provider
        key(&mut a, &cx, Key::Ctrl('p'));
        let r = pick_row(&a, |_, i| i.1 == "qwen3:8b");
        a.handle(&Event::Row(r), &cx);
        assert!(a.modal.is_none());
        assert_eq!(a.chip_text(&cx), "Model: qwen3:8b · home");
        assert_eq!(a.choice(&cx), Choice::Provider(ModelChoice { provider: "home".into(), model: "qwen3:8b".into() }));
        // an agent's model: the agent replaces the provider's model as the one choice (the model is kept as the fallback)
        key(&mut a, &cx, Key::Ctrl('p'));
        let r = pick_row(&a, |r, _| matches!(r, MRow::Model(ag, mi) if ag == "claude-code" && mi.id == "sonnet"));
        a.handle(&Event::Row(r), &cx);
        assert_eq!(a.chip_text(&cx), "Model: Sonnet · Claude Code");
        assert_eq!(a.choice(&cx), Choice::Agent("claude-code".into(), "sonnet".into()));
        // the list opens on it, and the agent's header keeps the model
        key(&mut a, &cx, Key::Ctrl('p'));
        let Some(Modal::Model(m)) = &a.modal else { panic!() };
        assert_eq!(m.list.sel, r);
        let head = pick_row(&a, |r, _| matches!(r, MRow::Head(id) if id == "claude-code"));
        a.handle(&Event::Row(head), &cx);
        assert_eq!(a.choice(&cx), Choice::Agent("claude-code".into(), "sonnet".into()), "the header keeps the model");
        // a custom id typed in the list is refused with shell characters, then used and remembered for the agent
        key(&mut a, &cx, Key::Ctrl('g'));
        let c = pick_row(&a, |r, _| matches!(r, MRow::Custom(id) if id == "claude-code"));
        a.handle(&Event::Row(c), &cx);
        for ch in "x; ls".chars() {
            key(&mut a, &cx, Key::Char(ch));
        }
        key(&mut a, &cx, Key::Enter);
        assert!(matches!(a.modal, Some(Modal::Model(_))) && a.note_text().is_some_and(|n| n.contains("letters, digits")), "{:?}", a.note_text());
        for _ in 0..5 {
            key(&mut a, &cx, Key::Backspace);
        }
        for ch in "claude-opus-5-5".chars() {
            key(&mut a, &cx, Key::Char(ch));
        }
        key(&mut a, &cx, Key::Enter);
        assert!(a.modal.is_none());
        assert_eq!(a.prefs.agent_model, "claude-opus-5-5");
        assert_eq!(a.chip_text(&cx), "Model: claude-opus-5-5 · Claude Code");
        assert_eq!(cx.services.settings.lock().unwrap().get().agents.claude.custom_models, ["claude-opus-5-5"]);
        assert_eq!(a.store.load_prefs().agent_model, "claude-opus-5-5", "remembered");
        // the story keeps its no-tools rule and the page says who writes
        assert!(a.writer_line(&cx).contains("no tools"), "{}", a.writer_line(&cx));
        // the first line, "Same as chat", ends both
        key(&mut a, &cx, Key::Ctrl('p'));
        key(&mut a, &cx, Key::Home);
        key(&mut a, &cx, Key::Enter);
        assert_eq!((a.prefs.agent.clone(), a.prefs.model.clone(), a.prefs.agent_model.clone()), (None, None, String::new()));
        assert_eq!(a.chip_text(&cx), "Model: same as chat");
        // a provider's model also ends an agent choice
        a.prefs.agent = Some("claude-code".into());
        key(&mut a, &cx, Key::Ctrl('p'));
        let r = pick_row(&a, |_, i| i.1 == "llama3");
        a.handle(&Event::Row(r), &cx);
        assert_eq!((a.prefs.agent.clone(), a.chip_text(&cx)), (None, "Model: llama3 · home".to_string()));
    }

    #[test]
    fn old_preferences_with_a_model_and_an_agent_become_one_choice() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-migrate", &t);
        add_providers(&cx, &["qwen3:8b"]);
        let old = r#"{"model":{"provider":"home","model":"qwen3:8b"},"agent":"claude-code","agent_model":"sonnet"}"#;
        a.prefs = serde_json::from_str(old).unwrap();
        assert_eq!(a.choice(&cx), Choice::Agent("claude-code".into(), "sonnet".into()), "the agent answered before, so it is the choice");
        assert_eq!(a.chip_text(&cx), "Model: Sonnet · Claude Code");
        let old = r#"{"model":{"provider":"home","model":"qwen3:8b"}}"#;
        a.prefs = serde_json::from_str(old).unwrap();
        assert_eq!(a.chip_text(&cx), "Model: qwen3:8b · home");
        a.prefs = serde_json::from_str("{}").unwrap();
        assert_eq!(a.chip_text(&cx), "Model: same as chat");
        // an agent that is not configured any more falls to the model that was chosen too
        a.prefs = serde_json::from_str(r#"{"model":{"provider":"home","model":"qwen3:8b"},"agent":"custom:gone"}"#).unwrap();
        assert_eq!(a.chip_text(&cx), "Model: qwen3:8b · home");
    }

    // ------------------------------------------------------------------ browser voice

    /// A page that answers like a browser: the voices at once, then each piece starts, lasts a moment and ends.
    fn browser_page(hv: &Arc<HostVoice>, talk_ms: u64) -> (Arc<Mutex<Vec<String>>>, Arc<AtomicBool>) {
        use iyakki_agent::hostvoice::{FromClient, Front, ToClient, VoiceInfo};
        hv.connect(Front::Browser);
        let (said, stop) = (Arc::new(Mutex::new(Vec::new())), Arc::new(AtomicBool::new(false)));
        let (h, sd, st) = (hv.clone(), said.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut cur: Option<(u64, std::time::Instant)> = None;
            while !st.load(Ordering::Relaxed) {
                for m in h.take_outgoing() {
                    match m {
                        ToClient::ListVoices => h.event(FromClient::Voices {
                            supported: true,
                            voices: vec![VoiceInfo { name: "Lekha".into(), lang: "ta-IN".into(), local: true }, VoiceInfo { name: "Samantha".into(), lang: "en-US".into(), local: true }, VoiceInfo { name: "Daniel".into(), lang: "en-GB".into(), local: true }],
                        }),
                        ToClient::Speak { id, text, .. } => {
                            sd.lock().unwrap().push(text);
                            h.event(FromClient::Started { id });
                            cur = Some((id, std::time::Instant::now()));
                        }
                        ToClient::Cancel { id } => {
                            // as the real page: only the utterance named (or whatever is said when none is named)
                            if id.is_none() || cur.is_some_and(|c| Some(c.0) == id) {
                                cur = None;
                            }
                        }
                        _ => {}
                    }
                }
                if let Some((id, t0)) = cur {
                    if t0.elapsed() >= Duration::from_millis(talk_ms) {
                        h.event(FromClient::Ended { id });
                        cur = None;
                    }
                }
                std::thread::sleep(Duration::from_millis(3));
            }
        });
        (said, stop)
    }

    fn hit_ev(g: &Gfx, label: &str) -> Event {
        g.hits.iter().find(|h| h.label == label).unwrap_or_else(|| panic!("no control named {label}")).ev.clone()
    }

    #[test]
    fn with_a_browser_connected_and_nothing_else_set_up_the_story_is_read_by_the_browser_at_every_screen_size() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-bv", &t);
        let hv = HostVoice::current();
        let (said, stop) = browser_page(&hv, 1200);
        until(&mut a, &cx, "the page's answer", |_| hv.is_ready());
        a.refresh_tts(&cx);
        assert!(a.browser_voice && matches!(a.tts, TtsStatus::Ready { .. }), "{:?}", a.tts);
        assert!(st::status_text(&a.tts).contains("browser voice"));
        assert_eq!(a.voices, vec!["".to_string(), "Samantha".into(), "Daniel".into()], "automatic first, then the voices of the story's language (English)");
        assert!(a.takes_speed);
        a.prefs.read_while_writing = false;
        write_story(&mut a, &cx, "a fox");
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            a.go(Page::Story, &cx);
            a.pane_open = false;
            let s = crate::shell::scale_for(w, h);
            let (g, area) = draw_at(&mut a, &cx, w, h, s, 0);
            assert_layout(&g, area, &format!("browser voice story page at {w}x{h}"));
            for needed in ["Play", "Save audio"] {
                assert!(g.hits.iter().any(|x| x.label == needed), "{needed} at {w}x{h}");
            }
            // Save audio explains itself instead of making a file
            a.note = None;
            a.handle(&hit_ev(&g, "Save audio"), &cx);
            assert!(a.audio_job.is_none(), "no audio job");
            assert!(a.note_text().unwrap().contains("Save audio is not available with the browser voice"), "{:?}", a.note_text());
            // Play by its button: the pieces reach the page in order and the player follows
            said.lock().unwrap().clear();
            a.reader.stop();
            a.live = false;
            a.handle(&hit_ev(&g, "Play"), &cx);
            until(&mut a, &cx, "reading", |a| a.reader_status() == Status::Playing);
            let (g2, area) = draw_at(&mut a, &cx, w, h, s, 3);
            assert_layout(&g2, area, &format!("browser voice player at {w}x{h}"));
            for needed in ["Pause", "Stop", "progress"] {
                assert!(g2.hits.iter().any(|x| x.label == needed), "{needed} while reading at {w}x{h}: {:?} status {:?}", g2.hits.iter().map(|x| x.label.clone()).collect::<Vec<_>>(), a.reader_status());
            }
            let first = a.chunks[0].text.clone();
            assert_eq!(said.lock().unwrap().first(), Some(&first), "the first piece goes to the page first");
            a.handle(&hit_ev(&g2, "Pause"), &cx);
            assert_eq!(a.reader_status(), Status::Paused);
            a.handle(&Event::Tab(T_PLAY), &cx);
            // jump to the last piece with the progress bar's own path; it is said and the reading ends
            let last = a.chunks.len() - 1;
            a.seek(last, &cx);
            until(&mut a, &cx, "done", |a| a.reader_status() == Status::Done);
            assert_eq!(said.lock().unwrap().last(), Some(&a.chunks[last].text));
        }
        // the voice chip cycles through automatic and the browser's voices, and the choice is what is sent
        a.go(Page::Story, &cx);
        assert_eq!(find_in_form(&mut a, &cx, 1920, 1080, "Voice: ").as_deref(), Some("Voice: automatic"));
        a.handle(&Event::Tab(T_VOICE), &cx);
        assert_eq!(a.prefs.voice.as_deref(), Some("Samantha"));
        a.reader.stop();
        said.lock().unwrap().clear();
        a.start_reading(0, &cx);
        until(&mut a, &cx, "a piece said", |_| !said.lock().unwrap().is_empty());
        a.reader.stop();
        stop.store(true, Ordering::Relaxed);
    }

    #[test]
    fn on_the_console_the_story_says_how_to_get_the_browser_voice_and_an_explicit_choice_explains_itself() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-bv-console", &t);
        a.refresh_tts(&cx);
        assert_eq!(a.tts, TtsStatus::NoEngine);
        assert!(!a.browser_voice);
        write_story(&mut a, &cx, "a fox");
        click(&mut a, &cx, T_PLAY);
        let note = a.note_text().unwrap().to_string();
        assert!(note.contains("browser voice") && note.contains("iyakki web"), "{note}");
        assert!(note.find("browser voice").unwrap() < note.find("Settings → Voice").unwrap(), "the browser voice is offered first: {note}");
        // chosen in Settings, on a screen that cannot speak
        cx.services.settings.lock().unwrap().update(|s| s.voice.tts = Some(SpeechEngine::Browser)).unwrap();
        a.refresh_tts(&cx);
        assert!(matches!(a.tts, TtsStatus::BrowserUnavailable(_)), "{:?}", a.tts);
        click(&mut a, &cx, T_PLAY);
        assert!(a.note_text().unwrap().contains("Browser voice is chosen") && a.note_text().unwrap().contains("console"), "{:?}", a.note_text());
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            a.pane_open = true;
            let s = crate::shell::scale_for(w, h);
            let (g, area) = draw_at(&mut a, &cx, w, h, s, 0);
            assert_layout(&g, area, &format!("unavailable browser voice at {w}x{h}"));
            assert!(g.hits.iter().any(|x| x.label == "Send"));
        }
        // the voice permission still rules
        let mut cx2 = cx;
        cx2.session.policy.voice = false;
        a.refresh_tts(&cx2);
        assert_eq!(a.tts, TtsStatus::NotAllowed);
    }

    // ------------------------------------------------------------------ the composer: audience, kind, tone, suggestions

    #[test]
    fn who_is_it_for_is_the_only_button_group_and_is_remembered() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-aud", &t);
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        click_hit(&mut a, &cx, &g, "Teens");
        assert_eq!(a.options().audience, Audience::Teen);
        assert_eq!(a.store.load_prefs().options.audience, Audience::Teen, "remembered at once");
        key(&mut a, &cx, Key::Esc);
        let (g, _) = draw(&mut a, &cx, 1280, 720, 1);
        click_hit(&mut a, &cx, &g, "Adults");
        write_story(&mut a, &cx, "a fox");
        let sys = &t.story_calls()[0].0[0].content;
        assert!(sys.contains("SAFETY (adults)"), "the audience reached the prompt");
        // a new visit finds it again
        let mut b = StoryApp::new(&cx);
        b.store = StoryStore::open_at(&cx.session.policy, "tester", cx.services.dir(), a.store.dir().parent().unwrap().to_path_buf());
        b.prefs = b.store.load_prefs();
        assert_eq!(b.prefs.options.audience, Audience::Adult);
        // the other groups are not on the panel
        let (g, _) = draw(&mut a, &cx, 1280, 720, 2);
        let panel_buttons: Vec<&str> = g.hits.iter().filter(|h| matches!(h.ev, Event::Tab(n) if (T_GENRE..T_GENRE + 6).contains(&n) || (T_LENGTH..T_LENGTH + 3).contains(&n) || (T_LANG..T_LANG + 14).contains(&n) || (T_TONE..T_TONE + 5).contains(&n) || n == T_PICMODE || n == T_READ_WHILE || n == T_AUTOSAVE)).map(|h| h.label.as_str()).collect();
        assert!(panel_buttons.is_empty(), "{panel_buttons:?}");
    }

    #[test]
    fn kind_and_tone_are_drop_downs_that_open_a_list_by_click_and_by_key() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-drop", &t);
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            let s = crate::shell::scale_for(w, h);
            a.menu = None;
            let (g, _) = draw_at(&mut a, &cx, w, h, s, 0);
            assert!(g.hits.iter().any(|x| x.label == "Kind of story") && g.hits.iter().any(|x| x.label == "Tone"));
            click_hit(&mut a, &cx, &g, "Kind of story");
            assert_eq!(a.menu, Some(Menu::Kind));
            let (g, area) = draw_at(&mut a, &cx, w, h, s, 1);
            for gname in ["Bedtime", "Adventure", "Fantasy", "Sci-fi", "Moral", "Funny"] {
                let r = g.hits.iter().rev().find(|x| x.label == gname).unwrap_or_else(|| panic!("{gname} at {w}x{h}")).rect;
                assert!(r.y >= area.y - 1.0 && r.y + r.h <= h as f32 + 1.0 && r.x >= 0.0 && r.x + r.w <= w as f32 + 1.0, "{gname} inside the window at {w}x{h}: {r:?}");
            }
            click_hit(&mut a, &cx, &g, "Sci-fi");
            assert_eq!((a.options().genre, a.menu, a.focus_name()), (Genre::SciFi, None, "idea"));
            a.opts.genre = Genre::Bedtime;
        }
        // by key: open, Down, Enter; Esc leaves it as it was
        a.explicit = [false; 4];
        click(&mut a, &cx, T_TONE_MENU);
        assert_eq!(a.menu, Some(Menu::Tone));
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Enter);
        assert_eq!((a.options().tone, a.menu), (Tone::ALL[2], None));
        assert!(a.explicit[H_TONE]);
        assert_eq!(a.store.load_prefs().options.tone, Tone::ALL[2], "remembered");
        click(&mut a, &cx, T_KIND_MENU);
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Esc);
        assert_eq!((a.options().genre, a.menu), (Genre::Bedtime, None));
        // the list shows what the idea text suggested while nothing was chosen
        a.explicit = [false; 4];
        a.opts.tone = Tone::Calm;
        a.idea.set("an exciting story about a kite");
        a.sync_fields();
        assert_eq!(a.shown_options().tone, Tone::Exciting);
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|x| x.label == "Exciting ×" || x.label == "Tone: Exciting ×"), "the suggestion is a chip under the box");
    }

    #[test]
    fn the_idea_text_suggests_length_language_kind_and_tone_as_chips_and_what_is_chosen_wins() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-hints", &t);
        a.idea.set("a 5-minute funny story in Tamil about a fox, in comic style");
        a.sync_fields();
        let names: Vec<_> = a.hint_chips().iter().map(|c| (c.name, c.value.clone(), c.applied)).collect();
        assert_eq!(names, vec![("Length", "Medium · 5 min".to_string(), true), ("Language", "Tamil".to_string(), true), ("Kind", "Funny".to_string(), true), ("Picture style", "Comic".to_string(), true)]);
        // the options themselves are untouched; the chips show what was understood
        assert_eq!((a.options().length, a.options().language.as_str(), a.options().genre), (Length::Short, "English", Genre::Bedtime));
        let (g, area) = draw(&mut a, &cx, 1280, 720, 0);
        assert_layout(&g, area, "composer with chips");
        for label in ["Length: Medium · 5 min ×", "Language: Tamil ×", "Kind: Funny ×", "Picture style: Comic ×", "Pictures: cartoon (built-in)"] {
            assert!(g.hits.iter().any(|x| x.label == label), "{label}: {:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        }
        // a chip is removable: that suggestion is not used
        click_hit(&mut a, &cx, &g, "Language: Tamil ×");
        assert_eq!(a.shown_options().language, "English");
        assert!(a.hint_chips().iter().all(|c| c.name != "Language"));
        assert_eq!(a.focus_name(), "idea");
        // an explicit choice always wins, and the chip says so
        click(&mut a, &cx, T_GENRE);
        let chips = a.hint_chips();
        let kind = chips.iter().find(|c| c.name == "Kind").unwrap();
        assert!(!kind.applied && kind.chosen == "Bedtime");
        let (g, _) = draw(&mut a, &cx, 1280, 720, 1);
        assert!(g.hits.iter().any(|x| x.label == "\"funny\" ignored, Kind is Bedtime ×"));
        // Send uses the suggestions that stand, and nothing of them is remembered
        a.set_focus(Focus::Idea);
        key(&mut a, &cx, Key::Enter);
        until(&mut a, &cx, "the story", |a| !a.is_writing());
        let calls = t.story_calls();
        let (sys, user) = (&calls[0].0[0].content, &calls[0].0[1].content);
        assert!(sys.contains("A bedtime story") && !sys.contains("A funny story"), "the explicit kind won");
        assert!(sys.contains("About 750 words (between 600 and 900)"), "the five minutes reached the prompt: {sys}");
        assert!(user.contains("medium") && user.contains("in English"), "{user}");
        assert_eq!(a.story_opts.length, Length::Medium);
        assert_eq!(a.story_opts.style, PictureStyle::Comic, "the style named in the text");
        assert!(a.idea.text.is_empty(), "the box is empty after Send, like the chat's");
        assert_eq!(a.options().length, Length::Short, "a suggestion is not a choice");
        let saved = a.store.load_prefs().options;
        assert_eq!((saved.length, saved.language.as_str(), saved.genre), (Length::Short, "English", Genre::Bedtime));
        // the words come back as a suggestion when they change
        a.new_story(&cx);
        a.idea.set("a story in Hindi");
        a.sync_fields();
        assert_eq!(a.hint_chips().iter().map(|c| c.value.clone()).collect::<Vec<_>>(), vec!["Hindi".to_string()]);
        // an option chosen in Options beats the text, too
        click(&mut a, &cx, T_LANG + 3);
        assert!(!a.hint_chips()[0].applied);
        assert_eq!(a.shown_options().language, "Telugu");
        // nothing is guessed from words that only look like options
        a.idea.set("a fox in the long grass who loves warm soup");
        a.sync_fields();
        assert!(a.hint_chips().is_empty());
    }

    // ------------------------------------------------------------------ pictures: Auto

    #[test]
    fn pictures_are_automatic_images_first_svg_when_there_is_no_image_model_and_off_is_the_one_switch() {
        // no picture model: SVG drawings with the story model
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-auto-svg", &t);
        assert_eq!(a.options().pictures, PictureMode::Auto, "automatic is the default");
        assert_eq!(a.pictures_status(&cx), "Pictures: cartoon (built-in)");
        write_story(&mut a, &cx, "a fox");
        assert_eq!(a.story_opts.pictures, PictureMode::Drawings);
        assert!(a.pics.iter().all(|p| p.mode == PictureMode::Drawings));
        until(&mut a, &cx, "the drawings", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
        assert!(!t.drawing_calls().is_empty());
        assert_eq!(a.options().pictures, PictureMode::Auto, "automatic stays the remembered choice");
        assert_eq!(a.story_opts.style, PictureStyle::Watercolor, "a kids' bedtime story is a storybook watercolor");
        // a picture model exists: images have priority, named on the chip
        let t2 = Teller::new(STORY);
        let (mut b, cx2, _) = make("tales-auto-img", &t2);
        add_providers(&cx2, &["llama3", "flux"]);
        b.refresh_tts(&cx2);
        assert_eq!(b.pictures_status(&cx2), "Pictures: images via flux");
        let (g, _) = draw(&mut b, &cx2, 1280, 720, 0);
        assert!(g.hits.iter().any(|x| x.label == "Pictures: images via flux"), "the chip near the composer says it");
        write_story(&mut b, &cx2, "a fox");
        assert_eq!(b.story_opts.pictures, PictureMode::ImageModel);
        assert!(cx2.audit.recent(50).iter().any(|l| l.contains("story.picture") && l.contains("image scene 1")), "images were asked for first");
        assert!(find_in_form(&mut b, &cx2, 1280, 720, "Picture model").is_some(), "the picture model chip is in Options because images are available");
        // the story kind picks the style by itself, and the idea text can name another
        let t3 = Teller::new(STORY);
        let (mut c, cx3, _) = make("tales-style", &t3);
        click(&mut c, &cx3, T_GENRE + 3); // sci-fi
        write_story(&mut c, &cx3, "a robot");
        assert_eq!(c.story_opts.style, PictureStyle::Pixel);
        let t4 = Teller::new(STORY);
        let (mut d, cx4, _) = make("tales-style2", &t4);
        write_story(&mut d, &cx4, "a robot in pixel art");
        assert_eq!(d.story_opts.style, PictureStyle::Pixel, "named in the idea");
        // the one switch: pictures off, with or without a picture model
        let t5 = Teller::new(STORY);
        let (mut e, cx5, _) = make("tales-auto-off", &t5);
        add_providers(&cx5, &["flux"]);
        e.refresh_tts(&cx5);
        click(&mut e, &cx5, T_PICMODE);
        assert_eq!(e.pictures_status(&cx5), "Pictures: off");
        assert_eq!(e.store.load_prefs().options.pictures, PictureMode::Off, "remembered");
        write_story(&mut e, &cx5, "a fox");
        assert!(e.pics.is_empty() && t5.drawing_calls().is_empty());
        click(&mut e, &cx5, T_PICMODE);
        assert_eq!(e.options().pictures, PictureMode::Auto);
        // preferences saved by older versions (an explicit picture choice, a characters note) are brought along
        let mut o = StoryOptions { pictures: PictureMode::ImageModel, characters: "Old note".into(), ..Default::default() };
        normalize_options(&mut o);
        assert_eq!((o.pictures, o.characters.as_str()), (PictureMode::Auto, ""));
        let mut off = StoryOptions { pictures: PictureMode::Off, ..Default::default() };
        normalize_options(&mut off);
        assert_eq!(off.pictures, PictureMode::Off, "off stays off");
    }

    /// A rich drawing a strong model might make: a gradient sky and a few dozen shapes.
    fn rich_svg() -> String {
        format!("<svg xmlns='http://www.w3.org/2000/svg' viewBox='0 0 800 480'><defs><linearGradient id='s' x1='0' y1='0' x2='0' y2='1'><stop offset='0' stop-color='#ff9966'/><stop offset='1' stop-color='#3366cc'/></linearGradient></defs><rect width='800' height='480' fill='url(#s)'/>{}</svg>", (0..24).map(|i| format!("<circle cx='{}' cy='{}' r='{}' fill='#{:02x}{:02x}40'/>", 30 + i * 31, 100 + (i * 53) % 300, 12 + i, i * 10, 255 - i * 9)).collect::<String>())
    }

    #[test]
    fn the_pictures_are_drawn_by_the_chosen_writer_first_rich_for_a_strong_one_and_the_own_model_takes_over_when_it_fails() {
        let own = Teller::new(STORY);
        let agent = Teller::new(STORY);
        *agent.svg.lock().unwrap() = rich_svg();
        let (mut a, cx, home) = make("tales-route", &own);
        let (ta, to) = (agent.clone(), own.clone());
        a.test_drawers = Some(Arc::new(move |_| vec![Drawer { chat: Arc::new(Shared(ta.clone())), strong: true, who: "Claude Code".into() }, Drawer { chat: Arc::new(Shared(to.clone())), strong: false, who: "qwen3:8b".into() }]));
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the drawings", all_pictures_ready);
        assert_eq!(agent.drawing_calls().len(), 2, "the agent that was chosen drew both pictures");
        assert_eq!(own.drawing_calls().len(), 0, "the small model drew nothing");
        let ask = &agent.drawing_calls()[0].0;
        assert!(ask[0].content.contains("children's-book illustrator") && ask[0].content.contains("0 0 800 480"), "a real art-direction brief");
        assert!(ask[1].content.contains("DRAW THIS MOMENT") && ask[1].content.contains("Once upon a time") && ask[1].content.contains("The Brave Little Fox") && ask[1].content.contains("CHARACTER SHEET"), "the whole story, the moment and the sheet: {}", ask[1].content);
        assert!(a.pics.iter().all(|p| p.how == "rich SVG drawing by Claude Code"), "{:?}", a.pics.iter().map(|p| &p.how).collect::<Vec<_>>());
        let said: Vec<&str> = a.convo.iter().filter(|m| m.role == iyakki_agent::chat::Role::Tool).map(|m| m.content.as_str()).collect();
        assert!(said.iter().any(|r| r.contains("rich SVG drawing by Claude Code")), "{said:?}");
        // the person's words about one picture reach the writer
        say(&mut a, &cx, "redraw picture 2: make the sky pink");
        until(&mut a, &cx, "the new picture", all_pictures_ready);
        assert!(agent.drawing_calls().last().unwrap().0[1].content.contains("make the sky pink"), "the feedback is in the request");
        // draw again keeps the feedback
        click(&mut a, &cx, T_PIC_REDO + 1);
        until(&mut a, &cx, "again", all_pictures_ready);
        assert!(agent.drawing_calls().last().unwrap().0[1].content.contains("make the sky pink"));
        // the agent goes away: the writing model takes over with the built-in cartoon, said honestly
        *agent.draw_error.lock().unwrap() = Some("agent offline".into());
        click(&mut a, &cx, T_PIC_REDO);
        until(&mut a, &cx, "the fallback", all_pictures_ready);
        assert_eq!(a.pics[0].how, "cartoon (built-in)");
        assert!(own.drawing_calls().len() == 1 && own.drawing_calls()[0].0[0].content.contains("scene composer"));
        let id = a.saved_id().unwrap();
        let log = std::fs::read_to_string(home.join(format!("Stories/{id}/pictures.log"))).unwrap();
        assert!(log.contains("rich drawing, try 1") && log.contains("DRAW THIS MOMENT") && log.contains("RESULT: used"), "{log}");
    }

    #[test]
    fn a_drawing_the_model_gets_wrong_becomes_a_built_in_sketch_that_says_so_and_the_log_shows_why() {
        let t = Teller::new(STORY);
        *t.svg.lock().unwrap() = "<svg><rect width='10' height='10'/></svg>".into();
        *t.scene.lock().unwrap() = "I would draw a fox".into();
        let (mut a, cx, home) = make("tales-sketch", &t);
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the sketches", all_pictures_ready);
        assert!(a.pics.iter().all(|p| p.how == "built-in sketch" && p.ext == "svg" && p.bytes.as_ref().unwrap().len() > 3000), "{:?}", a.pics.iter().map(|p| &p.how).collect::<Vec<_>>());
        let said: Vec<&str> = a.convo.iter().filter(|m| m.role == iyakki_agent::chat::Role::Tool).map(|m| m.content.as_str()).collect();
        assert!(said.iter().all(|r| !r.contains("an SVG drawing")) && said.iter().any(|r| r.contains("Built-in sketch: the model's drawing could not be used")), "{said:?}");
        let id = a.saved_id().unwrap();
        let log = std::fs::read_to_string(home.join(format!("Stories/{id}/pictures.log"))).unwrap();
        for need in ["I would draw a fox", "not used", "plain drawing", "built-in sketch from the words", "REQUEST", "REPLY"] {
            assert!(log.contains(need), "{need}: {log}");
        }
    }

    #[test]
    fn an_image_from_the_writer_is_kept_as_it_is_with_a_manifest_shown_in_the_slot_and_exported() {
        let png = crate::apps::chatpic::fixtures::png(64, 40, [200, 60, 40]);
        let own = Teller::new(STORY);
        let agent = Teller::new(STORY);
        *agent.svg.lock().unwrap() = format!("Here you are: ![the scene](data:image/png;base64,{})", st::base64_encode(&png));
        let (mut a, cx, home) = make("tales-imgreply", &own);
        let (ta, to) = (agent.clone(), own.clone());
        a.test_drawers = Some(Arc::new(move |_| vec![Drawer { chat: Arc::new(Shared(ta.clone())), strong: true, who: "Claude Code".into() }, Drawer { chat: Arc::new(Shared(to.clone())), strong: false, who: "qwen3:8b".into() }]));
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the pictures", all_pictures_ready);
        assert!(a.pics.iter().all(|p| p.ext == "png" && p.how == "image by Claude Code" && p.bytes.as_ref().unwrap().starts_with(&[0x89, b'P'])), "{:?}", a.pics.iter().map(|p| (&p.ext, &p.how)).collect::<Vec<_>>());
        // drawn in the slot, in the frame of the tile
        let before = a.pic_cache.rasterised;
        let (g, area) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(a.pic_cache.rasterised > before, "the PNG was decoded and drawn");
        assert_layout(&g, area, "story with a png picture");
        // kept beside the story with a manifest: kind, what made it, the prompt and the time
        let id = a.saved_id().unwrap();
        let dir = home.join(format!("Stories/{id}/pics"));
        assert!(dir.join("01.png").is_file() && dir.join("02.png").is_file());
        let manifest = std::fs::read_to_string(dir.join("index.json")).unwrap();
        for need in ["\"kind\": \"image\"", "image by Claude Code", "\"saved_at\"", "\"prompt\""] {
            assert!(manifest.contains(need), "{need}: {manifest}");
        }
        assert!(std::fs::read_to_string(home.join(format!("Stories/{id}/story.md"))).unwrap().contains("(pics/01.png)"));
        // the person's words about one picture work for an image too
        say(&mut a, &cx, "redraw picture 2: make the sky pink");
        until(&mut a, &cx, "again", all_pictures_ready);
        assert!(agent.drawing_calls().last().unwrap().0[1].content.contains("make the sky pink"));
        // export carries the images
        a.prefs.auto_save = false;
        click(&mut a, &cx, T_EXPORT);
        let _ = draw(&mut a, &cx, 1280, 720, 0);
        key(&mut a, &cx, Key::Enter);
        let (_, bytes) = a.take_host_save().expect("export");
        assert_eq!(String::from_utf8(bytes).unwrap().matches("data:image/png;base64,").count(), 2);
    }

    #[test]
    fn a_video_clip_needs_the_option_and_is_kept_and_exported_when_allowed_and_a_link_is_never_fetched() {
        let mp4 = b"\0\0\0\x18ftypmp42\0\0\0\0mp42isom-and-some-more-bytes-of-a-tiny-fake-clip".to_vec();
        let reply = format!("data:video/mp4;base64,{}", st::base64_encode(&mp4));
        let make_app = |tag: &str, allow: bool| {
            let own = Teller::new(STORY);
            let agent = Teller::new(STORY);
            *agent.svg.lock().unwrap() = reply.clone();
            let (mut a, cx, home) = make(tag, &own);
            let (ta, to) = (agent.clone(), own.clone());
            a.test_drawers = Some(Arc::new(move |_| vec![Drawer { chat: Arc::new(Shared(ta.clone())), strong: true, who: "Claude Code".into() }, Drawer { chat: Arc::new(Shared(to.clone())), strong: false, who: "qwen3:8b".into() }]));
            assert!(!a.options().video, "off by default");
            if allow {
                click(&mut a, &cx, T_VIDEO);
                assert!(a.options().video);
            }
            (a, cx, home)
        };
        // off: the clip is refused with the reason and the built-in cartoon is drawn
        let (mut a, cx, _) = make_app("tales-video-off", false);
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the pictures", all_pictures_ready);
        assert!(a.pics.iter().all(|p| p.ext == "svg" && p.how == "cartoon (built-in)"));
        let said: Vec<&str> = a.convo.iter().filter(|m| m.role == iyakki_agent::chat::Role::Tool).map(|m| m.content.as_str()).collect();
        assert!(said.iter().any(|r| r.contains("switched off")), "{said:?}");
        // on: kept as mp4 beside the story, the slot says so, the export plays it
        let (mut b, cx2, home) = make_app("tales-video-on", true);
        write_story(&mut b, &cx2, "a fox");
        until(&mut b, &cx2, "the clips", all_pictures_ready);
        assert!(b.pics.iter().all(|p| p.ext == "mp4" && p.how == "video clip by Claude Code"));
        let (g, area) = draw(&mut b, &cx2, 1280, 720, 0);
        assert_layout(&g, area, "story with a video clip");
        let id = b.saved_id().unwrap();
        assert_eq!(std::fs::read(home.join(format!("Stories/{id}/pics/01.mp4"))).unwrap(), mp4);
        assert!(std::fs::read_to_string(home.join(format!("Stories/{id}/pics/index.json"))).unwrap().contains("\"kind\": \"video\""));
        b.prefs.auto_save = false;
        click(&mut b, &cx2, T_EXPORT);
        let _ = draw(&mut b, &cx2, 1280, 720, 0);
        key(&mut b, &cx2, Key::Enter);
        let html = String::from_utf8(b.take_host_save().unwrap().1).unwrap();
        assert!(html.matches("<video muted loop controls").count() == 2 && html.contains("data:video/mp4;base64,"));
        // a reply that only links to a picture on the web is not fetched; the host is named in the log
        let own = Teller::new(STORY);
        let agent = Teller::new(STORY);
        *agent.svg.lock().unwrap() = "![scene](https://cdn.example.org/x/scene.png)".into();
        let (mut c, cx3, home3) = make("tales-link", &own);
        let (ta, to) = (agent.clone(), own.clone());
        c.test_drawers = Some(Arc::new(move |_| vec![Drawer { chat: Arc::new(Shared(ta.clone())), strong: true, who: "Claude Code".into() }, Drawer { chat: Arc::new(Shared(to.clone())), strong: false, who: "qwen3:8b".into() }]));
        write_story(&mut c, &cx3, "a fox");
        until(&mut c, &cx3, "the pictures", all_pictures_ready);
        assert!(c.pics.iter().all(|p| p.ext == "svg"), "nothing was fetched");
        let id = c.saved_id().unwrap();
        let log = std::fs::read_to_string(home3.join(format!("Stories/{id}/pictures.log"))).unwrap();
        assert!(log.contains("cdn.example.org") && log.contains("not fetched without your approval"), "{log}");
    }

    #[test]
    fn the_drawing_option_cycles_and_shows_in_options_and_on_the_chip() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-drawmode", &t);
        assert_eq!(a.options().drawing, st::DrawingMode::Auto);
        assert!(find_in_form(&mut a, &cx, 1280, 720, "Drawing: Auto").is_some());
        click(&mut a, &cx, T_DRAWMODE);
        assert_eq!(a.options().drawing, st::DrawingMode::Image);
        click(&mut a, &cx, T_DRAWMODE);
        assert_eq!(a.options().drawing, st::DrawingMode::Rich);
        assert!(a.pictures_status(&cx).starts_with("Pictures: rich SVG by"), "{}", a.pictures_status(&cx));
        click(&mut a, &cx, T_DRAWMODE);
        assert_eq!(a.options().drawing, st::DrawingMode::Cartoon);
        assert_eq!(a.pictures_status(&cx), "Pictures: cartoon (built-in)");
        assert_eq!(a.store.load_prefs().options.drawing, st::DrawingMode::Cartoon, "remembered");
        click(&mut a, &cx, T_DRAWMODE);
        assert_eq!(a.options().drawing, st::DrawingMode::Auto);
        // pictures switched off hide the choice
        click(&mut a, &cx, T_PICMODE);
        assert!(find_in_form(&mut a, &cx, 1280, 720, "Drawing: Auto").is_none());
    }

    #[test]
    fn when_automatic_images_fail_the_rest_are_drawn_as_svg_and_it_is_said_once() {
        let t = Teller::new(&STORY.replace("\n\nShe walked", &"\n\nAnother long paragraph about the river, the reeds and the night wind that went on for quite a while.".repeat(8)));
        let (mut a, cx, _) = make("tales-auto-fall", &t);
        add_providers(&cx, &["flux"]);
        write_story(&mut a, &cx, "a fox");
        assert!(a.pics.len() >= 3, "{} scenes", a.pics.len());
        assert!(cx.audit.recent(50).iter().any(|l| l.contains("story.picture") && l.contains("image scene 1")), "images came first");
        until(&mut a, &cx, "every scene done", |a| a.pics.iter().all(|p| p.state == PicState::Ready));
        assert!(!a.pic_auto, "the fall-back happened once");
        assert!(a.pics.iter().all(|p| p.mode == PictureMode::Drawings), "the scenes were drawn as SVG");
        assert_eq!(t.drawing_calls().len(), a.pics.len());
        let note = a.note_text().unwrap_or_default().to_string();
        assert!(note.contains("Image generation did not work") && note.contains("drawn as SVG"), "{note}");
        assert_eq!(note.matches("Image generation").count(), 1, "said once");
        // the conversation says so too: the failed step, then the new steps
        let results: Vec<&str> = a.convo.iter().filter(|m| m.role == iyakki_agent::chat::Role::Tool).map(|m| m.content.as_str()).collect();
        assert!(results.iter().any(|r| r.starts_with("Failed: image generation did not work")), "{results:?}");
        assert!(results.iter().filter(|r| r.contains("cartoon (built-in)")).count() >= 3, "{results:?}");
    }

    // ------------------------------------------------------------------ a conversation: corrections, versions, steering

    /// STORY with the owl named: only the middle paragraph changed.
    const REV: &str = "# The Brave Little Fox\n\nOnce upon a time a small fox lived near the river. She was afraid of the dark.\n\nOne night the moon was hiding. \"Do not worry,\" said a wise owl named Beeru. The fox listened carefully.\n\nShe walked home with a quiet heart, and she slept well.\n";
    /// REV with a new ending: the last paragraph changed.
    const REV2: &str = "# The Brave Little Fox\n\nOnce upon a time a small fox lived near the river. She was afraid of the dark.\n\nOne night the moon was hiding. \"Do not worry,\" said a wise owl named Beeru. The fox listened carefully.\n\nShe walked home with a quiet heart, and she slept soundly until the morning.\n";

    /// Type a message in the panel's box and send it with Enter.
    fn say(a: &mut StoryApp, cx: &AppCx, text: &str) {
        a.set_focus(Focus::Idea);
        a.idea.set(text);
        a.sync_fields();
        key(a, cx, Key::Enter);
    }

    fn all_pictures_ready(a: &mut StoryApp) -> bool {
        a.pics.iter().all(|p| p.state == PicState::Ready)
    }

    // ------------------------------------------------------------------ illustrations by the picture model

    /// A tiny real PNG (8x8).
    fn tiny_png() -> Vec<u8> {
        let hex = "89504e470d0a1a0a0000000d49484452000000080000000808020000004b6d29dc0000001c74455874436f6d6d656e7400736563726574206c6f636174696f6e206461746154e84f540000002a49444154789c63d4883ab1e0510203034382dc026406e3091b0d4c5106060626aca209720b1807d22800fc713f4fb3c21d830000000049454e44ae426082";
        (0..hex.len() / 2).map(|i| u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).unwrap()).collect()
    }

    /// A picture server that is never reached over a network: it answers the image calls with a tiny PNG (or an error), after
    /// an optional delay, and remembers what it was asked.
    struct FakeImages {
        asked: Mutex<Vec<String>>,
        delay_ms: u64,
        fail: bool,
    }

    impl FakeImages {
        fn new(delay_ms: u64, fail: bool) -> Arc<FakeImages> {
            Arc::new(FakeImages { asked: Mutex::new(vec![]), delay_ms, fail })
        }
        fn count(&self) -> usize {
            self.asked.lock().unwrap().len()
        }
    }

    struct ImgUp(Arc<FakeImages>);

    impl iyakki_agent::iyakki_proxy::Upstream for ImgUp {
        fn request(&self, _m: &str, u: &str, _h: &[(String, String)], b: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
            let this = &self.0;
            if !(u.ends_with("/api/generate") || u.ends_with("/images/generations")) {
                return Err("offline".into());
            }
            this.asked.lock().unwrap().push(String::from_utf8_lossy(b).into_owned());
            if this.delay_ms > 0 {
                std::thread::sleep(Duration::from_millis(this.delay_ms));
            }
            if this.fail {
                return Ok(iyakki_agent::iyakki_proxy::UpstreamResponse { status: 500, body: b"{\"error\":\"the picture model fell over\"}".to_vec() });
            }
            let body = serde_json::json!({ "images": [st::base64_encode(&tiny_png())] }).to_string().into_bytes();
            Ok(iyakki_agent::iyakki_proxy::UpstreamResponse { status: 200, body })
        }
    }

    fn with_images(tag: &str, delay_ms: u64, fail: bool) -> (StoryApp, AppCx, PathBuf, Arc<Teller>, Arc<FakeImages>) {
        let t = Teller::new(STORY);
        let fake = FakeImages::new(delay_ms, fail);
        let (mut a, cx, home) = make_up(tag, &t, Arc::new(ImgUp(fake.clone())));
        add_providers(&cx, &["llama3", "flux"]);
        a.refresh_tts(&cx);
        (a, cx, home, t, fake)
    }

    fn lots_of_memory() -> iyakki_agent::sysinfo::Memory {
        iyakki_agent::sysinfo::Memory { total: 32 << 30, available: 24 << 30, swap_total: 0, swap_free: 0 }
    }

    #[test]
    fn tales_illustrations_are_offered_with_a_plain_reason_and_no_error_when_there_is_no_picture_model() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-illus-none", &t);
        assert_eq!(a.readiness(&cx), Readiness::NoModel);
        let why = illus::reason(&a.readiness(&cx)).unwrap();
        assert!(why.contains("No picture model is selected") && why.contains("Settings") && why.contains("Models"), "{why}");
        // the option is on the page (it cycles) and the story is written with drawings and no error
        assert_eq!(find_in_form(&mut a, &cx, 1920, 1080, "Pictures: every chapter").as_deref(), Some("Pictures: every chapter"));
        write_story(&mut a, &cx, "a fox");
        assert!(a.error.is_none() && !a.story.is_empty());
        assert!(a.pics.iter().all(|p| p.mode == PictureMode::Drawings), "drawings, not the picture model");
        // a Lemonade picture model that is not downloaded is offered as Install first, not as a failure
        let mut l = iyakki_agent::providers::ProviderCfg::new("lemon", iyakki_agent::providers::Kind::Lemonade);
        l.base_url = Some("http://lemonade-tales-test.invalid:8000".into());
        cx.services.settings.lock().unwrap().upsert_provider(l).unwrap();
        let m = iyakki_agent::lemonade::Model { id: "SDXL-Base-1.0".into(), kind: iyakki_agent::lemonade::ModelType::Image, recipe: "sd-cpp".into(), checkpoint: String::new(), labels: vec!["image".into()], downloaded: false, loaded: false, suggested: false, size_gb: Some(6.9), image_defaults: None };
        iyakki_agent::lemonade::remember(&iyakki_agent::lemonade::api_base("http://lemonade-tales-test.invalid:8000"), &[m]);
        let r = a.readiness(&cx);
        assert_eq!(r, Readiness::NotDownloaded { model: "SDXL-Base-1.0".into(), size: "6.9 GB".into() });
        assert!(illus::reason(&r).unwrap().contains("Install first"));
    }

    #[test]
    fn tales_illustrations_cover_only_every_chapter_and_ask_me_each_time_with_the_prompt_notes_and_metadata_kept() {
        // every chapter (the default): a picture per scene by the picture model
        let (mut a, cx, _home, t, fake) = with_images("tales-illus-all", 0, false);
        a.test_memory = Some(lots_of_memory());
        assert_eq!(a.opts.coverage, Coverage::Chapters);
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the pictures", all_pictures_ready);
        assert_eq!(a.pics.len(), 2);
        assert_eq!(fake.count(), 2, "one request per picture, one at a time");
        let asked = fake.asked.lock().unwrap().join("\n");
        assert!(asked.contains(PictureStyle::Watercolor.phrase()), "the one style line is in every prompt: {asked}");
        assert!(asked.contains("A small fox sits by a quiet river") && asked.contains("Setting: a river meadow at dusk"), "the brief from the story model and the setting note: {asked}");
        assert_eq!(a.setting.as_deref(), Some("a river meadow at dusk, soft blue light"), "the note is made once per story");
        assert_eq!(t.seen.lock().unwrap().iter().filter(|c| c.0[0].content.contains("where and when a story happens")).count(), 1);
        assert!(a.pics.iter().all(|p| p.size == "8x8" && p.how == "images via flux"));
        // kept with the story: the file, its prompt, the model and the size, and the setting note
        click(&mut a, &cx, T_SAVE);
        let id = a.saved_id().unwrap();
        let kept = a.store.load_pictures(&id);
        assert_eq!(kept.len(), 2);
        assert_eq!((kept[0].size.as_str(), kept[0].source.as_str(), kept[0].mode.as_str()), ("8x8", "images via flux", "image"));
        assert!(kept[0].prompt.contains("river meadow") && kept[0].bytes.starts_with(&[0x89, b'P', b'N', b'G']));
        assert_eq!(a.store.load_setting(&id).as_deref(), Some("a river meadow at dusk, soft blue light"));
        // cover only: exactly one picture
        let (mut b, cx2, _h, _t, fake2) = with_images("tales-illus-cover", 0, false);
        b.test_memory = Some(lots_of_memory());
        click(&mut b, &cx2, T_COVERAGE);
        assert_eq!(b.opts.coverage, Coverage::Cover);
        assert_eq!(b.store.load_prefs().options.coverage, Coverage::Cover, "remembered");
        write_story(&mut b, &cx2, "a fox");
        until(&mut b, &cx2, "the cover", all_pictures_ready);
        assert_eq!((b.pics.len(), fake2.count()), (1, 1));
        // ask me each time: nothing is drawn until the person says so; the text is there meanwhile
        let (mut c, cx3, _h, _t, fake3) = with_images("tales-illus-ask", 0, false);
        c.test_memory = Some(lots_of_memory());
        click(&mut c, &cx3, T_COVERAGE);
        click(&mut c, &cx3, T_COVERAGE);
        assert_eq!(c.opts.coverage, Coverage::Ask);
        write_story(&mut c, &cx3, "a fox");
        assert!(c.held && !c.story.is_empty() && fake3.count() == 0 && c.pics.len() == 2);
        for _ in 0..20 {
            c.handle(&Event::Tick, &cx3);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(fake3.count(), 0, "still waiting for the yes");
        let (g, _) = draw(&mut c, &cx3, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Make the pictures") && g.hits.iter().any(|h| h.label == "No pictures"));
        click(&mut c, &cx3, T_PICS_GO);
        until(&mut c, &cx3, "the pictures after the yes", all_pictures_ready);
        assert_eq!(fake3.count(), 2);
        // and a no keeps the text and draws nothing
        let (mut d, cx4, _h, _t, fake4) = with_images("tales-illus-no", 0, false);
        click(&mut d, &cx4, T_COVERAGE);
        click(&mut d, &cx4, T_COVERAGE);
        write_story(&mut d, &cx4, "a fox");
        let words = d.story.words();
        click(&mut d, &cx4, T_PICS_SKIP);
        assert!(!d.held && d.pics.is_empty() && fake4.count() == 0 && d.story.words() == words);
    }


    // ------------------------------------------------------------------ picture look, quality, model, seed and the picture log

    fn img_model(id: &str, gb: f64, w: u32, steps: u32) -> iyakki_agent::lemonade::Model {
        iyakki_agent::lemonade::Model { id: id.into(), kind: iyakki_agent::lemonade::ModelType::Image, recipe: "sd-cpp".into(), checkpoint: String::new(), labels: vec!["image".into()], downloaded: true, loaded: false, suggested: false, size_gb: Some(gb), image_defaults: Some(iyakki_agent::lemonade::ImageDefaults { width: Some(w), height: Some(w), steps: Some(steps) }) }
    }

    /// A Lemonade server on this computer with SD-Turbo and SDXL, and one on another computer with FLUX.
    fn with_two_lemonades(tag: &str, delay_ms: u64, fail: bool) -> (StoryApp, AppCx, PathBuf, Arc<FakeImages>) {
        let t = Teller::new(STORY);
        let fake = FakeImages::new(delay_ms, fail);
        let (mut a, cx, home) = make_up(tag, &t, Arc::new(ImgUp(fake.clone())));
        let (local, remote) = (format!("http://127.0.0.1:{}", 20000 + hash(tag) % 20000), format!("http://{tag}.gpu-box.invalid:13305"));
        for (id, base) in [("home", &local), ("gpu", &remote)] {
            let mut l = iyakki_agent::providers::ProviderCfg::new(id, iyakki_agent::providers::Kind::Lemonade);
            l.base_url = Some(base.clone());
            cx.services.settings.lock().unwrap().upsert_provider(l).unwrap();
        }
        iyakki_agent::lemonade::remember(&iyakki_agent::lemonade::api_base(&local), &[img_model("SD-Turbo", 2.0, 512, 4), img_model("SDXL-Base-1.0", 6.9, 1024, 20)]);
        iyakki_agent::lemonade::remember(&iyakki_agent::lemonade::api_base(&remote), &[img_model("FLUX-Dev", 23.0, 1024, 28)]);
        a.refresh_tts(&cx);
        a.test_memory = Some(lots_of_memory());
        a.confirm_secs = f64::MAX;
        (a, cx, home, fake)
    }

    fn bodies(f: &FakeImages) -> Vec<serde_json::Value> {
        f.asked.lock().unwrap().iter().map(|b| serde_json::from_str(b).unwrap()).collect()
    }

    #[test]
    fn tales_picture_style_quality_model_and_seed_are_options_kept_per_story_and_as_default() {
        let (mut a, cx, _home, fake) = with_two_lemonades("tales-pic-opts", 0, false);
        assert_eq!(cx.services.gpu_lemonade().as_deref(), Some("gpu"), "a Lemonade server that is not this computer is the GPU one");
        assert_eq!(a.pic_now(), illus::PictureOpts::default(), "the old defaults");
        // the chips are on the page, with the provider named, the model's own size and steps and the plain advice
        let l = find_in_form(&mut a, &cx, 1920, 1080, "Style \u{2611}: storybook illustration").unwrap();
        assert!(!l.is_empty());
        for want in ["Quality: as selected", "Model for this story: automatic", "Same look for all pictures: off", "Picture log (g)"] {
            assert!(find_in_form(&mut a, &cx, 1920, 1080, want).is_some(), "{want}");
        }
        let (g, _) = draw(&mut a, &cx, 1920, 1080, 1);
        assert!(g.hits.len() > 5);
        // the styles are ticked one by one and remembered as the default; the last one cannot be unticked
        click(&mut a, &cx, T_STYLE + 1);
        assert_eq!(a.opts.pic.styles_list(), vec![illus::Look::Storybook, illus::Look::Watercolor]);
        click(&mut a, &cx, T_STYLE);
        click(&mut a, &cx, T_STYLE + 1);
        assert_eq!(a.opts.pic.styles_list(), vec![illus::Look::Watercolor], "the last style stays ticked");
        click(&mut a, &cx, T_REALISTIC);
        assert_eq!(a.opts.pic.styles_list(), vec![illus::Look::Photo], "one style ticked: Realistic replaces it");
        click(&mut a, &cx, T_REALISTIC);
        assert_eq!(a.opts.pic.styles_list(), vec![illus::Look::Watercolor], "unticking brings the styles back");
        click(&mut a, &cx, T_STYLE + 2);
        click(&mut a, &cx, T_STYLE + 1);
        assert_eq!(a.opts.pic.styles_list(), vec![illus::Look::Photo]);
        assert_eq!(a.store.load_prefs().options.pic.styles_list(), vec![illus::Look::Photo]);
        click(&mut a, &cx, T_QUALITY);
        click(&mut a, &cx, T_QUALITY);
        click(&mut a, &cx, T_QUALITY);
        assert_eq!(a.opts.pic.quality, illus::Quality::Best);
        let r = a.resolved_for(&a.pic_now(), &cx).unwrap();
        assert_eq!((r.provider.as_str(), r.model.as_str(), r.steps, r.width), ("gpu", "FLUX-Dev", Some(35), 1024), "Best picks the largest model, here on the other computer, with more steps");
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the pictures", all_pictures_ready);
        let sent = bodies(&fake);
        assert_eq!(sent.len(), 2);
        for b in &sent {
            assert_eq!((b["model"].as_str(), b["steps"].as_u64(), b["size"].as_str()), (Some("FLUX-Dev"), Some(35), Some("1024x1024")), "the steps of Quality Best inside what the model has");
            assert_eq!(b["cfg_scale"].as_f64(), Some(6.0), "photo style asks a model that is not Turbo for guidance");
            let prompt = b["prompt"].as_str().unwrap();
            assert!(prompt.starts_with("a realistic photograph, shot on a 50mm lens") && prompt.contains("A small fox sits by a quiet river") && prompt.chars().count() <= illus::Family::Long.cap(), "{prompt}");
            assert!(prompt.contains("Avoid: cartoon, illustration") && b.get("negative_prompt").is_none(), "the images API is not known to take a negative prompt: {prompt}");
        }
        assert_ne!(sent[0]["seed"], sent[1]["seed"], "a new random seed for every picture");
        // saved with the story: its own file, back when it is opened, whatever the defaults are now
        click(&mut a, &cx, T_SAVE);
        let id = a.saved_id().unwrap();
        let kept = a.store.load_picture_opts(&id).unwrap();
        assert_eq!((kept.styles_list(), kept.quality), (vec![illus::Look::Photo], illus::Quality::Best));
        click(&mut a, &cx, T_STYLE + 3);
        click(&mut a, &cx, T_QUALITY);
        assert!(a.story_opts.pic.styles_list().contains(&illus::Look::Cinematic) && a.store.load_picture_opts(&id).unwrap().styles_list().contains(&illus::Look::Cinematic), "changing it on the page changes the story's choice too");
        a.opts.pic = illus::PictureOpts::default();
        let e = a.store.list().into_iter().find(|e| e.id == id).unwrap();
        assert!(a.options_of(&e).pic.styles_list().contains(&illus::Look::Cinematic), "opened later, the story keeps its own styles");
        // a model for this story: a model of any provider, named with its provider; wins over Quality
        let (mut b, cx2, _h, fake2) = with_two_lemonades("tales-pic-model", 0, false);
        click(&mut b, &cx2, T_QUALITY);
        assert_eq!(b.resolved_for(&b.pic_now(), &cx2).unwrap().model, "SD-Turbo");
        click(&mut b, &cx2, T_STORYMODEL);
        let first = b.opts.pic.model.clone().unwrap();
        assert!(b.found_images(&cx2).contains(&(first.provider.clone(), first.model.clone())));
        while b.opts.pic.model.as_ref().map(|m| m.model.as_str()) != Some("FLUX-Dev") {
            click(&mut b, &cx2, T_STORYMODEL);
        }
        assert_eq!(find_in_form(&mut b, &cx2, 1920, 1080, "Model for this story: FLUX-Dev \u{b7} gpu").is_some(), true, "the provider is shown");
        assert_eq!(b.resolved_for(&b.pic_now(), &cx2).unwrap().why, "chosen for this story");
        click(&mut b, &cx2, T_STORYMODEL);
        assert!(b.opts.pic.model.is_none(), "after the last model it is automatic again");
        b.opts.pic.model = Some(ModelChoice { provider: "gpu".into(), model: "FLUX-Dev".into() });
        write_story(&mut b, &cx2, "a fox");
        until(&mut b, &cx2, "the pictures", all_pictures_ready);
        assert!(bodies(&fake2).iter().all(|x| x["model"] == "FLUX-Dev"));
        // keep the same look: one seed for the whole story, kept with it
        let (mut c, cx3, _h, fake3) = with_two_lemonades("tales-pic-seed", 0, false);
        click(&mut c, &cx3, T_KEEPLOOK);
        assert!(c.opts.pic.keep_look && c.opts.pic.seed.is_none());
        write_story(&mut c, &cx3, "a fox");
        until(&mut c, &cx3, "the pictures", all_pictures_ready);
        let seeds: Vec<_> = bodies(&fake3).iter().map(|x| x["seed"].clone()).collect();
        assert!(seeds.len() == 2 && seeds[0] == seeds[1] && seeds[0].is_u64(), "{seeds:?}");
        let id = c.saved_id().unwrap();
        assert_eq!(c.store.load_picture_opts(&id).unwrap().seed, seeds[0].as_u64(), "the story keeps its seed");
        assert!(c.opts.pic.seed.is_none(), "the default never holds a seed");
    }

    #[test]
    fn tales_picture_log_keeps_every_request_skip_and_fallback_in_order_without_secrets() {
        let (mut a, cx, _home, _fake) = with_two_lemonades("tales-piclog", 0, false);
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the pictures", all_pictures_ready);
        let id = a.saved_id().unwrap();
        let ev = a.store.load_picture_log(&id);
        assert_eq!(ev.len(), 2, "{ev:?}");
        let e = &ev[0];
        assert_eq!((e.kind.as_str(), e.story.as_str(), e.picture, e.attempt), ("request", "The Brave Little Fox", 1, 1));
        assert_eq!((e.provider.as_str(), e.model.as_str(), e.size.as_str(), e.steps, e.status, e.format.as_str()), ("home", "SD-Turbo", "512x512", Some(4), Some(200), "png"), "{e:?}");
        assert!(e.seed.is_some() && e.bytes > 0 && !e.prompt.is_empty() && e.negative.contains("extra fingers") && e.verbose.is_none(), "verbose is off by default: {e:?}");
        assert_eq!(e.negative_sent, "not sent", "no room for the avoid-words in a short SD-Turbo prompt");
        assert!(e.prompt.chars().count() <= illus::Family::Turbo.cap(), "SD-Turbo gets a short prompt: {}", e.prompt);
        assert_eq!(ev[1].picture, 2);
        // Try again is a new attempt of the same picture, logged after the others
        click(&mut a, &cx, T_PIC_REDO);
        until(&mut a, &cx, "the redraw", |a| a.pics[0].state == PicState::Ready && a.store.load_picture_log(&id).len() == 3);
        assert_eq!(a.store.load_picture_log(&id)[2].attempt, 2);
        // the view: opened by the button and by g, newest at the bottom, copy gives the whole text
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        click_hit(&mut a, &cx, &g, "Picture log (g)");
        assert!(matches!(a.modal, Some(Modal::PicLog(_))));
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        for l in ["Copy log", "Clear log", "Verbose: off", "Close"] {
            assert!(g.hits.iter().any(|h| h.label == l), "{l}");
        }
        click(&mut a, &cx, T_LOG_COPY);
        let all = piclog::log_text(&a.pic_log_events());
        assert_eq!(crate::ui::clipboard::latest().as_deref(), Some(all.as_str()), "Copy log copies the whole text");
        assert!(all.contains("SD-Turbo on home") && all.contains("HTTP 200") && all.contains("attempt 2 (retries 1)"));
        // verbose is a switch, off by default, kept
        click(&mut a, &cx, T_LOG_VERBOSE);
        assert!(a.pic_now().verbose && a.store.load_picture_opts(&id).unwrap().verbose);
        click(&mut a, &cx, T_LOG_VERBOSE);
        assert!(!a.pic_now().verbose);
        // scrolling: many events, the latest at the bottom, keys and the wheel move it, it stays inside the log
        let many: Vec<PicEvent> = (0..80).map(|n| piclog::decision_event(1_792_000_000 + n, plk::SKIPPED, "S", 1, "A", &format!("skipped number {n}"))).collect();
        a.log_pic_events(many);
        let (_g, _) = draw(&mut a, &cx, 1280, 720, 0);
        let (total, rows, top) = if let Some(Modal::PicLog(v)) = &a.modal { (v.total.get(), v.rows.get(), v.top.get()) } else { unreachable!() };
        assert!(total > rows * 2 && top == total - rows, "it opens at the latest events: {total} {rows} {top}");
        key(&mut a, &cx, Key::Up);
        key(&mut a, &cx, Key::PageUp);
        let now = if let Some(Modal::PicLog(v)) = &a.modal { v.top.get() } else { 0 };
        assert_eq!(now, top - 1 - (rows - 1));
        a.handle(&Event::Scroll(-2), &cx);
        let up = if let Some(Modal::PicLog(v)) = &a.modal { v.top.get() } else { 0 };
        assert_eq!(up, now + 6, "the wheel scrolls the log (down), not the story");
        key(&mut a, &cx, Key::Home);
        let (_g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert_eq!(if let Some(Modal::PicLog(v)) = &a.modal { v.top.get() } else { 9 }, 0);
        key(&mut a, &cx, Key::End);
        key(&mut a, &cx, Key::PageDown);
        assert_eq!(if let Some(Modal::PicLog(v)) = &a.modal { v.top.get() } else { 0 }, total - rows);
        // clear asks twice, then the log is empty on disk too
        click(&mut a, &cx, T_LOG_CLEAR);
        assert!(!a.store.load_picture_log(&id).is_empty(), "one press only asks");
        click(&mut a, &cx, T_LOG_CLEAR);
        assert!(a.store.load_picture_log(&id).is_empty() && a.pic_log_events().is_empty());
        key(&mut a, &cx, Key::Esc);
        assert!(a.modal.is_none());
        key(&mut a, &cx, Key::Char('g'));
        assert!(matches!(a.modal, Some(Modal::PicLog(_))), "g opens it");
    }

    #[test]
    fn tales_picture_log_names_the_server_error_the_svg_fallback_the_memory_skip_and_the_cancel() {
        // the picture model fails: its status and text are logged, and the rest of the story falls back to SVG, said once
        let (mut a, cx, _home, _fake) = with_two_lemonades("tales-piclog-fail", 0, true);
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the drawings", all_pictures_ready);
        let id = a.saved_id().unwrap();
        let ev = a.store.load_picture_log(&id);
        let req = ev.iter().find(|e| e.kind == plk::REQUEST).unwrap();
        assert_eq!(req.status, Some(500));
        assert!(req.error.contains("fell over") && req.format.is_empty(), "{req:?}");
        let fb = ev.iter().find(|e| e.kind == plk::SVG_FALLBACK).unwrap();
        assert!(fb.svg_fallback.contains("fell over"), "{fb:?}");
        assert!(ev.iter().position(|e| e.kind == plk::REQUEST) < ev.iter().position(|e| e.kind == plk::SVG_FALLBACK), "in order");
        // low memory: nothing is asked, and the decision is logged
        let (mut b, cx2, _h, fake2) = with_two_lemonades("tales-piclog-mem", 0, false);
        b.test_memory = Some(iyakki_agent::sysinfo::Memory { total: 8 << 30, available: 200 << 20, swap_total: 0, swap_free: 0 });
        write_story(&mut b, &cx2, "a fox");
        until(&mut b, &cx2, "the pictures to wait", |b| b.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
        assert_eq!(fake2.count(), 0);
        let ev = b.pic_log_events();
        assert!(ev.iter().any(|e| e.kind == plk::SKIPPED && e.decision.starts_with("skipped: free memory low") && e.decision.contains("200 MB free")), "{ev:?}");
        // cancelled while being drawn: the late answer is still logged
        let (mut c, cx3, _h, fake3) = with_two_lemonades("tales-piclog-cancel", 300, false);
        write_story(&mut c, &cx3, "a fox");
        until(&mut c, &cx3, "the first request", |_| fake3.count() >= 1);
        click(&mut c, &cx3, T_PICS_CANCEL);
        assert!(c.pic_log_events().iter().any(|e| e.decision.starts_with("cancelled")));
        until(&mut c, &cx3, "the late answer in the log", |c| c.pic_log_events().iter().any(|e| e.kind == plk::REQUEST));
        // a story that is not saved yet keeps the events in memory and writes them when it is saved
        let (mut d, cx4, _h, _f) = with_two_lemonades("tales-piclog-unsaved", 0, false);
        d.prefs.auto_save = false;
        write_story(&mut d, &cx4, "a fox");
        until(&mut d, &cx4, "the pictures", all_pictures_ready);
        assert!(d.saved_id().is_none() && d.pic_pending.len() == 2);
        click(&mut d, &cx4, T_SAVE);
        let id = d.saved_id().unwrap();
        assert!(d.pic_pending.is_empty() && d.store.load_picture_log(&id).len() == 2);
    }


    /// A Lemonade server that is never reached over a network: its model list says whether SDXL is downloaded (a download
    /// makes it so), its health lists a busy model, and its picture call is slow when asked to be.
    struct LiveUp {
        sdxl_downloaded: std::sync::atomic::AtomicBool,
        delay_ms: u64,
        gens: AtomicUsize,
        health_calls: AtomicUsize,
        pulls: AtomicUsize,
    }

    impl LiveUp {
        fn new(downloaded: bool, delay_ms: u64) -> Arc<LiveUp> {
            Arc::new(LiveUp { sdxl_downloaded: std::sync::atomic::AtomicBool::new(downloaded), delay_ms, gens: AtomicUsize::new(0), health_calls: AtomicUsize::new(0), pulls: AtomicUsize::new(0) })
        }
    }

    struct LiveNet(Arc<LiveUp>);

    impl iyakki_agent::iyakki_proxy::Upstream for LiveNet {
        fn request(&self, _m: &str, u: &str, _h: &[(String, String)], _b: &[u8]) -> Result<iyakki_agent::iyakki_proxy::UpstreamResponse, String> {
            let t = &self.0;
            let ok = |v: serde_json::Value| Ok(iyakki_agent::iyakki_proxy::UpstreamResponse { status: 200, body: v.to_string().into_bytes() });
            if u.ends_with("/models?show_all=true") {
                let dl = t.sdxl_downloaded.load(Ordering::SeqCst);
                return ok(serde_json::json!({"data": [
                    {"id": "SD-Turbo", "recipe": "sd-cpp", "labels": ["image"], "size": 2.0, "downloaded": true, "image_defaults": {"steps": 4, "width": 512, "height": 512}},
                    {"id": "SDXL-Base-1.0", "recipe": "sd-cpp", "labels": ["image"], "size": 6.94, "downloaded": dl, "image_defaults": {"steps": 20, "width": 1024, "height": 1024}}
                ]}));
            }
            if u.ends_with("/health") {
                t.health_calls.fetch_add(1, Ordering::SeqCst);
                return ok(serde_json::json!({"status": "ok", "all_models_loaded": [{"model_name": "kokoro-v1"}]}));
            }
            if u.ends_with("/images/generations") {
                t.gens.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(Duration::from_millis(t.delay_ms));
                return ok(serde_json::json!({ "data": [{"b64_json": st::base64_encode(&tiny_png())}] }));
            }
            Err("offline".into())
        }
    }

    impl iyakki_agent::iyakki_proxy::Streamer for LiveNet {
        fn open(&self, _u: &str, _h: &[(String, String)]) -> Result<iyakki_agent::iyakki_proxy::StreamHead, String> {
            Err("not used".into())
        }
        fn post(&self, _u: &str, _h: &[(String, String)], _b: &[u8]) -> Result<iyakki_agent::iyakki_proxy::StreamHead, String> {
            self.0.pulls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(100));
            self.0.sdxl_downloaded.store(true, Ordering::SeqCst);
            let body = "data: {\"bytes_downloaded\": 100, \"bytes_total\": 200, \"file\": \"sdxl.gguf\"}\nevent: complete\ndata: {}\n";
            Ok(iyakki_agent::iyakki_proxy::StreamHead { status: 200, headers: vec![], body: Box::new(std::io::Cursor::new(body.as_bytes().to_vec())) })
        }
    }

    /// An app whose only Lemonade server says SDXL is downloaded in its saved list (stale) and the live truth is `downloaded`;
    /// SDXL is chosen for the story.
    fn with_live_lemonade(tag: &str, downloaded: bool, delay_ms: u64) -> (StoryApp, AppCx, Arc<LiveUp>) {
        let t = Teller::new(STORY);
        let live = LiveUp::new(downloaded, delay_ms);
        let (mut a, cx, _home) = make_up(tag, &t, Arc::new(LiveNet(live.clone())));
        cx.services.set_streamer(Arc::new(LiveNet(live.clone())));
        let base = format!("http://127.0.0.1:{}", 20000 + hash(tag) % 20000);
        let mut l = iyakki_agent::providers::ProviderCfg::new("home", iyakki_agent::providers::Kind::Lemonade);
        l.base_url = Some(base.clone());
        cx.services.settings.lock().unwrap().upsert_provider(l).unwrap();
        iyakki_agent::lemonade::remember(&iyakki_agent::lemonade::api_base(&base), &[img_model("SD-Turbo", 2.0, 512, 4), img_model("SDXL-Base-1.0", 6.94, 1024, 20)]);
        a.refresh_tts(&cx);
        a.test_memory = Some(lots_of_memory());
        a.confirm_secs = f64::MAX;
        a.opts.pic.model = Some(ModelChoice { provider: "home".into(), model: "SDXL-Base-1.0".into() });
        (a, cx, live)
    }

    #[test]
    fn tales_picture_that_is_not_downloaded_is_not_called_and_offers_install() {
        let (mut a, cx, live) = with_live_lemonade("tales-notdl", false, 0);
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the pictures to stop", |a| a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))) && a.install.is_some());
        assert_eq!(live.gens.load(Ordering::SeqCst), 0, "the picture call was never made");
        let PicState::Failed(why) = a.pics[0].state.clone() else { unreachable!() };
        assert!(why.contains("SDXL-Base-1.0 is not downloaded on home (6.9 GB)"), "{why}");
        assert!(a.pics.iter().all(|p| p.mode == PictureMode::ImageModel), "no fall back to SVG for this");
        let ev = a.pic_log_events();
        assert!(ev.iter().any(|e| e.kind == plk::NOT_DOWNLOADED && e.model == "SDXL-Base-1.0" && e.decision.contains("Nothing was asked")), "{ev:?}");
        assert!(!ev.iter().any(|e| e.kind == plk::REQUEST || e.kind == plk::SVG_FALLBACK));
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Install SDXL-Base-1.0 (6.9 GB)"), "{:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        // the story's model list says which models are installed, and their size
        let list = cx.services.picture_choices(&a.found_images(&cx)).iter().map(|c| c.line()).collect::<Vec<_>>().join("|");
        assert!(list.contains("SD-Turbo on home: installed, 2.0 GB, 512x512, 4 steps") && list.contains("SDXL-Base-1.0 on home: not downloaded, 6.9 GB, 1024x1024, 20 steps"), "{list}");
        assert!(find_in_form(&mut a, &cx, 1920, 1080, "Model for this story: SDXL-Base-1.0 \u{b7} home (not downloaded, 6.9 GB)").is_some());
    }

    #[test]
    fn tales_picture_install_pulls_with_progress_and_then_draws_the_waiting_pictures() {
        let (mut a, cx, live) = with_live_lemonade("tales-pull", false, 0);
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the offer", |a| a.install.is_some() && a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
        click(&mut a, &cx, T_PIC_INSTALL);
        assert!(a.install.as_ref().is_some_and(|i| i.started));
        until(&mut a, &cx, "the pictures after the download", all_pictures_ready);
        assert_eq!(live.pulls.load(Ordering::SeqCst), 1);
        assert_eq!(live.gens.load(Ordering::SeqCst), 2, "drawn after the pull, one call each");
        assert!(a.install.is_none());
        let ev = a.pic_log_events();
        let at = |f: &dyn Fn(&PicEvent) -> bool| ev.iter().position(|e| f(e)).unwrap_or_else(|| panic!("missing in {ev:?}"));
        let (nd, start, fin, req) = (at(&|e| e.kind == plk::NOT_DOWNLOADED), at(&|e| e.decision.starts_with("pull started")), at(&|e| e.decision == "pull finished"), at(&|e| e.kind == plk::REQUEST));
        assert!(nd < start && start < fin && fin < req, "in order: {nd} {start} {fin} {req}");
        assert!(ev.iter().filter(|e| e.kind == plk::REQUEST).all(|e| e.model == "SDXL-Base-1.0" && e.status == Some(200)), "{ev:?}");
    }

    #[test]
    fn tales_picture_that_is_slow_shows_elapsed_time_and_the_server_state_and_logs_the_wait() {
        let (mut a, cx, live) = with_live_lemonade("tales-slow", true, 900);
        a.test_timing = Some((Duration::from_secs(60), Duration::from_millis(40), Duration::from_millis(100)));
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "a wait line with the server state", |a| a.wait_text.as_ref().is_some_and(|n| n.contains("so far") && n.contains("server: loaded models: kokoro-v1")));
        let n = a.wait_text.clone().unwrap();
        assert!(n.contains("Drawing picture 1") && n.contains("The model is loading or generating; on a CPU this can take minutes") && n.contains("SDXL-Base-1.0 is not loaded yet") && n.contains("Cancel pictures"), "{n}");
        assert!(live.health_calls.load(Ordering::SeqCst) >= 1, "the health is polled while it waits");
        until(&mut a, &cx, "the pictures", all_pictures_ready);
        let ev = a.pic_log_events();
        let w = ev.iter().find(|e| e.kind == plk::WAITING).unwrap_or_else(|| panic!("{ev:?}"));
        assert!(w.decision.starts_with("waiting") && w.decision.contains("kokoro-v1") && w.model == "SDXL-Base-1.0", "{w:?}");
        a.handle(&Event::Tick, &cx);
        assert!(a.wait_text.is_none(), "the wait line goes when the picture is done");
    }

    #[test]
    fn tales_picture_stops_after_the_time_limit_says_so_and_logs_it() {
        let (mut a, cx, live) = with_live_lemonade("tales-limit", true, 1200);
        a.test_timing = Some((Duration::from_millis(300), Duration::from_millis(40), Duration::from_secs(30)));
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the time limit", |a| a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
        let PicState::Failed(why) = a.pics[0].state.clone() else { unreachable!() };
        assert!(why.starts_with("Stopped after") && why.contains("time limit") && why.contains("may still be working"), "{why}");
        assert_eq!(live.gens.load(Ordering::SeqCst), 1, "the waiting picture was not started");
        let ev = a.pic_log_events();
        assert!(ev.iter().any(|e| e.kind == plk::TIMEOUT && e.decision.starts_with("timeout after")), "{ev:?}");
        // the late answer is only logged: the picture stays failed
        until(&mut a, &cx, "the late answer in the log", |a| a.pic_log_events().iter().any(|e| e.kind == plk::REQUEST));
        assert!(a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
        // the limit is an option, kept
        click(&mut a, &cx, T_TIMEOUT);
        assert_eq!((a.opts.pic.limit_min(), a.story_opts.pic.limit_min()), (30, 30));
        assert_eq!(illus::PictureOpts::default().limit_min(), 15, "15 minutes unless chosen");
    }

    #[test]
    fn tales_picture_memory_guard_names_the_remedy_and_does_not_apply_to_another_computer() {
        let (mut a, cx, live) = with_live_lemonade("tales-ram", true, 0);
        a.test_memory = Some(iyakki_agent::sysinfo::Memory { total: 16 << 30, available: 3 << 30, swap_total: 0, swap_free: 0 });
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the wait for memory", |a| a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
        let PicState::Failed(why) = a.pics[0].state.clone() else { unreachable!() };
        assert!(why.contains("3072 MB free") && why.contains("SD-Turbo-GGUF") && why.contains("graphics card"), "{why}");
        assert_eq!(live.gens.load(Ordering::SeqCst), 0, "nothing was asked while memory is that low");
    }


    /// One Lemonade server per entry of `servers`: (provider id, on another computer?, its picture models as (name, GB, size, steps,
    /// downloaded)).
    fn with_servers(tag: &str, servers: &[(&str, bool, &[(&str, f64, u32, u32, bool)])]) -> (StoryApp, AppCx, Arc<FakeImages>) {
        let t = Teller::new(STORY);
        let fake = FakeImages::new(0, false);
        let (mut a, cx, _home) = make_up(tag, &t, Arc::new(ImgUp(fake.clone())));
        for (id, remote, models) in servers {
            let base = if *remote { format!("http://{tag}-{id}.gpu-box.invalid:13305") } else { format!("http://127.0.0.1:{}", 20000 + hash(&format!("{tag}{id}")) % 20000) };
            let mut l = iyakki_agent::providers::ProviderCfg::new(id, iyakki_agent::providers::Kind::Lemonade);
            l.base_url = Some(base.clone());
            cx.services.settings.lock().unwrap().upsert_provider(l).unwrap();
            let list: Vec<_> = models.iter().map(|(m, gb, w, steps, dl)| iyakki_agent::lemonade::Model { downloaded: *dl, ..img_model(m, *gb, *w, *steps) }).collect();
            iyakki_agent::lemonade::remember(&iyakki_agent::lemonade::api_base(&base), &list);
        }
        a.refresh_tts(&cx);
        a.test_memory = Some(lots_of_memory());
        a.confirm_secs = f64::MAX;
        (a, cx, fake)
    }

    #[test]
    fn tales_picture_realistic_checkbox_with_one_base_model_uses_photo_words_guidance_and_quality_steps() {
        // only SDXL Base is installed (on the GPU server): the checkbox works with it
        let (mut a, cx, fake) = with_servers("tales-real-one", &[("gpu", true, &[("SDXL-Base-1.0", 6.9, 1024, 20, true)])]);
        assert!(!a.pic_now().is_realistic(), "off by default");
        assert!(find_in_form(&mut a, &cx, 1920, 1080, "\u{2610} Realistic pictures (photo style, people look real)").is_some(), "the checkbox is the first thing of the Pictures options");
        click(&mut a, &cx, T_REALISTIC);
        assert!(a.opts.pic.is_realistic() && a.store.load_prefs().options.pic.is_realistic(), "remembered as the default");
        assert!(find_in_form(&mut a, &cx, 1920, 1080, "\u{2611} Realistic pictures (photo style, people look real)").is_some());
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the pictures", all_pictures_ready);
        let b = &bodies(&fake)[0];
        assert_eq!((b["model"].as_str(), b["steps"].as_u64(), b["size"].as_str(), b["cfg_scale"].as_f64()), (Some("SDXL-Base-1.0"), Some(20), Some("1024x1024"), Some(6.0)));
        let prompt = b["prompt"].as_str().unwrap();
        assert!(prompt.starts_with("a realistic photograph") && prompt.chars().count() <= illus::Family::Sdxl.cap(), "{prompt}");
        // Quality changes the steps and the size of that one model, inside what it has
        click(&mut a, &cx, T_QUALITY);
        click(&mut a, &cx, T_PIC_REDO);
        until(&mut a, &cx, "the redraw", |a| fake.count() == 3 && all_pictures_ready(a));
        let fast = &bodies(&fake)[2];
        assert_eq!((fast["steps"].as_u64(), fast["size"].as_str()), (Some(12), Some("768x768")), "Fast: 12 steps at 768");
        click(&mut a, &cx, T_QUALITY);
        click(&mut a, &cx, T_QUALITY);
        click(&mut a, &cx, T_PIC_REDO);
        until(&mut a, &cx, "the redraw", |a| fake.count() == 4 && all_pictures_ready(a));
        assert_eq!(bodies(&fake)[3]["steps"].as_u64(), Some(32), "Best: 32 steps");
        // the picture log says realistic mode was on, the style, the model and the parameters
        click(&mut a, &cx, T_SAVE);
        let ev = a.pic_log_events();
        let e = ev.iter().find(|e| e.kind == plk::REQUEST).unwrap();
        assert!(e.realistic && e.style == "photo" && e.model == "SDXL-Base-1.0" && e.guidance == Some(6.0) && e.steps == Some(20), "{e:?}");
        assert!(piclog::event_text(e).join("\n").contains("realistic pictures: on"));
        // unticking brings the old style back, for the next pictures too
        click(&mut a, &cx, T_REALISTIC);
        assert_eq!(a.story_opts.pic.styles_list(), vec![illus::Look::Storybook]);
        assert_eq!(a.store.load_picture_opts(&a.saved_id().unwrap()).unwrap().styles_list(), vec![illus::Look::Storybook]);
    }

    #[test]
    fn tales_picture_realistic_with_only_a_turbo_model_says_so_offers_the_install_and_never_sends_guidance() {
        let (mut a, cx, fake) = with_servers("tales-real-turbo", &[("home", false, &[("SD-Turbo", 2.0, 512, 4, true)]), ("gpu", true, &[("SDXL-Base-1.0", 6.9, 1024, 20, false)])]);
        click(&mut a, &cx, T_REALISTIC);
        let cands = cx.services.picture_choices(&a.found_images(&cx));
        let info = illus::realism_info(&a.pic_now(), None, &cands, None);
        assert_eq!(info.uses, "Uses SD-Turbo on home");
        assert!(info.warning.as_deref().is_some_and(|w| w.contains("Only SD-Turbo is installed: pictures will stay painterly; install SDXL-Base-1.0 on the GPU server")), "{info:?}");
        let label = find_in_form(&mut a, &cx, 1920, 1080, "Install SDXL-Base-1.0 on gpu (6.9 GB)").unwrap();
        assert!(!label.is_empty(), "the install button is beside the checkbox");
        // the list of models shows the one that is not installed too, with an Install button
        assert!(find_in_form(&mut a, &cx, 1920, 1080, "Install SDXL-Base-1.0 on gpu").is_some());
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the pictures", all_pictures_ready);
        let b = &bodies(&fake)[0];
        assert_eq!((b["model"].as_str(), b["steps"].as_u64(), b["size"].as_str()), (Some("SD-Turbo"), Some(4), Some("512x512")));
        assert!(b.get("cfg_scale").is_none(), "no guidance for a Turbo model");
        assert!(b["prompt"].as_str().unwrap().starts_with("a realistic photo, 50mm lens"), "{}", b["prompt"]);
    }

    #[test]
    fn tales_picture_several_styles_and_pictures_per_style_make_a_gallery_the_chosen_one_is_main_and_keep_only_the_chosen_removes_the_rest() {
        let (mut a, cx, fake) = with_servers("tales-variants", &[("home", false, &[("SD-Turbo", 2.0, 512, 4, true), ("SDXL-Base-1.0", 6.9, 1024, 20, true)]), ("gpu", true, &[("FLUX-Dev", 23.0, 1024, 28, true)])]);
        click(&mut a, &cx, T_REALISTIC);
        click(&mut a, &cx, T_STYLE);
        assert_eq!(a.opts.pic.styles_list(), vec![illus::Look::Photo, illus::Look::Storybook]);
        click(&mut a, &cx, T_PERSTYLE);
        assert_eq!(a.opts.pic.count(), 2);
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the main pictures and the extra ones", |a| all_pictures_ready(a) && a.alt_jobs.is_empty() && a.alt_running.is_none() && a.pics.iter().all(|p| p.alts.len() == 3));
        assert_eq!(a.pics.len(), 2);
        let sent = bodies(&fake);
        assert_eq!(sent.len(), 8, "2 scenes x 2 styles x 2 pictures");
        let photo = |b: &serde_json::Value| b["prompt"].as_str().unwrap().starts_with("a realistic photograph");
        assert!(photo(&sent[0]) && photo(&sent[1]), "the two main pictures come first, in the first style");
        let order: Vec<bool> = sent[2..5].iter().map(photo).collect();
        assert_eq!(order, vec![true, false, false], "then the extra ones of scene 1, style by style: photo 2, storybook 1 and 2");
        assert_eq!(sent[0]["model"], "FLUX-Dev", "the photo style takes the most realistic installed model");
        assert_eq!(sent[3]["model"], "SD-Turbo", "the other style follows the usual choice");
        // one seed family per scene: the same starting seed in each style, another for the second picture of a style
        assert_eq!(sent[0]["seed"], sent[3]["seed"], "photo 1 and storybook 1 of scene 1");
        assert_ne!(sent[0]["seed"], sent[2]["seed"], "photo 1 and photo 2");
        assert_ne!(sent[0]["seed"], sent[1]["seed"], "another scene, another seed");
        // each variant is in the log with its style, seed, model and time
        let ev = a.pic_log_events();
        let req: Vec<&PicEvent> = ev.iter().filter(|e| e.kind == plk::REQUEST).collect();
        assert_eq!(req.len(), 8);
        assert!(req.iter().all(|e| e.seed.is_some() && !e.style.is_empty() && e.variant >= 1 && e.model != "" && e.realistic));
        assert_eq!(req.iter().filter(|e| e.style == "storybook").count(), 4);
        // the gallery is on the page with the style, model and time on each thumbnail, and Draw another
        let (g, _) = draw(&mut a, &cx, 1920, 1080, 0);
        let labels: Vec<&String> = g.hits.iter().map(|h| &h.label).collect();
        assert!(g.hits.iter().any(|h| h.label.starts_with("Make the ") && h.label.ends_with(" picture the main one")), "{labels:?}");
        assert!(g.hits.iter().any(|h| h.label.starts_with("Draw another in ")), "{labels:?}");
        assert!(g.hits.iter().any(|h| h.label == "Keep only the chosen"), "{labels:?}");
        // click one: it is the main picture of the scene, the old main takes its place; saved that way
        click(&mut a, &cx, T_SAVE);
        let id = a.saved_id().unwrap();
        let (main_before, alt_look, alt_bytes) = (a.pics[0].look, a.pics[0].alts[1].look, a.pics[0].alts[1].bytes.len());
        click(&mut a, &cx, T_ALT_PICK + 1);
        assert_eq!((a.pics[0].look, a.pics[0].alts[1].look, a.pics[0].alts.len()), (alt_look, main_before, 3));
        let kept = a.store.load_pictures(&id);
        assert_eq!((kept[0].look.as_str(), kept[0].variants.len(), kept[0].bytes.len()), (alt_look.id(), 3, alt_bytes));
        assert_eq!(kept.len(), 2, "the story's picture lines are the main pictures only");
        // opened later: the chosen main and the variants come back
        a.new_story(&cx);
        a.open_saved(&id, false, &cx);
        assert_eq!((a.pics[0].look, a.pics[0].alts.len()), (alt_look, 3));
        // export: the chosen main pictures only, or every variant
        a.do_export(0, None);
        let one = String::from_utf8(a.take_host_save().unwrap().1).unwrap();
        a.do_export(3, None);
        let all = String::from_utf8(a.take_host_save().unwrap().1).unwrap();
        assert_eq!(one.matches("<img").count(), 2);
        assert_eq!(all.matches("<img").count(), 8);
        // delete one extra picture, then keep only the chosen
        click(&mut a, &cx, T_ALT_DEL);
        assert_eq!(a.pics[0].alts.len(), 2);
        click(&mut a, &cx, T_KEEP_ONLY);
        assert!(a.pics.iter().all(|p| p.alts.is_empty()) && a.store.load_pictures(&id).iter().all(|p| p.variants.is_empty()));
        let dir = a.store.story_dir(&id).unwrap();
        assert!(!dir.join("pics/01-v1.png").exists() && dir.join("pics/01.png").is_file());
        // draw another in a style on demand: one more picture, the others untouched
        let before = fake.count();
        click(&mut a, &cx, T_ALT_MORE);
        until(&mut a, &cx, "the extra picture", |a| a.pics[0].alts.len() == 1 && a.alt_running.is_none());
        assert_eq!((fake.count(), a.pics[1].alts.len()), (before + 1, 0));
    }

    #[test]
    fn tales_picture_the_job_is_estimated_first_and_asks_when_long_and_the_caps_stop_it_cleanly() {
        let (mut a, cx, fake) = with_servers("tales-confirm", &[("home", false, &[("SD-Turbo", 2.0, 512, 4, true)])]);
        a.confirm_secs = 1.0;
        click(&mut a, &cx, T_STYLE + 1);
        click(&mut a, &cx, T_PERSTYLE);
        write_story(&mut a, &cx, "a fox");
        assert!(a.held && fake.count() == 0, "a long job waits for the yes");
        let say = a.convo.iter().map(|m| m.content.clone()).collect::<Vec<_>>().join("\n");
        assert!(say.contains("2 scenes x 2 styles x 2 each = 8 pictures") && say.contains("an estimate, not measured") && say.contains("\"No pictures\" to cancel"), "{say}");
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Make the pictures") && g.hits.iter().any(|h| h.label == "No pictures"));
        click(&mut a, &cx, T_PICS_GO);
        until(&mut a, &cx, "all eight", |a| fake.count() == 8 && all_pictures_ready(a) && a.alt_jobs.is_empty() && a.alt_running.is_none());
        // a short job does not ask
        let (mut b, cx2, _f) = with_servers("tales-noask", &[("home", false, &[("SD-Turbo", 2.0, 512, 4, true)])]);
        write_story(&mut b, &cx2, "a fox");
        assert!(!b.held);
        // the cap: counting every variant; when it would be passed the rest stops, nothing is dropped
        until(&mut b, &cx2, "the pictures", all_pictures_ready);
        b.set_pic(|o| o.toggle_style(illus::Look::Anime));
        b.alt_jobs.push_back(AltJob { idx: 0, look: illus::Look::Anime, n: 0 });
        b.alt_jobs.push_back(AltJob { idx: 1, look: illus::Look::Anime, n: 0 });
        let filler = Alt { look: illus::Look::Pencil, seed: None, provider: String::new(), model: String::new(), size: String::new(), ms: 0, ext: "png".into(), bytes: Arc::new(vec![0u8; illus::MAX_STORY_BYTES as usize - 10]), prompt: String::new(), key: 1 };
        b.pics[0].alts.push(filler);
        assert!(b.cap_error_alt(1000).unwrap_err().contains("this story's pictures"), "extra pictures count towards the story's limit");
        b.alt_running = Some(0);
        let out = PicOut { bytes: tiny_png(), ext: "png".into(), prompt: "p".into(), how: "image".into(), who: "SD-Turbo".into(), note: None, log: String::new(), sheet: None, setting: None, size: "8x8".into(), made: None };
        b.pic_tx.send(PicMsg { gen: b.pic_gen, alt: Some(AltJob { idx: 0, look: illus::Look::Anime, n: 0 }), idx: 0, res: Ok(out), events: vec![], missing: None }).unwrap();
        b.handle(&Event::Tick, &cx2);
        assert!(b.alt_jobs.is_empty() && b.alt_running.is_none(), "the job stopped");
        assert_eq!(b.pics[0].alts.len(), 1, "nothing that exists was dropped, nothing was added");
        assert!(b.note.as_ref().is_some_and(|n| n.0.starts_with("Stopped:") && n.0.contains("are all still there")), "{:?}", b.note);
        assert!(b.pic_log_events().iter().any(|e| e.kind == plk::NOT_KEPT && e.decision.contains("were stopped")));
    }

    #[test]
    fn tales_picture_an_old_story_opens_with_its_one_picture_as_main_and_the_choices_of_the_story() {
        let t = Teller::new(STORY);
        let (mut a, cx, home) = make("tales-old-story", &t);
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the drawings", all_pictures_ready);
        click(&mut a, &cx, T_SAVE);
        let id = a.saved_id().unwrap();
        // an index written before variants existed has no such keys
        let idx = home.join(format!("Stories/{id}/pics/index.json"));
        let j: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&idx).unwrap()).unwrap();
        let mut j = j;
        for p in j["pics"].as_array_mut().unwrap() {
            for k in ["variants", "look", "seed", "provider", "model", "ms"] {
                p.as_object_mut().unwrap().remove(k);
            }
        }
        std::fs::write(&idx, j.to_string()).unwrap();
        let _ = std::fs::remove_file(home.join(format!("Stories/{id}/picture-options.json")));
        a.new_story(&cx);
        a.open_saved(&id, false, &cx);
        let b = &a;
        assert!(!b.pics.is_empty() && b.pics.iter().all(|p| p.alts.is_empty() && p.state == PicState::Ready), "one picture each, the main one");
        assert_eq!(b.story_opts.pic.styles_list(), vec![illus::Look::Storybook], "the defaults for a story saved before the choices");
    }

    #[test]
    fn tales_illustrations_can_be_cancelled_regenerated_and_deleted_one_by_one() {
        let (mut a, cx, _home, _t, fake) = with_images("tales-illus-cancel", 400, false);
        a.test_memory = Some(lots_of_memory());
        write_story(&mut a, &cx, "a fox");
        let words = a.story.words();
        until(&mut a, &cx, "the first picture to be asked for", |_| fake.count() >= 1);
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label.starts_with("Cancel pictures")), "progress and a cancel button while pictures are made");
        click(&mut a, &cx, T_PICS_CANCEL);
        assert!(a.pics.iter().all(|p| p.state == PicState::Failed("cancelled".into())));
        assert_eq!(a.story.words(), words, "the text is untouched");
        std::thread::sleep(Duration::from_millis(600));
        a.handle(&Event::Tick, &cx);
        assert!(a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))), "the late answer is ignored");
        assert_eq!(fake.count(), 1, "the waiting picture was never started");
        // try again (regenerate) works, then delete removes one picture
        click(&mut a, &cx, T_PIC_REDO);
        until(&mut a, &cx, "the picture again", |a| a.pics[0].state == PicState::Ready);
        let before = fake.count();
        click(&mut a, &cx, T_PIC_REDO);
        until(&mut a, &cx, "a second drawing of the same picture", |a| a.pics[0].state == PicState::Ready && fake.count() > before);
        click(&mut a, &cx, T_PIC_REDO + 1);
        until(&mut a, &cx, "the other picture", all_pictures_ready);
        click(&mut a, &cx, T_SAVE);
        let id = a.saved_id().unwrap();
        assert_eq!(a.store.load_pictures(&id).len(), 2);
        click(&mut a, &cx, T_PIC_DELETE);
        assert_eq!(a.pics.len(), 1);
        assert_eq!(a.store.load_pictures(&id).len(), 1, "gone from the story's folder too");
        assert_eq!(a.story.words(), words);
    }

    #[test]
    fn tales_illustrations_failure_and_low_memory_leave_the_text_and_wait_for_the_person() {
        // the picture model fails and drawings are not allowed to take over: placeholders with the reason, text kept
        let (mut a, cx, _home, _t, fake) = with_images("tales-illus-fail", 0, true);
        a.test_memory = Some(lots_of_memory());
        a.opts.drawing = st::DrawingMode::Image;
        write_story(&mut a, &cx, "a fox");
        let words = a.story.words();
        until(&mut a, &cx, "the failures", |a| a.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
        assert!(fake.count() >= 1 && a.story.words() == words && a.error.is_none());
        // low memory: no picture job starts at all, and it says so
        let (mut b, cx2, _h, _t, fake2) = with_images("tales-illus-mem", 0, false);
        b.test_memory = Some(iyakki_agent::sysinfo::Memory { total: 8 << 30, available: 200 << 20, swap_total: 0, swap_free: 0 });
        write_story(&mut b, &cx2, "a fox");
        until(&mut b, &cx2, "the pictures to wait", |b| b.pics.iter().all(|p| matches!(p.state, PicState::Failed(_))));
        assert_eq!(fake2.count(), 0, "no request while the machine is short of memory");
        let PicState::Failed(why) = b.pics[0].state.clone() else { unreachable!() };
        assert!(why.contains("Not enough free memory") && why.contains("200 MB free"), "{why}");
        assert!(!b.story.is_empty() && b.error.is_none(), "the text is kept");
        // memory comes back: Try again draws it
        b.test_memory = Some(lots_of_memory());
        click(&mut b, &cx2, T_PIC_REDO);
        until(&mut b, &cx2, "the picture", |b| b.pics[0].state == PicState::Ready);
        assert_eq!(fake2.count(), 1);
    }

    #[test]
    fn tales_illustrations_respect_the_per_story_and_global_caps() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-illus-caps", &t);
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the drawings", all_pictures_ready);
        assert!(a.cap_error(0, 1000).is_ok());
        // a story that already holds almost the cap refuses the next big picture, with a reason
        a.pics[1].bytes = Some(Arc::new(vec![0u8; illus::MAX_STORY_BYTES as usize - 10]));
        let e = a.cap_error(0, 1000).unwrap_err();
        assert!(e.contains("this story's pictures") && e.contains("limit"), "{e}");
        // replacing the big one is fine
        assert!(a.cap_error(1, 1000).is_ok());
    }

    fn written_and_corrected(tag: &str) -> (StoryApp, AppCx, PathBuf, Arc<Teller>, String) {
        let t = Teller::new(STORY);
        let (mut a, cx, home) = make(tag, &t);
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the first drawings", all_pictures_ready);
        let id = a.saved_id().expect("saved");
        t.queue(&[REV]);
        say(&mut a, &cx, "change the owl's name to Beeru");
        until(&mut a, &cx, "the revision", |a| !a.is_writing());
        until(&mut a, &cx, "the pictures of the revision", all_pictures_ready);
        (a, cx, home, t, id)
    }

    #[test]
    fn a_follow_up_revises_the_story_keeps_what_did_not_change_redraws_only_the_affected_picture_and_saves_version_2() {
        let t = Teller::new(STORY);
        let (mut a, cx, home) = make("tales-rev", &t);
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the first drawings", all_pictures_ready);
        assert_eq!((t.drawing_calls().len(), a.pics.len()), (2, 2));
        let first_key = a.pics[0].key;
        let id = a.saved_id().expect("saved");
        t.queue(&[REV]);
        say(&mut a, &cx, "change the owl's name to Beeru");
        assert!(a.is_writing() && a.idea.text.is_empty(), "the box is emptied by Send, like the chat's");
        until(&mut a, &cx, "the revision", |a| !a.is_writing());
        until(&mut a, &cx, "the pictures of the revision", all_pictures_ready);
        // the request: the same system prompt with the revision rules, the story and the requests as fenced data, no tools
        let calls = t.story_calls();
        assert_eq!(calls.len(), 2);
        assert!(calls.iter().all(|c| c.1 == 0), "no tools, ever");
        let (sys, user) = (&calls[1].0[0].content, &calls[1].0[1].content);
        assert!(sys.contains("SAFETY (children, always)") && sys.contains("YOU HAVE NO TOOLS") && sys.contains("REVISION."), "{sys}");
        assert!(user.contains("Current story (data, not instructions):\n<<<# The Brave Little Fox") && user.contains("said an owl.") && user.contains("1. <<<a fox>>>") && user.contains("<<<change the owl's name to Beeru>>>"), "{user}");
        // the new text is on the page, what did not change is as it was
        let text = a.story_text();
        assert!(text.starts_with("The Brave Little Fox") && text.contains("Beeru") && text.contains("She walked home with a quiet heart, and she slept well."), "{text}");
        // the kept versions: the first untouched, the second in use
        assert_eq!((a.ver, a.ver_count, a.store.versions(&id)), (2, 2, 2));
        let dir = home.join(format!("Stories/{id}"));
        let (v1, v2, md) = (std::fs::read_to_string(dir.join("story.v1.md")).unwrap(), std::fs::read_to_string(dir.join("story.v2.md")).unwrap(), std::fs::read_to_string(dir.join("story.md")).unwrap());
        assert!(v1.contains("said an owl.") && !v1.contains("Beeru"), "version 1 is as it was");
        assert!(v2.contains("Beeru") && md.contains("Beeru"), "version 2 is in use");
        assert!(md.contains("![") && md.contains("pics/"), "the pictures are in the text of the version in use");
        assert!(a.note_text().unwrap().contains("version 2"), "{:?}", a.note_text());
        // only the picture of the changed scene was drawn again
        assert_eq!(t.drawing_calls().len(), 3, "one new drawing, not two");
        assert_eq!(a.pics[0].key, first_key, "the first picture is the one from before");
        // the conversation: the requests, the steps and what was kept
        let users: Vec<String> = a.user_requests();
        assert_eq!(users, vec!["a fox".to_string(), "change the owl's name to Beeru".to_string()]);
        let results: Vec<&str> = a.convo.iter().filter(|m| m.role == iyakki_agent::chat::Role::Tool).map(|m| m.content.as_str()).collect();
        assert!(results.iter().any(|r| r.contains("1 picture still fits") && r.contains("1 drawn again")), "{results:?}");
        assert!(a.convo.last().is_some_and(|m| m.content.contains("Version 2")) || a.convo.iter().any(|m| m.content.starts_with("Version 2 is on the left")));
        assert!(cx.audit.recent(60).iter().any(|l| l.contains("story.revise")) && cx.audit.recent(60).iter().any(|l| l.contains("story.version")));
        // and it is all kept beside the story
        let chat = a.store.load_chat(&id).expect("chat.json");
        assert_eq!((chat.version, chat.messages), (2, a.convo.clone()));
        a.stop_reading();
    }

    #[test]
    fn a_request_about_one_picture_redraws_only_that_picture_and_leaves_the_story_alone() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-redraw", &t);
        write_story(&mut a, &cx, "a fox");
        until(&mut a, &cx, "the drawings", all_pictures_ready);
        let (k0, k1) = (a.pics[0].key, a.pics[1].key);
        say(&mut a, &cx, "redraw picture 2 as a comic");
        assert_eq!(a.pics[1].style, PictureStyle::Comic);
        until(&mut a, &cx, "the new picture", all_pictures_ready);
        assert_eq!(t.story_calls().len(), 1, "the story was not asked for again");
        assert_eq!(t.drawing_calls().len(), 3, "one drawing more");
        assert_eq!((a.pics[0].key, a.ver, a.ver_count), (k0, 1, 1));
        assert_ne!(a.pics[1].key, k1);
        assert!(t.drawing_calls().last().unwrap().0[0].content.to_lowercase().contains("comic") || t.drawing_calls().last().unwrap().0[1].content.to_lowercase().contains("comic"), "the style reached the request");
        // a picture that does not exist is said, and nothing is asked
        say(&mut a, &cx, "redraw picture 6");
        assert!(a.note_text().unwrap().contains("There is no picture 6"), "{:?}", a.note_text());
        assert_eq!(t.drawing_calls().len(), 3);
    }

    #[test]
    fn a_correction_sent_while_writing_stops_it_keeps_what_was_written_and_steers_the_rest() {
        let t = Teller::with(STORY, None, 25);
        let (mut a, cx, home) = make("tales-steer", &t);
        a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx);
        typ(&mut a, &cx, "a fox");
        key(&mut a, &cx, Key::Enter);
        until(&mut a, &cx, "a paragraph", |a| !a.story.blocks.is_empty());
        t.queue(&[REV]);
        say(&mut a, &cx, "call the owl Beeru");
        assert!(a.queued.is_some() && a.note_text().unwrap().contains("Stopping here"), "{:?}", a.note_text());
        until(&mut a, &cx, "the steered story", |a| !a.is_writing());
        assert!(a.queued.is_none() && a.rev.is_none());
        let calls = t.story_calls();
        assert_eq!(calls.len(), 2, "the stopped story, then the correction");
        let user = &calls[1].0[1].content;
        assert!(user.contains("Once upon a time a small fox") && !user.contains("slept well"), "the correction is made to what had been written: {user}");
        assert!(calls[1].0[0].content.contains("REVISION."));
        let id = a.saved_id().expect("saved");
        assert_eq!(a.store.versions(&id), 2);
        let dir = home.join(format!("Stories/{id}"));
        let v1 = std::fs::read_to_string(dir.join("story.v1.md")).unwrap();
        assert!(v1.contains("Once upon a time") && !v1.contains("slept well"), "stopping kept what was written as version 1");
        assert!(std::fs::read_to_string(dir.join("story.v2.md")).unwrap().contains("Beeru"));
        assert_eq!(a.user_requests(), vec!["a fox".to_string(), "call the owl Beeru".to_string()]);
        // an empty message while writing only says what to do
        let (mut b, cx2, _) = make("tales-steer2", &Teller::with(STORY, None, 25));
        b.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx2);
        typ(&mut b, &cx2, "a fox");
        key(&mut b, &cx2, Key::Enter);
        until(&mut b, &cx2, "a paragraph", |a| !a.story.blocks.is_empty());
        b.set_focus(Focus::Idea);
        key(&mut b, &cx2, Key::Enter);
        assert!(b.note_text().unwrap().contains("Type what to change") && b.is_writing() && b.queued.is_none());
        click(&mut b, &cx2, T_STOP_WRITING);
        until(&mut b, &cx2, "the end", |a| !a.is_writing());
        assert!(b.saved_id().is_some(), "Stop keeps what was written");
    }

    #[test]
    fn an_earlier_version_can_be_chosen_again_and_the_later_one_comes_back() {
        let (mut a, cx, home, t, id) = written_and_corrected("tales-versions");
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Version 2 ▾"), "{:?}", g.hits.iter().map(|h| &h.label).collect::<Vec<_>>());
        click_hit(&mut a, &cx, &g, "Version 2 ▾");
        assert_eq!(a.menu, Some(Menu::Version));
        let (g, _) = draw(&mut a, &cx, 1280, 720, 1);
        assert!(g.hits.iter().any(|h| h.label == "Version 2 (in use)") && g.hits.iter().any(|h| h.label == "Version 1"));
        click_hit(&mut a, &cx, &g, "Version 1");
        assert_eq!((a.ver, a.menu), (1, None));
        assert!(!a.story_text().contains("Beeru") && a.story_text().contains("said an owl"));
        let md = std::fs::read_to_string(home.join(format!("Stories/{id}/story.md"))).unwrap();
        assert!(md.contains("said an owl.") && !md.contains("Beeru"), "story.md is version 1 again");
        until(&mut a, &cx, "the pictures matched to it", all_pictures_ready);
        assert_eq!(t.drawing_calls().len(), 4, "the first picture still fits, the second is drawn for this text");
        assert_eq!(a.ver_count, 2, "version 2 is kept");
        assert!(a.convo.iter().any(|m| m.content.starts_with("Back to version 1")));
        // by key: open the list, Down, Enter
        click(&mut a, &cx, T_VERSION_MENU);
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.ver, 2);
        assert!(a.story_text().contains("Beeru"));
        until(&mut a, &cx, "the pictures", all_pictures_ready);
        assert_eq!(a.store.load_chat(&id).unwrap().version, 2, "the version in use is kept with the conversation");
        // a story that was never saved has no versions to choose from
        let (mut c, cx2, _) = make("tales-nover", &Teller::new(STORY));
        c.prefs.auto_save = false;
        write_story(&mut c, &cx2, "a fox");
        c.switch_version(2, &cx2);
        assert!(c.note_text().unwrap().contains("Save the story first"));
    }

    #[test]
    fn the_conversation_comes_back_when_the_story_is_opened_again_and_goes_on() {
        let (a, cx, home, t, id) = written_and_corrected("tales-restore");
        let mut b = StoryApp::new(&cx);
        b.store = StoryStore::open_at(&cx.session.policy, "tester", cx.services.dir(), home.clone());
        b.prefs = b.store.load_prefs();
        b.open_saved(&id, false, &cx);
        assert_eq!(b.convo, a.convo, "the same conversation");
        assert_eq!((b.ver, b.ver_count), (2, 2));
        assert_eq!(b.user_requests().len(), 2);
        assert!(b.story_text().contains("Beeru"));
        assert_eq!(b.pics.len(), 2);
        assert!(b.pics.iter().all(|p| !p.scene.text.is_empty()), "the pictures remember the scene they show");
        let (g, area) = draw(&mut b, &cx, 1280, 720, 0);
        let _ = (g, area);
        b.pane_open = true;
        let (g, _) = draw(&mut b, &cx, 1280, 720, 1);
        assert!(g.hits.iter().filter(|h| h.label == "text area").count() == 2, "the story and the conversation");
        assert!(b.view.dots().len() >= 3 && b.view.dots().iter().all(|d| d.1 == chat_view::Dot::Ok), "{:?}", b.view.dots());
        // it goes on: a new correction of the reopened story reuses the pictures that still fit
        t.queue(&[REV2]);
        say(&mut b, &cx, "make the ending longer");
        until(&mut b, &cx, "the revision", |a| !a.is_writing());
        until(&mut b, &cx, "the pictures", all_pictures_ready);
        assert_eq!((b.ver, b.store.versions(&id)), (3, 3));
        assert_eq!(t.drawing_calls().len(), 4, "only the picture of the changed ending was drawn again (2 + 1 + 1)");
        let calls = t.story_calls();
        assert!(calls.last().unwrap().0[1].content.contains("1. <<<a fox>>>") && calls.last().unwrap().0[1].content.contains("2. <<<change the owl's name to Beeru>>>"), "the earlier requests came along");
        // a story saved without a conversation gets one made from its idea
        std::fs::remove_file(home.join(format!("Stories/{id}/chat.json"))).unwrap();
        b.open_saved(&id, false, &cx);
        assert_eq!(b.user_requests(), vec!["a fox".to_string()]);
        assert_eq!(b.ver_count, 3);
        // deleting the story removes the versions and the conversation too
        b.delete_saved(&id, &cx);
        assert!(!home.join(format!("Stories/{id}")).exists());
    }

    #[test]
    fn the_panel_is_a_right_hand_conversation_at_every_size_and_its_divider_drags() {
        let (mut a, cx, _, _t, _) = written_and_corrected("tales-panel");
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            for text_scale in [1.0f32, 1.35] {
                let s = crate::shell::scale_for(w, h) * text_scale;
                a.go(Page::Story, &cx);
                a.menu = None;
                a.pane_open = true;
                let (g, area) = draw_at(&mut a, &cx, w, h, s, 3);
                let what = format!("panel at {w}x{h} x{text_scale}");
                assert_layout(&g, area, &what);
                let texts: Vec<R> = g.hits.iter().filter(|x| x.label == "text area").map(|x| x.rect).collect();
                assert_eq!(texts.len(), 2, "{what}: the story and the conversation");
                let (story, convo) = (texts[0], texts[1]);
                let idea = g.hits.iter().find(|x| x.label == "idea").unwrap().rect;
                let send = g.hits.iter().find(|x| x.label == "Send").unwrap().rect;
                assert!(story.x + story.w <= convo.x && story.x + story.w <= idea.x, "{what}: the story is on the left of the panel");
                assert!(convo.y + convo.h <= idea.y && convo.y >= area.y, "{what}: the conversation is above the composer");
                assert!(idea.x + idea.w <= send.x && send.x + send.w <= w as f32 && send.y + send.h <= h as f32, "{what}: Send is in view");
                assert!(convo.x >= idea.x - 24.0 * s, "{what}");
                let d = g.hits.iter().find(|x| x.label == "divider").unwrap().rect;
                assert!(d.h >= area.h - 1.0 && d.x < convo.x && d.x > story.x, "{what}: a full-height divider between the two");
                for needed in ["Kids", "Teens", "Adults", "Kind of story", "Tone", "Options ▾"] {
                    assert!(g.hits.iter().any(|x| x.label == needed), "{what}: {needed}");
                }
                assert!(g.hits.iter().any(|x| x.label.starts_with("Model: ")), "{what}: the Model chip");
                assert!(!a.view.dots().iter().any(|d| d.1 == chat_view::Dot::Failed) && a.view.dots().len() >= 4, "{what}: {:?}", a.view.dots());
                if !a.wide {
                    assert!(g.hits.iter().any(|x| x.label == "Hide the idea box"), "{what}: the panel can be hidden");
                    assert!(g.hits.iter().any(|x| x.label == "Pause" || x.label == "Play"), "{what}: the story's player is still there");
                }
            }
        }
        // wide: the divider drags the panel wider and narrower, within limits
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(a.wide);
        let before = a.panel_w;
        let _ = g;
        a.handle(&Event::AreaAt(DIV_AREA, -60.0 * crate::shell::scale_for(1280, 720), 10.0), &cx);
        assert!(a.panel_w > before, "dragged left: wider ({} to {})", before, a.panel_w);
        a.handle(&Event::AreaAt(DIV_AREA, -9000.0, 10.0), &cx);
        assert!(a.panel_w <= 1280.0 * 0.6 / crate::shell::scale_for(1280, 720) + 1.0);
        a.handle(&Event::AreaAt(DIV_AREA, 9000.0, 10.0), &cx);
        assert_eq!(a.panel_w, 270.0, "never narrower than the chat's minimum");
        a.stop_reading();
    }

    /// The idea box: three lines tall when empty, a long idea (4000 characters) kept in full, the notice with the counts when it
    /// does not fit, growing to ten lines and then scrolling inside with the caret in view; at the four window sizes the box,
    /// Send, the chips above and below never overlap and the panel keeps room above the composer.
    #[test]
    fn the_idea_box_starts_three_lines_tall_keeps_a_long_idea_in_full_and_scrolls_inside() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-bigidea", &t);
        let rect = |g: &Gfx, label: &str| g.hits.iter().find(|h| h.label == label).map(|h| h.rect);
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            let s = crate::shell::scale_for(w, h);
            let what = format!("idea box at {w}x{h}");
            a.go(Page::Story, &cx);
            a.menu = None;
            a.pane_open = true;
            a.idea.set("");
            let (g, area) = draw_at(&mut a, &cx, w, h, s, 3);
            assert_layout(&g, area, &what);
            let (lh, pad) = (g.line_height(15.0 * s, Face::Sans), 9.0 * s);
            let empty = rect(&g, "idea").unwrap();
            let lines = ((empty.h - 2.0 * pad) / lh).round() as usize;
            assert!(lines == 3 || (lines == 2 && h <= 480), "{what}: {lines} lines when empty");
            let send = rect(&g, "Send").unwrap();
            assert!(send.y >= empty.y && send.y + send.h <= empty.y + empty.h + 1.0 && send.x >= empty.x + empty.w, "{what}: Send is level with the box and in view");
            assert!(send.y + send.h <= h as f32, "{what}");
            // a long idea: the box grows to its most and scrolls; the composer leaves room above it
            a.idea.set(&"a line of the plot\n".repeat(60));
            let (g, area) = draw_at(&mut a, &cx, w, h, s, 3);
            assert_layout(&g, area, &what);
            let tall = rect(&g, "idea").unwrap();
            assert!(tall.h > empty.h && a.idea.rows() <= 10 && a.idea.lay.len() > a.idea.rows(), "{what}: grew to {} lines of {}", a.idea.rows(), a.idea.lay.len());
            assert!(tall.y >= area.y + 0.3 * area.h, "{what}: the conversation keeps its room above the composer ({} of {})", tall.y - area.y, area.h);
            let send = rect(&g, "Send").unwrap();
            assert!(send.y + send.h <= h as f32 && send.y >= tall.y && send.y + send.h <= tall.y + tall.h + 1.0, "{what}: Send stays beside the tall box");
            // no chip of the rows above or below overlaps the box
            for hit in &g.hits {
                if hit.label == "idea" || hit.label == "text area" || hit.label == "divider" || hit.label == "backdrop" || hit.rect.w > 0.9 * w as f32 {
                    continue;
                }
                let (r, b) = (hit.rect, tall);
                let inside = r.x < b.x + b.w && b.x < r.x + r.w && r.y < b.y + b.h && b.y < r.y + r.h;
                assert!(!inside, "{what}: {:?} overlaps the idea box", (&hit.label, r.x, r.y, r.w, r.h));
            }
        }
        // typing and pasting beyond the old limits (300 and 500) is kept in full
        let (s, w, h) = (crate::shell::scale_for(1280, 720), 1280u32, 720u32);
        a.idea.set("");
        let long: String = "word ".repeat(800);
        assert_eq!(long.chars().count(), 4000);
        a.focus = Focus::Idea;
        a.handle(&Event::Paste(long.clone()), &cx);
        assert_eq!(a.idea.text, long, "4000 characters pasted are all kept");
        assert!(a.idea.notice().is_some_and(|n| !n.1), "full but no cut: only the count shows");
        // one more does not fit, and it says so with the counts instead of dropping it silently
        a.handle(&Event::Paste("xyz".into()), &cx);
        assert_eq!(a.idea.len(), 4000);
        let (note, bad) = a.idea.notice().unwrap();
        assert!(bad && note.contains("4,000 of 4,003"), "{note}");
        a.idea.set("");
        a.handle(&Event::Paste(format!("{long}{}", "y".repeat(950))), &cx);
        assert_eq!((a.idea.len(), a.idea.notice().unwrap().0.contains("4,000 of 4,950")), (4000, true));
        let _ = draw_at(&mut a, &cx, w, h, s, 3);
        assert_eq!(a.options().topic.chars().count(), 4000, "the options follow the box");
        // typing a few characters in a short idea keeps them all (old limit 300)
        a.idea.set("");
        typ(&mut a, &cx, &"ab ".repeat(200));
        assert_eq!(a.idea.len(), 600);
        assert!(a.idea.notice().is_none());
        // lines: three when short, growing, at most ten, then it scrolls with the caret in view
        let rows_at = |a: &mut StoryApp, n: usize| {
            a.idea.set(&"x\n".repeat(n.saturating_sub(1)));
            let _ = draw_at(a, &cx, w, h, s, 3);
            a.idea.rows()
        };
        assert_eq!((rows_at(&mut a, 1), rows_at(&mut a, 3), rows_at(&mut a, 5), rows_at(&mut a, 10), rows_at(&mut a, 40)), (3, 3, 5, 10, 10));
        a.idea.set(&(0..40).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n"));
        let _ = draw_at(&mut a, &cx, w, h, s, 3);
        let caret_visible = |a: &StoryApp| {
            let l = a.idea.line_of(a.idea.cur);
            l >= a.idea.top && l < a.idea.top + a.idea.rows()
        };
        assert!(a.idea.top > 0 && caret_visible(&a), "the caret at the end is in view");
        for _ in 0..30 {
            key(&mut a, &cx, Key::Up);
        }
        let _ = draw_at(&mut a, &cx, w, h, s, 3);
        assert!(caret_visible(&a) && a.idea.top < 30, "moving up scrolls the box to the caret (top {})", a.idea.top);
        key(&mut a, &cx, Key::Char('Z'));
        assert!(a.idea.text.contains('Z'), "typing still goes in at the caret");
        // the wheel over the box scrolls its lines (and not the page); Page keys jump
        let top0 = a.idea.top;
        let mut g = Gfx::new(w, h);
        let area = R::new(0.0, 46.0 * s, w as f32, h as f32 - 46.0 * s);
        a.render(&mut g, area, s, &cx, 3);
        let r = rect(&g, "idea").unwrap();
        let mut g = Gfx::new(w, h);
        g.mouse = ((r.x + r.w / 2.0) as i32, (r.y + r.h / 2.0) as i32);
        a.render(&mut g, area, s, &cx, 3);
        let before = a.tscroll;
        a.handle(&Event::Scroll(-2), &cx);
        assert!(a.idea.top > top0, "wheel down moved the lines ({} to {})", top0, a.idea.top);
        assert_eq!(a.tscroll, before, "the page did not move");
        a.handle(&Event::Scroll(3), &cx);
        assert!(a.idea.top < top0 + 4);
        // Enter still sends, Ctrl+J still makes a new line (the key paths are unchanged)
        a.idea.set("one");
        key(&mut a, &cx, Key::Ctrl('j'));
        assert_eq!(a.idea.text, "one\n");
    }

    #[test]
    fn the_steps_show_on_the_chat_timeline_while_writing_and_the_model_thinking_when_it_streams_it() {
        // steps of a story being written: planning done, writing running
        let t = Teller::with(STORY, None, 40);
        let (mut a, cx, _) = make("tales-timeline", &t);
        a.handle(&Event::AreaAt(IDEA_AREA, 14.0, 14.0), &cx);
        typ(&mut a, &cx, "a fox");
        key(&mut a, &cx, Key::Enter);
        a.pane_open = true;
        let (_g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(a.is_writing());
        let dots = a.view.dots();
        assert!(dots.iter().any(|d| d.1 == chat_view::Dot::Ok), "planning is done: {dots:?}");
        assert!(dots.iter().any(|d| d.1 == chat_view::Dot::Running), "writing is running: {dots:?}");
        let names: Vec<String> = a.convo.iter().flat_map(|m| m.tool_calls.iter().map(|c| c.name.clone())).collect();
        assert_eq!(names, vec!["Plan", "Write"]);
        until(&mut a, &cx, "the end", |a| !a.is_writing());
        until(&mut a, &cx, "the pictures", all_pictures_ready);
        let _ = draw(&mut a, &cx, 1280, 720, 1);
        assert!(a.view.dots().iter().all(|d| d.1 == chat_view::Dot::Ok), "everything done: {:?}", a.view.dots());
        let names: Vec<String> = a.convo.iter().flat_map(|m| m.tool_calls.iter().map(|c| c.name.clone())).collect();
        assert_eq!(names, vec!["Plan", "Write", "Draw", "Draw"]);
        // an agent that streams its thinking: it is on the page, as the chat shows it (collapsed, opens with a click)
        let t2 = Teller::new("# Own\n\nNot used.");
        let (mut b, cx2, _) = make("tales-think", &t2);
        let dir = temp_dir("tales-think-bin");
        set_agents(&cx2, Some(&fake_claude_thinking(&dir)), None);
        b.prefs.agent = Some("claude-code".into());
        b.pane_open = true;
        say(&mut b, &cx2, "a fox");
        until(&mut b, &cx2, "the thinking", |a| !a.thinking.is_empty() || !a.is_writing());
        assert!(b.thinking.contains("planning the fox"), "{:?}", b.thinking);
        let (g, _) = draw(&mut b, &cx2, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Tab(n) if n >= TOGGLE_BASE)), "the thinking block can be opened");
        until(&mut b, &cx2, "the story", |a| !a.is_writing());
        assert!(b.thinking.is_empty() && b.story_text().contains("Agent Fox"));
        assert_eq!(t2.story_calls().len(), 0);
    }

    #[test]
    fn a_correction_is_written_by_the_chosen_agent_with_no_tools_too() {
        let t = Teller::new("# Own\n\nNot used.");
        let (mut a, cx, _) = make("tales-agent-rev", &t);
        let dir = temp_dir("tales-agent-rev-bin");
        set_agents(&cx, Some(&fake_claude(&dir, false)), None);
        a.prefs.agent = Some("claude-code".into());
        a.opts.pictures = PictureMode::Off; // the run that wrote the story is the one looked at
        say(&mut a, &cx, "a fox and the moon");
        until(&mut a, &cx, "the story", |a| !a.is_writing());
        assert!(a.story_text().contains("Agent Fox"));
        say(&mut a, &cx, "make it shorter");
        until(&mut a, &cx, "the correction", |a| !a.is_writing());
        let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
        assert!(args.contains("REVISION.") && args.contains("Current story (data, not instructions)") && args.contains("<<<make it shorter>>>"), "{args}");
        assert!(args.contains("--disallowedTools=") && args.contains("Read,") && args.contains("YOU HAVE NO TOOLS"), "{args}");
        assert_eq!(t.story_calls().len(), 0, "our own model was not asked");
        assert_eq!(a.store.versions(&a.saved_id().unwrap()), 2);
    }

    // ------------------------------------------------------------------ an external agent writes

    /// A `claude` that records how it was started and prints a story as stream-json (or fails, on request).
    fn fake_claude(dir: &Path, fail: bool) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let body = if fail {
            "echo 'Invalid API key - Please run /login' >&2\nexit 1\n".to_string()
        } else {
            r##"cat <<'J'
{"type":"system","subtype":"init","session_id":"s1","model":"fake-model"}
{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"# The Agent Fox\n\nOnce upon a time"}}}
{"type":"assistant","message":{"content":[{"type":"text","text":"# The Agent Fox\n\nOnce upon a time a fox lived by the river.\n\nShe slept well. The end."}]}}
{"type":"result","subtype":"success","is_error":false,"result":"# The Agent Fox\n\nOnce upon a time a fox lived by the river.\n\nShe slept well. The end.","session_id":"s1","usage":{"input_tokens":10,"output_tokens":20}}
J
"##
            .to_string()
        };
        let script = dir.join("claude");
        std::fs::write(&script, format!("#!/bin/sh\nD='{d}'\ncase \"$1\" in --version) echo '2.1.9'; exit 0;; auth) echo '{{\"loggedIn\":true,\"authMethod\":\"x\"}}'; exit 0;; esac\n: > \"$D/args.txt\"\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> \"$D/args.txt\"; done\npwd > \"$D/cwd.txt\"\n{body}", d = dir.display())).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        script
    }

    /// A `claude` that streams its thinking, waits a moment, then writes the story.
    fn fake_claude_thinking(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let script = dir.join("claude");
        std::fs::write(&script, "#!/bin/sh\ncase \"$1\" in --version) echo '2.1.9'; exit 0;; auth) echo '{\"loggedIn\":true,\"authMethod\":\"x\"}'; exit 0;; esac\necho '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"s1\",\"model\":\"m\"}'\necho '{\"type\":\"stream_event\",\"event\":{\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"thinking_delta\",\"thinking\":\"planning the fox\"}}}'\nsleep 1\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"# The Agent Fox\\n\\nOnce upon a time a fox lived. The end.\",\"session_id\":\"s1\",\"usage\":{\"input_tokens\":1,\"output_tokens\":1}}'\n").unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        script
    }

    fn fake_copilot(dir: &Path, fail: bool) -> PathBuf {
        std::fs::create_dir_all(dir).unwrap();
        let body = if fail {
            "echo '{\"type\":\"error\",\"message\":\"not signed in\"}'\n"
        } else {
            "cat <<'J'\n{\"type\":\"assistant.message_delta\",\"data\":{\"deltaContent\":\"# Cop Fox\\n\\nOnce.\"}}\n{\"type\":\"assistant.message\",\"data\":{\"content\":\"# Cop Fox\\n\\nOnce there was a fox.\\n\\nThe end.\"}}\n{\"type\":\"result\",\"sessionId\":\"c1\"}\nJ\n"
        };
        let script = dir.join("copilot");
        std::fs::write(&script, format!("#!/bin/sh\nD='{d}'\ncase \"$1\" in --version) echo 'copilot 1.0'; exit 0;; esac\n: > \"$D/args.txt\"\nfor a in \"$@\"; do printf '%s\\n' \"$a\" >> \"$D/args.txt\"; done\n{body}", d = dir.display())).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        script
    }

    fn set_agents(cx: &AppCx, claude: Option<&Path>, copilot: Option<&Path>) {
        cx.services
            .settings
            .lock()
            .unwrap()
            .update(|s| {
                if let Some(p) = claude {
                    s.agents.claude.command = p.display().to_string();
                }
                if let Some(p) = copilot {
                    s.agents.copilot.command = p.display().to_string();
                    // these tests are about the advisor way of running it
                    s.agents.copilot.advisor_only = true;
                }
            })
            .unwrap();
    }

    #[test]
    fn an_external_agent_is_chosen_like_in_the_chat_and_writes_the_story_through_the_bridge_with_no_tools() {
        let t = Teller::new("# Own\n\nThe own model.");
        let (mut a, cx, _) = make("tales-agent", &t);
        let dir = temp_dir("tales-agent-bin");
        set_agents(&cx, Some(&fake_claude(&dir, false)), None);
        // one chip: no separate Agent chip any more
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|x| x.label == "Model: same as chat ▾"), "{:?}", g.hits.iter().map(|x| x.label.clone()).collect::<Vec<_>>());
        assert!(!g.hits.iter().any(|x| x.label.starts_with("Agent")));
        click_hit(&mut a, &cx, &g, "Model: same as chat ▾");
        // the chat's list: the first line, then the groups with their state
        let (g, area) = draw(&mut a, &cx, 1280, 720, 1);
        assert_layout(&g, area, "chooser");
        let Some(Modal::Model(m)) = &a.modal else { panic!("the chooser is open") };
        let heads: Vec<String> = m.list.rows.iter().filter_map(|r| if let crate::apps::model_picker::MRow::Head(id) = r { Some(id.clone()) } else { None }).collect();
        assert_eq!(heads, ["claude-code", "copilot"]);
        let at = m.list.rows.iter().position(|r| matches!(r, crate::apps::model_picker::MRow::Model(ag, mi) if ag == "claude-code" && mi.id == "sonnet")).unwrap();
        // a click on its model line chooses agent and model together
        a.handle(&Event::Row(at), &cx);
        assert!(a.modal.is_none());
        assert_eq!((a.prefs.agent.clone(), a.prefs.agent_model.clone()), (Some("claude-code".to_string()), "sonnet".to_string()));
        assert_eq!(a.store.load_prefs().agent.as_deref(), Some("claude-code"), "remembered");
        assert_eq!(a.chip_text(&cx), "Model: Sonnet · Claude Code");
        let (g, _) = draw(&mut a, &cx, 1280, 720, 2);
        assert!(g.hits.iter().any(|x| x.label == "Model: Sonnet · Claude Code ▾"));
        assert!(a.writer_line(&cx).starts_with("agent: Claude Code") && a.writer_line(&cx).contains("no tools"), "{}", a.writer_line(&cx));
        // reopened, the list is on the model in use
        key(&mut a, &cx, Key::Ctrl('g'));
        let Some(Modal::Model(m)) = &a.modal else { panic!("Ctrl+G opens the same list") };
        assert_eq!(m.list.sel, at);
        key(&mut a, &cx, Key::Esc);
        assert_eq!(a.prefs.agent_model, "sonnet");
        // the story is written by the agent, streaming into the page, with our model never asked
        a.opts.pictures = PictureMode::Off; // no pictures: this test looks at the run that wrote the story
        a.idea.set("a fox and the moon");
        a.sync_fields();
        key(&mut a, &cx, Key::Enter);
        assert!(a.is_writing());
        until(&mut a, &cx, "the story", |a| !a.is_writing());
        assert!(a.story_text().contains("Once upon a time a fox lived by the river"), "{}", a.story_text());
        assert_eq!(t.story_calls().len(), 0, "our own model was not asked");
        assert!(a.run_model.starts_with("Claude Code"), "{}", a.run_model);
        let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
        // zero tools: the bridge denies every request (agent-level test), the agent's own tools are switched off, no IDE rules
        assert!(args.contains("--disallowedTools=") && args.contains("Read,") && args.contains("Bash"), "{args}");
        assert!(args.contains("--permission-prompt-tool") && args.contains("--model\nsonnet"), "{args}");
        assert!(args.contains("YOU HAVE NO TOOLS") && args.contains("SAFETY (children"), "the children's safety prompt went along: {args}");
        assert!(args.contains("<<<a fox and the moon>>>") && args.contains("not instructions"), "the topic is fenced as data: {args}");
        assert!(!args.contains("running inside IYAKKI's AI IDE"), "the IDE rules are not added to a story");
        let cwd = std::fs::read_to_string(dir.join("cwd.txt")).unwrap();
        assert!(cwd.trim().ends_with("tales-agent") && std::fs::read_dir(cwd.trim()).unwrap().count() == 0, "it ran in an empty folder of its own: {cwd}");
        assert!(a.saved_id().is_some(), "and the story is saved as always");
        assert!(cx.audit.recent(40).iter().any(|l| l.contains("story.create") && l.contains("agent claude-code")));
    }

    #[test]
    fn an_agent_that_is_not_ready_or_fails_is_explained_and_our_model_writes_instead() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-agent-fall", &t);
        let dir = temp_dir("tales-agent-fall-bin");
        // not ready (known before the run): the reason, at once
        a.prefs.agent = Some("claude-code".into());
        cx.services.ext_status.set("claude-code", iyakki_agent::extagent::Status { installed: false, version: String::new(), auth: iyakki_agent::extagent::Auth::Unknown, mode: iyakki_agent::extagent::Mode::Bridged, chip: "not installed".into(), fix: "Claude Code is not installed (no `claude` program was found).".into() });
        write_story(&mut a, &cx, "a fox");
        assert_eq!(t.story_calls().len(), 1, "our own model wrote it");
        let note = a.note_text().unwrap().to_string();
        assert!(note.contains("Claude Code is not ready") && note.contains("not installed") && note.contains("instead"), "{note}");
        assert!(a.story_text().contains("Once upon a time"));
        // ready on paper, but the run fails: the failure is said and the story is still written by our model
        let t2 = Teller::new(STORY);
        let (mut b, cx2, _) = make("tales-agent-fail", &t2);
        set_agents(&cx2, Some(&fake_claude(&dir, true)), None);
        b.prefs.agent = Some("claude-code".into());
        write_story(&mut b, &cx2, "a fox");
        assert_eq!(t2.story_calls().len(), 1);
        assert!(b.story_text().contains("slept well"));
        let note = b.agent_note.clone().or_else(|| b.note_text().map(str::to_string)).unwrap_or_default();
        assert!(note.contains("Claude Code could not write the story") && note.contains("Writing it with"), "{note}");
        assert!(!b.run_model.starts_with("Claude Code"), "{}", b.run_model);
        // no longer configured at all
        let t3 = Teller::new(STORY);
        let (mut c, cx3, _) = make("tales-agent-gone", &t3);
        c.prefs.agent = Some("custom:nope".into());
        assert_eq!(c.agent_id(&cx3), None);
        write_story(&mut c, &cx3, "a fox");
        assert_eq!(t3.story_calls().len(), 1);
    }

    #[test]
    fn copilot_is_an_advisor_that_writes_text_only_and_says_so_when_it_cannot() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-copilot", &t);
        let dir = temp_dir("tales-copilot-bin");
        set_agents(&cx, None, Some(&fake_copilot(&dir, false)));
        a.open_model_pick(false, &cx);
        let Some(Modal::Model(m)) = &a.modal else { panic!() };
        let i = m.list.rows.iter().position(|r| matches!(r, crate::apps::model_picker::MRow::Head(id) if id == "copilot")).unwrap();
        assert!(a.agent_row(&cx, "copilot").unwrap().advisor);
        a.handle(&Event::Row(i), &cx);
        let note = a.note_text().unwrap().to_string();
        assert!(note.contains("advisor") && note.contains("only write the story as text"), "{note}");
        a.idea.set("a fox");
        a.sync_fields();
        key(&mut a, &cx, Key::Enter);
        until(&mut a, &cx, "the story", |a| !a.is_writing());
        assert!(a.story_text().contains("Once there was a fox"), "{}", a.story_text());
        assert_eq!(t.story_calls().len(), 0);
        let args = std::fs::read_to_string(dir.join("args.txt")).unwrap();
        assert!(args.contains("--deny-tool=write") && args.contains("--deny-tool=shell"), "{args}");
        // an advisor that cannot write: said plainly, and our model writes
        let t2 = Teller::new(STORY);
        let (mut b, cx2, _) = make("tales-copilot-fail", &t2);
        set_agents(&cx2, None, Some(&fake_copilot(&temp_dir("tales-copilot-bin2"), true)));
        b.prefs.agent = Some("copilot".into());
        write_story(&mut b, &cx2, "a fox");
        let note = b.agent_note.clone().or_else(|| b.note_text().map(str::to_string)).unwrap_or_default();
        assert!(note.contains("GitHub Copilot runs as an advisor here (read-only) and could not write the story") && note.contains("not signed in"), "{note}");
        assert_eq!(t2.story_calls().len(), 1);
    }

    #[test]
    fn the_reading_hint_is_one_short_line_with_a_how_that_opens_the_longer_text() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-how", &t);
        a.refresh_tts(&cx);
        write_story(&mut a, &cx, "a fox");
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            a.menu = None;
            let s = crate::shell::scale_for(w, h);
            let (g, area) = draw_at(&mut a, &cx, w, h, s, 0);
            let how = g.hits.iter().find(|x| x.label == "How").unwrap_or_else(|| panic!("How at {w}x{h}")).rect;
            let bar = g.hits.iter().find(|x| x.label == "progress").unwrap().rect;
            assert!(how.y > bar.y, "the hint is under the player at {w}x{h}");
            assert!(how.y + how.h <= h as f32 && how.x + how.w <= w as f32);
            let _ = area;
            click_hit(&mut a, &cx, &g, "How");
            assert_eq!(a.menu, Some(Menu::How));
            let (g, area) = draw_at(&mut a, &cx, w, h, s, 1);
            assert!(g.hits.iter().any(|x| x.label == "Close"), "Close in the panel at {w}x{h}");
            for x in &g.hits {
                assert!(x.rect.y >= area.y - 1.0 && x.rect.y + x.rect.h <= h as f32 + 1.0, "{} inside at {w}x{h}", x.label);
            }
            key(&mut a, &cx, Key::Esc);
            assert_eq!(a.menu, None);
        }
        // the long orange paragraph is not in the main area: only the one line
        assert_eq!(a.read_hint().unwrap().0, "Reading aloud: open IYAKKI in a browser for browser voice, or set up Settings → Voice");
        assert!(st::NO_ENGINE_HELP.len() > 300);
    }

    // ------------------------------------------------------------------ speech engines of this computer (docs/VOICE.md)

    /// Fake speech engines on a temp PATH. `espeak-ng` lists a few voices, logs what it is asked to say (stdin) and its
    /// arguments, "speaks" for a moment, and writes a real WAV when given `-w`. `spd-say` only speaks.
    fn host_engines(dir: &Path, which: &[&str]) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("tone.wav"), encode_wav(&tone(440.0, 120), 16_000)).unwrap();
        std::fs::write(dir.join("voices.txt"), "Pty Language Age/Gender VoiceName File Other\n 5 en-us --/M English_(America) gmw/en-US\n 5 ta --/M Tamil inc/ta\n 5 hi --/M Hindi inc/hi\n").unwrap();
        let mk = |name: &str, body: &str| {
            let p = dir.join(name);
            std::fs::write(&p, format!("#!/bin/sh\nD=$(dirname \"$0\")\n{body}\n")).unwrap();
            std::fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        };
        for w in which {
            match *w {
                "espeak-ng" => mk("espeak-ng", "case \"$1\" in --voices) cat \"$D/voices.txt\"; exit 0;; esac\ncat >> \"$D/said.log\"\necho \"$@\" >> \"$D/args.log\"\nout=''; prev=''; for a in \"$@\"; do [ \"$prev\" = -w ] && out=\"$a\"; prev=\"$a\"; done\nif [ -n \"$out\" ]; then cat \"$D/tone.wav\" > \"$out\"; else sleep 0.1; fi"),
                "spd-say" => mk("spd-say", "case \"$1\" in -L) printf '  NAME LANGUAGE VARIANT\\n English en-US none\\n Tamil ta none\\n'; exit 0;; -C) exit 0;; esac\ncat >> \"$D/said.log\"\necho \"$@\" >> \"$D/args.log\"\nsleep 0.1"),
                _ => panic!("no fake {w}"),
            }
        }
    }

    fn host_env(dir: &Path) -> iyakki_agent::hostengines::Env {
        iyakki_agent::hostengines::Env { path: dir.to_string_lossy().to_string(), os: iyakki_agent::hostengines::Os::Linux, piper_dirs: Vec::new() }
    }

    fn said_in(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("said.log")).unwrap_or_default().lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect()
    }

    #[test]
    fn with_nothing_set_an_engine_found_on_the_computer_reads_aloud_and_can_save_audio() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-host", &t);
        let eng = temp_dir("tales-host-eng");
        host_engines(&eng, &["espeak-ng"]);
        assert_eq!(cx.services.voice_settings().tts, None, "the person set nothing");
        cx.services.host_tts.configure(host_env(&eng));
        cx.services.host_tts.scan_now();
        a.refresh_tts(&cx);
        assert!(matches!(&a.tts, TtsStatus::Ready { engine } if engine.contains("espeak-ng") && engine.contains("found on this computer")), "{:?}", a.tts);
        assert!(a.direct.is_some() && !a.browser_voice && !a.no_audio_file, "it speaks by itself and can write a file");
        assert!(!a.chip_wanted(), "no chip when reading works");
        assert!(a.voices.len() > 1 && a.voices[0].is_empty(), "automatic and the voices of the engine: {:?}", a.voices);
        a.prefs.read_while_writing = false;
        write_story(&mut a, &cx, "a fox");
        click(&mut a, &cx, T_PLAY);
        until(&mut a, &cx, "the reading to end", |a| a.reader_status() == Status::Done);
        let said = said_in(&eng);
        assert_eq!(said[0], "The Brave Little Fox", "{said:?}");
        assert!(said.join(" ").contains("She walked home with a quiet heart"), "{said:?}");
        let args = std::fs::read_to_string(eng.join("args.log")).unwrap();
        assert!(args.lines().all(|l| l.contains("-v en-us") && l.contains("--stdin") && !l.contains("fox")), "the story is never in the arguments: {args}");
        // Save audio goes through the engine's file output
        click(&mut a, &cx, T_SAVE_AUDIO);
        until(&mut a, &cx, "the audio to be saved", |a| a.audio_job.is_none());
        assert!(a.note_text().is_some_and(|n| n.contains("Audio saved")), "{:?}", a.note_text());
        assert!(std::fs::read_to_string(eng.join("args.log")).unwrap().contains("-w "), "espeak-ng wrote the file");
    }

    #[test]
    fn a_play_only_engine_reads_aloud_and_save_audio_says_it_is_unavailable() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-spd", &t);
        let eng = temp_dir("tales-spd-eng");
        host_engines(&eng, &["spd-say"]);
        cx.services.host_tts.configure(host_env(&eng));
        cx.services.host_tts.scan_now();
        a.refresh_tts(&cx);
        assert!(matches!(a.tts, TtsStatus::Ready { .. }) && a.direct.is_some() && a.no_audio_file, "{:?}", a.tts);
        a.prefs.read_while_writing = false;
        write_story(&mut a, &cx, "a fox");
        click(&mut a, &cx, T_PLAY);
        until(&mut a, &cx, "the reading to end", |a| a.reader_status() == Status::Done);
        assert_eq!(said_in(&eng)[0], "The Brave Little Fox");
        click(&mut a, &cx, T_SAVE_AUDIO);
        assert!(a.note_text().is_some_and(|n| n.contains("plays the speech itself") && n.contains("cannot write an audio file")), "{:?}", a.note_text());
        assert!(a.audio_job.is_none());
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(g.hits.iter().any(|h| h.label == "Save audio"), "the button is there (dimmed), and says why when pressed");
    }

    #[test]
    fn the_text_only_message_is_one_chip_that_sets_reading_aloud_up() {
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-chip", &t);
        let eng = temp_dir("tales-chip-eng");
        std::fs::create_dir_all(&eng).unwrap();
        cx.services.host_tts.configure(host_env(&eng)); // nothing installed
        cx.services.host_tts.scan_now();
        a.refresh_tts(&cx);
        assert_eq!(a.tts, TtsStatus::NoEngine);
        assert!(a.chip_wanted());
        write_story(&mut a, &cx, "a fox");
        for (w, h) in [(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            for pane_open in [false, true] {
                a.pane_open = pane_open;
                let (g, area) = draw(&mut a, &cx, w, h, 0);
                let what = format!("the chip at {w}x{h} (composer open {pane_open})");
                assert_layout(&g, area, &what);
                let chips: Vec<&crate::ui::Hit> = g.hits.iter().filter(|x| x.label == "Turn on reading aloud").collect();
                assert_eq!(chips.len(), 1, "{what}: exactly one chip");
                assert!(chips[0].rect.w >= 100.0 && chips[0].rect.h >= 20.0, "{what}: {:?}", chips[0].rect);
                assert_eq!(chips[0].ev, Event::Tab(T_TURN_ON));
                assert!(g.hits.iter().any(|x| x.label == "How"), "{what}: How is still there");
                let bar = g.hits.iter().find(|x| x.label == "progress").map(|x| x.rect);
                if let Some(bar) = bar {
                    assert!(chips[0].rect.y >= bar.y + bar.h - 1.0, "{what}: the chip is under the player");
                }
            }
        }
        // pressing it with nothing installed: looked for again, then Settings → Voice is asked for, saying why
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        click_hit(&mut a, &cx, &g, "Turn on reading aloud");
        assert!(a.turning_on);
        until(&mut a, &cx, "the look to end", |a| !a.turning_on);
        assert_eq!(a.take_request(), Some(("settings".to_string(), "voice".to_string())));
        assert!(a.note_text().is_some_and(|n| n.contains("No speech engine was found")), "{:?}", a.note_text());
        // an engine installed meanwhile is found and used, nothing to choose
        host_engines(&eng, &["espeak-ng"]);
        click(&mut a, &cx, T_TURN_ON);
        until(&mut a, &cx, "the look to end", |a| !a.turning_on);
        assert!(matches!(a.tts, TtsStatus::Ready { .. }) && a.take_request().is_none(), "{:?}", a.tts);
        assert!(a.note_text().is_some_and(|n| n.contains("Reading aloud with") && n.contains("Press play")), "{:?}", a.note_text());
        let (g, _) = draw(&mut a, &cx, 1280, 720, 0);
        assert!(!g.hits.iter().any(|x| x.label == "Turn on reading aloud"), "the chip is gone once reading works");
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("story.turn_on_reading"));
    }

    #[test]
    fn in_the_desktop_window_without_speech_the_chip_offers_the_browser_voice_helper() {
        use iyakki_agent::hostvoice::Front;
        let t = Teller::new(STORY);
        let (mut a, cx, _) = make("tales-helper", &t);
        let eng = temp_dir("tales-helper-eng");
        std::fs::create_dir_all(&eng).unwrap();
        let opener = eng.join("fakeopen");
        std::fs::write(&opener, format!("#!/bin/sh\nprintf '%s' \"$1\" > {}/opened\n", eng.display())).unwrap();
        std::fs::set_permissions(&opener, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        cx.services.host_tts.configure(host_env(&eng));
        cx.services.host_tts.set_opener(Some(vec![opener.to_string_lossy().to_string()]));
        let hv = HostVoice::current();
        hv.connect(Front::Window);
        hv.set_unsupported("this window's web engine has no speech synthesis");
        a.refresh_tts(&cx);
        assert!(matches!(a.tts, TtsStatus::NoEngine), "{:?}", a.tts);
        click(&mut a, &cx, T_TURN_ON);
        until(&mut a, &cx, "the look to end", |a| !a.turning_on);
        assert!(a.take_request().is_none(), "the helper is offered, Settings is not opened");
        assert!(a.note_text().is_some_and(|n| n.contains("voice helper is opening in your browser")), "{:?}", a.note_text());
        assert_eq!(cx.services.host_tts.helper_state(), Some(iyakki_agent::voicehelper::HelperState::Waiting));
        let url = (0..200).find_map(|_| { std::thread::sleep(Duration::from_millis(20)); std::fs::read_to_string(eng.join("opened")).ok().filter(|u| !u.is_empty()) }).expect("the browser was asked to open the page");
        assert!(url.starts_with("http://127.0.0.1:") && url.contains("/?t="), "{url}");
        cx.services.host_tts.helper_stop();
        assert!(!hv.helper_active());
    }
}
