//! The certificate checks of the routes (rustls and rcgen live here, below the shared route table).

use iyakki_shared::cloudroute::{CertInfo, WARN_DAYS, name_covers, read_cert};

/// A certificate chain and private key (PEM), for `host`, at `now` (Unix seconds). Returns what it found and the line to show (a warning
/// when it runs out within [`WARN_DAYS`]).
pub fn check_certificate(chain_pem: &str, key_pem: &str, host: &str, now: i64) -> Result<(CertInfo, String), String> {
    use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(chain_pem.as_bytes()).collect::<Result<_, _>>().map_err(|e| format!("the certificate is not PEM: {e}"))?;
    let Some(first) = chain.first() else { return Err("no certificate found in the file (expected -----BEGIN CERTIFICATE-----)".into()) };
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(|_| "the private key is not PEM (expected -----BEGIN PRIVATE KEY-----)".to_string())?;
    let signer = rustls::crypto::ring::sign::any_supported_type(&key).map_err(|_| "the private key type is not supported (use RSA, ECDSA P-256/P-384 or Ed25519)".to_string())?;
    let certified = rustls::sign::CertifiedKey::new(chain.clone(), signer);
    certified.keys_match().map_err(|_| "the private key does not belong to this certificate".to_string())?;
    let info = read_cert(first.as_ref()).ok_or("the certificate could not be read")?;
    if info.names.is_empty() {
        return Err("the certificate has no Subject Alternative Names; browsers need them".into());
    }
    if !info.names.iter().any(|n| name_covers(n, host)) {
        return Err(format!("the certificate covers {} and not {host}", info.names.join(", ")));
    }
    if now < info.not_before {
        return Err("the certificate is not valid yet".into());
    }
    if now >= info.not_after {
        return Err("the certificate has expired".into());
    }
    let days = (info.not_after - now) / 86400;
    let line = if days <= WARN_DAYS { format!("valid, but runs out in {days} day(s): renew it soon") } else { format!("valid for {days} more days") };
    Ok((info, line))
}

/// The chain and key as something rustls can serve with (the listener, `cloudlisten.rs`). Call [`check_certificate`] first: this only
/// parses. The errors never contain key material.
pub fn load_certified(chain_pem: &str, key_pem: &str) -> Result<rustls::sign::CertifiedKey, String> {
    use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
    let chain: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(chain_pem.as_bytes()).collect::<Result<_, _>>().map_err(|e| format!("the certificate is not PEM: {e}"))?;
    let key = PrivateKeyDer::from_pem_slice(key_pem.as_bytes()).map_err(|_| "the private key is not PEM".to_string())?;
    let signer = rustls::crypto::ring::sign::any_supported_type(&key).map_err(|_| "the private key type is not supported".to_string())?;
    Ok(rustls::sign::CertifiedKey::new(chain, signer))
}

/// A self-signed certificate and key for a local trial: (chain PEM, key PEM). Marked as such by the caller; browsers will not trust it.
pub fn self_signed(host: &str) -> Result<(String, String), String> {
    if !iyakki_shared::cloudroute::wildcard_or_host(host) && host != "localhost" {
        return Err(format!("\"{host}\" is not a host name"));
    }
    let c = rcgen::generate_simple_self_signed(vec![host.to_string()]).map_err(|e| e.to_string())?;
    Ok((c.cert.pem(), c.key_pair.serialize_pem()))
}

