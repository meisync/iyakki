//! Accessibility choices that belong to the built-in Browser tile (docs/ACCESSIBILITY.md, "Browser"), and the chat tools for them.
//!
//! The operating system's own choices (screen reader, high contrast, text size, big focus ring, reduced motion) stay where they are
//! (`accessibility`, Settings -> Accessibility); the browser FOLLOWS them. This file holds only what the OS has no word for:
//! whether the quick keys of browse mode are on, whether pages are forced into the high-contrast palette (follow the OS, always, never),
//! a minimum font size, and whether moving pictures stop (follow Reduce motion, always, never). One file per person:
//! `browser-a11y-<name>.json`.
//!
//! Tools: `browser_accessibility_status` (free) shows the effective choices and what the page in the tab offers; `browser_a11y_set`
//! changes the choices above and asks every time. It does not repeat `accessibility_set`: the OS choices are changed there.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;

pub const STATUS_TOOL: &str = "browser_accessibility_status";
pub const SET_TOOL: &str = "browser_a11y_set";

pub const PROMPT_RULE: &str = "\nBROWSER ACCESSIBILITY: browser_accessibility_status shows how the built-in Browser follows the screen reader, high contrast, text size and reduced motion, and its own choices (browse mode quick keys, forced colours, minimum font size, stopping animations). browser_a11y_set changes those and always asks. The operating system's choices are changed with accessibility_set. Pages that run in the full web engine (F) are not covered by any of it, and you cannot hear the voice: never claim a page is accessible, only report what the status says.";

/// The minimum font sizes offered, in pixels at normal zoom (0 = off). The body text of a page is 16.
pub const MIN_FONT_CHOICES: [u16; 8] = [0, 12, 14, 18, 20, 24, 28, 32];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Colours {
    /// Forced colours when the computer is in High contrast (the default).
    #[default]
    Follow,
    On,
    Off,
}

impl Colours {
    pub fn word(self) -> &'static str {
        match self {
            Colours::Follow => "follow the computer",
            Colours::On => "always",
            Colours::Off => "never",
        }
    }
    pub fn parse(s: &str) -> Option<Colours> {
        match s.to_ascii_lowercase().as_str() {
            "follow" => Some(Colours::Follow),
            "on" | "always" => Some(Colours::On),
            "off" | "never" => Some(Colours::Off),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrowserA11y {
    /// Quick keys (H, L, F, B, T, R, 1 to 6) on the page: `None` follows the screen reader (on while it is on).
    pub browse_mode: Option<bool>,
    pub colours: Colours,
    pub min_font_px: u16,
    /// Pictures that move stand still: `None` follows Reduce motion.
    pub stop_animation: Option<bool>,
}

impl Default for BrowserA11y {
    fn default() -> Self {
        BrowserA11y { browse_mode: None, colours: Colours::Follow, min_font_px: 0, stop_animation: None }
    }
}

impl BrowserA11y {
    pub fn sane(mut self) -> BrowserA11y {
        if !MIN_FONT_CHOICES.contains(&self.min_font_px) {
            self.min_font_px = MIN_FONT_CHOICES.iter().copied().find(|c| *c >= self.min_font_px).unwrap_or(32);
        }
        self
    }
}

/// What the browser does now: its own choices with the operating system's applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Effective {
    pub browse_mode: bool,
    pub forced_colors: bool,
    pub min_font_px: u16,
    pub freeze_animation: bool,
    pub screen_reader: bool,
    pub reduce_motion: bool,
    pub big_ring: bool,
    pub os_high_contrast: bool,
}

impl Effective {
    pub fn describe(&self) -> String {
        let on = |b: bool| if b { "on" } else { "off" };
        format!(
            "browse mode quick keys {}; forced high-contrast page colours {}; minimum font size {}; moving pictures {}; big focus ring {}; text size follows Settings -> Display",
            on(self.browse_mode),
            on(self.forced_colors),
            if self.min_font_px == 0 { "off".to_string() } else { format!("{} px", self.min_font_px) },
            if self.freeze_animation { "stopped" } else { "play" },
            on(self.big_ring),
        )
    }
}

fn who(user: &str) -> String {
    user.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect()
}

pub fn path(dir: &Path, user: &str) -> PathBuf {
    dir.join(format!("browser-a11y-{}.json", who(user)))
}

pub fn load(dir: &Path, user: &str) -> BrowserA11y {
    std::fs::read_to_string(path(dir, user)).ok().and_then(|t| serde_json::from_str::<BrowserA11y>(&t).ok()).unwrap_or_default().sane()
}

pub fn save(dir: &Path, user: &str, a: &BrowserA11y) -> Result<(), String> {
    let p = path(dir, user);
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_string(&a.clone().sane()).map_err(|e| e.to_string())?).map_err(|e| format!("could not save: {e}"))?;
    std::fs::rename(&tmp, &p).map_err(|e| format!("could not save: {e}"))
}

