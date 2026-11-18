//! TLS for the cloud audio stream of My IYAKKI (F-VOICE-38, docs/development/CLOUD-AUDIO-STREAMING.md).
//!
//! The 20 ms frames of `audiostream` never cross a network as plain bytes. One connection per direction is wrapped in TLS 1.3 here, and the device
//! trusts exactly ONE certificate: the one the cloud machine made for this purpose, whose bytes reached the device through the session the person
//! already authenticated (the Connector). There is no name lookup and no certificate authority in this link: a certificate that is not the pinned one
//! is refused in the handshake, whoever signed it, and nothing is sent. There is no plain fallback: a peer that does not speak TLS never has a frame
//! read from it. Frames keep their own checks (number, time, CRC, size cap) inside the tunnel.

use std::io::Write;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::Arc;
use std::time::Duration;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::{verify_tls13_signature, CryptoProvider};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{ClientConfig, ClientConnection, DigitallySignedStruct, ServerConfig, ServerConnection, SignatureScheme, StreamOwned};

use crate::audiostream::{read_frame, write_frame, Frame};

/// The one name in the certificate and in the handshake.
pub const NAME: &str = "audio.iyakki.internal";
/// How long a handshake or a read may stall.
pub const IO_TIMEOUT: Duration = Duration::from_secs(5);

fn provider() -> Arc<CryptoProvider> {
    Arc::new(rustls::crypto::ring::default_provider())
}

/// What the cloud machine makes for the stream: a certificate and its key. The certificate bytes go to the device over the authenticated session;
/// the key never leaves the machine.
pub struct Identity {
    pub cert_der: Vec<u8>,
    key_der: Vec<u8>,
}

impl Identity {
    pub fn make() -> Result<Identity, String> {
        let ck = rcgen::generate_simple_self_signed(vec![NAME.to_string()]).map_err(|e| format!("could not make the audio certificate: {e}"))?;
        Ok(Identity { cert_der: ck.cert.der().to_vec(), key_der: ck.key_pair.serialize_der() })
    }
}

pub fn server_config(id: &Identity) -> Result<Arc<ServerConfig>, String> {
    let cfg = ServerConfig::builder_with_provider(provider())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(id.cert_der.clone())], PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(id.key_der.clone())))
        .map_err(|e| format!("the audio certificate cannot be used: {e}"))?;
    Ok(Arc::new(cfg))
}

/// Trusts exactly one certificate (compared byte for byte) and checks the handshake signature with it.
#[derive(Debug)]
struct Pinned {
    der: Vec<u8>,
    provider: Arc<CryptoProvider>,
}

impl ServerCertVerifier for Pinned {
    fn verify_server_cert(&self, end_entity: &CertificateDer<'_>, _intermediates: &[CertificateDer<'_>], _name: &ServerName<'_>, _ocsp: &[u8], _now: UnixTime) -> Result<ServerCertVerified, rustls::Error> {
        if end_entity.as_ref() == self.der.as_slice() {
            Ok(ServerCertVerified::assertion())
        } else {
            Err(rustls::Error::General("this is not the certificate the cloud machine gave this device".into()))
        }
    }
    fn verify_tls12_signature(&self, _m: &[u8], _c: &CertificateDer<'_>, _d: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        Err(rustls::Error::General("TLS 1.2 is not used for the audio stream".into()))
    }
    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// The device's side: trust only `pinned_cert_der`.
pub fn client_config(pinned_cert_der: &[u8]) -> Result<Arc<ClientConfig>, String> {
    let p = provider();
    let cfg = ClientConfig::builder_with_provider(p.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(Pinned { der: pinned_cert_der.to_vec(), provider: p }))
        .with_no_client_auth();
    Ok(Arc::new(cfg))
}

fn arm(s: &TcpStream) -> Result<(), String> {
    s.set_read_timeout(Some(IO_TIMEOUT)).and_then(|_| s.set_write_timeout(Some(IO_TIMEOUT))).map_err(|e| e.to_string())?;
    let _ = s.set_nodelay(true);
    Ok(())
}

/// Connect, shake hands and write `frames`, then close with a proper TLS end. Nothing is written before the handshake has proved the pinned
/// certificate: a different one ends here with an error and no frame leaves.
pub fn send_frames(addr: SocketAddr, cfg: Arc<ClientConfig>, frames: &[Frame]) -> Result<(), String> {
    let tcp = TcpStream::connect_timeout(&addr, IO_TIMEOUT).map_err(|e| format!("could not reach the cloud machine: {e}"))?;
    arm(&tcp)?;
    let name = ServerName::try_from(NAME).map_err(|e| e.to_string())?;
    let conn = ClientConnection::new(cfg, name).map_err(|e| e.to_string())?;
    let mut tls = StreamOwned::new(conn, tcp);
    // the handshake runs here, before any frame
    while tls.conn.is_handshaking() {
        tls.conn.complete_io(&mut tls.sock).map_err(|e| format!("the secure link was refused: {e}"))?;
    }
    for f in frames {
        write_frame(&mut tls, f).map_err(|e| format!("the secure link broke: {e}"))?;
    }
    tls.flush().map_err(|e| e.to_string())?;
    tls.conn.send_close_notify();
    let _ = tls.conn.complete_io(&mut tls.sock);
    Ok(())
}

/// Accept ONE connection on `listener`, require TLS, and hand every frame to `on_frame` until the sender ends it. Returns the number of frames.
/// A peer that does not speak TLS, or a stream that is damaged or cut, ends with a named error; frames that arrived before stay delivered.
pub fn receive_frames(listener: &TcpListener, cfg: Arc<ServerConfig>, on_frame: &mut dyn FnMut(Frame)) -> Result<u32, String> {
    let (tcp, _) = listener.accept().map_err(|e| e.to_string())?;
    arm(&tcp)?;
    let conn = ServerConnection::new(cfg).map_err(|e| e.to_string())?;
    let mut tls = StreamOwned::new(conn, tcp);
    let mut n = 0u32;
    loop {
        match read_frame(&mut tls) {
            Ok(Some(f)) => {
                on_frame(f);
                n += 1;
            }
            Ok(None) => return Ok(n),
            Err(e) => return Err(format!("the audio stream ended badly after {n} frames: {e:?}")),
        }
    }
}

#[cfg(test)]
#[path = "audiotls_tests.rs"]
mod tests;
