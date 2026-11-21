//! The plain data of the RADIO tile (step 1 of docs/development/RADIO-EXTRACTION-PLAN.md): a station, the certificate a person may trust for one,
//! the player's state and the level bands. They live here so the tile crate can use them without the agent; `iyakki-agent::radio` re-exports them,
//! so the agent and the shell name the same types as before and nothing is converted.

use serde::{Deserialize, Serialize};

/// The host name of a station address, lower case, without port, path or brackets (what the audit line and the policy check use).
pub fn host_of(u: &str) -> String {
    let rest = u.split_once("://").map_or(u, |x| x.1);
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    authority.rsplit_once(':').filter(|(_, p)| p.chars().all(|c| c.is_ascii_digit())).map_or(authority, |x| x.0).trim_matches(['[', ']']).to_ascii_lowercase()
}

/// A certificate fingerprint as plain upper-case hex (the same function as `iyakki_proxy::certinfo::norm_fingerprint`; a test in the shell keeps them equal).
pub fn norm_fingerprint(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_hexdigit()).map(|c| c.to_ascii_uppercase()).collect()
}

/// A fingerprint in groups of eight hex digits, for reading aloud or comparing by eye.
pub fn fingerprint_groups(fp: &str) -> String {
    let n = norm_fingerprint(fp);
    n.as_bytes().chunks(8).map(|c| String::from_utf8_lossy(c).to_string()).collect::<Vec<_>>().join(" ")
}

/// The first 8 hex digits of a fingerprint, for the audit trail.
pub fn short_fingerprint(fp: &str) -> String {
    norm_fingerprint(fp).chars().take(8).collect()
}

/// How many bars the level display has.
pub const BANDS: usize = 28;

/// Stations to start from, by language: `(name, stream address)`. The data itself stays in the agent (`radio_stations`).
pub struct Language {
    pub name: &'static str,
    pub stations: &'static [(&'static str, &'static str)],
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Station {
    pub name: String,
    pub url: String,
    /// The certificate the person approved for this station's server (see [`CertPin`]). Old files have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cert_pin: Option<CertPin>,
}

impl Station {
    pub fn new(name: &str, url: &str) -> Station {
        Station { name: name.to_string(), url: url.to_string(), cert_pin: None }
    }
}

/// A server certificate a person chose to trust for one station: the exact leaf certificate (SHA-256) of one host and port.
/// It is never a general "do not verify": the connection to that host must show exactly this certificate, for that name, within
/// its dates; a renewed certificate asks again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertPin {
    pub host: String,
    pub port: u16,
    /// Upper-case hex pairs with colons.
    pub sha256: String,
}

/// Why a station's secure connection was stopped, with what the person needs to decide about trusting the certificate.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CertProblem {
    pub station: String,
    pub url: String,
    pub host: String,
    pub port: u16,
    /// True when the station was trusted before and now shows a different certificate.
    pub changed: bool,
    pub sha256: String,
    pub subject: String,
    pub issuer: String,
    /// Unix time.
    pub not_after: i64,
    /// Trusting only helps when the certificate is for this host name and still valid.
    pub trustable: bool,
    /// The plain-language sentence for the person (also the radio's failure text).
    pub message: String,
}

impl CertProblem {
    pub fn pin(&self) -> CertPin {
        CertPin { host: self.host.clone(), port: self.port, sha256: self.sha256.clone() }
    }

    /// The end date as `YYYY-MM-DD` (UTC).
    pub fn expiry(&self) -> String {
        date_text(self.not_after)
    }
}

/// `YYYY-MM-DD` of a Unix time (UTC). The same arithmetic as `iyakki_proxy::certinfo::date_text`; a test in the shell keeps the two equal.
pub fn date_text(t: i64) -> String {
    let z = t.div_euclid(86400) + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// What holds the speaker when a station is about to start (another player of IYAKKI, or a program outside it), as plain data. The port
/// knows how to find it again and stop it (step 2 of docs/development/RADIO-EXTRACTION-PLAN.md).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Holder {
    /// "TALES is reading", "mpv is using the sound card".
    pub says: String,
    /// The key of an own player that announced itself, when it is one.
    pub own_key: Option<String>,
    /// The process id and shown name of a program outside IYAKKI, when it is one.
    pub outside: Option<(u32, String)>,
}

