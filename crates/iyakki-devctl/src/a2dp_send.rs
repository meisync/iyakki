//! Bluetooth audio, the sending side as one running stream (F-NET-26): this computer's sound goes to headphones or a speaker.
//!
//! [`Session`] drives [`crate::avdtp_src::Source`] over two message channels (the signalling channel and, once the sink accepted
//! Open, the media channel), then turns PCM of any rate and channel count into SBC frames ([`crate::sbc::Encoder`]) and RTP
//! packets ([`crate::avdtp_src::Packetizer`]), or, when the person has the audio add-on and the headphones offer AAC, into AAC frames
//! made by the add-on's coder ([`crate::a2dp_aac`]) in LATM packets. Sockets are not here: a [`Channel`] is the real L2CAP socket in
//! [`crate::a2dp_send_sys`] and a scripted one in the tests, so the protocol and the encoding are tested against this crate's own
//! sink and decoder, not against a radio. Pacing keeps the sound from being sent faster than it plays.

use std::collections::VecDeque;
use std::io;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::avdtp::{Msg, MsgType, Reassembler};
use crate::avdtp_src::{frames_per_packet, Packetizer, Source, SrcAction, SrcState};
use crate::a2dp_aac::{AacBackend, AacConfig, AacEncode, AacPacketizer};
use crate::bthid::{Channel, Recv};
use crate::sbc::{ChannelMode, Encoder, Params};

/// Largest media packet put on the air (the L2CAP default message size leaves this much for the RTP payload).
pub const MEDIA_MTU: usize = 650;

/// Converts interleaved PCM of one rate and channel count to another (linear interpolation; the encoder's filter bank band-limits).
pub struct Converter {
    in_rate: u32,
    in_ch: usize,
    out_rate: u32,
    out_ch: usize,
    /// position of the next output sample, in input samples counted from `prev`
    pos: f64,
    prev: Vec<f64>,
}

impl Converter {
    pub fn new(in_rate: u32, in_ch: usize, out_rate: u32, out_ch: usize) -> Converter {
        Converter { in_rate: in_rate.max(1), in_ch: in_ch.max(1), out_rate, out_ch: out_ch.max(1), pos: 0.0, prev: vec![0.0; in_ch.max(1)] }
    }

    pub fn matches(&self, rate: u32, ch: usize) -> bool {
        self.in_rate == rate && self.in_ch == ch.max(1)
    }

    fn mix(&self, frame: &[f64]) -> Vec<f64> {
        match (self.in_ch, self.out_ch) {
            (a, b) if a == b => frame.to_vec(),
            (_, 1) => vec![frame.iter().sum::<f64>() / frame.len() as f64],
            (1, b) => vec![frame[0]; b],
            (_, b) => (0..b).map(|c| frame[c.min(frame.len() - 1)]).collect(),
        }
    }

    pub fn run(&mut self, pcm: &[i16]) -> Vec<i16> {
        let step = self.in_rate as f64 / self.out_rate as f64;
        let n = pcm.len() / self.in_ch;
        let mut out = Vec::with_capacity((n as f64 / step) as usize * self.out_ch + 8);
        // input sample k (0-based) sits at position k+1 relative to prev (which is position 0)
        while self.pos < n as f64 {
            let i = self.pos.floor() as usize;
            let f = self.pos - i as f64;
            let a: Vec<f64> = (0..self.in_ch).map(|c| if i == 0 { self.prev[c] } else { pcm[(i - 1) * self.in_ch + c] as f64 }).collect();
            let b: Vec<f64> = (0..self.in_ch).map(|c| pcm[i * self.in_ch + c] as f64).collect();
            let lerp: Vec<f64> = a.iter().zip(&b).map(|(x, y)| x + (y - x) * f).collect();
            for v in self.mix(&lerp) {
                out.push(v.round().clamp(-32768.0, 32767.0) as i16);
            }
            self.pos += step;
        }
        self.pos -= n as f64;
        if n > 0 {
            for c in 0..self.in_ch {
                self.prev[c] = pcm[(n - 1) * self.in_ch + c] as f64;
            }
        }
        out
    }
}

/// Sends nothing early: at most `lead` of sound is ahead of the clock.
struct Pacer {
    start: Instant,
    sent: u64,
    rate: u32,
    lead: Duration,
}

