//! The TLS transport for encrypted camera streams (`rtsps://`, F-CAM-20): a TCP connection wrapped in rustls whose certificate is judged
//! afterwards by the person's trust choice (`iyakki_rtsp::trust`): the public authorities, or exactly one pinned certificate. The handshake
//! itself accepts anything so the certificate can be read, and NOTHING secret is sent (the RTSP sign-in follows only after the verdict
//! is Accept). A certificate the system does not trust comes back as `Ask` with its fingerprint, for a card; it is never accepted alone.

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iyakki_rtsp::trust::{self, Trust, Verdict};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use sha2::{Digest, Sha256};

#[derive(Debug)]
struct Recorder {
    chain: Arc<Mutex<Vec<Vec<u8>>>>,
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl ServerCertVerifier for Recorder {
    fn verify_server_cert(&self, end: &CertificateDer<'_>, inter: &[CertificateDer<'_>], _: &ServerName<'_>, _: &[u8], _: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        if let Ok(mut c) = self.chain.lock() {
            *c = std::iter::once(end).chain(inter).map(|d| d.as_ref().to_vec()).collect();
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn verify_tls13_signature(&self, m: &[u8], c: &CertificateDer<'_>, d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The system does not trust this certificate: ask the person, showing this fingerprint (`AB:CD:...`).
    Ask { fingerprint: String },
    Refused(String),
    Failed(String),
}

/// The connection after an accepted verdict.
pub struct Secure {
    tls: Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>,
    /// The leaf certificate's fingerprint, to keep as the pin when the person approved it.
    pub fingerprint: String,
}

impl Read for Secure {
    fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
        self.tls.read(b)
    }
}

impl Write for Secure {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        self.tls.write(b)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.tls.flush()
    }
}

/// Connect, handshake, read the certificate and judge it. `now` is seconds since 1970 (for the validity period).
pub fn connect(host: &str, port: u16, trust: &Trust, now: i64) -> Result<Secure, Refusal> {
    let addrs: Vec<_> = (host, port).to_socket_addrs().map_err(|_| Refusal::Failed(format!("could not find {host}")))?.collect();
    let mut tcp = None;
    for a in addrs {
        if let Ok(s) = TcpStream::connect_timeout(&a, Duration::from_secs(10)) {
            tcp = Some(s);
            break;
        }
    }
    let mut tcp = tcp.ok_or_else(|| Refusal::Failed(format!("could not reach {host}:{port}")))?;
    let _ = tcp.set_nodelay(true);
    let _ = tcp.set_read_timeout(Some(Duration::from_millis(200)));
    let _ = tcp.set_write_timeout(Some(Duration::from_secs(30)));
    let chain = Arc::new(Mutex::new(Vec::new()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let cfg = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| Refusal::Failed(format!("TLS is not available: {e}")))?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Recorder { chain: chain.clone(), provider }))
        .with_no_client_auth();
    let name = ServerName::try_from(host.to_string()).map_err(|_| Refusal::Failed("the host name is not valid for TLS".into()))?;
    let mut conn = rustls::ClientConnection::new(Arc::new(cfg), name).map_err(|e| Refusal::Failed(format!("TLS: {e}")))?;
    let deadline = Instant::now() + Duration::from_secs(20);
    while conn.is_handshaking() {
        if Instant::now() > deadline {
            return Err(Refusal::Failed("the TLS handshake did not finish".into()));
        }
        match conn.complete_io(&mut tcp) {
            Ok(_) => {}
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
            Err(e) => return Err(Refusal::Failed(format!("the TLS handshake failed ({e})"))),
        }
    }
    let chain = chain.lock().map(|c| c.clone()).unwrap_or_default();
    let leaf = chain.first().ok_or_else(|| Refusal::Refused("the camera showed no certificate".into()))?;
    let fp: String = Sha256::digest(leaf).iter().map(|b| format!("{b:02x}")).collect();
    let system_ok = matches!(trust, Trust::System) && iyakki_proxy::certview::view(host, &chain, now, iyakki_proxy::certview::public_roots()).map(|v| v.trust.is_ok() && v.names_match).unwrap_or(false);
    match trust::judge(trust, &fp, system_ok) {
        Verdict::Accept => Ok(Secure { tls: Box::new(rustls::StreamOwned::new(conn, tcp)), fingerprint: fp }),
        Verdict::Ask { fingerprint } => Err(Refusal::Ask { fingerprint: trust::display(&(0..32).filter_map(|i| u8::from_str_radix(&fingerprint[i * 2..i * 2 + 2], 16).ok()).collect::<Vec<_>>()) }),
        Verdict::Refuse(m) => Err(Refusal::Refused(m)),
    }
}

