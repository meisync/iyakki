//! Silence the speakers before the machine restarts or powers off.
//!
//! Why: the T2 chip's audio (apple-bce `aaudio`) plays a DMA ring of 16640 frames in a loop. If the OS goes down while a
//! stream is still running (the radio's `aplay`, the web engine's `alsasink`, a sound cue) nothing stops the stream, and the
//! chip keeps looping the last samples through the whole restart or power-off. [`quiet_for_shutdown`] is called by the one
//! place that powers off or restarts (`iyakki_core::boot`, which the shell's power menu, the console commands, the
//! dev-mode `power` scope and the update's `--reboot` all reach) right before the disks are flushed. In order:
//! 1. the software volume (the `IYAKKI` control, see [`crate::softvol`]) goes to 0 on every card that has it;
//! 2. every process that holds a playback substream open (`/proc/asound/card*/pcm*p/sub*/status` is not `closed`: the
//!    radio and media player's `aplay`/`ffplay`, the voice engine, the web engine) is asked to end (SIGTERM), and the
//!    substreams get a short time to close;
//! 3. what is still open has its owner killed (SIGKILL);
//! 4. if a substream is still open, a short period of silence is written through the `iyakki` device.
//!
//! The whole thing is bounded by [`TOTAL_BUDGET`] plus the silence write ([`SILENCE_MAX`]), never panics, and does nothing
//! without a sound card. The kernel driver has its own safety net (the apple-bce patch `0004-aaudio-silence-on-shutdown`).
//! Machine access goes through [`Env`] and the `/proc/asound` path is given, so tests use a fake tree and a fake machine.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the kernel lists the sound cards.
pub const ASOUND: &str = "/proc/asound";
/// How long the substreams get to close after SIGTERM, and after SIGKILL (together at most the budget).
pub const TERM_WAIT: Duration = Duration::from_millis(1000);
pub const KILL_WAIT: Duration = Duration::from_millis(500);
/// The most [`quiet_for_shutdown`] waits for streams to close.
pub const TOTAL_BUDGET: Duration = Duration::from_millis(1500);
/// The most the final silence may take.
pub const SILENCE_MAX: Duration = Duration::from_millis(300);
const POLL: Duration = Duration::from_millis(50);

/// What the shutdown needs from the machine.
pub trait Env: Send + Sync {
    /// Run `amixer` with these arguments.
    fn amixer(&self, args: &[String]) -> Result<(), String>;
    /// Send a signal (15 or 9) to a process.
    fn signal(&self, pid: u32, sig: i32);
    /// Wait.
    fn sleep(&self, d: Duration);
    /// Play a short period of silence through the `iyakki` device, taking at most `max`.
    fn silence(&self, max: Duration);
    /// This process (never signalled).
    fn own_pid(&self) -> u32;
}

/// The real machine.
pub struct System;

impl Env for System {
    fn amixer(&self, args: &[String]) -> Result<(), String> {
        let st = std::process::Command::new("amixer")
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("amixer: {e}"))?;
        if st.success() { Ok(()) } else { Err(format!("amixer ended with {st}")) }
    }

    fn signal(&self, pid: u32, sig: i32) {
        // SAFETY: kill takes no pointers; a pid that is gone just fails.
        unsafe {
            libc::kill(pid as libc::pid_t, sig);
        }
    }

    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }

    fn silence(&self, max: Duration) {
        use std::io::Write;
        let Ok(mut child) = std::process::Command::new("aplay")
            .args(["-q", "-D", crate::softvol::PCM, "-t", "raw", "-f", "S16_LE", "-r", "48000", "-c", "2", "-"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        else {
            return;
        };
        if let Some(mut si) = child.stdin.take() {
            // 100 ms of silence
            let _ = si.write_all(&vec![0u8; 48_000 / 10 * 2 * 2]);
        }
        let mut waited = Duration::ZERO;
        while waited < max {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
            waited += Duration::from_millis(10);
        }
        let _ = child.kill();
        let _ = child.wait();
    }

    fn own_pid(&self) -> u32 {
        std::process::id()
    }
}

/// A playback substream that is not closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenStream {
    /// `card0/pcm0p/sub0`
    pub name: String,
    /// The process that has it open, when the status says.
    pub owner: Option<u32>,
}

