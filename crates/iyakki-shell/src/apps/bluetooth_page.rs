//! Settings -> Bluetooth: the page.
//!
//! One switch with its state in words, the paired devices first (each row with its own buttons), a Scan row that opens a popup (like
//! the Wi-Fi scan: the same modal frame) with the short list of nearby devices, a popup for pairing, and an Advanced part with the
//! separate radio and controller switches. The page draws and takes
//! keys; everything about the machine is in [`crate::btmodel`] (a fake `Control` drives it in tests). The pairing popup follows the
//! pairing state of the machine, so a pairing the other device starts opens it too.
//!
//! Keys: Up and Down choose a row, Enter does its main action, `s` opens the scan popup, `e` the Bluetooth switch, `u` forgets the
//! chosen paired device, `t` the first phone's tethering, `a` Advanced, `r` looks again.
//!
//! The scan popup: Up and Down choose a device (the last stop is the Scan again button), Enter pairs the device (or scans again),
//! `s` scans again once the scan has finished, `m` shows more devices, `/` searches, Esc or Close stops the scan and closes it.

use crate::access::{self, Meta, Role, State};
use crate::btmodel::{BtModel, Fix, Icon, NearbyRow, PairState, PairedRow, RowAct, ScanView, Switch, Tone, NEARBY_SHOWN, TICKS_PER_SEC};
use crate::fx;
use crate::gfx::{rgb, Face, Gfx};
use crate::ui::{Event, Key, TextField, ACCENT_MAIN as ACCENT, DIM, MUTED, OK, R, TEXT};

use super::devices::DeviceEnv;

const WARN: [u8; 4] = rgb(255, 196, 94);
const SEARCH_ID: usize = 901;
/// Sub codes of the page's own buttons (the paired and nearby rows use their position in the focus list).
const SUB_SWITCH: u8 = 250;
const SUB_SCAN: u8 = 251;
const SUB_ADVANCED: u8 = 252;
const SUB_MORE: u8 = 253;
const SUB_FIRMWARE: u8 = 254;
const SUB_DRIVER: u8 = 255;
const SUB_ADV: u8 = 249;
const SUB_AGAIN: u8 = 247;
const SUB_AUDIO: u8 = 248;
const MAX_ROW_CODE: usize = 240;
/// A status line stays this many ticks.
const STATUS_TICKS: u64 = 8 * TICKS_PER_SEC;

/// What a row of the page stands for (what Up and Down choose).
#[derive(Clone, Debug, PartialEq, Eq)]
enum Focus {
    Switch,
    Paired(String),
    Scan,
    Advanced,
}

/// The scan popup. The list itself is the model's nearby list; this holds what the person did with it.
struct Popup {
    /// The chosen stop: a device row, or (one past the last row) the Scan again button.
    sel: usize,
    /// The first device row drawn, and how many rows fit (set when drawn).
    first: usize,
    fit: usize,
    /// A plain message under the list (a device that cannot be paired here, a scan that could not start).
    msg: String,
    /// The last thing announced to the screen reader.
    said: String,
}

/// The pairing popup.
struct Dialog {
    name: String,
    /// The person started it (an incoming request defaults to No).
    ours: bool,
    /// The last thing announced to the screen reader.
    said: String,
    /// What the person typed for a passkey or PIN the other device shows. Wiped when sent, cancelled or closed (also when dropped), never logged.
    entry: String,
    /// A plain message under the entry (a wrong length), spoken when it appears.
    msg: String,
}

impl Dialog {
    fn new(name: String, ours: bool) -> Dialog {
        Dialog { name, ours, said: String::new(), entry: String::new(), msg: String::new() }
    }

    /// Overwrites and empties the typed number.
    fn wipe(&mut self) {
        let mut b = std::mem::take(&mut self.entry).into_bytes();
        b.fill(0);
    }
}

impl Drop for Dialog {
    fn drop(&mut self) {
        self.wipe();
    }
}

/// Digits of a passkey, characters of a PIN.
const PASSKEY_LEN: usize = 6;
const PIN_MAX: usize = 16;

enum Block {
    Switch,
    Heading(&'static str),
    NoPaired,
    Paired(usize),
    AudioCard,
    Scan,
    AdvancedToggle,
    Advanced,
}

pub struct BtPage {
    pub model: BtModel,
    sel: usize,
    first: usize,
    advanced: bool,
    all: bool,
    search: TextField,
    search_focus: bool,
    popup: Option<Popup>,
    /// The scan was closed by the person: its end is not announced on the page.
    quiet_end: bool,
    dialog: Option<Dialog>,
    forget: Option<(String, String)>,
    /// A cancelled pairing: a question that arrives later is answered No by itself.
    suppress: bool,
    status: Option<(String, bool, u64)>,
    ticks: u64,
    request: Option<(String, String)>,
    opened: bool,
    was_scanning: bool,
}

impl BtPage {
    pub fn new(env: DeviceEnv) -> BtPage {
        BtPage {
            model: BtModel::new(env.ctl, env.roots),
            sel: 0,
            first: 0,
            advanced: false,
            all: false,
            search: TextField::new("Search nearby devices", false),
            search_focus: false,
            popup: None,
            quiet_end: false,
            dialog: None,
            forget: None,
            suppress: false,
            status: None,
            ticks: 0,
            request: None,
            opened: false,
            was_scanning: false,
        }
    }

    pub fn replace_machine(&mut self, env: DeviceEnv) {
        *self = BtPage::new(env);
    }

    pub fn status(&self) -> Option<(&str, bool)> {
        self.status.as_ref().map(|s| (s.0.as_str(), s.1))
    }

    pub fn has_dialog(&self) -> bool {
        self.dialog.is_some() || self.forget.is_some() || self.popup.is_some()
    }

    pub fn take_request(&mut self) -> Option<(String, String)> {
        self.request.take()
    }

    pub fn search_text(&self) -> &str {
        &self.search.value
    }

    /// The plain message under the scan list (empty when there is none).
    pub fn popup_message(&self) -> &str {
        self.popup.as_ref().map(|p| p.msg.as_str()).unwrap_or("")
    }

    /// The Scan row of the page is the chosen one.
    #[cfg(test)]
    pub fn scan_row_chosen(&self) -> bool {
        self.focus_list().get(self.sel) == Some(&Focus::Scan)
    }

    pub fn scan_popup_open(&self) -> bool {
        self.popup.is_some()
    }

    pub fn pairing_name(&self) -> Option<&str> {
        self.dialog.as_ref().map(|d| d.name.as_str())
    }

    /// The page is shown (the tab was chosen): the controller state goes to the log.
    pub fn open(&mut self) {
        if !self.opened {
            self.opened = true;
            self.model.opened();
        }
    }

    pub fn closed(&mut self) {
        self.opened = false;
    }

    fn ok(&mut self, m: impl Into<String>) {
        let m = m.into();
        access::announce(&m);
        self.status = Some((m, true, self.ticks + STATUS_TICKS));
    }

    fn fail(&mut self, m: impl Into<String>) {
        let m = m.into();
        access::announce(&m);
        self.status = Some((m, false, self.ticks + STATUS_TICKS));
    }

    fn run(&mut self, r: Result<String, String>) {
        match r {
            Ok(m) => self.ok(m),
            Err(e) => self.fail(e),
        }
        self.model.poll(self.ticks);
    }

    // ---------------------------------------------------------------------------------------------- heartbeat

    /// One tick: the model reads the machine; the popup follows the pairing; the log lines come back for the system log.
    pub fn tick(&mut self, ticks: u64) -> Vec<String> {
        self.ticks = ticks;
        self.open();
        self.model.poll(ticks);
        if self.status.as_ref().is_some_and(|s| s.2 <= ticks) {
            self.status = None;
        }
        let scanning = matches!(self.model.scanning(), ScanView::Running(_));
        if self.was_scanning && !scanning {
            let n = self.model.nearby("", true).total;
            let (words, good) = match self.model.scan_error() {
                Some(e) => (e.to_string(), false),
                None => (
                    match n {
                        0 => "The scan found no other device.".to_string(),
                        1 => "The scan found 1 device.".to_string(),
                        n => format!("The scan found {n} devices."),
                    },
                    true,
                ),
            };
            if std::mem::take(&mut self.quiet_end) {
                // closed by the person: nothing to say
            } else if self.popup.is_some() {
                // the popup shows it (and says it to the screen reader)
            } else if good {
                self.ok(words)
            } else {
                self.fail(words)
            }
        }
        self.was_scanning = scanning;
        self.follow_pairing();
        self.follow_popup();
        self.model.take_log()
    }

    fn follow_pairing(&mut self) {
        let state = self.model.pair.clone();
        let active = matches!(state, PairState::Running(_) | PairState::Confirm(..) | PairState::ConfirmNoNumber(_) | PairState::ShowPasskey(..) | PairState::ShowPin(..) | PairState::TypePasskey(_) | PairState::TypePin(..));
        if !active && !matches!(state, PairState::Done(_)) {
            self.suppress = false;
        }
        if self.suppress {
            if matches!(state, PairState::Confirm(..) | PairState::ConfirmNoNumber(_)) {
                let _ = self.model.pair_answer(false);
            }
            if !active {
                self.suppress = false;
            }
            return;
        }
        if self.dialog.is_none() && active {
            let name = match &state {
                PairState::Running(n) | PairState::Confirm(n, _) | PairState::ConfirmNoNumber(n) | PairState::ShowPasskey(n, _) | PairState::ShowPin(n, _) | PairState::TypePasskey(n) | PairState::TypePin(n, _) => n.clone(),
                _ => String::new(),
            };
            self.model.stop_scan();
            self.close_popup();
            self.dialog = Some(Dialog::new(name, self.model.pair_target.is_some() && matches!(state, PairState::Running(_))));
        }
        if let Some(d) = &self.dialog {
            let said = dialog_words(state_name(&state).unwrap_or(&d.name), &state);
            if said != d.said {
                access::announce(&said);
                if let Some(d) = self.dialog.as_mut() {
                    d.said = said;
                }
            }
        }
    }

