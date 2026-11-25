//! The chat backend of a team's local model host (`models.local`; F-TCFG-14).
//!
//! [`TeamLocalChat`] is a [`ChatBackend`] whose every call goes through [`super::runtime::call_local_target`]: the gate (`provider_gate`:
//! entry ready, model on the list, role, counters), the fair-share slot with its queue timeout, and the bearer token read from the secret
//! mechanism for that one call only. The call itself uses the ordinary [`ProviderClient`] (the same request and answer code as every
//! other provider): style `ollama` is Ollama's OpenAI-compatible endpoint `/v1/chat/completions`, style `openai_compatible` is
//! `{base}/v1/chat/completions`. Streaming is off (one answer per call).
//!
//! What this file adds around the client ([`Guard`], an [`Upstream`] wrapper):
//! * the token is put in the `Authorization` header by the guard for this call and held in a zeroising buffer; a header the client
//!   built is replaced; the token is never in an error text, an audit line or a log (every error is passed through a redaction);
//! * the request body may not be larger than the entry's `max_request_kb` (refused before anything is sent); the answer may not be larger
//!   than [`MAX_RESPONSE_BYTES`];
//! * the whole call may not take longer than the entry's `max_request_secs` (the transport's timeout);
//! * redirects are never followed (the transport does not), so a host cannot bounce the token elsewhere;
//! * errors become plain sentences that never repeat the host's address: 401/403 "the host refused the team's token", 404 "no such model
//!   or path", 429 "busy", 5xx "the host failed", a timeout, or "could not be reached".
//!
//! Tool calls and streaming: [`TeamLocalChat::complete_tools`] sends the tool list and returns the host's tool calls; with a
//! `streamer` set, `complete_tools_stream` reads the answer as server-sent events through the same guard (token header replaced, request
//! size cap, answer cap, whole-call deadline, no redirects). Without a streamer it falls back to one answer per call.
//!
//! A team run uses [`OwnedTeamChat`] (M49 2026-11-13): the same backends with the data they borrow held by value, so a run's worker
//! thread can own one (`webservice/tenantcfg.rs::team_backend`). Never run against a real Ollama, Lemonade or GPU.

use std::sync::Arc;
use std::time::Duration;

use iyakki_proxy::{StreamHead, Streamer, Upstream, UpstreamResponse};
use zeroize::Zeroizing;

use super::api::{emit, Env};
use super::runtime::{record_tokens, with_local_target, with_provider_target, LocalTarget, ProviderTarget};
use crate::chat::{ChatBackend, Message, ModelTurn, ToolSpec};
use crate::providers::{valid_model_name, Kind, ProviderCfg, ProviderClient};
use crate::vault::Secrets;

/// The largest answer body read from a host.
pub const MAX_RESPONSE_BYTES: usize = 4 << 20;

/// Makes the transport for one call; the argument is the call's time limit. The real one is [`real_transport`].
pub type Transport = Arc<dyn Fn(Duration) -> Arc<dyn Upstream> + Send + Sync>;

/// HTTP/HTTPS through `ureq` (rustls): no redirects, any status returned, the time limit for the whole call.
pub fn real_transport() -> Transport {
    Arc::new(|t| Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(t)))
}

/// Makes the streaming transport for one call; the argument is the time to the first byte. The real one is [`real_streamer`].
pub type StreamTransport = Arc<dyn Fn(Duration) -> Arc<dyn Streamer> + Send + Sync>;

/// HTTP/HTTPS streaming through `ureq` (rustls): no redirects, any status returned.
pub fn real_streamer() -> StreamTransport {
    Arc::new(iyakki_proxy::https::chat_streamer)
}

/// A reader that ends with an error after the whole call has taken too long, and with end-of-stream after `left` bytes.
struct Bounded {
    inner: Box<dyn std::io::Read + Send>,
    deadline: std::time::Instant,
    left: usize,
}

impl std::io::Read for Bounded {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if std::time::Instant::now() >= self.deadline {
            return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "timed out"));
        }
        if self.left == 0 {
            return Err(std::io::Error::other("TOO_LARGE_RESPONSE"));
        }
        let cap = buf.len().min(self.left);
        let n = self.inner.read(&mut buf[..cap])?;
        self.left -= n;
        Ok(n)
    }
}

/// [`Guard`] for the streaming transport.
struct StreamGuard {
    inner: Arc<dyn Streamer>,
    token: Option<Zeroizing<String>>,
    max_request: usize,
    max_response: usize,
    max_secs: u64,
}

