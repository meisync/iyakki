//! The QR code tool of DEV TOOLS: what to put in the code (plain text or a ready-made payload: web address, Wi-Fi network, phone
//! number, e-mail, text message, map position, contact card), the symbol itself (the agent's encoder, [`iyakki_agent::qr`]) and
//! the two files it can be saved as. Always black on white with the quiet zone of four modules, whatever the theme, so a phone
//! can scan it from the screen or from a print.

use iyakki_agent::qr::{self, Level, Qr, WifiSecurity};

/// The quiet zone the standard asks for, in modules, on every side.
pub const QUIET: usize = 4;

pub struct Preset {
    pub name: &'static str,
    /// The input box's label and hint for this kind of code.
    pub label: &'static str,
    pub hint: &'static str,
}

pub const TEXT: usize = 0;
pub const WIFI: usize = 2;

pub const PRESETS: [Preset; 8] = [
    Preset { name: "Text", label: "Text", hint: "Any text: a note, a code, a link" },
    Preset { name: "URL", label: "Web address", hint: "example.org/page (https:// is added when missing)" },
    Preset { name: "Wi-Fi", label: "Network name on line 1, password on line 2", hint: "Line 1: network name. Line 2: password" },
    Preset { name: "Phone", label: "Phone number", hint: "+49 30 1234567" },
    Preset { name: "Email", label: "Address on line 1, subject on line 2, message after that", hint: "name@example.org (line 2: subject, optional)" },
    Preset { name: "SMS", label: "Number on line 1, message after that", hint: "+49 30 1234567 (line 2: the message, optional)" },
    Preset { name: "Location", label: "Latitude, longitude", hint: "52.5200, 13.4050" },
    Preset { name: "Contact", label: "Name on line 1, phone on line 2, e-mail on line 3", hint: "Ada Lovelace (line 2: phone, line 3: e-mail)" },
];

/// A label for the choice of security on the Wi-Fi code.
pub fn security_name(s: WifiSecurity) -> &'static str {
    match s {
        WifiSecurity::Wpa => "WPA / WPA2 / WPA3",
        WifiSecurity::Wep => "WEP",
        WifiSecurity::Open => "Open (no password)",
    }
}

pub fn next_security(s: WifiSecurity) -> WifiSecurity {
    match s {
        WifiSecurity::Wpa => WifiSecurity::Wep,
        WifiSecurity::Wep => WifiSecurity::Open,
        WifiSecurity::Open => WifiSecurity::Wpa,
    }
}

pub fn next_level(l: Level) -> Level {
    Level::ALL[(l as usize + 1) % 4]
}

/// How much damage each level survives, in words.
pub fn level_name(l: Level) -> &'static str {
    match l {
        Level::L => "L, about 7%",
        Level::M => "M, about 15%",
        Level::Q => "Q, about 25%",
        Level::H => "H, about 30%",
    }
}

/// Percent-encoding of the characters a `mailto:` or `sms:` value may not hold as they are.
fn pct(s: &str) -> String {
    let mut o = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            o.push(b as char);
        } else {
            o.push_str(&format!("%{b:02X}"));
        }
    }
    o
}

/// A phone number as `+49301234567`: digits and a leading plus, the usual punctuation dropped.
fn number(s: &str) -> Result<String, String> {
    let mut o = String::new();
    for (i, c) in s.trim().chars().enumerate() {
        match c {
            '0'..='9' => o.push(c),
            '+' if i == 0 => o.push(c),
            ' ' | '-' | '.' | '(' | ')' | '/' => {}
            _ => return Err(format!("A phone number holds digits, and a plus sign at the start; \"{c}\" does not belong.")),
        }
    }
    if o.chars().filter(char::is_ascii_digit).count() < 3 {
        return Err("Type the phone number (at least three digits).".into());
    }
    Ok(o)
}

fn address(s: &str) -> Result<&str, String> {
    let s = s.trim();
    match s.split_once('@') {
        Some((a, d)) if !a.is_empty() && d.contains('.') && !d.ends_with('.') && !s.contains(char::is_whitespace) && !d.contains('@') => Ok(s),
        _ => Err("Type an e-mail address like name@example.org.".into()),
    }
}

/// Text of a contact card: backslash, semicolon, comma and line break written the way the format wants them.
fn vcard_text(s: &str) -> String {
    s.replace('\\', "\\\\").replace(';', "\\;").replace(',', "\\,").replace('\n', "\\n")
}