impl Holder {
    /// The question the dialog asks.
    pub fn question(&self, wanter: &str) -> String {
        format!("{}. Stop it so {wanter} can play?", self.says)
    }
    /// What the audit line says about the holder: its kind and name, nothing else (`own:tales`, `outside:mpv`).
    pub fn audit_kind(&self) -> String {
        match (&self.own_key, &self.outside) {
            (Some(k), _) => format!("own:{k}"),
            (None, Some((_, n))) => format!("outside:{n}"),
            _ => "unknown".to_string(),
        }
    }
}

/// The speaker question as the tile needs it: who is in the way, and stopping it on a thread of the port's own.
pub trait SoundGate {
    /// Whatever would be locked out by, or lock out, a new radio player now.
    fn conflict(&self) -> Option<Holder>;
    /// Stop the holder (an own player cleanly, a program outside by its owner of the open stream); `wait_secs` is how long an own player gets.
    /// The answer comes on the channel: the calm sentence, or why not.
    fn stop(&self, holder: &Holder, wait_secs: u64) -> std::sync::mpsc::Receiver<Result<String, String>>;
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RadioState {
    Idle,
    Connecting(String),
    Playing { station: String, title: String, kbps: u32 },
    Failed { station: String, why: String },
}

use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// What the player may use to carry on when a live station drops (F-RADIO-17): the person's current list (the stations the radio may
/// move on to, in order) and where to write the audit line of a switch (`action`, `subject`). An empty list means no switching.
#[derive(Clone, Default)]
pub struct Around {
    pub list: Vec<Station>,
    pub audit: Option<Arc<dyn Fn(&str, &str) + Send + Sync>>,
}

/// What a search answers: the stations found as `(name, address)`, and a note when the directory was partly out of reach.
pub type SearchAnswer = (Vec<(String, String)>, Option<String>);

pub trait RadioPort {
    // the player
    fn state(&self) -> RadioState;
    fn is_active(&self) -> bool;
    fn is_paused(&self) -> bool;
    fn cert_problem(&self) -> Option<CertProblem>;
    fn volume(&self) -> u8;
    fn set_volume(&self, v: u8);
    fn levels(&self) -> [f32; BANDS];
    /// Carry on after a pause. False when nothing was paused.
    fn resume(&self) -> bool;
    /// Pause. False when nothing was playing.
    fn pause(&self) -> bool;
    fn stop(&self);
    /// Start a station (the address was checked with [`RadioPort::allows_host`] first).
    fn play(&self, station: &Station);
    /// What the status line adds while the radio recovers ("Reconnecting 1 of 2", "Switched to NAME"), if anything.
    fn notice(&self) -> Option<String> {
        None
    }
    /// May this account reach the host of a station?
    fn allows_host(&self, host: &str) -> bool;
    // the person's own list
    fn mine(&self) -> Vec<Station>;
    fn add(&self, name: &str, url: &str) -> Result<Station, String>;
    fn remove(&self, name: &str) -> Result<(), String>;
    fn trust(&self, base: &Station, problem: &CertProblem) -> Result<Station, String>;
    fn forget_trust(&self, name: &str) -> Result<Station, String>;
    /// Look for stations in the directory on a thread of its own; the answer comes on the channel.
    fn search(&self, words: &str) -> Receiver<SearchAnswer>;
    /// Is the audio add-on (the AAC decoder) in place?
    fn addon_installed(&self) -> bool;
    /// One audit line of the signed-in person.
    fn audit(&self, action: &str, subject: &str);
    /// Who holds the speaker, and stopping them (the registry of the players of this process, and the mixer switch).
    fn sound(&self) -> Arc<dyn SoundGate>;
    /// The preset stations by language (the data stays with the agent).
    fn languages(&self) -> &'static [Language];
    /// Does a station of the preset list need the audio add-on (AAC or HLS)?
    fn url_needs_addon(&self, url: &str) -> bool;
    /// Is this failure text the one that says the audio add-on is missing?
    fn failure_needs_addon(&self, why: &str) -> bool;
}

/// A port that does nothing, for the tiles that are not the radio (their `TileCx` carries it).
pub struct NoRadio;

struct NoSound;

impl SoundGate for NoSound {
    fn conflict(&self) -> Option<Holder> {
        None
    }
    fn stop(&self, _: &Holder, _: u64) -> Receiver<Result<String, String>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = tx.send(Err("nothing to stop".into()));
        rx
    }
}

