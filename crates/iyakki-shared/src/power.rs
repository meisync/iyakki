//! Types moved out of `iyakki_agent::power` (F-SPLIT-03, scripted by dist/split/move_types.py); the agent re-exports them.

use std::path::PathBuf;

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
    pub fn cpufreq(&self) -> PathBuf {
        self.root.join("devices/system/cpu/cpufreq")
    }
    pub fn read(&self, rel: &str) -> String {
        std::fs::read_to_string(self.root.join(rel)).map(|s| s.trim().to_string()).unwrap_or_default()
    }
}

impl Default for PowerEnv {
    fn default() -> PowerEnv {
        PowerEnv::real()
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

/// What to do about being idle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Idle {
    Nothing,
    /// Lock the session and suspend.
    Sleep,
    /// Sleep is off the table here (a T2 Mac without the experiment): the power saver profile until the next input.
    SaverProfile,
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----
use serde::Deserialize;
use serde::Serialize;

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

impl PowerSettings {
    /// The next choice of sleep after idle.
    pub fn next_sleep(&self) -> u32 {
        let i = SLEEP_CHOICES.iter().position(|m| *m == self.sleep_after_min).map_or(0, |i| i + 1);
        SLEEP_CHOICES[i % SLEEP_CHOICES.len()]
    }
}

// ---- moved from iyakki-agent (F-SPLIT-03) ----

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

/// The minutes the page offers for sleep after idle.
pub const SLEEP_CHOICES: [u32; 6] = [0, 5, 10, 15, 30, 60];

// ---- moved from iyakki_agent::power (F-SPLIT-03) ----


/// What the Settings Power page shows of fan control: only on a computer with an applesmc fan (F-DRIVERS-14).
pub struct FanCard {
    pub enabled: bool,
    /// The one-line state: who runs the fans and what the service last reported.
    pub line: String,
}

/// The lid switch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lid {
    Open,
    Closed,
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

/// True when `/sys/power/state` offers `freeze` or `mem` (the same choice as the program that suspends: `disk` is never used).
pub fn sleep_listing(listing: &str) -> bool {
    listing.split_whitespace().any(|t| t == "freeze" || t == "mem")
}

pub fn can_sleep_now(s: &PowerSettings, caps: &Caps, busy: &[String]) -> bool {
    caps.sleep && !(caps.t2 && !s.sleep_experimental) && (busy.is_empty() || s.sleep_when_busy)
}