    // ---------------------------------------------------------------------------------------------- rows to choose

    fn focus_list(&self) -> Vec<Focus> {
        let mut v = vec![Focus::Switch];
        v.extend(self.model.paired_rows().into_iter().map(|r| Focus::Paired(r.address)));
        if self.model.view.switch == Switch::On || matches!(self.model.scanning(), ScanView::Running(_)) {
            v.push(Focus::Scan);
        }
        v.push(Focus::Advanced);
        v
    }

    fn clamp_sel(&mut self) -> Vec<Focus> {
        let l = self.focus_list();
        self.sel = self.sel.min(l.len() - 1);
        l
    }

    fn act(&mut self, a: RowAct, address: &str) {
        match a {
            RowAct::Connect => {
                let r = self.model.connect_toggle(address);
                self.run(r)
            }
            RowAct::Tether => {
                let r = self.model.tether_toggle(address);
                self.run(r)
            }
            RowAct::Audio => {
                let r = self.model.audio_toggle(address);
                self.run(r)
            }
            RowAct::Forget => {
                let name = self.model.paired_name(address).unwrap_or_else(|| address.to_string());
                self.forget = Some((address.to_string(), name));
            }
        }
    }

    fn main_action(&mut self, f: &Focus) {
        match f {
            Focus::Switch => self.toggle_switch(),
            Focus::Paired(a) => {
                if let Some(row) = self.model.paired_rows().into_iter().find(|r| r.address == *a) {
                    if let Some((act, _)) = row.buttons.first() {
                        self.act(*act, a);
                    }
                }
            }
            Focus::Scan => self.open_scan(),
            Focus::Advanced => self.advanced = !self.advanced,
        }
    }

    fn pair(&mut self, address: &str) {
        if let Err(e) = self.try_pair(address) {
            self.fail(e);
        }
    }

    /// Asks the device to pair and opens the pairing popup (the scan popup, if it is open, goes away); the reason when it cannot.
    fn try_pair(&mut self, address: &str) -> Result<(), String> {
        let Some(row) = self.model.nearby("", true).rows.into_iter().find(|r| r.address.eq_ignore_ascii_case(address)) else { return Err("Scan again: that device is no longer in the list.".into()) };
        if !row.pairable {
            return Err(row.note);
        }
        self.model.start_pair(address)?;
        self.model.poll(self.ticks);
        self.suppress = false;
        self.close_popup();
        self.dialog = Some(Dialog::new(row.name, true));
        self.follow_pairing();
        Ok(())
    }

    fn toggle_switch(&mut self) {
        let sw = self.model.view.switch;
        if !sw.can_toggle() {
            let why = sw.why();
            return self.fail(format!("{}. {why}", sw.words().trim_end_matches(": Install")));
        }
        let r = self.model.set_switch(sw != Switch::On);
        self.run(r);
    }

    /// Scan: the popup opens at once and the scan starts (a scan that is still running is shown, not started twice).
    fn open_scan(&mut self) {
        if !matches!(self.model.scanning(), ScanView::Running(_)) {
            if let Err(e) = self.model.start_scan() {
                return self.fail(e);
            }
            self.model.clear_nearby();
        }
        self.search.clear();
        self.search_focus = false;
        self.all = false;
        self.quiet_end = false;
        self.popup = Some(Popup { sel: 0, first: 0, fit: 1, msg: String::new(), said: String::new() });
        self.follow_popup();
    }

    /// Closes the scan popup and stops its scan.
    fn close_popup(&mut self) {
        if self.popup.take().is_some() {
            if matches!(self.model.scanning(), ScanView::Running(_)) {
                self.model.stop_scan();
                self.quiet_end = true;
            }
            self.search.clear();
            self.search_focus = false;
            self.all = false;
        }
    }

    /// Scan again in the same popup: the old list and the old message go, a new scan starts.
    fn scan_again(&mut self) {
        if matches!(self.model.scanning(), ScanView::Running(_)) {
            return;
        }
        match self.model.start_scan() {
            Ok(_) => {
                self.model.clear_nearby();
                self.search.clear();
                self.search_focus = false;
                self.all = false;
                if let Some(p) = self.popup.as_mut() {
                    p.sel = 0;
                    p.first = 0;
                    p.msg.clear();
                }
            }
            Err(e) => self.popup_msg(e),
        }
    }

    fn popup_msg(&mut self, m: String) {
        access::announce(&m);
        if let Some(p) = self.popup.as_mut() {
            p.msg = m;
        }
    }

    /// What the screen reader hears when the scan popup changes: scanning, finished with a count, or the problem.
    fn follow_popup(&mut self) {
        if self.popup.is_none() {
            return;
        }
        let words = match (self.model.scanning(), self.model.scan_error()) {
            (ScanView::Running(_), _) => "Scan for devices. Scanning.".to_string(),
            (_, Some(e)) => format!("Scan for devices. {e} Try again is the first button."),
            _ => match self.model.nearby("", true).total {
                0 => "Scan for devices. The scan found no other device. Scan again, or Close.".to_string(),
                1 => "Scan for devices. The scan found 1 device. Up and Down choose, Enter pairs.".to_string(),
                n => format!("Scan for devices. The scan found {n} devices. Up and Down choose, Enter pairs."),
            },
        };
        let stops = self.popup_stops();
        if let Some(p) = self.popup.as_mut() {
            p.sel = p.sel.min(stops.saturating_sub(1));
            if p.said != words {
                access::announce(&words);
                p.said = words;
            }
        }
    }

    /// How many stops Up and Down have in the popup: the device rows, and the Scan again button once the scan has finished.
    fn popup_stops(&self) -> usize {
        let rows = self.model.nearby(&self.search.value, self.all).rows.len();
        if matches!(self.model.scanning(), ScanView::Running(_)) {
            rows.max(1)
        } else {
            rows + 1
        }
    }

    fn firmware(&mut self) {
        self.request = Some(("quickinstall".into(), iyakki_agent::quickinstall::Item::MacFirmware.id().into()));
        self.ok("Opening Store, Setup, Hardware: Mac Wi-Fi and Bluetooth firmware.");
    }

    fn driver(&mut self) {
        let r = self.model.load_driver();
        self.run(r);
    }

    // ---------------------------------------------------------------------------------------------- events

    /// A key or click. `true`: the page took it.
    pub fn handle(&mut self, ev: &Event) -> bool {
        if let Event::MouseMove(..) = ev {
            return false;
        }
        if self.forget.is_some() {
            return self.forget_event(ev);
        }
        if self.dialog.is_some() {
            return self.dialog_event(ev);
        }
        if self.popup.is_some() {
            return self.popup_event(ev);
        }
        match ev {
            Event::Sub(i, k) => {
                self.sub(*i, k);
                true
            }
            Event::Scroll(n) => {
                let max = 60usize;
                self.first = (self.first as i32 - n.signum()).clamp(0, max as i32) as usize;
                true
            }
            Event::Key(k) => self.key(k),
            _ => false,
        }
    }

    fn after_search(&mut self) {
        if let Some(p) = self.popup.as_mut() {
            p.first = 0;
            p.sel = 0;
        }
    }

    /// A key or click while the scan popup is open: it takes everything (the page behind is out of reach).
    fn popup_event(&mut self, ev: &Event) -> bool {
        match ev {
            Event::Sub(SUB_SCAN, _) => {
                self.model.stop_scan();
                access::announce("Stopping the scan.");
            }
            Event::Sub(SUB_AGAIN, _) => self.scan_again(),
            Event::Sub(SUB_MORE, _) => self.popup_more(),
            Event::Sub(i, _) if (*i as usize) < MAX_ROW_CODE => {
                if let Some(row) = self.model.nearby(&self.search.value, self.all).rows.get(*i as usize) {
                    let a = row.address.clone();
                    self.popup_pair(&a);
                }
            }
            Event::FocusAt(SEARCH_ID, x) => {
                self.search_focus = true;
                self.search.click(*x);
            }
            Event::Paste(t) if self.search_focus => {
                self.search.paste(t);
                self.after_search();
            }
            Event::Scroll(n) => {
                let max = self.model.nearby(&self.search.value, self.all).rows.len().saturating_sub(1);
                if let Some(p) = self.popup.as_mut() {
                    p.first = (p.first as i32 - n.signum()).clamp(0, max as i32) as usize;
                    // the chosen row follows the window (it would pull the list back otherwise)
                    if p.sel <= max {
                        p.sel = p.sel.clamp(p.first, p.first + p.fit.max(1) - 1).min(max);
                    }
                }
            }
            Event::Key(k) => self.popup_key(k),
            _ => {}
        }
        true
    }

    fn popup_more(&mut self) {
        self.all = !self.all;
        if let Some(p) = self.popup.as_mut() {
            p.first = 0;
            p.sel = 0;
        }
    }

