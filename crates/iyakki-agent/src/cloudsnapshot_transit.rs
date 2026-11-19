//! A real key-management client for the snapshot envelope keyring (F-CLOUD-15): [`TransitKms`] speaks the HTTP interface of a
//! HashiCorp-style vault's transit engine (`POST {address}/v1/{mount}/encrypt/{key}` and `.../decrypt/{key}`), so the key that wraps the
//! snapshot keys never leaves the operator's service. It implements [`Kms`], so `KmsKeyring`, `migrate` and the snapshot command use it
//! like [`crate::cloudsnapshot_kms::LocalKms`].
//!
//! * The access token is a vault secret read by NAME when the call is made (never held, printed or put in an error); a refused token
//!   and an unreachable service are said in words that carry no address or header, and nothing falls back to a local key.
//! * The context is bound by the service (a derived transit key: the key must be created with `derived=true`), so a wrap made for one
//!   snapshot key id does not open as another.
//! * The wrapped form is the service's own ciphertext text (`vault:v1:...`) as bytes.

use std::sync::Arc;

use iyakki_proxy::Upstream;
use serde_json::{json, Value};
use zeroize::Zeroizing;

use crate::cloudsnapshot_kms::Kms;

/// Where the service is and which secret holds its token.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransitRef {
    /// `https://host:8200`, no path.
    pub address: String,
    /// The transit engine's mount, `transit` by default.
    pub mount: String,
    pub namespace: Option<String>,
}

/// Reads the token by the name of its secret (the person's or the tenant's vault).
pub type TokenSource = Arc<dyn Fn(&str) -> Result<Zeroizing<String>, String> + Send + Sync>;

pub struct TransitKms {
    cfg: TransitRef,
    token_secret: String,
    up: Arc<dyn Upstream>,
    token: TokenSource,
}

const UNREACHABLE: &str = "the key service could not be reached, so nothing was wrapped or opened. No local key was used instead.";

impl TransitKms {
    pub fn new(cfg: TransitRef, token_secret: &str, up: Arc<dyn Upstream>, token: TokenSource) -> Result<TransitKms, String> {
        if !(cfg.address.starts_with("https://") || cfg.address.starts_with("http://")) || cfg.address.contains(char::is_whitespace) || cfg.address.contains('@') {
            return Err("the key service address is https://host:port (no sign-in part)".into());
        }
        if cfg.mount.is_empty() || !cfg.mount.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
            return Err("the transit mount is a plain name (letters, digits, - and _)".into());
        }
        if token_secret.is_empty() {
            return Err("name the vault secret that holds the key service's token".into());
        }
        Ok(TransitKms { cfg, token_secret: token_secret.to_string(), up, token })
    }

    fn key_ok(key_ref: &str) -> Result<(), String> {
        if key_ref.is_empty() || key_ref.len() > 80 || !key_ref.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) {
            return Err("a transit key name is letters, digits, - _ and .".into());
        }
        Ok(())
    }

    fn post(&self, op: &str, key_ref: &str, body: Value) -> Result<Value, String> {
        Self::key_ok(key_ref)?;
        let tok = (self.token)(&self.token_secret).map_err(|_| format!("the key service's access secret \"{}\" is not available in the vault, so nothing was wrapped or opened. No local key was used instead.", self.token_secret))?;
        let mut h = vec![("X-Vault-Token".to_string(), tok.to_string()), ("Content-Type".to_string(), "application/json".to_string())];
        if let Some(ns) = &self.cfg.namespace {
            h.push(("X-Vault-Namespace".into(), ns.clone()));
        }
        let url = format!("{}/v1/{}/{op}/{key_ref}", self.cfg.address.trim_end_matches('/'), self.cfg.mount);
        let r = self.up.request("POST", &url, &h, body.to_string().as_bytes()).map_err(|_| UNREACHABLE.to_string())?;
        match r.status {
            200 => serde_json::from_slice::<Value>(&r.body).map_err(|_| "the key service answered with something that is not JSON".to_string()),
            401 | 403 => Err("the key service refused the access secret, so nothing was wrapped or opened. No local key was used instead.".into()),
            404 => Err("the key service has no such transit key (or engine); create the key with derived=true first".into()),
            400 => Err("the key service refused the request: the key may not be a derived key, or what is stored was changed".into()),
            s => Err(format!("the key service answered with an unexpected result (HTTP {s}); nothing was changed")),
        }
    }
}

