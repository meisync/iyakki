//! The view model of Settings -> Bluetooth.
//!
//! The page (`apps/bluetooth_page.rs`) is a tile: it draws and handles keys, and talks to the machine only through this model, which
//! holds a `Control` trait object (a fake one in tests) and the hardware roots. All reading of the adapter, switching, scanning, pairing
//! and the per-device connections happen here, and so does the bookkeeping the page needs: the single switch with its reason, the paired
//! devices with their state, a short sorted list of nearby devices that remembers names, one attempt at a time per device, the last
//! outcome of each device, and the lines for the system log (outcomes, never PINs or passkeys).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub use iyakki_devctl::a2dp::{AudioState, AudioView};
pub use iyakki_devctl::bt_pan::{TetherState, TetherView};
pub use iyakki_devctl::bthid::{DeviceView, LinkState, PairState};
pub use iyakki_devctl::btmgmt::{Found, Kind};
pub use iyakki_devctl::{Control, Roots};
use iyakki_devctl::{bthid, btmgmt, btstate, btview};

/// A scan stops by itself after this many seconds.
pub const SCAN_SECS: u64 = 15;
/// The page ticks about this often per second.
pub const TICKS_PER_SEC: u64 = 30;
/// Hardware is looked at again after this many ticks (the live parts are read at every tick).
const REFRESH_TICKS: u64 = 60;
/// A connect that shows no result after this many ticks may be tried again (25 s).
const PENDING_TICKS: u64 = 25 * TICKS_PER_SEC;
/// Nearby devices shown before "Show N more devices".
pub const NEARBY_SHOWN: usize = 8;
/// A scan keeps at most this many devices.
const NEARBY_KEPT: usize = 120;

// ------------------------------------------------------------------------------------------------------------ the switch

/// What the one Bluetooth switch says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    On,
    Off,
    BlockedByHardware,
    NoAdapter,
    /// A USB adapter is plugged in but no driver took it.
    NoDriver,
    /// The drivers are set aside until firmware exists, nothing is listed.
    Held,
    NeedsFirmware,
    NotSetUp,
    /// The kernel made a controller but it does not answer.
    NoAnswer,
}

impl Switch {
    /// The state in words.
    pub fn words(self) -> &'static str {
        match self {
            Switch::On => "On",
            Switch::Off => "Off",
            Switch::BlockedByHardware => "Blocked by the hardware switch",
            Switch::NoAdapter => "No Bluetooth adapter",
            Switch::NoDriver => "Adapter found, waiting for its driver",
            Switch::Held => "Driver not loaded",
            Switch::NeedsFirmware => "Needs firmware: Install",
            Switch::NotSetUp => "Controller not set up",
            Switch::NoAnswer => "Controller not answering",
        }
    }

    /// Why it cannot be turned on (empty when it can).
    pub fn why(self) -> &'static str {
        match self {
            Switch::On | Switch::Off => "",
            Switch::BlockedByHardware => "A key or switch on the machine has Bluetooth off. Switch it on there; this page cannot undo it.",
            Switch::NoAdapter => "No Bluetooth adapter was found in this computer. A USB adapter works once it is plugged in.",
            Switch::NoDriver => "A USB Bluetooth adapter is plugged in, but no driver has taken it yet.",
            Switch::Held => "The Bluetooth drivers are set aside until firmware exists. Install the firmware, or load the driver anyway (under Advanced).",
            Switch::NeedsFirmware => "The controller is found but runs in its start-up state: it has no address until its firmware file is installed. The file is fetched for your own Mac at your click, never shipped.",
            Switch::NotSetUp => "The controller has no valid address, so the kernel will not use it.",
            Switch::NoAnswer => "The kernel made the controller but it never answered: a firmware or serial-link problem. See TROUBLESHOOTING.",
        }
    }

    /// The switch can be pressed.
    pub fn can_toggle(self) -> bool {
        matches!(self, Switch::On | Switch::Off)
    }
}

/// A fix offered on the switch card.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fix {
    Firmware,
    LoadDriver,
}

/// The controller and the radio as the page shows them.
#[derive(Clone, Debug)]
pub struct BtView {
    pub state: btstate::BtState,
    pub switch: Switch,
    /// `Controller hci0: BCM4364 ...` and its second line.
    pub headline: Option<String>,
    pub detail: Option<String>,
    pub address: Option<String>,
    pub powered: bool,
    pub radio_on: bool,
}

impl BtView {
    pub fn fixes(&self) -> Vec<Fix> {
        let mut v = Vec::new();
        if self.switch == Switch::NeedsFirmware {
            v.push(Fix::Firmware);
        }
        if self.state.held && matches!(self.switch, Switch::Held | Switch::NeedsFirmware) {
            v.push(Fix::LoadDriver);
        }
        v
    }
}

