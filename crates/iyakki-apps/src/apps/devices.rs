//! The hardware pages of Settings: Network (Ethernet, WiFi), Display and Sound (Bluetooth has its own page, `bluetooth_page.rs`).
//!
//! Each page lists what the machine actually has: an adapter that is not detected is shown as such and cannot be
//! switched. Detection and switching live in `iyakki-devctl`; this file turns that into rows and key actions, and has
//! no drawing code so it can be tested against a fake machine.

use crate::safeclamp::SafeClamp;
use std::sync::Arc;

use iyakki_core_api::hardware::{self as hw, Control, Hardware, LinkKind, Roots};

use crate::gfx::rgb;
use crate::ui::{Key, DIM, OK};

/// The machine the pages look at and act on. Real in production; tests use a fake one.
#[derive(Clone)]
pub struct DeviceEnv {
    pub roots: Roots,
    pub ctl: Arc<dyn Control>,
}

impl DeviceEnv {
    pub fn real() -> DeviceEnv {
        #[cfg(any(test, feature = "testing"))]
        {
            // unit tests never look at (or touch) the machine they run on
            return DeviceEnv { roots: Roots { sys: "/nonexistent/sys".into(), proc_: "/nonexistent/proc".into() }, ctl: Arc::new(NoControl) };
        }
        #[allow(unreachable_code)]
        {
            // "Load the Bluetooth driver anyway" lives in the driver code above the hardware library
            crate::agent().macfw_register_bluetooth_loader();
            // sound from a phone over Bluetooth plays through the mixer (the play command is only the fallback when it is off)
            crate::agent().sound_probe().bluetooth_audio_install(Box::new(|| iyakki_shared::voice::DEFAULT_PLAY_CMD.to_string()));
            DeviceEnv { roots: Roots::real(), ctl: iyakki_core_local::system_control() }
        }
    }
}

/// Does nothing (used where the real machine must not be touched).
pub struct NoControl;

impl Control for NoControl {
    fn set_link(&self, _: &str, _: bool) -> Result<(), String> {
        Err("not available here".into())
    }
    fn ipv4(&self) -> Vec<(String, String)> {
        vec![]
    }
    fn volume(&self, _: u32) -> Option<hw::Volume> {
        None
    }
    fn change_volume(&self, _: u32, _: Option<i32>) -> Result<hw::Volume, String> {
        Err("not available here".into())
    }
    fn set_volume(&self, _: u32, _: u32) -> Result<hw::Volume, String> {
        Err("not available here".into())
    }
    fn dhcp(&self, _: &str) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DevTab {
    Network,
    Display,
    Sound,
}

/// What a row acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Nothing,
    Ethernet(String),
    /// A WiFi interface and the radio switches that belong to it.
    Wifi(String, Vec<String>),
    Radio(Vec<String>),
    Backlight(String),
    Card(u32),
    /// The private MAC address setting of one adapter.
    Mac(String, LinkKind),
    /// Settings -> Sound: which output IYAKKI plays on (Automatic or one device).
    SoundChoice,
    /// One sound output: its key (`PCH:3`) and ALSA device (`plughw:0,3`).
    SoundOut(String, String),
    /// IYAKKI's own volume and mute (applied in software).
    SoftVolume,
    /// Advanced: the software boost of this card (card id) in dB.
    SoundBoost(String),
    /// Advanced: which volume control this card uses (card id): Automatic, Hardware, Software.
    SoundVolMode(String),
    /// Advanced: "Does the hardware volume work?" (card, device, control name, card id).
    SoundHwCheck(u32, u32, String, String),
    /// Advanced: the stereo test (`false`) or the channel walk (`true`).
    SoundPattern(bool),
    /// Show or hide the Advanced rows.
    SoundAdvanced,
    /// Settings -> Sound: mix the sound of several apps (the IYAKKI mixer, experimental).
    SoundMixer,
    SoundWebMix,
    /// The Bluetooth headphones and speakers row (F-NET-28).
    SoundBluetooth,
    /// Settings -> Sound, Microphone: one input (its ALSA device, `plughw:2,0`; empty: Automatic).
    SoundMic(String),
    /// Settings -> Sound, Microphone: listen for three seconds and say how loud it was.
    SoundMicTest,
    /// Settings -> Display: ask the kernel to read the connected displays again (their modes), then draw at the best one.
    LookAgain,
    /// The Mac's built-in (T2) microphone: allow or forbid it (a risk card first).
    SoundT2Mic,
    /// Advanced: the highlight timing offset of read-aloud words, ms (-500..500).
    SoundHighlightOffset,
}

impl Target {
    /// A row that is a choice (an input, an output) or a harmless switch: a click or a touch does what Enter does, so the page is not
    /// keyboard-only. Rows that change the machine's connections stay select-then-press, as before.
    pub fn activates_on_click(&self) -> bool {
        matches!(self, Target::SoundChoice | Target::SoundOut(..) | Target::SoundMic(_) | Target::SoundMicTest | Target::SoundT2Mic | Target::SoundAdvanced | Target::LookAgain)
    }
}

/// A value the row can show as a slider.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliderVal {
    pub value: i64,
    pub min: i64,
    pub max: i64,
    pub step: i64,
    pub unit: &'static str,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DevRow {
    pub title: String,
    pub sub: String,
    pub chips: Vec<(String, [u8; 4])>,
    pub target: Target,
    /// Set for brightness and volume: the page draws a slider.
    pub slider: Option<SliderVal>,
}

impl DevRow {
    fn new(title: impl Into<String>, sub: impl Into<String>, target: Target) -> DevRow {
        DevRow { title: title.into(), sub: sub.into(), chips: vec![], target, slider: None }
    }
    fn with_slider(mut self, v: SliderVal) -> DevRow {
        self.slider = Some(v);
        self
    }
    fn chip(mut self, t: impl Into<String>, c: [u8; 4]) -> DevRow {
        self.chips.push((t.into(), c));
        self
    }
    /// Can be switched or adjusted.
    pub fn actionable(&self) -> bool {
        self.target != Target::Nothing
    }
}

/// What one key press did.
pub struct Done {
    pub result: Result<String, String>,
    /// For the audit log: what changed (names only).
    pub audit: String,
}

/// Seconds between looks at the hardware while a page is open, in ticks (30 per second).
const REFRESH_TICKS: u64 = 60;

struct Snapshot {
    links: Vec<hw::Link>,
    ips: Vec<(String, String)>,
    rfkill: Vec<hw::Rfkill>,
    /// A Broadcom 4360-family Wi-Fi card is in the machine (no driver for it here).
    bcm_nodriver: Option<&'static str>,
    screen: hw::Screen,
    cards: Vec<(hw::Card, Option<hw::Volume>)>,
    outputs: Vec<iyakki_shared::audioout::Output>,
    /// The card whose software volume (IYAKKI's ALSA softvol) carries the IYAKKI volume.
    soft_card: Option<u32>,
    /// Channels of the shared device behind the volume (4 on the MacBookPro14,1); None: not a shared device.
    mix_channels: Option<u32>,
    /// A sound card is still starting up (the built-in speakers of a MacBook start up slowly).
    starting: Option<String>,
    /// The card whose own hardware volume control is not used (it does nothing, or Software was chosen).
    hw_ignored: Option<u32>,
    /// The sound card is held by another program: the sentence to show.
    busy: Option<String>,
    mac: Vec<hw::mac::MacInfo>,
    /// The Bluetooth hardware (the page itself is `bluetooth_page.rs`; this keeps what the Overview needs).
    bt: hw::btstate::BtState,
    /// A MacBookPro13,x/14,x or iMac whose CS8409 codec has no driver (`iyakki_core_api::device_types::driver_packs::cs8409`): the line to show.
    cs8409: Option<String>,
    /// The microphones (capture devices) and whether a T2 sound card's microphones are hidden on purpose.
    mics: Vec<iyakki_shared::voicedesk::Mic>,
    mic_t2: iyakki_shared::voicedesk::T2Mic,
}

pub struct DevicesPane {
    env: DeviceEnv,
    /// What the pages read and switch on the machine (`LocalHardware` over `env`, or a fake).
    api: std::sync::Arc<dyn Hardware>,
    snap: Option<Snapshot>,
    /// Hardware classes already asked for from the lazy device broker (once per pane).
    asked: std::collections::BTreeSet<&'static str>,
    at: u64,
    /// What the private MAC address setting reads and changes.
    mac: hw::mac::MacEnv,
    /// Settings -> Sound (output choice, IYAKKI's volume), given by the Settings app.
    sound: iyakki_shared::audioout::SoundSettings,
    /// The microphone the person chose (`VoiceSettings::mic`; empty: Automatic).
    mic_choice: String,
    /// The Advanced rows of the Sound page are shown.
    advanced: bool,
    /// An app the shell should open next (Bluetooth firmware -> Store, Setup): taken by [`DevicesPane::take_request`].
    request: Option<(String, String)>,
    /// The core, for the mixer's numbers on the Sound page; without one the page shows the mixer as idle.
    core: Option<std::sync::Arc<dyn iyakki_core_api::Core>>,
}

impl DevicesPane {
    pub fn new(env: DeviceEnv) -> DevicesPane {
        #[cfg(any(test, feature = "testing"))]
        let mac = hw::mac::MacEnv { roots: env.roots.clone(), ..hw::mac::MacEnv::nowhere(env.ctl.clone()) };
        #[cfg(not(any(test, feature = "testing")))]
        let mac = hw::mac::MacEnv { ctl: env.ctl.clone(), roots: env.roots.clone(), ..hw::mac::MacEnv::real() };
        let api: std::sync::Arc<dyn Hardware> = std::sync::Arc::new(iyakki_core_local::LocalHardware::new(env.roots.clone(), env.ctl.clone()));
        DevicesPane { env, api, snap: None, asked: Default::default(), at: 0, mac, sound: Default::default(), mic_choice: String::new(), advanced: false, request: None, core: None }
    }

    /// Ask the mixer's state through this handle (`cx.core`).
    pub fn set_core(&mut self, core: std::sync::Arc<dyn iyakki_core_api::Core>) {
        self.core = Some(core);
    }

    pub fn core(&self) -> Option<std::sync::Arc<dyn iyakki_core_api::Core>> {
        self.core.clone()
    }

    /// Read and switch the hardware through this handle (`cx.os.hardware()` or a fake) instead of the one made from the environment.
    pub fn set_hardware(&mut self, api: std::sync::Arc<dyn Hardware>) {
        self.api = api;
        self.invalidate();
    }

