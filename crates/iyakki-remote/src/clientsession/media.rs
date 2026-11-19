//! The media half of a client session (docs/development/CLIENT-SPLIT-DESIGN.md sections 6 and 7b). EXPERIMENTAL.
//!
//! Sound to the client, the text clipboard, camera frames of the instance, and the client's own camera and microphone as virtual sources.
//! Every request needs the capability negotiated in `hello`, the person's role, the broker (caller kind `Client`, so the access log shows the
//! device and the person) and, for the hard asks, a card. The rules and the state are in `iyakki_agent::clientmedia`; this file only carries them.

use super::*;
use iyakki_agent::clientmedia::{Accepted, CamState, Lease, MediaHost, OfferErr, Opened, Taken};
use iyakki_core_api::broker::Denial;
use iyakki_core_api::client::media as m;
use m::{Side, SourceKind};

/// How often the broker is asked again about a running sound or camera, so a revoke is seen within about two seconds.
const RECHECK: Duration = Duration::from_secs(2);
/// One camera frame per this long (5 per second).
const CAMERA_GAP: Duration = Duration::from_millis(1000 / m::CAMERA_FPS as u64);

struct CamOpen {
    after: u64,
    last: Instant,
    checked: Instant,
    seq: u32,
}

#[derive(Default)]
pub(super) struct MediaState {
    pub(super) audio: bool,
    audio_checked: Option<Instant>,
    cams: HashMap<String, CamOpen>,
    /// The text the person last copied on the machine during this session (kept until asked for, never sent unasked).
    last_copy: Option<String>,
    warned: Vec<String>,
}

/// What a media request needs: the shared parts, this session's number, the person and the device.
struct Ctx {
    host: Arc<MediaHost>,
    session: u64,
    person: String,
    dev: Device,
}