fn classify(st: &btstate::BtState) -> Switch {
    if st.needs_firmware {
        return Switch::NeedsFirmware;
    }
    if st.hard_blocked() {
        return Switch::BlockedByHardware;
    }
    if let Some(c) = st.controller() {
        return if !c.configured {
            Switch::NotSetUp
        } else if st.soft_blocked() || !c.powered() {
            Switch::Off
        } else {
            Switch::On
        };
    }
    if st.held && st.adapters.is_empty() {
        return Switch::Held;
    }
    if !st.adapters.is_empty() {
        return Switch::NoAnswer;
    }
    if st.usb_present {
        return Switch::NoDriver;
    }
    if !st.switches.is_empty() {
        return if st.soft_blocked() { Switch::Off } else { Switch::On };
    }
    Switch::NoAdapter
}

// ------------------------------------------------------------------------------------------------------------ rows

/// The kind of device an icon and a word are chosen for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    Keyboard,
    Mouse,
    GamePad,
    Phone,
    Headphones,
    Other,
}

impl Icon {
    pub fn of_label(label: &str) -> Icon {
        match label {
            "keyboard" => Icon::Keyboard,
            "mouse" => Icon::Mouse,
            "game pad" => Icon::GamePad,
            "phone" => Icon::Phone,
            "headphones" | "speaker" => Icon::Headphones,
            _ => Icon::Other,
        }
    }
    pub fn of_kind(k: Kind) -> Icon {
        Icon::of_label(bthid::kind_label(k))
    }
    pub fn word(self) -> &'static str {
        match self {
            Icon::Keyboard => "Keyboard",
            Icon::Mouse => "Mouse",
            Icon::GamePad => "Game pad",
            Icon::Phone => "Phone",
            Icon::Headphones => "Headphones",
            Icon::Other => "Device",
        }
    }
}

/// How a chip is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Good,
    Quiet,
    Warn,
}

/// What a button on a paired row does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowAct {
    /// Connect or disconnect an input device.
    Connect,
    Tether,
    Audio,
    Forget,
}

/// One paired device as the page shows it.
#[derive(Clone, Debug)]
pub struct PairedRow {
    pub address: String,
    pub name: String,
    pub icon: Icon,
    pub chip: &'static str,
    pub tone: Tone,
    /// The line under the name: the last outcome or the state.
    pub detail: String,
    /// The buttons, the first one is also what Enter and a click on the row do.
    pub buttons: Vec<(RowAct, String)>,
}

/// One device the scan saw.
#[derive(Clone, Debug)]
pub struct NearbyRow {
    pub address: String,
    pub name: String,
    pub named: bool,
    pub icon: Icon,
    pub rssi: i8,
    pub signal: &'static str,
    pub pairable: bool,
    /// Why it cannot be paired here (empty when it can).
    pub note: String,
}

/// The nearby list for a query.
#[derive(Clone, Debug, Default)]
pub struct Nearby {
    pub rows: Vec<NearbyRow>,
    /// Devices not in `rows` (behind "Show N more devices").
    pub more: usize,
    /// Everything the scan saw that is not paired yet.
    pub total: usize,
}

fn signal_word(rssi: i8) -> &'static str {
    if rssi >= -60 {
        "strong signal"
    } else if rssi >= -75 {
        "good signal"
    } else {
        "weak signal"
    }
}

fn pairable(f: &Found) -> bool {
    bthid::is_pairable_kind(f.kind) && (f.addr_type == btmgmt::ADDR_BREDR || bthid::is_hid_kind(f.kind))
}

/// Sort rank: 0 named and pairable, 1 pairable, 2 named, 3 nothing useful (hidden behind Show more).
fn rank(f: &Found, named: bool) -> u8 {
    match (pairable(f), named) {
        (true, true) => 0,
        (true, false) => 1,
        (false, true) => 2,
        _ => 3,
    }
}

fn key(address: &str) -> String {
    address.to_ascii_uppercase()
}

enum ScanSlot {
    Idle,
    Running(u64),
    Done(Result<Vec<Found>, String>),
}

/// What the scan is doing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScanView {
    Idle,
    /// Seconds since it started.
    Running(u64),
}

// ------------------------------------------------------------------------------------------------------------ the model

pub struct BtModel {
    ctl: Arc<dyn Control>,
    roots: Roots,
    pub view: BtView,
    pub devices: Vec<DeviceView>,
    pub pair: PairState,
    pub tether: TetherView,
    pub tether_ip: Option<String>,
    pub audio: AudioView,
    seen_hw_at: Option<u64>,
    tick: u64,
    scan: Arc<Mutex<ScanSlot>>,
    stop: Arc<AtomicBool>,
    scan_error: Option<String>,
    /// The latest scan, with the names the earlier scans knew.
    found: Vec<Found>,
    names: HashMap<String, (String, Kind)>,
    /// Connections being started: key -> tick after which a new try is allowed.
    pending: HashMap<String, u64>,
    attempts: HashMap<String, u32>,
    last: HashMap<String, String>,
    log: Vec<String>,
    /// The device the person asked to pair (address), and what to retry with.
    pub pair_target: Option<Found>,
    prev: Option<Live>,
    logged_open: bool,
}

