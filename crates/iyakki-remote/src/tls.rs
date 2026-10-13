//! HTTPS: the server's certificate. One is made for this machine the first time (self-signed, kept next to the data) unless
//! the administrator has put their own `cert.pem` and `key.pem` in that folder.

use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;

pub struct Identity {
    pub config: Arc<rustls::ServerConfig>,
    /// SHA-256 of the certificate, to compare in the browser's warning page.
    pub fingerprint: String,
    pub self_signed: bool,
}

pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// Load `cert.pem` + `key.pem` from `dir`, or make a self-signed pair for `names` and keep it there.
pub fn identity(dir: &Path, names: &[String]) -> Result<Identity, String> {
    let (cert_path, key_path) = (dir.join("cert.pem"), dir.join("key.pem"));
    let mut self_signed = false;
    if !cert_path.exists() || !key_path.exists() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut names: Vec<String> = names.to_vec();
        for n in ["localhost", "iyakki"] {
            if !names.iter().any(|x| x == n) {
                names.push(n.into());
            }
        }
        let cert = rcgen::generate_simple_self_signed(names).map_err(|e| format!("could not make a certificate: {e}"))?;
        std::fs::write(&cert_path, cert.cert.pem()).map_err(|e| format!("{}: {e}", cert_path.display()))?;
        write_private(&key_path, cert.key_pair.serialize_pem().as_bytes())?;
        self_signed = true;
    }
    let cert_pem = std::fs::read(&cert_path).map_err(|e| format!("{}: {e}", cert_path.display()))?;
    let key_pem = std::fs::read(&key_path).map_err(|e| format!("{}: {e}", key_path.display()))?;
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&cert_pem).collect::<Result<_, _>>().map_err(|e| format!("cert.pem: {e}"))?;
    let key = PrivateKeyDer::from_pem_slice(&key_pem).map_err(|e| format!("key.pem: {e}"))?;
    let first = certs.first().ok_or("cert.pem has no certificate")?;
    let fp = fingerprint(first);
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("certificate and key do not fit: {e}"))?;
    Ok(Identity { config: Arc::new(config), fingerprint: fp, self_signed })
}

fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| f.write_all(data))
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_certificate_is_made_once_kept_and_reloaded_with_the_same_fingerprint() {
        let dir = std::env::temp_dir().join(format!("iyakki-tls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let a = identity(&dir, &["box.local".into()]).expect("made");
        assert!(a.self_signed && a.fingerprint.len() == 95);
        let b = identity(&dir, &[]).expect("reloaded");
        assert!(!b.self_signed);
        assert_eq!(a.fingerprint, b.fingerprint);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(dir.join("key.pem")).unwrap().permissions().mode() & 0o777, 0o600, "the key is private");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_key_file_is_reported_not_ignored() {
        let dir = std::env::temp_dir().join(format!("iyakki-tls-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("cert.pem"), "nonsense").unwrap();
        std::fs::write(dir.join("key.pem"), "nonsense").unwrap();
        assert!(identity(&dir, &[]).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
