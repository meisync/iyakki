//! The HTTP/2 transport of the Alexa Voice Service provider (F-VOICE-43), on the in-house HTTP/2 client of `iyakki-proxy` (`h2client`):
//! the events endpoint (`POST /v20160207/events`, a multipart request with the recording, a multipart answer) and the downchannel
//! (`GET /v20160207/directives`, opened first and kept open for the life of the connection).
//!
//! FAIL CLOSED: the AVS client id and product id belong to the owner's registered product (Amazon no longer opens AVS to new products).
//! They are read from `alexa-avs.json` in the data folder; with no file, an empty id or an id that is not plain text, [`AvsConfig::load`]
//! says so in a sentence and no connection is ever made. Nothing here has a default id.
//!
//! The downchannel carries only what the declared capabilities allow (speech: `avs::capabilities`); it is read in the background with a
//! size cap and ended when the connection ends. Directives that arrive on the event's own answer stream are the ones this provider acts on.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use iyakki_proxy::h2client::{Dialer, H2Pool, H2Request, Pooled};

use crate::avs::Transport;

/// The North America gateway of AVS (the public protocol description lists one per region).
pub const GATEWAY: &str = "alexa.na.gateway.devices.a2z.com";
pub const EVENTS_PATH: &str = "/v20160207/events";
pub const DIRECTIVES_PATH: &str = "/v20160207/directives";
pub const CONFIG_FILE: &str = "alexa-avs.json";
/// The most the downchannel's stream is read before it is closed and opened again.
pub const DOWNCHANNEL_MAX: usize = 1 << 20;
/// The words when the owner's product is not set.
pub const NO_CLIENT_ID: &str = "Alexa is not set up: the owner has not registered an Alexa Voice Service product on this machine (no client id). Nothing was sent to Amazon.";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvsConfig {
    pub client_id: String,
    pub product_id: String,
    pub gateway: String,
    pub port: u16,
}

impl AvsConfig {
    /// Read the owner's client id and product id. Missing or unusable: an error sentence, never a default.
    pub fn load(dir: &Path) -> Result<AvsConfig, String> {
        let text = std::fs::read_to_string(dir.join(CONFIG_FILE)).map_err(|_| NO_CLIENT_ID.to_string())?;
        AvsConfig::parse(&text)
    }

    pub fn parse(text: &str) -> Result<AvsConfig, String> {
        let v: serde_json::Value = serde_json::from_str(text).map_err(|_| NO_CLIENT_ID.to_string())?;
        let plain = |k: &str| v[k].as_str().map(str::trim).filter(|s| !s.is_empty() && s.len() <= 200 && s.chars().all(|c| c.is_ascii_graphic())).map(str::to_string);
        let client_id = plain("client_id").ok_or_else(|| NO_CLIENT_ID.to_string())?;
        let product_id = plain("product_id").ok_or_else(|| NO_CLIENT_ID.to_string())?;
        // the gateway may be changed (another region) but only to a plain host name; the port is 443 unless a test says otherwise
        let gateway = v["gateway"].as_str().map(str::trim).filter(|g| !g.is_empty()).map(str::to_string).unwrap_or_else(|| GATEWAY.to_string());
        if !gateway.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-') || !gateway.contains('.') {
            return Err("the gateway named in the Alexa settings is not a host name".into());
        }
        Ok(AvsConfig { client_id, product_id, gateway, port: 443 })
    }
}

/// The events and the downchannel over one HTTP/2 connection.
pub struct H2Events {
    pool: Arc<H2Pool>,
    gateway: String,
    port: u16,
    timeout: Duration,
    stop: Arc<AtomicBool>,
    down: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
    /// What the downchannel carried, interpreted: speech only, the newest [`DIRECTIVES_KEPT`] answers.
    directives: Arc<std::sync::Mutex<Vec<crate::avs::Turn>>>,
}

/// How many interpreted downchannel answers wait to be taken.
pub const DIRECTIVES_KEPT: usize = 16;

/// The dialer for the real gateway: TCP to the address that answers first, TLS 1.2 or 1.3 against the PUBLIC roots of the browser
/// (`iyakki_proxy::certview::public_roots`), `h2` offered. A gateway whose certificate those roots do not accept is refused: there is no
/// "trust anything" path here. Nothing is dialled until a request is made, and no request is made without the owner's client id
/// ([`AvsConfig::load`]).
pub fn real_dialer() -> Dialer {
    real_dialer_with(iyakki_proxy::certview::public_roots())
}