impl Pacer {
    fn wait(&mut self, samples: u64) {
        self.sent += samples;
        let due = Duration::from_secs_f64(self.sent as f64 / self.rate as f64);
        let ahead = due.saturating_sub(self.start.elapsed());
        if ahead > self.lead {
            std::thread::sleep(ahead - self.lead);
        }
    }
}

/// What a running stream says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SendInfo {
    pub codec: String,
    pub packets: u64,
    /// the sink's reported delay in tenths of a millisecond
    pub delay: Option<u16>,
}

pub struct Session<S: Channel, M: Channel> {
    sig: S,
    media: M,
    src: Source,
    re: Reassembler,
    enc: Option<Encoder>,
    pack: Option<Packetizer>,
    conv: Option<Converter>,
    pend: VecDeque<i16>,
    frames: Vec<Vec<u8>>,
    pacer: Option<Pacer>,
    packets: u64,
    delay: Option<u16>,
    ended: bool,
    /// The SBC settings (None for an AAC stream).
    params: Option<Params>,
    aac: Option<AacTx>,
}

/// The AAC half of a session: the coder (the add-on's, behind [`AacEncode`]), the packetiser and the settings.
struct AacTx {
    enc: Box<dyn AacEncode>,
    pack: AacPacketizer,
    cfg: AacConfig,
}

fn next_msg<S: Channel>(sig: &mut S, re: &mut Reassembler, wait: Duration) -> Result<Option<Msg>, String> {
    let until = Instant::now() + wait;
    loop {
        let left = until.saturating_duration_since(Instant::now());
        match sig.recv(left).map_err(|e| format!("the link failed: {e}"))? {
            Recv::Timeout => return Ok(None),
            Recv::Closed => return Err("the device closed the connection".into()),
            Recv::Data(d) => match re.push(&d) {
                Ok(Some(m)) => return Ok(Some(m)),
                Ok(None) => {}
                Err(e) => return Err(format!("the device sent a broken message ({e})")),
            },
        }
        if Instant::now() >= until {
            return Ok(None);
        }
    }
}

fn send_msg<S: Channel>(sig: &mut S, m: &Msg) -> Result<(), String> {
    sig.send(&m.encode()).map_err(|e| format!("could not send to the device: {e}"))
}

impl<S: Channel, M: Channel> Session<S, M> {
    /// Runs the set-up: Discover, capabilities, Set Configuration, Open, the media channel (`open_media`), Start. `paced` makes
    /// [`write`](Self::write) wait so that the sound is not sent faster than it plays.
    pub fn open(sig: S, open_media: impl FnOnce() -> io::Result<M>, paced: bool, step_wait: Duration) -> Result<Session<S, M>, String> {
        Session::open_with(sig, open_media, paced, step_wait, None)
    }

