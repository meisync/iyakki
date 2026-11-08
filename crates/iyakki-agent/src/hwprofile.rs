//! Per-computer profiles: one install (a data volume on an external disk) boots on several computers, and the settings that belong to a
//! COMPUTER (its sound output, display size, keyboard and pointer, microphone, GPU choice) are kept apart from the settings that belong to
//! the PERSON (models, providers, voice engines, store, firewall, time zone, host name, accounts, vault).
//!
//! * [`identify`] turns the DMI model fields (vendor, product, version, board; never a serial number or the machine's unique id) into a
//!   short stable id and a readable label. The hardware facts come through [`HardwareFacts`] (the real one reads through `iyakki-hwinfo`,
//!   the only module that reads the kernel's files; tests use a fake).
//! * [`PER_COMPUTER`] is DATA: one line per settings path (a JSON pointer into `settings.json`) that belongs to a computer, with the
//!   category it belongs to and why. Adding a key is one line.
//! * [`Computers`] keeps `<data>/computers/<id>.json` (atomic writes, a damaged file moved aside, a size cap) and `computers/current`.
//!   `SettingsStore` calls [`Computers::apply`] once when it is attached and [`Computers::record`] after every change, so the Settings UI,
//!   the dev config command and the chat tools all land in the profile through the one `update` path. The global `settings.json` keeps
//!   the LAST value as the fallback for a computer without a profile.
//! * A few per-computer choices do not live in `settings.json` (the display size is a file per account, the Lemonade GPU/CPU choice a file
//!   in the data folder): those go through the small [`extra_get`]/[`extra_set`] hooks into the same profile.
//!
//! Never lose a value: a missing, damaged or newer profile falls back to the global values.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use iyakki_hwinfo::DmiIdentity;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::settings::Settings;

/// The kinds of settings a person can choose to keep per computer or share between all of them (Settings -> This computer).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Sound,
    Display,
    Keyboard,
    Microphone,
    Gpu,
    Power,
}

impl Category {
    pub const ALL: [Category; 6] = [Category::Sound, Category::Display, Category::Keyboard, Category::Microphone, Category::Gpu, Category::Power];

    pub fn key(self) -> &'static str {
        match self {
            Category::Sound => "sound",
            Category::Display => "display",
            Category::Keyboard => "keyboard",
            Category::Microphone => "microphone",
            Category::Gpu => "gpu",
            Category::Power => "power",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Category::Sound => "Sound",
            Category::Display => "Display",
            Category::Keyboard => "Keyboard and pointer",
            Category::Microphone => "Microphone",
            Category::Gpu => "GPU",
            Category::Power => "Power and battery",
        }
    }

    pub fn parse(s: &str) -> Option<Category> {
        let w = s.trim().to_ascii_lowercase();
        Category::ALL.into_iter().find(|c| c.key() == w || c.label().to_ascii_lowercase() == w)
    }
}

/// One setting that belongs to a computer.
pub struct Item {
    /// A JSON pointer into the settings file (`/sound/output`).
    pub pointer: &'static str,
    pub category: Category,
    /// Why it belongs to a computer and not to the person.
    pub why: &'static str,
}

const fn item(pointer: &'static str, category: Category, why: &'static str) -> Item {
    Item { pointer, category, why }
}

const SND: &str = "which speaker or card exists, and how loud it is, depends on the computer";
const KBD: &str = "the keyboard and pointer are the computer's: an Apple keyboard needs other choices than a PC one";
const PWR: &str = "a laptop and a desktop need other power choices";