impl Streamer for StreamGuard {
    fn open(&self, _: &str, _: &[(String, String)]) -> Result<StreamHead, String> {
        Err("this transport only posts".into())
    }

    fn post(&self, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<StreamHead, String> {
        if body.len() > self.max_request {
            return Err(format!("TOO_LARGE_REQUEST {}", self.max_request / 1024));
        }
        let mut h: Vec<(String, String)> = headers.iter().filter(|(k, _)| !k.eq_ignore_ascii_case("authorization")).cloned().collect();
        if let Some(t) = &self.token {
            h.push(("Authorization".into(), format!("Bearer {}", t.as_str())));
        }
        let mut head = self.inner.post(url, &h, body)?;
        head.body = Box::new(Bounded { inner: head.body, deadline: std::time::Instant::now() + Duration::from_secs(self.max_secs.max(1)), left: self.max_response });
        Ok(head)
    }
}

/// Wraps the transport for one call: the token, the size limits and the redaction.
struct Guard {
    inner: Arc<dyn Upstream>,
    token: Option<Zeroizing<String>>,
    max_request: usize,
    max_response: usize,
}

impl Upstream for Guard {
    fn request(&self, method: &str, url: &str, headers: &[(String, String)], body: &[u8]) -> Result<UpstreamResponse, String> {
        if body.len() > self.max_request {
            return Err(format!("TOO_LARGE_REQUEST {}", self.max_request / 1024));
        }
        let mut h: Vec<(String, String)> = headers.iter().filter(|(k, _)| !k.eq_ignore_ascii_case("authorization")).cloned().collect();
        if let Some(t) = &self.token {
            h.push(("Authorization".into(), format!("Bearer {}", t.as_str())));
        }
        let r = self.inner.request(method, url, &h, body)?;
        if r.body.len() > self.max_response {
            return Err("TOO_LARGE_RESPONSE".into());
        }
        Ok(r)
    }
}

/// Take the token and the host's own address out of a text (they must never reach a person, an audit line or a log).
fn redact(s: &str, token: Option<&str>, hide: &[&str]) -> String {
    let mut out = match token.filter(|t| !t.is_empty()) {
        Some(t) => s.replace(t, "[token]"),
        None => s.to_string(),
    };
    for h in hide.iter().filter(|h| !h.is_empty()) {
        out = out.replace(h, "[host]");
    }
    out
}

/// The sentence a person gets for what went wrong. Never the token and never the host's own address (`hide`); what the host wrote is
/// kept short, left out for a server error, and cleaned of control characters.
pub fn explain(err: &str, token: Option<&str>, max_secs: u64, hide: &[&str]) -> String {
    let e = redact(err, token, hide);
    if let Some(k) = e.strip_prefix("TOO_LARGE_REQUEST ") {
        return format!("the request is larger than this host takes ({k} KB); shorten the conversation");
    }
    if e == "TOO_LARGE_RESPONSE" {
        return format!("the host's answer is larger than {} MB; it was not read", MAX_RESPONSE_BYTES >> 20);
    }
    if let Some(rest) = e.strip_prefix("HTTP ") {
        let code: u16 = rest.split(|c: char| !c.is_ascii_digit()).next().and_then(|c| c.parse().ok()).unwrap_or(0);
        let detail: String = rest.split_once(": ").map_or("", |(_, d)| d).chars().filter(|c| !c.is_control()).take(120).collect();
        let detail = if detail.is_empty() { String::new() } else { format!(": {detail}") };
        return match code {
            401 | 403 => format!("the host refused the team's token (HTTP {code}); ask a team admin to check the token of the local model host"),
            404 => format!("the host does not know this model or API path (HTTP 404); check the API style and the model name{detail}"),
            408 | 504 => format!("the host did not answer in time (HTTP {code})"),
            429 => "the host is busy and asked to wait (HTTP 429); try again in a moment".to_string(),
            500..=599 => format!("the host failed (HTTP {code})"),
            _ => format!("the host refused the request (HTTP {code}){detail}"),
        };
    }
    let l = e.to_ascii_lowercase();
    if l.contains("timed out") || l.contains("timeout") {
        return format!("the host did not answer within {max_secs} seconds");
    }
    if l.contains("not json") || l.contains("unexpected response") || l.contains("no choices") || l.contains("returned no") {
        return "the host answered, but not in the shape of its API style; check the API style of the entry".to_string();
    }
    "the local model host could not be reached".to_string()
}

fn config_for(t: &LocalTarget) -> Result<ProviderCfg, String> {
    let host = iyakki_sec::url_host(t.base).ok_or("the host address of this entry is not usable")?;
    if host.is_empty() || t.base.trim_end_matches('/').rsplit_once("://").is_some_and(|(_, r)| r.contains('/') || r.contains('@')) {
        return Err("the host address of this entry is not usable".into());
    }
    let b = t.base.trim_end_matches('/');
    let mut cfg = match t.style {
        "ollama" => ProviderCfg::new("team-local", Kind::Ollama),
        "openai_compatible" => ProviderCfg::new("team-local", Kind::Compat),
        _ => return Err("the entry has no API style".into()),
    };
    cfg.base_url = Some(if cfg.kind == Kind::Compat && !b.ends_with("/v1") { format!("{b}/v1") } else { b.to_string() });
    Ok(cfg)
}

/// One text answer from the host named by `t`.
pub fn answer(t: &LocalTarget, model: &str, history: &[Message], transport: &Transport) -> Result<String, String> {
    if !valid_model_name(model) {
        return Err("invalid model name".into());
    }
    if history.is_empty() {
        return Err("nothing to ask".into());
    }
    let cfg = config_for(t)?;
    let guard = Arc::new(Guard { inner: transport(Duration::from_secs(t.max_secs.max(1))), token: t.token.map(|s| Zeroizing::new(s.to_string())), max_request: t.max_request_bytes, max_response: MAX_RESPONSE_BYTES });
    let client = ProviderClient::new(cfg, Secrets::new(None, guard.clone()), guard);
    let authority = t.base.split_once("://").map_or(t.base, |(_, r)| r);
    let host = authority.rsplit_once(':').map_or(authority, |(h, _)| h);
    client.chat(model, history).map_err(|e| explain(&e, t.token, t.max_secs, &[authority, host]))
}

fn client_for(t: &LocalTarget, guard: Arc<Guard>) -> Result<ProviderClient, String> {
    let cfg = config_for(t)?;
    Ok(ProviderClient::new(cfg, Secrets::new(None, guard.clone()), guard))
}

fn hide_of(t: &LocalTarget) -> (String, String) {
    let authority = t.base.split_once("://").map_or(t.base, |(_, r)| r).to_string();
    let host = authority.rsplit_once(':').map_or(authority.as_str(), |(h, _)| h).to_string();
    (authority, host)
}

/// One model turn (text or tool calls) from the host named by `t`; one request, no streaming.
pub fn answer_turn(t: &LocalTarget, model: &str, history: &[Message], tools: &[ToolSpec], transport: &Transport) -> Result<ModelTurn, String> {
    if !valid_model_name(model) {
        return Err("invalid model name".into());
    }
    if history.is_empty() {
        return Err("nothing to ask".into());
    }
    let guard = Arc::new(Guard { inner: transport(Duration::from_secs(t.max_secs.max(1))), token: t.token.map(|s| Zeroizing::new(s.to_string())), max_request: t.max_request_bytes, max_response: MAX_RESPONSE_BYTES });
    let client = client_for(t, guard)?;
    let (a, h) = hide_of(t);
    client.chat_turn(model, history, tools).map_err(|e| explain(&e, t.token, t.max_secs, &[&a, &h]))
}

/// One model turn read as the host writes it; `on_text` gets each piece of visible text (`false` stops the read).
pub fn answer_turn_stream(t: &LocalTarget, model: &str, history: &[Message], tools: &[ToolSpec], transport: &Transport, streamer: &StreamTransport, on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
    if !valid_model_name(model) {
        return Err("invalid model name".into());
    }
    if history.is_empty() {
        return Err("nothing to ask".into());
    }
    let guard = Arc::new(Guard { inner: transport(Duration::from_secs(t.max_secs.max(1))), token: t.token.map(|s| Zeroizing::new(s.to_string())), max_request: t.max_request_bytes, max_response: MAX_RESPONSE_BYTES });
    let client = client_for(t, guard)?;
    let sg = StreamGuard { inner: streamer(Duration::from_secs(t.max_secs.max(1))), token: t.token.map(|s| Zeroizing::new(s.to_string())), max_request: t.max_request_bytes, max_response: MAX_RESPONSE_BYTES, max_secs: t.max_secs };
    let (a, h) = hide_of(t);
    client.chat_turn_stream(model, history, tools, &sg, on_text).map_err(|e| explain(&e, t.token, t.max_secs, &[&a, &h]))
}

/// A team's local host as a chat backend.
pub struct TeamLocalChat<'a> {
    pub env: &'a Env<'a>,
    pub team: &'a str,
    pub name: &'a str,
    pub user: &'a str,
    pub user_rank: i32,
    pub model: &'a str,
    pub clock: &'a (dyn Fn() -> u64 + Send + Sync),
    pub transport: Transport,
    /// The streaming transport; without one `complete_tools_stream` answers in one piece.
    pub streamer: Option<StreamTransport>,
}

