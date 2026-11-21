//! The lazy device broker (F-BOOT-04): drivers of a few hardware classes (sound, graphics for compute, Bluetooth) are not loaded at
//! boot but the first time something asks for them, and only when the asker is allowed to. Every decision is written to the audit log.
//!
//! This file holds the decisions and the bookkeeping, with no system calls of its own: the aliases of the devices that were found,
//! the permission check, the loader (`modprobe` in the real boot) and the audit sink are parameters, so the rules run in tests on
//! plain data. What is NOT here: the call from `boot::late_boot` that hands the deferred aliases to a [`Broker`] instead of loading
//! them (the boot order is unchanged until a real machine proves it), and the core API call a person's tile or the chat uses.
//!
//! Safety rules, all tested:
//! * A class not on the lazy list is never held back (the default list is sound and Bluetooth; graphics only when the caller says the
//!   display does not depend on that driver).
//! * An alias that matches no class is the caller's business (boot loads it as before): the broker never swallows it.
//! * A refused request loads nothing and is logged with the asker's name.
//! * A class loads once; a failed load can be asked for again (after [`RETRY_AFTER`]), a loaded one is a no-op.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

/// How long after a failed load the same class may be asked for again.
pub const RETRY_AFTER: Duration = Duration::from_secs(30);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    Audio,
    Gpu,
    Bluetooth,
}

impl Class {
    pub fn name(self) -> &'static str {
        match self {
            Class::Audio => "audio",
            Class::Gpu => "gpu",
            Class::Bluetooth => "bluetooth",
        }
    }
    pub fn parse(s: &str) -> Option<Class> {
        [Class::Audio, Class::Gpu, Class::Bluetooth].into_iter().find(|c| c.name() == s)
    }
    /// The classes held back by default: not graphics, because a display may depend on its driver.
    pub fn default_lazy() -> BTreeSet<Class> {
        [Class::Audio, Class::Bluetooth].into_iter().collect()
    }
}

/// Which class a device alias belongs to, if any.
pub fn class_of(modalias: &str) -> Option<Class> {
    let a = modalias.to_ascii_lowercase();
    if a.starts_with("pci:") {
        // base class 04 multimedia (sound), 03 display, 0d wireless (the Bluetooth radio of some laptops sits here)
        if a.contains("bc04") {
            return Some(Class::Audio);
        }
        if a.contains("bc03") {
            return Some(Class::Gpu);
        }
        return None;
    }
    if a.starts_with("usb:") {
        // interface class 01 audio, e0 wireless controller (Bluetooth)
        if a.contains("ic01") {
            return Some(Class::Audio);
        }
        if a.contains("ice0") {
            return Some(Class::Bluetooth);
        }
        return None;
    }
    if a.starts_with("hdaudio:") || a.starts_with("snd") {
        return Some(Class::Audio);
    }
    if a.starts_with("bluetooth") {
        return Some(Class::Bluetooth);
    }
    None
}

/// Split the found aliases: those to load at boot as before, and those held for the broker (grouped by class, in the order found).
pub fn split(aliases: &[String], lazy: &BTreeSet<Class>) -> (Vec<String>, BTreeMap<Class, Vec<String>>) {
    let mut now = Vec::new();
    let mut held: BTreeMap<Class, Vec<String>> = BTreeMap::new();
    for a in aliases {
        match class_of(a).filter(|c| lazy.contains(c)) {
            Some(c) => held.entry(c).or_default().push(a.clone()),
            None => now.push(a.clone()),
        }
    }
    (now, held)
}

/// The boot switch: `iyakki.lazydrivers=1` on the kernel command line turns the broker on. Off by default: until a boot test on real
/// machines shows sound and Bluetooth come up on first use, every driver loads at boot as before.
pub fn switch_on(cmdline: &str) -> bool {
    crate::boot::cmdline_param(cmdline, "iyakki.lazydrivers") == Some("1")
}

/// What boot does with the aliases it would load after sign-in: with the switch off everything loads (the old behaviour, exactly);
/// with it on, the held classes wait for the broker.
pub fn plan_deferred(aliases: &[String], enabled: bool) -> (Vec<String>, BTreeMap<Class, Vec<String>>) {
    if !enabled {
        return (aliases.to_vec(), BTreeMap::new());
    }
    split(aliases, &Class::default_lazy())
}

/// The app capabilities (`iyakki_core_api::appcaps`) that let an app wake a class. Graphics has none in the catalogue: only the core
/// itself (`who` = [`CORE`]) may wake it.
pub fn capabilities_for(class: Class) -> &'static [&'static str] {
    match class {
        Class::Audio => &["speaker.play", "microphone.listen"],
        Class::Bluetooth => &["bluetooth.use"],
        Class::Gpu => &[],
    }
}

