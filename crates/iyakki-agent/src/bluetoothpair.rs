//! The assistant's tools for pairing a new Bluetooth device (F-NET-18): `bluetooth_scan` looks for nearby devices, `bluetooth_pair`
//! starts pairing with one of them, `bluetooth_pair_status` says where the pairing is, and `bluetooth_pair_answer` says yes or no to
//! the number comparison. Each one that does something asks the person on a card.
//!
//! A number the other device shows (a passkey or PIN to type here) is typed by the person in Settings -> Bluetooth, never through
//! the chat: the assistant is told that a number is needed and where to type it, but there is no tool that takes the digits, so the
//! secret never passes through the model. A number the computer shows (to type on a keyboard) is told to the person, as it is on
//! the screen anyway. The work is the same as the Settings page's ([`iyakki_devctl::Control`]); this file finds the device, writes the
//! card and turns the pairing state into a sentence.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use iyakki_devctl::bthid::{is_pairable_kind, PairState};
use iyakki_devctl::btmgmt::{Found, Kind};
use iyakki_devctl::Control;

pub const PROMPT_RULE: &str = "\nPairing a new Bluetooth device: bluetooth_scan (asks) lists nearby devices, bluetooth_pair (asks) starts pairing with one the scan found, bluetooth_pair_status says where it is, bluetooth_pair_answer (asks) says yes or no to the number comparison after the person has looked at both screens. If the device shows a number to type on this computer, tell the person to type it in Settings -> Bluetooth: you cannot and must not take the digits. Keyboards, mice, game pads and phones can be paired; headphones and speakers cannot yet (say so).";

pub const SCAN: &str = "bluetooth_scan";
pub const PAIR: &str = "bluetooth_pair";
pub const PAIR_STATUS: &str = "bluetooth_pair_status";
pub const PAIR_ANSWER: &str = "bluetooth_pair_answer";
pub const WRITE_TOOLS: [&str; 3] = [SCAN, PAIR, PAIR_ANSWER];

pub fn is_pair_tool(n: &str) -> bool {
    n == PAIR_STATUS || WRITE_TOOLS.contains(&n)
}

/// What the tools need of the Bluetooth hardware (the real one is [`RealPairing`]; tests use a fake).
pub trait PairControl: Send + Sync {
    fn scan(&self, secs: u64) -> Result<Vec<Found>, String>;
    fn start(&self, found: &Found) -> Result<(), String>;
    fn state(&self) -> PairState;
    fn answer(&self, yes: bool) -> bool;
}

/// This computer's first powered Bluetooth controller.
pub struct RealPairing;

impl PairControl for RealPairing {
    fn scan(&self, secs: u64) -> Result<Vec<Found>, String> {
        let sys = iyakki_devctl::System;
        let ctrls = sys.bt_controllers()?;
        let c = ctrls.iter().find(|c| c.configured && c.powered()).ok_or_else(|| "Bluetooth is off or there is no controller: switch it on in Settings -> Bluetooth first".to_string())?;
        sys.bt_scan(c.index, secs)
    }
    fn start(&self, found: &Found) -> Result<(), String> {
        iyakki_devctl::System.bt_pair(found)
    }
    fn state(&self) -> PairState {
        iyakki_devctl::System.bt_pair_state()
    }
    fn answer(&self, yes: bool) -> bool {
        iyakki_devctl::System.bt_pair_answer(yes)
    }
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Keyboard => "keyboard",
        Kind::Mouse => "mouse",
        Kind::Gamepad => "game pad",
        Kind::Headphones => "headphones",
        Kind::Speaker => "speaker",
        Kind::Phone => "phone",
        Kind::Computer => "computer",
        Kind::Other => "other device",
    }
}

fn title(f: &Found) -> String {
    if f.name.is_empty() { f.address.clone() } else { f.name.clone() }
}

/// The scan result as lines: pairable and named devices first, strongest first; at most 15 lines.
pub fn scan_text(found: &[Found]) -> String {
    let mut v: Vec<&Found> = found.iter().collect();
    v.sort_by(|a, b| is_pairable_kind(b.kind).cmp(&is_pairable_kind(a.kind)).then(a.name.is_empty().cmp(&b.name.is_empty())).then(b.rssi.cmp(&a.rssi)));
    if v.is_empty() {
        return "No Bluetooth devices were found. The device must be switched on and in pairing mode (the person holds its pairing button).".into();
    }
    let mut out: Vec<String> = v
        .iter()
        .take(15)
        .map(|f| format!("{} · {} · {}{}", title(f), kind_name(f.kind), f.address, if is_pairable_kind(f.kind) { "" } else { " · cannot be paired in this version" }))
        .collect();
    if v.len() > 15 {
        out.push(format!("... and {} more", v.len() - 15));
    }
    out.join("\n")
}