    /// The microphone the person chose, so the Sound page can show it (the page tells the pane before drawing).
    pub fn set_mic_choice(&mut self, id: String) {
        self.mic_choice = id;
    }

    /// What Settings -> Sound says now (the rows show the choice and the volume).
    pub fn set_sound(&mut self, sound: iyakki_shared::audioout::SoundSettings) {
        self.sound = sound;
    }

    /// Show or hide the Advanced rows of the Sound page; the new state.
    pub fn toggle_advanced(&mut self) -> bool {
        self.advanced = !self.advanced;
        self.advanced
    }

    /// Does the page look at this computer's own sound cards (not a made-up tree)?
    pub fn real_sound(&self) -> bool {
        self.env.roots.proc_ == crate::host().proc_root()
    }

    /// The sound outputs seen at the last look.
    pub fn outputs(&mut self, tick: u64) -> Vec<iyakki_shared::audioout::Output> {
        self.snap(tick).outputs.clone()
    }

    /// The private MAC address settings and actions (a copy).
    pub fn mac(&self) -> &hw::mac::MacEnv {
        &self.mac
    }

    /// Replace what the private MAC address setting acts on (tests).
    pub fn set_mac(&mut self, mac: hw::mac::MacEnv) {
        self.mac = mac;
        self.invalidate();
    }

    /// The desktop window on someone else's computer: the host operating system owns the addresses.
    pub fn set_host_managed(&mut self, on: bool) {
        if self.mac.host_managed != on {
            self.mac.host_managed = on;
            self.invalidate();
        }
    }

    /// Look at the hardware again on the next [`rows`](Self::rows).
    pub fn invalidate(&mut self) {
        self.snap = None;
    }

    fn refresh(&mut self, tick: u64) {
        let r = &self.env.roots;
        let cards = self.api.cards();
        let mac = self.api.mac_infos(&self.mac.config());
        let core = self.core.clone();
        let sound: &dyn iyakki_core_api::SoundControl = match &core {
            Some(c) => c.sound(),
            None => &iyakki_core_api::NoSound,
        };
        let route = match &core {
            Some(c) => c.audio_route_at(&r.proc_, &r.sys),
            None => std::sync::Arc::new(iyakki_core_api::audioroute::NoAudioRoute),
        };
        let outputs = sound.outputs_via(&*route);
        let state = if self.real_sound() { sound.state() } else { Default::default() };
        let soft_card = state.softvol_card;
        let (mix_channels, starting, hw_ignored, busy) = if self.real_sound() {
            if state.active {
                (state.mix_channels, state.starting, state.hw_ignored, state.busy)
            } else {
                (None, None, None, None)
            }
        } else {
            (None, sound.card_starting_via(&*route).map(|_| iyakki_shared::audioout::STARTING_LINE.to_string()), None, None)
        };
        // first use of Bluetooth and sound: the core's lazy device broker loads their drivers now (a no-op unless `iyakki.lazydrivers=1`)
        for (class, who) in [("bluetooth", "settings"), ("audio", "core")] {
            if self.asked.insert(class) {
                let _ = self.api.request_class(class, who);
            }
        }
        let bt = self.api.bluetooth();
        let cs8409 = self.api.cs8409_line();
        self.snap = Some(Snapshot { links: self.api.links(), ips: self.api.ipv4(), rfkill: self.api.rfkill(), bcm_nodriver: self.api.broadcom_wifi_without_driver().then_some(hw::BROADCOM_NO_DRIVER_TEXT), screen: self.api.screen(), cards, outputs, soft_card, mix_channels, starting, hw_ignored, busy, mac, bt, cs8409, mics: crate::agent().voicedesk_list_mics(&r.proc_), mic_t2: iyakki_shared::voicedesk::t2_mic(&r.proc_) });
        self.at = tick;
    }

    /// Read (or refresh) the hardware snapshot now, so the page's buttons, which depend on it (Firmware, Load driver), are measured
    /// from the same state the rows are drawn from, even on the first frame.
    pub fn prime(&mut self, tick: u64) {
        let _ = self.snap(tick);
    }

    fn snap(&mut self, tick: u64) -> &Snapshot {
        if self.snap.is_none() || tick.saturating_sub(self.at) >= REFRESH_TICKS {
            self.refresh(tick);
        }
        self.snap.as_ref().expect("just refreshed")
    }

    pub fn rows(&mut self, tab: DevTab, tick: u64) -> Vec<DevRow> {
        let host = self.mac.host_managed;
        let sound = self.sound.clone();
        let mic_choice = self.mic_choice.clone();
        let advanced = self.advanced;
        let mixer = if tab == DevTab::Sound { self.core.as_ref().and_then(|c| c.mixer().stats()) } else { None };
        let s = self.snap(tick);
        match tab {
            DevTab::Network => network_rows(s, host),
            DevTab::Display => display_rows(s, &screen_in_use_now(s.screen.framebuffer.as_deref())),
            DevTab::Sound => sound_rows(s, &sound, advanced, &mic_choice, mixer.as_ref()),
        }
    }

    /// A key on the selected row. `None` when the key means nothing on this page.
    pub fn act(&mut self, tab: DevTab, sel: usize, key: &Key, tick: u64) -> Option<Done> {
        let rows = self.rows(tab, tick);
        let row = rows.get(sel)?.clone();
        let toggle = matches!(key, Key::Enter | Key::Char('e' | ' '));
        let up = matches!(key, Key::Char('+' | '='));
        let down = matches!(key, Key::Char('-'));
        let done = match (&row.target, tab) {
            (Target::Nothing, _) => Some(Done { result: Err(format!("{}: nothing to switch here.", row.title)), audit: String::new() }).filter(|_| toggle || up || down),
            (Target::LookAgain, _) if toggle => Some(self.look_again()),
            (Target::Ethernet(name), _) if toggle => Some(self.toggle_link(name, &row)),
            (Target::Wifi(name, ids), _) if toggle => Some(self.toggle_wifi(name, ids, &row)),
            (Target::Radio(ids), _) if toggle => Some(self.toggle_radio(ids, &row)),
            (Target::Backlight(name), _) if up || down => {
                let cur = self.snap.as_ref().and_then(|s| s.screen.backlights.iter().find(|b| &b.name == name).map(|b| b.percent())).unwrap_or(50);
                let want = if up { (cur + 10).min(100) } else { cur.saturating_sub(10).max(5) };
                Some(match self.api.set_brightness(name, want) {
                    Ok(()) => Done { result: Ok(format!("Brightness {want}%.")), audit: format!("brightness {name} {want}") },
                    Err(e) => Done { result: Err(e), audit: String::new() },
                })
            }
            (Target::Card(i), _) if up || down || matches!(key, Key::Char('m')) => {
                let delta = if up { Some(5) } else if down { Some(-5) } else { None };
                Some(match self.api.change_volume(*i, delta) {
                    Ok(v) => Done { result: Ok(if v.muted { "Muted.".to_string() } else { format!("Volume {}%.", v.percent) }), audit: format!("volume card{i}") },
                    Err(e) => Done { result: Err(e), audit: String::new() },
                })
            }
            _ => None,
        };
        if done.is_some() {
            self.invalidate();
        }
        done
    }

    /// Set a slider row to an exact value (a click on its track).
    pub fn set_value(&mut self, tab: DevTab, sel: usize, value: i64, tick: u64) -> Option<Done> {
        let rows = self.rows(tab, tick);
        let row = rows.get(sel)?.clone();
        let slider = row.slider?;
        let v = value.clamp_safe(slider.min, slider.max);
        let done = match &row.target {
            Target::Backlight(name) => match self.api.set_brightness(name, v as u32) {
                Ok(()) => Done { result: Ok(format!("Brightness {v}%.")), audit: format!("brightness {name} {v}") },
                Err(e) => Done { result: Err(e), audit: String::new() },
            },
            Target::Card(i) => match self.api.set_volume(*i, v as u32) {
                Ok(vol) => Done { result: Ok(if vol.muted { "Muted.".to_string() } else { format!("Volume {}%.", vol.percent) }), audit: format!("volume card{i}") },
                Err(e) => Done { result: Err(e), audit: String::new() },
            },
            _ => return None,
        };
        self.invalidate();
        Some(done)
    }

    fn toggle_link(&mut self, name: &str, row: &DevRow) -> Done {
        let on = row.chips.iter().any(|c| c.0 == "ON");
        // switching on is a new connection: the private address (if chosen) is set while the link is still down
        let note = if on { String::new() } else { self.private_address_note(name) };
        let r = if on { self.api.set_link(name, false) } else { self.api.set_link(name, true).and_then(|_| self.api.dhcp(name)) };
        match r {
            Ok(()) => Done { result: Ok(format!("Ethernet {name} turned {}.{note}", if on { "off" } else { "on" })), audit: format!("ethernet {name} {}", if on { "off" } else { "on" }) },
            Err(e) => Done { result: Err(e), audit: String::new() },
        }
    }

    fn toggle_wifi(&mut self, name: &str, ids: &[String], row: &DevRow) -> Done {
        let on = row.chips.iter().any(|c| c.0 == "ON");
        let mut note = String::new();
        let res = (|| -> Result<(), String> {
            if on {
                self.api.set_link(name, false)?;
                ids.iter().try_for_each(|id| self.api.set_rfkill(id, true))
            } else {
                ids.iter().try_for_each(|id| self.api.set_rfkill(id, false))?;
                note = self.private_address_note(name);
                self.api.set_link(name, true)?;
                self.api.dhcp(name)
            }
        })();
        match res {
            Ok(()) => Done { result: Ok(format!("WiFi {name} turned {}.{note}", if on { "off" } else { "on" })), audit: format!("wifi {name} {}", if on { "off" } else { "on" }) },
            Err(e) => Done { result: Err(e), audit: String::new() },
        }
    }

    /// Give the adapter its private address (when the setting calls for one) before it is switched on. A failure is said, never
    /// fatal: the adapter then keeps its hardware address.
    fn private_address_note(&self, name: &str) -> String {
        if self.mac.host_managed {
            return String::new();
        }
        let cfg = self.mac.config();
        let kind = self.api.links().into_iter().find(|l| l.name == name).map(|l| l.kind);
        let Some(kind) = kind else { return String::new() };
        let ssid = if kind == LinkKind::Wifi { self.mac.preferred_ssid() } else { None };
        if !cfg.mode_for(kind, name, ssid.as_deref()).is_private() {
            return String::new();
        }
        match self.mac.apply_now(name, ssid.as_deref(), true) {
            Ok(m) => format!(" {m}"),
            Err(e) => format!(" Private MAC address not applied ({e}); the hardware address is used."),
        }
    }

