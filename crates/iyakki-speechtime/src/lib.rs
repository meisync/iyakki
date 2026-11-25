//! Word times of one piece of reading aloud (moved out of `iyakki_agent::speechtime`, F-SPLIT-03); the agent re-exports them.

use iyakki_wordtime as wordtime;

/// Where the word times of a piece come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TimingSource {
    /// The engine's own word times.
    Native,
    /// A speech-to-text model's word times, matched to the text.
    Aligned,
    /// The browser voice's boundary events.
    Voice,
    /// A model of speech fitted to the length of the sound (or to the speed).
    #[default]
    Estimated,
}

impl TimingSource {
    pub fn label(self) -> &'static str {
        match self {
            TimingSource::Native => "native",
            TimingSource::Aligned => "aligned",
            TimingSource::Voice => "from the voice",
            TimingSource::Estimated => "estimated",
        }
    }
}

/// Word times of one piece: where each word of its text starts (milliseconds), then the end of the sound.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Timed {
    pub begin: Vec<f32>,
    pub source: TimingSource,
}

impl Timed {
    /// The timeline of `text` with these times (None when they do not fit its words).
    pub fn timeline(&self, text: &str) -> Option<wordtime::WordTimeline> {
        wordtime::WordTimeline::with_starts(text, self.begin.clone())
    }
}

use std::net::IpAddr;

/// Where a speech server runs, as far as privacy goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Locality {
    ThisComputer,
    Network,
    Internet,
}

impl Locality {
    pub fn label(self) -> &'static str {
        match self {
            Locality::ThisComputer => "on this computer",
            Locality::Network => "on the network",
            Locality::Internet => "on the internet",
        }
    }
}

/// Where the server at `url` is: this computer (loopback, `localhost`), the local network (private and link-local
/// addresses, `.local`, `.lan`, `.home.arpa`, names without a dot) or the internet (everything else).
pub fn locality(url: &str) -> Locality {
    let host = iyakki_sec::url_host(url).unwrap_or_default().to_ascii_lowercase();
    let host = host.trim_start_matches('[').trim_end_matches(']').to_string();
    if host == "localhost" || host.ends_with(".localhost") {
        return Locality::ThisComputer;
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        return match ip {
            IpAddr::V4(v) if v.is_loopback() || v.is_unspecified() => Locality::ThisComputer,
            IpAddr::V4(v) if v.is_private() || v.is_link_local() || (v.octets()[0] == 100 && (64..128).contains(&v.octets()[1])) => Locality::Network,
            IpAddr::V6(v) if v.is_loopback() || v.is_unspecified() => Locality::ThisComputer,
            IpAddr::V6(v) if (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80 => Locality::Network,
            _ => Locality::Internet,
        };
    }
    if host.is_empty() || !host.contains('.') || [".local", ".lan", ".home.arpa", ".internal", ".home"].iter().any(|s| host.ends_with(s)) {
        return Locality::Network;
    }
    Locality::Internet
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_have_distinct_labels_and_default_is_estimated() {
        let all = [TimingSource::Native, TimingSource::Aligned, TimingSource::Voice, TimingSource::Estimated];
        let labels: std::collections::HashSet<_> = all.iter().map(|s| s.label()).collect();
        assert_eq!(labels.len(), 4);
        assert_eq!(TimingSource::default(), TimingSource::Estimated);
    }

    #[test]
    fn timed_round_trips_through_json_and_refuses_a_wrong_word_count() {
        let t = Timed { begin: vec![0.0, 400.0, 900.0], source: TimingSource::Aligned };
        let back: Timed = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back, t);
        assert!(t.timeline("one two three four five").is_none());
    }

    #[test]
    fn locality_tells_this_computer_the_network_and_the_internet_apart() {
        assert_eq!(locality("http://localhost:8080/v1"), Locality::ThisComputer);
        assert_eq!(locality("http://127.0.0.1:1234"), Locality::ThisComputer);
        assert_eq!(locality("http://192.168.1.5:8000"), Locality::Network);
        assert_eq!(locality("http://box.local/"), Locality::Network);
        assert_eq!(locality("https://api.example.com/v1"), Locality::Internet);
        assert_eq!(Locality::ThisComputer.label(), "on this computer");
    }
}