    fn popup_pair(&mut self, address: &str) {
        if let Err(e) = self.try_pair(address) {
            self.popup_msg(e);
        }
    }

    fn popup_key(&mut self, k: &Key) {
        if self.search_focus {
            match k {
                Key::Esc => {
                    self.search.clear();
                    self.search_focus = false;
                    self.after_search();
                }
                Key::Enter | Key::Down => self.search_focus = false,
                other => {
                    self.search.handle(other);
                    self.after_search();
                }
            }
            return;
        }
        let rows = self.model.nearby(&self.search.value, self.all).rows;
        let stops = self.popup_stops();
        let Some(sel) = self.popup.as_ref().map(|p| p.sel.min(stops - 1)) else { return };
        let set = |s: &mut Self, v: usize| {
            if let Some(p) = s.popup.as_mut() {
                p.sel = v;
                p.msg.clear();
            }
        };
        match k {
            Key::Esc => self.close_popup(),
            Key::Up => set(self, sel.saturating_sub(1)),
            Key::Down => set(self, (sel + 1).min(stops - 1)),
            Key::Enter | Key::Char(' ') => match rows.get(sel) {
                Some(r) => self.popup_pair(&r.address),
                None => self.scan_again(),
            },
            Key::Char('s') | Key::Char('r') => self.scan_again(),
            Key::Char('m') => self.popup_more(),
            Key::Char('/') => {
                self.search_focus = true;
                self.all = true;
            }
            _ => {}
        }
    }

    fn key(&mut self, k: &Key) -> bool {
        let list = self.clamp_sel();
        match k {
            Key::Up => self.sel = self.sel.saturating_sub(1),
            Key::Down => self.sel = (self.sel + 1).min(list.len() - 1),
            Key::Enter | Key::Char(' ') => {
                let f = list[self.sel].clone();
                self.main_action(&f);
            }
            Key::Char('e') | Key::Char('p') => self.toggle_switch(),
            Key::Char('s') => self.open_scan(),
            Key::Char('a') => self.advanced = !self.advanced,
            Key::Char('f') if self.model.view.fixes().contains(&Fix::Firmware) => self.firmware(),
            Key::Char('f') => self.fail("Bluetooth does not need firmware on this machine."),
            Key::Char('l') if self.model.view.state.held => self.driver(),
            Key::Char('l') => self.fail("No Bluetooth driver is set aside on this machine: nothing to load."),
            Key::Char('r') => {
                self.model.invalidate();
                self.model.poll(self.ticks);
                self.ok("Looked at the Bluetooth hardware again.");
            }
            Key::Char('t') => match self.model.paired_rows().into_iter().find(|r| r.buttons.iter().any(|b| b.0 == RowAct::Tether)) {
                Some(r) => self.act(RowAct::Tether, &r.address),
                None => self.fail("No phone is paired yet. Scan, then choose Pair next to your phone in the scan list (Bluetooth must be on in the phone); then Tether connects its internet."),
            },
            Key::Char('u') => match &list[self.sel] {
                Focus::Paired(a) => {
                    let a = a.clone();
                    self.act(RowAct::Forget, &a)
                }
                _ => self.fail("Choose a paired device first (Up and Down), then u forgets it."),
            },
            _ => return false,
        }
        true
    }

    fn sub(&mut self, i: u8, k: &Key) {
        match (i, k) {
            (SUB_SWITCH, _) => self.toggle_switch(),
            (SUB_SCAN, _) => self.open_scan(),
            (SUB_ADVANCED, _) => self.advanced = !self.advanced,
            (SUB_FIRMWARE, _) => self.firmware(),
            (SUB_DRIVER, _) => self.driver(),
            (SUB_AUDIO, _) => {
                let r = self.model.audio_pairing_toggle();
                self.run(r)
            }
            (SUB_ADV, Key::Char('r')) => {
                let on = !self.model.view.radio_on;
                let r = self.model.set_radio(on);
                self.run(r)
            }
            (SUB_ADV, Key::Char('c')) => {
                let on = !self.model.view.powered;
                let r = self.model.set_controller(on);
                self.run(r)
            }
            (SUB_ADV, _) => {
                self.model.invalidate();
                self.model.poll(self.ticks);
                self.ok("Looked at the Bluetooth hardware again.");
            }
            (i, k) => {
                let list = self.focus_list();
                let Some(f) = list.get(i as usize).cloned() else { return };
                self.sel = i as usize;
                match (k, &f) {
                    (Key::Char(c), Focus::Paired(a)) => {
                        let act = match c {
                            'c' => Some(RowAct::Connect),
                            't' => Some(RowAct::Tether),
                            'a' => Some(RowAct::Audio),
                            'f' => Some(RowAct::Forget),
                            _ => None,
                        };
                        if let Some(act) = act {
                            self.act(act, a);
                        }
                    }
                    _ => self.main_action(&f),
                }
            }
        }
    }

    fn forget_event(&mut self, ev: &Event) -> bool {
        match ev {
            Event::Key(Key::Char('y')) => {
                if let Some((a, _)) = self.forget.take() {
                    let r = self.model.forget(&a);
                    self.run(r);
                }
            }
            Event::Key(Key::Esc) | Event::Key(Key::Char('n')) | Event::Key(Key::Char(_)) => self.forget = None,
            _ => {}
        }
        true
    }

    fn dialog_event(&mut self, ev: &Event) -> bool {
        let state = self.model.pair.clone();
        if matches!(state, PairState::TypePasskey(_) | PairState::TypePin(..)) {
            return self.typing_event(ev, matches!(state, PairState::TypePasskey(_)));
        }
        let Event::Key(k) = ev else { return true };
        let ours = self.dialog.as_ref().is_some_and(|d| d.ours);
        // the answer for Enter: Yes when the person started this pairing, No for a request the other device made
        let enter_yes = ours;
        let yes = matches!(k, Key::Char('y')) || (matches!(k, Key::Enter) && enter_yes);
        let no = matches!(k, Key::Char('n') | Key::Esc) || (matches!(k, Key::Enter) && !enter_yes);
        match state {
            PairState::Confirm(..) | PairState::ConfirmNoNumber(_) => {
                if yes || no {
                    let r = self.model.pair_answer(yes);
                    if let Err(e) = r {
                        self.fail(e);
                    }
                    if no {
                        self.dialog = None;
                        self.suppress = true;
                        self.ok("Pairing cancelled.");
                    }
                }
            }
            PairState::Done(Ok(_)) => {
                if matches!(k, Key::Enter | Key::Char('y')) {
                    self.connect_after_pairing();
                    self.dialog = None;
                } else if matches!(k, Key::Esc | Key::Char('n')) {
                    self.dialog = None;
                }
            }
            PairState::Done(Err(_)) => {
                if matches!(k, Key::Enter | Key::Char('y')) {
                    match self.model.pair_target.clone() {
                        Some(f) => {
                            self.dialog = None;
                            self.pair(&f.address);
                        }
                        None => self.dialog = None,
                    }
                } else if matches!(k, Key::Esc | Key::Char('n')) {
                    self.dialog = None;
                }
            }
            PairState::Idle => self.dialog = None,
            // connecting, or the person must type something on the other device: Esc cancels
            _ => {
                if matches!(k, Key::Esc | Key::Char('n')) {
                    self.dialog = None;
                    self.suppress = true;
                    self.ok("Pairing cancelled.");
                }
            }
        }
        true
    }

    /// The popup asks for the passkey or PIN the other device shows: digits (or characters) go into the entry, Enter sends it, Esc gives up.
    fn typing_event(&mut self, ev: &Event, passkey: bool) -> bool {
        let max = if passkey { PASSKEY_LEN } else { PIN_MAX };
        let ok_char = |c: char| if passkey { c.is_ascii_digit() } else { !c.is_control() && c.is_ascii() };
        let Some(d) = self.dialog.as_mut() else { return true };
        match ev {
            Event::Key(Key::Esc) => {
                d.wipe();
                let r = self.model.pair_type(None);
                self.dialog = None;
                self.suppress = true;
                match r {
                    Ok(_) => self.ok("Pairing cancelled."),
                    Err(e) => self.fail(e),
                }
            }
            Event::Key(Key::Enter) => {
                let text = std::mem::take(&mut d.entry);
                match self.model.pair_type(Some(text.clone())) {
                    Ok(_) => {
                        let mut t = text.into_bytes();
                        t.fill(0);
                        if let Some(d) = self.dialog.as_mut() {
                            d.msg.clear();
                        }
                    }
                    Err(e) => {
                        // refused: the typing stays, the reason is said in the popup
                        if let Some(d) = self.dialog.as_mut() {
                            d.entry = text;
                            access::announce(&e);
                            d.msg = e;
                        }
                    }
                }
            }
            Event::Key(Key::Backspace) => {
                d.entry.pop();
                d.msg.clear();
            }
            Event::Key(Key::Char(c)) => {
                if ok_char(*c) && d.entry.chars().count() < max {
                    d.entry.push(*c);
                    d.msg.clear();
                } else if passkey && !c.is_ascii_digit() {
                    d.msg = "A passkey has digits only.".into();
                }
            }
            Event::Paste(t) => {
                // digits only for a passkey (spaces and dashes in the pasted number are dropped); the rest is cut at the limit
                let room = max.saturating_sub(d.entry.chars().count());
                let add: String = t.chars().filter(|c| ok_char(*c)).take(room).collect();
                if add.is_empty() && !t.is_empty() && passkey {
                    d.msg = "A passkey has digits only.".into();
                } else {
                    d.entry.push_str(&add);
                    d.msg.clear();
                }
            }
            _ => {}
        }
        true
    }