    /// [`open`](Self::open), and with `aac` (the add-on's coders) AAC is chosen when the headphones offer it; otherwise SBC.
    pub fn open_with(mut sig: S, open_media: impl FnOnce() -> io::Result<M>, paced: bool, step_wait: Duration, aac: Option<Arc<dyn AacBackend>>) -> Result<Session<S, M>, String> {
        let mut src = if aac.is_some() { Source::new().with_aac() } else { Source::new() };
        let mut cfg_aac: Option<AacConfig> = None;
        let mut re = Reassembler::default();
        let mut cfg: Option<Params> = None;
        let mut media: Option<M> = None;
        let mut open_media = Some(open_media);
        let mut cmd = Some(src.begin());
        let mut started = false;
        let mut guard = 0;
        while !started {
            guard += 1;
            if guard > 40 {
                return Err("the device did not finish the audio set-up".into());
            }
            if let Some(c) = cmd.take() {
                send_msg(&mut sig, &c)?;
            }
            let Some(reply) = next_msg(&mut sig, &mut re, step_wait)? else { return Err("the device did not answer in time".into()) };
            let (next, acts) = src.on_msg(&reply).map_err(|e| e.text())?;
            if let Some(n) = next {
                cmd = Some(n);
            }
            for a in acts {
                match a {
                    SrcAction::Configured(p) => cfg = Some(p),
                    SrcAction::ConfiguredAac(c) => cfg_aac = Some(c),
                    SrcAction::Opened => {
                        let f = open_media.take().ok_or("opened twice")?;
                        media = Some(f().map_err(|e| format!("could not open the audio channel: {e}"))?);
                        cmd = src.start();
                    }
                    SrcAction::Started => started = true,
                    SrcAction::Ended => return Err("the device ended the stream during set-up".into()),
                    SrcAction::Delay(_) | SrcAction::Suspended => {}
                }
            }
            if let Some(m) = &cmd {
                // a reply to a command of the sink is sent at once, the next command of ours on the next turn
                if m.kind == MsgType::Accept || m.kind == MsgType::Reject {
                    send_msg(&mut sig, m)?;
                    cmd = None;
                }
            }
        }
        let ssrc = 0x4959_414b ^ (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(1));
        let media = media.ok_or("no audio channel")?;
        if let (Some(c), Some(b)) = (cfg_aac, aac) {
            let enc = b.encoder(&c)?;
            let pacer = paced.then(|| Pacer { start: Instant::now(), sent: 0, rate: c.rate, lead: Duration::from_millis(120) });
            return Ok(Session { sig, media, src, re, enc: None, pack: None, conv: None, pend: VecDeque::new(), frames: vec![], pacer, packets: 0, delay: None, ended: false, params: None, aac: Some(AacTx { enc, pack: AacPacketizer::new(ssrc), cfg: c }) });
        }
        let params = cfg.ok_or("the device never agreed on settings")?;
        let enc = Encoder::new(params).map_err(|e| e.to_string())?;
        let pacer = paced.then(|| Pacer { start: Instant::now(), sent: 0, rate: params.sample_rate(), lead: Duration::from_millis(120) });
        Ok(Session { sig, media, src, re, enc: Some(enc), pack: Some(Packetizer::new(ssrc, &params)), conv: None, pend: VecDeque::new(), frames: vec![], pacer, packets: 0, delay: None, ended: false, params: Some(params), aac: None })
    }

    /// The SBC settings; None when the stream is an AAC one.
    pub fn params(&self) -> Option<&Params> {
        self.params.as_ref()
    }

    /// The AAC settings; None when the stream is an SBC one.
    pub fn aac_config(&self) -> Option<&AacConfig> {
        self.aac.as_ref().map(|a| &a.cfg)
    }

    pub fn state(&self) -> SrcState {
        self.src.state()
    }

    pub fn ended(&self) -> bool {
        self.ended
    }

    pub fn info(&self) -> SendInfo {
        SendInfo { codec: self.aac.as_ref().map(|a| crate::a2dp_aac::codec_text(&a.cfg)).or_else(|| self.params.as_ref().map(crate::a2dp::codec_text)).unwrap_or_default(), packets: self.packets, delay: self.delay }
    }

    /// Answers what the device said on the signalling channel (a delay report, a close) without waiting.
    pub fn poll_signalling(&mut self) -> Result<(), String> {
        while let Some(m) = next_msg(&mut self.sig, &mut self.re, Duration::from_millis(0))? {
            let (reply, acts) = self.src.on_msg(&m).map_err(|e| e.text())?;
            if let Some(r) = reply {
                send_msg(&mut self.sig, &r)?;
            }
            for a in acts {
                match a {
                    SrcAction::Delay(d) => self.delay = Some(d),
                    SrcAction::Ended => self.ended = true,
                    _ => {}
                }
            }
        }
        Ok(())
    }

    /// Sound in (interleaved, any rate): whole frames are encoded and sent, the rest waits for the next call.
    pub fn write(&mut self, pcm: &[i16], rate: u32, channels: usize) -> Result<(), String> {
        if self.ended {
            return Err("the device ended the stream".into());
        }
        self.poll_signalling()?;
        if self.ended {
            return Err("the device ended the stream".into());
        }
        if rate == 0 || channels == 0 || channels > 8 {
            return Err("this sound has no usable rate or channel count".into());
        }
        let (out_rate, out_ch) = match (&self.aac, &self.params) {
            (Some(a), _) => (a.cfg.rate, a.cfg.channels as usize),
            (None, Some(p)) => (p.sample_rate(), p.channels()),
            _ => return Err("no audio settings".into()),
        };
        if !self.conv.as_ref().is_some_and(|c| c.matches(rate, channels)) {
            self.conv = Some(Converter::new(rate, channels, out_rate, out_ch));
        }
        let out = self.conv.as_mut().map(|c| c.run(pcm)).unwrap_or_default();
        if self.aac.is_some() {
            return self.write_aac(&out, out_ch);
        }
        let (Some(enc), Some(params)) = (self.enc.as_mut(), self.params) else { return Err("no audio settings".into()) };
        self.pend.extend(out);
        let need = enc.frame_input_len();
        let per = frames_per_packet(MEDIA_MTU, params.frame_len());
        while self.pend.len() >= need {
            let chunk: Vec<i16> = self.pend.drain(..need).collect();
            let frame = self.enc.as_mut().ok_or("no audio settings")?.encode(&chunk).map_err(|e| e.to_string())?;
            self.frames.push(frame);
            if self.frames.len() >= per {
                self.flush()?;
            }
        }
        if !self.frames.is_empty() {
            self.flush()?;
        }
        Ok(())
    }