/// The pairing state as an instruction for the assistant. A number the other device shows is asked for in Settings, never repeated.
pub fn state_text(s: &PairState) -> String {
    match s {
        PairState::Idle => "No pairing is running.".into(),
        PairState::Running(n) => format!("Pairing with {n} is in progress; look again with bluetooth_pair_status in a few seconds."),
        PairState::Confirm(n, num) => format!("{n} and this computer must show the same number: {num:06}. Ask the person whether {n} shows {num:06}, then call bluetooth_pair_answer with yes or no."),
        PairState::ConfirmNoNumber(n) => format!("{n} asks to pair and shows no number. Ask the person whether they want to pair it, then call bluetooth_pair_answer with yes or no."),
        PairState::ShowPasskey(n, num) => format!("Tell the person to type {num:06} on {n} (its own keys), then press Enter there. Then look again with bluetooth_pair_status."),
        PairState::ShowPin(n, pin) => format!("Tell the person to type {pin} on {n} (its own keys), then press Enter there. Then look again with bluetooth_pair_status."),
        PairState::TypePasskey(n) => format!("{n} shows a 6-digit number. Tell the person to type it in Settings -> Bluetooth, in the pairing window that is open there (the digits must not go through the chat). Then look again with bluetooth_pair_status."),
        PairState::TypePin(n, _) => format!("{n} needs a PIN. Tell the person to type it in Settings -> Bluetooth, in the pairing window that is open there (the PIN must not go through the chat). Then look again with bluetooth_pair_status."),
        PairState::Done(Ok(m)) => format!("Paired: {m}"),
        PairState::Done(Err(e)) => format!("Pairing failed: {e}"),
    }
}

/// Whether the state is one the assistant should report now instead of waiting longer.
fn settled(s: &PairState) -> bool {
    !matches!(s, PairState::Idle | PairState::Running(_))
}

pub struct BluetoothPairTools {
    inner: Arc<dyn Tools>,
    ctl: Arc<dyn PairControl>,
    last: Mutex<Vec<Found>>,
    wait: Duration,
    poll: Duration,
    scan_secs: u64,
}

impl BluetoothPairTools {
    pub fn new(inner: Arc<dyn Tools>, ctl: Arc<dyn PairControl>) -> BluetoothPairTools {
        BluetoothPairTools { inner, ctl, last: Mutex::new(Vec::new()), wait: Duration::from_secs(20), poll: Duration::from_millis(500), scan_secs: 8 }
    }

    pub fn real(inner: Arc<dyn Tools>) -> BluetoothPairTools {
        BluetoothPairTools::new(inner, Arc::new(RealPairing))
    }

    /// Shorter waits (tests).
    pub fn with_wait(mut self, wait: Duration, poll: Duration) -> BluetoothPairTools {
        self.wait = wait;
        self.poll = poll;
        self
    }

    /// The device of the last scan that the name or address means.
    fn find(&self, args: &Value) -> Result<Found, String> {
        let who = args.get("device").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if who.is_empty() {
            return Err("name the device (its address or its name; bluetooth_scan shows them)".into());
        }
        let list = self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        if list.is_empty() {
            return Err("scan first: call bluetooth_scan, then pair one of the devices it lists".into());
        }
        let f = if let Some(f) = list.iter().find(|f| f.address.eq_ignore_ascii_case(&who)) {
            f.clone()
        } else {
            let hits: Vec<&Found> = list.iter().filter(|f| f.name.eq_ignore_ascii_case(&who)).collect();
            match hits.len() {
                1 => hits[0].clone(),
                0 => return Err("the last scan did not find a device with that name (bluetooth_scan shows them)".into()),
                _ => return Err("more than one device has that name: use its address".into()),
            }
        };
        if !is_pairable_kind(f.kind) {
            return Err(format!("“{}” is a {}: this version pairs keyboards, mice, game pads and phones only", title(&f), kind_name(f.kind)));
        }
        Ok(f)
    }

