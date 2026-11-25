//! The end-of-session hooks of the services (D-SESSION-END, `docs/development/SESSION-END-AUDIT.md`): one place where the long-lived
//! per-person sound and capture resources of the agent say what stops when a person signs out, when the sound changes owner or when the
//! screen locks. The registry itself is `iyakki_shared::sessionend`; `Shell::sign_out` calls it once. A new player, reader or capture
//! gets one `register` line here (the ratchet test `session_end_registered` fails until it has one).

use crate::media::MediaPlayer;
use crate::radio::RadioPlayer;
use crate::services::Services;
use crate::storykit::ReaderGroup;
use iyakki_shared::sessionend::{Ctx, Kind, LiveSet, SessionEnd, StopSet, Why};
use std::sync::Arc;

/// Every kind this module registers, in order: the conformance test walks this list.
pub const REGISTERED: &[(&str, Kind)] = &[
    ("radio", Kind::Sound),
    ("media-player", Kind::Sound),
    ("story-readers", Kind::Sound),
    ("camera-grant", Kind::Capture),
    ("screen-grant", Kind::Capture),
    // sound and capture owners of lane SO2 (the voice stack, the cloud audio link, browser sessions)
    ("voice-players", Kind::Sound),
    ("voice-assistant-speaker", Kind::Sound),
    ("cloud-speaker", Kind::Sound),
    ("voice-recorders", Kind::Capture),
    ("cloud-audio-listener", Kind::Capture),
    ("browser-capture", Kind::Capture),
    // what the person started that is neither sound nor capture: ends on sign-out only
    ("terminals", Kind::Process),
    ("port-forwards", Kind::Process),
    ("linux-windows", Kind::Process),
    ("ide-task-runs", Kind::Process),
    ("lab-jobs", Kind::Process),
    ("browser-audio", Kind::Process),
];

/// Where the hooks of the lower crates find what runs: the product points them at the process-wide sets, a test at sets of its own (so a
/// sign-out in one test never cuts a recorder, player or window of another test running at the same time).
#[derive(Clone, Copy)]
pub struct Stops {
    pub recorders: &'static StopSet,
    pub players: &'static StopSet,
    pub assistant_speaker: &'static StopSet,
    pub cloud_speaker: &'static StopSet,
    pub windows: &'static StopSet,
}

fn leak_set() -> &'static StopSet {
    Box::leak(Box::new(StopSet::new()))
}

impl Stops {
    /// The sets of this process; in a test binary fresh private ones.
    pub fn process() -> Stops {
        if crate::voice::is_test_binary() {
            return Stops::private();
        }
        Stops { recorders: &crate::voice::RECORDER_STOPS, players: &crate::voice::PLAYER_STOPS, assistant_speaker: &crate::voiceassist_ports::SPEAKER_STOPS, cloud_speaker: &crate::cloudvoice::SPEAKER_STOPS, windows: &iyakki_shared::linuxwin::WINDOW_STOPS }
    }

    /// Sets nobody else uses (tests).
    pub fn private() -> Stops {
        Stops { recorders: leak_set(), players: leak_set(), assistant_speaker: leak_set(), cloud_speaker: leak_set(), windows: leak_set() }
    }
}

/// The other owners the process hooks reach.
#[derive(Clone)]
pub struct Owners {
    pub stops: Stops,
    /// The K8s port-forwards of this machine.
    pub forwards: Arc<dyn iyakki_core_api::portforward::PortForwards>,
    /// The embedded terminals of the cores of this process.
    pub terminals: &'static LiveSet<crate::terminalfacet::Terminals>,
}

impl Owners {
    /// The owners of this process; in a test binary private ones (no forwards, no terminals of other tests).
    pub fn process() -> Owners {
        if crate::voice::is_test_binary() {
            return Owners { stops: Stops::private(), forwards: Arc::new(iyakki_core_api::portforward::NoPortForwards), terminals: Box::leak(Box::new(LiveSet::new())) };
        }
        Owners { stops: Stops::process(), forwards: iyakki_core_local::LocalPortForwards::shared(), terminals: crate::terminalfacet::Terminals::live() }
    }
}

/// The radio is one machine-wide player with no recorded owner: it is the sound owner's, so it stops with them.
pub fn end_radio(radio: &RadioPlayer, c: &Ctx) -> bool {
    if !c.sound_owner || matches!(radio.state(), iyakki_tile_api::radio::RadioState::Idle) {
        return false;
    }
    radio.stop();
    true
}

/// Register the hooks of `svc` into `reg` (the product passes the process-wide registry; a test passes its own).
pub fn register(svc: &Arc<Services>, reg: &SessionEnd) {
    register_with(svc, reg, crate::media::shared(), crate::storykit::shared_readers());
}

/// [`register`] with the media player and the reader group given (a test hands in its own).
pub fn register_with(svc: &Arc<Services>, reg: &SessionEnd, player: Arc<MediaPlayer>, readers: Arc<ReaderGroup>) {
    register_all(svc, reg, player, readers, &Owners::process());
}

