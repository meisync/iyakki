//! The assistant's tools for paired Bluetooth keyboards, mice and game pads (F-NET-14): `bluetooth_list` reads (free);
//! `bluetooth_connect`, `bluetooth_disconnect` and `bluetooth_forget` each ask on a card, and forgetting asks every time (it removes the
//! keys, so the device must be paired again). Pairing needs the person at the device (a number to compare, a PIN to type), so it is done
//! in Settings -> Bluetooth, not by the assistant. Sending sound to headphones (A2DP source) is not built; a phone playing through this computer is `btaudiotools`.
//!
//! A paired phone is not an input device: `bluetooth_tether_status` (free), `bluetooth_tether_connect` and `bluetooth_tether_disconnect`
//! (each asks) use its Bluetooth tethering as this computer's internet ([`iyakki_devctl::bt_pan`]); a failure comes back with its plain reason
//! (tethering is off on the phone, the phone refused, no controller ...).
//!
//! The work is [`iyakki_devctl::bthid::Devices`]; this file only finds the device, writes the card and waits for the result.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use iyakki_devctl::bt_pan::{Tether, TetherState};
use iyakki_devctl::bthid::{find_device, DeviceView, Devices, LinkState};

pub const PROMPT_RULE: &str = "\nBluetooth: bluetooth_list shows the paired keyboards, mice and game pads and whether each is connected (and, when a connection failed, why). To connect, disconnect or forget one, call bluetooth_connect, bluetooth_disconnect or bluetooth_forget: each asks the person. Pairing a new device needs the person at the device: send them to Settings -> Bluetooth. Sending sound from this computer to Bluetooth headphones or speakers is not available yet: say so plainly (a phone playing through this computer is available, see the Bluetooth audio tools). A paired phone shares its internet with bluetooth_tether_connect (asks; Bluetooth tethering must be switched on in the phone, the person does that) and bluetooth_tether_disconnect (asks); bluetooth_tether_status shows whether it is up and, if not, why. Phone data may cost the person money: say that the traffic goes through the phone.";

pub const WRITE_TOOLS: [&str; 5] = ["bluetooth_connect", "bluetooth_disconnect", "bluetooth_forget", "bluetooth_tether_connect", "bluetooth_tether_disconnect"];
pub const READ_TOOL: &str = "bluetooth_list";
pub const TETHER_STATUS: &str = "bluetooth_tether_status";

pub fn is_bluetooth_tool(n: &str) -> bool {
    n == READ_TOOL || n == TETHER_STATUS || WRITE_TOOLS.contains(&n)
}

pub struct BluetoothTools {
    inner: Arc<dyn Tools>,
    devs: Arc<dyn Devices>,
    tether: Arc<dyn Tether>,
    /// How long `bluetooth_connect` waits for the result, and how often it looks.
    wait: Duration,
    poll: Duration,
}

impl BluetoothTools {
    pub fn new(inner: Arc<dyn Tools>, devs: Arc<dyn Devices>) -> BluetoothTools {
        BluetoothTools { inner, devs, tether: iyakki_devctl::bt_pan::tether(), wait: Duration::from_secs(20), poll: Duration::from_millis(500) }
    }

    /// The tools over this computer's real Bluetooth hub.
    pub fn real(inner: Arc<dyn Tools>) -> BluetoothTools {
        BluetoothTools::new(inner, Arc::new(iyakki_devctl::bthid_sys::RealDevices))
    }

    /// Another phone-tethering manager (tests use a fake).
    pub fn with_tether(mut self, t: Arc<dyn Tether>) -> BluetoothTools {
        self.tether = t;
        self
    }

    /// The phone a tether tool means: the one named, else the one that is connected, else the only paired phone.
    fn phone(&self, args: &Value) -> Result<DeviceView, String> {
        let list: Vec<DeviceView> = self.devs.list().into_iter().filter(|d| d.kind == "phone").collect();
        let who = args.get("phone").and_then(Value::as_str).unwrap_or("").trim();
        if !who.is_empty() {
            return find_device(&list, who).map_err(|e| e.replace("device", "phone"));
        }
        match list.len() {
            0 => Err("no phone is paired: the person pairs it in Settings -> Bluetooth (Bluetooth must be on in the phone) first".into()),
            1 => Ok(list[0].clone()),
            _ => Err("more than one phone is paired: name the one to use (bluetooth_list shows them)".into()),
        }
    }

