//! The typed profile. It has **no field for a secret**: keys and passwords travel in [`ProfileSecrets`] and the store
//! keeps them in the vault; the profile on disk holds only what is public (peer public keys, endpoints, routes).

use serde::{Deserialize, Serialize};

use crate::b64;
use crate::error::{rejected, VpnError};
use crate::filter::FilterSet;
use crate::net::Cidr;
use crate::secret::ProfileSecrets;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Wireguard,
    Openvpn,
}

impl Kind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Kind::Wireguard => "wireguard",
            Kind::Openvpn => "openvpn",
        }
    }
}

/// `host:port`; the host is an address (IPv6 without brackets) or a DNS name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
}

impl std::fmt::Display for Endpoint {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.host.contains(':') {
            write!(f, "[{}]:{}", self.host, self.port)
        } else {
            write!(f, "{}:{}", self.host, self.port)
        }
    }
}

/// A WireGuard public key (not secret). Serialised as base64.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct PublicKey(pub [u8; 32]);

impl PublicKey {
    pub fn parse(s: &str) -> Option<PublicKey> {
        b64::decode(s.trim())?.try_into().ok().map(PublicKey)
    }
}

impl std::fmt::Display for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&b64::encode(&self.0))
    }
}

impl std::fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "PublicKey({self})")
    }
}

impl From<PublicKey> for String {
    fn from(k: PublicKey) -> String {
        k.to_string()
    }
}

impl TryFrom<String> for PublicKey {
    type Error = String;
    fn try_from(s: String) -> Result<PublicKey, String> {
        PublicKey::parse(&s).ok_or_else(|| "not a 32-byte base64 key".to_string())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WgPeer {
    pub public_key: PublicKey,
    pub endpoint: Option<Endpoint>,
    pub allowed_ips: Vec<Cidr>,
    /// Seconds, 0 or absent = off.
    pub keepalive: Option<u16>,
    /// A preshared key exists; the key itself is in the vault.
    pub has_preshared: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WgConfig {
    pub addresses: Vec<Cidr>,
    pub dns: Vec<std::net::IpAddr>,
    pub mtu: Option<u16>,
    pub listen_port: Option<u16>,
    pub peers: Vec<WgPeer>,
}

impl WgConfig {
    /// True when any peer routes the default route (a full tunnel).
    pub fn full_tunnel(&self) -> bool {
        self.peers.iter().flat_map(|p| &p.allowed_ips).any(|c| c.is_default())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvpnRemote {
    pub host: String,
    pub port: u16,
    pub proto: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OvpnConfig {
    /// The accepted directives and the public inline blocks (`ca`, `cert`, `dh`), re-written by us; secret blocks are cut out.
    pub text: String,
    pub remotes: Vec<OvpnRemote>,
    /// Static `route` lines, checked.
    pub routes: Vec<Cidr>,
    pub dns: Vec<std::net::IpAddr>,
    pub redirect_gateway: bool,
    /// `auth-user-pass` without a file: a user name and password are needed at connect time.
    pub needs_credentials: bool,
    /// Names of the inline blocks that were moved into the secrets (they are put back at connect time).
    pub secret_blocks: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Body {
    Wireguard(WgConfig),
    Openvpn(OvpnConfig),
}

/// How a profile uses the tunnel.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// All traffic goes through the VPN.
    #[default]
    Full,
    /// Only what the person's filters match goes through the VPN; everything else goes direct.
    Hybrid,
}

/// The person's choices for one profile (never imported from a file: a `.conf` or `.ovpn` cannot set them).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    pub mode: Mode,
    /// FULL only: keep blocking outside traffic when the tunnel drops. Default off.
    pub kill_switch: bool,
    /// HYBRID only: when the tunnel is down, matched destinations are dropped and never sent direct. Default on for HYBRID.
    pub fail_closed: bool,
    /// HYBRID only: the filters.
    pub filters: FilterSet,
    /// The public-IP lookup contacts a third party. Default off.
    pub public_ip_lookup: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings::full()
    }
}

impl Settings {
    pub fn full() -> Settings {
        Settings { mode: Mode::Full, kill_switch: false, fail_closed: false, filters: FilterSet::new(), public_ip_lookup: false }
    }
    pub fn hybrid(filters: FilterSet) -> Settings {
        Settings { mode: Mode::Hybrid, kill_switch: false, fail_closed: true, filters, public_ip_lookup: false }
    }
    /// Refuse combinations that make no sense. Catch-all patterns were already refused when the filters were parsed.
    pub fn validate(&self) -> Result<(), VpnError> {
        match self.mode {
            Mode::Full if !self.filters.is_empty() => Err(rejected(0, "filters", "filters belong to HYBRID mode; FULL sends everything through the VPN")),
            Mode::Hybrid if self.kill_switch => Err(rejected(0, "kill switch", "the kill switch is a FULL mode option; HYBRID has the fail-closed option")),
            Mode::Hybrid if !self.filters.has_includes() => Err(VpnError::Missing("a HYBRID profile needs at least one rule that sends something through the VPN".into())),
            _ => Ok(()),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Profile {
    /// Random, assigned by the store; never derived from the name and never a file name taken from input.
    pub id: String,
    pub name: String,
    pub body: Body,
    /// Old profiles without it are FULL with every option off.
    #[serde(default)]
    pub settings: Settings,
}

impl Profile {
    pub fn kind(&self) -> Kind {
        match self.body {
            Body::Wireguard(_) => Kind::Wireguard,
            Body::Openvpn(_) => Kind::Openvpn,
        }
    }
    /// The first server, for the list and the audit line (`host:port`).
    pub fn endpoint_label(&self) -> Option<String> {
        match &self.body {
            Body::Wireguard(w) => w.peers.iter().find_map(|p| p.endpoint.as_ref()).map(|e| e.to_string()),
            Body::Openvpn(o) => o.remotes.first().map(|r| format!("{}:{}", r.host, r.port)),
        }
    }
    pub fn full_tunnel(&self) -> bool {
        match &self.body {
            Body::Wireguard(w) => w.full_tunnel(),
            Body::Openvpn(o) => o.redirect_gateway,
        }
    }
}

/// The result of an import: the body, the secrets cut out of it and warnings for the person.
#[derive(Debug)]
pub struct Imported {
    pub name: String,
    pub body: Body,
    pub secrets: ProfileSecrets,
    pub warnings: Vec<String>,
}

impl Imported {
    pub fn into_profile(self, id: &str) -> (Profile, ProfileSecrets, Vec<String>) {
        (Profile { id: id.to_string(), name: self.name, body: self.body, settings: Settings::full() }, self.secrets, self.warnings)
    }
}
