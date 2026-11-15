//! Power settings (F-PWR-01): the power profile through the CPU's frequency governor and energy preference, sleep after idle, and what
//! this machine can really do.
//!
//! Everything is read from and written to the kernel's own files under `/sys` (a made-up folder in tests): no program is started.
//!
//! * **Profile**: performance, balanced (the default) or power saver. For each CPU policy under `devices/system/cpu/cpufreq` the
//!   governor and, where the CPU has one (Intel and AMD "pstate" drivers), the energy performance preference are set. A machine
//!   without CPU frequency control (most virtual machines) is told so in words and nothing is written.
//! * **Sleep after idle**: off by default. It never fires while something is running ([`idle_action`] takes the reasons the shell
//!   found), unless the person said it may. On a T2 Mac, where the kernel's sleep is not dependable, it is marked experimental: unless
//!   the person turns the experiment on, the idle action is the safe one, the power saver profile with the screen off (the screen saver
//!   settings decide when the screen goes off; this is the profile part), and the chosen profile comes back with the first input.
//! * **Lid and power button** (M37): closing the lid can do nothing (the default), lock the screen or sleep ([`LidAction`]); the power
//!   button can open the power card (the default), lock, sleep or do nothing ([`ButtonAction`]). The lid is read from the kernel's
//!   ACPI button files ([`lid_state`]); only the moment it goes from open to closed counts ([`lid_step`]), so a lid that was already
//!   shut at start does nothing. A sleep that cannot happen (no sleep state, a T2 Mac without the experiment, work in progress and
//!   the person did not allow sleeping then) becomes a lock for the lid and the power card for the button: never a silent no-op.
//!   Screen blank times are Settings -> Screen saver (`screensaver.rs`); the battery is `battery.rs`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where the kernel's files are: `/sys`, or a made-up folder in tests.
#[derive(Clone, Debug)]
pub struct PowerEnv {
    pub root: PathBuf,
    /// Where `/proc` is (the lid switch is there, under `acpi/button/lid`).
    pub proc: PathBuf,
}

impl PowerEnv {
    pub fn real() -> PowerEnv {
        PowerEnv { root: PathBuf::from("/sys"), proc: PathBuf::from("/proc") }
    }
    /// A made-up machine: the `/sys` files are under `root`, the `/proc` files under `root/proc`.
    pub fn at(root: impl Into<PathBuf>) -> PowerEnv {
        let root = root.into();
        PowerEnv { proc: root.join("proc"), root }
    }
    fn cpufreq(&self) -> PathBuf {
        self.root.join("devices/system/cpu/cpufreq")
    }
    fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root.join(rel)).map(|s| s.trim().to_string()).unwrap_or_default()
    }
}

impl Default for PowerEnv {
    fn default() -> PowerEnv {
        PowerEnv::real()
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Performance,
    #[default]
    Balanced,
    PowerSaver,
}

impl Profile {
    pub const ALL: [Profile; 3] = [Profile::Performance, Profile::Balanced, Profile::PowerSaver];
    pub fn label(self) -> &'static str {
        match self {
            Profile::Performance => "Performance",
            Profile::Balanced => "Balanced",
            Profile::PowerSaver => "Power saver",
        }
    }
    pub fn help(self) -> &'static str {
        match self {
            Profile::Performance => "The processor runs fast all the time. Louder fans, shorter battery life.",
            Profile::Balanced => "The processor's own default: fast when there is work, slow when there is not.",
            Profile::PowerSaver => "The processor stays slow unless it really has to. Cooler and quieter, longer battery life.",
        }
    }
}

/// What closing the lid does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LidAction {
    /// Nothing happens (the default, as before).
    #[default]
    Nothing,
    /// The screen locks; apps and runs go on.
    Lock,
    /// The screen locks and the computer sleeps (a lock when sleeping is not possible).
    Sleep,
}

impl LidAction {
    pub const ALL: [LidAction; 3] = [LidAction::Nothing, LidAction::Lock, LidAction::Sleep];
    pub fn label(self) -> &'static str {
        match self {
            LidAction::Nothing => "nothing",
            LidAction::Lock => "lock the screen",
            LidAction::Sleep => "sleep",
        }
    }
    pub fn next(self) -> LidAction {
        LidAction::ALL[(LidAction::ALL.iter().position(|x| *x == self).unwrap_or(0) + 1) % LidAction::ALL.len()]
    }
    pub fn parse(word: &str) -> Option<LidAction> {
        LidAction::ALL.iter().copied().find(|x| format!("{x:?}").to_lowercase() == word.replace('_', ""))
    }
}