    /// "Paired. Connect now?" Yes: a keyboard, mouse or game pad connects, a phone starts tethering.
    fn connect_after_pairing(&mut self) {
        self.model.poll(self.ticks);
        let name = self.dialog.as_ref().map(|d| d.name.clone()).unwrap_or_default();
        let addr = self.model.pair_target.as_ref().map(|f| f.address.clone()).or_else(|| self.model.paired_by_name(&name).map(|d| d.address.clone()));
        let Some(addr) = addr else { return self.ok("Paired.") };
        let phone = self.model.paired_rows().iter().any(|r| r.address.eq_ignore_ascii_case(&addr) && r.buttons.iter().any(|b| b.0 == RowAct::Tether));
        let r = if phone { self.model.tether_toggle(&addr) } else { self.model.connect_toggle(&addr) };
        self.run(r);
    }

    // ---------------------------------------------------------------------------------------------- drawing

    /// The page, inside `a` (below the title and above nothing: the status line is at its top).
    pub fn render(&mut self, g: &mut Gfx, a: R, s: f32) {
        let list = self.clamp_sel();
        // the status line has one fixed place: the top of the page
        match &self.status {
            Some((m, ok, _)) => {
                fx::status_line(g, a.x, a.y, a.w, s, m, *ok);
            }
            None => {}
        }
        let top = a.y + 26.0 * s;
        let avail = (a.y + a.h - top).max(0.0);
        let cw = a.w - 14.0 * s;
        let blocks = self.blocks(g, cw, s);
        let sel_block = blocks.iter().position(|(b, _)| self.block_focus(b, &list) == Some(self.sel)).unwrap_or(0);
        self.first = self.first.min(blocks.len().saturating_sub(1));
        if sel_block < self.first {
            self.first = sel_block;
        }
        // advance the first block until the chosen one fits
        loop {
            let used: f32 = blocks[self.first..=sel_block.max(self.first)].iter().map(|b| b.1 + 8.0 * s).sum();
            if used <= avail || self.first >= sel_block {
                break;
            }
            self.first += 1;
        }
        let mut y = top;
        for (idx, (b, h)) in blocks.iter().enumerate().skip(self.first) {
            let h = *h;
            if y + h > a.y + a.h + 0.5 {
                if idx == self.first {
                    // a block taller than the page: draw it anyway from the top (nothing else would be reachable)
                } else {
                    break;
                }
            }
            let h = &h;
            // a heading with nothing under it is not drawn at the bottom
            if matches!(b, Block::Heading(_)) && blocks.get(idx + 1).is_some_and(|n| y + h + 8.0 * s + n.1 > a.y + a.h + 0.5) {
                break;
            }
            let r = R::new(a.x, y, cw, *h);
            self.draw_block(g, b, r, s, &list);
            y += h + 8.0 * s;
        }
        let first = self.first;
        let visible = blocks.iter().enumerate().skip(first).scan(top, |y, (_, (_, h))| {
            let at = *y;
            *y += h + 8.0 * s;
            Some(at + h <= a.y + a.h + 0.5)
        }).filter(|v| *v).count();
        fx::scrollbar(g, R::new(a.x + a.w - 6.0 * s, top, 6.0 * s, avail), s, first, visible.max(1), blocks.len(), ACCENT);
    }

    /// The popup (and the forget card), drawn last over everything.
    pub fn render_dialogs(&mut self, g: &mut Gfx, area: R, s: f32) {
        if let Some((_, name)) = &self.forget {
            let text = format!("Forget {name}? It must be paired again to be used.");
            super::widgets::render_confirm(g, area, s, &text, "Yes, forget it");
            return;
        }
        let Some(d) = &self.dialog else { return self.render_popup(g, area, s) };
        let state = self.model.pair.clone();
        let ours = d.ours;
        let name = state_name(&state).unwrap_or(&d.name).to_string();
        let title = format!("Pair with {name}");
        let (digits, lines, buttons): (Option<String>, Vec<String>, Vec<(String, [u8; 4], Event)>) = match &state {
            PairState::Idle | PairState::Running(_) => (None, vec!["Connecting ...".into(), "Put the device in pairing mode if it does not answer.".into()], vec![("Cancel".into(), MUTED, Event::Key(Key::Esc))]),
            PairState::Confirm(_, v) => (
                Some(format!("{:03} {:03}", v / 1000, v % 1000)),
                vec!["Confirm this number on both devices.".into(), "Choose Yes only if it is the same.".into()],
                vec![("Yes".into(), if ours { ACCENT } else { MUTED }, Event::Key(Key::Char('y'))), ("No".into(), if ours { MUTED } else { ACCENT }, Event::Key(Key::Char('n')))],
            ),
            PairState::ConfirmNoNumber(_) => (
                None,
                vec![format!("Accept pairing with {name}?"), "It will not send audio or files. Say Yes only if you started this just now.".into()],
                vec![("Yes".into(), if ours { ACCENT } else { MUTED }, Event::Key(Key::Char('y'))), ("No".into(), if ours { MUTED } else { ACCENT }, Event::Key(Key::Char('n')))],
            ),
            PairState::ShowPasskey(_, k) => (Some(format!("{:03} {:03}", k / 1000, k % 1000)), vec![format!("Type this passkey on {name}, then press Enter there.")], vec![("Cancel".into(), ACCENT, Event::Key(Key::Esc))]),
            PairState::ShowPin(_, p) => (Some(p.clone()), vec![format!("Type this PIN on {name}, then press Enter there.")], vec![("Cancel".into(), ACCENT, Event::Key(Key::Esc))]),
            PairState::TypePasskey(_) => (
                None,
                vec![format!("Type the passkey shown on {name}."), "Then choose OK or press Enter.".into()],
                vec![("OK".into(), ACCENT, Event::Key(Key::Enter)), ("Cancel".into(), MUTED, Event::Key(Key::Esc))],
            ),
            PairState::TypePin(_, hint) => {
                let mut l = vec![format!("Type the PIN shown on {name} (1 to 16 characters).")];
                let mut b = vec![("OK".into(), ACCENT, Event::Key(Key::Enter)), ("Cancel".into(), MUTED, Event::Key(Key::Esc))];
                if *hint {
                    l.push(format!("If {name} shows no PIN, many use 0000 or 1234."));
                    b.push(("Use 0000".into(), MUTED, Event::Paste("0000".into())));
                    b.push(("Use 1234".into(), MUTED, Event::Paste("1234".into())));
                }
                (None, l, b)
            }
            PairState::Done(Ok(_)) => (None, vec!["Paired.".into(), "Connect now?".into()], vec![("Connect now".into(), ACCENT, Event::Key(Key::Enter)), ("Not now".into(), MUTED, Event::Key(Key::Esc))]),
            PairState::Done(Err(e)) => (None, vec!["Pairing did not work.".into(), e.clone()], vec![("Try again".into(), ACCENT, Event::Key(Key::Enter)), ("Close".into(), MUTED, Event::Key(Key::Esc))]),
        };
        let pw = (500.0 * s).min(area.w - 24.0 * s);
        let text_w = pw - 52.0 * s;
        let mut wrapped: Vec<String> = Vec::new();
        for l in &lines {
            wrapped.extend(g.wrap(15.0 * s, l, Face::Sans, text_w));
        }
        let size = 14.0 * s;
        let mut rows = 1usize;
        let mut bx = 0.0f32;
        for (l, _, _) in &buttons {
            let bw = g.text_width(size, l, Face::Bold) + size * 1.7;
            if bx > 0.0 && bx + bw > text_w {
                rows += 1;
                bx = 0.0;
            }
            bx += bw + 10.0 * s;
        }
        let typing = match &state {
            PairState::TypePasskey(_) => Some(true),
            PairState::TypePin(..) => Some(false),
            _ => None,
        };
        let msg_lines: Vec<String> = match (typing, d.msg.is_empty()) {
            (Some(_), false) => g.wrap(14.0 * s, &d.msg, Face::Sans, text_w),
            _ => vec![],
        };
        // the passkey is six boxes, the PIN one box; both sized to fit the popup at any text size
        let box_w = ((text_w - 5.0 * 6.0 * s) / PASSKEY_LEN as f32).clamp(10.0, 46.0 * s);
        let entry_h = match typing {
            Some(true) => (box_w * 1.3).max(30.0 * s).min(60.0 * s) + 12.0 * s,
            Some(false) => 52.0 * s + 12.0 * s,
            None => 0.0,
        };
        let digits_h = (if digits.is_some() { 74.0 * s } else { 0.0 }) + entry_h + msg_lines.len() as f32 * 19.0 * s;
        let ph = 56.0 * s + digits_h + wrapped.len() as f32 * 21.0 * s + 14.0 * s + rows as f32 * (size * 2.1 + 10.0 * s) + 20.0 * s;
        let p = fx::modal(g, area, s, pw, ph, ACCENT, false);
        let mut y = fx::modal_title(g, p, s, &title, ACCENT) + 4.0 * s;
        if let Some(dg) = &digits {
            let ds = 52.0 * s;
            let w = g.text_width(ds, dg, Face::Mono);
            g.text(p.x + (p.w - w) / 2.0, y, ds, TEXT, dg, Face::Mono);
            g.hit(R::new(p.x + 20.0 * s, y, p.w - 40.0 * s, 66.0 * s), Event::Tick, &format!("Number: {}", dg.replace(' ', " ")));
            y += digits_h;
        }
        for l in &wrapped {
            g.text(p.x + 26.0 * s, y, 15.0 * s, TEXT, l, Face::Sans);
            y += 21.0 * s;
        }
        if let Some(passkey) = typing {
            y = draw_entry(g, p.x + 26.0 * s, y + 4.0 * s, text_w, s, passkey, &d.entry, box_w, (self.ticks / 15) % 2 == 0);
            for l in &msg_lines {
                g.text(p.x + 26.0 * s, y, 14.0 * s, WARN, l, Face::Sans);
                y += 19.0 * s;
            }
        }
        y += 14.0 * s;
        g.button_flow(p.x + 26.0 * s, y, text_w, size, 10.0 * s, &buttons);
    }