    fn card(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            SCAN => Ok(format!("Scan for nearby Bluetooth devices for about {} seconds. Nothing is paired or changed.", self.scan_secs)),
            PAIR => {
                let f = self.find(args)?;
                Ok(format!("Pair this computer with the Bluetooth {} “{}” ({}). The person must be at the device: it may show a number to compare, or ask for a PIN. After pairing it can be used and is remembered.", kind_name(f.kind), title(&f), f.address))
            }
            PAIR_ANSWER => {
                let yes = args.get("yes").and_then(Value::as_bool).ok_or_else(|| "say yes (true) or no (false)".to_string())?;
                let s = self.ctl.state();
                if !matches!(s, PairState::Confirm(..) | PairState::ConfirmNoNumber(_)) {
                    return Err("no pairing waits for a yes or no (bluetooth_pair_status shows where it is)".into());
                }
                Ok(if yes { "Answer YES: the number on the other device is the same, pair them." } else { "Answer NO: do not pair; the pairing is cancelled." }.to_string())
            }
            _ => Err("not a pairing tool".into()),
        }
    }

    fn wait_for_person_or_end(&self) -> PairState {
        let until = Instant::now() + self.wait;
        loop {
            let s = self.ctl.state();
            if settled(&s) || Instant::now() >= until {
                return s;
            }
            std::thread::sleep(self.poll);
        }
    }
}

impl Tools for BluetoothPairTools {

    fn inner_tools(&self) -> Option<&dyn Tools> {

        Some(&*self.inner)

    }
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_pair_tool(&d.spec.name));
        let none = |desc: &str| json!({"type": "object", "properties": {}, "description": desc});
        let mk = |name: &str, desc: &str, schema: Value, risk: Risk| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema }, risk, untrusted: false };
        v.push(mk(SCAN, "Scan for nearby Bluetooth devices for about 8 seconds and list them (keyboards, mice, game pads and phones can be paired). The person approves a card.", none("Scan for nearby Bluetooth devices."), Risk::Ask));
        v.push(mk(
            PAIR,
            "Start pairing with a device the last bluetooth_scan found. The person must be at the device. Waits up to 20 seconds and tells what happens next (a number to compare, a number to type in Settings, the result). The person approves a card.",
            json!({"type": "object", "properties": {"device": {"type": "string", "description": "The device's address or name from the last scan."}}, "required": ["device"]}),
            Risk::Ask,
        ));
        v.push(mk(PAIR_STATUS, "Say where the running pairing is and what the person has to do next.", none("Pairing status."), Risk::Safe));
        v.push(mk(
            PAIR_ANSWER,
            "Answer the number comparison of a pairing: yes when the person has checked that both screens show the same number (or wants a phone without a number paired), no otherwise. The person approves a card.",
            json!({"type": "object", "properties": {"yes": {"type": "boolean"}}, "required": ["yes"]}),
            Risk::Ask,
        ));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            SCAN => {
                let found = self.ctl.scan(self.scan_secs)?;
                let text = scan_text(&found);
                *self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = found;
                Ok(text)
            }
            PAIR => {
                let f = self.find(args)?;
                self.ctl.start(&f)?;
                let s = self.wait_for_person_or_end();
                match &s {
                    PairState::Done(Err(e)) => Err(format!("pairing with {} failed: {e}", title(&f))),
                    PairState::Idle | PairState::Running(_) => Ok(format!("Still pairing with {}; look again with bluetooth_pair_status in a few seconds.", title(&f))),
                    _ => Ok(state_text(&s)),
                }
            }
            PAIR_STATUS => Ok(state_text(&self.ctl.state())),
            PAIR_ANSWER => {
                let yes = args.get("yes").and_then(Value::as_bool).ok_or_else(|| "say yes (true) or no (false)".to_string())?;
                if !self.ctl.answer(yes) {
                    return Err("no pairing waits for a yes or no (bluetooth_pair_status shows where it is)".into());
                }
                if !yes {
                    return Ok("Answered no: the pairing is cancelled.".into());
                }
                let s = self.wait_for_person_or_end();
                match &s {
                    PairState::Done(Err(e)) => Err(format!("pairing failed: {e}")),
                    PairState::Idle | PairState::Running(_) => Ok("Answered yes; the pairing is still finishing: look again with bluetooth_pair_status.".into()),
                    _ => Ok(state_text(&s)),
                }
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if WRITE_TOOLS.contains(&name) {
            return match self.card(name, args) {
                Ok(c) => c.split('.').next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            };
        }
        if name == PAIR_STATUS {
            return "Show the Bluetooth pairing state".into();
        }
        self.inner.describe(name, args)
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).ok();
        }
        if name == PAIR_STATUS { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).err();
        }
        if name == PAIR_STATUS { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_pair_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !WRITE_TOOLS.contains(&name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == PAIR || name == PAIR_ANSWER { Some("pairing a Bluetooth device needs the person at the device, so it asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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
}

#[cfg(test)]
#[path = "bluetoothpair_tests.rs"]
mod tests;