/// What the power button does when it is pressed (the key the kernel calls `KEY_POWER`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonAction {
    /// The power card opens and nothing happens until the person chooses (the default, as before).
    #[default]
    Card,
    Lock,
    Sleep,
    /// The press is ignored.
    Nothing,
}

impl ButtonAction {
    pub const ALL: [ButtonAction; 4] = [ButtonAction::Card, ButtonAction::Lock, ButtonAction::Sleep, ButtonAction::Nothing];
    pub fn label(self) -> &'static str {
        match self {
            ButtonAction::Card => "open the power card",
            ButtonAction::Lock => "lock the screen",
            ButtonAction::Sleep => "sleep",
            ButtonAction::Nothing => "nothing",
        }
    }
    pub fn next(self) -> ButtonAction {
        ButtonAction::ALL[(ButtonAction::ALL.iter().position(|x| *x == self).unwrap_or(0) + 1) % ButtonAction::ALL.len()]
    }
    pub fn parse(word: &str) -> Option<ButtonAction> {
        ButtonAction::ALL.iter().copied().find(|x| format!("{x:?}").to_lowercase() == word.replace('_', ""))
    }
}

/// The choices, kept in the settings file (`power`). Every field means "as before" when it is missing from an older file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerSettings {
    pub profile: Profile,
    /// Minutes without input before the computer sleeps. 0 = never (the default).
    pub sleep_after_min: u32,
    /// Sleep even while an install, download, write, job or a model is working. Off by default.
    pub sleep_when_busy: bool,
    /// On a machine where sleep is unreliable (T2 Macs): try real sleep anyway. Off by default.
    pub sleep_experimental: bool,
    /// What closing the lid does (a laptop). Nothing by default.
    pub lid: LidAction,
    /// What the power button does. The power card by default.
    pub button: ButtonAction,
}

/// The minutes the page offers for sleep after idle.
pub const SLEEP_CHOICES: [u32; 6] = [0, 5, 10, 15, 30, 60];

impl PowerSettings {
    /// The next choice of sleep after idle.
    pub fn next_sleep(&self) -> u32 {
        let i = SLEEP_CHOICES.iter().position(|m| *m == self.sleep_after_min).map_or(0, |i| i + 1);
        SLEEP_CHOICES[i % SLEEP_CHOICES.len()]
    }
}

/// What this machine can do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Caps {
    /// The CPU's frequency can be controlled (cpufreq policies exist).
    pub profiles: bool,
    /// The CPU also has an energy performance preference.
    pub energy_preference: bool,
    /// The kernel offers a sleep state (`freeze` or `mem`).
    pub sleep: bool,
    /// An Apple T2 Mac: the kernel's sleep is not dependable there.
    pub t2: bool,
    /// The machine has a battery (a laptop).
    pub laptop: bool,
    /// The kernel shows a lid switch (`/proc/acpi/button/lid/*/state`).
    pub lid: bool,
}

pub fn caps(env: &PowerEnv) -> Caps {
    let mut c = Caps::default();
    for p in policies(env) {
        c.profiles = true;
        if p.join("energy_performance_preference").is_file() {
            c.energy_preference = true;
        }
    }
    c.sleep = sleep_listing(&env.read("power/state"));
    c.t2 = iyakki_drivers::macboard::is_t2(&env.read("class/dmi/id/product_name")) && env.read("class/dmi/id/sys_vendor").starts_with("Apple");
    c.laptop = crate::battery::read(&crate::battery::BatteryEnv::at(&env.root)).shown();
    c.lid = lid_state(&env.proc).is_some();
    c
}

/// The lid switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lid {
    Open,
    Closed,
}

/// The lid as the kernel's ACPI button driver shows it (`state:      open` or `closed` in `acpi/button/lid/<name>/state`), `None` when
/// this computer has no such switch. With several, any closed one counts as closed.
pub fn lid_state(proc_root: &Path) -> Option<Lid> {
    let mut seen = None;
    for e in std::fs::read_dir(proc_root.join("acpi/button/lid")).ok()?.flatten() {
        let text = std::fs::read_to_string(e.path().join("state")).unwrap_or_default();
        match text.split(':').nth(1).map(str::trim) {
            Some("closed") => return Some(Lid::Closed),
            Some("open") => seen = Some(Lid::Open),
            _ => {}
        }
    }
    seen
}