fn b64(b: &[u8]) -> String {
    iyakki_shared::b64::base64_encode(b)
}

impl Kms for TransitKms {
    fn wrap(&self, key_ref: &str, secret: &[u8], context: &str) -> Result<Vec<u8>, String> {
        let v = self.post("encrypt", key_ref, json!({"plaintext": b64(secret), "context": b64(context.as_bytes())}))?;
        let ct = v["data"]["ciphertext"].as_str().filter(|c| c.starts_with("vault:") && c.len() < 4096).ok_or("the key service did not return a ciphertext")?;
        Ok(ct.as_bytes().to_vec())
    }

    fn unwrap(&self, key_ref: &str, wrapped: &[u8], context: &str) -> Result<Vec<u8>, String> {
        let ct = std::str::from_utf8(wrapped).ok().filter(|c| c.starts_with("vault:") && c.len() < 4096).ok_or("the wrapped key is damaged")?;
        let v = self.post("decrypt", key_ref, json!({"ciphertext": ct, "context": b64(context.as_bytes())}))?;
        let pt = v["data"]["plaintext"].as_str().ok_or("the key service did not return the key")?;
        iyakki_shared::b64::b64_decode(pt).ok_or_else(|| "the key service returned a damaged key".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloudsnapshot::KeySource;
    use crate::cloudsnapshot_kms::{Blobs, KmsKeyring};
    use iyakki_proxy::UpstreamResponse;
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    /// A toy transit engine: the "ciphertext" is the plaintext xor 0x5a with the context and key name appended, so a wrong
    /// context or key name fails to open, as the real service does for a derived key.
    #[derive(Default)]
    struct FakeTransit {
        calls: Mutex<Vec<(String, Vec<(String, String)>)>>,
        down: bool,
        status: Option<u16>,
    }

    impl Upstream for FakeTransit {
        fn request(&self, _m: &str, url: &str, h: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
            self.calls.lock().unwrap().push((url.to_string(), h.to_vec()));
            if self.down {
                return Err("connect to 10.0.0.9:8200 refused".into());
            }
            if let Some(s) = self.status {
                return Ok(UpstreamResponse { status: s, body: b"{}".to_vec() });
            }
            let key = url.rsplit('/').next().unwrap();
            let v: Value = serde_json::from_slice(body).unwrap();
            let out = if url.contains("/encrypt/") {
                let pt = iyakki_shared::b64::b64_decode(v["plaintext"].as_str().unwrap()).unwrap();
                let x: Vec<u8> = pt.iter().map(|b| b ^ 0x5a).collect();
                json!({"data": {"ciphertext": format!("vault:v1:{}|{}|{}", b64(&x), key, v["context"].as_str().unwrap())}})
            } else {
                let ct = v["ciphertext"].as_str().unwrap().strip_prefix("vault:v1:").unwrap();
                let parts: Vec<&str> = ct.split('|').collect();
                if parts[1] != key || parts[2] != v["context"].as_str().unwrap() {
                    return Ok(UpstreamResponse { status: 400, body: b"{}".to_vec() });
                }
                let x: Vec<u8> = iyakki_shared::b64::b64_decode(parts[0]).unwrap().iter().map(|b| b ^ 0x5a).collect();
                json!({"data": {"plaintext": b64(&x)}})
            };
            Ok(UpstreamResponse { status: 200, body: out.to_string().into_bytes() })
        }
    }

    fn kms(f: Arc<FakeTransit>) -> TransitKms {
        let token: TokenSource = Arc::new(|n| if n == "svc.kms-token" { Ok(Zeroizing::new("hvs.SECRET".to_string())) } else { Err("no such secret".into()) });
        TransitKms::new(TransitRef { address: "https://kms.example.org:8200".into(), mount: "transit".into(), namespace: Some("team".into()) }, "svc.kms-token", f, token).unwrap()
    }

    #[test]
    fn a_secret_is_wrapped_and_opened_by_the_service_with_the_context_bound_and_the_token_only_in_the_header() {
        let f = Arc::new(FakeTransit::default());
        let k = kms(f.clone());
        let w = k.wrap("snap", b"0123456789abcdef0123456789abcdef", "key-id-1").unwrap();
        assert!(w.starts_with(b"vault:v1:") && !w.windows(32).any(|x| x == b"0123456789abcdef0123456789abcdef"));
        assert_eq!(k.unwrap("snap", &w, "key-id-1").unwrap(), b"0123456789abcdef0123456789abcdef");
        assert!(k.unwrap("snap", &w, "key-id-2").unwrap_err().contains("refused the request"));
        assert!(k.unwrap("other", &w, "key-id-1").is_err());
        let calls = f.calls.lock().unwrap();
        assert!(calls[0].0 == "https://kms.example.org:8200/v1/transit/encrypt/snap");
        assert!(calls[0].1.contains(&("X-Vault-Token".into(), "hvs.SECRET".into())) && calls[0].1.contains(&("X-Vault-Namespace".into(), "team".into())));
    }

    #[test]
    fn the_envelope_keyring_works_over_the_service_and_keeps_only_the_wrapped_form() {
        #[derive(Default)]
        struct Mem(Mutex<BTreeMap<String, Vec<u8>>>);
        impl Blobs for Mem {
            fn get(&self, n: &str) -> Result<Vec<u8>, String> {
                self.0.lock().unwrap().get(n).cloned().ok_or_else(|| format!("{n} is not there"))
            }
            fn put(&self, n: &str, d: &[u8]) -> Result<(), String> {
                self.0.lock().unwrap().insert(n.into(), d.to_vec());
                Ok(())
            }
            fn names(&self) -> Result<Vec<String>, String> {
                Ok(self.0.lock().unwrap().keys().cloned().collect())
            }
        }
        let (m, blobs) = (kms(Arc::new(FakeTransit::default())), Mem::default());
        let ring = KmsKeyring { kms: &m, blobs: &blobs, key_ref: "snap" };
        let k = ring.rotate().unwrap();
        assert_eq!(ring.current().unwrap().to_hex(), k.to_hex());
        assert_eq!(ring.get(k.id()).unwrap().id(), k.id());
        let stored = blobs.get(&format!("snapshot-key.{}.wrapped", k.id())).unwrap();
        assert!(stored.starts_with(b"vault:v1:") && !String::from_utf8_lossy(&stored).contains(&k.to_hex()));
    }

    #[test]
    fn a_missing_token_a_refusal_and_an_unreachable_service_say_so_without_an_address_or_a_token_and_use_no_local_key() {
        let f = Arc::new(FakeTransit { down: true, ..Default::default() });
        let e = kms(f).wrap("snap", b"x", "c").unwrap_err();
        assert!(e.contains("could not be reached") && e.contains("No local key") && !e.contains("10.0.0.9"), "{e}");
        let f = Arc::new(FakeTransit { status: Some(403), ..Default::default() });
        assert!(kms(f).wrap("snap", b"x", "c").unwrap_err().contains("refused the access secret"));
        let token: TokenSource = Arc::new(|_| Err("locked".into()));
        let k = TransitKms::new(TransitRef { address: "https://kms.example.org".into(), mount: "transit".into(), namespace: None }, "svc.kms-token", Arc::new(FakeTransit::default()), token).unwrap();
        let e = k.wrap("snap", b"x", "c").unwrap_err();
        assert!(e.contains("svc.kms-token") && e.contains("not available") && !e.contains("locked"), "{e}");
        assert!(kms(Arc::new(FakeTransit::default())).wrap("a/../b", b"x", "c").is_err());
    }

    #[test]
    fn a_bad_address_mount_or_empty_token_name_is_refused_when_it_is_set_up() {
        let t: TokenSource = Arc::new(|_| Ok(Zeroizing::new("t".to_string())));
        let up: Arc<dyn Upstream> = Arc::new(FakeTransit::default());
        let r = |a: &str, m: &str| TransitRef { address: a.into(), mount: m.into(), namespace: None };
        assert!(TransitKms::new(r("ftp://x", "transit"), "s", up.clone(), t.clone()).is_err());
        assert!(TransitKms::new(r("https://u:p@x", "transit"), "s", up.clone(), t.clone()).is_err());
        assert!(TransitKms::new(r("https://x", "a/b"), "s", up.clone(), t.clone()).is_err());
        assert!(TransitKms::new(r("https://x", "transit"), "", up, t).is_err());
    }
}