    fn tether_status(&self) -> String {
        let v = self.tether.view();
        let who = if v.name.is_empty() { v.address.clone() } else { v.name.clone() };
        let phones: Vec<String> = self.devs.list().into_iter().filter(|d| d.kind == "phone").map(|d| d.title()).collect();
        let state = match &v.state {
            TetherState::Idle => "not connected".to_string(),
            TetherState::Connecting => format!("connecting to {who} ..."),
            TetherState::Connected => format!("connected: this computer uses the internet of {who} over Bluetooth tethering (the traffic counts against the phone's data plan)"),
            TetherState::Failed(why) => format!("not connected, the last try failed: {why}"),
        };
        format!("Bluetooth tethering: {state}. Paired phones: {}.", if phones.is_empty() { "none (the person pairs one in Settings -> Bluetooth)".to_string() } else { phones.join(", ") })
    }

    /// Shorter waits (tests).
    pub fn with_wait(mut self, wait: Duration, poll: Duration) -> BluetoothTools {
        self.wait = wait;
        self.poll = poll;
        self
    }

    fn find(&self, args: &Value) -> Result<DeviceView, String> {
        find_device(&self.devs.list(), args.get("device").and_then(Value::as_str).unwrap_or(""))
    }

    fn card(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            "bluetooth_tether_connect" => {
                let p = self.phone(args)?;
                return Ok(format!("Use the internet of the phone “{}” ({}) over Bluetooth tethering: this computer's traffic goes through the phone and counts against its data plan, until it is disconnected. Bluetooth tethering must be switched on in the phone.", p.title(), p.address));
            }
            "bluetooth_tether_disconnect" => {
                let v = self.tether.view();
                return Ok(format!("Stop using the phone “{}” as this computer's internet: the Bluetooth tethering link is closed. The phone stays paired.", if v.name.is_empty() { v.address } else { v.name }));
            }
            _ => {}
        }
        let d = self.find(args)?;
        let who = format!("“{}” ({}, {})", d.title(), d.kind, d.address);
        match name {
            "bluetooth_connect" => Ok(format!("Connect the Bluetooth {who}: it becomes a {} of this computer until it is disconnected or switched off.", d.kind)),
            "bluetooth_disconnect" => Ok(format!("Disconnect the Bluetooth {who}: it stops working until it connects again. It stays paired.")),
            "bluetooth_forget" => Ok(format!("FORGET the Bluetooth {who}: the pairing and its keys are removed, so it must be paired again in Settings -> Bluetooth.")),
            _ => Err("not a Bluetooth tool".into()),
        }
    }
}

fn one_line(d: &DeviceView) -> String {
    format!("{} · {}", d.title(), d.line())
}