/// What the shell does about a press or a lid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Do {
    Nothing,
    Lock,
    Sleep,
    /// Open the power card.
    Card,
}

fn can_sleep_now(s: &PowerSettings, caps: &Caps, busy: &[String]) -> bool {
    caps.sleep && !(caps.t2 && !s.sleep_experimental) && (busy.is_empty() || s.sleep_when_busy)
}

/// Is a screen other than the laptop's own connected (`class/drm/card*-<connector>/status` says `connected` for a connector that is
/// not eDP, LVDS or DSI)? Closing the lid then is "I work on the other screen", not "I am leaving".
pub fn external_display(sys: &Path) -> bool {
    std::fs::read_dir(sys.join("class/drm")).into_iter().flatten().flatten().any(|e| {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some((_, connector)) = name.split_once('-') else { return false };
        let internal = ["eDP", "LVDS", "DSI"].iter().any(|p| connector.starts_with(p));
        !internal && std::fs::read_to_string(e.path().join("status")).is_ok_and(|s| s.trim() == "connected")
    })
}

/// The lid went from `prev` to `now`. Only open to closed acts; a lid that is shut when the shell starts (`prev` unknown) does not.
/// With another screen connected (`docked`) a sleep becomes a lock: the person is still working.
pub fn lid_step(prev: Option<Lid>, now: Option<Lid>, s: &PowerSettings, caps: &Caps, busy: &[String], docked: bool) -> Do {
    if !(prev == Some(Lid::Open) && now == Some(Lid::Closed)) {
        return Do::Nothing;
    }
    match s.lid {
        LidAction::Nothing => Do::Nothing,
        LidAction::Lock => Do::Lock,
        LidAction::Sleep if !docked && can_sleep_now(s, caps, busy) => Do::Sleep,
        LidAction::Sleep => Do::Lock,
    }
}

/// The power button was pressed.
pub fn button_press(s: &PowerSettings, caps: &Caps, busy: &[String]) -> Do {
    match s.button {
        ButtonAction::Card => Do::Card,
        ButtonAction::Nothing => Do::Nothing,
        ButtonAction::Lock => Do::Lock,
        ButtonAction::Sleep if can_sleep_now(s, caps, busy) => Do::Sleep,
        ButtonAction::Sleep => Do::Card,
    }
}

/// True when `/sys/power/state` offers `freeze` or `mem` (the same choice as the program that suspends: `disk` is never used).
pub fn sleep_listing(listing: &str) -> bool {
    listing.split_whitespace().any(|t| t == "freeze" || t == "mem")
}

fn policies(env: &PowerEnv) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(env.cpufreq())
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("policy") && e.path().join("scaling_governor").is_file())
        .map(|e| e.path())
        .collect();
    v.sort();
    v
}

fn words(p: &Path, name: &str) -> Vec<String> {
    std::fs::read_to_string(p.join(name)).unwrap_or_default().split_whitespace().map(str::to_string).collect()
}

/// What to write for a profile on one policy: (governor, energy preference).
fn choice(profile: Profile, governors: &[String], prefs: &[String]) -> (Option<String>, Option<String>) {
    let has = |l: &[String], w: &str| l.iter().any(|x| x == w);
    let first = |l: &[String], ws: &[&str]| ws.iter().find(|w| has(l, w)).map(|w| w.to_string());
    let epp = |ws: &[&str]| first(prefs, ws);
    match profile {
        Profile::Performance => (first(governors, &["performance"]), epp(&["performance"])),
        // with an energy preference the governor stays `powersave` (the pstate drivers' own default) and the preference does the work
        Profile::Balanced => match epp(&["balance_performance", "default"]) {
            Some(e) => (first(governors, &["powersave", "schedutil", "ondemand"]), Some(e)),
            None => (first(governors, &["schedutil", "ondemand", "conservative", "powersave"]), None),
        },
        Profile::PowerSaver => (first(governors, &["powersave", "conservative"]), epp(&["power", "balance_power"])),
    }
}

/// What a profile would write.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    /// (file, value)
    pub writes: Vec<(PathBuf, String)>,
}

