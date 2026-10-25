//! The "iyakki-dev" beacon: a few text lines in one UDP datagram that a computer sends on every direct link every few seconds so the
//! other end finds it without typing an address. No secrets: host name, version, whether developer mode is on and whether a pairing
//! code is open (never the code), the link's MAC address and a random id to recognise our own packets. The parser never panics and
//! refuses anything over [`MAX_LEN`] bytes, with unknown characters, too many lines or a bad field.

use serde::{Deserialize, Serialize};

pub const MAGIC: &str = "IYAKKI-DEV 1";
pub const MAX_LEN: usize = 512;
const MAX_LINES: usize = 14;
const MAX_LINE: usize = 100;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Beacon {
    pub host: String,
    pub version: String,
    /// `target`: developer mode is on here and it can be paired with; `host`: this computer can pair with others.
    pub role: String,
    pub dev_on: bool,
    pub pairing_open: bool,
    pub mac: String,
    pub id: String,
    pub port: u16,
}

#[derive(Debug, PartialEq, Eq)]
pub enum BeaconError {
    TooLong,
    NotText,
    Magic,
    TooManyLines,
    Field(&'static str),
    Missing(&'static str),
}

impl std::fmt::Display for BeaconError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BeaconError::TooLong => write!(f, "longer than {MAX_LEN} bytes"),
            BeaconError::NotText => write!(f, "not plain text"),
            BeaconError::Magic => write!(f, "not an iyakki-dev beacon"),
            BeaconError::TooManyLines => write!(f, "too many lines"),
            BeaconError::Field(n) => write!(f, "bad value for {n}"),
            BeaconError::Missing(n) => write!(f, "{n} is missing"),
        }
    }
}

fn host_ok(s: &str) -> bool {
    (1..=63).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

fn version_ok(s: &str) -> bool {
    (1..=32).contains(&s.len()) && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
}

fn mac_ok(s: &str) -> bool {
    super::linklocal::parse_mac(s).is_some()
}

fn id_ok(s: &str) -> bool {
    (8..=32).contains(&s.len()) && s.chars().all(|c| c.is_ascii_hexdigit())
}

impl Beacon {
    pub fn encode(&self) -> Vec<u8> {
        format!(
            "{MAGIC}\nhost={}\nver={}\nrole={}\ndev={}\npairing={}\nmac={}\nid={}\nport={}\n",
            self.host,
            self.version,
            self.role,
            u8::from(self.dev_on),
            u8::from(self.pairing_open),
            self.mac,
            self.id,
            self.port
        )
        .into_bytes()
    }

    pub fn parse(b: &[u8]) -> Result<Beacon, BeaconError> {
        if b.len() > MAX_LEN {
            return Err(BeaconError::TooLong);
        }
        let t = std::str::from_utf8(b).map_err(|_| BeaconError::NotText)?;
        if t.chars().any(|c| (c.is_control() && c != '\n') || !c.is_ascii()) {
            return Err(BeaconError::NotText);
        }
        let mut lines = t.lines();
        if lines.next() != Some(MAGIC) {
            return Err(BeaconError::Magic);
        }
        let (mut host, mut ver, mut role, mut dev, mut pairing, mut mac, mut id, mut port) = (None, None, None, None, None, None, None, None);
        for (n, l) in lines.enumerate() {
            if n >= MAX_LINES {
                return Err(BeaconError::TooManyLines);
            }
            if l.len() > MAX_LINE {
                return Err(BeaconError::TooLong);
            }
            let Some((k, v)) = l.split_once('=') else { continue };
            match k {
                "host" => host = Some(v.to_string()).filter(|s| host_ok(s)).ok_or(BeaconError::Field("host")).map(Some)?,
                "ver" => ver = Some(v.to_string()).filter(|s| version_ok(s)).ok_or(BeaconError::Field("ver")).map(Some)?,
                "role" => role = Some(v.to_string()).filter(|s| s == "target" || s == "host").ok_or(BeaconError::Field("role")).map(Some)?,
                "dev" => dev = Some(flag(v).ok_or(BeaconError::Field("dev"))?),
                "pairing" => pairing = Some(flag(v).ok_or(BeaconError::Field("pairing"))?),
                "mac" => mac = Some(v.to_ascii_lowercase()).filter(|s| mac_ok(s)).ok_or(BeaconError::Field("mac")).map(Some)?,
                "id" => id = Some(v.to_ascii_lowercase()).filter(|s| id_ok(s)).ok_or(BeaconError::Field("id")).map(Some)?,
                "port" => port = Some(v.parse::<u16>().ok().filter(|p| *p >= 1024).ok_or(BeaconError::Field("port"))?),
                _ => {}
            }
        }
        Ok(Beacon {
            host: host.ok_or(BeaconError::Missing("host"))?,
            version: ver.ok_or(BeaconError::Missing("ver"))?,
            role: role.ok_or(BeaconError::Missing("role"))?,
            dev_on: dev.ok_or(BeaconError::Missing("dev"))?,
            pairing_open: pairing.unwrap_or(false),
            mac: mac.ok_or(BeaconError::Missing("mac"))?,
            id: id.ok_or(BeaconError::Missing("id"))?,
            port: port.unwrap_or(super::DEV_PORT),
        })
    }
}

fn flag(v: &str) -> Option<bool> {
    match v {
        "0" => Some(false),
        "1" => Some(true),
        _ => None,
    }
}
