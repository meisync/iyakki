//! Alexa Voice Service client logic (F-VOICE-43), written against the public protocol description in docs/development/ALEXA-FEASIBILITY.md
//! and tested against a FAKE service: Login with Amazon device-code linking and token refresh, the `SpeechRecognizer.Recognize`
//! multipart event, the multipart answer with directives and audio attachments, and a push-to-talk turn.
//!
//! What is NOT here: the HTTP/2 transport (a long-lived downchannel and multiplexed event streams; no HTTP/2 client exists in the code
//! base), the real Amazon endpoints, a client id (the owner must register an AVS product, and Amazon says AVS is no longer open to new
//! products), MP3 decoding of the spoken answer, and the consent card and settings that put it behind the voice seam. The [`Transport`]
//! and [`Http`] traits are the seams where those go. Nothing here talks to the network by itself.
//!
//! This provider replaces the whole turn (audio in, Alexa's own spoken answer out); it does not use IYAKKI's tools or approval cards,
//! and the person is told so (label [`LABEL`]).

use serde_json::{json, Value};

pub const LABEL: &str = "Amazon Alexa (Amazon's assistant, not IYAKKI's: your voice goes to Amazon)";
pub const AUDIO_FORMAT: &str = "AUDIO_L16_RATE_16000_CHANNELS_1";
/// The most audio one request may carry (60 seconds of 16 kHz 16-bit mono).
pub const MAX_AUDIO: usize = 16_000 * 2 * 60;

// ------------------------------------------------------------------------------------------------ Login with Amazon