/// Apply the operating system's choices to the browser's.
pub fn resolve(own: &BrowserA11y, os: &crate::accessibility::Access, os_high_contrast: bool) -> Effective {
    Effective {
        browse_mode: own.browse_mode.unwrap_or(os.screen_reader),
        forced_colors: match own.colours {
            Colours::On => true,
            Colours::Off => false,
            Colours::Follow => os_high_contrast,
        },
        min_font_px: own.min_font_px,
        freeze_animation: own.stop_animation.unwrap_or(os.reduce_motion),
        screen_reader: os.screen_reader,
        reduce_motion: os.reduce_motion,
        big_ring: os.big_ring,
        os_high_contrast,
    }
}

/// What applies to `user` now (files only).
pub fn effective(dir: &Path, user: &str) -> Effective {
    resolve(&load(dir, user), &crate::accessibility::effective(dir, user), crate::accessibility::high_contrast(dir, user))
}

/// A cheap fingerprint of every file the browser's accessibility depends on: the browser's own, the OS choices and the theme.
pub fn stamp(dir: &Path, user: &str) -> [Option<std::time::SystemTime>; 4] {
    let m = |p: PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    let os = crate::accessibility::stamp(dir, user);
    [m(path(dir, user)), os[0], os[1], os[2]]
}

/// What the Browser tile wrote about the page in the tab: the site, the title and the counts (never the address or the text).
pub fn page_path(dir: &Path, user: &str) -> PathBuf {
    dir.join(format!("browser-a11y-page-{}.txt", who(user)))
}

