//! The pure parts of the WiFi things [`crate::wifi`] does not cover yet (F-NET-07): the supplicant's configuration for a WPA
//! enterprise network and for a WEP network, finding out whether a network has a sign-in page (captive portal), the live signal
//! of the network the machine is joined to, and the choice between two WiFi adapters. Nothing here runs a program or opens a
//! socket; the text of `iw` and of an HTTP answer is passed in.
// SPDX-License-Identifier: Apache-2.0

use crate::wifi::{valid_ssid, Ap, Security};

// ------------------------------------------------------------------------------------------------ enterprise

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EapMethod {
    /// Protected EAP with a user name and password inside (the usual school and company network).
    Peap,
    /// Tunnelled TLS with a user name and password inside.
    Ttls,
    /// A certificate of the person's own, no password.
    Tls,
}

impl EapMethod {
    pub fn label(self) -> &'static str {
        match self {
            EapMethod::Peap => "PEAP",
            EapMethod::Ttls => "TTLS",
            EapMethod::Tls => "TLS",
        }
    }
}

/// What a person enters for a WPA enterprise network. Paths are files in the machine (certificates the person added).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Enterprise {
    pub method: EapMethod,
    pub identity: String,
    /// The outer name some networks want instead of the real one (sent in the clear).
    pub anonymous_identity: Option<String>,
    /// Not for TLS.
    pub password: Option<String>,
    /// The inner method for PEAP and TTLS: MSCHAPV2 (default), PAP or GTC.
    pub inner: Option<String>,
    /// The certificate authority file that checks the network's server. Required: without it anyone can pose as the network.
    pub ca_cert: String,
    /// Part of the server's name it must have (for example `radius.example.edu`); optional but advised.
    pub server_name_match: Option<String>,
    /// TLS only.
    pub client_cert: Option<String>,
    pub private_key: Option<String>,
    /// The password of the private key, if it has one.
    pub private_key_password: Option<String>,
}

fn plain_text_ok(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && !s.chars().any(|c| c == '\0' || c == '\n' || c == '\r')
}

fn path_ok(s: &str) -> bool {
    s.starts_with('/') && plain_text_ok(s, 255) && !s.chars().any(|c| c.is_control() || c == '"' || c == '\\') && !s.split('/').any(|p| p == "..")
}

fn host_ok(s: &str) -> bool {
    !s.is_empty() && s.len() <= 253 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
}

/// A string the supplicant's file takes without any quoting trouble: the hexadecimal form.
fn hex_str(s: &str) -> String {
    s.bytes().map(|b| format!("{b:02x}")).collect()
}

impl Enterprise {
    /// The reason this entry cannot be used, in plain words.
    pub fn check(&self) -> Result<(), String> {
        if !plain_text_ok(&self.identity, 128) {
            return Err("the user name must be 1 to 128 characters on one line".into());
        }
        if let Some(a) = &self.anonymous_identity {
            if !plain_text_ok(a, 128) {
                return Err("the outer (anonymous) name must be 1 to 128 characters on one line".into());
            }
        }
        if !path_ok(&self.ca_cert) {
            return Err("a certificate file to check the network is required (a full path, no quotes): without it anyone could pose as the network and read the password".into());
        }
        if let Some(h) = &self.server_name_match {
            if !host_ok(h) {
                return Err("the server name to check must be a host name such as radius.example.edu".into());
            }
        }
        match self.method {
            EapMethod::Tls => {
                if self.client_cert.as_deref().is_none_or(|p| !path_ok(p)) || self.private_key.as_deref().is_none_or(|p| !path_ok(p)) {
                    return Err("a certificate network needs your certificate file and its private key file (full paths)".into());
                }
                if self.password.is_some() {
                    return Err("a certificate network takes no password (only a password for the key file)".into());
                }
            }
            EapMethod::Peap | EapMethod::Ttls => {
                if self.password.as_deref().is_none_or(|p| !plain_text_ok(p, 256)) {
                    return Err("the password must be 1 to 256 characters on one line".into());
                }
                if let Some(i) = &self.inner {
                    if !["MSCHAPV2", "PAP", "GTC"].contains(&i.as_str()) {
                        return Err("the inner method must be MSCHAPV2, PAP or GTC".into());
                    }
                }
            }
        }
        if let Some(p) = &self.private_key_password {
            if !plain_text_ok(p, 256) {
                return Err("the key password must be 1 to 256 characters on one line".into());
            }
        }
        Ok(())
    }
}