/// [`real_dialer`] with roots given (a test trusts its own loopback certificate).
pub fn real_dialer_with(roots: rustls::RootCertStore) -> Dialer {
    iyakki_proxy::h2client::tls_dialer(roots, Duration::from_secs(10))
}

impl H2Events {
    pub fn new(cfg: &AvsConfig, dial: Dialer) -> H2Events {
        H2Events { pool: Arc::new(H2Pool::new(dial)), gateway: cfg.gateway.clone(), port: cfg.port, timeout: Duration::from_secs(90), stop: Arc::new(AtomicBool::new(false)), down: std::sync::Mutex::new(None), directives: Default::default() }
    }

    fn request(&self, method: &str, path: &str, bearer: &str, content_type: Option<&str>, body: &[u8]) -> H2Request {
        let mut headers = vec![("authorization".to_string(), format!("Bearer {bearer}"))];
        if let Some(c) = content_type {
            headers.push(("content-type".into(), c.to_string()));
        }
        H2Request { method: method.into(), authority: self.gateway.clone(), path: path.into(), headers, body: body.to_vec(), max_body: DOWNCHANNEL_MAX, media_cap: 0 }
    }

    /// Open the downchannel (AVS wants it open before the first event) and keep reading it in the background. Returns at once.
    pub fn open_downchannel(&self, bearer: &str) {
        let mut g = self.down.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if g.as_ref().is_some_and(|h| !h.is_finished()) {
            return;
        }
        self.stop.store(false, Ordering::SeqCst);
        let (pool, host, port, stop, kept) = (self.pool.clone(), self.gateway.clone(), self.port, self.stop.clone(), self.directives.clone());
        let req = self.request("GET", DIRECTIVES_PATH, bearer, None, &[]);
        *g = Some(std::thread::spawn(move || {
            // the answer of this stream is read when it ends or reaches the cap; what it holds is interpreted (speech only: a directive
            // the declared capabilities do not cover is listed as ignored, never acted on)
            if stop.load(Ordering::SeqCst) {
                return;
            }
            if let Pooled::Done(Ok(r)) = pool.fetch(&host, port, &req, Duration::from_secs(3600)) {
                let ct = r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.clone()).unwrap_or_default();
                if r.status == 200 && !stop.load(Ordering::SeqCst) {
                    if let Ok(turn) = crate::avs::parse_multipart(&ct, &r.body).and_then(|p| crate::avs::interpret(&p)) {
                        let mut k = kept.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        k.push(turn);
                        let over = k.len().saturating_sub(DIRECTIVES_KEPT);
                        k.drain(..over);
                    }
                }
            }
        }));
    }

    /// What the downchannel carried since the last call (the person's Alexa speaks on her own: a timer, a reminder), oldest first.
    pub fn take_directives(&self) -> Vec<crate::avs::Turn> {
        std::mem::take(&mut *self.directives.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }

    pub fn downchannel_open(&self) -> bool {
        self.down.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().is_some_and(|h| !h.is_finished())
    }

    /// The person left: nothing more is opened.
    pub fn close(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Transport for H2Events {
    fn post_event(&mut self, bearer: &str, content_type: &str, body: &[u8]) -> Result<(u16, String, Vec<u8>), String> {
        let mut req = self.request("POST", EVENTS_PATH, bearer, Some(content_type), body);
        req.max_body = 8 << 20;
        match self.pool.fetch(&self.gateway, self.port, &req, self.timeout) {
            Pooled::Done(Ok(r)) => {
                let ct = r.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-type")).map(|(_, v)| v.clone()).unwrap_or_default();
                Ok((r.status, ct, r.body))
            }
            Pooled::Done(Err(e)) => Err(format!("the connection to Amazon failed: {e}")),
            Pooled::Http1 => Err("Amazon's gateway did not agree to HTTP/2, which the Alexa Voice Service needs".into()),
        }
    }
}

#[cfg(test)]
#[path = "avs_h2_tests.rs"]
mod tests;