impl ChatBackend for TeamLocalChat<'_> {
    fn provider_name(&self) -> String {
        self.name.to_string()
    }

    fn complete(&self, history: &[Message]) -> Result<String, String> {
        let now = (self.clock)();
        let r = with_local_target(self.env, self.team, self.name, self.user, self.user_rank, self.model, now, self.clock, &mut |t| answer(t, self.model, history, &self.transport));
        self.record(r.as_ref().map(|a| (a.len(), 0))).and(r)
    }

    fn complete_tools(&self, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        if tools.is_empty() {
            return self.complete(history).map(|text| ModelTurn { text, calls: vec![] });
        }
        let now = (self.clock)();
        let r = with_local_target(self.env, self.team, self.name, self.user, self.user_rank, self.model, now, self.clock, &mut |t| answer_turn(t, self.model, history, tools, &self.transport));
        self.record(r.as_ref().map(|a| (a.text.len(), a.calls.len()))).and(r)
    }

    fn complete_tools_stream(&self, history: &[Message], tools: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        let Some(streamer) = &self.streamer else { return self.complete_tools(history, tools) };
        let now = (self.clock)();
        let r = with_local_target(self.env, self.team, self.name, self.user, self.user_rank, self.model, now, self.clock, &mut |t| answer_turn_stream(t, self.model, history, tools, &self.transport, streamer, on_text));
        self.record(r.as_ref().map(|a| (a.text.len(), a.calls.len()))).and(r)
    }
}

