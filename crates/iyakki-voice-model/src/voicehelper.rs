//! The state of the voice helper (a browser page that speaks for IYAKKI); `iyakki_shared::voicehelper` re-exports it.



#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum HelperState {
    /// Listening; the browser has not connected yet.
    Waiting,
    Connected,
    /// Over, and why.
    Closed(String),
}
