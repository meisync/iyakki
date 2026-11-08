//! The assistant's tools for paired Bluetooth keyboards, mice and game pads (F-NET-14): `bluetooth_list` reads (free);
//! `bluetooth_connect`, `bluetooth_disconnect` and `bluetooth_forget` each ask on a card, and forgetting asks every time (it removes the
//! keys, so the device must be paired again). Pairing needs the person at the device (a number to compare, a PIN to type), so it is done
//! in Settings -> Bluetooth, not by the assistant. No audio (A2DP) yet; the tools say so.
//!
//! The work is [`iyakki_devctl::bthid::Devices`]; this file only finds the device, writes the card and waits for the result.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::agent::{Risk, ToolDef, Tools};
use crate::chat::ToolSpec;
use iyakki_devctl::bthid::{find_device, DeviceView, Devices, LinkState};

pub const PROMPT_RULE: &str = "\nBluetooth: bluetooth_list shows the paired keyboards, mice and game pads and whether each is connected (and, when a connection failed, why). To connect, disconnect or forget one, call bluetooth_connect, bluetooth_disconnect or bluetooth_forget: each asks the person. Pairing a new device needs the person at the device: send them to Settings -> Bluetooth. Bluetooth audio (headphones, speakers) is not available yet: say so plainly.";

pub const WRITE_TOOLS: [&str; 3] = ["bluetooth_connect", "bluetooth_disconnect", "bluetooth_forget"];
pub const READ_TOOL: &str = "bluetooth_list";

pub fn is_bluetooth_tool(n: &str) -> bool {
    n == READ_TOOL || WRITE_TOOLS.contains(&n)
}

pub struct BluetoothTools {
    inner: Arc<dyn Tools>,
    devs: Arc<dyn Devices>,
    /// How long `bluetooth_connect` waits for the result, and how often it looks.
    wait: Duration,
    poll: Duration,
}

impl BluetoothTools {
    pub fn new(inner: Arc<dyn Tools>, devs: Arc<dyn Devices>) -> BluetoothTools {
        BluetoothTools { inner, devs, wait: Duration::from_secs(20), poll: Duration::from_millis(500) }
    }

    /// The tools over this computer's real Bluetooth hub.
    pub fn real(inner: Arc<dyn Tools>) -> BluetoothTools {
        BluetoothTools::new(inner, Arc::new(iyakki_devctl::bthid_sys::RealDevices))
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
        v.push(ask("bluetooth_forget", "Forget a paired Bluetooth device: its keys are removed and it must be paired again. The person approves a card every time."));
        v
    }

    fn call(&self, name: &str, args: &Value) -> Result<String, String> {
        match name {
            READ_TOOL => {
                let l = self.devs.list();
                if l.is_empty() {
                    return Ok("No paired Bluetooth devices. Keyboards, mice and game pads (classic Bluetooth) are paired in Settings -> Bluetooth; Bluetooth audio is not available yet.".into());
                }
                Ok(l.iter().map(one_line).collect::<Vec<_>>().join("\n"))
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
        self.inner.describe(name, args)
    }

    fn preview(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).ok();
        }
        if name == READ_TOOL { None } else { self.inner.preview(name, args) }
    }

    fn refusal(&self, name: &str, args: &Value) -> Option<String> {
        if WRITE_TOOLS.contains(&name) {
            return self.card(name, args).err();
        }
        if name == READ_TOOL { None } else { self.inner.refusal(name, args) }
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