/// The name boot itself uses when it asks.
pub const CORE: &str = "core";

/// A gate over the capability catalogue: the core may wake anything; an app may wake a class when it holds one of the class's
/// capabilities. `holds(app, capability)` is the core's own check (`appcaps::builtin_holds`, or the person's approval record).
pub struct CapGate<F: Fn(&str, &str) -> bool> {
    pub holds: F,
}

impl<F: Fn(&str, &str) -> bool> Gate for CapGate<F> {
    fn allowed(&self, who: &str, class: Class) -> bool {
        who == CORE || capabilities_for(class).iter().any(|c| (self.holds)(who, c))
    }
}

/// What a request came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The drivers of this many aliases were loaded now.
    Loaded(usize),
    /// Loaded earlier: nothing to do.
    AlreadyLoaded,
    /// The asker may not use this class: nothing was loaded.
    Denied,
    /// No device of this class was found (or it is not on the lazy list): nothing to load.
    NothingToLoad,
    /// The loader failed; the class can be asked for again later.
    Failed(String),
    /// Failed a moment ago; try again later.
    TooSoon,
}

impl Outcome {
    /// One plain sentence for a page or the chat.
    pub fn sentence(&self) -> String {
        match self {
            Outcome::Loaded(n) => format!("loaded the drivers for {n} devices"),
            Outcome::AlreadyLoaded => "the drivers are loaded already".into(),
            Outcome::Denied => "not allowed: this app does not hold the permission for that hardware".into(),
            Outcome::NothingToLoad => "nothing to load: no held-back device of this class".into(),
            Outcome::Failed(e) => format!("the drivers did not load: {e}"),
            Outcome::TooSoon => "the drivers failed a moment ago; try again later".into(),
        }
    }
}

/// The text form of a request, for the device API (`Hardware::request_class`): an unknown class name is refused in words.
pub fn answer(class: &str, who: &str, ask: impl FnOnce(Class, &str) -> Outcome) -> String {
    match Class::parse(class) {
        Some(c) => ask(c, who).sentence(),
        None => format!("unknown hardware class {class:?} (audio, gpu or bluetooth)"),
    }
}

/// Is `who` allowed to wake `class`? (In the real system this asks the core's permission check.)
pub trait Gate {
    fn allowed(&self, who: &str, class: Class) -> bool;
}

/// Loads drivers for the aliases (`modprobe -b -a` in the real boot). Returns an error text when it could not.
pub trait Loader {
    fn load(&mut self, aliases: &[String]) -> Result<(), String>;
}

/// One line per decision: who, what class, what came of it.
pub trait Audit {
    fn record(&mut self, line: &str);
}

pub struct Broker<G: Gate, L: Loader, A: Audit> {
    held: BTreeMap<Class, Vec<String>>,
    loaded: BTreeSet<Class>,
    failed_at: BTreeMap<Class, Instant>,
    gate: G,
    loader: L,
    audit: A,
}

impl<G: Gate, L: Loader, A: Audit> Broker<G, L, A> {
    pub fn new(held: BTreeMap<Class, Vec<String>>, gate: G, loader: L, audit: A) -> Self {
        Broker { held, loaded: BTreeSet::new(), failed_at: BTreeMap::new(), gate, loader, audit }
    }

    /// Aliases found later (a device that appeared after boot) for a class that is not loaded yet.
    pub fn add_found(&mut self, class: Class, aliases: &[String]) {
        if !self.loaded.contains(&class) {
            self.held.entry(class).or_default().extend(aliases.iter().cloned());
        }
    }

    pub fn is_loaded(&self, class: Class) -> bool {
        self.loaded.contains(&class)
    }

    pub fn waiting(&self) -> Vec<(Class, usize)> {
        self.held.iter().filter(|(c, v)| !self.loaded.contains(c) && !v.is_empty()).map(|(c, v)| (*c, v.len())).collect()
    }

