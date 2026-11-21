//! The sound of the machine over a real loopback socket against the reference server (F-SPLIT-03, D-SPLIT-03-APPS): the UI's mixer, output
//! selector, cue player and sound route are the core's, asked over the socket; a sound the UI starts plays on the core until the UI lets go.
//! Test level: real transport, the fake core of `iyakki-core-fake`, no timing.

use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iyakki_core_api::audioroute::PcmHolder;
use iyakki_core_api::contract::RateLimit;
use iyakki_core_api::soundctl::{CuePlayer, SoundControl, SoundState};
use iyakki_core_api::soundtypes::{Cue, Kind, Outcome, Output, SoundSettings};
use iyakki_core_api::{AppStatus, AppStatuses, AudioRoute, Core, Mixer, MixerStats, SoundKind, StatusKind, WaitingCall};
use iyakki_core_fake::FakeCore;
use iyakki_ui_client::client::Connection;
use iyakki_ui_client::server::{serve, settings_hello, Served};
use iyakki_ui_client::{CallContext, Scope, Transport};
use iyakki_ui_rpc::{remote_core_sound, serve_core_sound, serve_uploads, Host, RemoteCoreSound, Rpc};

struct Machine {
    addr: String,
    stop: Arc<AtomicBool>,
    host: Arc<Host>,
    thread: Option<std::thread::JoinHandle<()>>,
}

