//! A small in-house ONVIF client for pan, tilt and zoom (F-SDK-167, F-CAM-22).
//!
//! ONVIF is SOAP over HTTP. Only what a camera view needs is spoken here: `GetCapabilities` (where the PTZ service lives),
//! `GetProfiles` (the profile token that has a PTZ configuration), `ContinuousMove` and `Stop`. The login is the WS-Security
//! UsernameToken with a password digest (`Base64(SHA1(nonce + created + password))`), so the password never crosses the wire.
//!
//! The network is a [`Transport`] (a test gives a fake camera; [`HttpTransport`] speaks plain HTTP over TCP). Time and the nonce come from
//! a [`Source`], so a test is exact. Nothing here logs an address, a user name or a password.

use iyakki_core_api::cameras::PtzCommand;
use iyakki_link::wsproto::{base64, sha1};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// One SOAP request to a URL; the answer is the response body.
pub trait Transport: Send + Sync {
    fn post(&self, url: &str, soap_action: &str, body: &str) -> Result<String, String>;
}

/// The time (ISO 8601, UTC) and sixteen random bytes for one request.
pub trait Source: Send + Sync {
    fn created(&self) -> String;
    fn nonce(&self) -> [u8; 16];
}

/// The real clock and the system's random bytes.
pub struct SystemSource;

impl Source for SystemSource {
    fn created(&self) -> String {
        let t = crate::clock::now_unix();
        let (days, rem) = (t.div_euclid(86_400), t.rem_euclid(86_400));
        // civil date from days since 1970-01-01 (Howard Hinnant's algorithm)
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = yoe + era * 400 + i64::from(m <= 2);
        format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
    }
    fn nonce(&self) -> [u8; 16] {
        let mut b = [0u8; 16];
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            let _ = f.read_exact(&mut b);
        }
        b
    }
}

/// Plain HTTP over TCP with a short time limit; an `https://` address is refused (no TLS client is used for cameras here).
pub struct HttpTransport {
    pub timeout: Duration,
}

impl Default for HttpTransport {
    fn default() -> Self {
        HttpTransport { timeout: Duration::from_secs(3) }
    }
}