/// The settings paths that belong to a computer. Everything NOT listed stays with the person (models, providers, voice engines,
/// store, firewall, time zone, host name, accounts, vault, per-kind sound levels). Every pointer must exist in the default settings
/// (a test checks it), so a typo or a renamed field is caught.
pub const PER_COMPUTER: &[Item] = &[
    item("/sound/output", Category::Sound, "the output key names a card of this computer (PCH:3)"),
    item("/sound/volume_pct", Category::Sound, SND),
    item("/sound/muted", Category::Sound, SND),
    item("/sound/levels", Category::Sound, "the last volume of each output; the outputs are this computer's cards"),
    item("/sound/boosts", Category::Sound, "a boost in dB for a card whose codec has a low amplifier gain"),
    item("/sound/volume_modes", Category::Sound, "which volume control a card uses"),
    item("/sound/mixer", Category::Sound, "the IYAKKI mixer is chosen per machine (T2 Macs need it, others may not)"),
    item("/sound/web_mix", Category::Sound, "web sound through the mixer follows the mixer"),
    item("/sound/highlight_offset_ms", Category::Sound, "the delay a card adds to read-aloud word highlighting"),
    item("/voice/play_cmd", Category::Sound, "the play command names this computer's sound device"),
    item("/voice/mic", Category::Microphone, "the microphone is an ALSA id of this computer (plughw:1,0)"),
    item("/voice/record_cmd", Category::Microphone, "the record command names this computer's microphone"),
    item("/screensaver_signin", Category::Display, "the sign-in saver timings depend on the screen"),
    item("/keyboard/profile", Category::Keyboard, KBD),
    item("/keyboard/fn_mode", Category::Keyboard, KBD),
    item("/keyboard/iso_layout", Category::Keyboard, KBD),
    item("/keyboard/swap_opt_cmd", Category::Keyboard, KBD),
    item("/keyboard/trackpad_tap", Category::Keyboard, KBD),
    item("/keyboard/trackpad_natural", Category::Keyboard, KBD),
    item("/keyboard/scroll_speed_pct", Category::Keyboard, KBD),
    item("/keyboard/wheel_natural", Category::Keyboard, KBD),
    item("/keyboard/scroll_smooth", Category::Keyboard, KBD),
    item("/keyboard/scroll_momentum", Category::Keyboard, KBD),
    item("/keyboard/wheel_lines", Category::Keyboard, KBD),
    item("/keyboard/repeat_delay_ms", Category::Keyboard, KBD),
    item("/keyboard/repeat_rate", Category::Keyboard, KBD),
    item("/keyboard/touchbar", Category::Keyboard, "only some Macs have a Touch Bar"),
    item("/keyboard/touchbar_strips", Category::Keyboard, "only some Macs have a Touch Bar"),
    item("/keyboard/touchbar_flip", Category::Keyboard, "only some Macs have a Touch Bar"),
    item("/power/profile", Category::Power, PWR),
    item("/power/sleep_after_min", Category::Power, PWR),
    item("/power/sleep_when_busy", Category::Power, PWR),
    item("/power/sleep_experimental", Category::Power, "real sleep works on some computers only (not on T2 Macs)"),
    item("/battery/hide_percent", Category::Power, "only laptops have a battery"),
    item("/battery/no_warnings", Category::Power, "only laptops have a battery"),
    item("/battery/no_shutdown", Category::Power, "only laptops have a battery"),
];

/// Per-computer choices kept outside `settings.json`: the display size of an account is `display.size:<account>`, the Lemonade
/// GPU/CPU choice `gpu.lemonade_compute`.
pub const EXTRA_DISPLAY_PREFIX: &str = "display.size:";
pub const EXTRA_GPU: &str = "gpu.lemonade_compute";

fn extra_category(key: &str) -> Option<Category> {
    if key.starts_with(EXTRA_DISPLAY_PREFIX) {
        Some(Category::Display)
    } else if key == EXTRA_GPU {
        Some(Category::Gpu)
    } else {
        None
    }
}

// ------------------------------------------------------------------------------------------------------------------ identity

/// What the computer says about itself (a fake in tests).
pub trait HardwareFacts: Send + Sync {
    fn dmi(&self) -> DmiIdentity;
}

/// The real thing, read through `iyakki-hwinfo`.
pub struct RealFacts;