    /// AAC: the sound (already at the stream's rate) goes to the add-on's coder; the frames that are ready are sent, one per packet. The
    /// wait that keeps the stream in time is on the sound going in, since the coder works in its own process.
    fn write_aac(&mut self, pcm: &[i16], channels: usize) -> Result<(), String> {
        if let Some(p) = self.pacer.as_mut() {
            p.wait((pcm.len() / channels.max(1)) as u64);
        }
        let Some(a) = self.aac.as_mut() else { return Err("no audio settings".into()) };
        a.enc.feed(pcm)?;
        for f in a.enc.drain()? {
            let pkt = a.pack.packet(&f);
            self.media.send(&pkt).map_err(|e| format!("the sound could not be sent: {e}"))?;
            self.packets += 1;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        let n = self.frames.len();
        let (Some(pack), Some(params)) = (self.pack.as_mut(), self.params) else { return Err("no audio settings".into()) };
        let pkt = pack.packet(&self.frames);
        self.frames.clear();
        if let Some(p) = self.pacer.as_mut() {
            p.wait((n * params.frame_samples()) as u64);
        }
        self.media.send(&pkt).map_err(|e| format!("the sound could not be sent: {e}"))?;
        self.packets += 1;
        Ok(())
    }

    /// Ends the stream properly (Close), best effort.
    pub fn close(&mut self) {
        if let Some(c) = self.src.close() {
            let _ = send_msg(&mut self.sig, &c);
            let _ = next_msg(&mut self.sig, &mut self.re, Duration::from_millis(500)).map(|m| m.map(|m| self.src.on_msg(&m)));
        }
        self.ended = true;
    }

    /// Pauses the stream (Suspend), best effort.
    pub fn suspend(&mut self) -> Result<(), String> {
        let Some(c) = self.src.suspend() else { return Ok(()) };
        send_msg(&mut self.sig, &c)?;
        if let Some(m) = next_msg(&mut self.sig, &mut self.re, Duration::from_secs(2))? {
            self.src.on_msg(&m).map_err(|e| e.text())?;
        }
        Ok(())
    }
}

/// What Settings and the assistant see of the sending side.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SendView {
    pub active: bool,
    pub device: String,
    pub codec: String,
    pub packets: u64,
    pub delay_ms: Option<f32>,
    /// why nothing is sent (empty when it works or when nothing was tried)
    pub why: String,
}

impl SendView {
    pub fn line(&self) -> String {
        if self.active {
            let d = self.delay_ms.map(|d| format!(", the device adds about {d:.0} ms")).unwrap_or_default();
            format!("sending sound to {} ({}, {} packets sent{})", self.device, self.codec, self.packets, d)
        } else if self.why.is_empty() {
            "not sending sound to any headphones or speaker".into()
        } else {
            format!("not sending: {}", self.why)
        }
    }
}

/// What the assistant and Settings use for sending sound to headphones. The real one is in `a2dp_send_sys`; tests use a fake.
pub trait SendControl: Send + Sync {
    fn view(&self) -> SendView;
    /// Connects to a paired speaker or headphones and starts the stream. Returns when it is running or failed.
    fn start(&self, address: &str, name: &str) -> Result<(), String>;
    fn stop(&self) -> Result<(), String>;
    /// Sound for the running stream (blocks to keep time).
    fn write(&self, pcm: &[i16], rate: u32, channels: usize) -> Result<(), String>;
}