#[derive(Clone)]
struct Live {
    devices: Vec<DeviceView>,
    pair: PairState,
    tether: TetherView,
    audio: AudioView,
}

impl BtModel {
    pub fn new(ctl: Arc<dyn Control>, roots: Roots) -> BtModel {
        // nothing is read until the page is shown (opening Settings must not ask the Bluetooth controller)
        let state = btstate::BtState { adapters: vec![], usb_present: false, switches: vec![], unstarted: vec![], controllers: Ok(vec![]), product: String::new(), fw_file: false, held: false, needs_firmware: false };
        let view = Self::view_of(state);
        BtModel {
            ctl,
            roots,
            view,
            devices: Vec::new(),
            pair: PairState::Idle,
            tether: TetherView::idle(),
            tether_ip: None,
            audio: AudioView::default(),
            seen_hw_at: None,
            tick: 0,
            scan: Arc::new(Mutex::new(ScanSlot::Idle)),
            stop: Arc::new(AtomicBool::new(false)),
            scan_error: None,
            found: Vec::new(),
            names: HashMap::new(),
            pending: HashMap::new(),
            attempts: HashMap::new(),
            last: HashMap::new(),
            log: Vec::new(),
            pair_target: None,
            prev: None,
            logged_open: false,
        }
    }

    fn view_of(state: btstate::BtState) -> BtView {
        let switch = classify(&state);
        let chip = iyakki_agent::macfw::uart_chip(&state.product);
        let (headline, detail, address, powered) = match state.controller() {
            Some(c) => (Some(btview::headline(c, chip, state.fw_file)), Some(btview::detail(c)), Some(c.address.clone()), c.powered()),
            None => (None, None, None, false),
        };
        let radio_on = !state.switches.is_empty() && !state.soft_blocked() && !state.hard_blocked();
        BtView { state, switch, headline, detail, address, powered, radio_on }
    }

    /// Look at the hardware again at the next [`poll`](Self::poll).
    pub fn invalidate(&mut self) {
        self.seen_hw_at = None;
    }

    /// The page was opened: the controller state goes to the log once.
    pub fn opened(&mut self) {
        self.logged_open = false;
        self.invalidate();
    }