/// One `network={...}` block of the supplicant's file for an enterprise network. Names and passwords are written in hexadecimal so no
/// character in them can end the line or the block.
pub fn enterprise_block(ssid: &str, hidden: bool, priority: usize, e: &Enterprise) -> Result<String, String> {
    if !valid_ssid(ssid) {
        return Err("the network name must be 1 to 32 characters without quotes or backslashes".into());
    }
    e.check()?;
    let mut s = format!("network={{\n    ssid=\"{ssid}\"\n    priority={priority}\n");
    if hidden {
        s.push_str("    scan_ssid=1\n");
    }
    s.push_str(&format!("    key_mgmt=WPA-EAP\n    eap={}\n    identity={}\n", e.method.label(), hex_str(&e.identity)));
    if let Some(a) = &e.anonymous_identity {
        s.push_str(&format!("    anonymous_identity={}\n", hex_str(a)));
    }
    if let Some(p) = &e.password {
        s.push_str(&format!("    password={}\n", hex_str(p)));
    }
    if e.method != EapMethod::Tls {
        s.push_str(&format!("    phase2=\"auth={}\"\n", e.inner.as_deref().unwrap_or("MSCHAPV2")));
    }
    s.push_str(&format!("    ca_cert=\"{}\"\n", e.ca_cert));
    if let Some(h) = &e.server_name_match {
        s.push_str(&format!("    domain_suffix_match=\"{h}\"\n"));
    }
    if let (EapMethod::Tls, Some(c), Some(k)) = (e.method, &e.client_cert, &e.private_key) {
        s.push_str(&format!("    client_cert=\"{c}\"\n    private_key=\"{k}\"\n"));
        if let Some(p) = &e.private_key_password {
            s.push_str(&format!("    private_key_passwd={}\n", hex_str(p)));
        }
    }
    s.push_str("    ieee80211w=1\n}\n");
    Ok(s)
}

// ------------------------------------------------------------------------------------------------ WEP

/// What is said before a WEP network is joined: it is not safe.
pub fn wep_warning() -> &'static str {
    "WEP is an old protection that can be broken in minutes: anyone nearby can read what this computer sends over this network. Join only if you must, and do not sign in to anything private over it."
}

/// A WEP key as typed: 5 or 13 characters, or 10 or 26 hexadecimal digits.
pub fn check_wep_key(k: &str) -> Result<(), String> {
    let hex = matches!(k.len(), 10 | 26) && k.chars().all(|c| c.is_ascii_hexdigit());
    let text = matches!(k.len(), 5 | 13) && k.is_ascii() && !k.chars().any(|c| c.is_control() || c == '"' || c == '\\');
    if hex || text { Ok(()) } else { Err("a WEP key is 5 or 13 characters, or 10 or 26 hexadecimal digits".into()) }
}

/// The block for a WEP network (the first of four keys, used to send).
pub fn wep_block(ssid: &str, hidden: bool, priority: usize, key: &str) -> Result<String, String> {
    if !valid_ssid(ssid) {
        return Err("the network name must be 1 to 32 characters without quotes or backslashes".into());
    }
    check_wep_key(key)?;
    let hex = matches!(key.len(), 10 | 26) && key.chars().all(|c| c.is_ascii_hexdigit());
    let k = if hex { key.to_ascii_lowercase() } else { format!("\"{key}\"") };
    Ok(format!("network={{\n    ssid=\"{ssid}\"\n    priority={priority}\n{}    key_mgmt=NONE\n    wep_key0={k}\n    wep_tx_keyidx=0\n}}\n", if hidden { "    scan_ssid=1\n" } else { "" }))
}

// ------------------------------------------------------------------------------------------------ captive portal

/// The address the probe asks, and what a free internet answers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Probe {
    /// `GET /generate_204`: free internet answers 204 and nothing else.
    NoContent,
    /// A small page whose body is exactly `Success` (the page some phones use).
    SuccessPage,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortalState {
    /// The probe got its expected answer: the network lets traffic out.
    Open,
    /// Something answered in place of the internet: the network wants a sign-in. The page to open, when the answer named one.
    SignIn(Option<String>),
    /// No answer was understood.
    Unknown(String),
}

