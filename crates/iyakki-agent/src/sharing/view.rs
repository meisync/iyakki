//! What the Settings page and `iyakki share list` show: the machine's links as two lists with a plain state for each.

use super::plan::free_for_sharing;
use super::{IfaceInfo, LinkKind, ShareError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub name: String,
    /// `Ethernet enp3s0 - cable connected`
    pub label: String,
    pub usable: bool,
    /// Why it cannot be chosen (empty when it can).
    pub why: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choices {
    /// Links that have a route to the internet.
    pub uplinks: Vec<Choice>,
    /// Links that could carry the shared internet.
    pub downs: Vec<Choice>,
    /// Shown under the lists when no Wi-Fi card can be an access point on this build.
    pub wifi_line: Option<String>,
}

fn state(i: &IfaceInfo) -> &'static str {
    if !i.up {
        "down"
    } else if i.carrier {
        "cable connected"
    } else {
        "no cable"
    }
}

pub fn choices(ifs: &[IfaceInfo], wifi_ap: bool) -> Choices {
    let mut c = Choices::default();
    for i in ifs {
        if i.default_route && i.up {
            c.uplinks.push(Choice { name: i.name.clone(), label: format!("{} {} - has internet", i.kind.label(), i.name), usable: true, why: String::new() });
        }
        match i.kind {
            // a Wi-Fi interface is a client; the hotspot is one entry of its own below
            LinkKind::Wifi => {}
            LinkKind::Other => {}
            // the phone's tethering is only ever a source of internet, never a cable to give it to
            LinkKind::BluetoothPanu => {}
            _ => {
                let r = free_for_sharing(i);
                let label = format!("{} {} - {}", i.kind.label(), i.name, if r.is_err() { "in use".to_string() } else { state(i).to_string() });
                let why = match &r {
                    Ok(()) => String::new(),
                    Err(ShareError::Busy(_)) => "it carries this computer's own network".into(),
                    Err(e) => e.line(),
                };
                c.downs.push(Choice { name: i.name.clone(), label, usable: r.is_ok(), why });
            }
        }
    }
    if wifi_ap {
        c.downs.push(Choice { name: crate::wifi_ap::AP_IFACE.into(), label: "Wi-Fi hotspot - this computer's Wi-Fi card makes its own network".into(), usable: true, why: String::new() });
    }
    // Bluetooth: a connected phone is an uplink above; this entry says what is missing, and sharing ours (NAP) is not built yet
    let phone_is_up = ifs.iter().any(|i| i.kind == LinkKind::BluetoothPanu && i.default_route && i.up);
    for (kind, name, up) in [(LinkKind::BluetoothPanu, "bluetooth-panu", true), (LinkKind::BluetoothNap, "bluetooth-nap", false)] {
        if phone_is_up && kind == LinkKind::BluetoothPanu {
            continue; // the connected phone is listed above like any uplink
        }
        let ch = Choice { name: name.into(), label: kind.label().into(), usable: false, why: bluetooth_why(kind) };
        if up {
            c.uplinks.push(ch);
        } else {
            c.downs.push(ch);
        }
    }
    if !wifi_ap {
        c.wifi_line = Some("Wi-Fi hotspot: not available on this build".into());
    }
    c
}

/// The three lines of warning the page shows while sharing is on or about to start.
pub fn warnings(isolate: bool, idle_hours: u8) -> Vec<String> {
    vec![
        "This computer will give other computers access to your internet connection. Everything they download counts as yours.".into(),
        if isolate { "Client isolation is on: the devices cannot reach each other, and they cannot open anything on this computer except to ask for an address and for names.".to_string() } else { "Client isolation is OFF: the devices on the cable can reach each other.".to_string() },
        format!("It switches itself off after {idle_hours} h with nobody using it. Turn it off when you do not need it."),
    ]
}

/// Why a Bluetooth entry cannot be chosen right now, in one line (empty: it can).
fn bluetooth_why(kind: LinkKind) -> String {
    match kind {
        // the phone side: a controller with an address is all it needs (the phone is chosen in Settings -> Bluetooth)
        LinkKind::BluetoothPanu => match crate::bt_pan::controller_state(&iyakki_devctl::Roots::real()) {
            Err(why) => why,
            Ok(_) => "Not connected yet: Settings -> Bluetooth, choose the paired phone and press Connect under Tether.".into(),
        },
        _ => "Letting phones connect to this computer is not built yet (use the Wi-Fi hotspot).".into(),
    }
}