    /// The controller is listed but needs firmware (as of the last look): Settings shows the "Get firmware" button first.
    pub fn bt_needs_firmware(&self) -> bool {
        self.snap.as_ref().is_some_and(|s| s.bt.needs_firmware)
    }

    /// The app the shell should open next, once (the Bluetooth firmware row leads to Store -> Setup).
    /// Ask the shell to open another app next (Settings -> Storage opens DISKS).
    pub fn ask_open(&mut self, app: &str) {
        self.request = Some((app.into(), String::new()));
    }

    /// [`DevicesHw::ask_open`] with the thing to open in it (Settings' "Share diagnostics" opens DOCTOR with its preview).
    pub fn ask_open_item(&mut self, app: &str, item: &str) {
        self.request = Some((app.into(), item.into()));
    }

    pub fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }

    /// Ask the shell to open app `app` with `arg` (the Network page's sign-in page goes to the Browser).
    pub fn ask_shell(&mut self, app: &str, arg: String) {
        self.request = Some((app.to_string(), arg));
    }

    /// "Look again": the kernel reads every connected display again (`detect`), a few times in the background; when its mode list becomes
    /// right the screen is drawn again at the best mode (`crate::display::look_again`).
    fn look_again(&mut self) -> Done {
        let line = crate::display::look_again(&self.env.roots.sys.join("class/drm"));
        self.snap = None; // the next look reads the modes again
        Done { result: Ok(format!("Looking at the displays again: {line}. A display that now offers a bigger size is used within a few seconds.")), audit: "display look again".into() }
    }

    fn toggle_radio(&mut self, ids: &[String], row: &DevRow) -> Done {
        let on = row.chips.iter().any(|c| c.0 == "ON");
        match ids.iter().try_for_each(|id| self.api.set_rfkill(id, on)) {
            Ok(()) => Done { result: Ok(format!("{} turned {}.", row.title, if on { "off" } else { "on" })), audit: format!("radio {} {}", ids.join(","), if on { "off" } else { "on" }) },
            Err(e) => Done { result: Err(e), audit: String::new() },
        }
    }
}

const WARN: [u8; 4] = rgb(255, 196, 94);

fn onoff(on: bool) -> (&'static str, [u8; 4]) {
    if on { ("ON", OK) } else { ("OFF", DIM) }
}

/// The private MAC address row of one adapter: what is chosen, the hardware address, the one in use now.
fn mac_row(i: &hw::mac::MacInfo, host: bool) -> DevRow {
    let now = format!("now {}{}", i.current, if i.locally_administered { " · locally administered" } else { "" });
    if host {
        return DevRow::new(format!("Private MAC · {}", i.iface), format!("{now} · managed by the host operating system"), Target::Nothing).chip("host", DIM);
    }
    let sub = format!("{} · hardware {} · {now}", i.mode.key(), i.permanent.as_deref().unwrap_or("unknown"));
    let (label, col) = match i.mode {
        hw::mac::MacMode::Off => ("MAC OFF", DIM),
        hw::mac::MacMode::PerNetwork => ("PER NETWORK", OK),
        hw::mac::MacMode::Random => ("RANDOM", OK),
    };
    DevRow::new(format!("Private MAC · {}", i.iface), sub, Target::Mac(i.iface.clone(), i.kind)).chip(label, col).chip(if i.private { "private" } else { "hardware" }, if i.private { OK } else { DIM })
}

fn network_rows(s: &Snapshot, host: bool) -> Vec<DevRow> {
    let mut rows = Vec::new();
    let ip_of = |n: &str| s.ips.iter().find(|(i, _)| i == n).map(|(_, a)| a.clone());
    let eth: Vec<_> = s.links.iter().filter(|l| l.kind == LinkKind::Ethernet).collect();
    for l in &eth {
        let state = if !l.up { "off".to_string() } else if !l.carrier { "no cable".to_string() } else { ip_of(&l.name).map_or("connected, waiting for an address".to_string(), |a| format!("connected · {a}")) };
        let speed = l.speed.map(|v| format!(" · {v} Mb/s")).unwrap_or_default();
        let (t, c) = onoff(l.up);
        rows.push(DevRow::new(format!("Ethernet · {}", l.name), format!("{state}{speed} · {}", l.mac), Target::Ethernet(l.name.clone())).chip(t, c));
    }
    if eth.is_empty() {
        rows.push(DevRow::new("Ethernet", "no adapter detected", Target::Nothing).chip("not found", DIM));
    }
    let wlan_ids: Vec<String> = s.rfkill.iter().filter(|k| k.kind == "wlan").map(|k| k.id.clone()).collect();
    let wlan_on = hw::radio_on(&s.rfkill.iter().filter(|k| k.kind == "wlan").cloned().collect::<Vec<_>>());
    let hard = s.rfkill.iter().any(|k| k.kind == "wlan" && k.hard);
    let wifi: Vec<_> = s.links.iter().filter(|l| l.kind == LinkKind::Wifi).collect();
    for l in &wifi {
        let on = l.up && wlan_on;
        let state = if hard { "switched off by a key or switch on the machine".to_string() } else if !on { "off".to_string() } else if !l.carrier { "on, not connected to a network".to_string() } else { ip_of(&l.name).map_or("connected, waiting for an address".to_string(), |a| format!("connected · {a}")) };
        let (t, c) = onoff(on);
        let mut row = DevRow::new(format!("WiFi · {}", l.name), format!("{state} · {}", l.mac), if hard { Target::Nothing } else { Target::Wifi(l.name.clone(), wlan_ids.clone()) }).chip(t, c);
        if hard {
            row = row.chip("hardware off", WARN);
        }
        rows.push(row);
    }
    if wifi.is_empty() {
        let sub = if let Some(t) = s.bcm_nodriver { t } else if s.rfkill.iter().any(|k| k.kind == "wlan") { "a radio is present but has no driver loaded (it may need firmware)" } else { "no adapter detected, so WiFi stays disabled" };
        rows.push(DevRow::new("WiFi", sub, Target::Nothing).chip(if s.bcm_nodriver.is_some() { "no driver" } else { "not found" }, DIM));
    }
    // the private MAC address of each adapter, after the adapters themselves
    rows.extend(s.mac.iter().map(|i| mac_row(i, host)));
    rows
}
/// What the "Screen in use" row says: the size the shell is driving now, not the framebuffer size sysfs reported at the last refresh
/// (which stayed at 1920x1080 on a Mac mini while the shell drew 3840x2160). `kms` are the displays the shell drives itself (name and
/// size); `driven` the size of the framebuffer it draws on; `fb` the framebuffer's own line ("3840x2160 · 32 bit · i915drmfb").
pub(crate) fn screen_in_use(fb: Option<&str>, driven: Option<(u32, u32)>, kms: &[(String, (u32, u32))]) -> String {
    if !kms.is_empty() {
        return format!("{} (the shell draws on each display itself)", kms.iter().map(|(n, (w, h))| format!("{n} {w}x{h}")).collect::<Vec<_>>().join(", "));
    }
    match (driven, fb) {
        (Some((w, h)), Some(fb)) if fb.starts_with(&format!("{w}x{h} ")) || fb == format!("{w}x{h}") => fb.to_string(),
        (Some((w, h)), Some(fb)) => format!("{w}x{h} drawn by the shell · framebuffer {fb}"),
        (Some((w, h)), None) => format!("{w}x{h} drawn by the shell"),
        (None, Some(fb)) => fb.to_string(),
        (None, None) => "no framebuffer found".into(),
    }
}

fn screen_in_use_now(fb: Option<&str>) -> String {
    let st = crate::display::outputs::hub().status();
    let kms: Vec<(String, (u32, u32))> = if st.kms { st.outputs.iter().filter(|o| o.connected).filter_map(|o| o.current.map(|c| (o.name.clone(), c))).collect() } else { Vec::new() };
    screen_in_use(fb, crate::display::driven(), &kms)
}

fn display_rows(s: &Snapshot, in_use: &str) -> Vec<DevRow> {
    let mut rows = Vec::new();
    let scr = &s.screen;
    rows.push(DevRow::new("Screen in use", in_use, Target::Nothing));
    let mut idle = 0;
    for o in &scr.outputs {
        if !o.connected {
            idle += 1;
            continue;
        }
        let mut modes: Vec<&String> = Vec::new();
        for m in &o.modes {
            if !modes.contains(&m) {
                modes.push(m); // the same size can be listed once per refresh rate
            }
        }
        let sub = match modes.split_first() {
            None => "connected".to_string(),
            Some((best, rest)) if rest.is_empty() => format!("best {best}"),
            Some((best, rest)) => format!("best {best} · also {}", rest.iter().take(4).map(|m| m.as_str()).collect::<Vec<_>>().join(", ")),
        };
        rows.push(DevRow::new(format!("Output · {}", o.name), sub, Target::Nothing).chip("connected", OK));
    }
    if scr.outputs.iter().any(|o| o.connected) {
        rows.push(DevRow::new("Look again", "Press Enter: the computer reads the connected displays again and uses the best size they offer (use it when a display shows a smaller size than it can do). It also happens by itself when a display is plugged in.", Target::LookAgain));
    }
    if idle > 0 {
        rows.push(DevRow::new("Other outputs", format!("{idle} not connected"), Target::Nothing).chip("free", DIM));
    }
    for b in &scr.backlights {
        rows.push(DevRow::new(format!("Brightness · {}", b.name), "How bright this screen's backlight is. It never goes fully dark.", Target::Backlight(b.name.clone())).with_slider(SliderVal { value: b.percent() as i64, min: 5, max: 100, step: 5, unit: "%" }));
    }
    rows
}