/// A HTTP/1.1 request for the probe (no cookies, no body, connection closed after the answer).
pub fn probe_request(host: &str, path: &str) -> Result<Vec<u8>, String> {
    if !host_ok(host) || !path.starts_with('/') || path.chars().any(|c| c.is_control() || c == ' ') {
        return Err("not a probe address".into());
    }
    Ok(format!("GET {path} HTTP/1.1\r\nHost: {host}\r\nUser-Agent: iyakki-connectivity-check\r\nAccept: */*\r\nConnection: close\r\n\r\n").into_bytes())
}

/// The status, the Location header and the start of the body of an HTTP answer.
pub fn parse_http(d: &[u8]) -> Result<(u16, Option<String>, String), String> {
    let end = d.windows(4).position(|w| w == b"\r\n\r\n").ok_or("the answer has no end of headers")?;
    let head = String::from_utf8_lossy(&d[..end]).to_string();
    let mut lines = head.lines();
    let first = lines.next().ok_or("empty answer")?;
    let mut it = first.split_whitespace();
    if !it.next().is_some_and(|v| v.starts_with("HTTP/1.")) {
        return Err("not an HTTP answer".into());
    }
    let status: u16 = it.next().and_then(|s| s.parse().ok()).ok_or("no status")?;
    let location = lines.filter_map(|l| l.split_once(':')).find(|(k, _)| k.eq_ignore_ascii_case("location")).map(|(_, v)| v.trim().to_string());
    let body = String::from_utf8_lossy(&d[end + 4..d.len().min(end + 4 + 512)]).to_string();
    Ok((status, location, body))
}

/// A sign-in address that is safe to hand to the browser: http or https with a host, no control characters, not too long.
pub fn portal_url(u: &str) -> Option<String> {
    let u = u.trim();
    let rest = u.strip_prefix("http://").or_else(|| u.strip_prefix("https://"))?;
    let host = rest.split(['/', '?', '#']).next()?;
    if host.is_empty() || u.len() > 2000 || u.chars().any(|c| c.is_control() || c == ' ') || host.contains('@') {
        return None;
    }
    Some(u.to_string())
}

/// What the answer to the probe means.
pub fn classify_probe(probe: Probe, status: u16, location: Option<&str>, body: &str) -> PortalState {
    match (probe, status) {
        (Probe::NoContent, 204) => PortalState::Open,
        (Probe::SuccessPage, 200) if body.trim() == "Success" || body.contains("<BODY>Success</BODY>") => PortalState::Open,
        (_, 301 | 302 | 303 | 307 | 308) => PortalState::SignIn(location.and_then(portal_url)),
        // a page in place of the empty answer, or "network authentication required"
        (_, 200 | 511) => PortalState::SignIn(None),
        (_, s) => PortalState::Unknown(format!("the check got status {s}, which says nothing about a sign-in")),
    }
}

/// Ask the check address once over plain HTTP and say what the answer means. `addr` is `host:port`; the connection and the answer
/// each wait at most `timeout`; nothing is sent but the request of [`probe_request`].
pub fn probe_once(addr: &str, host: &str, path: &str, probe: Probe, timeout: std::time::Duration) -> PortalState {
    use std::io::{Read, Write};
    use std::net::ToSocketAddrs;
    let req = match probe_request(host, path) {
        Ok(r) => r,
        Err(e) => return PortalState::Unknown(e),
    };
    let Some(sa) = addr.to_socket_addrs().ok().and_then(|mut a| a.next()) else { return PortalState::Unknown("the check address could not be found".into()) };
    let mut s = match std::net::TcpStream::connect_timeout(&sa, timeout) {
        Ok(s) => s,
        Err(e) => return PortalState::Unknown(format!("no connection to the check address ({e})")),
    };
    let _ = s.set_read_timeout(Some(timeout));
    let _ = s.set_write_timeout(Some(timeout));
    if s.write_all(&req).is_err() {
        return PortalState::Unknown("the request was not sent".into());
    }
    let mut buf = Vec::new();
    let _ = s.take(4096).read_to_end(&mut buf);
    match parse_http(&buf) {
        Ok((st, loc, body)) => classify_probe(probe, st, loc.as_deref(), &body),
        Err(e) => PortalState::Unknown(e),
    }
}

/// The sentence for Settings and the assistant.
pub fn portal_sentence(s: &PortalState) -> String {
    match s {
        PortalState::Open => "The network lets traffic out; no sign-in is needed.".into(),
        PortalState::SignIn(Some(u)) => format!("This network wants a sign-in first (a hotel, café or school page). Open {u} in the browser, sign in there, then check again."),
        PortalState::SignIn(None) => "This network wants a sign-in first (a hotel, café or school page), but did not say where. Open any plain http page in the browser to be sent to it.".into(),
        PortalState::Unknown(why) => format!("Could not tell whether the network wants a sign-in: {why}."),
    }
}