    /// The system log lines since the last call (outcomes and requests; never a PIN or a passkey).
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }

    fn say(&mut self, line: String) {
        if self.log.len() < 200 {
            self.log.push(line);
        }
    }

    /// One heartbeat (about 30 a second): the live parts are read every time, the hardware every two seconds.
    pub fn poll(&mut self, tick: u64) {
        self.tick = tick;
        if self.seen_hw_at.is_none_or(|t| tick.saturating_sub(t) >= REFRESH_TICKS) {
            self.view = Self::view_of(btstate::read(&*self.ctl, &self.roots));
            self.seen_hw_at = Some(tick);
            if !self.logged_open {
                self.logged_open = true;
                let v = &self.view;
                let line = format!(
                    "bluetooth controller {} address {} powered {} radio {} firmware {}",
                    v.state.controller().map(|c| c.hci()).unwrap_or_else(|| "none".into()),
                    v.address.clone().unwrap_or_else(|| "none".into()),
                    if v.powered { "on" } else { "off" },
                    if v.state.hard_blocked() { "blocked-by-hardware" } else if v.state.soft_blocked() { "off" } else { "on" },
                    if v.state.needs_firmware { "needed" } else if v.state.held { "held" } else if v.state.fw_file { "present" } else { "not needed" },
                );
                self.say(line);
            }
        }
        let live = Live { devices: self.ctl.bt_devices(), pair: self.ctl.bt_pair_state(), tether: self.ctl.bt_tether_view(), audio: self.ctl.bt_audio_view() };
        self.tether_ip = if live.tether.state == TetherState::Connected { btstate::tether_address(&*self.ctl) } else { None };
        self.observe(&live);
        self.devices = live.devices.clone();
        self.pair = live.pair.clone();
        self.tether = live.tether.clone();
        self.audio = live.audio.clone();
        self.prev = Some(live);
        self.pending.retain(|_, until| *until > tick);
        self.take_scan_result();
    }

    fn take_scan_result(&mut self) {
        let done = {
            let mut g = self.scan.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(*g, ScanSlot::Done(_)) {
                std::mem::replace(&mut *g, ScanSlot::Idle)
            } else {
                return;
            }
        };
        if let ScanSlot::Done(r) = done {
            match r {
                Ok(list) => {
                    self.scan_error = None;
                    self.merge(list);
                }
                Err(e) => self.scan_error = Some(e),
            }
        }
    }

    /// The latest scan replaces the list, but a device seen with a name keeps it (and its kind) in later scans.
    fn merge(&mut self, list: Vec<Found>) {
        let mut out = Vec::new();
        for mut f in list.into_iter().take(NEARBY_KEPT) {
            let k = key(&f.address);
            if f.name.is_empty() {
                if let Some((n, kind)) = self.names.get(&k) {
                    f.name = n.clone();
                    if f.kind == Kind::Other {
                        f.kind = *kind;
                    }
                }
            }
            if !f.name.is_empty() || f.kind != Kind::Other {
                let e = self.names.entry(k).or_insert_with(|| (String::new(), Kind::Other));
                if !f.name.is_empty() {
                    e.0 = f.name.clone();
                }
                if f.kind != Kind::Other {
                    e.1 = f.kind;
                }
            }
            out.push(f);
        }
        self.found = out;
    }

    // ---------------------------------------------------------------------------------------------- outcomes

    fn set_last(&mut self, address: &str, text: String) {
        self.last.insert(key(address), text);
    }

    /// The last outcome of a device ("Tether failed: tethering is off on the phone").
    pub fn last_outcome(&self, address: &str) -> Option<&str> {
        self.last.get(&key(address)).map(|s| s.as_str())
    }

    fn observe(&mut self, new: &Live) {
        let Some(old) = self.prev.clone() else {
            return;
        };
        // pairing
        if old.pair != new.pair {
            match &new.pair {
                PairState::Done(Ok(m)) => {
                    let who = self.pair_target.as_ref().map(|f| f.address.clone()).unwrap_or_else(|| m.clone());
                    self.say(format!("bluetooth pairing {who} paired"));
                }
                PairState::Done(Err(e)) => {
                    let who = self.pair_target.as_ref().map(|f| f.address.clone()).unwrap_or_else(|| "device".into());
                    self.say(format!("bluetooth pairing {who} failed: {e}"));
                }
                _ => {}
            }
        }
        // classic and low energy input devices: the session
        for d in &new.devices {
            let before = old.devices.iter().find(|o| o.address.eq_ignore_ascii_case(&d.address)).map(|o| &o.state);
            if before == Some(&d.state) {
                continue;
            }
            let was_conn = before == Some(&LinkState::Connected);
            match &d.state {
                LinkState::Connected => {
                    self.say(format!("bluetooth hid {} session started", d.address));
                    self.set_last(&d.address, "Connected".into());
                    self.pending.remove(&format!("dev:{}", key(&d.address)));
                }
                LinkState::Failed(why) => {
                    self.say(format!("bluetooth hid {} failed: {why}", d.address));
                    self.set_last(&d.address, format!("Connection failed: {why}"));
                    self.pending.remove(&format!("dev:{}", key(&d.address)));
                }
                LinkState::Disconnected if was_conn => {
                    self.say(format!("bluetooth hid {} session stopped", d.address));
                    self.set_last(&d.address, "Disconnected".into());
                }
                _ => {}
            }
        }
        // tethering
        if old.tether.state != new.tether.state || old.tether.address != new.tether.address {
            let a = new.tether.address.clone();
            let n = self.attempts.get(&key(&a)).copied().unwrap_or(1);
            match &new.tether.state {
                TetherState::Connected => {
                    let ip = btstate::tether_address(&*self.ctl);
                    let shown = ip.clone().unwrap_or_else(|| "no address yet".into());
                    self.say(format!("bluetooth tether {a} attempt {n} connected {} {shown}", iyakki_devctl::BT_PAN_IFACE));
                    self.set_last(&a, match ip {
                        Some(ip) => format!("Tether connected {ip}"),
                        None => "Tether connected".into(),
                    });
                    self.attempts.remove(&key(&a));
                    self.pending.remove(&format!("tether:{}", key(&a)));
                }
                TetherState::Failed(why) => {
                    self.say(format!("bluetooth tether {a} attempt {n} failed: {why}"));
                    self.set_last(&a, format!("Tether failed: {why}{}", if n > 1 { format!(" (attempt {n})") } else { String::new() }));
                    self.pending.remove(&format!("tether:{}", key(&a)));
                }
                TetherState::Idle if old.tether.state == TetherState::Connected => {
                    self.say(format!("bluetooth tether {a} disconnected"));
                    self.set_last(&a, "Tether disconnected".into());
                }
                TetherState::Connecting => self.set_last(&a, format!("Tether connecting (attempt {n})")),
                TetherState::Idle => {}
            }
            if old.tether.state == TetherState::Connected && matches!(new.tether.state, TetherState::Failed(_)) {
                self.say(format!("bluetooth tether {} disconnected: link lost", old.tether.address));
            }
        }
        // audio
        match (&old.audio.device, &new.audio.device) {
            (None, Some(d)) => {
                self.say(format!("bluetooth audio {} connected {}", d.address, if d.codec.is_empty() { "codec not chosen yet" } else { &d.codec }));
                self.set_last(&d.address, if d.codec.is_empty() { "Audio connected".into() } else { format!("Audio connected · {}", d.codec) });
                self.pending.remove(&format!("audio:{}", key(&d.address)));
                if d.state == AudioState::Playing {
                    self.say(format!("bluetooth audio {} playing from {}", d.address, d.title()));
                }
            }
            (Some(o), None) => {
                self.say(format!("bluetooth audio {} disconnected", o.address));
                self.set_last(&o.address, "Audio disconnected".into());
            }
            (Some(o), Some(d)) => {
                if o.state != d.state {
                    if d.state == AudioState::Playing {
                        self.say(format!("bluetooth audio {} playing from {}", d.address, d.title()));
                        self.set_last(&d.address, format!("Playing from {}", d.title()));
                    } else {
                        self.say(format!("bluetooth audio {} stopped playing", d.address));
                        self.set_last(&d.address, "Audio connected, not playing".into());
                    }
                }
                if o.error != d.error {
                    if let Some(e) = &d.error {
                        self.say(format!("bluetooth audio {} failed: {e}", d.address));
                        self.set_last(&d.address, format!("Audio failed: {e}"));
                    }
                }
            }
            (None, None) => {}
        }
        // an audio connect that nobody answered
        let expired: Vec<String> = self.pending.iter().filter(|(k, until)| k.starts_with("audio:") && **until <= self.tick + 1 && new.audio.device.is_none()).map(|(k, _)| k.clone()).collect();
        for k in expired {
            let a = k.trim_start_matches("audio:").to_string();
            self.say(format!("bluetooth audio {a} failed: the phone did not connect for audio in time"));
            self.set_last(&a, "Audio failed: the phone did not connect in time. Start playing on the phone and try again.".into());
            self.pending.remove(&k);
        }
    }

    // ---------------------------------------------------------------------------------------------- lists

    pub fn scanning(&self) -> ScanView {
        match &*self.scan.lock().unwrap_or_else(|e| e.into_inner()) {
            ScanSlot::Running(from) => ScanView::Running(self.tick.saturating_sub(*from) / TICKS_PER_SEC),
            _ => ScanView::Idle,
        }
    }

    pub fn scan_error(&self) -> Option<&str> {
        self.scan_error.as_deref()
    }

    pub fn has_scanned(&self) -> bool {
        !self.found.is_empty() || self.scan_error.is_some()
    }

    /// The paired devices, each with its chip, its last outcome and its buttons.
    pub fn paired_rows(&self) -> Vec<PairedRow> {
        let mut rows = Vec::new();
        for d in &self.devices {
            let k = key(&d.address);
            let name = d.title();
            let icon = Icon::of_label(&d.kind);
            let last = self.last_outcome(&d.address).map(|s| s.to_string());
            if d.kind == "phone" {
                let t = &self.tether;
                let mine = t.address.eq_ignore_ascii_case(&d.address);
                let tether_state = if mine { Some(&t.state) } else { None };
                let audio = self.audio.device.as_ref().filter(|a| a.address.eq_ignore_ascii_case(&d.address));
                let connecting = matches!(tether_state, Some(TetherState::Connecting)) || self.pending.contains_key(&format!("tether:{k}")) || self.pending.contains_key(&format!("audio:{k}"));
                let (chip, tone) = if audio.is_some_and(|a| a.state == AudioState::Playing) {
                    ("Playing", Tone::Good)
                } else if matches!(tether_state, Some(TetherState::Connected)) {
                    ("Tethering", Tone::Good)
                } else if audio.is_some() {
                    ("Audio connected", Tone::Good)
                } else if connecting {
                    ("Connecting...", Tone::Good)
                } else {
                    ("Not connected", Tone::Quiet)
                };
                let tether_label = match tether_state {
                    Some(TetherState::Connected) => "Stop tether",
                    Some(TetherState::Connecting) => "Connecting...",
                    _ if self.pending.contains_key(&format!("tether:{k}")) => "Connecting...",
                    _ => "Tether",
                };
                let detail = last.unwrap_or_else(|| "Paired phone. Turn Bluetooth tethering on in the phone, then choose Tether.".into());
                rows.push(PairedRow {
                    address: d.address.clone(),
                    name,
                    icon,
                    chip,
                    tone,
                    detail,
                    buttons: vec![(RowAct::Tether, tether_label.into()), (RowAct::Audio, if audio.is_some() { "Stop audio".into() } else { "Audio".into() }), (RowAct::Forget, "Forget".into())],
                });
            } else {
                let (chip, tone) = match &d.state {
                    LinkState::Connected => ("Connected", Tone::Good),
                    LinkState::Connecting => ("Connecting...", Tone::Good),
                    LinkState::Disconnected => ("Not connected", Tone::Quiet),
                    LinkState::Failed(_) => ("Not connected", Tone::Warn),
                };
                let detail = match &d.state {
                    LinkState::Failed(why) => format!("Connection failed: {why}"),
                    _ => last.unwrap_or_else(|| format!("{} · {}", icon.word(), d.address)),
                };
                let label = if d.state == LinkState::Connected { "Disconnect" } else { "Connect" };
                rows.push(PairedRow { address: d.address.clone(), name, icon, chip, tone, detail, buttons: vec![(RowAct::Connect, label.into()), (RowAct::Forget, "Forget".into())] });
            }
        }
        rows
    }

    /// The nearby devices (not the paired ones). With an empty query and `all` false: the best [`NEARBY_SHOWN`] and a count of the rest.
    pub fn nearby(&self, query: &str, all: bool) -> Nearby {
        let paired: Vec<String> = self.devices.iter().map(|d| key(&d.address)).collect();
        let mut list: Vec<(u8, &Found)> = self.found.iter().filter(|f| !paired.contains(&key(&f.address))).map(|f| (rank(f, !f.name.is_empty()), f)).collect();
        list.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.rssi.cmp(&a.1.rssi)).then(a.1.address.cmp(&b.1.address)));
        let total = list.len();
        let q = query.trim().to_lowercase();
        let searching = !q.is_empty();
        if searching {
            list.retain(|(_, f)| f.name.to_lowercase().contains(&q) || f.address.to_lowercase().contains(&q));
        } else if !all {
            list.retain(|(r, _)| *r < 3);
            list.truncate(NEARBY_SHOWN);
        }
        let shown = list.len();
        let rows = list
            .into_iter()
            .map(|(_, f)| {
                let ok = pairable(f);
                let note = if ok {
                    String::new()
                } else if f.kind == Kind::Headphones || f.kind == Kind::Speaker {
                    "This computer cannot send sound to headphones or speakers yet.".to_string()
                } else if f.addr_type != btmgmt::ADDR_BREDR {
                    "A low-energy device that is not a keyboard, mouse or game pad.".to_string()
                } else {
                    "Only keyboards, mice, game pads and phones can be paired here.".to_string()
                };
                NearbyRow { address: f.address.clone(), name: if f.name.is_empty() { f.address.clone() } else { f.name.clone() }, named: !f.name.is_empty(), icon: Icon::of_kind(f.kind), rssi: f.rssi, signal: signal_word(f.rssi), pairable: ok, note }
            })
            .collect();
        Nearby { rows, more: if searching { 0 } else { total.saturating_sub(shown) }, total }
    }

    // ---------------------------------------------------------------------------------------------- actions

    fn busy(&self, k: &str) -> bool {
        self.pending.get(k).is_some_and(|until| *until > self.tick)
    }

    fn start(&mut self, k: String) {
        self.pending.insert(k, self.tick + PENDING_TICKS);
    }

    /// The one Bluetooth switch.
    pub fn set_switch(&mut self, on: bool) -> Result<String, String> {
        let r = if on { btstate::switch_on(&*self.ctl, &self.roots) } else { btstate::switch_off(&*self.ctl, &self.roots) };
        self.invalidate();
        match &r {
            Ok(_) => self.say(format!("bluetooth switch {}", if on { "on" } else { "off" })),
            Err(e) => self.say(format!("bluetooth switch {} failed: {e}", if on { "on" } else { "off" })),
        }
        r
    }

    /// Advanced: only the radio.
    pub fn set_radio(&mut self, on: bool) -> Result<String, String> {
        let r = btstate::set_radio(&self.roots, on);
        self.invalidate();
        if r.is_ok() {
            self.say(format!("bluetooth radio {}", if on { "on" } else { "off" }));
        }
        r
    }

    /// Advanced: only the controller's power.
    pub fn set_controller(&mut self, on: bool) -> Result<String, String> {
        let Some(c) = self.view.state.controller().filter(|c| c.configured).cloned() else { return Err("There is no controller that is set up.".into()) };
        let blocked = (self.view.state.soft_blocked(), self.view.state.hard_blocked());
        let r = self.ctl.bt_power(c.index, on).map_err(|e| btstate::power_failure(&e, on, blocked));
        self.invalidate();
        r.map(|_| format!("The controller is powered {}.", if on { "on" } else { "off" }))
    }

    pub fn load_driver(&mut self) -> Result<String, String> {
        let r = self.ctl.bt_load_anyway();
        self.invalidate();
        if r.is_ok() {
            self.say("bluetooth load anyway".into());
        }
        r
    }

    /// Starts a scan that ends by itself (or with [`stop_scan`](Self::stop_scan)).
    pub fn start_scan(&mut self) -> Result<String, String> {
        if self.view.switch != Switch::On {
            return Err("Turn Bluetooth on first.".into());
        }
        let Some(index) = self.view.state.controller().map(|c| c.index) else { return Err("There is no controller to scan with.".into()) };
        {
            let mut g = self.scan.lock().unwrap_or_else(|e| e.into_inner());
            if matches!(*g, ScanSlot::Running(_)) {
                return Err("A scan is running already.".into());
            }
            *g = ScanSlot::Running(self.tick);
        }
        self.scan_error = None;
        self.stop.store(false, Ordering::Relaxed);
        let (ctl, slot, stop) = (self.ctl.clone(), self.scan.clone(), self.stop.clone());
        std::thread::spawn(move || {
            let r = ctl.bt_scan_until(index, SCAN_SECS, &stop);
            *slot.lock().unwrap_or_else(|e| e.into_inner()) = ScanSlot::Done(r);
        });
        self.say(format!("bluetooth hci{index} scan"));
        Ok("Scanning for devices ...".into())
    }

    pub fn stop_scan(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }

    /// Asks the device to pair. The popup follows [`pair`](Self::pair) state.
    pub fn start_pair(&mut self, address: &str) -> Result<String, String> {
        let Some(f) = self.found.iter().find(|f| f.address.eq_ignore_ascii_case(address)).cloned() else { return Err("Scan again: that device is no longer in the list.".into()) };
        self.stop_scan();
        match self.ctl.bt_pair(&f) {
            Ok(()) => {
                self.say(format!("bluetooth pair {}", f.address));
                let name = if f.name.is_empty() { f.address.clone() } else { f.name.clone() };
                self.pair_target = Some(f);
                Ok(format!("Pairing with {name} ..."))
            }
            Err(e) => Err(e),
        }
    }

    /// Answers the pairing question. The answer itself is logged, the number never.
    pub fn pair_answer(&mut self, yes: bool) -> Result<String, String> {
        if self.ctl.bt_pair_answer(yes) {
            self.say(format!("bluetooth pairing answer {}", if yes { "yes" } else { "no" }));
            Ok(if yes { "Confirmed.".into() } else { "Pairing cancelled.".into() })
        } else {
            Err("Nothing is waiting for an answer.".into())
        }
    }

    /// Hands the typed passkey or PIN to the pairing (`None`: the person gave up). A wrong length is refused here with a plain message and the
    /// prompt stays open. What was typed is never logged, only that something was sent.
    pub fn pair_type(&mut self, text: Option<String>) -> Result<String, String> {
        match &text {
            Some(t) => match &self.pair {
                PairState::TypePasskey(_) => {
                    iyakki_devctl::bthid::check_typed_passkey(t)?;
                }
                PairState::TypePin(..) => {
                    iyakki_devctl::bthid::check_typed_pin(t)?;
                }
                _ => return Err("Nothing is waiting for a passkey or PIN.".into()),
            },
            None => {}
        }
        let sent = text.is_some();
        match self.ctl.bt_pair_type(text) {
            Ok(true) => {
                self.say(format!("bluetooth pairing entry {}", if sent { "sent" } else { "cancelled" }));
                Ok(if sent { "Sent.".into() } else { "Pairing cancelled.".into() })
            }
            Ok(false) => Err("Nothing is waiting for a passkey or PIN.".into()),
            Err(e) => Err(e),
        }
    }

    /// Connect or disconnect a paired input device; a second click while it connects is ignored.
    pub fn connect_toggle(&mut self, address: &str) -> Result<String, String> {
        let k = format!("dev:{}", key(address));
        let state = self.devices.iter().find(|d| d.address.eq_ignore_ascii_case(address)).map(|d| d.state.clone());
        if state == Some(LinkState::Connecting) || self.busy(&k) {
            return Ok("Still connecting. Wait for the result.".into());
        }
        let connected = state == Some(LinkState::Connected);
        let r = if connected { self.ctl.bt_device_disconnect(address) } else { self.ctl.bt_device_connect(address) };
        r?;
        self.say(format!("bluetooth {address} {}", if connected { "disconnect" } else { "connect" }));
        if !connected {
            self.start(k);
            self.set_last(address, "Connecting ...".into());
        }
        Ok(if connected { "Disconnected.".into() } else { "Connecting ...".into() })
    }

    /// Connect or disconnect the phone's tethering; one attempt at a time.
    pub fn tether_toggle(&mut self, address: &str) -> Result<String, String> {
        let k = format!("tether:{}", key(address));
        let mine = self.tether.address.eq_ignore_ascii_case(address);
        if mine && self.tether.state == TetherState::Connecting || self.busy(&k) {
            return Ok("Still connecting to the phone's internet. Wait for the result.".into());
        }
        if mine && self.tether.state == TetherState::Connected {
            self.ctl.bt_tether_disconnect()?;
            self.say(format!("bluetooth tether {address} disconnect"));
            self.attempts.remove(&key(address));
            return Ok("Disconnecting the phone's internet.".into());
        }
        self.ctl.bt_tether_connect(address)?;
        let n = self.attempts.entry(key(address)).and_modify(|n| *n += 1).or_insert(1);
        let n = *n;
        self.say(format!("bluetooth tether {address} connect attempt {n}"));
        self.start(k);
        self.set_last(address, format!("Tether connecting (attempt {n})"));
        Ok("Connecting to the phone's internet ... Bluetooth tethering must be on in the phone.".into())
    }

    /// Connect the phone for audio, or stop its audio.
    pub fn audio_toggle(&mut self, address: &str) -> Result<String, String> {
        let k = format!("audio:{}", key(address));
        if self.busy(&k) {
            return Ok("Still waiting for the phone. Start playing on it.".into());
        }
        if self.audio.device.as_ref().is_some_and(|d| d.address.eq_ignore_ascii_case(address)) {
            self.ctl.bt_audio_disconnect()?;
            self.say(format!("bluetooth audio {address} disconnect"));
            return Ok("Disconnecting the audio.".into());
        }
        self.ctl.bt_audio_connect(address)?;
        self.say(format!("bluetooth audio {address} connect"));
        self.start(k);
        self.set_last(address, "Audio connecting ... start playing on the phone".into());
        Ok("Connecting for audio ... start playing on the phone.".into())
    }

    /// The audio card: lets a phone pair for two minutes, closes that, or disconnects the phone that is connected.
    pub fn audio_pairing_toggle(&mut self) -> Result<String, String> {
        if let Some(d) = self.audio.device.clone() {
            self.ctl.bt_audio_disconnect()?;
            self.say(format!("bluetooth audio {} disconnect", d.address));
            return Ok("Disconnecting the phone's audio.".into());
        }
        let on = !self.audio.pairing_open;
        self.ctl.bt_audio_pairing(on)?;
        self.say(format!("bluetooth audio pairing {}", if on { "open" } else { "closed" }));
        Ok(if on { "This computer is visible to phones for two minutes. On the phone, open Bluetooth and choose this computer.".into() } else { "Pairing for phones is closed.".into() })
    }

    pub fn forget(&mut self, address: &str) -> Result<String, String> {
        self.ctl.bt_device_forget(address)?;
        self.say(format!("bluetooth {address} forget"));
        self.last.remove(&key(address));
        self.pending.retain(|k, _| !k.ends_with(&key(address)));
        Ok("Forgotten. Pair it again to use it.".into())
    }

    /// The name of a paired device, or the address.
    pub fn paired_name(&self, address: &str) -> Option<String> {
        self.devices.iter().find(|d| d.address.eq_ignore_ascii_case(address)).map(|d| d.title())
    }

    /// The paired device with this name (a device that paired with this computer by itself).
    pub fn paired_by_name(&self, name: &str) -> Option<&DeviceView> {
        self.devices.iter().find(|d| d.name == name)
    }

    /// The tick the model last saw.
    pub fn tick(&self) -> u64 {
        self.tick
    }
}