fn numbered(name: &str, prefix: &str) -> bool {
    name.strip_prefix(prefix).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn dirs(p: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    v.sort();
    v
}

fn file_name(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

/// The numbers of the sound cards under `root` (`card0`, `card1`, ...).
pub fn cards(root: &Path) -> Vec<u32> {
    dirs(root).iter().filter_map(|p| file_name(p).strip_prefix("card").and_then(|n| n.parse().ok())).collect()
}

/// Playback substreams (`pcm*p/sub*`) whose status is not `closed`. Capture (`pcm*c`) is never looked at.
pub fn open_playback(root: &Path) -> Vec<OpenStream> {
    let mut out = Vec::new();
    for card in dirs(root).into_iter().filter(|p| numbered(&file_name(p), "card")) {
        for pcm in dirs(&card).into_iter().filter(|p| file_name(p).strip_prefix("pcm").and_then(|r| r.strip_suffix('p')).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))) {
            for sub in dirs(&pcm).into_iter().filter(|p| numbered(&file_name(p), "sub")) {
                let Ok(status) = std::fs::read_to_string(sub.join("status")) else { continue };
                let status = status.trim();
                if status.is_empty() || status == "closed" {
                    continue;
                }
                let owner = status.lines().find_map(|l| l.strip_prefix("owner_pid").and_then(|r| r.trim_start().strip_prefix(':')).and_then(|r| r.trim().parse().ok()));
                out.push(OpenStream { name: format!("{}/{}/{}", file_name(&card), file_name(&pcm), file_name(&sub)), owner });
            }
        }
    }
    out
}

/// What `/proc/asound/cardN/pcmMp/subK/status` says about a playback substream (its first word or its `state:` line).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PcmState {
    /// Nobody has it open.
    Closed,
    /// Open but not set up (`OPEN`, `SETUP`).
    Open,
    /// Set up and ready, not playing: a sink that opened the card and is waiting (or a stream a paused element left behind).
    Prepared,
    Running,
    /// `PAUSED`, `DRAINING`, `XRUN`, `SUSPENDED`, `DISCONNECTED`, anything unknown.
    Other,
}

/// A playback substream with its state and owner, for the programs that must not take the speaker from someone.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stream {
    pub card: u32,
    /// `card0/pcm0p/sub0`
    pub name: String,
    pub state: PcmState,
    pub owner: Option<u32>,
}

/// Read a status file: the state and `owner_pid`.
pub fn parse_status(text: &str) -> (PcmState, Option<u32>) {
    let t = text.trim();
    if t.is_empty() || t.lines().next().is_some_and(|l| l.trim() == "closed") {
        return (PcmState::Closed, None);
    }
    let word = t.lines().find_map(|l| l.trim().strip_prefix("state:")).map(str::trim).unwrap_or("");
    let state = match word {
        "OPEN" | "SETUP" => PcmState::Open,
        "PREPARED" => PcmState::Prepared,
        "RUNNING" => PcmState::Running,
        _ => PcmState::Other,
    };
    let owner = t.lines().find_map(|l| l.strip_prefix("owner_pid").and_then(|r| r.trim_start().strip_prefix(':')).and_then(|r| r.trim().parse().ok()));
    (state, owner)
}

/// Every playback substream under `root`, closed ones included (capture is never looked at).
pub fn playback_streams(root: &Path) -> Vec<Stream> {
    let mut out = Vec::new();
    for card in dirs(root).into_iter().filter(|p| numbered(&file_name(p), "card")) {
        let Some(n) = file_name(&card).strip_prefix("card").and_then(|n| n.parse().ok()) else { continue };
        for pcm in dirs(&card).into_iter().filter(|p| file_name(p).strip_prefix("pcm").and_then(|r| r.strip_suffix('p')).is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))) {
            for sub in dirs(&pcm).into_iter().filter(|p| numbered(&file_name(p), "sub")) {
                let Ok(status) = std::fs::read_to_string(sub.join("status")) else { continue };
                let (state, owner) = parse_status(&status);
                out.push(Stream { card: n, name: format!("{}/{}/{}", file_name(&card), file_name(&pcm), file_name(&sub)), state, owner });
            }
        }
    }
    out
}