pub fn write_page_summary(dir: &Path, user: &str, text: &str) {
    let p = page_path(dir, user);
    let tmp = p.with_extension("tmp");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

pub fn read_page_summary(dir: &Path, user: &str) -> Option<String> {
    std::fs::read_to_string(page_path(dir, user)).ok().map(|t| t.trim().to_string()).filter(|t| !t.is_empty())
}

// -------------------------------------------------------------------------------------------------------------------- the tools

pub fn is_tool(n: &str) -> bool {
    n == STATUS_TOOL || n == SET_TOOL
}

pub trait Backend: Send + Sync {
    fn own(&self) -> BrowserA11y;
    fn save(&self, a: &BrowserA11y) -> Result<(), String>;
    fn os(&self) -> (crate::accessibility::Access, bool);
    /// What the page in the active Browser tab offers ("12 links, 3 headings, ..."), when a page is open and the tile can say.
    fn page_summary(&self) -> Option<String>;
    fn audit(&self, action: &str, detail: &str);
}

pub struct BrowserA11yTools {
    inner: Arc<dyn Tools>,
    backend: Arc<dyn Backend>,
}

fn plan(cur: &BrowserA11y, args: &Value) -> Result<BrowserA11y, String> {
    let mut a = cur.clone();
    let mut any = false;
    if let Some(v) = args.get("browse_mode") {
        a.browse_mode = match v {
            Value::Bool(b) => Some(*b),
            Value::String(s) if s == "follow" => None,
            _ => return Err("`browse_mode` must be true, false or \"follow\"".into()),
        };
        any = true;
    }
    if let Some(v) = args.get("forced_colors") {
        a.colours = v.as_str().and_then(Colours::parse).ok_or("`forced_colors` must be \"follow\", \"always\" or \"never\"")?;
        any = true;
    }
    if let Some(v) = args.get("min_font_px") {
        let px = v.as_u64().and_then(|m| u16::try_from(m).ok()).filter(|m| MIN_FONT_CHOICES.contains(m)).ok_or_else(|| format!("`min_font_px` must be one of {MIN_FONT_CHOICES:?} (0 = off)"))?;
        a.min_font_px = px;
        any = true;
    }
    if let Some(v) = args.get("stop_animation") {
        a.stop_animation = match v {
            Value::Bool(b) => Some(*b),
            Value::String(s) if s == "follow" => None,
            _ => return Err("`stop_animation` must be true, false or \"follow\"".into()),
        };
        any = true;
    }
    if !any {
        return Err("say what to change: browse_mode, forced_colors, min_font_px or stop_animation".into());
    }
    Ok(a)
}

impl BrowserA11yTools {
    pub fn new(inner: Arc<dyn Tools>, backend: Arc<dyn Backend>) -> BrowserA11yTools {
        BrowserA11yTools { inner, backend }
    }

    fn status(&self) -> String {
        let (os, hc) = self.backend.os();
        let own = self.backend.own();
        let eff = resolve(&own, &os, hc);
        let page = self.backend.page_summary().map_or("No page is open in the Browser tab (or the tile cannot say).".to_string(), |s| format!("The page in the tab has {s}."));
        format!(
            "{}. Own choices: browse mode {}, page colours {}, stop animations {}. {page} Pages in the full web engine (F) and pages made of canvas or heavy scripts are not covered. Keys: Tab and Shift+Tab walk links and controls, v turns browse mode on or off (then H, 1-6, L, F, B, T, R jump, Shift goes back), Ctrl+F or / in browse mode finds, K lists links, J lists headings, A reads the page, Esc stops reading.",
            eff.describe(),
            own.browse_mode.map_or("follows the screen reader".to_string(), |b| if b { "on".into() } else { "off".into() }),
            own.colours.word(),
            own.stop_animation.map_or("follows Reduce motion".to_string(), |b| if b { "on".into() } else { "off".into() }),
        )
    }
}

impl Tools for BrowserA11yTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_tool(&d.spec.name));
        v.push(ToolDef { spec: ToolSpec { name: STATUS_TOOL.into(), description: "Show how the built-in Browser handles accessibility: quick keys, forced high-contrast page colours, minimum font size, stopped animations, and what the page in the tab offers (links, headings, landmarks, unlabelled fields).".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ToolDef {
            spec: ToolSpec {
                name: SET_TOOL.into(),
                description: "Change the Browser's own accessibility choices (any of them in one call). The person approves a card first. Screen reader, high contrast, text size and reduced motion are changed with accessibility_set.".into(),
                schema: json!({"type": "object", "properties": {
                    "browse_mode": {"description": "true, false or \"follow\" (on while the screen reader is on)"},
                    "forced_colors": {"type": "string", "enum": ["follow", "always", "never"]},
                    "min_font_px": {"type": "integer", "enum": MIN_FONT_CHOICES},
                    "stop_animation": {"description": "true, false or \"follow\" (Reduce motion)"}
                }}),
            },
            risk: Risk::Ask,
            untrusted: false,
        });
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            STATUS_TOOL => Ok(self.status()),
            SET_TOOL => {
                let a = plan(&self.backend.own(), args)?;
                self.backend.save(&a)?;
                let (os, hc) = self.backend.os();
                self.backend.audit("settings.browser_accessibility", &resolve(&a, &os, hc).describe());
                Ok(format!("Saved. It applies to the open page within a second. {}", self.status()))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        match name {
            STATUS_TOOL => "Show the Browser's accessibility choices".into(),
            SET_TOOL => match plan(&self.backend.own(), args) {
                Ok(a) => {
                    let (os, hc) = self.backend.os();
                    format!("Change the Browser's accessibility: {}", resolve(&a, &os, hc).describe())
                }
                Err(_) => "Change the Browser's accessibility choices".into(),
            },
            _ => self.inner.describe(name, args),
        }
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SET_TOOL => Some(match plan(&self.backend.own(), args) {
                Ok(a) => {
                    let (os, hc) = self.backend.os();
                    format!("Now: {}.\nAfter: {}.", resolve(&self.backend.own(), &os, hc).describe(), resolve(&a, &os, hc).describe())
                }
                Err(e) => format!("Cannot change it: {e}"),
            }),
            STATUS_TOOL => None,
            _ => self.inner.preview(name, args),
        }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        match name {
            SET_TOOL => plan(&self.backend.own(), args).err(),
            STATUS_TOOL => None,
            _ => self.inner.refusal(name, args),
        }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        name != SET_TOOL && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == SET_TOOL { Some("it changes how pages are shown and spoken, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
    }
    fn grant_scope(&self, name: &str, args: &Value) -> String {
        self.inner.grant_scope(name, args)
    }
    fn unresolved_work(&self) -> Vec<String> {
        self.inner.unresolved_work()
    }
    fn begin_step(&self) {
        self.inner.begin_step()
    }
    fn should_end_run(&self) -> Option<String> {
        self.inner.should_end_run()
    }
    fn take_diagnosis(&self) -> Option<(String, String)> {
        self.inner.take_diagnosis()
    }
    fn note_external(&self, name: &str, args: &Value, ok: bool) {
        self.inner.note_external(name, args, ok)
    }
    fn take_notes(&self) -> Vec<String> {
        self.inner.take_notes()
    }
    fn jobs_running(&self) -> Vec<String> {
        self.inner.jobs_running()
    }
}

/// The real backend: the data folder and the audit log. The page summary is the small file the Browser tile writes when a page shows.
pub struct RealBackend {
    pub dir: PathBuf,
    pub user: String,
    pub audit: Arc<dyn Fn(&str, &str) + Send + Sync>,
}

impl Backend for RealBackend {
    fn own(&self) -> BrowserA11y {
        load(&self.dir, &self.user)
    }
    fn save(&self, a: &BrowserA11y) -> Result<(), String> {
        save(&self.dir, &self.user, a)
    }
    fn os(&self) -> (crate::accessibility::Access, bool) {
        (crate::accessibility::effective(&self.dir, &self.user), crate::accessibility::high_contrast(&self.dir, &self.user))
    }
    fn page_summary(&self) -> Option<String> {
        read_page_summary(&self.dir, &self.user)
    }
    fn audit(&self, action: &str, detail: &str) {
        (self.audit)(action, detail)
    }
}

#[cfg(test)]
#[path = "browsera11y_tests.rs"]
mod tests;