// ------------------------------------------------------------------------------------------------ live signal

/// The joined network as `iw dev X link` shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub ssid: String,
    pub bssid: String,
    pub freq_mhz: u32,
    pub signal_dbm: f32,
    pub tx_mbit: Option<f32>,
}

/// Reads `iw dev X link`. None when not connected.
pub fn parse_link(text: &str) -> Option<Link> {
    let first = text.lines().next()?.trim();
    let bssid = first.strip_prefix("Connected to ")?.split_whitespace().next()?.to_ascii_lowercase();
    let field = |k: &str| text.lines().find_map(|l| l.trim().strip_prefix(k)).map(|v| v.trim().to_string());
    let ssid = field("SSID:").unwrap_or_default();
    let freq_mhz = field("freq:")?.split_whitespace().next()?.split('.').next()?.parse().ok()?;
    let signal_dbm = field("signal:")?.split_whitespace().next()?.parse().ok()?;
    let tx_mbit = field("tx bitrate:").and_then(|v| v.split_whitespace().next().and_then(|n| n.parse().ok()));
    Some(Link { ssid, bssid, freq_mhz, signal_dbm, tx_mbit })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trend {
    Rising,
    Steady,
    Falling,
}

/// Follows the signal over time: a smoothed value (the last readings averaged), its bars and whether it is getting better or worse.
#[derive(Default)]
pub struct SignalTracker {
    last: std::collections::VecDeque<f32>,
}

const KEEP: usize = 8;

impl SignalTracker {
    pub fn new() -> SignalTracker {
        SignalTracker::default()
    }

    pub fn push(&mut self, dbm: f32) {
        if !dbm.is_finite() {
            return;
        }
        self.last.push_back(dbm.clamp(-120.0, 0.0));
        while self.last.len() > KEEP {
            self.last.pop_front();
        }
    }

    pub fn smoothed(&self) -> Option<f32> {
        (!self.last.is_empty()).then(|| self.last.iter().sum::<f32>() / self.last.len() as f32)
    }

    pub fn bars(&self) -> Option<usize> {
        self.smoothed().map(bars_from_dbm)
    }

    /// Compares the older half of the readings with the newer half (a difference of 3 dB or more counts).
    pub fn trend(&self) -> Trend {
        let n = self.last.len();
        if n < 4 {
            return Trend::Steady;
        }
        let avg = |s: &[f32]| s.iter().sum::<f32>() / s.len() as f32;
        let v: Vec<f32> = self.last.iter().copied().collect();
        let d = avg(&v[n / 2..]) - avg(&v[..n / 2]);
        if d >= 3.0 { Trend::Rising } else if d <= -3.0 { Trend::Falling } else { Trend::Steady }
    }

    /// For the page: "Signal: good (-58 dBm, 3 of 4 bars), getting weaker".
    pub fn sentence(&self) -> String {
        let Some(s) = self.smoothed() else { return "Signal: not connected.".into() };
        let word = match bars_from_dbm(s) {
            4 => "excellent",
            3 => "good",
            2 => "fair",
            1 => "weak: move closer to the access point",
            _ => "very weak: the connection will drop",
        };
        let t = match self.trend() {
            Trend::Rising => ", getting stronger",
            Trend::Falling => ", getting weaker",
            Trend::Steady => "",
        };
        format!("Signal: {word} ({:.0} dBm, {} of 4 bars){t}", s, bars_from_dbm(s))
    }
}

/// 0 to 4 bars, the same table as [`Ap::bars`].
pub fn bars_from_dbm(dbm: f32) -> usize {
    Ap { ssid: String::new(), bssid: String::new(), freq_mhz: 0, signal_dbm: dbm, security: Security::Open, connected: false }.bars()
}

// ------------------------------------------------------------------------------------------------ adapters

/// One WiFi adapter as `iw dev` lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Adapter {
    pub name: String,
    pub phy: String,
    pub address: String,
    /// `managed` (a client), `AP`, `monitor` ...
    pub mode: String,
    /// The network it is joined to, if any.
    pub ssid: Option<String>,
}