fn sound_rows(s: &Snapshot, prefs: &iyakki_shared::audioout::SoundSettings, advanced: bool, mic_choice: &str, mixer: Option<&iyakki_core_api::MixerStats>) -> Vec<DevRow> {
    use iyakki_shared::audioout::{Kind};
    use iyakki_shared::audioout::choose;
    if s.cards.is_empty() && s.outputs.is_empty() {
        if let Some(line) = &s.starting {
            return vec![DevRow::new("Sound", line.clone(), Target::Nothing).chip("starting", WARN)];
        }
        return vec![DevRow::new("Sound", format!("no sound card detected · {}", iyakki_shared::audioout::NO_OUTPUT), Target::Nothing).chip("not found", DIM)];
    }
    let mut rows = Vec::new();
    // which output IYAKKI plays on, and why
    let choice = choose(&s.outputs, &prefs.output);
    let mode = if choice.chosen {
        "chosen"
    } else if !prefs.output.is_empty() {
        "Automatic (chosen one missing)"
    } else {
        "Automatic"
    };
    let (sub, chip) = match &choice.output {
        Some(o) => (format!("playing on {} · {}", o.label(), choice.reason), (if choice.chosen { "CHOSEN" } else { "AUTO" }, OK)),
        None => (choice.reason.clone(), ("NO OUTPUT", WARN)),
    };
    rows.push(DevRow::new(format!("Sound output · {mode}"), sub, Target::SoundChoice).chip(chip.0, chip.1));
    // IYAKKI's own volume: a software volume on the output (ALSA softvol) when it has no hardware volume, else applied to
    // the sound IYAKKI plays
    let hdmi = choice.output.as_ref().is_some_and(|o| o.kind == Kind::Hdmi);
    let help = if s.soft_card.is_some() {
        "This output has no hardware volume: IYAKKI gives it a software volume (ALSA softvol) that every program follows, media players included."
    } else if hdmi {
        "Volume is applied by IYAKKI for HDMI (HDMI has no hardware volume): radio, media, reading aloud and sounds."
    } else {
        "Applied by IYAKKI to the radio, media, reading aloud and sounds."
    };
    let mut vol = DevRow::new("IYAKKI volume", help, Target::SoftVolume).with_slider(SliderVal { value: prefs.volume_pct as i64, min: 0, max: 100, step: 5, unit: "%" });
    vol = if prefs.muted { vol.chip("muted", WARN) } else { vol.chip(format!("{}%", prefs.volume_pct), OK) };
    rows.push(vol);
    for o in &s.outputs {
        let active = choice.output.as_ref().is_some_and(|a| a.key() == o.key());
        let mut row = DevRow::new(o.label(), format!("card {} · device {} · {} · {}", o.card, o.device, o.alsa(), o.state()), Target::SoundOut(o.key(), o.alsa()));
        if active {
            row = row.chip("IN USE", OK);
        } else if o.connected == Some(true) {
            row = row.chip("connected", OK);
        } else if o.connected == Some(false) {
            row = row.chip(if o.kind == Kind::Hdmi { "no monitor" } else { "unplugged" }, DIM);
        }
        rows.push(row);
    }
    if let Some(line) = &s.starting {
        rows.push(DevRow::new("Built-in speakers", line.clone(), Target::Nothing).chip("starting", WARN));
    }
    if let Some(line) = &s.busy {
        rows.push(DevRow::new("Sound card in use", line.clone(), Target::Nothing).chip("busy", WARN));
    }
    if let Some(line) = &s.cs8409 {
        rows.push(DevRow::new("Built-in speakers", line.clone(), Target::Nothing).chip("no driver", WARN));
    }
    for (c, v) in &s.cards {
        rows.push(card_row(c, v.as_ref(), s.soft_card.or(s.hw_ignored), s.hw_ignored, prefs));
    }
    rows.extend(mixer_rows(prefs, mixer));
    rows.extend(mic_rows(&s.mics, mic_choice, s.mic_t2));
    advanced_sound_rows(s, prefs, &choice, advanced, &mut rows);
    rows
}

/// The Microphone part of the Sound page: which input is used and why, every input with the one in use marked, the Test row, and (when none was
/// found) what to plug in. The same choice (`VoiceSettings::mic`) serves dictation and the web pages in the Browser.
pub fn mic_rows(mics: &[iyakki_shared::voicedesk::Mic], chosen: &str, t2: iyakki_shared::voicedesk::T2Mic) -> Vec<DevRow> {
    let t2_hidden = t2 == iyakki_shared::voicedesk::T2Mic::Off;
    let picked = iyakki_shared::voicedesk::pick_mic(mics, chosen);
    let mut rows = Vec::new();
    let (sub, chip): (String, (&str, _)) = match picked {
        Some(m) if !chosen.is_empty() && m.id == chosen => (format!("recording from {} ({})", m.label, m.id), ("CHOSEN", OK)),
        Some(m) if !chosen.is_empty() => (format!("the chosen microphone is missing: recording from {} ({}) instead", m.label, m.id), ("AUTO", WARN)),
        Some(m) => (format!("Automatic: recording from {} ({})", m.label, m.id), ("AUTO", OK)),
        None => (
            if t2_hidden { "no microphone found: the Mac's built-in microphones are hidden on purpose (they listen all the time). Plug in a USB microphone or a camera with one." } else { "no microphone found: plug in a USB microphone or a camera that has one." }.to_string(),
            ("NO MIC", WARN),
        ),
    };
    rows.push(DevRow::new("Microphone", sub, Target::SoundMic(String::new())).chip(chip.0, chip.1));
    for m in mics {
        let used = picked.is_some_and(|p| p.id == m.id);
        let mut r = DevRow::new(m.label.clone(), format!("{}{}", m.id, if m.usb { " · USB" } else { "" }), Target::SoundMic(m.id.clone()));
        if used {
            r = r.chip("IN USE", OK);
        }
        rows.push(r);
    }
    if t2 != iyakki_shared::voicedesk::T2Mic::Absent {
        let on = t2 == iyakki_shared::voicedesk::T2Mic::On;
        rows.push(
            DevRow::new(
                T2_MIC_TITLE,
                if on { "The Mac's own microphone is allowed until the next restart. Press Enter to forbid it again. If the Mac freezes while you record, restart it: it comes back with this off." } else { T2_MIC_HELP },
                Target::SoundT2Mic,
            )
            .chip(if on { "on" } else { "off" }, if on { WARN } else { DIM }),
        );
    }
    if picked.is_some() {
        rows.push(DevRow::new("Test microphone", "Press Enter or r, then speak for three seconds: it says how loud it was. A very low number means the microphone is muted or too quiet.", Target::SoundMicTest));
    }
    rows
}

pub const T2_MIC_TITLE: &str = "Use the Mac's built-in microphone (experimental)";
pub const T2_MIC_HELP: &str = "Off: the T2 chip's sound driver can freeze the whole Mac when it records, so the built-in microphones are hidden. Press Enter to allow them until the next restart (it asks first). A USB microphone or a camera with one needs none of this.";

/// What the Sound page says about the experimental mixer: its switch with the explanation and, when it is on, a live status line.
pub const MIXER_TITLE: &str = "Mix sounds from several apps (experimental)";
pub const MIXER_HELP: &str = "Radio, voice, alarms and notifications can play together and the others get quieter while one speaks. The web browser's own sound is not mixed unless you also turn on the row below; without it, the browser asks before it takes the sound card from a player.";

pub const WEB_MIX_TITLE: &str = "Browser sound through the mixer (experimental)";
pub const WEB_MIX_HELP: &str = "The web browser's sound is sent into the mixer through a pipe instead of opening the sound card itself, so it can play with the radio and be turned down for speech. Needs the mixer above. Applies the next time the web engine starts. Not tested on real hardware yet: if the browser goes silent, turn it off.";

pub const BT_SOUND_TITLE: &str = "Bluetooth headphones and speakers";

/// The row that sends the sound of the whole computer to paired Bluetooth headphones or a speaker (F-NET-28): Enter or B starts it (after a
/// card) or stops it. The pairing itself is in Settings -> Bluetooth.
fn bluetooth_row() -> DevRow {
    let v = crate::agent().sound_probe().bluetooth_sound_view();
    let help = if v.sinks.is_empty() {
        "No headphones or speaker are paired: put them in pairing mode and pair them in Settings -> Bluetooth first. Then this row sends all of this computer's sound to them (SBC only) and silences its own speakers until you stop it.".to_string()
    } else {
        format!("{} Paired: {}. Press Enter or B to {}; the computer's own speakers are silent while it sends.", v.line, v.sinks.iter().map(|s| s.1.clone()).collect::<Vec<_>>().join(", "), if v.active { "stop sending" } else { "send all sound to them" })
    };
    DevRow::new(BT_SOUND_TITLE, help, Target::SoundBluetooth).chip(if v.active { "sending" } else { "off" }, if v.active { OK } else { DIM })
}

fn mixer_rows(prefs: &iyakki_shared::audioout::SoundSettings, stats: Option<&iyakki_core_api::MixerStats>) -> Vec<DevRow> {
    let mut rows = vec![DevRow::new(MIXER_TITLE, MIXER_HELP, Target::SoundMixer).chip(if prefs.mixer { "on" } else { "off" }, if prefs.mixer { WARN } else { DIM })];
    rows.push(DevRow::new(WEB_MIX_TITLE, WEB_MIX_HELP, Target::SoundWebMix).chip(if prefs.web_mix { "on" } else { "off" }, if prefs.web_mix { WARN } else { DIM }));
    rows.push(bluetooth_row());
    rows.push(DevRow::new("Browser sound status", crate::agent().sound_probe().web_mix_status_text(prefs.web_mix, prefs.mixer, crate::agent().webmix_tap_state()), Target::Nothing).chip(if prefs.web_mix && prefs.mixer { "on" } else { "off" }, DIM));
    if prefs.mixer {
        let (state, line) = match stats {
            Some(st) => (if st.sink_open { "running" } else { "idle" }, format!("{} · {}{}", if st.sink_open { "running" } else { "idle" }, st.detail, st.last_error.as_ref().map(|e| format!(" · last problem: {e}")).unwrap_or_default())),
            None => ("idle", "idle · 0 source(s) · 0.0% of a core · 0 underrun(s)".to_string()),
        };
        rows.push(DevRow::new("Mixer status", line, Target::Nothing).chip(state, if state == "running" { OK } else { DIM }));
    }
    rows
}