impl RadioPort for NoRadio {
    fn state(&self) -> RadioState {
        RadioState::Idle
    }
    fn is_active(&self) -> bool {
        false
    }
    fn is_paused(&self) -> bool {
        false
    }
    fn cert_problem(&self) -> Option<CertProblem> {
        None
    }
    fn volume(&self) -> u8 {
        0
    }
    fn set_volume(&self, _: u8) {}
    fn levels(&self) -> [f32; BANDS] {
        [0.0; BANDS]
    }
    fn resume(&self) -> bool {
        false
    }
    fn pause(&self) -> bool {
        false
    }
    fn stop(&self) {}
    fn play(&self, _: &Station) {}
    fn allows_host(&self, _: &str) -> bool {
        false
    }
    fn mine(&self) -> Vec<Station> {
        Vec::new()
    }
    fn add(&self, _: &str, _: &str) -> Result<Station, String> {
        Err("no radio here".into())
    }
    fn remove(&self, _: &str) -> Result<(), String> {
        Err("no radio here".into())
    }
    fn trust(&self, _: &Station, _: &CertProblem) -> Result<Station, String> {
        Err("no radio here".into())
    }
    fn forget_trust(&self, _: &str) -> Result<Station, String> {
        Err("no radio here".into())
    }
    fn search(&self, _: &str) -> Receiver<SearchAnswer> {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = tx.send((Vec::new(), Some("no radio here".to_string())));
        rx
    }
    fn addon_installed(&self) -> bool {
        false
    }
    fn audit(&self, _: &str, _: &str) {}
    fn sound(&self) -> Arc<dyn SoundGate> {
        Arc::new(NoSound)
    }
    fn languages(&self) -> &'static [Language] {
        &[]
    }
    fn url_needs_addon(&self, _: &str) -> bool {
        false
    }
    fn failure_needs_addon(&self, _: &str) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_pins() {
        assert_eq!(date_text(0), "1970-01-01");
        assert_eq!(date_text(1_700_000_000), "2023-11-14");
        let p = CertProblem { station: "s".into(), url: "https://h:8443/x".into(), host: "h".into(), port: 8443, changed: false, sha256: "AA:BB".into(), subject: String::new(), issuer: String::new(), not_after: 86400, trustable: true, message: String::new() };
        assert_eq!(p.pin(), CertPin { host: "h".into(), port: 8443, sha256: "AA:BB".into() });
        assert_eq!(p.expiry(), "1970-01-02");
    }

    #[test]
    fn host_and_fingerprint_helpers() {
        assert_eq!(host_of("https://Radio.Example.org:8443/live?x=1"), "radio.example.org");
        assert_eq!(host_of("http://[::1]:8000/a"), "::1");
        assert_eq!(norm_fingerprint("ab:cd:EF"), "ABCDEF");
        assert_eq!(fingerprint_groups("00:11:22:33:44:55:66:77:88:99"), "00112233 44556677 8899");
        assert_eq!(short_fingerprint("aa:bb:cc:dd:ee"), "AABBCCDD");
    }

    #[test]
    fn the_no_radio_port_does_nothing_and_says_so() {
        let r = NoRadio;
        assert_eq!(r.state(), RadioState::Idle);
        assert!(!r.is_active() && !r.allows_host("x") && r.mine().is_empty());
        assert!(r.add("a", "b").is_err());
        assert!(r.sound().conflict().is_none());
        assert_eq!(r.search("x").recv().unwrap().1.as_deref(), Some("no radio here"));
    }
}
