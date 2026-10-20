//! Who signed this, and how far do we trust them. Everything is offline: ed25519 over bytes already on the device, with
//! the pinned IYAKKI root keys (`official-roots.txt`), the administrator's own keys (`trusted-keys.txt`), publisher
//! certificates and the revocation list. See docs/STORE.md, section 2.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::manifest::{valid_publisher_id, Publisher};
use super::revocation::Revocations;
use crate::pkg::{hex, public_key_hex, unhex, Manifest};

// ------------------------------------------------------------------------------------------------ signing documents

/// The document as bytes everyone agrees on: keys sorted, no spaces, `sig` left out.
pub fn canonical(v: &Value) -> String {
    fn go(v: &Value, out: &mut String, top: bool) {
        match v {
            Value::Object(m) => {
                let mut keys: Vec<&String> = m.keys().filter(|k| !(top && *k == "sig")).collect();
                keys.sort();
                out.push('{');
                for (i, k) in keys.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    out.push_str(&Value::String((*k).clone()).to_string());
                    out.push(':');
                    go(&m[*k], out, false);
                }
                out.push('}');
            }
            Value::Array(a) => {
                out.push('[');
                for (i, x) in a.iter().enumerate() {
                    if i > 0 {
                        out.push(',');
                    }
                    go(x, out, false);
                }
                out.push(']');
            }
            other => out.push_str(&other.to_string()),
        }
    }
    let mut s = String::new();
    go(v, &mut s, true);
    s
}

fn signed_bytes(purpose: &str, doc: &Value) -> Vec<u8> {
    format!("IYAKKI-STORE {purpose} v1\n{}", canonical(doc)).into_bytes()
}

/// Sign a JSON document for `purpose` (`catalog`, `cert`, `revocations`, `license`); the signature is hex.
pub fn sign_doc(purpose: &str, doc: &Value, key: &SigningKey) -> String {
    hex(&key.sign(&signed_bytes(purpose, doc)).to_bytes())
}

pub fn verify_doc(purpose: &str, doc: &Value, key_hex: &str, sig_hex: &str) -> bool {
    let Some(k) = unhex::<32>(key_hex).and_then(|b| VerifyingKey::from_bytes(&b).ok()) else { return false };
    let Some(s) = unhex::<64>(sig_hex).map(|b| Signature::from_bytes(&b)) else { return false };
    k.verify(&signed_bytes(purpose, doc), &s).is_ok()
}

// ------------------------------------------------------------------------------------------------ roots

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RootKey {
    pub id: String,
    pub key: String,
    pub not_before: i64,
    pub not_after: i64,
}

/// The IYAKKI root keys a device trusts.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Roots(Vec<RootKey>);

impl Roots {
    /// The keys compiled into this build.
    pub fn pinned() -> Roots {
        Roots::parse(include_str!("official-roots.txt"))
    }

    pub fn parse(text: &str) -> Roots {
        Roots(
            text.lines()
                .filter_map(|l| {
                    let l = l.trim();
                    if l.is_empty() || l.starts_with('#') {
                        return None;
                    }
                    let f: Vec<&str> = l.split_whitespace().collect();
                    let (id, key) = (f.first()?, f.get(1)?);
                    unhex::<32>(key).and_then(|b| VerifyingKey::from_bytes(&b).ok())?;
                    Some(RootKey { id: id.to_string(), key: key.to_ascii_lowercase(), not_before: f.get(2).and_then(|x| x.parse().ok()).unwrap_or(0), not_after: f.get(3).and_then(|x| x.parse().ok()).unwrap_or(i64::MAX) })
                })
                .collect(),
        )
    }

    /// For tests and tools: one root, valid for ever.
    pub fn with(id: &str, key: &SigningKey) -> Roots {
        Roots(vec![RootKey { id: id.into(), key: public_key_hex(key), not_before: 0, not_after: i64::MAX }])
    }