/// The Advanced part of the Sound page: boost, which volume control, the hardware self-check and the speaker tests.
fn advanced_sound_rows(s: &Snapshot, prefs: &iyakki_shared::audioout::SoundSettings, choice: &iyakki_shared::audioout::Choice, advanced: bool, rows: &mut Vec<DevRow>) {
    use iyakki_shared::audioout::VolumeMode;
    rows.push(DevRow::new("Advanced", if advanced { "Boost, which volume control, speaker tests. Press A or Enter to hide." } else { "Boost, which volume control, speaker tests. Press A or Enter to show." }, Target::SoundAdvanced).chip(if advanced { "shown" } else { "hidden" }, DIM));
    if advanced {
        let ms = prefs.highlight_offset_ms;
        let chip = if ms == 0 { "0 ms".to_string() } else { format!("{ms:+} ms") };
        rows.push(
            DevRow::new("Highlight timing offset", format!("Moves the lit word of TALES and chat read-aloud against the sound: later (+) if the word runs ahead of the voice, earlier (-) if it lags behind. -500 to 500 ms, 0 is the mixer's own measure. Now {chip}. + and - step 25 ms, Enter sets 0."), Target::SoundHighlightOffset)
                .with_slider(SliderVal { value: ms as i64, min: -500, max: 500, step: 25, unit: " ms" })
                .chip(chip, if ms == 0 { DIM } else { WARN }),
        );
    }
    let (true, Some(o)) = (advanced, choice.output.as_ref()) else { return };
    // the boost is part of IYAKKI's own volume path: it is offered whenever that path is (or would be) in use, and otherwise shown with the way to turn it on
    let hw_now = o.uses_hw_volume(prefs.volume_mode_for(&o.card_id));
    if s.soft_card.is_some() || !hw_now || o.hw_volume.is_some() {
        let db = prefs.boost_for(&o.card_id);
        let mut r = DevRow::new(format!("Boost · {}", o.card_name), "Extra software gain for a card whose speakers are too quiet at 100 %. May distort: loud peaks are clipped. Take effect with the next sound. 0 dB is off.", Target::SoundBoost(o.card_id.clone())).with_slider(SliderVal { value: db as i64, min: 0, max: iyakki_shared::softvol::MAX_BOOST_DB as i64, step: 1, unit: " dB" });
        r = if hw_now && s.soft_card.is_none() { r.chip("needs Software volume control", DIM) } else if db > 0 { r.chip(format!("+{db} dB"), WARN) } else { r.chip("off", DIM) };
        rows.push(r);
    }
    if let Some(ctl) = &o.hw_volume {
        let mode = prefs.volume_mode_for(&o.card_id);
        let using = if o.uses_hw_volume(mode) { "the card's hardware control" } else { "IYAKKI's software volume" };
        let dummy = if o.dummy_volume { " This codec is known to have a hardware control that does nothing." } else { "" };
        rows.push(DevRow::new(format!("Volume control · {}", mode.label()), format!("Now using {using}. Enter switches Automatic, Hardware, Software.{dummy}"), Target::SoundVolMode(o.card_id.clone())).chip(mode.label(), if mode == VolumeMode::Automatic { DIM } else { WARN }));
        rows.push(DevRow::new("Does the hardware volume work?", format!("Plays a quiet tone at 100 % and again at about 12 % of the card's {ctl} control; you say whether the second was quieter. Enter starts."), Target::SoundHwCheck(o.card, o.device, ctl.clone(), o.card_id.clone())));
    }
    rows.push(DevRow::new("Stereo test", "The left speaker for 1 s, then the right one, through the same path and volume as the radio.", Target::SoundPattern(false)));
    if s.mix_channels.is_some_and(|n| n > 2) {
        rows.push(DevRow::new("Channel walk", "Every channel of this four-channel card alone for 1 s, to tell which speaker is which.", Target::SoundPattern(true)));
    }
}

