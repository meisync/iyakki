//! The certificate and key the owner gives on the command line, as a rustls server. The key is read once, never printed and never
//! written anywhere; every error here names the file and what is wrong with it, not its content.

use std::sync::Arc;

use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::ServerConfig;

/// Build the server configuration from PEM text (a chain, leaf first, and one private key).
pub fn config_from_pem(chain_pem: &str, key_pem: &str) -> Result<Arc<ServerConfig>, String> {
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(chain_pem.as_bytes()).collect::<Result<_, _>>().map_err(|_| "the certificate file is not PEM".to_string())?;
    if chain.is_empty() {
        return Err("no certificate found in the certificate file (expected -----BEGIN CERTIFICATE-----)".into());
    }
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(|_| "the key file holds no private key in PEM".to_string())?;
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(chain, key)
        .map_err(|_| "the certificate and the key do not belong together, or the key type is not supported".to_string())?;
    Ok(Arc::new(cfg))
}

/// Read the two files and build the configuration. On Unix a key readable by group or others is refused.
pub fn config_from_files(cert: &str, key: &str) -> Result<Arc<ServerConfig>, String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(key).map_err(|e| format!("cannot read the key file: {e}"))?.permissions().mode();
        if mode & 0o077 != 0 {
            return Err("the key file can be read by others: run chmod 600 on it".into());
        }
    }
    let c = std::fs::read_to_string(cert).map_err(|e| format!("cannot read the certificate file: {e}"))?;
    let k = std::fs::read_to_string(key).map_err(|e| format!("cannot read the key file: {e}"))?;
    config_from_pem(&c, &k)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn garbage_and_mismatches_are_named_without_content() {
        let e = config_from_pem("nothing", "nothing").unwrap_err();
        assert!(e.contains("no certificate"), "{e}");
        let e = config_from_pem("-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n", "secret-bytes-here").unwrap_err();
        assert!(!e.contains("secret-bytes-here"));
    }

    #[test]
    fn missing_files_say_which_kind() {
        let e = config_from_files("/nonexistent/c.pem", "/nonexistent/k.pem").unwrap_err();
        assert!(e.contains("key file") || e.contains("certificate file"), "{e}");
    }
}