    /// The scan popup: the frame of the Wi-Fi list, "Scanning..." or the problem, a short list of devices (scrolls), the buttons.
    fn render_popup(&mut self, g: &mut Gfx, area: R, s: f32) {
        if self.popup.is_none() {
            return;
        }
        let running = match self.model.scanning() {
            ScanView::Running(secs) => Some(secs),
            ScanView::Idle => None,
        };
        let err = self.model.scan_error().map(|e| e.to_string());
        let n = self.model.nearby(&self.search.value, self.all);
        let rows = n.rows.clone();
        let pw = (520.0 * s).min(area.w - 40.0 * s);
        let text_w = pw - 52.0 * s;
        let msg = self.popup.as_ref().map(|p| p.msg.clone()).unwrap_or_default();
        // the words under the title
        let (info, info_col): (String, [u8; 4]) = match (&running, &err) {
            (Some(secs), _) if rows.is_empty() => (format!("Scanning... {secs} s. Keyboards, mice, game pads and phones appear here."), TEXT),
            (Some(secs), _) => (format!("Scanning... {secs} s"), TEXT),
            (None, Some(e)) => (format!("The scan did not work. {e}"), WARN),
            (None, None) if !self.search.value.is_empty() && rows.is_empty() => ("No device matches the search.".to_string(), MUTED),
            (None, None) if n.total == 0 => ("No other device was found. Put the device in pairing mode and scan again.".to_string(), MUTED),
            (None, None) => (format!("Scan finished: {} device{} found. Choose Pair next to the one you want.", n.total, if n.total == 1 { "" } else { "s" }), MUTED),
        };
        let mut info_lines: Vec<String> = g.wrap(14.0 * s, &info, Face::Sans, text_w).into_iter().take(4).collect();
        if !msg.is_empty() {
            info_lines.extend(g.wrap(13.0 * s, &msg, Face::Sans, text_w).into_iter().take(3));
        }
        let msg_from = info_lines.len() - if msg.is_empty() { 0 } else { g.wrap(13.0 * s, &msg, Face::Sans, text_w).len().min(3) };
        // the buttons
        let more_label = if self.all { "Show fewer devices".to_string() } else { format!("Show {} more device{}", n.more, if n.more == 1 { "" } else { "s" }) };
        let mut buttons: Vec<(String, [u8; 4], Event)> = Vec::new();
        let mut again_at = None;
        if running.is_some() {
            buttons.push(("Stop".into(), MUTED, Event::Sub(SUB_SCAN, Key::Enter)));
        } else {
            again_at = Some(buttons.len());
            buttons.push((if err.is_some() { "Try again" } else { "Scan again" }.into(), ACCENT, Event::Sub(SUB_AGAIN, Key::Enter)));
            if self.search.value.is_empty() && (n.more > 0 || (self.all && n.total > NEARBY_SHOWN)) {
                buttons.push((more_label, MUTED, Event::Sub(SUB_MORE, Key::Enter)));
            }
        }
        buttons.push(("Close".into(), MUTED, Event::Key(Key::Esc)));
        let bsize = 14.0 * s;
        let bh = bsize * 2.1;
        let brows = flow_rows(g, text_w, bsize, 10.0 * s, &buttons.iter().map(|b| b.0.as_str()).collect::<Vec<_>>());
        let search_h = if self.all || n.total > NEARBY_SHOWN { 44.0 * s } else { 0.0 };
        let rowh = 58.0 * s;
        let fixed = 56.0 * s + info_lines.len() as f32 * 19.0 * s + 8.0 * s + search_h + 12.0 * s + brows as f32 * (bh + 10.0 * s) + 20.0 * s;
        let room = (area.h - 16.0 * s - fixed).max(0.0);
        let fit = if rows.is_empty() { 0 } else { ((room / rowh).floor() as usize).clamp(1, 5).min(rows.len()) };
        let ph = fixed + fit as f32 * rowh;
        let p = fx::modal(g, area, s, pw, ph, ACCENT, false);
        let mut y = fx::modal_title(g, p, s, "Scan for devices", ACCENT) + 2.0 * s;
        if running.is_some() {
            let pulse = ((self.ticks / 6) % 3) as f32;
            for k in 0..3 {
                g.circle(p.x + p.w - 30.0 * s + k as f32 * 8.0 * s - 12.0 * s, p.y + 30.0 * s, 2.0 * s + if k as f32 == pulse { 2.0 * s } else { 0.0 }, ACCENT);
            }
        }
        for (i, l) in info_lines.iter().enumerate() {
            let (size, col) = if i >= msg_from { (13.0 * s, WARN) } else { (14.0 * s, info_col) };
            g.text(p.x + 26.0 * s, y, size, col, l, Face::Sans);
            y += 19.0 * s;
        }
        y += 8.0 * s;
        if search_h > 0.0 {
            let focused = self.search_focus;
            self.search.render(g, R::new(p.x + 26.0 * s, y, text_w, 36.0 * s), focused, s, (self.ticks / 15) % 2 == 0, Face::Sans, SEARCH_ID);
            y += search_h;
        }
        // keep the chosen row inside the window
        let sel = self.popup.as_ref().map(|p| p.sel).unwrap_or(0);
        let mut first = self.popup.as_ref().map(|p| p.first).unwrap_or(0);
        if fit > 0 {
            if sel < rows.len() {
                if sel < first {
                    first = sel;
                } else if sel >= first + fit {
                    first = sel + 1 - fit;
                }
            }
            first = first.min(rows.len().saturating_sub(fit));
        } else {
            first = 0;
        }
        if let Some(pp) = self.popup.as_mut() {
            pp.first = first;
            pp.fit = fit.max(1);
        }
        for (k, row) in rows.iter().enumerate().skip(first).take(fit) {
            let rr = R::new(p.x + 20.0 * s, y + (k - first) as f32 * rowh, p.w - 40.0 * s - if rows.len() > fit { 10.0 * s } else { 0.0 }, rowh - 4.0 * s);
            self.draw_nearby(g, rr, s, row, k, k == sel);
            if k == sel {
                access::report_focus(access::Node::new(&format!("{}, {}", row.name, if row.pairable { "pair" } else { "not supported" }), Role::Button, State::default()));
            }
        }
        if rows.len() > fit && fit > 0 {
            fx::scrollbar(g, R::new(p.x + p.w - 28.0 * s, y, 8.0 * s, fit as f32 * rowh), s, first, fit, rows.len(), ACCENT);
        }
        y += fit as f32 * rowh + 12.0 * s;
        // the buttons, one flow; the chosen one gets a ring
        let (mut bx, mut by) = (p.x + 26.0 * s, y);
        let x0 = bx;
        for (i, (label, acc, ev)) in buttons.iter().enumerate() {
            let bw = g.text_width(bsize, label, Face::Bold) + bsize * 1.7;
            if bx > x0 && bx + bw > x0 + text_w {
                bx = x0;
                by += bh + 10.0 * s;
            }
            g.button(bx, by, bsize, label, *acc, ev.clone());
            if again_at == Some(i) && sel == rows.len() {
                g.rrect_outline(bx - 3.0 * s, by - 3.0 * s, bw + 6.0 * s, bh + 6.0 * s, (bh + 6.0 * s) / 2.0, 2.0 * s, TEXT);
                access::report_focus(access::Node::new(label, Role::Button, State::default()));
            }
            bx += bw + 10.0 * s;
        }
    }

    fn blocks(&self, g: &mut Gfx, w: f32, s: f32) -> Vec<(Block, f32)> {
        let mut v = Vec::new();
        v.push((Block::Switch, self.switch_height(g, w, s)));
        v.push((Block::Heading("Paired devices"), 22.0 * s));
        let paired = self.model.paired_rows();
        if paired.is_empty() {
            v.push((Block::NoPaired, 44.0 * s));
        }
        for (i, r) in paired.iter().enumerate() {
            v.push((Block::Paired(i), self.paired_height(g, w, s, r)));
        }
        if self.model.view.switch == Switch::On && self.model.audio.ready {
            v.push((Block::AudioCard, 56.0 * s));
        }
        v.push((Block::Heading("Nearby devices"), 22.0 * s));
        v.push((Block::Scan, self.scan_height(g, w, s)));
        v.push((Block::AdvancedToggle, 46.0 * s));
        if self.advanced {
            v.push((Block::Advanced, self.advanced_height(g, w, s)));
        }
        v
    }