/// The writes for `profile` on this machine. `Err` says in words why nothing can be done.
pub fn plan(env: &PowerEnv, profile: Profile) -> Result<Plan, String> {
    let pols = policies(env);
    if pols.is_empty() {
        return Err("This computer does not let IYAKKI change the processor's speed (no CPU frequency control; virtual machines often have none).".into());
    }
    let mut out = Plan::default();
    for p in pols {
        let (gov, epp) = choice(profile, &words(&p, "scaling_available_governors"), &words(&p, "energy_performance_available_preferences"));
        if let Some(g) = gov {
            out.writes.push((p.join("scaling_governor"), g));
        }
        if let Some(e) = epp {
            out.writes.push((p.join("energy_performance_preference"), e));
        }
    }
    if out.writes.is_empty() {
        return Err(format!("This processor offers no setting that matches \"{}\".", profile.label()));
    }
    Ok(out)
}

/// Write a plan and read each file back. Returns the number of files that now hold the value, or what failed.
pub fn apply(p: &Plan) -> Result<usize, String> {
    let mut bad = Vec::new();
    let mut ok = 0;
    for (f, v) in &p.writes {
        let name = f.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match std::fs::write(f, v) {
            Err(e) => bad.push(format!("{name}: {e}")),
            Ok(()) if std::fs::read_to_string(f).map(|s| s.trim() == v).unwrap_or(false) => ok += 1,
            Ok(()) => bad.push(format!("{name}: the kernel did not keep {v}")),
        }
    }
    if bad.is_empty() { Ok(ok) } else { Err(format!("Some processor settings could not be changed ({}).", bad.join("; "))) }
}

/// Set the profile now.
pub fn set_profile(env: &PowerEnv, profile: Profile) -> Result<usize, String> {
    apply(&plan(env, profile)?)
}

/// The profile the first CPU policy is in now, or `None` when the machine has no CPU frequency control or the values are not one
/// of ours.
pub fn current(env: &PowerEnv) -> Option<Profile> {
    let p = policies(env).into_iter().next()?;
    let gov = std::fs::read_to_string(p.join("scaling_governor")).unwrap_or_default().trim().to_string();
    let epp = std::fs::read_to_string(p.join("energy_performance_preference")).unwrap_or_default().trim().to_string();
    Some(match (gov.as_str(), epp.as_str()) {
        ("performance", _) => Profile::Performance,
        ("powersave", "power") | ("powersave", "balance_power") | ("conservative", _) => Profile::PowerSaver,
        _ => Profile::Balanced,
    })
}

/// What to do about being idle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Idle {
    Nothing,
    /// Lock the session and suspend.
    Sleep,
    /// Sleep is off the table here (a T2 Mac without the experiment): the power saver profile until the next input.
    SaverProfile,
}

/// What to do after `idle_secs` without input. `busy` are the reasons the shell found for work in progress (an install, a download, a
/// write, a job, a model answering, an unfinished update...).
pub fn idle_action(s: &PowerSettings, caps: &Caps, idle_secs: u64, busy: &[String]) -> Idle {
    if s.sleep_after_min == 0 || idle_secs < u64::from(s.sleep_after_min) * 60 {
        return Idle::Nothing;
    }
    if !busy.is_empty() && !s.sleep_when_busy {
        return Idle::Nothing;
    }
    if caps.t2 && !s.sleep_experimental {
        return if caps.profiles { Idle::SaverProfile } else { Idle::Nothing };
    }
    if caps.sleep { Idle::Sleep } else { Idle::Nothing }
}

/// Plain lines for the page about what this machine cannot do.
pub fn limits(c: &Caps) -> Vec<String> {
    let mut v = Vec::new();
    if !c.profiles {
        v.push("Power profile: not available. This computer does not let IYAKKI change the processor's speed.".to_string());
    }
    if !c.sleep {
        v.push("Sleep: not available. The kernel on this computer offers no sleep state.".to_string());
    } else if c.t2 {
        v.push("Sleep: experimental on this Mac. Sleep and wake are not dependable on Apple T2 Macs. By default it only switches to the power saver profile when idle.".to_string());
    }
    if c.laptop && !c.lid {
        v.push("Lid: this computer shows no lid switch that IYAKKI can read, so closing the lid does nothing here.".to_string());
    }
    v
}

/// Set the saved profile (at start). Does nothing for the default profile on a machine that is already in it, and never fails loudly:
/// the answer is a line for the log.
pub fn apply_saved(env: &PowerEnv, s: &PowerSettings) -> Result<String, String> {
    if current(env) == Some(s.profile) {
        return Ok(format!("power profile {} already set", s.profile.label()));
    }
    set_profile(env, s.profile).map(|n| format!("power profile {} set ({n} values)", s.profile.label()))
}

#[cfg(test)]
#[path = "power_tests.rs"]
mod tests;
