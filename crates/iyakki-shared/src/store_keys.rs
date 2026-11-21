

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeyInfo {
    pub label: String,
    pub public: String,
    /// Groups of four hex digits, for people to compare.
    pub fingerprint: String,
    pub default: bool,
}

impl KeyInfo {
    /// The line an administrator adds to the store's `trusted-keys.txt`.
    pub fn admin_line(&self) -> String {
        format!("{} {}", self.public, self.label)
    }
}
pub use ed25519_dalek::SigningKey;