/// A map position: two numbers split by a comma, a semicolon or spaces.
fn position(s: &str) -> Result<String, String> {
    let parts: Vec<&str> = s.split(|c: char| c == ',' || c == ';' || c.is_whitespace()).filter(|p| !p.is_empty()).collect();
    let [lat, lon] = parts[..] else { return Err("Type two numbers: the latitude and the longitude, like 52.5200, 13.4050.".into()) };
    let (a, o): (f64, f64) = (lat.parse().map_err(|_| format!("\"{lat}\" is not a number."))?, lon.parse().map_err(|_| format!("\"{lon}\" is not a number."))?);
    if !(-90.0..=90.0).contains(&a) {
        return Err("The latitude is between -90 and 90.".into());
    }
    if !(-180.0..=180.0).contains(&o) {
        return Err("The longitude is between -180 and 180.".into());
    }
    Ok(format!("geo:{a},{o}"))
}

/// The text the code holds for a kind of code and what was typed. An empty box gives an empty text (nothing to draw yet);
/// a mistake gives a plain sentence.
pub fn payload(preset: usize, typed: &str, security: WifiSecurity, hidden: bool) -> Result<String, String> {
    if typed.trim().is_empty() && preset != TEXT {
        return Ok(String::new());
    }
    let lines: Vec<&str> = typed.split('\n').map(|l| l.strip_suffix('\r').unwrap_or(l)).collect();
    let line = |i: usize| lines.get(i).map_or("", |l| l.trim());
    match preset {
        1 => {
            let u = typed.trim();
            if u.contains(char::is_whitespace) {
                return Err("A web address has no spaces; encode them as %20 or use the Text kind.".into());
            }
            Ok(if u.contains("://") { u.to_string() } else { format!("https://{u}") })
        }
        2 => {
            let ssid = lines[0].trim_end();
            let pass = lines.get(1).copied().unwrap_or("");
            if ssid.is_empty() {
                return Err("Type the network name on the first line.".into());
            }
            if ssid.len() > 32 {
                return Err("A network name is at most 32 bytes.".into());
            }
            if security != WifiSecurity::Open && pass.is_empty() {
                return Err("Type the password on the second line, or choose Open network.".into());
            }
            Ok(qr::wifi_payload_with(ssid, pass, security, hidden))
        }
        3 => Ok(format!("tel:{}", number(typed)?)),
        4 => {
            let mut q = Vec::new();
            if !line(1).is_empty() {
                q.push(format!("subject={}", pct(line(1))));
            }
            let body = lines.iter().skip(2).copied().collect::<Vec<_>>().join("\n");
            if !body.trim().is_empty() {
                q.push(format!("body={}", pct(body.trim_end())));
            }
            let to = address(line(0))?;
            Ok(if q.is_empty() { format!("mailto:{to}") } else { format!("mailto:{to}?{}", q.join("&")) })
        }
        5 => {
            let to = number(line(0))?;
            let body = lines.iter().skip(1).copied().collect::<Vec<_>>().join("\n");
            Ok(if body.trim().is_empty() { format!("sms:{to}") } else { format!("sms:{to}?body={}", pct(body.trim_end())) })
        }
        6 => position(typed),
        7 => {
            let name = line(0);
            if name.is_empty() {
                return Err("Type the name on the first line.".into());
            }
            let (given, family) = name.rsplit_once(' ').map_or((name, ""), |(g, f)| (g.trim(), f));
            let mut v = format!("BEGIN:VCARD\nVERSION:3.0\nN:{};{};;;\nFN:{}\n", vcard_text(family), vcard_text(given), vcard_text(name));
            if !line(1).is_empty() {
                v.push_str(&format!("TEL:{}\n", number(line(1))?));
            }
            if !line(2).is_empty() {
                v.push_str(&format!("EMAIL:{}\n", address(line(2))?));
            }
            v.push_str("END:VCARD");
            Ok(v)
        }
        _ => Ok(typed.to_string()),
    }
}

/// What the page shows about the symbol made from the typed text.
pub struct Made {
    pub payload: String,
    pub qr: Option<Qr>,
    /// One plain line: the size and the bytes, or what is wrong.
    pub status: String,
    pub err: bool,
}