/// The text the shell shows when a pairing needs the person while Settings is not open (an incoming request), if any.
pub fn pairing_notice(ctl: &dyn Control) -> Option<String> {
    match ctl.bt_pair_state() {
        PairState::Confirm(n, _) | PairState::ConfirmNoNumber(n) => Some(format!("{n} wants to pair. Open Settings, Bluetooth to answer.")),
        PairState::ShowPasskey(n, _) | PairState::ShowPin(n, _) => Some(format!("{n} is pairing: Settings, Bluetooth shows what to type.")),
        PairState::TypePasskey(n) | PairState::TypePin(n, _) => Some(format!("{n} is pairing: Settings, Bluetooth asks for the number it shows.")),
        _ => None,
    }
}

static LAST_NOTICE: Mutex<String> = Mutex::new(String::new());

/// [`pairing_notice`] once per request (the shell calls it every second or so and must not repeat itself).
pub fn next_pairing_notice(ctl: &dyn Control) -> Option<String> {
    let now = pairing_notice(ctl);
    let mut last = LAST_NOTICE.lock().unwrap_or_else(|e| e.into_inner());
    match now {
        Some(n) if *last != n => {
            *last = n.clone();
            Some(n)
        }
        Some(_) => None,
        None => {
            last.clear();
            None
        }
    }
}