    /// The focus index a block stands for.
    fn block_focus(&self, b: &Block, list: &[Focus]) -> Option<usize> {
        let paired = self.model.paired_rows();
        let _ = paired;
        match b {
            Block::Switch => Some(0),
            Block::Paired(i) => Some(1 + i),
            Block::Scan => list.iter().position(|f| *f == Focus::Scan),
            Block::AdvancedToggle => list.iter().position(|f| *f == Focus::Advanced),
            _ => None,
        }
    }

    fn switch_lines(&self, g: &mut Gfx, w: f32, s: f32) -> Vec<String> {
        let why = self.model.view.switch.why();
        if why.is_empty() {
            return Vec::new();
        }
        g.wrap(13.0 * s, why, Face::Sans, w - 36.0 * s).into_iter().take(4).collect()
    }

    fn fix_labels(&self) -> Vec<(String, [u8; 4], Event)> {
        self.model
            .view
            .fixes()
            .into_iter()
            .map(|f| match f {
                Fix::Firmware => ("Install firmware".to_string(), ACCENT, Event::Sub(SUB_FIRMWARE, Key::Enter)),
                Fix::LoadDriver => ("Load the driver anyway".to_string(), MUTED, Event::Sub(SUB_DRIVER, Key::Enter)),
            })
            .collect()
    }

    fn switch_height(&self, g: &mut Gfx, w: f32, s: f32) -> f32 {
        let mut h = 70.0 * s;
        if self.model.view.headline.is_some() {
            h += 20.0 * s;
        }
        h += self.switch_lines(g, w, s).len() as f32 * 17.0 * s;
        let fixes = self.fix_labels();
        if !fixes.is_empty() {
            let rows = flow_rows(g, w - 36.0 * s, 14.0 * s, 10.0 * s, &fixes.iter().map(|f| f.0.as_str()).collect::<Vec<_>>());
            h += 8.0 * s + rows as f32 * (14.0 * s * 2.1 + 10.0 * s);
        }
        h + 8.0 * s
    }

    fn paired_inline(&self, g: &mut Gfx, w: f32, s: f32, r: &PairedRow) -> bool {
        let labels: Vec<&str> = r.buttons.iter().map(|b| b.1.as_str()).collect();
        let bw = buttons_width(g, 13.0 * s, &labels, 8.0 * s);
        w - 66.0 * s - bw - 20.0 * s >= 190.0 * s
    }

    fn paired_height(&self, g: &mut Gfx, w: f32, s: f32, r: &PairedRow) -> f32 {
        if self.paired_inline(g, w, s, r) {
            64.0 * s
        } else {
            let labels: Vec<&str> = r.buttons.iter().map(|b| b.1.as_str()).collect();
            let rows = flow_rows(g, w - 66.0 * s - 12.0 * s, 13.0 * s, 8.0 * s, &labels);
            64.0 * s + rows as f32 * (13.0 * s * 2.1 + 8.0 * s) + 4.0 * s
        }
    }

    fn scan_height(&self, g: &mut Gfx, w: f32, s: f32) -> f32 {
        let tw = self.scan_text_w(g, w, s);
        let lines = g.wrap(13.0 * s, &self.scan_text(), Face::Sans, tw).len().min(3) as f32;
        (lines * 17.0 * s + 22.0 * s).max(56.0 * s)
    }

    fn scan_text(&self) -> String {
        if self.model.view.switch == Switch::On {
            "Look for keyboards, mice, game pads and phones to pair. The list opens in a popup.".to_string()
        } else {
            "Turn Bluetooth on to look for devices.".to_string()
        }
    }

    fn scan_text_w(&self, g: &mut Gfx, w: f32, s: f32) -> f32 {
        let bw = if self.model.view.switch == Switch::On { g.text_width(14.0 * s, "Scan", Face::Bold) + 14.0 * s * 1.7 } else { 0.0 };
        (w - 36.0 * s - bw - 12.0 * s).max(60.0)
    }

    fn advanced_height(&self, _g: &mut Gfx, _w: f32, s: f32) -> f32 {
        let mut n = 3.0;
        if self.model.view.state.held {
            n += 1.0;
        }
        n * 50.0 * s + 8.0 * s
    }

    fn draw_block(&mut self, g: &mut Gfx, b: &Block, r: R, s: f32, list: &[Focus]) {
        match b {
            Block::Switch => self.draw_switch(g, r, s, list),
            Block::Heading(t) => {
                fx::section_title(g, r.x, r.y, r.w, s, t, fx::CYAN);
            }
            Block::NoPaired => {
                let t = if self.model.view.switch == Switch::On { "Nothing is paired yet. Put a keyboard, mouse or phone in pairing mode and choose Scan, then Pair next to it." } else { "Nothing is paired yet." };
                for (i, l) in g.wrap(13.0 * s, t, Face::Sans, r.w - 8.0 * s).into_iter().take(2).enumerate() {
                    g.text(r.x + 4.0 * s, r.y + 4.0 * s + i as f32 * 17.0 * s, 13.0 * s, DIM, &l, Face::Sans);
                }
            }
            Block::Paired(i) => {
                let rows = self.model.paired_rows();
                if let Some(row) = rows.get(*i) {
                    self.draw_paired(g, r, s, row, 1 + i);
                }
            }
            Block::AudioCard => self.draw_audio(g, r, s),
            Block::Scan => self.draw_scan(g, r, s, list),
            Block::AdvancedToggle => {
                let fi = list.iter().position(|f| *f == Focus::Advanced).unwrap_or(0);
                let t = if self.advanced { "Advanced (open)" } else { "Advanced" };
                self.draw_simple_row(g, r, s, t, "The radio and the controller separately, the driver, look again.", fi, Event::Sub(SUB_ADVANCED, Key::Enter));
                access::describe(t, &Event::Sub(SUB_ADVANCED, Key::Enter), Meta::new(Role::Button, State::default().expanded(self.advanced)).named("Advanced"));
            }
            Block::Advanced => self.draw_advanced(g, r, s),
        }
    }

    fn draw_simple_row(&mut self, g: &mut Gfx, r: R, s: f32, title: &str, sub: &str, fi: usize, ev: Event) {
        let hot = g.hovered(r);
        fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, fi == self.sel, hot, ACCENT);
        g.hit(r, ev, title);
        let t = g.fit(15.0 * s, title, Face::Bold, r.w - 36.0 * s);
        g.text(r.x + 18.0 * s, r.y + 6.0 * s, 15.0 * s, TEXT, &t, Face::Bold);
        let sub = g.fit(12.0 * s, sub, Face::Sans, r.w - 36.0 * s);
        g.text(r.x + 18.0 * s, r.y + 26.0 * s, 12.0 * s, DIM, &sub, Face::Sans);
    }

    fn draw_switch(&mut self, g: &mut Gfx, r: R, s: f32, list: &[Focus]) {
        let sw = self.model.view.switch;
        let _ = list;
        let hot = g.hovered(r);
        fx::row_at(g, r.x, r.y, r.w, r.h, 14.0 * s, self.sel == 0, hot, ACCENT);
        g.text(r.x + 18.0 * s, r.y + 12.0 * s, 17.0 * s, TEXT, "Bluetooth", Face::Bold);
        let col = match sw {
            Switch::On => OK,
            Switch::Off => DIM,
            _ => WARN,
        };
        g.text(r.x + 18.0 * s, r.y + 38.0 * s, 15.0 * s, col, sw.words(), Face::Bold);
        let (swr_w, swr_h) = (56.0 * s, 30.0 * s);
        let sr = R::new(r.x + r.w - 18.0 * s - swr_w, r.y + 20.0 * s, swr_w, swr_h);
        let on = sw == Switch::On;
        fx::switch(g, sr, on, sw.can_toggle(), ACCENT, s);
        // the whole top of the card is the switch (a fix button below has its own region)
        let top = R::new(r.x, r.y, r.w, 68.0 * s);
        g.hit(top, Event::Sub(SUB_SWITCH, Key::Enter), "Bluetooth");
        access::describe("Bluetooth", &Event::Sub(SUB_SWITCH, Key::Enter), Meta::new(Role::Toggle, State::default().checked(on).disabled(!sw.can_toggle()).value(sw.words())).named("Bluetooth"));
        let mut y = r.y + 66.0 * s;
        if let Some(h) = &self.model.view.headline {
            let t = g.fit(12.0 * s, h, Face::Sans, r.w - 36.0 * s);
            g.text(r.x + 18.0 * s, y, 12.0 * s, MUTED, &t, Face::Sans);
            y += 20.0 * s;
        }
        for l in self.switch_lines(g, r.w, s) {
            g.text(r.x + 18.0 * s, y, 13.0 * s, DIM, &l, Face::Sans);
            y += 17.0 * s;
        }
        let fixes = self.fix_labels();
        if !fixes.is_empty() {
            g.button_flow(r.x + 18.0 * s, y + 8.0 * s, r.w - 36.0 * s, 14.0 * s, 10.0 * s, &fixes);
        }
    }