/// A [`SendControl`] over any way of connecting (real sockets or scripted ones): one stream at a time.
pub struct Sender<S: Channel + Send + 'static, M: Channel + Send + 'static> {
    connect: Box<dyn Fn(&str) -> Result<(S, Box<dyn FnOnce() -> io::Result<M> + Send>), String> + Send + Sync>,
    live: std::sync::Mutex<Live<S, M>>,
    paced: bool,
    /// The coders of the add-on, asked at each start (None: SBC only).
    aac: Option<Box<dyn Fn() -> Option<Arc<dyn AacBackend>> + Send + Sync>>,
}

struct Live<S: Channel, M: Channel> {
    session: Option<Session<S, M>>,
    device: String,
    why: String,
}

impl<S: Channel + Send + 'static, M: Channel + Send + 'static> Sender<S, M> {
    pub fn new(paced: bool, connect: impl Fn(&str) -> Result<(S, Box<dyn FnOnce() -> io::Result<M> + Send>), String> + Send + Sync + 'static) -> Self {
        Sender { connect: Box::new(connect), live: std::sync::Mutex::new(Live { session: None, device: String::new(), why: String::new() }), paced, aac: None }
    }

    /// AAC is used when `f` returns the add-on's coders at the time of a start and the headphones offer AAC.
    pub fn with_aac(mut self, f: impl Fn() -> Option<Arc<dyn AacBackend>> + Send + Sync + 'static) -> Self {
        self.aac = Some(Box::new(f));
        self
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Live<S, M>> {
        self.live.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl<S: Channel + Send + 'static, M: Channel + Send + 'static> SendControl for Sender<S, M> {
    fn view(&self) -> SendView {
        let g = self.lock();
        match g.session.as_ref().filter(|s| !s.ended()) {
            Some(s) => {
                let i = s.info();
                SendView { active: true, device: g.device.clone(), codec: i.codec, packets: i.packets, delay_ms: i.delay.map(|d| d as f32 / 10.0), why: String::new() }
            }
            None => SendView { why: g.why.clone(), ..Default::default() },
        }
    }

    fn start(&self, address: &str, name: &str) -> Result<(), String> {
        if self.lock().session.as_ref().is_some_and(|s| !s.ended()) {
            return Err("sound is already being sent to a device: stop that first".into());
        }
        let fail = |e: String| {
            self.lock().why = e.clone();
            Err(e)
        };
        let (sig, media) = match (self.connect)(address) {
            Ok(x) => x,
            Err(e) => return fail(e),
        };
        let backend = self.aac.as_ref().and_then(|f| f());
        let first = Session::open_with(sig, media, self.paced, Duration::from_secs(5), backend.clone());
        // headphones that offer AAC but refuse it: the same stream again with SBC only
        let opened = match (first, backend) {
            (Err(_), Some(_)) => match (self.connect)(address) {
                Ok((sig, media)) => Session::open_with(sig, media, self.paced, Duration::from_secs(5), None),
                Err(e) => Err(e),
            },
            (r, _) => r,
        };
        match opened {
            Ok(s) => {
                let mut g = self.lock();
                g.session = Some(s);
                g.device = if name.is_empty() { address.to_string() } else { name.to_string() };
                g.why.clear();
                Ok(())
            }
            Err(e) => fail(e),
        }
    }

    fn stop(&self) -> Result<(), String> {
        let mut g = self.lock();
        match g.session.take() {
            Some(mut s) => {
                s.close();
                Ok(())
            }
            None => Err("no sound is being sent to a device".into()),
        }
    }

    fn write(&self, pcm: &[i16], rate: u32, channels: usize) -> Result<(), String> {
        let mut g = self.lock();
        let Some(s) = g.session.as_mut() else { return Err("no sound is being sent to a device".into()) };
        let r = s.write(pcm, rate, channels);
        if let Err(e) = &r {
            g.why = e.clone();
            g.session = None;
        }
        r
    }
}

/// The best SBC settings this module asks for on mono sinks etc. are chosen in [`crate::avdtp_src::choose_sbc`]; this names the mode.
pub fn mode_name(m: ChannelMode) -> &'static str {
    match m {
        ChannelMode::Mono => "mono",
        ChannelMode::Dual => "dual channel",
        ChannelMode::Stereo => "stereo",
        ChannelMode::Joint => "joint stereo",
    }
}

#[cfg(test)]
#[path = "a2dp_send_tests.rs"]
mod tests;
