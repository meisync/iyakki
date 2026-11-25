//! One line of a `known_hosts` file and its fingerprint (moved out of `iyakki_shared::ssh`, which re-exports it).

use sha2::{Digest, Sha256};

/// One line of a `known_hosts` file.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostKey {
    /// `host`, or `[host]:port` when the port is not 22.
    pub pattern: String,
    pub kind: String,
    pub b64: String,
}

impl HostKey {
    /// `SHA256:…`, as OpenSSH prints it.
    pub fn fingerprint(&self) -> String {
        let blob = iyakki_k8s_model::b64::b64_decode(&self.b64).unwrap_or_default();
        let digest = Sha256::digest(&blob);
        format!("SHA256:{}", iyakki_k8s_model::b64::base64_encode(&digest).trim_end_matches('='))
    }

    pub fn line(&self) -> String {
        format!("{} {} {}", self.pattern, self.kind, self.b64)
    }
}