/// A plain HTTPS form POST (the TLS client of the code base in the real system).
pub trait Http {
    fn post_form(&self, url: &str, form: &[(&str, &str)]) -> Result<(u16, String), String>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceCode {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    pub interval_secs: u64,
    pub expires_in: u64,
}

pub const LWA_CODE_URL: &str = "https://api.amazon.com/auth/O2/create/codepair";
pub const LWA_TOKEN_URL: &str = "https://api.amazon.com/auth/o2/token";

pub fn start_linking(http: &dyn Http, client_id: &str, product_id: &str, serial: &str) -> Result<DeviceCode, String> {
    if client_id.is_empty() || product_id.is_empty() {
        return Err("an AVS client id and product id are needed (the owner registers the product with Amazon)".into());
    }
    let scope_data = json!({"alexa:all": {"productID": product_id, "productInstanceAttributes": {"deviceSerialNumber": serial}}}).to_string();
    let (st, body) = http.post_form(LWA_CODE_URL, &[("response_type", "device_code"), ("client_id", client_id), ("scope", "alexa:all"), ("scope_data", &scope_data)])?;
    if st != 200 {
        return Err(format!("Amazon refused to start linking ({st})"));
    }
    let v: Value = serde_json::from_str(&body).map_err(|_| "Amazon's answer is not readable".to_string())?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let n = |k: &str, d: u64| v.get(k).and_then(Value::as_u64).unwrap_or(d);
    let c = DeviceCode { device_code: s("device_code"), user_code: s("user_code"), verification_uri: s("verification_uri"), interval_secs: n("interval", 5).clamp(1, 60), expires_in: n("expires_in", 600).min(3600) };
    if c.device_code.is_empty() || c.user_code.is_empty() || !c.verification_uri.starts_with("https://") {
        return Err("Amazon's answer lacks the code or the address".into());
    }
    Ok(c)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Poll {
    /// The person has not entered the code yet: ask again after the interval.
    Waiting,
    /// Ask less often.
    SlowDown,
    Expired,
    Denied,
    Linked(Tokens),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tokens {
    pub access: String,
    pub refresh: String,
    pub expires_in: u64,
}

fn tokens_of(body: &str) -> Result<Tokens, String> {
    let v: Value = serde_json::from_str(body).map_err(|_| "Amazon's answer is not readable".to_string())?;
    let access = v.get("access_token").and_then(Value::as_str).unwrap_or("");
    let refresh = v.get("refresh_token").and_then(Value::as_str).unwrap_or("");
    if access.is_empty() || refresh.is_empty() {
        return Err("Amazon's answer has no tokens".into());
    }
    Ok(Tokens { access: access.into(), refresh: refresh.into(), expires_in: v.get("expires_in").and_then(Value::as_u64).unwrap_or(3600).min(86_400) })
}

pub fn poll_linking(http: &dyn Http, client_id: &str, code: &DeviceCode) -> Result<Poll, String> {
    let (st, body) = http.post_form(LWA_TOKEN_URL, &[("grant_type", "device_code"), ("device_code", &code.device_code), ("user_code", &code.user_code), ("client_id", client_id)])?;
    if st == 200 {
        return tokens_of(&body).map(Poll::Linked);
    }
    let err = serde_json::from_str::<Value>(&body).ok().and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string)).unwrap_or_default();
    Ok(match err.as_str() {
        "authorization_pending" => Poll::Waiting,
        "slow_down" => Poll::SlowDown,
        "expired_token" => Poll::Expired,
        "access_denied" => Poll::Denied,
        _ => return Err(format!("Amazon answered {st} while linking")),
    })
}

/// A new access token from the refresh token. The refresh token may be replaced by Amazon; the caller stores what comes back.
pub fn refresh(http: &dyn Http, client_id: &str, refresh_token: &str) -> Result<Tokens, String> {
    let (st, body) = http.post_form(LWA_TOKEN_URL, &[("grant_type", "refresh_token"), ("refresh_token", refresh_token), ("client_id", client_id)])?;
    match st {
        200 => tokens_of(&body),
        400 | 401 => Err("the link to the Amazon account is no longer valid: link the device again".into()),
        s => Err(format!("Amazon answered {s} while refreshing the sign-in")),
    }
}

// ------------------------------------------------------------------------------------------------ events

fn id(prefix: &str) -> String {
    let mut b = [0u8; 8];
    let _ = iyakki_sec::random_fill(&mut b);
    format!("{prefix}-{}", b.iter().map(|x| format!("{x:02x}")).collect::<String>())
}

/// The JSON part of a `SpeechRecognizer.Recognize` event (push to talk: the profile is `NEAR_FIELD`).
pub fn recognize_event(message_id: &str, dialog_request_id: &str) -> Value {
    json!({
        "event": {
            "header": {"namespace": "SpeechRecognizer", "name": "Recognize", "messageId": message_id, "dialogRequestId": dialog_request_id},
            "payload": {"profile": "NEAR_FIELD", "format": AUDIO_FORMAT, "initiator": {"type": "TAP"}}
        }
    })
}

/// `SpeechSynthesizer.SpeechStarted` / `SpeechFinished` for a Speak directive's token.
pub fn speech_event(name: &str, token: &str) -> Result<Value, String> {
    if !matches!(name, "SpeechStarted" | "SpeechFinished") {
        return Err("not a speech event".into());
    }
    Ok(json!({"event": {"header": {"namespace": "SpeechSynthesizer", "name": name, "messageId": id("msg")}, "payload": {"token": token}}}))
}

/// The capabilities this client declares: speech in and out only; everything else is left out so Alexa does not send it.
pub fn capabilities() -> Value {
    json!({"capabilities": [
        {"type": "AlexaInterface", "interface": "SpeechRecognizer", "version": "2.0"},
        {"type": "AlexaInterface", "interface": "SpeechSynthesizer", "version": "1.3"},
        {"type": "AlexaInterface", "interface": "System", "version": "2.0"}
    ]})
}

// ------------------------------------------------------------------------------------------------ multipart

/// A multipart body of a JSON part and an audio part, and the Content-Type to send it with.
pub fn build_recognize(event: &Value, audio: &[u8]) -> Result<(String, Vec<u8>), String> {
    if audio.is_empty() || audio.len() > MAX_AUDIO {
        return Err("the recording is empty or longer than 60 seconds".into());
    }
    let boundary = id("iyakkiavs");
    let mut b = Vec::new();
    b.extend_from_slice(format!("--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{event}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"audio\"\r\nContent-Type: application/octet-stream\r\n\r\n").as_bytes());
    b.extend_from_slice(audio);
    b.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
    Ok((format!("multipart/form-data; boundary={boundary}"), b))
}

#[derive(Debug, PartialEq, Eq)]
pub struct Part {
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Part {
    pub fn header(&self, k: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.as_str())
    }
}

fn find(h: &[u8], n: &[u8], from: usize) -> Option<usize> {
    h.get(from..)?.windows(n.len()).position(|w| w == n).map(|i| i + from)
}

/// Split a `multipart/related` answer. Limits: 16 MiB, 16 parts; a part without its end boundary is an error.
pub fn parse_multipart(content_type: &str, body: &[u8]) -> Result<Vec<Part>, String> {
    let boundary = content_type.split(';').filter_map(|p| p.trim().strip_prefix("boundary=")).next().map(|b| b.trim_matches('"')).filter(|b| !b.is_empty() && b.len() <= 100).ok_or("the answer has no boundary")?;
    if body.len() > 16 << 20 {
        return Err("the answer is too large".into());
    }
    let delim = format!("--{boundary}").into_bytes();
    let mut parts = Vec::new();
    let mut pos = find(body, &delim, 0).ok_or("the answer has no first part")? + delim.len();
    loop {
        if body.get(pos..pos + 2) == Some(b"--") {
            return Ok(parts);
        }
        let head_start = find(body, b"\r\n", pos).ok_or("a part is cut short")? + 2;
        let head_end = find(body, b"\r\n\r\n", head_start - 2).ok_or("a part has no header end")?;
        let head = std::str::from_utf8(&body[head_start..head_end.max(head_start)]).map_err(|_| "a part header is not text")?;
        let headers = head.split("\r\n").filter_map(|l| l.split_once(':')).map(|(k, v)| (k.trim().to_string(), v.trim().to_string())).collect();
        let data_start = head_end + 4;
        let next = find(body, format!("\r\n--{boundary}").as_bytes(), data_start).ok_or("a part is not closed")?;
        parts.push(Part { headers, body: body[data_start..next].to_vec() });
        if parts.len() > 16 {
            return Err("too many parts".into());
        }
        pos = next + 2 + delim.len();
    }
}

// ------------------------------------------------------------------------------------------------ directives and a turn

#[derive(Debug, PartialEq, Eq)]
pub enum Directive {
    /// Alexa speaks: the audio attachment of this content id, and the token to report back with.
    Speak { token: String, audio_cid: String },
    ExpectSpeech { timeout_ms: u64 },
    StopCapture,
    /// Understood but not supported (not declared in the capabilities): ignored, and listed so the person can see it.
    Unsupported(String),
}

pub fn parse_directive(v: &Value) -> Result<Directive, String> {
    let d = v.get("directive").ok_or("not a directive")?;
    let h = d.get("header").ok_or("a directive without a header")?;
    let (ns, name) = (h.get("namespace").and_then(Value::as_str).unwrap_or(""), h.get("name").and_then(Value::as_str).unwrap_or(""));
    let p = d.get("payload").unwrap_or(&Value::Null);
    Ok(match (ns, name) {
        ("SpeechSynthesizer", "Speak") => {
            let cid = p.get("url").and_then(Value::as_str).and_then(|u| u.strip_prefix("cid:")).unwrap_or("");
            let token = p.get("token").and_then(Value::as_str).unwrap_or("");
            if cid.is_empty() || token.is_empty() {
                return Err("a Speak directive without its audio or token".into());
            }
            Directive::Speak { token: token.into(), audio_cid: cid.into() }
        }
        ("SpeechRecognizer", "ExpectSpeech") => Directive::ExpectSpeech { timeout_ms: p.get("timeoutInMilliseconds").and_then(Value::as_u64).unwrap_or(8000).min(60_000) },
        ("SpeechRecognizer", "StopCapture") => Directive::StopCapture,
        (a, b) => Directive::Unsupported(format!("{a}.{b}")),
    })
}

/// What one turn gave: Alexa's spoken audio (still encoded: MP3 in the documents found, decoded elsewhere), whether she wants the person
/// to answer, what was ignored, and the events to send back (speech started and finished).
#[derive(Debug, Default, PartialEq)]
pub struct Turn {
    pub speech: Vec<Vec<u8>>,
    pub expect_speech: bool,
    pub ignored: Vec<String>,
    pub report: Vec<Value>,
}

/// Interpret an answer's parts: JSON directives and the attachments they name.
pub fn interpret(parts: &[Part]) -> Result<Turn, String> {
    let mut turn = Turn::default();
    let cid_of = |p: &Part| p.header("Content-ID").map(|c| c.trim_matches(['<', '>']).to_string());
    for p in parts.iter().filter(|p| p.header("Content-Type").is_some_and(|t| t.starts_with("application/json"))) {
        let v: Value = serde_json::from_slice(&p.body).map_err(|_| "a directive is not readable")?;
        match parse_directive(&v)? {
            Directive::Speak { token, audio_cid } => {
                let a = parts.iter().find(|x| cid_of(x).as_deref() == Some(audio_cid.as_str())).ok_or_else(|| format!("the audio {audio_cid} is missing from the answer"))?;
                turn.speech.push(a.body.clone());
                turn.report.push(speech_event("SpeechStarted", &token)?);
                turn.report.push(speech_event("SpeechFinished", &token)?);
            }
            Directive::ExpectSpeech { .. } => turn.expect_speech = true,
            Directive::StopCapture => {}
            Directive::Unsupported(n) => turn.ignored.push(n),
        }
    }
    Ok(turn)
}

/// The transport: one event with an attachment out, the multipart answer back. (The real one is HTTP/2 with a persistent downchannel.)
pub trait Transport {
    /// POST to the events endpoint with the bearer token. Returns (status, content type of the answer, body).
    fn post_event(&mut self, bearer: &str, content_type: &str, body: &[u8]) -> Result<(u16, String, Vec<u8>), String>;
}

pub struct Session<'a> {
    pub client_id: String,
    pub tokens: Tokens,
    /// Seconds since the tokens were issued (the caller's clock).
    pub age_secs: u64,
    pub http: &'a dyn Http,
}

impl Session<'_> {
    /// One push-to-talk turn: refresh the sign-in when it is about to run out (or after a 403), send the recording, read the answer.
    pub fn turn(&mut self, t: &mut dyn Transport, audio: &[u8]) -> Result<Turn, String> {
        if self.age_secs + 60 >= self.tokens.expires_in {
            self.renew()?;
        }
        let ev = recognize_event(&id("msg"), &id("dlg"));
        let (ct, body) = build_recognize(&ev, audio)?;
        let mut r = t.post_event(&self.tokens.access, &ct, &body)?;
        if r.0 == 403 {
            self.renew()?;
            r = t.post_event(&self.tokens.access, &ct, &body)?;
        }
        match r.0 {
            200 => interpret(&parse_multipart(&r.1, &r.2)?),
            204 => Ok(Turn::default()),
            403 => Err("Amazon refused the sign-in: link the device again".into()),
            s => Err(format!("Amazon answered {s}")),
        }
    }

    fn renew(&mut self) -> Result<(), String> {
        self.tokens = refresh(self.http, &self.client_id, &self.tokens.refresh)?;
        self.age_secs = 0;
        Ok(())
    }
}

#[cfg(test)]
#[path = "avs_tests.rs"]
mod tests;