/// The rule for every program of IYAKKI's own that would open the speaker only to test it (the softvol "prime", the shutdown
/// silence, a sound test): with one owner per card (no dmix on the T2) an open from here can take the speaker from, or make
/// `Resource busy` for, the program that is playing or about to. So: refuse when ANY playback substream of the card is not
/// closed, whatever its state (a PREPARED one is a sink waiting for its first samples). Returns who and why, for the log.
pub fn must_not_open(streams: &[Stream], card: u32) -> Option<String> {
    streams.iter().find(|s| s.card == card && s.state != PcmState::Closed).map(|s| {
        let who = s.owner.map_or("another program".to_string(), |p| format!("process {p}"));
        format!("{} is {:?} ({who})", s.name, s.state)
    })
}

/// What [`quiet_with`] did, for the log and the tests.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Quieted {
    /// Cards whose `IYAKKI` control was set to 0.
    pub muted: Vec<u32>,
    /// Streams that were open when it began.
    pub found: usize,
    /// Processes sent SIGTERM / SIGKILL.
    pub termed: Vec<u32>,
    pub killed: Vec<u32>,
    /// The silence was written because a stream stayed open.
    pub silenced: bool,
    /// Streams still open at the end.
    pub left: usize,
}

fn wait_closed(env: &dyn Env, root: &Path, max: Duration) -> Vec<OpenStream> {
    let mut waited = Duration::ZERO;
    loop {
        let open = open_playback(root);
        if open.is_empty() || waited >= max {
            return open;
        }
        env.sleep(POLL);
        waited += POLL;
    }
}