    pub fn push(&mut self, r: RootKey) {
        self.0.push(r);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn list(&self) -> &[RootKey] {
        &self.0
    }

    /// The root with this key, if it is listed and valid at `now`.
    pub fn valid_at(&self, key: &str, now: i64) -> Option<&RootKey> {
        self.0.iter().find(|r| r.key == key && r.not_before <= now && now <= r.not_after)
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.iter().any(|r| r.key == key)
    }
}

// ------------------------------------------------------------------------------------------------ certificates

/// A publisher certificate: a root key's signature over who the publisher is, which key is theirs, for how long and for
/// which kinds of package.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Cert {
    pub v: u32,
    pub publisher: Publisher,
    pub key: String,
    pub not_before: i64,
    pub not_after: i64,
    pub scopes: Vec<String>,
    pub issuer: String,
    pub sig: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CertProblem {
    Malformed,
    UnknownIssuer,
    BadSignature,
    WrongKey,
    NotYetValid(i64),
    Expired(i64),
    OutOfScope(String),
}

impl CertProblem {
    pub fn words(&self) -> String {
        match self {
            CertProblem::Malformed => "the publisher certificate cannot be read".into(),
            CertProblem::UnknownIssuer => "the publisher certificate was not issued by a key this system trusts".into(),
            CertProblem::BadSignature => "the publisher certificate has a bad signature".into(),
            CertProblem::WrongKey => "the publisher certificate belongs to a different key".into(),
            CertProblem::NotYetValid(t) => format!("the publisher certificate is not valid before {}", date(*t)),
            CertProblem::Expired(t) => format!("the publisher certificate expired on {}", date(*t)),
            CertProblem::OutOfScope(k) => format!("the publisher certificate does not cover {k} packages"),
        }
    }
}

pub fn date(t: i64) -> String {
    let (y, m, d) = crate::clock::civil(t.div_euclid(86_400));
    format!("{y}-{m:02}-{d:02}")
}

impl Cert {
    pub fn issue(root: &SigningKey, publisher: Publisher, key: &str, not_before: i64, not_after: i64, scopes: &[&str]) -> Cert {
        let mut c = Cert { v: 1, publisher, key: key.into(), not_before, not_after, scopes: scopes.iter().map(|s| s.to_string()).collect(), issuer: public_key_hex(root), sig: String::new() };
        c.sig = sign_doc("cert", &serde_json::to_value(&c).unwrap_or_default(), root);
        c
    }