/// `(host:port, path)` of an `http://` URL.
pub fn split_http(url: &str) -> Result<(String, String), String> {
    let rest = url.strip_prefix("http://").ok_or("only http:// camera control addresses are used")?;
    let (hp, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    if hp.is_empty() || hp.contains('@') {
        return Err("the camera control address has no host".into());
    }
    let hp = if hp.contains(':') && !hp.ends_with(']') { hp.to_string() } else { format!("{hp}:80") };
    Ok((hp, path.to_string()))
}

impl Transport for HttpTransport {
    fn post(&self, url: &str, soap_action: &str, body: &str) -> Result<String, String> {
        let (hp, path) = split_http(url)?;
        let addr = std::net::ToSocketAddrs::to_socket_addrs(&hp).map_err(|_| "the camera's address does not resolve".to_string())?.next().ok_or("the camera's address does not resolve")?;
        let mut s = TcpStream::connect_timeout(&addr, self.timeout).map_err(|_| "the camera did not answer".to_string())?;
        let _ = s.set_read_timeout(Some(self.timeout));
        let _ = s.set_write_timeout(Some(self.timeout));
        let host = hp.rsplit_once(':').map(|x| x.0).unwrap_or(&hp);
        let req = format!("POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/soap+xml; charset=utf-8; action=\"{soap_action}\"\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
        s.write_all(req.as_bytes()).map_err(|_| "the camera did not take the request".to_string())?;
        let mut buf = Vec::new();
        let _ = s.take(1 << 20).read_to_end(&mut buf);
        let text = String::from_utf8_lossy(&buf).to_string();
        let (head, rest) = text.split_once("\r\n\r\n").ok_or("the camera sent no answer")?;
        let code: u32 = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
        if code == 401 || code == 403 {
            return Err("the camera refused the user name or password".into());
        }
        if !(200..300).contains(&code) && !rest.contains("Fault") {
            return Err(format!("the camera answered {code}"));
        }
        Ok(rest.to_string())
    }
}

// ------------------------------------------------------------------------------------------------ SOAP

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The text of the first `<prefix:name ...>text</prefix:name>` (or `<name>`) in `xml`.
pub fn tag_text(xml: &str, name: &str) -> Option<String> {
    let mut from = 0;
    while let Some(i) = xml[from..].find('<') {
        let start = from + i + 1;
        let end = xml[start..].find('>')? + start;
        let tag = &xml[start..end];
        let local = tag.split_whitespace().next().unwrap_or("").rsplit(':').next().unwrap_or("");
        if local == name && !tag.starts_with('/') && !tag.ends_with('/') {
            let close = xml[end + 1..].find("</")? + end + 1;
            return Some(xml[end + 1..close].trim().to_string());
        }
        from = end;
    }
    None
}

/// The value of attribute `attr` of the first element called `name`.
pub fn tag_attr(xml: &str, name: &str, attr: &str) -> Option<String> {
    let mut from = 0;
    while let Some(i) = xml[from..].find('<') {
        let start = from + i + 1;
        let end = xml[start..].find('>')? + start;
        let tag = &xml[start..end];
        let local = tag.split_whitespace().next().unwrap_or("").rsplit(':').next().unwrap_or("");
        if local == name {
            let key = format!("{attr}=\"");
            let k = tag.find(&key)? + key.len();
            let v = tag[k..].find('"')? + k;
            return Some(tag[k..v].to_string());
        }
        from = end;
    }
    None
}

/// What one answer says when the camera refused: the fault's reason text.
fn fault(xml: &str) -> Option<String> {
    if !xml.contains("Fault") {
        return None;
    }
    Some(tag_text(xml, "Text").unwrap_or_else(|| "the camera refused the request".into()).chars().filter(|c| !c.is_control()).take(120).collect())
}

pub struct Onvif {
    transport: Box<dyn Transport>,
    source: Box<dyn Source>,
    user: String,
    pass: zeroize::Zeroizing<String>,
}

impl Onvif {
    pub fn new(transport: Box<dyn Transport>, source: Box<dyn Source>, user: &str, pass: &str) -> Onvif {
        Onvif { transport, source, user: user.into(), pass: zeroize::Zeroizing::new(pass.into()) }
    }

    /// The UsernameToken header for one request.
    pub fn security_header(&self) -> String {
        let (created, nonce) = (self.source.created(), self.source.nonce());
        let mut d = nonce.to_vec();
        d.extend_from_slice(created.as_bytes());
        d.extend_from_slice(self.pass.as_bytes());
        let digest = base64(&sha1(&d));
        format!(
            "<s:Header><Security xmlns=\"http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd\"><UsernameToken><Username>{}</Username><Password Type=\"http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest\">{digest}</Password><Nonce EncodingType=\"http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-soap-message-security-1.0#Base64Binary\">{}</Nonce><Created xmlns=\"http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd\">{created}</Created></UsernameToken></Security></s:Header>",
            esc(&self.user),
            base64(&nonce)
        )
    }

    fn call(&self, url: &str, action: &str, body: &str) -> Result<String, String> {
        let env = format!("<?xml version=\"1.0\" encoding=\"utf-8\"?><s:Envelope xmlns:s=\"http://www.w3.org/2003/05/soap-envelope\">{}<s:Body>{body}</s:Body></s:Envelope>", self.security_header());
        let out = self.transport.post(url, action, &env)?;
        match fault(&out) {
            Some(f) => Err(f),
            None => Ok(out),
        }
    }

    /// The addresses of the PTZ and media services of a camera whose device service is at `device_url`.
    pub fn capabilities(&self, device_url: &str) -> Result<(Option<String>, Option<String>), String> {
        let out = self.call(device_url, "http://www.onvif.org/ver10/device/wsdl/GetCapabilities", "<GetCapabilities xmlns=\"http://www.onvif.org/ver10/device/wsdl\"><Category>All</Category></GetCapabilities>")?;
        let ptz = out.find("PTZ>").and_then(|i| tag_text(&out[i.saturating_sub(1)..], "XAddr"));
        let media = out.find("Media>").and_then(|i| tag_text(&out[i.saturating_sub(1)..], "XAddr"));
        Ok((ptz, media))
    }

    /// The token of the first media profile that has a PTZ configuration.
    pub fn ptz_profile(&self, media_url: &str) -> Result<Option<String>, String> {
        let out = self.call(media_url, "http://www.onvif.org/ver10/media/wsdl/GetProfiles", "<GetProfiles xmlns=\"http://www.onvif.org/ver10/media/wsdl\"/>")?;
        // each profile is one `Profiles` element; take the first whose text holds a PTZConfiguration
        for chunk in out.split("Profiles ").skip(1) {
            let one = chunk.split("</trt:Profiles>").next().unwrap_or(chunk);
            if one.contains("PTZConfiguration") {
                if let Some(i) = one.find("token=\"") {
                    let rest = &one[i + 7..];
                    return Ok(rest.find('"').map(|j| rest[..j].to_string()));
                }
            }
        }
        Ok(None)
    }

    pub fn move_continuous(&self, ptz_url: &str, profile: &str, pan: f32, tilt: f32, zoom: f32) -> Result<(), String> {
        let body = format!(
            "<ContinuousMove xmlns=\"http://www.onvif.org/ver20/ptz/wsdl\"><ProfileToken>{}</ProfileToken><Velocity><PanTilt x=\"{pan:.3}\" y=\"{tilt:.3}\" xmlns=\"http://www.onvif.org/ver10/schema\"/><Zoom x=\"{zoom:.3}\" xmlns=\"http://www.onvif.org/ver10/schema\"/></Velocity></ContinuousMove>",
            esc(profile)
        );
        self.call(ptz_url, "http://www.onvif.org/ver20/ptz/wsdl/ContinuousMove", &body).map(|_| ())
    }

    /// The camera's saved positions as `(token, name)`, at most 32 (`GetPresets`).
    pub fn presets(&self, ptz_url: &str, profile: &str) -> Result<Vec<(String, String)>, String> {
        let body = format!("<GetPresets xmlns=\"http://www.onvif.org/ver20/ptz/wsdl\"><ProfileToken>{}</ProfileToken></GetPresets>", esc(profile));
        let out = self.call(ptz_url, "http://www.onvif.org/ver20/ptz/wsdl/GetPresets", &body)?;
        let mut v = Vec::new();
        for chunk in out.split("Preset ").skip(1).take(32) {
            let one = chunk.split("</tptz:Preset>").next().unwrap_or(chunk);
            let Some(i) = one.find("token=\"") else { continue };
            let rest = &one[i + 7..];
            let Some(j) = rest.find('"') else { continue };
            let token = rest[..j].to_string();
            let name: String = tag_text(one, "Name").unwrap_or_default().chars().filter(|c| !c.is_control()).take(40).collect();
            v.push((token.clone(), if name.is_empty() { format!("position {token}") } else { name }));
        }
        Ok(v)
    }

    /// Move to a saved position (`GotoPreset`); the token must be one `presets` named (a token with markup is refused before it is sent).
    pub fn goto_preset(&self, ptz_url: &str, profile: &str, token: &str) -> Result<(), String> {
        if token.is_empty() || token.len() > 64 || !token.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.')) {
            return Err("that is not a saved position of this camera".into());
        }
        let body = format!("<GotoPreset xmlns=\"http://www.onvif.org/ver20/ptz/wsdl\"><ProfileToken>{}</ProfileToken><PresetToken>{}</PresetToken></GotoPreset>", esc(profile), esc(token));
        self.call(ptz_url, "http://www.onvif.org/ver20/ptz/wsdl/GotoPreset", &body).map(|_| ())
    }

    pub fn stop(&self, ptz_url: &str, profile: &str) -> Result<(), String> {
        let body = format!("<Stop xmlns=\"http://www.onvif.org/ver20/ptz/wsdl\"><ProfileToken>{}</ProfileToken><PanTilt>true</PanTilt><Zoom>true</Zoom></Stop>", esc(profile));
        self.call(ptz_url, "http://www.onvif.org/ver20/ptz/wsdl/Stop", &body).map(|_| ())
    }
}

/// A camera whose pan, tilt and zoom were found: the service address and the profile to move.
pub struct PtzCamera {
    client: Onvif,
    ptz_url: String,
    profile: String,
}

impl PtzCamera {
    /// Ask the camera at `device_url` whether it can move. `Ok(None)` is a camera that answers and has nothing to move.
    pub fn probe(client: Onvif, device_url: &str) -> Result<Option<PtzCamera>, String> {
        let (ptz, media) = client.capabilities(device_url)?;
        let (Some(ptz_url), Some(media_url)) = (ptz, media) else { return Ok(None) };
        match client.ptz_profile(&media_url)? {
            Some(profile) => Ok(Some(PtzCamera { client, ptz_url, profile })),
            None => Ok(None),
        }
    }

    /// The saved positions of the camera as `(token, name)`.
    pub fn presets(&self) -> Result<Vec<(String, String)>, String> {
        self.client.presets(&self.ptz_url, &self.profile)
    }

    /// Go to one of the saved positions by the name or token `presets` gave; an unknown one is a plain sentence, nothing is sent.
    pub fn go_to(&self, which: &str) -> Result<String, String> {
        let list = self.presets()?;
        let w = which.trim();
        let hit = list.iter().find(|(t, n)| t == w || n.eq_ignore_ascii_case(w)).ok_or_else(|| format!("this camera has no saved position called {w}"))?;
        self.client.goto_preset(&self.ptz_url, &self.profile, &hit.0)?;
        Ok(hit.1.clone())
    }

    /// Apply one checked command: a move at the given speeds, or stop.
    pub fn command(&self, cmd: PtzCommand) -> Result<(), String> {
        match cmd.checked()? {
            PtzCommand::Stop => self.client.stop(&self.ptz_url, &self.profile),
            PtzCommand::Move { pan, tilt, zoom } => self.client.move_continuous(&self.ptz_url, &self.profile, pan, tilt, zoom),
        }
    }
}

/// The device service address of a camera whose stream address is `address` (`rtsp://host:554/...` or `http://host/...`): the same host on
/// the ONVIF port `onvif_port` (80 by default), path `/onvif/device_service`.
pub fn device_url(address: &str, onvif_port: Option<u16>) -> Option<String> {
    let rest = address.split_once("://")?.1;
    let hostport = rest.split('/').next()?.rsplit('@').next()?;
    let host = if hostport.starts_with('[') { hostport.split_once(']').map(|x| format!("{}]", x.0))? } else { hostport.split(':').next()?.to_string() };
    if host.is_empty() {
        return None;
    }
    Some(match onvif_port {
        Some(p) if p != 80 => format!("http://{host}:{p}/onvif/device_service"),
        _ => format!("http://{host}/onvif/device_service"),
    })
}

/// The ports to try for the device service of a camera, in order: the port its own `http://` stream address names, then the usual ones
/// (80, 8000, 8080, 2020). A camera answers on one of them; the first that answers is used and remembered by the caller.
pub fn candidate_ports(address: &str) -> Vec<u16> {
    let mut v = Vec::new();
    if let Some(rest) = address.strip_prefix("http://") {
        let hostport = rest.split('/').next().unwrap_or("").rsplit('@').next().unwrap_or("");
        if let Some(p) = hostport.rsplit_once(':').filter(|(h, _)| !h.ends_with(']') || hostport.contains(']')).and_then(|(_, p)| p.parse::<u16>().ok()) {
            v.push(p);
        }
    }
    for p in [80u16, 8000, 8080, 2020] {
        if !v.contains(&p) {
            v.push(p);
        }
    }
    v
}

#[cfg(test)]
#[path = "onvif_tests.rs"]
mod tests;