impl HardwareFacts for RealFacts {
    fn dmi(&self) -> DmiIdentity {
        iyakki_hwinfo::dmi_identity(&iyakki_hwinfo::Roots::real())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComputerId {
    /// 10 hex characters, a hash of the model fields.
    pub id: String,
    pub label: String,
}

const PLACEHOLDERS: &[&str] = &["to be filled by o.e.m.", "default string", "system product name", "system version", "not specified", "none", "n/a", "unknown", "o.e.m.", "not applicable", "system manufacturer", "type1productconfigid", "default"];

fn field(v: &Option<String>) -> String {
    let t = v.as_deref().unwrap_or("").split_whitespace().collect::<Vec<_>>().join(" ");
    if PLACEHOLDERS.contains(&t.to_ascii_lowercase().as_str()) { String::new() } else { t }
}

fn short_vendor(v: &str) -> String {
    let mut s = v.to_string();
    for suffix in [" Inc.", " Inc", " Corporation", " Corp.", " Co., Ltd.", " Co.,Ltd."] {
        if let Some(x) = s.strip_suffix(suffix) {
            s = x.to_string();
        }
    }
    s
}

/// A stable id and a label for the computer the facts describe. Virtual machines have DMI fields too (QEMU: vendor `QEMU`), so they get
/// their own key the same way. No DMI at all (an ARM board) gives one key for "this computer".
pub fn identify(facts: &dyn HardwareFacts) -> ComputerId {
    let d = facts.dmi();
    let (vendor, product, version, board) = (field(&d.sys_vendor), field(&d.product_name), field(&d.product_version), field(&d.board_name));
    let norm = [&vendor, &product, &version, &board].iter().map(|s| s.to_lowercase()).collect::<Vec<_>>().join("\u{1f}");
    let mut h = Sha256::new();
    h.update(b"iyakki-computer-v1\n");
    h.update(norm.as_bytes());
    let id: String = h.finalize().iter().take(5).map(|b| format!("{b:02x}")).collect();
    let numeric_version = version.chars().all(|c| c.is_ascii_digit() || c == '.');
    let mut parts = vec![short_vendor(&vendor), product.clone()];
    if !numeric_version {
        parts.push(version.clone());
    }
    parts.retain(|p| !p.is_empty());
    if parts.is_empty() && !board.is_empty() {
        parts.push(board);
    }
    let label = if parts.is_empty() { "This computer (no hardware name)".to_string() } else { parts.join(" ") };
    ComputerId { id, label }
}

// ------------------------------------------------------------------------------------------------------------------ profile files

const MAX_PROFILE_BYTES: u64 = 256 * 1024;
const PROFILE_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub v: u32,
    pub id: String,
    pub label: String,
    pub first_seen: i64,
    pub last_seen: i64,
    pub boots: u32,
    /// JSON pointer into the settings -> value.
    pub values: BTreeMap<String, Value>,
    /// Choices kept outside `settings.json` (see [`EXTRA_GPU`]).
    pub extras: BTreeMap<String, Value>,
}

impl Default for Profile {
    fn default() -> Profile {
        Profile { v: PROFILE_VERSION, id: String::new(), label: String::new(), first_seen: 0, last_seen: 0, boots: 0, values: BTreeMap::new(), extras: BTreeMap::new() }
    }
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct SharingFile {
    v: u32,
    shared: Vec<Category>,
}

/// One writer at a time inside the process (the settings store, the display-size hook and the tools all touch the same files).
static IO: Mutex<()> = Mutex::new(());

fn lock() -> std::sync::MutexGuard<'static, ()> {
    IO.lock().unwrap_or_else(|e| e.into_inner())
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// What a load found.
enum Loaded {
    Ok(Profile),
    Missing,
    /// Unreadable or too large; moved aside.
    Damaged,
}

fn load_profile(dir: &Path, id: &str, now: i64) -> Loaded {
    let path = dir.join(format!("{id}.json"));
    let Ok(meta) = std::fs::metadata(&path) else { return Loaded::Missing };
    let parsed = if meta.len() > MAX_PROFILE_BYTES {
        None
    } else {
        std::fs::read_to_string(&path).ok().and_then(|t| serde_json::from_str::<Profile>(&t).ok()).filter(|p| p.id == id && p.v >= 1)
    };
    match parsed {
        Some(p) => Loaded::Ok(p),
        None => {
            let aside = dir.join(format!("{id}.json.corrupt-{now}"));
            let _ = std::fs::rename(&path, &aside);
            iyakki_schema::report(format!("computer profile {}: damaged; moved to {} and the global settings are used", path.display(), aside.display()));
            Loaded::Damaged
        }
    }
}

fn save_profile(dir: &Path, p: &Profile) -> Result<(), String> {
    let text = serde_json::to_string_pretty(p).map_err(|e| e.to_string())?;
    if text.len() as u64 > MAX_PROFILE_BYTES {
        return Err("the computer profile would be too large".into());
    }
    iyakki_schema::write_atomic(&dir.join(format!("{}.json", p.id)), text.as_bytes(), 0o600)
}

fn defaults_value() -> Value {
    serde_json::to_value(Settings::default()).unwrap_or(Value::Null)
}

/// The per-computer values of `s` (every listed pointer), skipping the categories in `skip`.
fn values_of(s: &Settings, skip: &BTreeSet<Category>) -> BTreeMap<String, Value> {
    let v = serde_json::to_value(s).unwrap_or(Value::Null);
    PER_COMPUTER.iter().filter(|i| !skip.contains(&i.category)).filter_map(|i| v.pointer(i.pointer).map(|x| (i.pointer.to_string(), x.clone()))).collect()
}

/// The detection defaults: what a fresh install has at every listed pointer (keyboard profile automatic, sound output automatic...).
pub fn default_values() -> BTreeMap<String, Value> {
    let d = defaults_value();
    PER_COMPUTER.iter().filter_map(|i| d.pointer(i.pointer).map(|x| (i.pointer.to_string(), x.clone()))).collect()
}

/// Puts `values` into `s` for the categories not in `skip`. A result that no longer parses or validates is refused and `s` is left alone.
fn overlay(s: &mut Settings, values: &BTreeMap<String, Value>, skip: &BTreeSet<Category>) -> bool {
    let Ok(mut v) = serde_json::to_value(&*s) else { return false };
    for i in PER_COMPUTER.iter().filter(|i| !skip.contains(&i.category)) {
        if let (Some(new), Some(slot)) = (values.get(i.pointer), v.pointer_mut(i.pointer)) {
            *slot = new.clone();
        }
    }
    match serde_json::from_value::<Settings>(v) {
        Ok(n) if n.validate().is_ok() => {
            *s = n;
            true
        }
        _ => false,
    }
}

/// The per-computer choices outside `settings.json` that exist now: the Lemonade file and the `ui-<account>.json` display sizes.
fn capture_extras(data: &Path) -> BTreeMap<String, Value> {
    let mut m = BTreeMap::new();
    if let Ok(t) = std::fs::read_to_string(data.join("lemonade-compute")) {
        let w = t.trim().to_ascii_lowercase();
        if w == "gpu" || w == "cpu" {
            m.insert(EXTRA_GPU.to_string(), Value::String(w));
        }
    }
    for e in std::fs::read_dir(data).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(who) = name.strip_prefix("ui-").and_then(|n| n.strip_suffix(".json")) else { continue };
        if let Some(size) = std::fs::read_to_string(e.path()).ok().and_then(|t| serde_json::from_str::<Value>(&t).ok()).and_then(|v| v["size"].as_u64()) {
            m.insert(format!("{EXTRA_DISPLAY_PREFIX}{who}"), Value::from(size));
        }
    }
    m
}

// ------------------------------------------------------------------------------------------------------------------ the computers

/// The computers this install has seen, and the one it runs on now.
#[derive(Clone, Debug)]
pub struct Computers {
    data: PathBuf,
    dir: PathBuf,
    pub me: ComputerId,
    /// This start met a computer it did not know while others were known: the person is told once.
    pub new_computer: bool,
    /// The profile file was damaged and was moved aside.
    pub fell_back: bool,
}

impl Computers {
    /// Recognises the computer and loads or creates its profile (`boots` + 1). The first computer an install ever sees adopts the global
    /// values (they were made on it); a later unknown computer starts from the detection defaults, never from another computer's values.
    pub fn open(data: &Path, facts: &dyn HardwareFacts, now: i64, current: &Settings) -> Computers {
        let me = identify(facts);
        let dir = data.join("computers");
        let _g = lock();
        let _ = std::fs::create_dir_all(&dir);
        let shared = read_sharing(&dir);
        let mut c = Computers { data: data.into(), dir, me: me.clone(), new_computer: false, fell_back: false };
        let mut p = match load_profile(&c.dir, &me.id, now) {
            Loaded::Ok(p) => p,
            Loaded::Damaged => {
                c.fell_back = true;
                c.adopted_profile(now, current, &shared)
            }
            Loaded::Missing => {
                if list_profiles(&c.dir).iter().any(|o| o.id != me.id) {
                    c.new_computer = true;
                    Profile { id: me.id.clone(), label: me.label.clone(), first_seen: now, values: default_values(), ..Profile::default() }
                } else {
                    c.adopted_profile(now, current, &shared)
                }
            }
        };
        // a pointer added to the table after this profile was made takes the value the settings have now
        for (k, v) in values_of(current, &BTreeSet::new()) {
            p.values.entry(k).or_insert(v);
        }
        p.label = me.label.clone();
        p.last_seen = now;
        p.boots = p.boots.saturating_add(1);
        if let Err(e) = save_profile(&c.dir, &p) {
            iyakki_schema::report(format!("computer profile {}: not saved ({e})", me.id));
        }
        let _ = iyakki_schema::write_atomic(&c.dir.join("current"), format!("{}\n", me.id).as_bytes(), 0o600);
        c
    }