impl TeamLocalChat<'_> {
    // what is recorded: the outcome and the sizes, never the question, the answer, the token or the address
    fn record(&self, r: Result<(usize, usize), &String>) -> Result<(), String> {
        match r {
            Ok((bytes, calls)) => emit(self.env, self.user, "tenant.local.answer", &format!("name={} team={} model={} ok=true answer_bytes={bytes} tool_calls={calls}", self.name, self.team, self.model)),
            Err(e) => emit(self.env, self.user, "tenant.local.failed", &format!("name={} team={} model={} reason={}", self.name, self.team, self.model, e.chars().take(120).collect::<String>())),
        }
        Ok(())
    }
}

// ------------------------------------------------------------------------------------------------------------------ team providers

/// The largest request body and the longest call for a provider entry (it has no limits of its own).
const PROVIDER_MAX_REQUEST: usize = 1 << 20;
const PROVIDER_MAX_SECS: u64 = 120;

fn provider_cfg(p: &ProviderTarget) -> Result<ProviderCfg, String> {
    let mut cfg = match p.api {
        "ollama" => ProviderCfg::new("team-provider", Kind::Ollama),
        "openai" | "other" => ProviderCfg::new("team-provider", Kind::Compat),
        "anthropic" => return Err("a team provider with the Anthropic API is not supported by this backend yet".into()),
        _ => return Err("the entry has no API style".into()),
    };
    let b = p.base_url.trim().trim_end_matches('/');
    let (scheme, rest) = b.split_once("://").ok_or("the base URL of this entry is not usable")?;
    if !matches!(scheme, "http" | "https") || rest.is_empty() || rest.split('/').next().is_some_and(|a| a.contains('@')) {
        return Err("the base URL of this entry is not usable".into());
    }
    cfg.base_url = Some(b.to_string());
    Ok(cfg)
}

/// One model turn from a team provider (one request; `tools` may be empty). Errors never repeat the key or the address.
pub fn provider_turn(p: &ProviderTarget, model: &str, history: &[Message], tools: &[ToolSpec], transport: &Transport) -> Result<ModelTurn, String> {
    if !valid_model_name(model) {
        return Err("invalid model name".into());
    }
    if history.is_empty() {
        return Err("nothing to ask".into());
    }
    let cfg = provider_cfg(p)?;
    let guard = Arc::new(Guard { inner: transport(Duration::from_secs(PROVIDER_MAX_SECS)), token: p.key.map(|s| Zeroizing::new(s.to_string())), max_request: PROVIDER_MAX_REQUEST, max_response: MAX_RESPONSE_BYTES });
    let client = ProviderClient::new(cfg, Secrets::new(None, guard.clone()), guard);
    let authority = p.base_url.split_once("://").map_or(p.base_url, |(_, r)| r).split('/').next().unwrap_or("");
    let host = authority.rsplit_once(':').map_or(authority, |(h, _)| h);
    client.chat_turn(model, history, tools).map_err(|e| explain(&e, p.key, PROVIDER_MAX_SECS, &[authority, host]).replace("local model host", "provider").replace("the host", "the provider").replace("this host", "this provider"))
}