/// [`register_with`] with the other owners given (the conformance test hands in its own).
pub fn register_all(svc: &Arc<Services>, reg: &SessionEnd, player: Arc<MediaPlayer>, readers: Arc<ReaderGroup>, owners: &Owners) {
    let s = Arc::downgrade(svc);
    let w = s.clone();
    reg.register("radio", Kind::Sound, move |c: &Ctx| w.upgrade().is_some_and(|svc| end_radio(&svc.radio, c)));
    reg.register("media-player", Kind::Sound, move |c: &Ctx| player.end_session(&c.user));
    // readers are made by the apps of whoever is signed in, one at a time on this machine's sound
    reg.register("story-readers", Kind::Sound, move |c: &Ctx| c.sound_owner && readers.stop_all_active() > 0);
    let w = s.clone();
    reg.register("camera-grant", Kind::Capture, move |c: &Ctx| w.upgrade().is_some_and(|svc| svc.camera.end_capture(&c.user)));
    let w = s.clone();
    reg.register("screen-grant", Kind::Capture, move |c: &Ctx| w.upgrade().is_some_and(|svc| svc.screen.end_capture(&c.user)));
    register_sound_and_capture(svc, reg, &owners.stops);
    register_processes(svc, reg, owners);
}

/// A sound hook belongs to the person who owns the machine's sound (one mixer, no per-person sources).
fn sound_hook(set: &'static StopSet) -> impl Fn(&Ctx) -> bool + Send + Sync + 'static {
    move |c: &Ctx| c.sound_owner && set.stop_all() > 0
}

fn register_sound_and_capture(svc: &Arc<Services>, reg: &SessionEnd, stops: &Stops) {
    reg.register("voice-players", Kind::Sound, sound_hook(stops.players));
    reg.register("voice-assistant-speaker", Kind::Sound, sound_hook(stops.assistant_speaker));
    reg.register("cloud-speaker", Kind::Sound, sound_hook(stops.cloud_speaker));
    // the microphone processes of the voice tile, dictation, the voice desk and the apps' listening: one hook at the one place they start
    let rec = stops.recorders;
    reg.register("voice-recorders", Kind::Capture, move |_: &Ctx| rec.stop_all() > 0);
    let w = Arc::downgrade(svc);
    reg.register("cloud-audio-listener", Kind::Capture, move |_: &Ctx| w.upgrade().is_some_and(|svc| svc.end_cloud_voice()));
    // a browser session's microphone and camera (the page is told to stop its devices); a suspend reaches every page
    reg.register("browser-capture", Kind::Capture, |c: &Ctx| iyakki_shared::browsermedia::stop_capture_of(if c.why == Why::Suspend { "" } else { &c.user }) > 0);
}

/// The jobs a person started in the IDE: a workspace of their own. Install and download jobs of the system have a workspace of the system's
/// own and go on (they are not the person's sound or capture; `docs/development/SESSION-END-AUDIT.md`).
pub fn is_ide_job(j: &iyakki_shared::jobs::Job) -> bool {
    !j.workspace.is_empty() && j.workspace != iyakki_shared::quickinstall::WORKSPACE
}

/// How long a job is given to end after TERM at a sign-out before it is killed.
const JOB_GRACE: std::time::Duration = std::time::Duration::from_millis(500);

fn register_processes(svc: &Arc<Services>, reg: &SessionEnd, o: &Owners) {
    let live = o.terminals;
    reg.register_counted("terminals", Kind::Process, move |c: &Ctx| {
        let mut n = 0;
        live.each(|t| {
            n += t.end_session(&c.user);
            false
        });
        n
    });
    let f = o.forwards.clone();
    reg.register_counted("port-forwards", Kind::Process, move |_: &Ctx| f.stop_all());
    let win = o.stops.windows;
    reg.register_counted("linux-windows", Kind::Process, move |_: &Ctx| win.stop_all());
    let w = Arc::downgrade(svc);
    reg.register_counted("ide-task-runs", Kind::Process, move |c: &Ctx| w.upgrade().map_or(0, |svc| svc.jobs.end_session(&c.user, &is_ide_job, JOB_GRACE)));
    let w = Arc::downgrade(svc);
    reg.register_counted("lab-jobs", Kind::Process, move |c: &Ctx| w.upgrade().map_or(0, |svc| svc.jobs.end_session(&c.user, &|j: &iyakki_shared::jobs::Job| j.workspace.is_empty(), JOB_GRACE)));
    reg.register("browser-audio", Kind::Process, |c: &Ctx| iyakki_shared::browsermedia::stop_audio_of(&c.user));
}

/// The machine is about to sleep: capture and sound of the process-wide registry stop first (D-SESSION-END). Called by `boot::sleep`, so every
/// way into a sleep (the power menu, the lid, the idle timer, a button of a browser session) stops the microphone and the camera, whoever asked.
/// The shell's own hooks (the wake listener) are run by the shell before it asks the machine to sleep. Returns the names that stopped something.
pub fn end_for_suspend(audit: &dyn Fn(&str, &str)) -> Vec<String> {
    let ctx = Ctx { user: String::new(), why: Why::Suspend, sound_owner: true };
    iyakki_shared::sessionend::global().end(&ctx, audit).stopped
}