/// What the person decided about each encrypted camera's certificate: `pinned` (trusted, by id) and `pending` (shown, waiting for the person).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TrustStore {
    #[serde(default)]
    pub pinned: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    pub pending: std::collections::BTreeMap<String, String>,
}

fn store_path(data: &std::path::Path) -> std::path::PathBuf {
    data.join("camera-trust.json")
}

impl TrustStore {
    pub fn load(data: &std::path::Path) -> TrustStore {
        std::fs::read(store_path(data)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
    }

    pub fn save(&self, data: &std::path::Path) -> Result<(), String> {
        let t = serde_json::to_vec(self).map_err(|e| e.to_string())?;
        let p = store_path(data);
        let tmp = p.with_extension("tmp");
        std::fs::write(&tmp, t).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
    }

    /// Read, change and write the store as one step. Every writer (the recorder and the live view saving a waiting certificate, the person's
    /// approval) goes through here: with a separate load and save a camera retrying at the same moment saved its older copy over the
    /// approval and the yes was lost.
    pub fn change<T>(data: &std::path::Path, f: impl FnOnce(&mut TrustStore) -> Result<T, String>) -> Result<T, String> {
        static WRITERS: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = WRITERS.lock().unwrap_or_else(|e| e.into_inner());
        let mut st = TrustStore::load(data);
        let out = f(&mut st)?;
        st.save(data)?;
        Ok(out)
    }

    pub fn trust_for(&self, id: &str) -> Trust {
        self.pinned.get(id).map(|p| Trust::Pin(p.clone())).unwrap_or(Trust::System)
    }

    /// The person approved the certificate that was shown: it becomes the pin. Only a pending fingerprint can be approved (never one a tool names).
    pub fn approve(&mut self, id: &str) -> Result<String, String> {
        let fp = self.pending.remove(id).ok_or("this camera has no certificate waiting for approval")?;
        self.pinned.insert(id.to_string(), fp.clone());
        Ok(fp)
    }

    /// Take the trust away (a replaced camera shows a new certificate; the next connection asks again).
    pub fn forget(&mut self, id: &str) -> bool {
        self.pending.remove(id).is_some() | self.pinned.remove(id).is_some()
    }
}

/// Connect for the camera `id` and keep what has to be kept: a certificate to ask about becomes `pending`.
pub fn connect_for(data: &std::path::Path, id: &str, host: &str, port: u16) -> Result<Secure, String> {
    let st = TrustStore::load(data);
    match connect(host, port, &st.trust_for(id), iyakki_schema::now_secs() as i64) {
        Ok(s) => Ok(s),
        Err(Refusal::Ask { fingerprint }) => {
            let fp = trust::normalize(&fingerprint);
            // the approval may have come in since the store was read above: do not put a waiting card over a yes
            let _ = TrustStore::change(data, |st| {
                if st.pinned.get(id) != Some(&fp) {
                    st.pending.insert(id.to_string(), fp.clone());
                }
                Ok(())
            });
            Err(format!("This camera's certificate is not signed by a known authority (most cameras sign their own). Its fingerprint is {fingerprint}. If this is your camera, approve it (camera_trust_set) and it is remembered."))
        }
        Err(Refusal::Refused(m)) | Err(Refusal::Failed(m)) => Err(m),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TLS server with a fresh self-signed certificate; it answers one line.
    fn server() -> (u16, String) {
        let ck = rcgen::generate_simple_self_signed(vec!["localhost".to_string()]).unwrap();
        let der = ck.cert.der().to_vec();
        let fp: String = Sha256::digest(&der).iter().map(|b| format!("{b:02x}")).collect();
        let key = rustls::pki_types::PrivateKeyDer::try_from(ck.key_pair.serialize_der()).unwrap();
        let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider())).with_safe_default_protocol_versions().unwrap().with_no_client_auth().with_single_cert(vec![CertificateDer::from(der)], key).unwrap();
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        let cfg = Arc::new(cfg);
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let cfg = cfg.clone();
                std::thread::spawn(move || {
                    let mut s = s;
                    let Ok(mut c) = rustls::ServerConnection::new(cfg) else { return };
                    let mut t = rustls::Stream::new(&mut c, &mut s);
                    let mut b = [0u8; 16];
                    if let Ok(n) = t.read(&mut b) {
                        if n > 0 {
                            let _ = t.write_all(b"RTSP/1.0 200 OK\r\n\r\n");
                            let _ = t.flush();
                        }
                    }
                });
            }
        });
        (port, fp)
    }

    #[test]
    fn a_self_signed_camera_is_asked_about_then_pinned_then_refused_when_the_certificate_changes() {
        let (port, fp) = server();
        let now = iyakki_schema::now_secs() as i64;
        let ask = match connect("localhost", port, &Trust::System, now) {
            Err(Refusal::Ask { fingerprint }) => fingerprint,
            other => panic!("{:?}", other.err()),
        };
        assert_eq!(trust::normalize(&ask), fp, "the card shows the real fingerprint");
        let mut s = connect("localhost", port, &Trust::Pin(ask.clone()), now).expect("the pinned certificate is accepted");
        assert_eq!(s.fingerprint, fp);
        s.write_all(b"OPTIONS").unwrap();
        s.flush().unwrap();
        let mut got = Vec::new();
        for _ in 0..50 {
            let mut b = [0u8; 64];
            match s.read(&mut b) {
                Ok(0) => break,
                Ok(n) => {
                    got.extend_from_slice(&b[..n]);
                    break;
                }
                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => continue,
                Err(_) => break,
            }
        }
        assert!(String::from_utf8_lossy(&got).starts_with("RTSP/1.0 200"), "{got:?}");
        // a second camera (another certificate) on the same pin is refused in words
        let (port2, _) = server();
        assert!(matches!(connect("localhost", port2, &Trust::Pin(ask), now), Err(Refusal::Refused(m)) if m.contains("different certificate")));
    }

    #[test]
    fn the_store_keeps_a_waiting_certificate_until_the_person_approves_it_and_connects_with_the_pin() {
        let (port, fp) = server();
        let d = crate::testdir::outside_project("camtls", "store");
        let msg = connect_for(&d, "cam1", "localhost", port).err().expect("asks first");
        assert!(msg.contains("approve it (camera_trust_set)"), "{msg}");
        let st = TrustStore::load(&d);
        assert_eq!(st.pending.get("cam1"), Some(&fp));
        assert!(TrustStore::load(&d).approve("cam2").is_err(), "nothing waits for another camera");
        let mut st = st;
        assert_eq!(st.approve("cam1").unwrap(), fp);
        st.save(&d).unwrap();
        assert!(connect_for(&d, "cam1", "localhost", port).is_ok(), "the approved pin connects");
        let mut st = TrustStore::load(&d);
        assert!(st.forget("cam1") && !st.forget("cam1"));
        assert_eq!(st.trust_for("cam1"), Trust::System);
    }

    #[test]
    fn writers_that_change_the_store_at_the_same_time_lose_nothing_and_a_waiting_card_never_covers_a_yes() {
        let d = std::env::temp_dir().join(format!("iyakki-camtls-change-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        TrustStore::change(&d, |st| {
            st.pending.insert("yes".into(), "AA".into());
            Ok(())
        })
        .unwrap();
        // many cameras saving a waiting certificate while the person approves one: every write is kept
        std::thread::scope(|sc| {
            for n in 0..16 {
                let d = &d;
                sc.spawn(move || {
                    TrustStore::change(d, |st| {
                        st.pending.insert(format!("cam{n}"), format!("{n:02X}"));
                        Ok(())
                    })
                    .unwrap();
                });
            }
            let d = &d;
            sc.spawn(move || {
                TrustStore::change(d, |st| st.approve("yes")).unwrap();
            });
        });
        let st = TrustStore::load(&d);
        assert_eq!(st.pending.len(), 16, "no waiting certificate was lost");
        assert_eq!(st.pinned.get("yes").map(String::as_str), Some("AA"), "the approval was kept");
        assert!(!st.pending.contains_key("yes"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
