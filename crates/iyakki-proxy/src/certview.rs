//! The certificate details view (F-BROW-98): what a person needs to see about the certificate a site showed, in words: who it was
//! issued to, who issued it, when it is valid, its fingerprint, whether its names fit the address, and whether it chains to a
//! trusted authority. The certificates come from a handshake of their own ([`crate::pintls::peer_chain`]), which sends nothing but
//! the handshake; the trust check is the same one the browser's transport uses (the web's public roots).

use crate::certinfo;
use std::sync::Arc;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub host: String,
    pub subject: String,
    pub issuer: String,
    pub not_before: String,
    pub not_after: String,
    /// Negative when it ran out.
    pub days_left: i64,
    pub sha256: String,
    pub names_match: bool,
    /// `Ok` when the chain leads to a trusted authority at the given time, else why not.
    pub trust: Result<(), String>,
    pub sent: usize,
    pub self_issued: bool,
    pub warnings: Vec<String>,
}

/// The public roots the browser trusts.
pub fn public_roots() -> rustls::RootCertStore {
    rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() }
}

/// Reads a chain (leaf first) into a view. `now` is seconds since 1970.
pub fn view(host: &str, chain: &[Vec<u8>], now: i64, roots: rustls::RootCertStore) -> Result<View, String> {
    use rustls::client::danger::ServerCertVerifier;
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    let leaf = chain.first().ok_or("the site showed no certificate")?;
    let info = certinfo::parse(leaf).map_err(|e| format!("the certificate could not be read ({e})"))?;
    let names_match = crate::pintls::names_match(leaf, host);
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let trust = (|| -> Result<(), String> {
        let name = ServerName::try_from(host.to_string()).map_err(|e| e.to_string())?;
        let v = rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider).build().map_err(|e| e.to_string())?;
        let ee = CertificateDer::from(leaf.clone());
        let rest: Vec<CertificateDer> = chain[1..].iter().map(|c| CertificateDer::from(c.clone())).collect();
        v.verify_server_cert(&ee, &rest, &name, &[], UnixTime::since_unix_epoch(std::time::Duration::from_secs(now.max(0) as u64))).map(|_| ()).map_err(|e| e.to_string())
    })();
    let days_left = (info.not_after - now).div_euclid(86_400);
    let mut warnings = Vec::new();
    if now > info.not_after {
        warnings.push(format!("It ran out on {}.", certinfo::date_text(info.not_after)));
    } else if now < info.not_before {
        warnings.push(format!("It is not valid before {}.", certinfo::date_text(info.not_before)));
    } else if days_left < 14 {
        warnings.push(format!("It runs out in {days_left} days."));
    }
    if !names_match {
        warnings.push(format!("Its names do not include {host}."));
    }
    if let Err(e) = &trust {
        warnings.push(format!("It does not lead to a trusted authority ({e})."));
    }
    if info.self_issued {
        warnings.push("It was issued by itself, not by an authority.".into());
    }
    Ok(View { host: host.to_string(), subject: info.subject, issuer: info.issuer, not_before: certinfo::date_text(info.not_before), not_after: certinfo::date_text(info.not_after), days_left, sha256: info.sha256, names_match, trust, sent: chain.len(), self_issued: info.self_issued, warnings })
}

impl View {
    pub fn is_clean(&self) -> bool {
        self.warnings.is_empty()
    }

    /// The view as lines of text.
    pub fn text(&self) -> String {
        let mut o = vec![
            format!("Certificate shown by {}", self.host),
            format!("Issued to: {}", self.subject),
            format!("Issued by: {}", self.issuer),
            format!("Valid: {} to {} ({})", self.not_before, self.not_after, if self.days_left < 0 { "expired".to_string() } else { format!("{} days left", self.days_left) }),
            format!("Fingerprint (SHA-256): {}", self.sha256),
            format!("Names fit the address: {}", if self.names_match { "yes" } else { "NO" }),
            format!("Trusted authority: {}", match &self.trust { Ok(()) => "yes".to_string(), Err(e) => format!("NO ({e})") }),
            format!("Certificates sent: {}", self.sent),
        ];
        if self.warnings.is_empty() {
            o.push("Nothing looks wrong with it.".into());
        } else {
            o.push("Warnings:".into());
            o.extend(self.warnings.iter().map(|w| format!("- {w}")));
        }
        o.join("\n")
    }
}

/// The certificate the browser's own connection to `host:port` was given, read like [`probe`]'s answer; `None` when the browser has not
/// completed a handshake with that site over its in-house HTTPS client (see [`crate::certseen`]).
pub fn seen(host: &str, port: u16, now: i64) -> Option<Result<View, String>> {
    crate::certseen::latest(host, port).map(|chain| view(host, &chain, now, public_roots()))
}