/// One card's mixer row. The card that carries IYAKKI's software volume (its only control is IYAKKI's softvol) moves the
/// IYAKKI volume, so the two never disagree and mute keeps the level to come back to.
fn card_row(c: &hw::Card, v: Option<&hw::Volume>, soft_card: Option<u32>, ignored: Option<u32>, prefs: &iyakki_shared::audioout::SoundSettings) -> DevRow {
    let title = if c.long_name.is_empty() { c.name.clone() } else { c.long_name.clone() };
    let soft = soft_card == Some(c.index) && (ignored == Some(c.index) || v.is_none_or(|v| v.control == iyakki_shared::softvol::CONTROL));
    if soft {
        let row = DevRow::new(format!("Sound · {title}"), "no hardware volume · IYAKKI's software volume (the IYAKKI volume above)", Target::SoftVolume).with_slider(SliderVal { value: prefs.volume_pct as i64, min: 0, max: 100, step: 5, unit: "%" });
        return if prefs.muted { row.chip("muted", WARN) } else { row.chip(format!("{}%", prefs.volume_pct), OK) };
    }
    let (sub, chip) = match v {
        Some(v) if v.muted => (format!("{} · muted", v.control), ("muted".to_string(), WARN)),
        Some(v) => (format!("{} volume", v.control), (format!("{}%", v.percent), OK)),
        None => ("no volume control on this card".to_string(), ("no mixer".to_string(), DIM)),
    };
    let target = if v.is_some() { Target::Card(c.index) } else { Target::Nothing };
    let mut row = DevRow::new(format!("Sound · {title}"), sub, target).chip(chip.0, chip.1);
    if let Some(v) = v {
        row = row.with_slider(SliderVal { value: v.percent as i64, min: 0, max: 100, step: 5, unit: "%" });
    }
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_look_at_bluetooth_and_sound_asks_the_device_broker_once_each() {
        let fake = std::sync::Arc::new(iyakki_core_fake::FakeHardware::new());
        let mut pane = DevicesPane::new(DeviceEnv::real());
        pane.set_hardware(fake.clone());
        pane.prime(0);
        pane.prime(1_000_000);
        let asks: Vec<String> = fake.calls().into_iter().filter(|c| c.starts_with("request_class")).collect();
        assert_eq!(asks, vec!["request_class(bluetooth,settings)".to_string(), "request_class(audio,core)".to_string()]);
    }

    fn mic(id: &str, label: &str, usb: bool) -> iyakki_shared::voicedesk::Mic {
        iyakki_shared::voicedesk::Mic { id: id.into(), label: label.into(), usb }
    }

    #[test]
    fn the_microphone_section_says_which_input_is_used_and_why_and_what_to_do_when_there_is_none() {
        let cam = mic("plughw:2,0", "EMEET SmartCam Nova 4K (USB)", true);
        let desk = mic("plughw:3,0", "USB Audio (USB)", true);
        // Automatic: the first one, marked AUTO, with the Test row
        let rows = mic_rows(&[cam.clone(), desk.clone()], "", iyakki_shared::voicedesk::T2Mic::Absent);
        assert_eq!(rows[0].title, "Microphone");
        assert!(rows[0].sub.contains("Automatic") && rows[0].sub.contains("EMEET") && rows[0].chips.iter().any(|c| c.0 == "AUTO"), "{:?}", rows[0]);
        assert!(rows.iter().any(|r| r.target == Target::SoundMicTest), "a Test row when there is a microphone");
        let used: Vec<&str> = rows.iter().filter(|r| r.chips.iter().any(|c| c.0 == "IN USE")).map(|r| r.title.as_str()).collect();
        assert_eq!(used, vec!["EMEET SmartCam Nova 4K (USB)"], "exactly one input is marked in use");
        // a choice that is there
        let rows = mic_rows(&[cam.clone(), desk.clone()], "plughw:3,0", iyakki_shared::voicedesk::T2Mic::Absent);
        assert!(rows[0].chips.iter().any(|c| c.0 == "CHOSEN") && rows[0].sub.contains("USB Audio"), "{:?}", rows[0]);
        assert!(rows.iter().any(|r| r.target == Target::SoundMic("plughw:3,0".into()) && r.chips.iter().any(|c| c.0 == "IN USE")));
        // a choice that was unplugged: said, and the first one is used instead
        let rows = mic_rows(&[cam], "plughw:9,0", iyakki_shared::voicedesk::T2Mic::Absent);
        assert!(rows[0].sub.contains("missing") && rows[0].chips.iter().any(|c| c.0 == "AUTO"), "{:?}", rows[0]);
        // none at all: what to plug in; no Test row; the T2 reason when the built-in microphones are hidden
        let rows = mic_rows(&[], "", iyakki_shared::voicedesk::T2Mic::Absent);
        assert!(rows[0].sub.contains("plug in") && rows[0].chips.iter().any(|c| c.0 == "NO MIC") && rows.iter().all(|r| r.target != Target::SoundMicTest), "{:?}", rows[0]);
        assert!(mic_rows(&[], "", iyakki_shared::voicedesk::T2Mic::Off)[0].sub.contains("hidden on purpose"));
        // the switch for the Mac's own microphone: only where a T2 sound chip is, off or on, never on a PC
        use iyakki_shared::voicedesk::T2Mic;
        let sw = |t2| mic_rows(&[], "", t2).into_iter().find(|r| r.target == Target::SoundT2Mic);
        assert!(sw(T2Mic::Absent).is_none());
        assert!(sw(T2Mic::Off).is_some_and(|r| r.chips.iter().any(|c| c.0 == "off") && r.sub.contains("freeze")));
        assert!(sw(T2Mic::On).is_some_and(|r| r.chips.iter().any(|c| c.0 == "on")));
    }
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    fn put(root: &Path, rel: &str, content: &str) {
        let p = root.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, content).unwrap();
    }

    #[derive(Default)]
    struct Fake {
        calls: Mutex<Vec<String>>,
        volume: Mutex<Option<iyakki_core_api::device_types::devctl::Volume>>,
        fail_links: bool,
    }

    impl Control for Fake {
        fn set_link(&self, n: &str, up: bool) -> Result<(), String> {
            if self.fail_links {
                return Err("refused".into());
            }
            self.calls.lock().unwrap().push(format!("link {n} {}", if up { "up" } else { "down" }));
            Ok(())
        }
        fn ipv4(&self) -> Vec<(String, String)> {
            vec![("eno1".into(), "192.168.1.9".into())]
        }
        fn volume(&self, _: u32) -> Option<iyakki_core_api::device_types::devctl::Volume> {
            self.volume.lock().unwrap().clone()
        }
        fn change_volume(&self, c: u32, d: Option<i32>) -> Result<iyakki_core_api::device_types::devctl::Volume, String> {
            let mut v = self.volume.lock().unwrap();
            let cur = v.as_mut().ok_or("no mixer")?;
            match d {
                Some(d) => cur.percent = (cur.percent as i32 + d).clamp(0, 100) as u32,
                None => cur.muted = !cur.muted,
            }
            self.calls.lock().unwrap().push(format!("volume {c} {d:?}"));
            Ok(cur.clone())
        }
        fn set_volume(&self, c: u32, p: u32) -> Result<iyakki_core_api::device_types::devctl::Volume, String> {
            let mut v = self.volume.lock().unwrap();
            let cur = v.as_mut().ok_or("no mixer")?;
            cur.percent = p.min(100);
            self.calls.lock().unwrap().push(format!("set volume {c} {p}"));
            Ok(cur.clone())
        }
        fn dhcp(&self, n: &str) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("dhcp {n}"));
            Ok(())
        }
    }

    fn machine(tag: &str, full: bool) -> (PathBuf, Roots) {
        let d = std::env::temp_dir().join(format!("iyakki-devpane-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (sys, proc_) = (d.join("sys"), d.join("proc"));
        std::fs::create_dir_all(&sys).unwrap();
        std::fs::create_dir_all(&proc_).unwrap();
        if full {
            put(&sys, "class/net/eno1/type", "1\n");
            put(&sys, "class/net/eno1/flags", "0x1003\n");
            put(&sys, "class/net/eno1/carrier", "1\n");
            put(&sys, "class/net/eno1/address", "aa:bb:cc:00:00:01\n");
            put(&sys, "class/net/eno1/speed", "1000\n");
            put(&sys, "class/net/eno1/device/x", "");
            put(&sys, "class/net/wlan0/type", "1\n");
            put(&sys, "class/net/wlan0/flags", "0x1003\n");
            put(&sys, "class/net/wlan0/carrier", "0\n");
            put(&sys, "class/net/wlan0/address", "aa:bb:cc:00:00:02\n");
            put(&sys, "class/net/wlan0/wireless/x", "");
            put(&sys, "class/net/wlan0/device/x", "");
            put(&sys, "class/rfkill/rfkill0/name", "phy0\n");
            put(&sys, "class/rfkill/rfkill0/type", "wlan\n");
            put(&sys, "class/rfkill/rfkill0/soft", "0\n");
            put(&sys, "class/rfkill/rfkill0/hard", "0\n");
            put(&sys, "class/rfkill/rfkill1/name", "hci0\n");
            put(&sys, "class/rfkill/rfkill1/type", "bluetooth\n");
            put(&sys, "class/rfkill/rfkill1/soft", "0\n");
            put(&sys, "class/rfkill/rfkill1/hard", "0\n");
            put(&sys, "class/graphics/fb0/virtual_size", "1920,1080\n");
            put(&sys, "class/graphics/fb0/bits_per_pixel", "32\n");
            put(&sys, "class/graphics/fb0/name", "simpledrmdrmfb\n");
            put(&sys, "class/drm/card0-HDMI-A-1/status", "connected\n");
            put(&sys, "class/drm/card0-HDMI-A-1/modes", "1920x1080\n1280x720\n");
            put(&sys, "class/backlight/bl0/max_brightness", "100\n");
            put(&sys, "class/backlight/bl0/brightness", "50\n");
            put(&proc_, "asound/cards", " 0 [PCH]: HDA-Intel - HDA Intel PCH\n                      HDA Intel PCH at 0x1 irq 3\n");
        }
        (sys.clone(), Roots { sys, proc_ })
    }

    fn pane(tag: &str, full: bool, fake: Arc<Fake>) -> (DevicesPane, Roots, Arc<Fake>) {
        let (_, roots) = machine(tag, full);
        let mut p = DevicesPane::new(DeviceEnv { roots: roots.clone(), ctl: fake.clone() });
        p.set_core(crate::devpop::scanning_core());
        (p, roots, fake)
    }

    #[test]
    fn the_mixer_status_row_comes_from_the_core_handle_and_a_pane_without_one_shows_it_idle() {
        let core = Arc::new(iyakki_core_fake::FakeCore::new());
        core.mixer_fake().set_stats(Some(iyakki_core_api::MixerStats { sink_open: true, detail: "2 source(s) from the fake".into(), last_error: Some("late".into()) }));
        let (mut p, _, _) = pane("mixer-core", true, Arc::new(Fake::default()));
        p.set_sound(iyakki_shared::audioout::SoundSettings { mixer: true, ..Default::default() });
        let idle = p.rows(DevTab::Sound, 0).into_iter().find(|r| r.title == "Mixer status").expect("status row while the mixer is on");
        assert!(idle.sub.starts_with("idle") && idle.chips[0].0 == "idle", "{idle:?}");
        p.set_core(core.clone());
        let r = p.rows(DevTab::Sound, 0).into_iter().find(|r| r.title == "Mixer status").unwrap();
        assert!(r.sub.starts_with("running") && r.sub.contains("2 source(s) from the fake") && r.sub.contains("last problem: late"), "{r:?}");
        assert_eq!(r.chips[0].0, "running");
        core.mixer_fake().set_stats(None);
        assert_eq!(p.rows(DevTab::Sound, 0).into_iter().find(|r| r.title == "Mixer status").unwrap().chips[0].0, "idle");
    }

    #[test]
    fn a_machine_with_nothing_shows_every_page_as_not_found_and_cannot_be_switched() {
        let (mut p, _, fake) = pane("empty", false, Arc::new(Fake::default()));
        for tab in [DevTab::Network, DevTab::Sound] {
            let rows = p.rows(tab, 0);
            assert!(rows.iter().all(|r| !r.actionable()), "{tab:?}");
            assert!(rows.iter().any(|r| r.chips.iter().any(|c| c.0 == "not found")), "{tab:?} says nothing was detected");
        }
        let mut bt = crate::btmodel::BtModel::new(fake.clone(), p.env.roots.clone());
        bt.poll(0);
        assert_eq!(bt.view.switch, crate::btmodel::Switch::NoAdapter, "Bluetooth says nothing was detected");
        let net = p.rows(DevTab::Network, 0);
        assert_eq!(net.len(), 2, "one line each for Ethernet and WiFi");
        assert!(net[1].sub.contains("WiFi stays disabled"));
        assert!(p.act(DevTab::Network, 1, &Key::Enter, 0).unwrap().result.is_err(), "pressing Enter on a missing adapter explains why");
        assert!(p.act(DevTab::Network, 1, &Key::Char('x'), 0).is_none(), "other keys do nothing");
        assert!(fake.calls.lock().unwrap().is_empty(), "nothing was touched");
    }

    #[test]
    fn a_broadcom_4360_card_without_a_driver_is_named_on_the_wifi_row() {
        let (mut p, roots, _) = pane("bcm", false, Arc::new(Fake::default()));
        put(&roots.sys, "bus/pci/devices/0000:03:00.0/vendor", "0x14e4\n");
        put(&roots.sys, "bus/pci/devices/0000:03:00.0/device", "0x43a0\n");
        let net = p.rows(DevTab::Network, 0);
        let w = net.iter().find(|r| r.title == "WiFi").unwrap();
        assert!(w.sub.contains("Broadcom BCM43xx") && w.sub.contains("USB Wi-Fi adapter") && w.sub.contains("tether"), "{}", w.sub);
        assert!(w.chips.iter().any(|c| c.0 == "no driver"));
    }

    #[test]
    fn ethernet_and_wifi_rows_show_their_state() {
        let (mut p, _, _) = pane("net", true, Arc::new(Fake::default()));
        let rows = p.rows(DevTab::Network, 0);
        assert_eq!(rows.len(), 4, "the two adapters, then the private MAC address row of each");
        assert!(rows[2].title.contains("Private MAC") && rows[2].title.contains("eno1") && matches!(rows[2].target, Target::Mac(..)) && rows[3].title.contains("wlan0"), "{:?}", rows);
        assert!(rows[0].title.contains("eno1") && rows[0].sub.contains("192.168.1.9") && rows[0].sub.contains("1000 Mb/s"), "{:?}", rows[0]);
        assert_eq!(rows[0].chips[0].0, "ON");
        assert!(rows[1].title.contains("wlan0") && rows[1].sub.contains("not connected"), "{:?}", rows[1]);
    }

    #[test]
    fn switching_ethernet_off_and_on_goes_through_the_link_and_asks_for_an_address() {
        let (mut p, _, fake) = pane("eth", true, Arc::new(Fake::default()));
        let d = p.act(DevTab::Network, 0, &Key::Enter, 0).unwrap();
        assert_eq!(d.result.unwrap(), "Ethernet eno1 turned off.");
        assert_eq!(d.audit, "ethernet eno1 off");
        assert_eq!(fake.calls.lock().unwrap().clone(), vec!["link eno1 down"]);
        // the (fake) kernel now reports it down
        put(&machine("eth", true).0, "class/net/eno1/flags", "0x1002\n");
        let rows = p.rows(DevTab::Network, 1);
        assert_eq!(rows[0].chips[0].0, "OFF");
        let d = p.act(DevTab::Network, 0, &Key::Char('e'), 1).unwrap();
        assert_eq!(d.result.unwrap(), "Ethernet eno1 turned on.");
        assert_eq!(fake.calls.lock().unwrap()[1..], ["link eno1 up", "dhcp eno1"]);
    }

    #[test]
    fn the_link_switch_and_the_card_volume_go_through_the_hardware_handle_the_core_gave_and_not_the_control() {
        use iyakki_core_api::hardware::{Card, Link, LinkKind};
        let (mut p, _, ctl) = pane("via-hw", true, Arc::new(Fake::default()));
        let hw = Arc::new(iyakki_core_fake::FakeHardware::new());
        hw.set(|s| {
            s.links = vec![Link { name: "eno1".into(), kind: LinkKind::Ethernet, up: true, carrier: true, mac: "aa:bb:cc:00:00:01".into(), speed: Some(1000), driver: String::new() }];
            s.cards = vec![(Card { index: 0, name: "PCH".into(), long_name: "HDA Intel PCH".into() }, Some(iyakki_core_api::device_types::devctl::Volume { control: "Master".into(), percent: 50, muted: false }))];
        });
        p.set_hardware(hw.clone());
        assert_eq!(p.act(DevTab::Network, 0, &Key::Enter, 0).unwrap().result.unwrap(), "Ethernet eno1 turned off.");
        assert_eq!(p.act(DevTab::Network, 0, &Key::Char('e'), 1).unwrap().result.unwrap(), "Ethernet eno1 turned on.");
        let c = p.rows(DevTab::Sound, 2).iter().position(|r| matches!(r.target, Target::Card(0))).unwrap();
        assert_eq!(p.act(DevTab::Sound, c, &Key::Char('+'), 3).unwrap().result.unwrap(), "Volume 55%.");
        assert_eq!(p.set_value(DevTab::Sound, c, 20, 4).unwrap().result.unwrap(), "Volume 20%.");
        assert_eq!(hw.calls().into_iter().filter(|c| !c.starts_with("request_class")).collect::<Vec<_>>(), vec!["set_link(eno1,false)", "set_link(eno1,true)", "dhcp(eno1)", "change_volume(0,Some(5))", "set_volume(0,20)"]);
        assert!(ctl.calls.lock().unwrap().is_empty(), "the control the environment holds was not used for these: {:?}", ctl.calls.lock().unwrap());
    }

    #[test]
    fn switching_wifi_off_blocks_the_radio_and_on_unblocks_it() {
        let (mut p, roots, fake) = pane("wifi", true, Arc::new(Fake::default()));
        let d = p.act(DevTab::Network, 1, &Key::Enter, 0).unwrap();
        assert_eq!(d.result.unwrap(), "WiFi wlan0 turned off.");
        assert!(iyakki_core_api::device_types::devctl::rfkill_list(&roots).iter().find(|k| k.kind == "wlan").unwrap().soft, "radio blocked");
        put(&roots.sys, "class/net/wlan0/flags", "0x1002\n");
        let rows = p.rows(DevTab::Network, 1);
        assert_eq!(rows[1].chips[0].0, "OFF");
        let d = p.act(DevTab::Network, 1, &Key::Enter, 1).unwrap();
        let said = d.result.unwrap();
        // WiFi is private per network by default; this stand-in driver cannot change an address, which is said, not fatal
        assert!(said.starts_with("WiFi wlan0 turned on.") && said.contains("Private MAC address not applied") && said.contains("hardware address is used"), "{said}");
        assert!(!iyakki_core_api::device_types::devctl::rfkill_list(&roots).iter().find(|k| k.kind == "wlan").unwrap().soft, "radio free again");
        assert!(fake.calls.lock().unwrap().contains(&"dhcp wlan0".to_string()));
    }

    #[test]
    fn a_hardware_switch_cannot_be_overridden() {
        let (mut p, roots, _) = pane("hard", true, Arc::new(Fake::default()));
        put(&roots.sys, "class/rfkill/rfkill0/hard", "1\n");
        put(&roots.sys, "class/rfkill/rfkill1/hard", "1\n");
        let rows = p.rows(DevTab::Network, 0);
        assert!(!rows[1].actionable(), "{:?}", rows[1]);
        assert!(rows[1].sub.contains("switched off by a key"), "{:?}", rows[1]);
        let mut bt = crate::btmodel::BtModel::new(Arc::new(Fake::default()), roots.clone());
        bt.poll(0);
        assert_eq!(bt.view.switch, crate::btmodel::Switch::BlockedByHardware);
        assert!(bt.set_switch(true).unwrap_err().contains("hardware switch"), "a hardware block is reported, never worked around");
    }

    #[test]
    fn a_failing_switch_reports_the_error_and_changes_nothing_else() {
        let (mut p, _, _) = pane("fail", true, Arc::new(Fake { fail_links: true, ..Fake::default() }));
        let d = p.act(DevTab::Network, 0, &Key::Enter, 0).unwrap();
        assert_eq!(d.result.unwrap_err(), "refused");
        assert!(d.audit.is_empty(), "nothing changed, nothing to audit");
    }

    #[test]
    fn bluetooth_can_be_switched_and_detection_covers_a_usb_adapter_without_a_driver() {
        let (_, roots) = machine("bt", true);
        let mut m = crate::btmodel::BtModel::new(Arc::new(Fake::default()), roots.clone());
        m.poll(0);
        assert_eq!(m.view.switch, crate::btmodel::Switch::On);
        assert_eq!(m.set_switch(false).unwrap(), "Bluetooth is off.");
        assert!(iyakki_core_api::device_types::devctl::rfkill_list(&roots).iter().find(|k| k.kind == "bluetooth").unwrap().soft);
        assert_eq!(m.set_switch(true).unwrap(), "The Bluetooth radio is on.");
        assert!(!iyakki_core_api::device_types::devctl::rfkill_list(&roots).iter().find(|k| k.kind == "bluetooth").unwrap().soft);
        let (_, r) = machine("btusb", false);
        put(&r.sys, "bus/usb/devices/1-1/bDeviceClass", "e0\n");
        let mut q = crate::btmodel::BtModel::new(Arc::new(Fake::default()), r);
        q.poll(0);
        assert_eq!(q.view.switch, crate::btmodel::Switch::NoDriver);
    }

    #[test]
    fn the_phones_audio_is_read_connected_disconnected_and_opened_for_pairing_through_the_cores_handle_not_the_control() {
        let (_, roots) = machine("btaudio", true);
        let h = Arc::new(iyakki_core_fake::FakeBtAudio::default());
        h.set(|s| s.paired = vec![iyakki_core_fake::BtPaired { address: "AA:01".into(), name: "Pixel 8".into(), kind: "phone".into() }]);
        let mut m = crate::btmodel::BtModel::new(Arc::new(Fake::default()), roots);
        m.set_bt_audio(h.clone());
        m.poll(0);
        assert!(m.audio.ready && m.audio.device.is_none(), "the view is the handle's");
        assert!(m.audio_toggle("AA:01").unwrap().starts_with("Connecting for audio"));
        m.poll(1);
        assert_eq!(m.audio.device.as_ref().map(|d| d.address.as_str()), Some("AA:01"));
        assert!(m.audio_toggle("AA:01").unwrap().contains("Disconnecting the audio"));
        m.poll(2);
        assert!(m.audio_pairing_toggle().unwrap().contains("visible to phones"));
        m.poll(3);
        assert!(m.audio.pairing_open && m.audio.device.is_none());
        assert_eq!(h.calls(), vec!["apply(bluetooth_audio_connect)", "apply(bluetooth_audio_disconnect)", "apply(bluetooth_audio_pairing)"]);
        // a phone that is not paired is the handle's refusal, and nothing is asked of the control
        assert!(m.audio_toggle("ZZ:99").unwrap_err().contains("no phone or laptop"));
    }

    #[test]
    fn brightness_moves_in_steps_and_never_goes_dark() {
        let (mut p, roots, _) = pane("bl", true, Arc::new(Fake::default()));
        let rows = p.rows(DevTab::Display, 0);
        assert!(rows[0].sub.contains("1920x1080"));
        let bl = rows.iter().position(|r| matches!(r.target, Target::Backlight(_))).unwrap();
        assert!(rows.iter().any(|r| r.title.contains("HDMI-A-1") && r.sub.contains("best 1920x1080")));
        assert_eq!(p.act(DevTab::Display, bl, &Key::Char('+'), 0).unwrap().result.unwrap(), "Brightness 60%.");
        assert_eq!(iyakki_core_api::device_types::devctl::screen(&roots).backlights[0].value, 60);
        for t in 1..12 {
            p.act(DevTab::Display, bl, &Key::Char('-'), t);
        }
        assert_eq!(iyakki_core_api::device_types::devctl::screen(&roots).backlights[0].value, 5, "the floor is 5 %");
        assert!(p.act(DevTab::Display, 0, &Key::Char('+'), 20).unwrap().result.is_err(), "the screen row has no brightness");
    }

    #[test]
    fn screen_in_use_shows_the_size_the_shell_drives_and_look_again_is_a_row() {
        // sysfs said 1920x1080 at the last refresh, the shell draws 3840x2160
        let fb = "1920x1080 · 32 bit · i915drmfb";
        assert_eq!(screen_in_use(Some(fb), Some((3840, 2160)), &[]), "3840x2160 drawn by the shell · framebuffer 1920x1080 · 32 bit · i915drmfb");
        assert_eq!(screen_in_use(Some(fb), Some((1920, 1080)), &[]), fb, "same size: the framebuffer line as before");
        assert_eq!(screen_in_use(Some(fb), None, &[]), fb);
        assert_eq!(screen_in_use(None, Some((3840, 2160)), &[]), "3840x2160 drawn by the shell");
        assert_eq!(screen_in_use(None, None, &[]), "no framebuffer found");
        assert_eq!(screen_in_use(Some(fb), Some((1920, 1080)), &[("DP-1".into(), (3840, 2160)), ("DP-3".into(), (1920, 1080))]), "DP-1 3840x2160, DP-3 1920x1080 (the shell draws on each display itself)");
        // the row, and Enter on it reads the displays again through the pane's own sysfs
        let (mut p, roots, _) = pane("look", true, Arc::new(Fake::default()));
        let rows = p.rows(DevTab::Display, 0);
        let at = rows.iter().position(|r| r.target == Target::LookAgain).expect("a Look again row");
        assert!(Target::LookAgain.activates_on_click());
        let done = p.act(DevTab::Display, at, &Key::Enter, 1).unwrap();
        assert!(done.result.unwrap().contains("Looking at the displays again"));
        assert_eq!(done.audit, "display look again");
        assert!(roots.sys.join("class/drm").exists());
    }

    #[test]
    fn volume_and_mute_go_to_the_card() {
        let fake = Arc::new(Fake::default());
        *fake.volume.lock().unwrap() = Some(iyakki_core_api::device_types::devctl::Volume { control: "Master".into(), percent: 50, muted: false });
        let (mut p, _, fake) = pane("snd", true, fake);
        let rows = p.rows(DevTab::Sound, 0);
        assert_eq!(rows.len(), 10, "the output, IYAKKI's volume, the card, the mixer switch, the browser mixer switch, the Bluetooth headphones row (alpha.7 d5), its status, the mixer status (the mixer is on by default), the Microphone row (no microphone in the test machine), the Advanced switch: {rows:?}");
        let c = rows.iter().position(|r| matches!(r.target, Target::Card(0))).unwrap();
        assert!(rows[c].title.contains("HDA Intel PCH") && rows[c].chips[0].0 == "50%");
        assert_eq!(p.act(DevTab::Sound, c, &Key::Char('+'), 0).unwrap().result.unwrap(), "Volume 55%.");
        assert_eq!(p.act(DevTab::Sound, c, &Key::Char('-'), 1).unwrap().result.unwrap(), "Volume 50%.");
        assert_eq!(p.act(DevTab::Sound, c, &Key::Char('m'), 2).unwrap().result.unwrap(), "Muted.");
        assert_eq!(p.rows(DevTab::Sound, 3)[c].chips[0].0, "muted");
        assert_eq!(fake.calls.lock().unwrap().len(), 3);
    }

    #[test]
    fn the_card_carrying_iyakkis_software_volume_moves_the_iyakki_volume_and_others_keep_their_mixer() {
        let t2 = iyakki_core_api::device_types::devctl::Card { index: 1, name: "AppleT2x1".into(), long_name: "Apple T2 Audio".into() };
        let prefs = iyakki_shared::audioout::SoundSettings { volume_pct: 35, ..Default::default() };
        let soft = iyakki_core_api::device_types::devctl::Volume { control: iyakki_shared::softvol::CONTROL.into(), percent: 35, muted: false };
        // the Mac mini 2018's T2 card: its only control is the softvol, so the row is the IYAKKI volume (mute keeps the level)
        for v in [Some(&soft), None] {
            let r = card_row(&t2, v, Some(1), None, &prefs);
            assert!(matches!(r.target, Target::SoftVolume) && r.sub.contains("software volume"), "{r:?}");
            assert_eq!(r.slider.unwrap().value, 35);
            assert_eq!(r.chips[0].0, "35%");
        }
        let muted = iyakki_shared::audioout::SoundSettings { muted: true, ..prefs.clone() };
        assert_eq!(card_row(&t2, Some(&soft), Some(1), None, &muted).chips[0].0, "muted");
        // another card, or a card with a hardware control, keeps its own mixer
        let pch = iyakki_core_api::device_types::devctl::Card { index: 0, name: "HDA-Intel".into(), long_name: "HDA Intel PCH".into() };
        assert!(matches!(card_row(&pch, None, Some(1), None, &prefs).target, Target::Nothing));
        let master = iyakki_core_api::device_types::devctl::Volume { control: "Master".into(), percent: 70, muted: false };
        assert!(matches!(card_row(&pch, Some(&master), Some(0), None, &prefs).target, Target::Card(0)), "a hardware Master stays the card's own");
        assert!(matches!(card_row(&t2, Some(&soft), None, None, &prefs).target, Target::Card(1)), "without IYAKKI's softvol in use the control is a plain mixer");
    }

    #[test]
    fn a_macbook_with_a_cs8409_codec_and_no_driver_says_so_instead_of_an_empty_mixer() {
        let (mut p, roots, _) = pane("cs8409", true, Arc::new(Fake::default()));
        assert!(!p.rows(DevTab::Sound, 0).iter().any(|r| r.title == "Built-in speakers"), "another machine: no such row");
        put(&roots.sys, "class/dmi/id/product_name", "MacBookPro14,1\n");
        put(&roots.proc_, "asound/card0/codec#0", "Codec: Cirrus Logic Generic\nAddress: 0\nVendor Id: 0x10138409\nSubsystem Id: 0x106b3300\n");
        p.invalidate();
        let rows = p.rows(DevTab::Sound, 0);
        let r = rows.iter().find(|r| r.title == "Built-in speakers").expect("the explaining row");
        assert!(r.sub.starts_with("This Mac's speakers need the CS8409 driver (not loaded)") && r.sub.contains("MacBookPro14,1"), "{}", r.sub);
        assert!(!r.actionable() && r.chips[0].0 == "no driver");
    }

    #[test]
    fn a_card_without_a_mixer_is_listed_but_not_adjustable() {
        let (mut p, _, _) = pane("nomix", true, Arc::new(Fake::default()));
        let rows = p.rows(DevTab::Sound, 0);
        let c = rows.iter().position(|r| r.title.contains("HDA Intel PCH")).unwrap();
        assert!(!rows[c].actionable() && rows[c].sub.contains("no volume control"));
    }

    #[test]
    fn the_hardware_is_looked_at_again_every_two_seconds_or_after_a_change() {
        let (mut p, roots, _) = pane("refresh", true, Arc::new(Fake::default()));
        assert_eq!(p.rows(DevTab::Network, 0)[0].chips[0].0, "ON");
        put(&roots.sys, "class/net/eno1/flags", "0x1002\n");
        assert_eq!(p.rows(DevTab::Network, 30)[0].chips[0].0, "ON", "within two seconds the picture is reused");
        assert_eq!(p.rows(DevTab::Network, 61)[0].chips[0].0, "OFF", "then it is refreshed");
    }

    #[test]
    fn display_and_sound_rows_carry_a_slider_and_a_click_on_it_sets_an_exact_value() {
        let fake = Arc::new(Fake::default());
        *fake.volume.lock().unwrap() = Some(iyakki_core_api::device_types::devctl::Volume { control: "Master".into(), percent: 50, muted: false });
        let (mut p, roots, fake) = pane("slide", true, fake);
        let rows = p.rows(DevTab::Display, 0);
        let bl = rows.iter().position(|r| matches!(r.target, Target::Backlight(_))).unwrap();
        assert_eq!(rows[bl].slider, Some(SliderVal { value: 50, min: 5, max: 100, step: 5, unit: "%" }));
        assert_eq!(p.set_value(DevTab::Display, bl, 85, 0).unwrap().result.unwrap(), "Brightness 85%.");
        assert_eq!(iyakki_core_api::device_types::devctl::screen(&roots).backlights[0].value, 85);
        assert_eq!(p.set_value(DevTab::Display, bl, 0, 1).unwrap().result.unwrap(), "Brightness 5%.", "the lowest brightness is the floor");
        assert!(p.set_value(DevTab::Display, 0, 50, 2).is_none(), "the screen row has no slider");
        let rows = p.rows(DevTab::Sound, 3);
        let c = rows.iter().position(|r| matches!(r.target, Target::Card(0))).unwrap();
        assert_eq!(rows[c].slider, Some(SliderVal { value: 50, min: 0, max: 100, step: 5, unit: "%" }));
        assert_eq!(p.set_value(DevTab::Sound, c, 30, 4).unwrap().result.unwrap(), "Volume 30%.");
        assert_eq!(p.set_value(DevTab::Sound, c, 999, 5).unwrap().result.unwrap(), "Volume 100%.", "values are kept in range");
        assert!(fake.calls.lock().unwrap().contains(&"set volume 0 30".to_string()));
        // a card without a mixer has no slider
        let (mut q, _, _) = pane("nomixer2", true, Arc::new(Fake::default()));
        let rows = q.rows(DevTab::Sound, 0);
        assert!(rows.iter().find(|r| r.title.contains("HDA Intel PCH")).unwrap().slider.is_none());
    }

    #[test]
    fn a_slow_starting_card_is_said_and_the_advanced_rows_come_and_go() {
        let (mut p, roots, _) = pane("slow", true, Arc::new(Fake::default()));
        put(&roots.sys, "class/dmi/id/product_name", "MacBookPro14,1\n");
        put(&roots.proc_, "asound/card0/codec#0", "Codec: Cirrus Logic CS8409\n");
        p.invalidate();
        let rows = p.rows(DevTab::Sound, 0);
        let r = rows.iter().find(|r| r.title == "Built-in speakers" && r.chips.iter().any(|c| c.0 == "starting"));
        assert!(r.is_some_and(|r| r.sub.contains("waiting for the sound card") && r.sub.contains("start up slowly")), "{rows:?}");
        // hidden by default; A shows them; the stereo test is always there, the channel walk only for four channels
        assert!(!rows.iter().any(|r| matches!(r.target, Target::SoundPattern(_))));
        assert!(p.toggle_advanced());
        p.invalidate();
        let rows = p.rows(DevTab::Sound, 1);
        assert!(rows.iter().any(|r| matches!(r.target, Target::SoundPattern(false))) || rows.iter().all(|r| !matches!(r.target, Target::SoundPattern(_))), "{rows:?}");
        assert!(!rows.iter().any(|r| matches!(r.target, Target::SoundPattern(true))), "no four-channel chain here");
        assert!(rows.iter().any(|r| matches!(r.target, Target::SoundAdvanced) && r.chips[0].0 == "shown"));
    }

    #[test]
    fn the_display_and_radio_rows_follow_a_fake_machine_and_the_switches_reach_it() {
        use iyakki_core_api::hardware::{Backlight, Rfkill, Screen};
        use iyakki_core_fake::FakeOs;
        use iyakki_core_api::Os;
        let os = FakeOs::new();
        let fake = os.hardware_fake();
        fake.set(|s| {
            s.screen = Screen { framebuffer: Some("1920x1080 · 32 bit · fake".into()), outputs: vec![], backlights: vec![Backlight { name: "panel".into(), value: 50, max: 100 }] };
            s.rfkill = vec![Rfkill { id: "rfkill1".into(), name: "hci0".into(), kind: "wlan".into(), soft: false, hard: false }];
            s.links = vec![hw::Link { name: "wlan0".into(), kind: LinkKind::Wifi, up: true, carrier: true, mac: "02:00:00:00:00:01".into(), speed: None, driver: "fake".into() }];
        });
        let (mut p, _, _) = pane("fakeos", false, Arc::new(Fake::default()));
        p.set_hardware(os.hardware());
        let rows = p.rows(DevTab::Display, 0);
        let bl = rows.iter().position(|r| matches!(r.target, Target::Backlight(_))).expect("the fake's backlight is a row");
        assert_eq!(p.act(DevTab::Display, bl, &Key::Char('+'), 0).unwrap().result.unwrap(), "Brightness 60%.");
        assert_eq!(fake.screen().backlights[0].value, 60, "the change reached the fake machine");
        for t in 1..12 {
            p.act(DevTab::Display, bl, &Key::Char('-'), t);
        }
        assert_eq!(fake.screen().backlights[0].value, 5, "the floor is 5 %");
        assert!(fake.calls().iter().any(|c| c == "set_brightness(panel,60)"), "{:?}", fake.calls());
        // switching WiFi off blocks its radio on the fake machine
        let rows = p.rows(DevTab::Network, 20);
        let r = rows.iter().position(|r| matches!(r.target, Target::Wifi(..))).expect("the fake's WiFi interface is a row");
        p.act(DevTab::Network, r, &Key::Enter, 21).unwrap().result.unwrap();
        assert!(fake.rfkill()[0].soft, "the radio was blocked on the fake machine");
    }
    #[test]
    fn the_output_list_and_the_starting_card_come_from_the_cores_sound_route_not_the_pages_own_reads() {
        // the page's folders hold no sound at all: whatever it lists is what the core's route said
        let (_, roots) = machine("route-only", false);
        let core = Arc::new(iyakki_core_fake::FakeCore::new());
        let route = core.audio_route_fake();
        route.put("cards", " 0 [PCH            ]: HDA-Intel - HDA Intel PCH\n");
        route.put("card0/pcm0p/info", "id: ALC\nname: ALC3234 Analog\nsubdevices_count: 1\n");
        core.sound_fake().set_route_scanner(|r| iyakki_agent::audioout::scan_via(r, &|_| None));
        let mut p = DevicesPane::new(DeviceEnv { roots, ctl: Arc::new(Fake::default()) });
        p.set_core(core.clone());
        let outs = p.outputs(0);
        assert!(!outs.is_empty(), "the card named by the route is listed: {outs:?}");
        assert!(route.calls().iter().any(|c| c == "text:cards"), "the facts were asked of the route: {:?}", route.calls());
    }
}
