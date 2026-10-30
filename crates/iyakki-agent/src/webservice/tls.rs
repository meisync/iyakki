//! The service's certificate: the administrator's own (`tls.cert` and `tls.key` PEM files), or a self-signed one made the first
//! time under `<data_dir>/service/tls` and kept, whose SHA-256 fingerprint is logged and shown by `iyakki serve --print-fingerprint`
//! so a client can pin it. A broken file is an error, never a silent fall back to another certificate.

use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::sync::Arc;

pub struct Identity {
    pub config: Arc<rustls::ServerConfig>,
    /// SHA-256 of the certificate, `AA:BB:..`.
    pub fingerprint: String,
    pub self_signed: bool,
}

pub fn fingerprint(der: &[u8]) -> String {
    Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

fn build(cert_pem: &[u8], key_pem: &[u8], self_signed: bool) -> Result<Identity, String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(cert_pem).collect::<Result<_, _>>().map_err(|e| format!("certificate: {e}"))?;
    let key = PrivateKeyDer::from_pem_slice(key_pem).map_err(|e| format!("key: {e}"))?;
    let first = certs.first().ok_or("the certificate file has no certificate")?;
    let fp = fingerprint(first);
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| format!("certificate and key do not fit: {e}"))?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Identity { config: Arc::new(config), fingerprint: fp, self_signed })
}

/// The administrator's certificate and key.
pub fn from_files(cert: &Path, key: &Path) -> Result<Identity, String> {
    let c = std::fs::read(cert).map_err(|e| format!("{}: {e}", cert.display()))?;
    let k = std::fs::read(key).map_err(|e| format!("{}: {e}", key.display()))?;
    build(&c, &k, false)
}

/// A self-signed certificate kept in `dir` (made on first use, key mode 0600).
pub fn self_signed(dir: &Path, names: &[String]) -> Result<Identity, String> {
    let (cp, kp) = (dir.join("cert.pem"), dir.join("key.pem"));
    let mut made = false;
    if !cp.exists() || !kp.exists() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut n: Vec<String> = names.to_vec();
        for x in ["localhost", "iyakki"] {
            if !n.iter().any(|y| y == x) {
                n.push(x.into());
            }
        }
        let cert = rcgen::generate_simple_self_signed(n).map_err(|e| format!("could not make a certificate: {e}"))?;
        std::fs::write(&cp, cert.cert.pem()).map_err(|e| format!("{}: {e}", cp.display()))?;
        write_private(&kp, cert.key_pair.serialize_pem().as_bytes())?;
        made = true;
    }
    let c = std::fs::read(&cp).map_err(|e| format!("{}: {e}", cp.display()))?;
    let k = std::fs::read(&kp).map_err(|e| format!("{}: {e}", kp.display()))?;
    build(&c, &k, made)
}

fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path).and_then(|mut f| f.write_all(data)).map_err(|e| format!("{}: {e}", path.display()))
}