    pub fn parse(json: &str) -> Result<Cert, CertProblem> {
        let c: Cert = serde_json::from_str(json).map_err(|_| CertProblem::Malformed)?;
        if c.v != 1 || !valid_publisher_id(&c.publisher.id) || unhex::<32>(&c.key).is_none() {
            return Err(CertProblem::Malformed);
        }
        Ok(c)
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The signature and the issuer only (no dates, no key match): what makes it a certificate at all.
    pub fn authentic(&self, roots: &Roots) -> Result<(), CertProblem> {
        if !roots.contains(&self.issuer) {
            return Err(CertProblem::UnknownIssuer);
        }
        if !verify_doc("cert", &serde_json::to_value(self).unwrap_or_default(), &self.issuer, &self.sig) {
            return Err(CertProblem::BadSignature);
        }
        Ok(())
    }

    /// Whether this certificate vouches for `signer` publishing `kind` at `now`.
    pub fn vouches(&self, roots: &Roots, signer: &str, kind: &str, now: i64) -> Result<(), CertProblem> {
        self.authentic(roots)?;
        if self.key != signer {
            return Err(CertProblem::WrongKey);
        }
        if roots.valid_at(&self.issuer, now).is_none() {
            return Err(CertProblem::UnknownIssuer);
        }
        if now < self.not_before {
            return Err(CertProblem::NotYetValid(self.not_before));
        }
        if now > self.not_after {
            return Err(CertProblem::Expired(self.not_after));
        }
        if !self.scopes.iter().any(|s| s == kind || s == "*") {
            return Err(CertProblem::OutOfScope(kind.into()));
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------ levels and verdicts

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Level {
    Official,
    Verified,
    Trusted,
    Community,
    Unsigned,
    /// Installed from a paired developer machine in developer mode (`devmode`): never more trusted than that, never promoted.
    Developer,
    Revoked,
}

impl Level {
    pub const ALL: [Level; 7] = [Level::Official, Level::Verified, Level::Trusted, Level::Community, Level::Unsigned, Level::Developer, Level::Revoked];

    pub fn label(self) -> &'static str {
        match self {
            Level::Official => "Official",
            Level::Verified => "Verified",
            Level::Trusted => "Trusted",
            Level::Community => "Community",
            Level::Unsigned => "Unsigned",
            Level::Developer => "Developer (local)",
            Level::Revoked => "Revoked",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Level::Official => "official",
            Level::Verified => "verified",
            Level::Trusted => "trusted",
            Level::Community => "community",
            Level::Unsigned => "unsigned",
            Level::Developer => "developer",
            Level::Revoked => "revoked",
        }
    }

    /// What the badge means, in plain words (the tooltip and the line under the badge).
    pub fn plain(self) -> &'static str {
        match self {
            Level::Official => "Made by IYAKKI and signed with its release key.",
            Level::Verified => "The publisher's identity was checked by IYAKKI, which signed their key.",
            Level::Trusted => "Signed by a key your administrator chose to trust.",
            Level::Community => "Signed, but by a publisher nobody has vouched for. Read what it asks for.",
            Level::Unsigned => "Not signed: anyone could have written it. Only for people who know where it came from.",
            Level::Developer => "Installed from a paired developer computer in developer mode. Nobody reviewed it; it is never updated from a store.",
            Level::Revoked => "Withdrawn because of a problem. It cannot be installed and is turned off if it was.",
        }
    }

    /// Whether the person needs to accept a warning before it can be installed.
    pub fn needs_warning_ack(self) -> bool {
        matches!(self, Level::Community | Level::Unsigned | Level::Developer)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verdict {
    pub level: Level,
    pub signer: Option<String>,
    pub publisher: Option<Publisher>,
    /// One line: who vouches for it.
    pub label: String,
    /// The chain of trust, one sentence per link.
    pub chain: Vec<String>,
    pub warnings: Vec<String>,
    /// Set when the package must not be installed.
    pub refusal: Option<String>,
}

impl Verdict {
    fn new(level: Level, signer: Option<&str>) -> Verdict {
        Verdict { level, signer: signer.map(str::to_string), publisher: None, label: String::new(), chain: vec![], warnings: vec![], refusal: None }
    }

    /// Installed from a paired developer computer (`source` is `developer:<key name>`): labelled as such whatever key signed it, so a
    /// developer build is never shown as more trusted than it is, and never promoted later.
    pub fn developer(source: &str, signer: Option<&str>) -> Verdict {
        let who = source.strip_prefix(crate::devmode::SOURCE_PREFIX).unwrap_or(source);
        Verdict {
            level: Level::Developer,
            signer: signer.map(str::to_string),
            publisher: None,
            label: format!("installed in developer mode from the paired computer “{who}”"),
            chain: vec![format!("Sent by the paired developer computer “{who}”, which an administrator allowed in developer mode. Nobody reviewed it.")],
            warnings: vec!["Developer build: not reviewed, not from a store. It stays labelled Developer (local).".into()],
            refusal: None,
        }
    }

    /// Built into this IYAKKI image: it came with the system, so it is as trusted as the system.
    pub fn builtin() -> Verdict {
        Verdict { level: Level::Official, signer: None, publisher: Some(Publisher { id: "iyakki".into(), name: "IYAKKI".into(), url: String::new() }), label: "built into this IYAKKI image".into(), chain: vec!["It is part of the system image you booted, not something that came from the network.".into()], warnings: vec![], refusal: None }
    }
}

/// What a judgement needs to know.
pub struct Judge<'a> {
    pub roots: &'a Roots,
    /// The administrator's own keys: `(public key, label)`.
    pub own_keys: &'a [(String, String)],
    pub revocations: &'a Revocations,
    pub now: i64,
}

fn short(k: &str) -> String {
    format!("{}…", &k[..12.min(k.len())])
}

impl Judge<'_> {
    /// The trust level of a package whose hash and signature were already checked by `pkg::parse`.
    pub fn verdict(&self, m: &Manifest) -> Verdict {
        let cert = m.cert.as_deref().map(Cert::parse);
        let cert_ok = cert.as_ref().and_then(|c| c.as_ref().ok());
        let pub_of = |c: Option<&Cert>| c.map(|c| c.publisher.clone());
        let mut v = match &m.signer {
            None => {
                let mut v = Verdict::new(Level::Unsigned, None);
                v.label = "not signed".into();
                v.chain.push("There is no signature, so nothing says who wrote it.".into());
                v.warnings.push("Not signed: anyone could have written this.".into());
                v
            }
            Some(k) if self.roots.valid_at(k, self.now).is_some() => {
                let id = self.roots.valid_at(k, self.now).map(|r| r.id.clone()).unwrap_or_default();
                let mut v = Verdict::new(Level::Official, Some(k));
                v.publisher = Some(Publisher { id: "iyakki".into(), name: "IYAKKI".into(), url: String::new() });
                v.label = "signed by the IYAKKI release key".into();
                v.chain.push(format!("The signature is by the pinned IYAKKI root key {id} ({}).", short(k)));
                v
            }
            Some(k) if self.roots.contains(k) => {
                let mut v = Verdict::new(Level::Community, Some(k));
                v.label = format!("signed by an IYAKKI root key outside its validity window ({})", short(k));
                v.warnings.push("The IYAKKI key that signed this is not valid today (expired or not yet in use).".into());
                v
            }
            Some(k) => match self.own_keys.iter().find(|(t, _)| t == k) {
                Some((_, label)) => {
                    let mut v = Verdict::new(Level::Trusted, Some(k));
                    v.label = format!("signed by {label} (a key your administrator trusts)");
                    v.chain.push(format!("The signature is by {label}, a key listed in trusted-keys.txt ({}).", short(k)));
                    v
                }
                None => {
                    let mut v = Verdict::new(Level::Community, Some(k));
                    v.label = format!("signed by an unknown key {}", short(k));
                    match cert_ok {
                        Some(c) => match c.vouches(self.roots, k, &m.kind, self.now) {
                            Ok(()) => {
                                v.level = Level::Verified;
                                v.publisher = pub_of(Some(c));
                                v.label = format!("signed by {} (identity verified by IYAKKI)", c.publisher.name);
                                v.chain.push(format!("The signature is by {} ({}).", c.publisher.name, short(k)));
                                v.chain.push(format!("Their key is certified by the IYAKKI root key {} until {}.", short(&c.issuer), date(c.not_after)));
                            }
                            Err(e) => {
                                v.warnings.push(format!("{}; treated as an unknown publisher.", e.words()));
                                v.chain.push("The signature is valid, but the key is not vouched for by anyone this system trusts.".into());
                            }
                        },
                        None => {
                            if let Some(Err(e)) = &cert {
                                v.warnings.push(e.words());
                            }
                            v.chain.push("The signature is valid, but the key is not vouched for by anyone this system trusts.".into());
                            v.warnings.push("Signed by a publisher this system does not know. Check that you trust where it came from.".into());
                        }
                    }
                    if v.level == Level::Community && v.warnings.is_empty() {
                        v.warnings.push("Signed by a publisher this system does not know.".into());
                    }
                    v
                }
            },
        };
        // revocation beats everything, including the pinned root
        if let Some(why) = self.revoked(m, v.publisher.as_ref().map(|p| p.id.as_str()).or(cert_ok.map(|c| c.publisher.id.as_str()))) {
            v.level = Level::Revoked;
            v.refusal = Some(format!("This package was revoked: {why}"));
            v.label = "revoked".into();
        }
        v
    }

    fn revoked(&self, m: &Manifest, publisher: Option<&str>) -> Option<String> {
        if let Some(k) = &m.signer {
            if let Some(r) = self.revocations.key_revoked(k) {
                return Some(format!("the signing key was withdrawn ({r})"));
            }
        }
        if let Some(p) = publisher {
            if let Some(r) = self.revocations.publisher_revoked(p) {
                return Some(format!("the publisher {p} was withdrawn ({r})"));
            }
        }
        self.revocations.package_revoked(&m.kind, &m.name, &m.version, &m.sha256)
    }
}

// ------------------------------------------------------------------------------------------------ look-alike names

fn squash(n: &str) -> String {
    n.chars()
        .filter(|c| *c != '-' && *c != '_')
        .map(|c| match c {
            '0' => 'o',
            '1' | 'i' => 'l',
            '5' => 's',
            '3' => 'e',
            '$' => 's',
            c => c,
        })
        .collect()
}

/// Edit distance with swaps of neighbours counting as one change.
fn distance(a: &[char], b: &[char]) -> usize {
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=b.len() {
        d[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// Names in `known` (name, publisher id) that `name` by `publisher` could be mistaken for: a different publisher, and
/// either the same name once look-alike characters and dashes are ignored, or one change away (two for long names).
pub fn look_alikes(name: &str, publisher: &str, known: &[(String, String)]) -> Vec<String> {
    let mine: Vec<char> = squash(name).chars().collect();
    let mut out: Vec<String> = known
        .iter()
        .filter(|(n, p)| n != name && p != publisher)
        .filter(|(n, _)| {
            let other: Vec<char> = squash(n).chars().collect();
            if other == mine {
                return true;
            }
            let limit = if mine.len().min(other.len()) >= 9 { 2 } else if mine.len().min(other.len()) >= 5 { 1 } else { 0 };
            limit > 0 && distance(&mine, &other) <= limit
        })
        .map(|(n, _)| n.clone())
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pkg::new_signing_key;

    fn manifest(kind: &str, signer: Option<&SigningKey>, cert: Option<&Cert>) -> Manifest {
        Manifest { kind: kind.into(), name: "demo".into(), version: "1.0.0".into(), license: "MIT".into(), sha256: "ab".repeat(32), signer: signer.map(public_key_hex), meta: None, cert: cert.map(Cert::to_json) }
    }

    fn acme() -> Publisher {
        Publisher { id: "acme".into(), name: "Acme Ltd".into(), url: "https://acme.example".into() }
    }

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn documents_sign_over_a_canonical_form_that_ignores_key_order_and_the_signature() {
        let k = new_signing_key().unwrap();
        let a: Value = serde_json::from_str(r#"{"b":[1,{"y":2,"x":1}],"a":"é","sig":"zz"}"#).unwrap();
        let b: Value = serde_json::from_str(r#"{"a":"é","sig":"other","b":[1,{"x":1,"y":2}]}"#).unwrap();
        assert_eq!(canonical(&a), canonical(&b));
        assert_eq!(canonical(&a), r#"{"a":"é","b":[1,{"x":1,"y":2}]}"#);
        let sig = sign_doc("catalog", &a, &k);
        assert!(verify_doc("catalog", &b, &public_key_hex(&k), &sig));
        assert!(!verify_doc("license", &b, &public_key_hex(&k), &sig), "a signature for one purpose is no good for another");
        let c: Value = serde_json::from_str(r#"{"a":"e","b":[1,{"x":1,"y":2}]}"#).unwrap();
        assert!(!verify_doc("catalog", &c, &public_key_hex(&k), &sig));
        assert!(!verify_doc("catalog", &a, "zz", &sig) && !verify_doc("catalog", &a, &public_key_hex(&k), "00"));
    }

    #[test]
    fn roots_parse_skip_junk_and_honour_their_window() {
        let k = new_signing_key().unwrap();
        let line = format!("# comment\n\nroot-1 {} 100 200\nbad zz\nroot-2 {}\n", public_key_hex(&k), public_key_hex(&new_signing_key().unwrap()));
        let r = Roots::parse(&line);
        assert_eq!(r.list().len(), 2);
        assert!(r.valid_at(&public_key_hex(&k), 150).is_some() && r.valid_at(&public_key_hex(&k), 99).is_none() && r.valid_at(&public_key_hex(&k), 201).is_none());
        assert!(r.contains(&public_key_hex(&k)));
        assert!(Roots::pinned().is_empty(), "no official key has been generated yet: see docs/STORE.md section 10");
    }

    #[test]
    fn every_trust_level_is_reached_by_its_own_path() {
        let (root, publisher, community, own) = (new_signing_key().unwrap(), new_signing_key().unwrap(), new_signing_key().unwrap(), new_signing_key().unwrap());
        let roots = Roots::with("r1", &root);
        let revs = Revocations::default();
        let own_keys = vec![(public_key_hex(&own), "Our IT".to_string())];
        let j = Judge { roots: &roots, own_keys: &own_keys, revocations: &revs, now: NOW };
        let cert = Cert::issue(&root, acme(), &public_key_hex(&publisher), NOW - 1000, NOW + 1000, &["skill", "tool"]);
        // official: the root key itself
        let v = j.verdict(&manifest("skill", Some(&root), None));
        assert_eq!((v.level, v.refusal.is_none()), (Level::Official, true));
        assert!(v.label.contains("IYAKKI release key") && v.chain[0].contains("r1"));
        // verified: a publisher key with a certificate from the root
        let v = j.verdict(&manifest("skill", Some(&publisher), Some(&cert)));
        assert_eq!(v.level, Level::Verified, "{v:?}");
        assert_eq!(v.publisher.as_ref().unwrap().id, "acme");
        assert!(v.chain.iter().any(|c| c.contains("certified by the IYAKKI root")));
        // trusted: the administrator's own key
        assert_eq!(j.verdict(&manifest("skill", Some(&own), None)).level, Level::Trusted);
        // community: a valid signature from an unknown key
        let v = j.verdict(&manifest("skill", Some(&community), None));
        assert_eq!(v.level, Level::Community);
        assert!(v.warnings.iter().any(|w| w.contains("does not know")));
        // unsigned
        let v = j.verdict(&manifest("skill", None, None));
        assert_eq!(v.level, Level::Unsigned);
        assert!(v.warnings[0].contains("anyone could have written"));
        // the certificate must be for this key
        let stolen = j.verdict(&manifest("skill", Some(&community), Some(&cert)));
        assert_eq!(stolen.level, Level::Community);
        assert!(stolen.warnings.iter().any(|w| w.contains("different key")), "{stolen:?}");
    }

    #[test]
    fn a_certificate_that_is_expired_out_of_scope_forged_or_from_a_stranger_gives_only_community() {
        let (root, publisher, stranger) = (new_signing_key().unwrap(), new_signing_key().unwrap(), new_signing_key().unwrap());
        let roots = Roots::with("r1", &root);
        let revs = Revocations::default();
        let j = Judge { roots: &roots, own_keys: &[], revocations: &revs, now: NOW };
        let pk = public_key_hex(&publisher);
        let level = |kind: &str, c: &Cert| j.verdict(&manifest(kind, Some(&publisher), Some(c)));
        let expired = Cert::issue(&root, acme(), &pk, NOW - 2000, NOW - 1, &["skill"]);
        let v = level("skill", &expired);
        assert_eq!(v.level, Level::Community);
        assert!(v.warnings[0].contains("expired on"), "{v:?}");
        let early = Cert::issue(&root, acme(), &pk, NOW + 500, NOW + 900, &["skill"]);
        assert!(level("skill", &early).warnings[0].contains("not valid before"));
        let narrow = Cert::issue(&root, acme(), &pk, NOW - 5, NOW + 5, &["skill"]);
        assert_eq!(level("skill", &narrow).level, Level::Verified);
        let v = level("tool", &narrow);
        assert_eq!(v.level, Level::Community);
        assert!(v.warnings[0].contains("does not cover tool"));
        let by_stranger = Cert::issue(&stranger, acme(), &pk, NOW - 5, NOW + 5, &["skill"]);
        assert!(level("skill", &by_stranger).warnings[0].contains("not issued by a key this system trusts"));
        let mut forged = narrow.clone();
        forged.publisher.name = "Someone Else".into();
        assert!(level("skill", &forged).warnings[0].contains("bad signature"));
        let mut widened = narrow.clone();
        widened.scopes = vec!["*".into()];
        assert!(level("tool", &widened).warnings[0].contains("bad signature"));
        // junk in the header
        let mut m = manifest("skill", Some(&publisher), None);
        m.cert = Some("{nope".into());
        let v = j.verdict(&m);
        assert_eq!(v.level, Level::Community);
        assert!(v.warnings.iter().any(|w| w.contains("cannot be read")));
        assert_eq!(Cert::parse(&narrow.to_json()).unwrap(), narrow);
    }

    #[test]
    fn revocation_beats_every_level_including_the_root() {
        let (root, publisher) = (new_signing_key().unwrap(), new_signing_key().unwrap());
        let roots = Roots::with("r1", &root);
        let cert = Cert::issue(&root, acme(), &public_key_hex(&publisher), NOW - 5, NOW + 5, &["skill"]);
        let mut revs = Revocations::default();
        let signed = revs.sign_for_tests(&root, 1, NOW, vec![public_key_hex(&publisher)], vec![], vec![]);
        revs.accept(&signed, &roots, NOW).unwrap();
        let j = Judge { roots: &roots, own_keys: &[], revocations: &revs, now: NOW };
        let v = j.verdict(&manifest("skill", Some(&publisher), Some(&cert)));
        assert_eq!(v.level, Level::Revoked);
        assert!(v.refusal.unwrap().contains("signing key was withdrawn"));
        // a revoked publisher id and a revoked package hash
        let mut revs2 = Revocations::default();
        let signed = revs2.sign_for_tests(&root, 1, NOW, vec![], vec!["acme".into()], vec![("demo".into(), Some("0.9.0".into()))]);
        revs2.accept(&signed, &roots, NOW).unwrap();
        let j2 = Judge { roots: &roots, own_keys: &[], revocations: &revs2, now: NOW };
        assert_eq!(j2.verdict(&manifest("skill", Some(&publisher), Some(&cert))).level, Level::Revoked);
        let v = j2.verdict(&manifest("skill", Some(&root), None));
        assert_eq!(v.level, Level::Official, "version 1.0.0 is above the revoked floor");
        let mut old = manifest("skill", Some(&root), None);
        old.version = "0.8.0".into();
        let v = j2.verdict(&old);
        assert_eq!(v.level, Level::Revoked);
        assert!(v.refusal.unwrap().contains("before 0.9.0"));
    }

    #[test]
    fn look_alike_names_from_other_publishers_are_flagged_but_ones_own_and_unrelated_are_not() {
        let known: Vec<(String, String)> = [("lang-python", "iyakki"), ("kubectl-helper", "iyakki"), ("notes", "iyakki"), ("my-notes", "acme")].iter().map(|(a, b)| (a.to_string(), b.to_string())).collect();
        assert_eq!(look_alikes("lang-pyth0n", "evil", &known), vec!["lang-python".to_string()], "0 for o");
        assert_eq!(look_alikes("langpython", "evil", &known), vec!["lang-python".to_string()], "dashes do not count");
        assert_eq!(look_alikes("lang-pythno", "evil", &known), vec!["lang-python".to_string()], "swapped letters");
        assert_eq!(look_alikes("kubect1-helper", "evil", &known), vec!["kubectl-helper".to_string()]);
        assert!(look_alikes("lang-python", "evil", &known).is_empty(), "the same name is not a look-alike (it is a clash, handled by the publisher check)");
        assert!(look_alikes("lang-pyth0n", "iyakki", &known).is_empty(), "one's own names do not count");
        assert!(look_alikes("weather", "evil", &known).is_empty() && look_alikes("note", "evil", &known).is_empty(), "short names need an exact squash match");
    }

    #[test]
    fn levels_have_plain_words_and_only_the_weak_ones_need_a_warning_ack() {
        for l in Level::ALL {
            assert!(!l.label().is_empty() && l.plain().ends_with('.') && !l.id().is_empty());
        }
        assert_eq!(Level::ALL.iter().filter(|l| l.needs_warning_ack()).count(), 3);
        assert!(Level::Official < Level::Verified && Level::Verified < Level::Community && Level::Community < Level::Unsigned);
        assert_eq!(date(0), "1970-01-01");
        assert!(Verdict::builtin().label.contains("built into"));
    }
}
