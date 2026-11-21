//! The fake [`Hardware`]: lists in memory, every change recorded and visible in the next read, nothing touches the machine.

use iyakki_core_api::hardware::{btstate, mac, Card, Hardware, Link, Rfkill, Screen, Volume};
use std::sync::Mutex;

pub struct State {
    pub links: Vec<Link>,
    pub ipv4: Vec<(String, String)>,
    pub rfkill: Vec<Rfkill>,
    pub broadcom_no_driver: bool,
    pub screen: Screen,
    pub cards: Vec<(Card, Option<Volume>)>,
    pub mac_infos: Vec<mac::MacInfo>,
    pub bluetooth: btstate::BtState,
    pub cs8409: Option<String>,
    pub mac_summary: Option<String>,
    pub hostname: String,
    pub report: Vec<String>,
    pub calls: Vec<String>,
}

pub struct FakeHardware {
    state: Mutex<State>,
}

impl Default for FakeHardware {
    fn default() -> FakeHardware {
        FakeHardware {
            state: Mutex::new(State {
                links: vec![],
                ipv4: vec![],
                rfkill: vec![],
                broadcom_no_driver: false,
                screen: Screen { framebuffer: None, outputs: vec![], backlights: vec![] },
                cards: vec![],
                mac_infos: vec![],
                bluetooth: btstate::BtState { adapters: vec![], usb_present: false, switches: vec![], unstarted: vec![], controllers: Ok(vec![]), product: String::new(), fw_file: false, held: false, needs_firmware: false },
                cs8409: None,
                mac_summary: None,
                hostname: String::new(),
                report: vec![],
                calls: vec![],
            }),
        }
    }
}

impl FakeHardware {
    pub fn new() -> FakeHardware {
        FakeHardware::default()
    }
    /// Change what the machine looks like.
    pub fn set(&self, f: impl FnOnce(&mut State)) {
        f(&mut self.state.lock().unwrap());
    }
    /// Every change so far, as `method(arguments)`.
    pub fn calls(&self) -> Vec<String> {
        self.state.lock().unwrap().calls.clone()
    }
}

impl Hardware for FakeHardware {
    fn links(&self) -> Vec<Link> {
        self.state.lock().unwrap().links.clone()
    }
    fn ipv4(&self) -> Vec<(String, String)> {
        self.state.lock().unwrap().ipv4.clone()
    }
    fn rfkill(&self) -> Vec<Rfkill> {
        self.state.lock().unwrap().rfkill.clone()
    }
    fn set_rfkill(&self, id: &str, blocked: bool) -> Result<(), String> {
        let mut s = self.state.lock().unwrap();
        s.calls.push(format!("set_rfkill({id},{blocked})"));
        let k = s.rfkill.iter_mut().find(|k| k.id == id).ok_or_else(|| format!("no radio switch {id}"))?;
        k.soft = blocked;
        Ok(())
    }
    fn broadcom_wifi_without_driver(&self) -> bool {
        self.state.lock().unwrap().broadcom_no_driver
    }
    fn screen(&self) -> Screen {
        self.state.lock().unwrap().screen.clone()
    }
    fn set_brightness(&self, name: &str, percent: u32) -> Result<(), String> {
        let mut s = self.state.lock().unwrap();
        s.calls.push(format!("set_brightness({name},{percent})"));
        let b = s.screen.backlights.iter_mut().find(|b| b.name == name).ok_or_else(|| format!("no backlight {name}"))?;
        b.value = (b.max * percent.min(100) + 50) / 100;
        Ok(())
    }
    fn cards(&self) -> Vec<(Card, Option<Volume>)> {
        self.state.lock().unwrap().cards.clone()
    }
    fn mac_infos(&self, _: &mac::MacConfig) -> Vec<mac::MacInfo> {
        self.state.lock().unwrap().mac_infos.clone()
    }
    fn bluetooth(&self) -> btstate::BtState {
        self.state.lock().unwrap().bluetooth.clone()
    }
    fn cs8409_line(&self) -> Option<String> {
        self.state.lock().unwrap().cs8409.clone()
    }
    fn set_hostname(&self, name: &str, _etc_hostname: &std::path::Path) -> Result<(), String> {
        let mut s = self.state.lock().unwrap();
        s.calls.push(format!("set_hostname({name})"));
        if !iyakki_core_api::hardware::hostname_ok(name) {
            return Err(format!("\"{name}\" is not a host name: use lower-case letters, digits and '-'"));
        }
        s.hostname = name.to_string();
        Ok(())
    }
    fn mac_summary(&self) -> Option<String> {
        self.state.lock().unwrap().mac_summary.clone()
    }
    fn hardware_report(&self) -> Vec<String> {
        self.state.lock().unwrap().report.clone()
    }
    fn volume(&self, card: u32) -> Option<Volume> {
        self.state.lock().unwrap().cards.iter().find(|(c, _)| c.index == card).and_then(|(_, v)| v.clone())
    }
    fn set_volume(&self, card: u32, percent: u32) -> Result<Volume, String> {
        let mut s = self.state.lock().unwrap();
        s.calls.push(format!("set_volume({card},{percent})"));
        let v = s.cards.iter_mut().find(|(c, _)| c.index == card).and_then(|(_, v)| v.as_mut()).ok_or_else(|| format!("sound card {card} has no volume control"))?;
        v.percent = percent.min(100);
        v.muted = false;
        Ok(v.clone())
    }
    fn change_volume(&self, card: u32, delta: Option<i32>) -> Result<Volume, String> {
        let mut s = self.state.lock().unwrap();
        s.calls.push(format!("change_volume({card},{delta:?})"));
        let v = s.cards.iter_mut().find(|(c, _)| c.index == card).and_then(|(_, v)| v.as_mut()).ok_or_else(|| format!("sound card {card} has no volume control"))?;
        match delta {
            Some(d) => v.percent = (v.percent as i64 + d as i64).clamp(0, 100) as u32,
            None => v.muted = !v.muted,
        }
        Ok(v.clone())
    }
    fn set_link(&self, name: &str, up: bool) -> Result<(), String> {
        let mut s = self.state.lock().unwrap();
        s.calls.push(format!("set_link({name},{up})"));
        let l = s.links.iter_mut().find(|l| l.name == name).ok_or_else(|| format!("no network connection {name}"))?;
        l.up = up;
        Ok(())
    }
    fn dhcp(&self, name: &str) -> Result<(), String> {
        let mut s = self.state.lock().unwrap();
        s.calls.push(format!("dhcp({name})"));
        if s.links.iter().any(|l| l.name == name && l.up) { Ok(()) } else { Err(format!("{name} is not up")) }
    }
    fn request_class(&self, class: &str, who: &str) -> String {
        self.state.lock().unwrap().calls.push(format!("request_class({class},{who})"));
        "nothing to load: the drivers are loaded already".into()
    }
}