impl Tools for BluetoothTools {
    fn defs(&self) -> Vec<ToolDef> {
        let mut v = self.inner.defs();
        v.retain(|d| !is_bluetooth_tool(&d.spec.name));
        let dev = |desc: &str| json!({"type": "object", "properties": {"device": {"type": "string", "description": "The paired device's address or name (bluetooth_list shows them)."}}, "required": ["device"], "description": desc});
        let ask = |name: &str, desc: &str| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema: dev(desc) }, risk: Risk::Ask, untrusted: false };
        v.push(ToolDef { spec: ToolSpec { name: READ_TOOL.into(), description: "List the paired Bluetooth keyboards, mice and game pads with their connection state and, if a connection failed, the reason.".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ask("bluetooth_connect", "Connect a paired Bluetooth keyboard, mouse or game pad. Waits up to 20 seconds and reports the result or the reason. The person approves a card."));
        v.push(ask("bluetooth_disconnect", "Disconnect a Bluetooth keyboard, mouse or game pad; it stays paired. The person approves a card."));
        let phone = |desc: &str| json!({"type": "object", "properties": {"phone": {"type": "string", "description": "The paired phone's name or address; leave out when only one phone is paired."}}, "description": desc});
        let ask_phone = |name: &str, desc: &str| ToolDef { spec: ToolSpec { name: name.into(), description: desc.into(), schema: phone(desc) }, risk: Risk::Ask, untrusted: false };
        v.push(ToolDef { spec: ToolSpec { name: TETHER_STATUS.into(), description: "Say whether this computer is using a phone's Bluetooth tethering as its internet, and if not, why the last try failed (tethering off on the phone, phone refused, no controller ...).".into(), schema: json!({"type": "object", "properties": {}}) }, risk: Risk::Safe, untrusted: false });
        v.push(ask_phone("bluetooth_tether_connect", "Use a paired phone's Bluetooth tethering as this computer's internet. Waits up to 20 seconds and reports the result or the plain reason. Tethering must be on in the phone. The person approves a card."));
        v.push(ask_phone("bluetooth_tether_disconnect", "Stop using the phone's Bluetooth tethering as the internet; the phone stays paired. The person approves a card."));
        v.push(ask("bluetooth_forget", "Forget a paired Bluetooth device: its keys are removed and it must be paired again. The person approves a card every time."));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => {
                let l = self.devs.list();
                if l.is_empty() {
                    return Ok("No paired Bluetooth devices. Keyboards, mice and game pads (classic and low energy) and phones are paired in Settings -> Bluetooth; sending sound to Bluetooth headphones is not available yet.".into());
                }
                Ok(l.iter().map(one_line).collect::<Vec<_>>().join("\n"))
            }
            TETHER_STATUS => Ok(self.tether_status()),
            "bluetooth_tether_connect" => {
                let p = self.phone(args)?;
                self.tether.connect(&p.address)?;
                let until = Instant::now() + self.wait;
                loop {
                    let v = self.tether.view();
                    match v.state {
                        TetherState::Connected => return Ok(format!("Connected: this computer now uses the internet of {} over Bluetooth tethering. The network address arrives within a few seconds; bluetooth_tether_status and the Network page show it.", p.title())),
                        TetherState::Failed(why) => return Err(format!("{} did not connect: {why}", p.title())),
                        TetherState::Idle => return Err(format!("{} did not connect (the link was closed).", p.title())),
                        TetherState::Connecting => {}
                    }
                    if Instant::now() >= until {
                        return Ok(format!("Still connecting to {}; look again with bluetooth_tether_status in a few seconds.", p.title()));
                    }
                    std::thread::sleep(self.poll);
                }
            }
            "bluetooth_tether_disconnect" => {
                self.tether.disconnect()?;
                Ok("Disconnected the phone's internet; it is still paired.".into())
            }
            "bluetooth_connect" => {
                let d = self.find(args)?;
                self.devs.connect(&d.address)?;
                let until = Instant::now() + self.wait;
                loop {
                    let now = self.devs.list().into_iter().find(|x| x.address == d.address);
                    match now.map(|x| x.state) {
                        Some(LinkState::Connected) => return Ok(format!("Connected {}: it works as a {} now.", d.title(), d.kind)),
                        Some(LinkState::Failed(why)) => return Err(format!("{} did not connect: {why}", d.title())),
                        None => return Err("the device is gone from the paired list".into()),
                        _ => {}
                    }
                    if Instant::now() >= until {
                        return Ok(format!("Still connecting to {}; look again with bluetooth_list in a few seconds.", d.title()));
                    }
                    std::thread::sleep(self.poll);
                }
            }
            "bluetooth_disconnect" => {
                let d = self.find(args)?;
                self.devs.disconnect(&d.address)?;
                Ok(format!("Disconnected {}; it is still paired.", d.title()))
            }
            "bluetooth_forget" => {
                let d = self.find(args)?;
                self.devs.forget(&d.address)?;
                Ok(format!("Forgot {}; pair it again in Settings -> Bluetooth to use it.", d.title()))
            }
            _ => self.inner.call(name, args),
        }
    }

    fn describe(&self, name: &str, args: &Value) -> String {
        if WRITE_TOOLS.contains(&name) {
            return match self.card(name, args) {
                Ok(c) => c.split(':').next().unwrap_or("").to_string(),
                Err(e) => format!("{name} (invalid: {e})"),
            };
        }
        if name == READ_TOOL {
            return "List the paired Bluetooth devices".into();
        }
        if name == TETHER_STATUS {
            return "Show the Bluetooth tethering state".into();
        }
        self.inner.describe(name, args)
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).ok();
        }
        if name == READ_TOOL || name == TETHER_STATUS { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).err();
        }
        if name == READ_TOOL || name == TETHER_STATUS { None } else { self.inner.refusal(name, args) }
    }

    fn pre_approved(&self, name: &str) -> bool {
        !is_bluetooth_tool(name) && self.inner.pre_approved(name)
    }

    fn approved_always(&self, name: &str) -> bool {
        !WRITE_TOOLS.contains(&name) && self.inner.approved_always(name)
    }

    fn grant_blocker(&self, name: &str, args: &Value) -> Option<String> {
        if name == "bluetooth_forget" { Some("forgetting a Bluetooth device asks every time".into()) } else { self.inner.grant_blocker(name, args) }
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
