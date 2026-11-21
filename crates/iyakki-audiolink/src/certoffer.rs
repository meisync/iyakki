//! A certificate on its way to the other end inside the authenticated session (F-VOICE-61).

/// The most a certificate offer may carry.
pub const MAX_CERT: usize = 4096;

/// A certificate on its way to the other end inside the authenticated session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CertOffer {
    pub machine: String,
    pub cert_der: Vec<u8>,
}

impl CertOffer {
    /// The record the session carries: `{"machine": "...", "cert": "<base64 of the DER>"}`.
    pub fn to_json(&self) -> String {
        serde_json::json!({"machine": self.machine, "cert": iyakki_link::sigscheme::base64(&self.cert_der)}).to_string()
    }

    /// Read a record from the session. A machine name with control characters, an empty or oversized certificate, or text that is not a
    /// certificate at all is refused (the check is the DER sequence's own length, not a guess).
    pub fn from_json(text: &str) -> Result<CertOffer, String> {
        let v: serde_json::Value = serde_json::from_str(text.get(..MAX_CERT * 2).unwrap_or(text)).map_err(|_| "the certificate offer is not valid".to_string())?;
        let machine = v["machine"].as_str().unwrap_or("").to_string();
        if machine.is_empty() || machine.len() > 60 || machine.chars().any(char::is_control) {
            return Err("the certificate offer names no machine".into());
        }
        let der = v["cert"].as_str().and_then(iyakki_link::sigscheme::unbase64).ok_or("the certificate offer has no certificate")?;
        if der.len() < 64 || der.len() > MAX_CERT || der[0] != 0x30 || !der_length_matches(&der) {
            return Err("that is not a certificate".into());
        }
        Ok(CertOffer { machine, cert_der: der })
    }
}

/// A DER SEQUENCE whose own length is the length of the bytes.
fn der_length_matches(d: &[u8]) -> bool {
    let (len, header) = match d.get(1) {
        Some(b) if *b < 0x80 => (*b as usize, 2),
        Some(0x81) => (d.get(2).copied().unwrap_or(0) as usize, 3),
        Some(0x82) => (((d.get(2).copied().unwrap_or(0) as usize) << 8) | d.get(3).copied().unwrap_or(0) as usize, 4),
        _ => return false,
    };
    header + len == d.len()
}