    /// First use of a class by `who`.
    pub fn request(&mut self, class: Class, who: &str, now: Instant) -> Outcome {
        let who_line: String = who.chars().filter(|c| c.is_ascii_alphanumeric() || "-_.:@".contains(*c)).take(64).collect();
        if !self.gate.allowed(who, class) {
            self.audit.record(&format!("broker: {who_line} asked for {}: denied", class.name()));
            return Outcome::Denied;
        }
        if self.loaded.contains(&class) {
            return Outcome::AlreadyLoaded;
        }
        if let Some(t) = self.failed_at.get(&class) {
            if now.saturating_duration_since(*t) < RETRY_AFTER {
                return Outcome::TooSoon;
            }
        }
        let aliases = self.held.get(&class).cloned().unwrap_or_default();
        if aliases.is_empty() {
            self.audit.record(&format!("broker: {who_line} asked for {}: no device", class.name()));
            return Outcome::NothingToLoad;
        }
        match self.loader.load(&aliases) {
            Ok(()) => {
                self.loaded.insert(class);
                self.failed_at.remove(&class);
                self.audit.record(&format!("broker: {who_line} asked for {}: loaded {} aliases", class.name(), aliases.len()));
                Outcome::Loaded(aliases.len())
            }
            Err(e) => {
                self.failed_at.insert(class, now);
                let e: String = e.lines().next().unwrap_or("").chars().filter(|c| !c.is_control()).take(120).collect();
                self.audit.record(&format!("broker: {who_line} asked for {}: failed ({e})", class.name()));
                Outcome::Failed(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_device_api_answers_in_words_and_refuses_an_unknown_class() {
        assert_eq!(answer("bluetooth", "settings", |c, w| { assert_eq!((c, w), (Class::Bluetooth, "settings")); Outcome::Loaded(3) }), "loaded the drivers for 3 devices");
        assert!(answer("audio", "x", |_, _| Outcome::Denied).starts_with("not allowed"));
        assert!(answer("printer", "x", |_, _| unreachable!()).contains("unknown hardware class"));
    }
    use std::cell::RefCell;
    use std::rc::Rc;

    struct Allow(Vec<(&'static str, Class)>);
    impl Gate for Allow {
        fn allowed(&self, who: &str, class: Class) -> bool {
            self.0.iter().any(|(w, c)| *w == who && *c == class)
        }
    }
    #[derive(Clone, Default)]
    struct Fake {
        calls: Rc<RefCell<Vec<Vec<String>>>>,
        fail: Rc<RefCell<bool>>,
    }
    impl Loader for Fake {
        fn load(&mut self, a: &[String]) -> Result<(), String> {
            self.calls.borrow_mut().push(a.to_vec());
            if *self.fail.borrow() { Err("modprobe: not found\nsecond line".into()) } else { Ok(()) }
        }
    }
    #[derive(Clone, Default)]
    struct Log(Rc<RefCell<Vec<String>>>);
    impl Audit for Log {
        fn record(&mut self, l: &str) {
            self.0.borrow_mut().push(l.to_string());
        }
    }

    fn aliases() -> Vec<String> {
        ["pci:v00008086d00009D71sv0bc04sc03i00", "pci:v00001002d00006861sv0bc03sc00i00", "usb:v8087p0A2Bdc00dsc00dp00icE0isc01ip01in00", "pci:v00008086d00009D2Fsv0bc0Csc03i30", "hdaudio:v10EC0298rev001100a01", "platform:i8042"].iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn aliases_fall_into_classes_and_the_rest_stays_with_boot() {
        let a = aliases();
        let got: Vec<Option<Class>> = a.iter().map(|x| class_of(x)).collect();
        assert_eq!(got, vec![Some(Class::Audio), Some(Class::Gpu), Some(Class::Bluetooth), None, Some(Class::Audio), None]);
        let (now, held) = split(&a, &Class::default_lazy());
        assert_eq!(now.len(), 3, "graphics, the USB host and the keyboard controller load as before: {now:?}");
        assert!(now.iter().any(|x| x.contains("bc03")), "graphics is not lazy by default");
        assert_eq!(held[&Class::Audio].len(), 2);
        assert_eq!(held[&Class::Bluetooth].len(), 1);
        let all: BTreeSet<Class> = [Class::Audio, Class::Gpu, Class::Bluetooth].into_iter().collect();
        assert_eq!(split(&a, &all).1[&Class::Gpu].len(), 1, "graphics is lazy only when asked");
        assert_eq!(split(&a, &BTreeSet::new()).0, a, "no lazy class: everything loads at boot");
    }

    fn broker(allow: Vec<(&'static str, Class)>) -> (Broker<Allow, Fake, Log>, Fake, Log) {
        let (f, l) = (Fake::default(), Log::default());
        let (_, held) = split(&aliases(), &Class::default_lazy());
        (Broker::new(held, Allow(allow), f.clone(), l.clone()), f, l)
    }

    #[test]
    fn first_use_loads_once_and_a_refusal_loads_nothing() {
        let (mut b, f, l) = broker(vec![("player", Class::Audio)]);
        let t = Instant::now();
        assert_eq!(b.waiting(), vec![(Class::Audio, 2), (Class::Bluetooth, 1)]);
        assert_eq!(b.request(Class::Audio, "stranger", t), Outcome::Denied);
        assert_eq!(b.request(Class::Bluetooth, "player", t), Outcome::Denied, "allowed for audio only");
        assert!(f.calls.borrow().is_empty(), "nothing loaded for a refusal");
        assert_eq!(b.request(Class::Audio, "player", t), Outcome::Loaded(2));
        assert_eq!(b.request(Class::Audio, "player", t), Outcome::AlreadyLoaded);
        assert_eq!(f.calls.borrow().len(), 1, "a class loads once");
        assert!(b.is_loaded(Class::Audio) && !b.is_loaded(Class::Bluetooth));
        let log = l.0.borrow().join("\n");
        assert!(log.contains("stranger asked for audio: denied") && log.contains("player asked for audio: loaded 2 aliases"), "{log}");
    }

    #[test]
    fn a_failed_load_can_be_asked_again_only_after_a_pause() {
        let (mut b, f, l) = broker(vec![("player", Class::Bluetooth)]);
        *f.fail.borrow_mut() = true;
        let t = Instant::now();
        assert!(matches!(b.request(Class::Bluetooth, "player", t), Outcome::Failed(e) if e == "modprobe: not found"));
        assert_eq!(b.request(Class::Bluetooth, "player", t + Duration::from_secs(5)), Outcome::TooSoon);
        *f.fail.borrow_mut() = false;
        assert_eq!(b.request(Class::Bluetooth, "player", t + RETRY_AFTER), Outcome::Loaded(1));
        assert!(l.0.borrow().iter().any(|x| x.contains("failed (modprobe: not found")));
    }

    #[test]
    fn no_device_means_nothing_to_load_and_late_devices_join_the_wait() {
        let (mut b, f, _) = broker(vec![("p", Class::Gpu), ("p", Class::Audio)]);
        let t = Instant::now();
        assert_eq!(b.request(Class::Gpu, "p", t), Outcome::NothingToLoad, "graphics was not held back");
        b.add_found(Class::Gpu, &["pci:v1bc03".to_string()]);
        assert_eq!(b.request(Class::Gpu, "p", t), Outcome::Loaded(1));
        b.request(Class::Audio, "p", t);
        b.add_found(Class::Audio, &["snd:late".to_string()]);
        assert_eq!(b.waiting(), vec![(Class::Bluetooth, 1)], "a loaded class does not wait again");
        assert_eq!(f.calls.borrow().len(), 2);
    }

    #[test]
    fn the_switch_is_off_unless_the_command_line_says_one_and_off_means_the_old_behaviour() {
        assert!(!switch_on("quiet root=/dev/sda1") && !switch_on("iyakki.lazydrivers=0") && switch_on("quiet iyakki.lazydrivers=1"));
        let a = aliases();
        let (now, held) = plan_deferred(&a, false);
        assert_eq!((now, held.len()), (a.clone(), 0), "off: every alias loads, in the order found");
        let (now, held) = plan_deferred(&a, true);
        assert!(now.len() < a.len() && held.len() == 2);
    }

    #[test]
    fn the_gate_follows_the_capability_catalogue_and_graphics_is_the_cores_alone() {
        let g = CapGate { holds: |app: &str, cap: &str| iyakki_core_api::appcaps::builtin_holds(app, cap) };
        assert!(g.allowed(CORE, Class::Gpu) && !g.allowed("some-app", Class::Gpu));
        // playing sound is held by every built-in app by default; Bluetooth only by the apps in the built-in table
        assert!(g.allowed("any-built-in-app", Class::Audio) && !g.allowed("any-built-in-app", Class::Bluetooth));
        let only = CapGate { holds: |app: &str, cap: &str| app == "player" && cap == "speaker.play" };
        assert!(only.allowed("player", Class::Audio) && !only.allowed("player", Class::Bluetooth) && !only.allowed("other", Class::Audio));
        for c in [Class::Audio, Class::Bluetooth] {
            assert!(capabilities_for(c).iter().all(|n| iyakki_core_api::appcaps::lookup(n).is_some()), "{c:?} names real capabilities");
        }
    }

    #[test]
    fn names_in_the_audit_log_are_cleaned() {
        let (mut b, _, l) = broker(vec![]);
        b.request(Class::Audio, "evil\nforged line", Instant::now());
        assert_eq!(l.0.borrow().len(), 1);
        assert!(!l.0.borrow()[0].contains('\n') && l.0.borrow()[0].contains("evilforgedline"), "{:?}", l.0.borrow());
        assert_eq!(Class::parse("bluetooth"), Some(Class::Bluetooth));
        assert_eq!(Class::parse("camera"), None);
    }
}
