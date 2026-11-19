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

pub use iyakki_shared::power::{PowerSettings};
pub use iyakki_shared::power::{ButtonAction, LidAction, Profile, SLEEP_CHOICES};
use std::path::{Path, PathBuf};

pub use iyakki_shared::power::{Caps, Idle, PowerEnv, lid_step, button_press, idle_action, limits, sleep_listing, Lid, Do, FanCard};



/// The fan control card, or `None` when this computer has no fan IYAKKI could run. `dir` is the settings folder the service reads.
pub fn fan_card(env: &PowerEnv, dir: &Path) -> Option<FanCard> {
    if !iyakki_drivers::fanservice::supported(&env.root) {
        return None;
    }
    let s = iyakki_drivers::fanservice::load(dir);
    let rep = iyakki_drivers::fanservice::status(dir);
    Some(FanCard { enabled: s.enabled, line: format!("{}. {}", s.describe(), rep) })
}

/// Turns fan control on or off (the same file the chat tool `fan_control_set` writes; init applies it within seconds). Returns the new state.
pub fn fan_toggle(env: &PowerEnv, dir: &Path) -> Result<bool, String> {
    if !iyakki_drivers::fanservice::supported(&env.root) {
        return Err("This computer has no applesmc fan with a temperature input, so there is nothing to control.".into());
    }
    let mut s = iyakki_drivers::fanservice::load(dir);
    s.enabled = !s.enabled;
    iyakki_drivers::fanservice::save(dir, &s)?;
    Ok(s.enabled)
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