pub fn make(preset: usize, typed: &str, level: Level, security: WifiSecurity, hidden: bool) -> Made {
    let payload = match payload(preset, typed, security, hidden) {
        Ok(p) => p,
        Err(e) => return Made { payload: String::new(), qr: None, status: e, err: true },
    };
    if payload.is_empty() {
        return Made { payload, qr: None, status: "Type something above and the code appears here.".into(), err: false };
    }
    match qr::encode_level(payload.as_bytes(), level) {
        Some(q) => {
            let cap = qr::capacity(q.version(), level);
            let status = format!("Version {}, {s} x {s} modules, level {}, {} of {cap} bytes used.", q.version(), level_name(level), payload.len(), s = q.size);
            Made { payload, qr: Some(q), status, err: false }
        }
        None => {
            let most = qr::capacity(qr::MAX_VERSION, level);
            let status = format!("Too long for a QR code: {} bytes, and level {} holds at most {most}. Shorten the text{}.", payload.len(), level.letter(), if level == Level::L { "" } else { " or choose a lower error correction" });
            Made { payload, qr: None, status, err: true }
        }
    }
}

/// The picture as SVG: one path of dark runs on a white square, quiet zone included.
pub fn svg(q: &Qr) -> String {
    let n = q.size + 2 * QUIET;
    let mut d = String::new();
    for y in 0..q.size {
        let mut x = 0;
        while x < q.size {
            if q.get(x, y) {
                let start = x;
                while x < q.size && q.get(x, y) {
                    x += 1;
                }
                d.push_str(&format!("M{} {}h{}v1h-{}z", start + QUIET, y + QUIET, x - start, x - start));
            } else {
                x += 1;
            }
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {n} {n}\" width=\"{px}\" height=\"{px}\" shape-rendering=\"crispEdges\">\n<rect width=\"{n}\" height=\"{n}\" fill=\"#ffffff\"/>\n<path d=\"{d}\" fill=\"#000000\"/>\n</svg>\n",
        px = n * 8
    )
}

/// Opaque RGBA pixels of the picture, `px` pixels to a module, quiet zone included: (width, height, pixels).
pub fn rgba(q: &Qr, px: usize) -> (u32, u32, Vec<u8>) {
    let side = (q.size + 2 * QUIET) * px;
    let mut out = vec![255u8; side * side * 4];
    for y in 0..q.size {
        for x in 0..q.size {
            if !q.get(x, y) {
                continue;
            }
            for dy in 0..px {
                let row = ((y + QUIET) * px + dy) * side;
                for dx in 0..px {
                    let i = (row + (x + QUIET) * px + dx) * 4;
                    out[i..i + 3].fill(0);
                }
            }
        }
    }
    (side as u32, side as u32, out)
}

/// The picture as a PNG about 600 pixels wide, a whole number of pixels to a module.
pub fn png(q: &Qr) -> Result<Vec<u8>, String> {
    let px = (600 / (q.size + 2 * QUIET)).clamp(4, 16);
    let (w, h, data) = rgba(q, px);
    crate::camera::pixels::png_with_dpi(w, h, &data, 96.0)
}

/// `qr-20261105-143012.png`: the name of a saved picture, from the time.
pub fn file_name(ext: &str) -> String {
    format!("qr-{}.{ext}", crate::camera::stamp())
}

/// What the page keeps for this tool.
pub struct QrState {
    pub preset: usize,
    pub level: Level,
    pub security: WifiSecurity,
    pub hidden: bool,
    /// What the last Save or Copy said: (words, is a mistake).
    pub note: Option<(String, bool)>,
    /// The typed text of every kind of code (switching kinds keeps what was typed).
    pub texts: Vec<String>,
    made: Option<((usize, Level, WifiSecurity, bool, String), Made)>,
}

impl Default for QrState {
    fn default() -> QrState {
        QrState { preset: TEXT, level: Level::M, security: WifiSecurity::Wpa, hidden: false, note: None, texts: vec![String::new(); PRESETS.len()], made: None }
    }
}

impl QrState {
    /// The symbol for the typed text, made again only when something changed.
    pub fn made(&mut self, typed: &str) -> &Made {
        let key = (self.preset, self.level, self.security, self.hidden, typed.to_string());
        if self.made.as_ref().is_none_or(|(k, _)| *k != key) {
            let m = make(self.preset, typed, self.level, self.security, self.hidden);
            self.made = Some((key, m));
        }
        self.made.as_ref().map(|(_, m)| m).expect("just made")
    }
}