impl Session<'_> {
    fn media_err(&mut self, what: &str, code: &str, message: &str) {
        say(&mut self.out, &ServerMsg::MediaErr { what: what.into(), code: code.into(), message: message.into() });
    }

    /// The parts a media request needs, or a refusal said to the client.
    fn media_ctx(&mut self, what: &str, cap: &str) -> Option<Ctx> {
        let Some(host) = self.ep.media.clone() else {
            self.media_err(what, "unsupported", "this machine does not serve that to clients");
            return None;
        };
        let (Some(person), Some(dev), Some((session, _))) = (self.person.clone(), self.device.clone(), self.open.as_ref()) else {
            self.complain(ErrorCode::NotSignedIn, "sign in first");
            return None;
        };
        if !self.agreed.iter().any(|c| c == cap) {
            self.media_err(what, "capability_missing", &format!("this connection did not agree on {cap}: offer it in hello"));
            return None;
        }
        Some(Ctx { host, session: *session, person, dev })
    }

    /// Role, then the broker (which writes the access log line and asks the card of a hard ask).
    fn media_decide(&mut self, c: &Ctx, what: &str, cap: &str, unit: Option<&str>, label: &str) -> bool {
        if !self.ep.calls.role_allows(&c.person, cap) {
            (self.ep.audit)(&c.person, "client.call.refused", &format!("{cap} role {}", c.dev.id));
            self.media_err(what, ErrorCode::RoleDenied.as_str(), "your role does not allow that");
            return false;
        }
        match self.decide_cap(c, cap, unit, label) {
            Ok(()) => true,
            Err(d) => {
                self.media_err(what, d.code(), &d.to_string());
                false
            }
        }
    }

    fn decide_cap(&self, c: &Ctx, cap: &str, unit: Option<&str>, label: &str) -> Result<(), Denial> {
        let caller = Caller { app: "client".into(), kind: CallerKind::Client { device_id: c.dev.id.clone() }, person: c.person.clone(), device: c.dev.name.clone() };
        self.ep.broker.decide(&Call { caller: &caller, capability: cap, unit, label })
    }

    // ------------------------------------------------------------------------------------------ sound

    pub(super) fn audio_open(&mut self, side: Option<String>) {
        let Some(c) = self.media_ctx("audio", proto::cap::AUDIO_OUT) else { return };
        let Some(side) = Side::parse(side.as_deref()) else { return self.media_err("audio", "bad_request", "the side is client or both") };
        if !self.media_decide(&c, "audio", "speaker.play", None, "sound to the client") {
            return;
        }
        let opened = c.host.audio.open(c.session, &c.person, side);
        if opened != Opened::Same {
            (self.ep.audit)(&c.person, "client.audio.open", &format!("{} {}", side.name(), c.dev.id));
        }
        self.media.audio = true;
        self.media.audio_checked = Some(Instant::now());
        say(&mut self.out, &ServerMsg::AudioOk { rate: m::AUDIO_RATE, channels: 2, chunk_ms: m::CHUNK_MS, side: side.name().into() });
    }

    pub(super) fn audio_close(&mut self) {
        let Some(c) = self.media_ctx("audio", proto::cap::AUDIO_OUT) else { return };
        self.stop_audio(&c, "closed");
    }

    fn stop_audio(&mut self, c: &Ctx, reason: &str) {
        if c.host.audio.close(c.session) {
            (self.ep.audit)(&c.person, "client.audio.close", &format!("{reason} {}", c.dev.id));
        }
        if std::mem::take(&mut self.media.audio) {
            say(&mut self.out, &ServerMsg::AudioClosed { reason: reason.into() });
        }
    }

    // ------------------------------------------------------------------------------------------ clipboard

    pub(super) fn clipboard_put(&mut self, text: &str, confirmed: bool) {
        let Some(c) = self.media_ctx("clipboard", proto::cap::CLIPBOARD_TEXT) else { return };
        if !confirmed {
            return self.media_err("clipboard", "consent_required", "press the button for each use: the clipboard is never sent by itself");
        }
        let text = match m::clean_clipboard(text) {
            Ok(t) => t,
            Err(m::ClipError::TooBig) => return self.media_err("clipboard", "too_big", &format!("the clipboard text is limited to {} KB", m::MAX_CLIPBOARD / 1024)),
            Err(m::ClipError::Empty) => return self.media_err("clipboard", "empty", "there is no text to send"),
        };
        if self.screen.is_none() {
            return self.complain(ErrorCode::NotSignedIn, "sign in first");
        }
        // the press of the button is this use's answer; the broker still checks the grant and writes the log line
        c.host.consent.note(&c.dev.id, "clipboard.client");
        let ok = self.media_decide(&c, "clipboard", "clipboard.client", None, "text to the machine");
        c.host.consent.forget(&c.dev.id, "clipboard.client");
        if !ok {
            return;
        }
        let n = text.len();
        if let Some(screen) = self.screen.as_mut() {
            // pasted where the person is typing on the machine
            screen.input(Input::Event(iyakki_shell::ui::Event::Paste(text)));
        }
        (self.ep.audit)(&c.person, "client.clipboard.put", &format!("{n} bytes {}", c.dev.id));
        iyakki_agent::appevent_extra::clipboard_used();
        say(&mut self.out, &ServerMsg::ClipboardOk);
    }

    pub(super) fn clipboard_get(&mut self, confirmed: bool) {
        let Some(c) = self.media_ctx("clipboard", proto::cap::CLIPBOARD_TEXT) else { return };
        if !confirmed {
            return self.media_err("clipboard", "consent_required", "press the button for each use: the machine's clipboard is never sent by itself");
        }
        c.host.consent.note(&c.dev.id, "clipboard.client");
        let ok = self.media_decide(&c, "clipboard", "clipboard.client", None, "text from the machine");
        c.host.consent.forget(&c.dev.id, "clipboard.client");
        if !ok {
            return;
        }
        match self.media.last_copy.clone() {
            None => self.media_err("clipboard", "empty", "nothing has been copied on the machine yet"),
            Some(t) => {
                (self.ep.audit)(&c.person, "client.clipboard.get", &format!("{} bytes {}", t.len(), c.dev.id));
                iyakki_agent::appevent_extra::clipboard_used();
                say(&mut self.out, &ServerMsg::ClipboardText { text: t });
            }
        }
    }

    // ------------------------------------------------------------------------------------------ instance cameras

    /// Which cameras this person may watch from here (names, never device paths). Needs the `camera.view` grant like watching does.
    pub(super) fn camera_list(&mut self) {
        let Some(c) = self.media_ctx("camera", proto::cap::CAMERA_FRAMES) else { return };
        let Some(feed) = c.host.cameras.clone() else { return self.media_err("camera", "unsupported", "this machine has no camera to show to clients") };
        if !self.media_decide(&c, "camera", "camera.view", None, "list of cameras") {
            return;
        }
        let cameras: Vec<proto::CameraEntry> = feed.list(&c.person).into_iter().take(64).map(|(unit, name)| proto::CameraEntry { unit, name }).collect();
        (self.ep.audit)(&c.person, "client.camera.list", &format!("{} {}", cameras.len(), c.dev.id));
        say(&mut self.out, &ServerMsg::CameraList { cameras });
    }

    pub(super) fn camera_open(&mut self, unit: &str) {
        let Some(c) = self.media_ctx("camera", proto::cap::CAMERA_FRAMES) else { return };
        let Some(feed) = c.host.cameras.clone() else { return self.media_err("camera", "unsupported", "this machine has no camera to show to clients") };
        if unit.is_empty() || unit.len() > m::MAX_UNIT || unit.chars().any(char::is_control) {
            return self.media_err("camera", "bad_request", "that camera name is not understood");
        }
        let label = feed.label(&c.person, unit);
        if !self.media_decide(&c, "camera", "camera.view", Some(unit), &label) {
            return;
        }
        match feed.state(&c.person, unit) {
            CamState::Missing => return self.media_err("camera", "not_found", "no such camera, or it is not connected"),
            CamState::Busy(h) => return self.media_err("camera", "in_use", &format!("in use by {h}")),
            CamState::Refused(why) => return self.media_err("camera", "unavailable", &why),
            CamState::Ready => {}
        }
        // a camera is shared for viewing (the hub fans one capture out); an exclusive holder, or the person at the machine, comes first
        match c.host.leases.acquire(unit, Lease { session: c.session, who: c.dev.name.clone(), exclusive: false, local: false }) {
            Taken::InUse(who) => return self.media_err("camera", "in_use", &format!("in use by {who}")),
            Taken::Granted { .. } => {}
        }
        (self.ep.audit)(&c.person, "client.camera.view", &format!("{label} {}", c.dev.id));
        self.media.cams.insert(unit.to_string(), CamOpen { after: 0, last: Instant::now() - CAMERA_GAP, checked: Instant::now(), seq: 0 });
        say(&mut self.out, &ServerMsg::CameraOk { unit: unit.into(), fps: m::CAMERA_FPS });
    }

    pub(super) fn camera_close(&mut self, unit: &str) {
        let Some(c) = self.media_ctx("camera", proto::cap::CAMERA_FRAMES) else { return };
        self.stop_camera(&c, unit, "closed");
    }

    fn stop_camera(&mut self, c: &Ctx, unit: &str, reason: &str) {
        c.host.leases.release(unit, c.session);
        if self.media.cams.remove(unit).is_some() {
            say(&mut self.out, &ServerMsg::CameraClosed { unit: unit.into(), reason: reason.into() });
        }
    }

    // ------------------------------------------------------------------------------------------ the client's own devices

    pub(super) fn offer_open(&mut self, kind: &str) {
        let Some(k) = SourceKind::parse(kind) else { return self.media_err("offer", "bad_request", "an offer is a camera or a microphone") };
        let cap = if k == SourceKind::Camera { proto::cap::CLIENT_CAMERA } else { proto::cap::CLIENT_MICROPHONE };
        let Some(c) = self.media_ctx("offer", cap) else { return };
        let name = m::source_name(&c.dev.name, k);
        // a hard ask: the card on the machine, every time; the browser asks its own permission on the client
        if !self.media_decide(&c, "offer", k.capability(), None, &name) {
            return;
        }
        match c.host.sources.offer(k, c.session, &c.dev.id, &c.dev.name, &c.person, &c.host.tenant(&c.person)) {
            Ok(src) => {
                (self.ep.audit)(&c.person, &format!("client.offer.{}", k.word()), &format!("{} {}", src.name, c.dev.id));
                say(&mut self.out, &ServerMsg::OfferOk { kind: kind.into(), source: src.name });
            }
            Err(OfferErr::Already(n)) => self.media_err("offer", "in_use", &format!("{n} is already offered by this device from another connection")),
            Err(OfferErr::TooMany) => self.media_err("offer", "unsupported", "too many client devices are offering their devices"),
        }
    }

    pub(super) fn offer_close(&mut self, kind: &str) {
        let Some(k) = SourceKind::parse(kind) else { return self.media_err("offer", "bad_request", "an offer is a camera or a microphone") };
        let Some(c) = self.media_ctx("offer", if k == SourceKind::Camera { proto::cap::CLIENT_CAMERA } else { proto::cap::CLIENT_MICROPHONE }) else { return };
        if let Some(src) = c.host.sources.close(k, c.session) {
            (self.ep.audit)(&c.person, "client.offer.end", &format!("{} {}", src.name, c.dev.id));
            say(&mut self.out, &ServerMsg::OfferClosed { kind: kind.into(), reason: "closed".into() });
        }
    }

    /// A binary message from the client: the microphone or camera it offered. With no consumer on the instance yet the data is checked and
    /// dropped.
    pub(super) fn client_binary(&mut self, data: &[u8]) {
        let Some(host) = self.ep.media.clone() else { return self.complain(ErrorCode::Unsupported, "binary messages to the instance are not used yet") };
        let (Some((session, _)), Some(_)) = (self.open.as_ref().map(|(i, k)| (*i, k.clone())), self.person.as_ref()) else { return self.complain(ErrorCode::NotSignedIn, "sign in first") };
        let kind = match proto::split_binary(data) {
            Some((proto::STREAM_MIC_IN, _)) => SourceKind::Microphone,
            Some((proto::STREAM_CAMERA_IN, _)) => SourceKind::Camera,
            _ => return self.complain(ErrorCode::Unsupported, "that stream is not used by the instance"),
        };
        if let Accepted::Refused(why) = host.sources.accept(session, kind, &data[1..]) {
            let key = format!("{}:{why}", kind.word());
            if !self.media.warned.contains(&key) {
                self.media.warned.push(key);
                self.media_err("offer", why, &format!("a {} frame was refused ({why})", kind.word()));
            }
        }
    }

    // ------------------------------------------------------------------------------------------ the loop

    /// Called every turn of the session loop: sound, camera frames, the person's last copy, and the broker again for what runs.
    pub(super) fn pump_media(&mut self) {
        let Some(host) = self.ep.media.clone() else { return };
        let (Some(person), Some(dev), Some((session, _))) = (self.person.clone(), self.device.clone(), self.open.as_ref().map(|(i, k)| (*i, k.clone()))) else { return };
        let c = Ctx { host: host.clone(), session, person, dev };
        // what the person copied on the machine, held for when they ask
        if let Some(t) = self.screen.as_mut().and_then(|s| s.take_copy()) {
            self.media.last_copy = (t.len() <= m::MAX_CLIPBOARD).then_some(t);
        }
        if self.media.audio {
            if self.media.audio_checked.is_none_or(|t| t.elapsed() >= RECHECK) {
                self.media.audio_checked = Some(Instant::now());
                if self.decide_cap(&c, "speaker.play", None, "sound to the client").is_err() {
                    return self.stop_audio(&c, "revoked");
                }
            }
            let p = host.audio.pull(session);
            if p.flush {
                say(&mut self.out, &ServerMsg::AudioFlush);
            }
            for chunk in p.chunks {
                self.out.extend(ws::binary_frame(&proto::binary(proto::STREAM_AUDIO, &chunk)));
            }
        }
        let units: Vec<String> = self.media.cams.keys().cloned().collect();
        for unit in units {
            self.pump_camera(&c, &unit);
        }
    }

    fn pump_camera(&mut self, c: &Ctx, unit: &str) {
        let Some(feed) = c.host.cameras.clone() else { return };
        let Some(open) = self.media.cams.get_mut(unit) else { return };
        if open.last.elapsed() < CAMERA_GAP {
            return;
        }
        open.last = Instant::now();
        let recheck = open.checked.elapsed() >= Duration::from_secs(1);
        if recheck {
            open.checked = Instant::now();
        }
        let after = open.after;
        // the person at the machine (or an exclusive holder) can take the camera: say so and let go
        if !c.host.leases.holds(unit, c.session) {
            let who = c.host.leases.exclusive_holder(unit).unwrap_or_else(|| "another program".into());
            return self.stop_camera(c, unit, &format!("in_use: {who}"));
        }
        if recheck {
            if let Err(d) = self.decide_cap(c, "camera.view", Some(unit), unit) {
                return self.stop_camera(c, unit, d.code());
            }
            if let CamState::Busy(h) = feed.state(&c.person, unit) {
                return self.stop_camera(c, unit, &format!("in_use: {h}"));
            }
        }
        match feed.jpeg(&c.person, unit, after) {
            Ok(Some((seq, jpeg))) => {
                let n = {
                    let o = self.media.cams.get_mut(unit);
                    o.map(|o| {
                        o.after = seq;
                        o.seq = o.seq.wrapping_add(1);
                        o.seq
                    })
                };
                if let (Some(n), Some(p)) = (n, m::encode_camera(unit, 0, &jpeg)) {
                    // the number counts frames sent to this client; a skipped picture shows as a gap
                    let mut p = p;
                    let at = 1 + unit.len();
                    p[at..at + 4].copy_from_slice(&n.to_le_bytes());
                    self.out.extend(ws::binary_frame(&proto::binary(proto::STREAM_CAMERA, &p)));
                }
            }
            Ok(None) => {}
            Err(_) => self.stop_camera(c, unit, "failed"),
        }
    }

    /// The session ended: the machine's speakers come back, the leases go, the client's devices vanish from the instance.
    pub(super) fn media_teardown(&mut self) {
        let Some(host) = self.ep.media.clone() else { return };
        let Some((session, _)) = self.open.as_ref() else { return };
        let session = *session;
        host.audio.close(session);
        host.leases.release_session(session);
        for src in host.sources.vanish(session) {
            (self.ep.audit)(self.person.as_deref().unwrap_or(""), "client.offer.end", &format!("{} disconnected", src.name));
        }
        self.media = MediaState::default();
    }
}