    fn draw_audio(&mut self, g: &mut Gfx, r: R, s: f32) {
        let a = &self.model.audio;
        let (title, button) = match &a.device {
            Some(d) if d.state == crate::btmodel::AudioState::Playing => (format!("Playing from {}", d.title()), "Disconnect"),
            Some(d) => (format!("{} is connected for audio", d.title()), "Disconnect"),
            None if a.pairing_open => ("Waiting for a phone to pair".to_string(), "Close pairing"),
            None => ("Audio from a phone".to_string(), "Let a phone pair"),
        };
        g.rrect(r.x, r.y, r.w, r.h, 12.0 * s, rgb(22, 26, 42));
        let size = 13.0 * s;
        let bw = g.text_width(size, button, Face::Bold) + size * 1.7;
        let tw = (r.w - bw - 40.0 * s).max(60.0);
        let t = g.fit(15.0 * s, &title, Face::Bold, tw);
        g.text(r.x + 16.0 * s, r.y + 7.0 * s, 15.0 * s, TEXT, &t, Face::Bold);
        let line = g.fit(12.0 * s, &a.line(), Face::Sans, tw);
        g.text(r.x + 16.0 * s, r.y + 30.0 * s, 12.0 * s, DIM, &line, Face::Sans);
        g.button(r.x + r.w - bw - 12.0 * s, r.y + (r.h - size * 2.1) / 2.0, size, button, ACCENT, Event::Sub(SUB_AUDIO, Key::Enter));
    }

    /// The page's Scan row: what it does, and the button that opens the scan popup.
    fn draw_scan(&mut self, g: &mut Gfx, r: R, s: f32, list: &[Focus]) {
        let can = self.model.view.switch == Switch::On;
        let fi = list.iter().position(|f| *f == Focus::Scan);
        let hot = g.hovered(r);
        fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, fi == Some(self.sel), hot, ACCENT);
        let tw = self.scan_text_w(g, r.w, s);
        for (i, l) in g.wrap(13.0 * s, &self.scan_text(), Face::Sans, tw).into_iter().take(3).enumerate() {
            g.text(r.x + 18.0 * s, r.y + 10.0 * s + i as f32 * 17.0 * s, 13.0 * s, if can { MUTED } else { DIM }, &l, Face::Sans);
        }
        if can {
            g.hit(r, Event::Sub(SUB_SCAN, Key::Enter), "Scan for devices");
            let size = 14.0 * s;
            let bw = g.text_width(size, "Scan", Face::Bold) + size * 1.7;
            g.button_touch(r.x + r.w - bw - 12.0 * s, r.y + (r.h - size * 2.1) / 2.0, size, "Scan", ACCENT, Event::Sub(SUB_SCAN, Key::Enter), 40.0 * s);
        }
    }

    fn draw_paired(&mut self, g: &mut Gfx, r: R, s: f32, row: &PairedRow, fi: usize) {
        let hot = g.hovered(r);
        fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, fi == self.sel, hot, ACCENT);
        if fi < MAX_ROW_CODE {
            g.hit(r, Event::Sub(fi as u8, Key::Enter), &format!("{}, {}", row.name, row.chip));
        }
        draw_icon(g, row.icon, r.x + 14.0 * s, r.y + 14.0 * s, 36.0 * s, MUTED);
        let inline = self.paired_inline(g, r.w, s, row);
        let labels: Vec<&str> = row.buttons.iter().map(|b| b.1.as_str()).collect();
        let bw = buttons_width(g, 13.0 * s, &labels, 8.0 * s);
        let text_w = if inline { r.w - 66.0 * s - bw - 20.0 * s } else { r.w - 66.0 * s - 12.0 * s };
        let tx = r.x + 62.0 * s;
        let chip_w = g.text_width(12.0 * s, row.chip, Face::Bold) + 12.0 * s * 1.4 + 8.0 * s;
        let name = g.fit(16.0 * s, &row.name, Face::Bold, (text_w - chip_w).max(40.0));
        let nw = g.text_width(16.0 * s, &name, Face::Bold);
        g.text(tx, r.y + 8.0 * s, 16.0 * s, TEXT, &name, Face::Bold);
        let col = match row.tone {
            Tone::Good => OK,
            Tone::Quiet => DIM,
            Tone::Warn => WARN,
        };
        fx::chip(g, tx + nw + 8.0 * s, r.y + 7.0 * s, s, row.chip, col, false);
        let det = g.fit(12.0 * s, &row.detail, Face::Sans, text_w);
        g.text(tx, r.y + 34.0 * s, 12.0 * s, MUTED, &det, Face::Sans);
        let items: Vec<(String, [u8; 4], Event)> = row
            .buttons
            .iter()
            .map(|(a, l)| {
                let code = match a {
                    RowAct::Connect => 'c',
                    RowAct::Tether => 't',
                    RowAct::Audio => 'a',
                    RowAct::Forget => 'f',
                };
                let acc = if *a == RowAct::Forget { MUTED } else { ACCENT };
                (l.clone(), acc, Event::Sub(fi.min(255) as u8, Key::Char(code)))
            })
            .collect();
        if inline {
            let bh = 13.0 * s * 2.1;
            let mut bx = r.x + r.w - 12.0 * s - bw + 8.0 * s;
            for (l, acc, ev) in &items {
                bx += g.button(bx, r.y + (r.h - bh) / 2.0, 13.0 * s, l, *acc, ev.clone()) + 8.0 * s;
            }
        } else {
            g.button_flow(tx, r.y + 58.0 * s, r.w - 66.0 * s - 12.0 * s, 13.0 * s, 8.0 * s, &items);
        }
    }

    fn draw_nearby(&mut self, g: &mut Gfx, r: R, s: f32, row: &NearbyRow, fi: usize, chosen: bool) {
        let hot = g.hovered(r);
        fx::row_at(g, r.x, r.y, r.w, r.h, 12.0 * s, chosen, hot, ACCENT);
        if fi < MAX_ROW_CODE {
            g.hit(r, Event::Sub(fi as u8, Key::Enter), &format!("{}, {}", row.name, if row.pairable { "pair" } else { "not supported" }));
        }
        draw_icon(g, row.icon, r.x + 12.0 * s, r.y + (r.h - 32.0 * s) / 2.0, 32.0 * s, MUTED);
        let size = 13.0 * s;
        let right_w = if row.pairable { g.text_width(size, "Pair", Face::Bold) + size * 1.7 + 12.0 * s } else { g.text_width(12.0 * s, "Not supported", Face::Bold) + 24.0 * s };
        let tx = r.x + 54.0 * s;
        let tw = (r.w - 54.0 * s - right_w - 8.0 * s).max(60.0);
        let name = g.fit(15.0 * s, &row.name, Face::Bold, tw);
        g.text(tx, r.y + 7.0 * s, 15.0 * s, if row.named { TEXT } else { MUTED }, &name, if row.named { Face::Bold } else { Face::Mono });
        let sub = if row.pairable || row.note.is_empty() { format!("{} · {}", row.icon.word(), row.signal) } else { format!("{} · {}", row.icon.word(), row.note) };
        let sub = g.fit(12.0 * s, &sub, Face::Sans, tw);
        g.text(tx, r.y + 29.0 * s, 12.0 * s, DIM, &sub, Face::Sans);
        if row.pairable {
            let bw = g.text_width(size, "Pair", Face::Bold) + size * 1.7;
            g.button(r.x + r.w - bw - 12.0 * s, r.y + (r.h - size * 2.1) / 2.0, size, "Pair", ACCENT, Event::Sub(fi.min(255) as u8, Key::Enter));
        } else {
            fx::chip(g, r.x + r.w - right_w + 4.0 * s, r.y + (r.h - 12.0 * s * 1.9) / 2.0, s, "Not supported", DIM, false);
        }
    }

    fn draw_advanced(&mut self, g: &mut Gfx, r: R, s: f32) {
        let v = &self.model.view;
        let radio = if v.state.switches.is_empty() { "no radio switch on this adapter".to_string() } else if v.state.hard_blocked() { "blocked by the hardware switch".into() } else if v.radio_on { "on".into() } else { "off".into() };
        let ctl = match (v.state.controller(), v.powered) {
            (None, _) => "no controller".to_string(),
            (Some(c), _) if !c.configured => "not set up".into(),
            (_, true) => "powered on".into(),
            _ => "powered off".into(),
        };
        let mut items: Vec<(String, String, String, Event)> = vec![
            ("Radio".into(), format!("The radio switch (the key on the machine, rfkill): {radio}. The Bluetooth switch above sets this and the controller together."), if v.radio_on { "Turn off".into() } else { "Turn on".into() }, Event::Sub(SUB_ADV, Key::Char('r'))),
            ("Controller".into(), format!("The controller's power: {ctl}."), if v.powered { "Power off".into() } else { "Power on".into() }, Event::Sub(SUB_ADV, Key::Char('c'))),
        ];
        if v.state.held {
            items.push(("Driver".into(), "The Bluetooth drivers are set aside until firmware exists.".into(), "Load driver".into(), Event::Sub(SUB_DRIVER, Key::Enter)));
        }
        items.push(("Hardware".into(), "Read the adapter and its state again.".into(), "Look again".into(), Event::Sub(SUB_ADV, Key::Char('l'))));
        let rh = 50.0 * s;
        for (i, (t, d, b, ev)) in items.iter().enumerate() {
            let rr = R::new(r.x + 12.0 * s, r.y + i as f32 * rh, r.w - 12.0 * s, rh - 4.0 * s);
            g.rrect(rr.x, rr.y, rr.w, rr.h, 10.0 * s, rgb(22, 26, 42));
            let size = 13.0 * s;
            let bw = g.text_width(size, b, Face::Bold) + size * 1.7;
            let tw = (rr.w - bw - 36.0 * s).max(60.0);
            g.text(rr.x + 14.0 * s, rr.y + 5.0 * s, 14.0 * s, TEXT, t, Face::Bold);
            let d = g.fit(11.0 * s, d, Face::Sans, tw);
            g.text(rr.x + 14.0 * s, rr.y + 25.0 * s, 11.0 * s, DIM, &d, Face::Sans);
            g.button(rr.x + rr.w - bw - 10.0 * s, rr.y + (rr.h - size * 2.1) / 2.0, size, b, ACCENT, ev.clone());
        }
    }
}