/// Asks the site at `host:port` for its certificates and reads them (the real network).
pub fn probe(host: &str, port: u16, now: i64) -> Result<View, String> {
    let chain = crate::pintls::peer_chain(host, port)?;
    view(host, &chain, now, public_roots())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rcgen::{BasicConstraints, CertificateParams, DnType, IsCa, KeyPair};
    use rustls::pki_types::CertificateDer;

    fn made(names: &[&str], years: Option<(i32, i32)>) -> (Vec<u8>, Vec<u8>) {
        let mut cp = CertificateParams::new(vec![]).unwrap();
        cp.distinguished_name.push(DnType::CommonName, "Test Root");
        cp.is_ca = IsCa::Ca(BasicConstraints::Unconstrained);
        let ck = KeyPair::generate().unwrap();
        let ca = cp.self_signed(&ck).unwrap();
        let mut lp = CertificateParams::new(names.iter().map(|s| s.to_string()).collect::<Vec<_>>()).unwrap();
        lp.distinguished_name.push(DnType::CommonName, names[0]);
        if let Some((a, b)) = years {
            lp.not_before = rcgen::date_time_ymd(a, 1, 1);
            lp.not_after = rcgen::date_time_ymd(b, 1, 1);
        }
        let lk = KeyPair::generate().unwrap();
        let leaf = lp.signed_by(&lk, &ca, &ck).unwrap();
        (leaf.der().to_vec(), ca.der().to_vec())
    }

    fn roots(ca: &[u8]) -> rustls::RootCertStore {
        let mut r = rustls::RootCertStore::empty();
        r.add(CertificateDer::from(ca.to_vec())).unwrap();
        r
    }

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn a_good_certificate_is_described_and_nothing_looks_wrong() {
        let (leaf, ca) = made(&["shop.example"], Some((2026, 2030)));
        let v = view("shop.example", &[leaf.clone()], NOW, roots(&ca)).unwrap();
        assert!(v.is_clean(), "{:?}", v.warnings);
        assert!(v.names_match && v.trust.is_ok());
        assert_eq!(v.sha256, certinfo::fingerprint(&leaf));
        assert!(v.subject.contains("shop.example") && v.issuer.contains("Test Root"));
        assert!(v.days_left > 300);
        let t = v.text();
        assert!(t.contains("Issued to:") && t.contains("Trusted authority: yes") && t.contains("Nothing looks wrong"), "{t}");
    }

    #[test]
    fn a_wrong_name_an_unknown_authority_and_an_expired_certificate_each_warn() {
        let (leaf, ca) = made(&["shop.example"], Some((2026, 2030)));
        let v = view("other.example", &[leaf.clone()], NOW, roots(&ca)).unwrap();
        assert!(!v.names_match && v.warnings.iter().any(|w| w.contains("names do not include other.example")));
        let v = view("shop.example", &[leaf.clone()], NOW, rustls::RootCertStore::empty()).unwrap();
        assert!(v.trust.is_err() && v.warnings.iter().any(|w| w.contains("trusted authority")));
        assert!(v.text().contains("Trusted authority: NO"));
        let (old, ca2) = made(&["shop.example"], Some((2000, 2001)));
        let v = view("shop.example", &[old], NOW, roots(&ca2)).unwrap();
        assert!(v.days_left < 0 && v.warnings.iter().any(|w| w.contains("ran out")), "{:?}", v.warnings);
        assert!(v.text().contains("expired"));
        let (soon, ca3) = made(&["shop.example"], Some((2026, 2027)));
        let v = view("shop.example", &[soon], 1_798_000_000, roots(&ca3)).unwrap();
        assert!(v.days_left >= 0 && v.warnings.iter().any(|w| w.contains("runs out in")), "{:?}", v.warnings);
    }

    #[test]
    fn nothing_or_garbage_is_an_error_not_a_panic() {
        assert!(view("a.example", &[], NOW, rustls::RootCertStore::empty()).is_err());
        assert!(view("a.example", &[vec![1, 2, 3]], NOW, rustls::RootCertStore::empty()).is_err());
    }

    #[test]
    fn a_self_issued_certificate_is_called_out() {
        let der = crate::testcerts::leaf_der(&["self.example"], None, None);
        let v = view("self.example", &[der], NOW, rustls::RootCertStore::empty()).unwrap();
        assert!(v.self_issued && v.warnings.iter().any(|w| w.contains("issued by itself")));
    }

    /// Against real servers (run by hand: `cargo test -p iyakki-proxy --lib live_cert -- --ignored`): the chain a real site shows is read,
    /// its names fit, it leads to the web's public roots and it is not about to run out.
    #[test]
    #[ignore = "needs the internet"]
    fn live_cert_real_sites_show_chains_that_read_and_verify() {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs() as i64;
        for host in ["www.wikipedia.org", "www.google.com", "www.cloudflare.com"] {
            let v = probe(host, 443, now).unwrap_or_else(|e| panic!("{host}: {e}"));
            assert!(v.trust.is_ok(), "{host}: {:?}", v.trust);
            assert!(v.names_match && v.days_left > 0 && v.sent >= 2, "{host}: {v:?}");
            assert_eq!(v.sha256.split(':').count(), 32, "{host}");
            eprintln!("{host}: issued by {}, {} days left, {} certificates", v.issuer, v.days_left, v.sent);
        }
        // a name that is not the site's: the same chain, flagged
        let chain = crate::pintls::peer_chain("www.wikipedia.org", 443).unwrap();
        let v = view("www.example.invalid", &chain, now, public_roots()).unwrap();
        assert!(!v.names_match && v.warnings.iter().any(|w| w.contains("names do not include")));
    }
}
