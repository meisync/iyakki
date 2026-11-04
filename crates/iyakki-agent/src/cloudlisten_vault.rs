//! A [`CertSource`] that reads certificates from the vault (docs/development/CLOUD-MULTITENANT-PLAN.md, sections 3b and 3c).
//!
//! A route's `CertRef::Vault(NAME)` is three secrets of one vault scope: `NAME-chain` (the PEM chain), `NAME-key` (the PEM private key)
//! and, for a mutual-TLS route, the client CA under the name given in `Options::client_ca` (the PEM of the CA). The secrets are read on
//! every load (the listener reloads on a timer or on request), so a rotation in the vault reaches new connections without a restart; the
//! listener keeps the last good certificate when a read or a check fails (a vault that is down, a half-written pair, an expired
//! replacement).
//!
//! Key material is never logged: an error says which secret failed and why the vault answered, never what the secret held. The reader
//! is a small trait so tests need no vault; [`Reader`] over a [`crate::cloudvaultd::Client`] is the real one (it is only a thin
//! adapter and is not exercised here against a live vault service).

use std::time::Duration;

use super::CertSource;
use crate::cloudroute::CertRef;
use crate::cloudstorage::plain_name;

/// Reads one named secret as bytes. `Err` is a message that never contains the secret.
pub trait SecretReader: Send + Sync {
    fn read(&self, name: &str) -> Result<Vec<u8>, String>;
}

/// The vault client of a member (or the operator) and the scope the certificates live in.
pub struct Reader {
    pub client: crate::cloudvaultd::Client,
    pub scope: crate::cloudvaultd::Scope,
}

impl SecretReader for Reader {
    fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        self.client.get(&self.scope, name)
    }
}

pub struct VaultCerts {
    reader: Box<dyn SecretReader>,
}

impl VaultCerts {
    pub fn new(reader: Box<dyn SecretReader>) -> VaultCerts {
        VaultCerts { reader }
    }

    fn text(&self, secret: &str) -> Result<String, String> {
        let b = self.reader.read(secret).map_err(|e| format!("vault secret \"{secret}\": {e}"))?;
        String::from_utf8(b).map_err(|_| format!("vault secret \"{secret}\" is not text (PEM expected)"))
    }
}

impl std::fmt::Debug for VaultCerts {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("VaultCerts")
    }
}

impl CertSource for VaultCerts {
    fn load(&self, host: &str, cert: &CertRef) -> Result<(String, String), String> {
        let CertRef::Vault(name) = cert else {
            return Err(format!("{host}: a Kubernetes Secret is not in the vault; give the route a vault certificate name"));
        };
        if !plain_name(name) {
            return Err(format!("\"{name}\" is not a vault name"));
        }
        // both are read before either is used: a half-rotated pair is caught by the listener's key check
        let chain = self.text(&format!("{name}-chain"))?;
        let key = self.text(&format!("{name}-key"))?;
        Ok((chain, key))
    }

    fn load_client_ca(&self, name: &str) -> Result<String, String> {
        if !plain_name(name) {
            return Err(format!("\"{name}\" is not a vault name"));
        }
        self.text(name)
    }
}

/// How long the real vault is given per secret (the client has its own 15 s limit; this is documentation for callers that wrap it).
pub const READ_TIMEOUT: Duration = Duration::from_secs(15);
