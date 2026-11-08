//! Mutual TLS for the domain listener: a route whose host is named in [`Options::client_ca`] accepts only clients that present a
//! certificate signed by that route's CA.
//!
//! One rustls configuration serves every host, so the handshake asks for a client certificate with one verifier that trusts the union of
//! all the CAs and lets a client with no certificate through (that is the plain routes' case). The decision per route is made after the
//! handshake, in [`verdict`], against that route's own CA only: a certificate of the CA of route A is refused on route B. The handshake
//! has already proved the client holds the certificate's private key. Nothing about the certificate is passed to the target.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use rustls::client::danger::HandshakeSignatureValid;
use rustls::pki_types::{pem::PemObject, CertificateDer, UnixTime};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::server::WebPkiClientVerifier;
use rustls::{DigitallySignedStruct, DistinguishedName, RootCertStore, SignatureScheme};

use super::{Inner, Sock};
use crate::cloudroute::{Route, RouteTable, Scheme};

#[derive(Default)]
struct Cas {
    /// Per route host: its own verifier and the CA certificates behind it.
    per_host: HashMap<String, (Arc<dyn ClientCertVerifier>, Vec<CertificateDer<'static>>)>,
    /// The handshake's verifier: every CA of every host, unauthenticated clients allowed.
    union: Option<Arc<dyn ClientCertVerifier>>,
}

#[derive(Default)]
pub(super) struct AuthState(RwLock<Cas>);

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

fn build(roots: &[CertificateDer<'static>]) -> Result<Arc<dyn ClientCertVerifier>, String> {
    let mut store = RootCertStore::empty();
    for c in roots {
        store.add(c.clone()).map_err(|_| "a certificate of the CA file cannot be used as a trust anchor".to_string())?;
    }
    WebPkiClientVerifier::builder_with_provider(Arc::new(store), provider()).allow_unauthenticated().build().map_err(|e| format!("the client CA is not usable: {e}"))
}

fn parse_ca(pem: &str) -> Result<Vec<CertificateDer<'static>>, String> {
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(pem.as_bytes()).collect::<Result<_, _>>().map_err(|e| format!("the client CA is not PEM: {e}"))?;
    if certs.is_empty() {
        return Err("no certificate found in the client CA (expected -----BEGIN CERTIFICATE-----)".into());
    }
    Ok(certs)
}

/// The CA name of every host that needs a client certificate: the command line's (`Options::client_ca`) and the route file's
/// (`Route::client_ca`). When both name one for a host the command line wins. Hosts are lower case.
pub(super) fn wanted(inner: &Inner, t: &RouteTable) -> std::collections::BTreeMap<String, String> {
    let mut m = std::collections::BTreeMap::new();
    for r in t.routes.iter().filter(|r| r.scheme == Scheme::Https) {
        if let Some(n) = &r.client_ca {
            m.entry(r.host.to_ascii_lowercase()).or_insert_with(|| n.clone());
        }
    }
    for (h, n) in &inner.opts.client_ca {
        m.insert(h.to_ascii_lowercase(), n.clone());
    }
    m
}

/// Load the CAs of all mutual-TLS routes of `t` again. A CA that fails to load keeps the previous one of that host; a host that never had
/// one has no verifier, so its route refuses every client (503). Returns one line per host.
pub(super) fn reload_cas(inner: &Inner, t: &RouteTable) -> Vec<String> {
    let mut lines = Vec::new();
    let mut fresh: HashMap<String, (Arc<dyn ClientCertVerifier>, Vec<CertificateDer<'static>>)> = HashMap::new();
    let old = inner.auth.0.read().unwrap_or_else(|e| e.into_inner());
    for (host, name) in &wanted(inner, t) {
        if !t.routes.iter().any(|r| r.scheme == Scheme::Https && r.host.eq_ignore_ascii_case(host)) {
            continue;
        }
        match inner.source.load_client_ca(name).and_then(|pem| parse_ca(&pem)).and_then(|certs| Ok((build(&certs)?, certs))) {
            Ok(v) => {
                lines.push(format!("client CA {host}: {} certificate(s) loaded", v.1.len()));
                fresh.insert(host.to_ascii_lowercase(), v);
            }
            Err(e) => match old.per_host.get(&host.to_ascii_lowercase()) {
                Some(o) => {
                    lines.push(format!("client CA {host}: new CA refused ({e}); the previous one stays"));
                    fresh.insert(host.to_ascii_lowercase(), o.clone());
                }
                None => lines.push(format!("client CA {host}: refused, every client of this route is turned away ({e})")),
            },
        }
    }
    drop(old);
    let all: Vec<CertificateDer<'static>> = fresh.values().flat_map(|(_, c)| c.iter().cloned()).collect();
    let union = if all.is_empty() { None } else { build(&all).ok() };
    *inner.auth.0.write().unwrap_or_else(|e| e.into_inner()) = Cas { per_host: fresh, union };
    lines
}

/// `None`: let the request through. `Some((status, why))`: refuse it. Called for requests on TLS connections only.
pub(super) fn verdict(inner: &Inner, route: &Route, sock: &dyn Sock) -> Option<(u16, &'static str)> {
    let host = route.host.to_ascii_lowercase();
    // the CA belongs to the host: a second route of the same host (another path) asks for it too, so a path cannot be a way round
    if !wanted(inner, &inner.table()).contains_key(&host) {
        return None;
    }
    let verifier = inner.auth.0.read().unwrap_or_else(|e| e.into_inner()).per_host.get(&host).map(|(v, _)| v.clone());
    let Some(verifier) = verifier else {
        return Some((503, "the client CA of this route could not be loaded, so no client can be accepted"));
    };
    let certs = sock.peer_certs();
    let Some((first, rest)) = certs.split_first() else {
        return Some((403, "this route needs a client certificate"));
    };
    let rest: Vec<CertificateDer<'_>> = rest.iter().map(|c| CertificateDer::from(c.as_slice())).collect();
    match verifier.verify_client_cert(&CertificateDer::from(first.as_slice()), &rest, UnixTime::now()) {
        Ok(_) => None,
        Err(_) => Some((403, "the client certificate is not accepted by this route")),
    }
}

/// The header the target reads to know which client certificate was accepted: the SHA-256 of its DER, in lower-case hex. Empty when the
/// connection presented none.
pub(super) fn identity(sock: &dyn Sock) -> Option<String> {
    use sha2::{Digest, Sha256};
    let certs = sock.peer_certs();
    let first = certs.first()?;
    Some(Sha256::digest(first).iter().map(|b| format!("{b:02x}")).collect())
}

/// The verifier the TLS configuration holds. It follows [`AuthState`], so CAs can be replaced without a restart.
pub(super) struct DynVerifier {
    state: Arc<AuthState>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl DynVerifier {
    pub(super) fn new(state: Arc<AuthState>) -> DynVerifier {
        DynVerifier { state, provider: provider() }
    }
}

impl std::fmt::Debug for DynVerifier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DynVerifier")
    }
}

impl ClientCertVerifier for DynVerifier {
    fn offer_client_auth(&self) -> bool {
        self.state.0.read().unwrap_or_else(|e| e.into_inner()).union.is_some()
    }
    fn client_auth_mandatory(&self) -> bool {
        false
    }
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(&self, end: &CertificateDer<'_>, inter: &[CertificateDer<'_>], now: UnixTime) -> Result<ClientCertVerified, rustls::Error> {
        let u = self.state.0.read().unwrap_or_else(|e| e.into_inner()).union.clone();
        match u {
            Some(v) => v.verify_client_cert(end, inter, now),
            None => Err(rustls::Error::General("no client CA is configured".into())),
        }
    }
    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}