/// A team's model provider (`models.provider`) as a chat backend: the gate, the vault key for this call only, the provider client, and
/// the answer's tokens added to the person's day count.
pub struct TeamProviderChat<'a> {
    pub env: &'a Env<'a>,
    pub team: &'a str,
    pub name: &'a str,
    pub user: &'a str,
    pub user_rank: i32,
    pub model: &'a str,
    pub clock: &'a (dyn Fn() -> u64 + Send + Sync),
    pub transport: Transport,
}

impl TeamProviderChat<'_> {
    fn turn(&self, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        let now = (self.clock)();
        let r = with_provider_target(self.env, self.team, self.name, self.user, self.user_rank, self.model, now, &mut |p| {
            let turn = provider_turn(p, self.model, history, tools, &self.transport)?;
            // the provider's own count when it says one, otherwise a rough one from the sizes (about four bytes a token)
            let tokens = crate::providers::reported_usage().map_or_else(|| (history.iter().map(|m| m.content.len()).sum::<usize>() + turn.text.len()) as u64 / 4 + 1, |(i, o)| i + o);
            record_tokens(self.env, "models.provider", Some(self.team), self.name, self.user, now, tokens);
            Ok(turn)
        });
        match &r {
            Ok(t) => emit(self.env, self.user, "tenant.provider.answer", &format!("name={} team={} model={} ok=true answer_bytes={} tool_calls={}", self.name, self.team, self.model, t.text.len(), t.calls.len())),
            Err(e) => emit(self.env, self.user, "tenant.provider.failed", &format!("name={} team={} model={} reason={}", self.name, self.team, self.model, e.chars().take(120).collect::<String>())),
        }
        r
    }
}

impl ChatBackend for TeamProviderChat<'_> {
    fn provider_name(&self) -> String {
        self.name.to_string()
    }

    fn complete(&self, history: &[Message]) -> Result<String, String> {
        self.turn(history, &[]).map(|t| t.text)
    }

    fn complete_tools(&self, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.turn(history, tools)
    }
}

/// Which kind of team entry an [`OwnedTeamChat`] talks to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TeamKind {
    Local,
    Provider,
}

/// A team's local host or provider as a backend that owns what it needs (data folder, secret store, audit sink, transports), so it can
/// live as long as a run. Every call builds the borrowing backend and goes through the same gate, counters, caps and audit lines.
pub struct OwnedTeamChat {
    pub kind: TeamKind,
    pub dir: std::path::PathBuf,
    pub vault: Arc<dyn crate::tenantconfig::vault::SecretStore>,
    pub audit: Arc<dyn super::api::ConfigAudit>,
    pub team: String,
    pub name: String,
    pub user: String,
    pub user_rank: i32,
    pub model: String,
    pub transport: Transport,
    pub streamer: Option<StreamTransport>,
    pub clock: Arc<dyn Fn() -> u64 + Send + Sync>,
}

impl OwnedTeamChat {
    fn with<R>(&self, f: impl FnOnce(&dyn ChatBackend) -> R) -> R {
        let env = Env { dir: &self.dir, vault: self.vault.clone(), audit: self.audit.as_ref() };
        let clock = || (self.clock)();
        match self.kind {
            TeamKind::Local => f(&TeamLocalChat { env: &env, team: &self.team, name: &self.name, user: &self.user, user_rank: self.user_rank, model: &self.model, clock: &clock, transport: self.transport.clone(), streamer: self.streamer.clone() }),
            TeamKind::Provider => f(&TeamProviderChat { env: &env, team: &self.team, name: &self.name, user: &self.user, user_rank: self.user_rank, model: &self.model, clock: &clock, transport: self.transport.clone() }),
        }
    }
}

impl ChatBackend for OwnedTeamChat {
    fn provider_name(&self) -> String {
        self.name.clone()
    }
    fn complete(&self, history: &[Message]) -> Result<String, String> {
        self.with(|b| b.complete(history))
    }
    fn complete_tools(&self, history: &[Message], tools: &[ToolSpec]) -> Result<ModelTurn, String> {
        self.with(|b| b.complete_tools(history, tools))
    }
    fn complete_tools_stream(&self, history: &[Message], tools: &[ToolSpec], on_text: &mut dyn FnMut(&str) -> bool) -> Result<ModelTurn, String> {
        self.with(|b| b.complete_tools_stream(history, tools, on_text))
    }
}