    fn adopted_profile(&self, now: i64, current: &Settings, _shared: &BTreeSet<Category>) -> Profile {
        Profile { id: self.me.id.clone(), label: self.me.label.clone(), first_seen: now, values: values_of(current, &BTreeSet::new()), extras: capture_extras(&self.data), ..Profile::default() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The categories kept in the global settings instead of per computer.
    pub fn shared(&self) -> BTreeSet<Category> {
        read_sharing(&self.dir)
    }

    pub fn profile(&self) -> Option<Profile> {
        match load_profile(&self.dir, &self.me.id, 0) {
            Loaded::Ok(p) => Some(p),
            _ => None,
        }
    }

    /// All profiles, the most recently seen first.
    pub fn list(&self) -> Vec<Profile> {
        list_profiles(&self.dir)
    }

    /// Puts this computer's values over `s` (not for shared categories). False: nothing changed (no usable profile).
    pub fn apply(&self, s: &mut Settings) -> bool {
        let _g = lock();
        let skip = read_sharing(&self.dir);
        match load_profile(&self.dir, &self.me.id, 0) {
            Loaded::Ok(p) => overlay(s, &p.values, &skip),
            _ => false,
        }
    }

    /// Writes the per-computer values of `s` that differ from the profile into the profile (after every settings change).
    pub fn record(&self, s: &Settings) -> Result<(), String> {
        let _g = lock();
        let skip = read_sharing(&self.dir);
        let mut p = match load_profile(&self.dir, &self.me.id, 0) {
            Loaded::Ok(p) => p,
            _ => Profile { id: self.me.id.clone(), label: self.me.label.clone(), first_seen: crate::clock::now_unix(), ..Profile::default() },
        };
        let now = values_of(s, &skip);
        let mut changed = false;
        for (k, v) in now {
            if p.values.get(&k) != Some(&v) {
                p.values.insert(k, v);
                changed = true;
            }
        }
        if changed { save_profile(&self.dir, &p) } else { Ok(()) }
    }

    /// Moves a category between "per computer" and "shared by all". Returns the new set.
    pub fn set_shared(&self, cat: Category, shared: bool) -> Result<BTreeSet<Category>, String> {
        let _g = lock();
        let mut set = read_sharing(&self.dir);
        if shared { set.insert(cat) } else { set.remove(&cat) };
        let f = SharingFile { v: 1, shared: set.iter().copied().collect() };
        iyakki_schema::write_atomic(&self.dir.join("sharing.json"), serde_json::to_string_pretty(&f).map_err(|e| e.to_string())?.as_bytes(), 0o600)?;
        Ok(set)
    }

    /// Finds another computer by id, or by label (case-insensitive, a unique part is enough).
    pub fn find_other(&self, name: &str) -> Result<Profile, String> {
        let others: Vec<Profile> = self.list().into_iter().filter(|p| p.id != self.me.id).collect();
        let w = name.trim().to_lowercase();
        if w.is_empty() {
            return Err("name the other computer".into());
        }
        if let Some(p) = others.iter().find(|p| p.id == w || p.label.to_lowercase() == w) {
            return Ok(p.clone());
        }
        let part: Vec<&Profile> = others.iter().filter(|p| p.label.to_lowercase().contains(&w)).collect();
        match part.len() {
            1 => Ok(part[0].clone()),
            0 => Err(format!("no other computer matches `{name}`")),
            _ => Err(format!("`{name}` matches more than one computer; use its id")),
        }
    }

    /// Takes another computer's values over `s` (not for shared categories) and its choices outside the settings file. The caller runs
    /// this inside a settings update, so [`Computers::record`] stores it.
    pub fn copy_from(&self, other_id: &str, s: &mut Settings) -> Result<(), String> {
        if other_id == self.me.id || !valid_id(other_id) {
            return Err("that is not another computer".into());
        }
        let _g = lock();
        let skip = read_sharing(&self.dir);
        let Loaded::Ok(other) = load_profile(&self.dir, other_id, 0) else { return Err("that computer's profile cannot be read".into()) };
        if !overlay(s, &other.values, &skip) {
            return Err("those settings do not fit here".into());
        }
        self.update_profile(|p| {
            p.extras.retain(|k, _| extra_category(k).is_none_or(|c| skip.contains(&c)));
            for (k, v) in &other.extras {
                if extra_category(k).is_some_and(|c| !skip.contains(&c)) {
                    p.extras.insert(k.clone(), v.clone());
                }
            }
        })
    }

    /// Back to the detection defaults for every per-computer category.
    pub fn reset(&self, s: &mut Settings) -> Result<(), String> {
        let _g = lock();
        let skip = read_sharing(&self.dir);
        if !overlay(s, &default_values(), &skip) {
            return Err("the defaults did not apply".into());
        }
        self.update_profile(|p| p.extras.retain(|k, _| extra_category(k).is_none_or(|c| skip.contains(&c))))
    }

    /// Deletes another computer's profile.
    pub fn forget(&self, id: &str) -> Result<(), String> {
        if id == self.me.id || !valid_id(id) {
            return Err("the computer this is running on cannot be forgotten".into());
        }
        let _g = lock();
        std::fs::remove_file(self.dir.join(format!("{id}.json"))).map_err(|e| e.to_string())
    }

    fn update_profile(&self, f: impl FnOnce(&mut Profile)) -> Result<(), String> {
        let mut p = match load_profile(&self.dir, &self.me.id, 0) {
            Loaded::Ok(p) => p,
            _ => Profile { id: self.me.id.clone(), label: self.me.label.clone(), first_seen: crate::clock::now_unix(), ..Profile::default() },
        };
        f(&mut p);
        save_profile(&self.dir, &p)
    }
}

fn read_sharing(dir: &Path) -> BTreeSet<Category> {
    std::fs::read_to_string(dir.join("sharing.json")).ok().and_then(|t| serde_json::from_str::<SharingFile>(&t).ok()).map(|f| f.shared.into_iter().collect()).unwrap_or_default()
}

fn list_profiles(dir: &Path) -> Vec<Profile> {
    let mut v: Vec<Profile> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let id = name.strip_suffix(".json").filter(|i| valid_id(i))?.to_string();
            if e.metadata().ok()?.len() > MAX_PROFILE_BYTES {
                return None;
            }
            let p: Profile = serde_json::from_str(&std::fs::read_to_string(e.path()).ok()?).ok()?;
            (p.id == id).then_some(p)
        })
        .collect();
    v.sort_by(|a, b| b.last_seen.cmp(&a.last_seen).then(a.label.cmp(&b.label)));
    v
}

/// "3 days ago"-style text is the shell's job; this is the plain date for the status text.
pub fn status_text(c: &Computers) -> String {
    let shared = c.shared();
    let mut out = format!("This computer: {} (id {}).\n", c.me.label, c.me.id);
    if let Some(p) = c.profile() {
        out.push_str(&format!("Started {} time(s) on it.\n", p.boots));
    }
    let others: Vec<String> = c.list().into_iter().filter(|p| p.id != c.me.id).map(|p| format!("{} (id {})", p.label, p.id)).collect();
    out.push_str(&if others.is_empty() { "No other computer has used this install yet.\n".to_string() } else { format!("Other computers: {}.\n", others.join("; ")) });
    for cat in Category::ALL {
        out.push_str(&format!("- {}: {}\n", cat.label(), if shared.contains(&cat) { "shared by all computers" } else { "kept per computer" }));
    }
    out
}

// ------------------------------------------------------------------------------------------------------------------ inbox notice

/// The one-time card for a computer the install has not seen before: to every administrator named. Returns whether a note was raised.
pub fn announce_new_computer(svc: &crate::services::Services, c: &Computers, admins: &[String]) -> bool {
    if !c.new_computer {
        return false;
    }
    let body = "Its sound, display and keyboard settings are kept separately from your other computers. Settings -> This computer shows and changes that.";
    for a in admins {
        let _ = svc.notify_once(a, crate::inbox::Kind::Device, &format!("New computer: {}", c.me.label), body, Some(crate::inbox::Link::new("settings", "computer")));
    }
    !admins.is_empty()
}

// ------------------------------------------------------------------------------------------------------------------ outside settings.json

/// The computer this process runs on, for the small hooks below (`None` until the real start attached it).
static ACTIVE: Mutex<Option<Computers>> = Mutex::new(None);

pub fn activate(c: &Computers) {
    *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = Some(c.clone());
}

fn active_for(data: &Path) -> Option<Computers> {
    ACTIVE.lock().unwrap_or_else(|e| e.into_inner()).clone().filter(|c| c.data == data)
}

/// `None`: use the global file (no computer is attached for this data folder, or the category is shared).
/// `Some(Null)`: this computer has no choice yet (the default applies). `Some(v)`: its choice.
pub fn extra_get(data: &Path, key: &str) -> Option<Value> {
    let c = active_for(data)?;
    let cat = extra_category(key)?;
    let _g = lock();
    if read_sharing(&c.dir).contains(&cat) {
        return None;
    }
    match load_profile(&c.dir, &c.me.id, 0) {
        Loaded::Ok(p) => Some(p.extras.get(key).cloned().unwrap_or(Value::Null)),
        _ => None,
    }
}

/// Records a choice made outside the settings file in this computer's profile (the caller also writes the global file, the fallback).
pub fn extra_set(data: &Path, key: &str, v: Value) {
    let Some(c) = active_for(data) else { return };
    let Some(cat) = extra_category(key) else { return };
    let _g = lock();
    if read_sharing(&c.dir).contains(&cat) {
        return;
    }
    if let Err(e) = c.update_profile(|p| {
        p.extras.insert(key.to_string(), v);
    }) {
        log::warn!("computer profile: {e}");
    }
}

pub fn display_size_get(data: &Path, user: &str) -> Option<Option<usize>> {
    extra_get(data, &format!("{EXTRA_DISPLAY_PREFIX}{user}")).map(|v| v.as_u64().map(|n| n as usize))
}

pub fn display_size_set(data: &Path, user: &str, level: usize) {
    extra_set(data, &format!("{EXTRA_DISPLAY_PREFIX}{user}"), Value::from(level as u64));
}

/// Settings state for [`crate::settings::SettingsStore::attach_computers`] at the real start: recognises this computer, overlays its
/// values, and arms the hooks.
pub fn attach_real(svc: &crate::services::Services) -> Option<Computers> {
    let mut st = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
    let c = Computers::open(svc.dir(), &RealFacts, crate::clock::now_unix(), st.get());
    st.attach_computers(c.clone());
    activate(&c);
    Some(c)
}

#[cfg(test)]
pub(crate) fn clear_active_for_test() {
    *ACTIVE.lock().unwrap_or_else(|e| e.into_inner()) = None;
}

// ------------------------------------------------------------------------------------------------------------------ what the page and the tools do

/// The computers of a running install (`None` before the real start attached one, e.g. in a cloud instance without a profile).
pub fn computers_of(svc: &crate::services::Services) -> Option<Computers> {
    svc.settings.lock().unwrap_or_else(|e| e.into_inner()).computers().cloned()
}

fn no_computers() -> String {
    "This install does not keep per-computer settings here".into()
}

/// Share a category between all computers (`shared`) or keep it per computer.
pub fn set_sharing(svc: &crate::services::Services, cat: Category, shared: bool) -> Result<String, String> {
    let mut st = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
    let c = st.computers().cloned().ok_or_else(no_computers)?;
    c.set_shared(cat, shared)?;
    // going back to per computer: this computer's profile takes what the settings hold now
    st.update(|_| {}).map_err(|e| e.to_string())?;
    Ok(if shared { format!("{} is now shared by all computers.", cat.label()) } else { format!("{} is now kept per computer.", cat.label()) })
}

/// Takes another computer's settings over this one's (the categories kept per computer).
pub fn copy_from(svc: &crate::services::Services, name: &str) -> Result<String, String> {
    let mut st = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
    let c = st.computers().cloned().ok_or_else(no_computers)?;
    let other = c.find_other(name)?;
    let mut res = Ok(());
    st.update(|s| res = c.copy_from(&other.id, s)).map_err(|e| e.to_string())?;
    res?;
    Ok(format!("This computer now has the settings of {} (the display size applies at the next sign-in).", other.label))
}

/// Back to the detection defaults for this computer's categories.
pub fn reset(svc: &crate::services::Services) -> Result<String, String> {
    let mut st = svc.settings.lock().unwrap_or_else(|e| e.into_inner());
    let c = st.computers().cloned().ok_or_else(no_computers)?;
    let mut res = Ok(());
    st.update(|s| res = c.reset(s)).map_err(|e| e.to_string())?;
    res?;
    Ok("This computer's sound, display, keyboard, microphone, GPU and power settings are back to automatic.".into())
}

/// Forget another computer: its profile is deleted (it starts fresh if it is used again).
pub fn forget(svc: &crate::services::Services, name: &str) -> Result<String, String> {
    let c = computers_of(svc).ok_or_else(no_computers)?;
    let other = c.find_other(name)?;
    c.forget(&other.id)?;
    Ok(format!("{} is forgotten.", other.label))
}
