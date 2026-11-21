//! The connector's end of "My IYAKKI" in cloud mode (F-VOICE-61, F-VOICE-38, F-VOICE-45): the person's microphone leaves this computer only
//! after the person answered the consent card here, over the pinned TLS link to the cloud machine; the answer's sound comes back over a link
//! in the other direction to a certificate this computer made. The parts (frames, link, gate, loop) are `iyakki-audiolink`, the crate the
//! agent uses too, so this program stays small and does not depend on the agent.
//!
//! What this module does not do: reading the real microphone (an operating-system call, given as a [`Mic`]), and asking the instance for the
//! cloud machine's address and certificate (the authenticated session carries a [`CertOffer`]; the session call is the program's).

use std::io::{BufRead, Write};
use std::net::{SocketAddr, TcpListener, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use iyakki_audiolink::audiotls::{self, Identity};
use iyakki_audiolink::certoffer::CertOffer;
use iyakki_audiolink::device::{AnswerPlayer, DeviceClient, Link, Step, TlsLink};
use iyakki_audiolink::gate::{Gate, Verdict};
use iyakki_audiolink::Mic;

use crate::voicecapture;

/// The time between steps of the loop (a batch is 100 ms of sound).
pub const PAUSE: Duration = Duration::from_millis(20);

/// One voice session of this computer with one cloud machine.
pub struct DeviceVoice {
    gate: Gate,
    /// `None` until the cloud machine's certificate came over the session.
    link: Option<Box<dyn Link>>,
    stop: Arc<AtomicBool>,
    run: Option<JoinHandle<Step>>,
    answer_id: Identity,
}

impl DeviceVoice {
    /// `enabled` is the instance's switch (`voice.my_iyakki_cloud_stream`, off by default), `machine` the cloud machine's name for the card.
    pub fn new(enabled: bool, machine: &str) -> Result<DeviceVoice, String> {
        Ok(DeviceVoice { gate: Gate::new(enabled, machine), link: None, stop: Arc::new(AtomicBool::new(false)), run: None, answer_id: Identity::make()? })
    }

    /// The cloud machine's address and the certificate offer that came inside the authenticated session. A bad offer is refused.
    pub fn connect(&mut self, addr: SocketAddr, offer_json: &str) -> Result<String, String> {
        let offer = CertOffer::from_json(offer_json)?;
        self.link = Some(Box::new(TlsLink::new(addr, &offer.cert_der)?));
        Ok(offer.machine)
    }

    /// Another transport (tests).
    pub fn connect_with(&mut self, link: Box<dyn Link>) {
        self.link = Some(link);
    }

    /// Where the gate stands: `Card(text)` waits for the person's tap or key on this computer.
    pub fn verdict(&self) -> Verdict {
        self.gate.verdict()
    }

    /// The person answered the card here (a key at the prompt or a button of the window). Nothing else calls this: no function takes words.
    pub fn press(&mut self, yes: bool) {
        self.gate.press(yes);
    }

    /// The certificate this computer offers back, for the answer's sound: the cloud machine pins it.
    pub fn answer_offer(&self, machine: &str) -> CertOffer {
        CertOffer { machine: machine.chars().filter(|c| !c.is_control()).take(60).collect(), cert_der: self.answer_id.cert_der.clone() }
    }

    /// Listen once for the answer's sound on `listener`, with the certificate of [`DeviceVoice::answer_offer`]. Returns the sound.
    pub fn hear_answer(&self, listener: &TcpListener) -> Result<Vec<i16>, String> {
        let mut p = AnswerPlayer::new();
        p.receive(listener, audiotls::server_config(&self.answer_id)?)?;
        Ok(p.take())
    }

    /// Start sending the microphone. Refused (with the words why) unless the person allowed it in this session and a link is set.
    pub fn start(&mut self, mic: Box<dyn Mic>) -> Result<(), String> {
        if self.run.is_some() {
            return Err("the microphone is already being sent".into());
        }
        if let Verdict::Card(_) | Verdict::Off | Verdict::Denied | Verdict::Muted = self.gate.verdict() {
            return Err(self.gate.why_not().unwrap_or_else(|| "not allowed".into()));
        }
        let link = self.link.take().ok_or("the cloud machine's certificate has not arrived yet")?;
        let mut client = DeviceClient::with_link(self.gate.clone(), mic, link);
        self.stop.store(false, Ordering::SeqCst);
        let stop = self.stop.clone();
        self.run = Some(std::thread::spawn(move || client.run(&stop, PAUSE)));
        Ok(())
    }

    /// Mute: the loop stops and closes the microphone at the source. Sending again needs a new session and a new answer to the card.
    pub fn mute(&mut self) -> Option<Step> {
        self.stop.store(true, Ordering::SeqCst);
        self.gate.set_muted(true);
        self.gate.new_session();
        self.run.take().and_then(|h| h.join().ok())
    }

    pub fn running(&self) -> bool {
        self.run.as_ref().is_some_and(|h| !h.is_finished())
    }
}

impl DeviceVoice {
    /// [`DeviceVoice::connect`] with the address as the server wrote it (`host:port`, looked up here).
    pub fn connect_host(&mut self, addr: &str, offer_json: &str) -> Result<String, String> {
        let sa = addr.to_socket_addrs().map_err(|_| "the audio address does not resolve".to_string())?.next().ok_or("the audio address does not resolve")?;
        self.connect(sa, offer_json)
    }
}

/// The real microphone on a Linux computer (`arecord`). The same reader for macOS and Windows is [`voicecapture::CommandMic`].
pub struct ArecordMic(voicecapture::CommandMic);

impl ArecordMic {
    pub fn new() -> ArecordMic {
        ArecordMic(voicecapture::CommandMic::new(voicecapture::CaptureCmd::linux()))
    }
}

impl Default for ArecordMic {
    fn default() -> Self {
        ArecordMic::new()
    }
}

impl Mic for ArecordMic {
    fn open(&mut self) -> Result<(), String> {
        self.0.open()
    }
    fn is_open(&self) -> bool {
        self.0.is_open()
    }
    fn drain(&mut self) -> Vec<i16> {
        self.0.drain()
    }
    fn alive(&mut self) -> bool {
        self.0.alive()
    }
    fn close(&mut self) {
        self.0.close()
    }
}

/// The way the answer's sound comes back to this computer: a listener with a certificate of its own, the offer that tells the cloud machine
/// where and whom to trust (sent inside the authenticated session, only after the person said yes), and a [`voicecapture::Sink`] that plays it.
pub struct ReturnEnd {
    listener: TcpListener,
    id: Identity,
    advertise: String,
}

impl ReturnEnd {
    /// Listen on `bind` (`host:port`); `advertise` is the address the cloud machine can reach it at (behind a router it differs from `bind`).
    /// An advertised port of 0 means the port that was bound.
    pub fn new(bind: &str, advertise: &str) -> Result<ReturnEnd, String> {
        let mut adv: SocketAddr = advertise.parse().map_err(|_| "the address to advertise must be an IP address and port".to_string())?;
        let listener = TcpListener::bind(bind).map_err(|e| format!("could not listen for the answer on {bind}: {e}"))?;
        if adv.port() == 0 {
            adv.set_port(listener.local_addr().map_err(|e| e.to_string())?.port());
        }
        Ok(ReturnEnd { listener, id: Identity::make()?, advertise: adv.to_string() })
    }

    /// What the instance is told: the advertised address and the certificate offer (`machine` is this computer's name for the card).
    pub fn offer(&self, machine: &str) -> (String, String) {
        (self.advertise.clone(), CertOffer { machine: machine.chars().filter(|c| !c.is_control()).take(60).collect(), cert_der: self.id.cert_der.clone() }.to_json())
    }

    /// Where it really listens (a port of 0 is filled in).
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.listener.local_addr().ok()
    }

    /// Take the answer's sound, piece by piece, into `sink` until `stop` is set. A link that cannot be read (wrong certificate, cut) ends that
    /// piece only; the next one is waited for. Returns the number of pieces played. `stop` is noticed after the next piece or after
    /// [`ReturnEnd::wake`].
    pub fn serve(&self, sink: &mut dyn voicecapture::Sink, stop: &AtomicBool) -> usize {
        let Ok(cfg) = audiotls::server_config(&self.id) else { return 0 };
        let mut played = 0;
        while !stop.load(Ordering::SeqCst) {
            let mut p = AnswerPlayer::new();
            let got = p.receive(&self.listener, cfg.clone());
            if stop.load(Ordering::SeqCst) {
                break;
            }
            if got.is_ok() {
                let pcm = p.take();
                if sink.play(&pcm).is_ok() && !pcm.is_empty() {
                    played += 1;
                }
            }
        }
        played
    }

    /// Wake a `serve` that is waiting for a connection so that it sees `stop` (a plain connection that is dropped at once).
    pub fn wake(&self) {
        if let Some(a) = self.local_addr() {
            let _ = std::net::TcpStream::connect_timeout(&a, Duration::from_secs(2));
        }
    }
}