fn machine(host: Host) -> Machine {
    let host = Arc::new(host);
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let stop = Arc::new(AtomicBool::new(false));
    let served = Served {
        hello: settings_hello("rpc", RateLimit { calls: 100_000, per_ms: 1000 }),
        settings: None,
        vault: None,
        flows: None,
        assistant: None,
        cameras: None,
        rpc: Some(host.clone()),
        appevents: None,
        ctx: CallContext::remote("ui", Transport::LocalSocket, &[Scope::SETTINGS_READ]),
    };
    let s2 = stop.clone();
    let thread = Some(std::thread::spawn(move || serve(l, Arc::new(served), s2).unwrap()));
    Machine { addr, stop, host, thread }
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

struct Quiet;
impl iyakki_core_api::soundtypes::CueOut for Quiet {
    fn ready(&self) -> Result<(), String> {
        Ok(())
    }
    fn play(&self, _: &[u8]) -> Result<(), String> {
        Ok(())
    }
}

fn setup() -> (Machine, Arc<FakeCore>, Arc<RemoteCoreSound>, Arc<Mutex<Vec<String>>>) {
    let core = Arc::new(FakeCore::new());
    let said: Arc<Mutex<Vec<String>>> = Default::default();
    let s2 = said.clone();
    let audit: iyakki_shared::gitkeys::AuditFn = Arc::new(move |who, what| s2.lock().unwrap().push(format!("{who}: {what}")));
    let as_core: Arc<dyn Core> = core.clone();
    let m = machine(Host::new().with_root(serve_uploads()).with_root(serve_core_sound(as_core, audit)));
    let rpc = Rpc::new(Connection::tcp(m.addr.as_str(), Duration::from_secs(10)).unwrap());
    let sound = remote_core_sound(&rpc, 0, Arc::new(Quiet));
    (m, core, sound, said)
}

fn speakers() -> Output {
    Output { card: 1, device: 0, card_id: "PCH".into(), card_name: "HDA Intel PCH".into(), pcm_name: "ALC3246 Analog".into(), kind: Kind::Hdmi, connected: Some(true), monitor: Some("LG TV".into()), speakers: Some(true), hdmi_no: 2, digital_off: false, builtin_speaker: true, hw_volume: Some("Master".into()), dummy_volume: false }
}

#[test]
fn the_mixer_of_the_core_answers_and_a_sound_plays_there_until_the_ui_lets_go() {
    let (m, core, sound, said) = setup();
    assert_eq!(sound.stats(), None);
    core.mixer_fake().set_stats(Some(MixerStats { sink_open: true, detail: "2 sources, no underruns".into(), last_error: Some("one underrun".into()) }));
    core.mixer_fake().set_sources(vec![("Radio X".into(), 70), ("Media".into(), 40)]);
    assert_eq!(sound.stats(), Some(MixerStats { sink_open: true, detail: "2 sources, no underruns".into(), last_error: Some("one underrun".into()) }));
    assert_eq!(sound.active_sources(), [("Radio X".to_string(), 70), ("Media".to_string(), 40)]);
    // 150 000 bytes of sound do not fit a line: the core gets them whole, in chunks
    let wav = vec![7u8; 150_000];
    let playing = sound.play_wav(SoundKind::Alarm, "Timer", &wav).unwrap();
    assert_eq!(core.mixer_fake().played(), [(SoundKind::Alarm, "Timer".to_string(), 150_000)]);
    assert_eq!(said.lock().unwrap().as_slice(), ["iyakki-ui: sound.play alarm Timer (150000 bytes)"], "the core's audit log has the sound the UI started");
    // the object that keeps the sound alive is one the core holds; dropping it tells the core to let go
    let held = m.host.held_total();
    drop(playing);
    assert_eq!(m.host.held_total(), held - 1, "the core let go of the sound");
}

#[test]
fn the_output_selectors_picture_the_outputs_and_the_prefs_are_the_cores() {
    let (_m, core, sound, _) = setup();
    assert_eq!(sound.state(), SoundState::default());
    let state = SoundState { active: true, chosen: Some(speakers()), reason: "the TV says it has speakers".into(), by_person: true, sound_server: false, mix_channels: Some(2), starting: Some("the card is starting".into()), hw_ignored: Some(3), busy: None, softvol_card: Some(1) };
    core.sound_fake().set_state(state.clone());
    core.sound_fake().set_outputs(vec![speakers()]);
    core.sound_fake().set_card_starting(Some(4));
    core.sound_fake().set_volume_note(Some("no control moves the sound"));
    core.sound_fake().push_notice("the HDMI cable was unplugged");
    core.sound_fake().set_prefs(SoundSettings { volume_pct: 35, muted: true, ..SoundSettings::default() });
    assert_eq!(sound.state(), state);
    let route = sound.clone();
    assert_eq!(sound.outputs_via(route.as_ref()), vec![speakers()]);
    assert_eq!(sound.card_starting_via(route.as_ref()), Some(4));
    assert_eq!(sound.volume_note().as_deref(), Some("no control moves the sound"));
    assert_eq!(sound.take_notices(), ["the HDMI cable was unplugged"]);
    assert!(sound.take_notices().is_empty(), "a notice comes once");
    let prefs = sound.prefs();
    assert_eq!((prefs.volume_pct, prefs.muted), (35, true));
    assert_eq!(sound.command_with_device("aplay -q", "hw:1,0"), Ok("aplay -q -D hw:1,0".to_string()));
    assert!(!sound.server_running());
    // a folder the UI names is not read for it: these two seams answer nothing
    assert!(sound.outputs_at(std::path::Path::new("/etc")).is_empty());
    assert_eq!(sound.card_starting(std::path::Path::new("/etc"), std::path::Path::new("/etc")), None);
}

#[test]
fn a_cue_is_played_by_the_core_and_the_handle_says_how_it_went() {
    let (_m, core, sound, _) = setup();
    let h = sound.play(Cue::Login, sound.out());
    assert_eq!(h.wait(Duration::from_secs(30)), Some(Outcome::Played));
    core.cues_fake().set_unavailable(Some("no card"));
    let h = sound.play(Cue::Notify, sound.out());
    assert_eq!(h.wait(Duration::from_secs(30)), Some(Outcome::NoOutput("no card".into())));
    assert_eq!(core.cues_fake().played(), [Cue::Login, Cue::Notify], "the core's own player ran both, in order");
}

#[test]
fn the_kernel_facts_of_the_sound_route_are_read_by_the_core_and_never_outside_asound() {
    let (_m, core, sound, _) = setup();
    let route = core.audio_route_fake();
    route.put("cards", " 0 [PCH ]: HDA-Intel - HDA Intel PCH\n 1 [HDMI]: HDA-Intel - HDA Intel HDMI\n");
    route.put("card0/codec#0", "Codec: Realtek ALC3246");
    route.set_holders(0, 0, vec![PcmHolder { pid: 42, name: "aplay".into() }]);
    route.set_mixer(0, "numid=1,iface=MIXER,name='Master Playback Volume'");
    route.set_process(42, 1, "aplay");
    route.set_product(Some("MacBookPro14,1"));
    assert_eq!(sound.card_count(), 2);
    assert!(sound.cards_text().contains("HDA Intel HDMI"));
    assert_eq!(sound.asound_text("card0/codec#0").as_deref(), Some("Codec: Realtek ALC3246"));
    assert!(sound.asound_has("cards") && !sound.asound_has("nothing"));
    assert_eq!(sound.product_name().as_deref(), Some("MacBookPro14,1"));
    assert_eq!(sound.pcm_holders(0, 0), [PcmHolder { pid: 42, name: "aplay".into() }]);
    assert_eq!((sound.process_parent(42), sound.process_name(42).as_deref(), sound.process_parent(7)), (Some(1), Some("aplay"), None));
    assert!(sound.mixer_contents(0).unwrap().contains("Master Playback Volume"));
    assert!(!sound.is_machine(), "the fake's route is a made-up tree, and the UI is told so");
    // a path that leaves /proc/asound is refused by the core before the route is asked
    let before = route.calls().len();
    assert_eq!((sound.asound_text("../cmdline"), sound.asound_text("/etc/passwd"), sound.asound_dir("card0/../../")), (None, None, Vec::<String>::new()));
    assert_eq!(route.calls().len(), before, "the route was not asked");
}

#[test]
fn a_lost_core_is_a_core_without_sound_and_nothing_panics() {
    let rpc = Rpc::new(Connection::new(Box::new(std::io::Cursor::new(Vec::new())), Box::new(std::io::sink())));
    let sound = remote_core_sound(&rpc, 9, Arc::new(Quiet));
    assert_eq!((sound.stats(), sound.active_sources().len(), sound.state(), sound.card_count()), (None, 0, SoundState::default(), 0));
    assert!(sound.play_wav(SoundKind::Media, "x", &[1]).is_err());
    assert!(matches!(sound.play(Cue::Login, sound.out()).wait(Duration::from_secs(30)), Some(Outcome::NoOutput(_))));
}

#[test]
fn what_the_apps_say_about_themselves_and_the_waiting_approvals_come_from_the_core() {
    let (_m, core, sound, _) = setup();
    assert!(sound.all("ann").is_empty() && sound.tokens_24h("ann").is_none() && sound.waiting_calls("ann").is_empty());
    let rows = vec![
        ("radio".to_string(), AppStatus { kind: StatusKind::Running, line: "Playing Radio X".into(), count: 0, needs: None }),
        ("store".to_string(), AppStatus { kind: StatusKind::Setup, line: "3 updates".into(), count: 3, needs: Some(("Updates".into(), "3 are ready".into())) }),
        ("inbox".to_string(), AppStatus { kind: StatusKind::Attention, line: "an approval".into(), count: 1, needs: None }),
    ];
    core.statuses_fake().set(rows.clone());
    core.statuses_fake().set_tokens(Some((1200, 300, 9)));
    core.statuses_fake().set_waiting(vec![WaitingCall { tool: "radio_play".into(), summary: "Play Radio X".into() }]);
    assert_eq!(sound.all("ann"), rows);
    assert_eq!(sound.tokens_24h("ann"), Some((1200, 300, 9)));
    assert_eq!(sound.waiting_calls("ann"), [WaitingCall { tool: "radio_play".into(), summary: "Play Radio X".into() }]);
}