/// Reads `iw dev`.
pub fn parse_iw_dev(text: &str) -> Vec<Adapter> {
    let (mut out, mut phy): (Vec<Adapter>, String) = (Vec::new(), String::new());
    for l in text.lines() {
        let t = l.trim();
        if let Some(p) = t.strip_prefix("phy#") {
            phy = format!("phy{}", p.trim());
        } else if let Some(n) = t.strip_prefix("Interface ") {
            out.push(Adapter { name: n.trim().to_string(), phy: phy.clone(), address: String::new(), mode: String::new(), ssid: None });
        } else if let Some(c) = out.last_mut() {
            if let Some(a) = t.strip_prefix("addr ") {
                c.address = a.trim().to_ascii_lowercase();
            } else if let Some(m) = t.strip_prefix("type ") {
                c.mode = m.trim().to_string();
            } else if let Some(s) = t.strip_prefix("ssid ") {
                c.ssid = Some(s.to_string());
            }
        }
    }
    out
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AdapterChoice {
    /// No usable adapter, and why.
    None(String),
    /// Use this one.
    Use(String),
    /// Two or more are usable and nothing decides: the person picks among these names.
    Ask(Vec<String>),
}

/// Chooses the adapter for joining networks. The hotspot's own interface and adapters that are not clients are never chosen. A
/// remembered choice wins when it is still there; else the adapter already joined to a network; else, if only one is left, that one;
/// else the person is asked.
pub fn choose_adapter(all: &[Adapter], remembered: Option<&str>) -> AdapterChoice {
    let usable: Vec<&Adapter> = all.iter().filter(|a| a.mode == "managed" && a.name != crate::HOTSPOT_IFACE).collect();
    if usable.is_empty() {
        return AdapterChoice::None("no WiFi adapter that can join networks was found".into());
    }
    if let Some(r) = remembered {
        if let Some(a) = usable.iter().find(|a| a.name == r) {
            return AdapterChoice::Use(a.name.clone());
        }
    }
    if usable.len() == 1 {
        return AdapterChoice::Use(usable[0].name.clone());
    }
    let joined: Vec<&&Adapter> = usable.iter().filter(|a| a.ssid.is_some()).collect();
    if joined.len() == 1 {
        return AdapterChoice::Use(joined[0].name.clone());
    }
    AdapterChoice::Ask(usable.iter().map(|a| a.name.clone()).collect())
}

// ------------------------------------------------------------------------------------------------ timers

/// When the portal probe and the signal read are due (pure: the caller passes the seconds and does the work). A joined network
/// that lets traffic out is checked rarely; one that wants a sign-in or gave no understood answer is checked often until it clears.
#[derive(Clone, Debug, Default)]
pub struct LinkTimers {
    last_probe: Option<u64>,
    last_signal: Option<u64>,
    state_open: bool,
    joined_at: Option<u64>,
}

impl LinkTimers {
    pub const SIGNAL_EVERY: u64 = 5;
    pub const PROBE_OPEN_EVERY: u64 = 120;
    pub const PROBE_WAITING_EVERY: u64 = 10;
    /// The first probe after joining waits this long for the address to arrive.
    pub const PROBE_AFTER_JOIN: u64 = 3;

    /// A network was joined (or the machine came back to it) at `now`: probes restart.
    pub fn joined(&mut self, now: u64) {
        self.joined_at = Some(now);
        self.last_probe = None;
        self.state_open = false;
    }

    /// The network was left: nothing is due.
    pub fn left(&mut self) {
        *self = LinkTimers::default();
    }

    pub fn signal_due(&self, now: u64) -> bool {
        self.joined_at.is_some() && self.last_signal.is_none_or(|l| now.saturating_sub(l) >= Self::SIGNAL_EVERY)
    }

    pub fn probe_due(&self, now: u64) -> bool {
        let Some(j) = self.joined_at else { return false };
        match self.last_probe {
            None => now.saturating_sub(j) >= Self::PROBE_AFTER_JOIN,
            Some(l) => now.saturating_sub(l) >= if self.state_open { Self::PROBE_OPEN_EVERY } else { Self::PROBE_WAITING_EVERY },
        }
    }

    pub fn signal_done(&mut self, now: u64) {
        self.last_signal = Some(now);
    }

    pub fn probe_done(&mut self, now: u64, state: &PortalState) {
        self.last_probe = Some(now);
        self.state_open = matches!(state, PortalState::Open);
    }
}

#[cfg(test)]
#[path = "wifi_ext_tests.rs"]
mod tests;