/// What a session needs to take the answer back: the listener, the program that plays it, and the call that tells the instance (the signed-in
/// session's `voice_return`).
pub struct Return {
    pub end: Arc<ReturnEnd>,
    pub sink: Box<dyn voicecapture::Sink>,
    pub tell: Box<dyn Fn(&str, &str) -> Result<(), String> + Send>,
}

/// `voice ADDRESS USER`: ask the signed-in instance where to send the microphone and which certificate to trust (the offer), put the card to
/// the person (their own line from `input` or a key at the prompt: the only way to say yes), then send the microphone until `stop` is set or
/// the link gives up. The password is the first line of `input`, the answer to the card the second. Nothing leaves before a "y".
pub fn session(offer: (String, String), input: &mut dyn BufRead, out: &mut dyn Write, mic: Box<dyn Mic>, stop: &AtomicBool) -> Result<(), String> {
    session_with_return(offer, input, out, mic, stop, None)
}

/// [`session`] that also takes the answer's sound back: after the "y" (never before) the device's own address and certificate are given to the
/// instance and the answer is played as it arrives. A refusal of that call is told in words and the microphone still goes; without it the
/// assistant's answer stays words.
pub fn session_with_return(offer: (String, String), input: &mut dyn BufRead, out: &mut dyn Write, mic: Box<dyn Mic>, stop: &AtomicBool, ret: Option<Return>) -> Result<(), String> {
    let (addr, offer_json) = offer;
    let machine = CertOffer::from_json(&offer_json)?.machine;
    let mut dv = DeviceVoice::new(true, &machine)?;
    dv.connect_host(&addr, &offer_json)?;
    let Verdict::Card(text) = dv.verdict() else { return Err("the consent card was not asked".into()) };
    writeln!(out, "{text}").and_then(|_| write!(out, "Send your microphone to {machine}? [y/N] ")).and_then(|_| out.flush()).map_err(|e| e.to_string())?;
    let mut a = String::new();
    input.read_line(&mut a).map_err(|e| e.to_string())?;
    let yes = matches!(a.trim().to_ascii_lowercase().as_str(), "y" | "yes");
    dv.press(yes);
    if !yes {
        return Err("not sent: you did not allow it; the microphone was never opened".into());
    }
    dv.start(mic)?;
    let mut answer: Option<(Arc<ReturnEnd>, Arc<AtomicBool>, JoinHandle<usize>)> = None;
    if let Some(Return { end, mut sink, tell }) = ret {
        let (a, o) = end.offer(&machine);
        match tell(&a, &o) {
            Ok(()) => {
                let done = Arc::new(AtomicBool::new(false));
                let (e2, d2) = (end.clone(), done.clone());
                answer = Some((end, done, std::thread::spawn(move || e2.serve(&mut *sink, &d2))));
                writeln!(out, "The answer's sound will play on this computer.").map_err(|e| e.to_string())?;
            }
            Err(e) => writeln!(out, "The answer will stay words: {e}").map_err(|e| e.to_string())?,
        }
    }
    writeln!(out, "Sending your microphone to {machine} (the red mark is this program running). Press Ctrl+C to stop.").map_err(|e| e.to_string())?;
    while dv.running() && !stop.load(Ordering::SeqCst) {
        std::thread::sleep(PAUSE);
    }
    let end = dv.mute();
    if let Some((e, done, h)) = answer {
        done.store(true, Ordering::SeqCst);
        e.wake();
        let _ = h.join();
    }
    writeln!(out, "Stopped; the microphone is closed.{}", end.map(|e| format!(" Last state: {e:?}.")).unwrap_or_default()).map_err(|e| e.to_string())
}