fn owners(open: &[OpenStream], env: &dyn Env) -> Vec<u32> {
    let mut v: Vec<u32> = open.iter().filter_map(|s| s.owner).filter(|p| *p > 1 && *p != env.own_pid()).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The sequence, on the given tree and machine.
pub fn quiet_with(env: &dyn Env, root: &Path) -> Quieted {
    let mut q = Quieted::default();
    let cards = cards(root);
    if cards.is_empty() {
        return q;
    }
    for c in &cards {
        let args = ["-q", "-c", &c.to_string(), "sset", crate::softvol::CONTROL, "0%"].map(String::from);
        if env.amixer(&args).is_ok() {
            q.muted.push(*c);
        }
    }
    let open = open_playback(root);
    q.found = open.len();
    if open.is_empty() {
        return q;
    }
    q.termed = owners(&open, env);
    for p in &q.termed {
        env.signal(*p, libc::SIGTERM);
    }
    let mut open = wait_closed(env, root, TERM_WAIT);
    if !open.is_empty() {
        q.killed = owners(&open, env);
        for p in &q.killed {
            env.signal(*p, libc::SIGKILL);
        }
        open = wait_closed(env, root, KILL_WAIT);
    }
    if !open.is_empty() {
        env.silence(SILENCE_MAX);
        q.silenced = true;
    }
    q.left = open_playback(root).len();
    q
}

/// Called right before a restart or power-off. Bounded (about 2 s at most), never panics, does nothing without a card.
pub fn quiet_for_shutdown() {
    let r = std::panic::catch_unwind(|| quiet_with(&System, Path::new(ASOUND)));
    match r {
        Ok(q) if q.found > 0 => log::info!("shutdown: {} sound stream(s) open, ended {:?}, killed {:?}, silence {}, {} left", q.found, q.termed, q.killed, q.silenced, q.left),
        Ok(_) => {}
        Err(_) => log::warn!("shutdown: quieting the sound failed; going on"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Fake {
        calls: Arc<Mutex<Vec<String>>>,
        root: PathBuf,
        /// pids whose streams close on SIGTERM / on SIGKILL; (stream dir, pid) pairs
        on_term: Arc<Mutex<Vec<(PathBuf, u32)>>>,
        on_kill: Arc<Mutex<Vec<(PathBuf, u32)>>>,
        no_amixer: bool,
        slept: Arc<Mutex<Duration>>,
    }

    impl Fake {
        fn close(list: &[(PathBuf, u32)], pid: u32) {
            for (p, owner) in list {
                if *owner == pid {
                    std::fs::write(p.join("status"), "closed\n").unwrap();
                }
            }
        }
    }

    impl Env for Fake {
        fn amixer(&self, args: &[String]) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("amixer {}", args.join(" ")));
            if self.no_amixer { Err("not found".into()) } else { Ok(()) }
        }
        fn signal(&self, pid: u32, sig: i32) {
            self.calls.lock().unwrap().push(format!("signal {pid} {sig}"));
            let list = if sig == libc::SIGTERM { self.on_term.lock().unwrap().clone() } else { self.on_kill.lock().unwrap().clone() };
            Self::close(&list, pid);
        }
        fn sleep(&self, d: Duration) {
            *self.slept.lock().unwrap() += d;
        }
        fn silence(&self, max: Duration) {
            assert!(max <= SILENCE_MAX);
            self.calls.lock().unwrap().push("silence".into());
        }
        fn own_pid(&self) -> u32 {
            4242
        }
    }

    fn tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("quiet-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn stream(root: &Path, card: u32, pcm: &str, status: &str) -> PathBuf {
        let d = root.join(format!("card{card}/{pcm}/sub0"));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("status"), status).unwrap();
        d
    }

    fn running(pid: u32) -> String {
        format!("state: RUNNING\nowner_pid   : {pid}\ntrigger_time: 1.0\n")
    }

    fn fake(root: &Path) -> Fake {
        Fake { root: root.to_path_buf(), ..Fake::default() }
    }

    #[test]
    fn no_sound_card_does_nothing() {
        let root = tree("nocard");
        let f = fake(&root);
        assert_eq!(quiet_with(&f, &root), Quieted::default());
        assert!(f.calls.lock().unwrap().is_empty());
        assert_eq!(quiet_with(&f, &root.join("missing")), Quieted::default());
    }

    #[test]
    fn closed_streams_only_lower_the_volume() {
        let root = tree("closed");
        stream(&root, 1, "pcm0p", "closed\n");
        stream(&root, 1, "pcm1c", &running(900));
        let f = fake(&root);
        let q = quiet_with(&f, &root);
        assert_eq!(q.muted, [1]);
        assert_eq!(q.found, 0, "capture is never looked at");
        assert_eq!(*f.calls.lock().unwrap(), ["amixer -q -c 1 sset IYAKKI 0%"]);
        assert_eq!(*f.slept.lock().unwrap(), Duration::ZERO);
    }

    #[test]
    fn a_running_player_is_ended_politely_and_nothing_else_happens() {
        let root = tree("term");
        let s = stream(&root, 1, "pcm0p", &running(900));
        let f = fake(&root);
        f.on_term.lock().unwrap().push((s, 900));
        let q = quiet_with(&f, &root);
        assert_eq!((q.found, q.termed.clone(), q.killed.clone(), q.silenced, q.left), (1, vec![900], vec![], false, 0));
        assert_eq!(*f.calls.lock().unwrap(), ["amixer -q -c 1 sset IYAKKI 0%", "signal 900 15"], "volume first, then the producer");
    }

    #[test]
    fn a_stubborn_owner_is_killed_after_the_wait() {
        let root = tree("kill");
        let s = stream(&root, 0, "pcm0p", &running(901));
        let f = fake(&root);
        f.on_kill.lock().unwrap().push((s, 901));
        let q = quiet_with(&f, &root);
        assert_eq!((q.termed.clone(), q.killed.clone(), q.silenced, q.left), (vec![901], vec![901], false, 0));
        assert!(*f.slept.lock().unwrap() >= TERM_WAIT, "SIGTERM got its time");
        assert!(*f.slept.lock().unwrap() <= TOTAL_BUDGET, "never more than the budget");
    }

    #[test]
    fn a_stream_that_never_closes_gets_silence_within_the_budget() {
        let root = tree("stuck");
        stream(&root, 0, "pcm0p", &running(902));
        stream(&root, 0, "pcm1p", "state: RUNNING\n");
        let f = fake(&root);
        let q = quiet_with(&f, &root);
        assert_eq!((q.found, q.silenced, q.left), (2, true, 2));
        assert!(*f.slept.lock().unwrap() <= TOTAL_BUDGET);
        assert_eq!(f.calls.lock().unwrap().last().unwrap(), "silence");
    }

    #[test]
    fn this_process_and_init_are_never_signalled() {
        let root = tree("self");
        stream(&root, 0, "pcm0p", &running(4242));
        stream(&root, 0, "pcm1p", &running(1));
        let f = fake(&root);
        let q = quiet_with(&f, &root);
        assert!(q.termed.is_empty() && q.killed.is_empty());
        assert!(!f.calls.lock().unwrap().iter().any(|c| c.starts_with("signal")));
        assert!(q.silenced);
    }

    #[test]
    fn a_missing_amixer_does_not_stop_the_rest() {
        let root = tree("noamixer");
        let s = stream(&root, 2, "pcm0p", &running(903));
        let mut f = fake(&root);
        f.no_amixer = true;
        f.on_term.lock().unwrap().push((s, 903));
        let q = quiet_with(&f, &root);
        assert!(q.muted.is_empty());
        assert_eq!(q.termed, [903]);
        assert_eq!(q.left, 0);
    }

    #[test]
    fn two_streams_of_one_owner_get_one_signal() {
        let root = tree("dup");
        let a = stream(&root, 0, "pcm0p", &running(904));
        let d = root.join("card0/pcm0p/sub1");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("status"), running(904)).unwrap();
        let f = fake(&root);
        f.on_term.lock().unwrap().extend([(a, 904), (d, 904)]);
        let q = quiet_with(&f, &root);
        assert_eq!((q.found, q.termed), (2, vec![904]));
    }

    #[test]
    fn the_status_text_is_read_as_state_and_owner() {
        assert_eq!(parse_status("closed\n"), (PcmState::Closed, None));
        assert_eq!(parse_status(""), (PcmState::Closed, None));
        assert_eq!(parse_status(&running(77)), (PcmState::Running, Some(77)));
        assert_eq!(parse_status("state: PREPARED\nowner_pid   : 12\n"), (PcmState::Prepared, Some(12)));
        assert_eq!(parse_status("state: SETUP\n").0, PcmState::Open);
        assert_eq!(parse_status("state: XRUN\n").0, PcmState::Other);
        assert_eq!(parse_status("state: PAUSED\nowner_pid : x\n"), (PcmState::Other, None));
    }

    #[test]
    fn the_speaker_may_be_opened_only_when_its_card_has_no_open_stream() {
        let root = tree("mustnot");
        stream(&root, 0, "pcm0p", "closed\n");
        stream(&root, 0, "pcm0c", &running(5));
        stream(&root, 1, "pcm0p", "state: PREPARED\nowner_pid : 9\n");
        let s = playback_streams(&root);
        assert_eq!(s.len(), 2, "capture is not listed");
        assert_eq!(must_not_open(&s, 0), None, "closed: free");
        assert!(must_not_open(&s, 1).unwrap().contains("process 9"), "another card's stream does not matter, this one's does");
        assert_eq!(must_not_open(&s, 2), None);
        for (status, state) in [("state: OPEN\n", PcmState::Open), ("state: SETUP\n", PcmState::Open), ("state: PREPARED\n", PcmState::Prepared), ("state: RUNNING\n", PcmState::Running), ("state: PAUSED\n", PcmState::Other), ("state: DRAINING\n", PcmState::Other), ("state: XRUN\n", PcmState::Other), ("state: SUSPENDED\n", PcmState::Other)] {
            stream(&root, 0, "pcm0p", status);
            let s = playback_streams(&root);
            assert_eq!(s.iter().find(|x| x.card == 0).unwrap().state, state, "{status}");
            assert!(must_not_open(&s, 0).is_some(), "{status}: must not open");
        }
        assert!(playback_streams(&root.join("missing")).is_empty());
    }

    #[test]
    fn the_real_call_returns_quickly_on_a_machine_without_open_streams() {
        let t = std::time::Instant::now();
        quiet_for_shutdown();
        assert!(t.elapsed() < Duration::from_secs(3));
    }
}