fn state_name(state: &PairState) -> Option<&str> {
    match state {
        PairState::Running(n) | PairState::Confirm(n, _) | PairState::ConfirmNoNumber(n) | PairState::ShowPasskey(n, _) | PairState::ShowPin(n, _) | PairState::TypePasskey(n) | PairState::TypePin(n, _) if !n.is_empty() => Some(n),
        _ => None,
    }
}

/// The words the screen reader gets when the popup changes (the PIN and the passkey are spoken, never logged).
pub fn dialog_words(name: &str, state: &PairState) -> String {
    let spaced = |v: String| v.chars().map(|c| c.to_string()).collect::<Vec<_>>().join(" ");
    match state {
        PairState::Idle | PairState::Running(_) => format!("Pair with {name}. Connecting."),
        PairState::Confirm(_, v) => format!("Pair with {name}. Confirm this number on both devices: {}. Yes or No.", spaced(format!("{v:06}"))),
        PairState::ConfirmNoNumber(_) => format!("Pair with {name}. Accept pairing with {name}? It will not send audio or files. Yes or No."),
        PairState::ShowPasskey(_, k) => format!("Pair with {name}. Type this passkey on {name}: {}.", spaced(format!("{k:06}"))),
        PairState::ShowPin(_, p) => format!("Pair with {name}. Type this PIN on {name}: {}.", spaced(p.clone())),
        PairState::TypePasskey(_) => format!("Pair with {name}. Type the passkey shown on {name}. Passkey, edit text, 6 digits. Enter to send, Escape to cancel."),
        PairState::TypePin(_, hint) => format!(
            "Pair with {name}. Type the PIN shown on {name}. PIN, edit text, 1 to 16 characters.{} Enter to send, Escape to cancel.",
            if *hint { " If it shows none, many use 0000 or 1234." } else { "" }
        ),
        PairState::Done(Ok(_)) => format!("Pair with {name}. Paired. Connect now?"),
        PairState::Done(Err(e)) => format!("Pair with {name}. Pairing did not work. {e}"),
    }
}

/// The entry of the passkey (six boxes, the digits visible: the number is on the other device's screen too) or the PIN (one box). Registers
/// one field for the screen reader: name and count, never what was typed. Returns the y below it.
#[allow(clippy::too_many_arguments)]
fn draw_entry(g: &mut Gfx, x: f32, y: f32, w: f32, s: f32, passkey: bool, entry: &str, box_w: f32, blink: bool) -> f32 {
    let typed: Vec<char> = entry.chars().collect();
    let (label, count, h) = if passkey {
        ("Passkey", format!("{} of {PASSKEY_LEN} digits", typed.len()), (box_w * 1.3).max(30.0 * s).min(60.0 * s))
    } else {
        ("PIN", format!("{} of up to {PIN_MAX} characters", typed.len()), 52.0 * s)
    };
    let field = R::new(x, y, if passkey { 6.0 * box_w + 5.0 * 6.0 * s } else { w }, h);
    if passkey {
        for i in 0..PASSKEY_LEN {
            let bx = x + i as f32 * (box_w + 6.0 * s);
            let here = i == typed.len().min(PASSKEY_LEN - 1);
            g.rrect(bx, y, box_w, h, 8.0 * s, rgb(10, 14, 28));
            g.rrect_outline(bx, y, box_w, h, 8.0 * s, if here { 2.0 * s } else { 1.0 * s }, if here { ACCENT } else { DIM });
            let ds = (h * 0.62).min(box_w * 1.1);
            if let Some(c) = typed.get(i) {
                let t = c.to_string();
                let tw = g.text_width(ds, &t, Face::Mono);
                g.text(bx + (box_w - tw) / 2.0, y + (h - g.line_height(ds, Face::Mono)) / 2.0, ds, TEXT, &t, Face::Mono);
            } else if here && blink {
                g.rect(bx + box_w / 2.0 - 1.0 * s, y + h * 0.22, 2.0 * s, h * 0.56, ACCENT);
            }
        }
    } else {
        g.rrect(x, y, w, h, 10.0 * s, rgb(10, 14, 28));
        g.rrect_outline(x, y, w, h, 10.0 * s, 2.0 * s, ACCENT);
        let ds = 30.0 * s;
        let shown: String = typed.iter().collect();
        let shown = g.fit(ds, &shown, Face::Mono, w - 28.0 * s);
        let ty = y + (h - g.line_height(ds, Face::Mono)) / 2.0;
        g.text(x + 14.0 * s, ty, ds, TEXT, &shown, Face::Mono);
        if blink {
            let cw = g.text_width(ds, &shown, Face::Mono);
            g.rect(x + 16.0 * s + cw, y + h * 0.2, 2.0 * s, h * 0.6, ACCENT);
        }
    }
    g.hit(field, Event::Tick, label);
    let node_state = State::default().value(count);
    access::describe(label, &Event::Tick, Meta::new(Role::Field, node_state.clone()).named(label));
    access::report_focus(access::Node::new(label, Role::Field, node_state));
    y + h + 12.0 * s
}

fn buttons_width(g: &mut Gfx, size: f32, labels: &[&str], gap: f32) -> f32 {
    labels.iter().map(|l| g.text_width(size, l, Face::Bold) + size * 1.7 + gap).sum::<f32>()
}

fn flow_rows(g: &mut Gfx, w: f32, size: f32, gap: f32, labels: &[&str]) -> usize {
    let (mut rows, mut x) = (1usize, 0.0f32);
    for l in labels {
        let bw = g.text_width(size, l, Face::Bold) + size * 1.7;
        if x > 0.0 && x + bw > w {
            rows += 1;
            x = 0.0;
        }
        x += bw + gap;
    }
    rows
}

/// A small picture of the kind of device, drawn with strokes (no font, no image).
fn draw_icon(g: &mut Gfx, icon: Icon, x: f32, y: f32, size: f32, col: [u8; 4]) {
    let u = size / 36.0;
    g.rrect(x, y, size, size, 8.0 * u, rgb(26, 31, 52));
    let (cx, cy) = (x + size / 2.0, y + size / 2.0);
    match icon {
        Icon::Keyboard => {
            g.rrect_outline(x + 5.0 * u, cy - 7.0 * u, 26.0 * u, 14.0 * u, 3.0 * u, 1.6 * u, col);
            for row in 0..2 {
                for k in 0..4 {
                    g.rect(x + 9.0 * u + k as f32 * 5.5 * u, cy - 4.0 * u + row as f32 * 5.0 * u, 3.0 * u, 2.0 * u, col);
                }
            }
        }
        Icon::Mouse => {
            g.rrect_outline(cx - 7.0 * u, cy - 11.0 * u, 14.0 * u, 22.0 * u, 7.0 * u, 1.6 * u, col);
            g.line(cx, cy - 11.0 * u, cx, cy - 3.0 * u, 1.4 * u, col);
        }
        Icon::GamePad => {
            g.rrect_outline(x + 5.0 * u, cy - 7.0 * u, 26.0 * u, 15.0 * u, 7.0 * u, 1.6 * u, col);
            g.line(x + 11.0 * u, cy - 2.0 * u, x + 11.0 * u, cy + 4.0 * u, 1.6 * u, col);
            g.line(x + 8.0 * u, cy + 1.0 * u, x + 14.0 * u, cy + 1.0 * u, 1.6 * u, col);
            g.circle(x + 25.0 * u, cy - 1.0 * u, 1.8 * u, col);
            g.circle(x + 21.0 * u, cy + 3.0 * u, 1.8 * u, col);
        }
        Icon::Phone => {
            g.rrect_outline(cx - 7.0 * u, cy - 12.0 * u, 14.0 * u, 24.0 * u, 3.0 * u, 1.6 * u, col);
            g.rect(cx - 3.0 * u, cy + 8.0 * u, 6.0 * u, 1.5 * u, col);
        }
        Icon::Headphones => {
            g.rrect_outline(cx - 10.0 * u, cy - 11.0 * u, 20.0 * u, 18.0 * u, 9.0 * u, 1.8 * u, col);
            g.rrect(cx - 12.0 * u, cy - 1.0 * u, 6.0 * u, 11.0 * u, 2.5 * u, col);
            g.rrect(cx + 6.0 * u, cy - 1.0 * u, 6.0 * u, 11.0 * u, 2.5 * u, col);
        }
        Icon::Other => {
            g.circle(cx, cy, 9.0 * u, col);
            g.circle(cx, cy, 6.5 * u, rgb(26, 31, 52));
            g.circle(cx, cy, 2.0 * u, col);
        }
    }
}

#[cfg(test)]
mod entry_tests {
    use super::*;

    #[test]
    fn the_typed_number_is_overwritten_and_emptied_by_wipe() {
        let mut d = Dialog::new("Pixel".into(), true);
        d.entry.push_str("123456");
        d.wipe();
        assert!(d.entry.is_empty());
    }
}
