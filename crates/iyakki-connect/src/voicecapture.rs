//! Reading the person's microphone and playing the answer on the three desktop systems (F-VOICE-61), as commands of programs the person
//! already has (never shipped here): `arecord` and `aplay` on Linux, and the person's own FFmpeg (`ffmpeg` with `avfoundation` on macOS, `dshow`
//! on Windows; `ffplay` for the sound) where the system has no recorder of its own. Everything that decides what is run is plain data and plain
//! text parsing, so it is tested without a microphone; only [`CommandMic::open`] and [`CommandSink::play`] start a program, and only when the
//! person has said yes to the card.

use std::io::Write;
use std::sync::Arc;
use std::thread::JoinHandle;

use iyakki_audiolink::Mic;

/// The audio format of the link: 16 kHz, mono, 16-bit little-endian.
pub const RATE: &str = "16000";

/// What to run to read the microphone as raw 16 kHz mono 16-bit sound on standard output.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureCmd {
    pub program: String,
    pub args: Vec<String>,
    /// The words for a computer that has no such program.
    pub missing: String,
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

impl CaptureCmd {
    pub fn linux() -> CaptureCmd {
        CaptureCmd { program: "arecord".into(), args: s(&["-q", "-t", "raw", "-f", "S16_LE", "-r", RATE, "-c", "1"]), missing: "this computer has no `arecord` to read the microphone with".into() }
    }

    /// macOS: FFmpeg's `avfoundation` input, the audio device by its number (`:N`).
    pub fn macos(device: u32) -> CaptureCmd {
        CaptureCmd {
            program: "ffmpeg".into(),
            args: vec!["-loglevel".into(), "error".into(), "-nostdin".into(), "-f".into(), "avfoundation".into(), "-i".into(), format!(":{device}"), "-ac".into(), "1".into(), "-ar".into(), RATE.into(), "-f".into(), "s16le".into(), "-".into()],
            missing: "this computer has no `ffmpeg` to read the microphone with (the audio and video add-on provides it)".into(),
        }
    }

    /// Windows: FFmpeg's `dshow` input, the audio device by name. A name that could be taken for an option or holds a control character is refused.
    pub fn windows(device: &str) -> Result<CaptureCmd, String> {
        if device.is_empty() || device.len() > 200 || device.chars().any(|c| c.is_control() || c == '"') {
            return Err("the microphone's name is not usable".into());
        }
        Ok(CaptureCmd {
            program: "ffmpeg".into(),
            args: vec!["-loglevel".into(), "error".into(), "-nostdin".into(), "-f".into(), "dshow".into(), "-i".into(), format!("audio={device}"), "-ac".into(), "1".into(), "-ar".into(), RATE.into(), "-f".into(), "s16le".into(), "-".into()],
            missing: "this computer has no `ffmpeg` to read the microphone with (the audio and video add-on provides it)".into(),
        })
    }
}

/// What to run to play raw 16 kHz mono 16-bit sound given on standard input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlayCmd {
    pub program: String,
    pub args: Vec<String>,
    pub missing: String,
}

impl PlayCmd {
    pub fn linux() -> PlayCmd {
        PlayCmd { program: "aplay".into(), args: s(&["-q", "-t", "raw", "-f", "S16_LE", "-r", RATE, "-c", "1"]), missing: "this computer has no `aplay` to play the answer with".into() }
    }
    /// macOS and Windows: FFmpeg's player with no window.
    pub fn ffplay() -> PlayCmd {
        PlayCmd { program: "ffplay".into(), args: s(&["-nodisp", "-autoexit", "-loglevel", "error", "-f", "s16le", "-ar", RATE, "-ac", "1", "-i", "-"]), missing: "this computer has no `ffplay` to play the answer with (the audio and video add-on provides it)".into() }
    }
}

/// The audio devices FFmpeg lists for `dshow` (`ffmpeg -list_devices true -f dshow -i dummy`, text on its error output): the names of the audio ones.
/// Both layouts are read: the newer one marks every line `"name" (audio)`, the older has a heading `DirectShow audio devices`. "Alternative name"
/// lines are not devices.
pub fn parse_dshow_audio_devices(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_audio = false;
    for line in text.lines() {
        let l = line.trim();
        if l.contains("DirectShow audio devices") {
            in_audio = true;
            continue;
        }
        if l.contains("DirectShow video devices") {
            in_audio = false;
            continue;
        }
        if l.contains("Alternative name") {
            continue;
        }
        let Some(a) = l.find('"') else { continue };
        let Some(b) = l[a + 1..].find('"') else { continue };
        let name = &l[a + 1..a + 1 + b];
        let typed = l[a + 2 + b..].contains("(audio)");
        let untyped_video = l[a + 2 + b..].contains("(video)") || l[a + 2 + b..].contains("(none)");
        if !name.is_empty() && (typed || (in_audio && !untyped_video)) && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    out
}

/// The audio devices FFmpeg lists for `avfoundation` (`ffmpeg -f avfoundation -list_devices true -i ""`): (number, name) under the audio heading.
pub fn parse_avfoundation_audio_devices(text: &str) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    let mut in_audio = false;
    for line in text.lines() {
        if line.contains("AVFoundation audio devices") {
            in_audio = true;
            continue;
        }
        if line.contains("AVFoundation video devices") {
            in_audio = false;
            continue;
        }
        if !in_audio {
            continue;
        }
        // `[AVFoundation indev @ 0x...] [0] MacBook Pro Microphone`
        let Some(p) = line.rfind("] [") else { continue };
        let rest = &line[p + 3..];
        let Some((n, name)) = rest.split_once("] ") else { continue };
        if let Ok(n) = n.parse::<u32>() {
            out.push((n, name.trim().to_string()));
        }
    }
    out
}


/// The program of the system, run for real (output of both streams).
pub fn run_lister(program: &str, args: &[String]) -> Result<String, String> {
    let o = std::process::Command::new(program).args(args).stdin(std::process::Stdio::null()).output().map_err(|_| format!("`{program}` is not on this computer"))?;
    Ok(format!("{}\n{}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout)))
}

/// The capture command for the system this program runs on, found with `list` for the systems that need a device: the first audio device.
pub fn capture_for(os: &str, list: &dyn Fn(&str, &[String]) -> Result<String, String>) -> Result<CaptureCmd, String> {
    match os {
        "linux" => Ok(CaptureCmd::linux()),
        "macos" => {
            let text = list("ffmpeg", &s(&["-hide_banner", "-f", "avfoundation", "-list_devices", "true", "-i", ""]))?;
            let first = parse_avfoundation_audio_devices(&text).into_iter().next().ok_or("ffmpeg lists no microphone on this computer")?;
            Ok(CaptureCmd::macos(first.0))
        }
        "windows" => {
            let text = list("ffmpeg", &s(&["-hide_banner", "-list_devices", "true", "-f", "dshow", "-i", "dummy"]))?;
            let first = parse_dshow_audio_devices(&text).into_iter().next().ok_or("ffmpeg lists no microphone on this computer")?;
            CaptureCmd::windows(&first)
        }
        other => Err(format!("reading the microphone is not written for {other}")),
    }
}

/// The player for this system.
pub fn play_for(os: &str) -> Result<PlayCmd, String> {
    match os {
        "linux" => Ok(PlayCmd::linux()),
        "macos" | "windows" => Ok(PlayCmd::ffplay()),
        other => Err(format!("playing the answer is not written for {other}")),
    }
}

/// Raw little-endian 16-bit bytes into samples; an odd last byte is kept in `odd` for the next chunk. Never more than `cap` samples are kept
/// waiting (a stalled link must not fill the memory): the oldest go first.
pub fn push_pcm(heard: &mut Vec<i16>, odd: &mut Option<u8>, chunk: &[u8], cap: usize) {
    let mut bytes: Vec<u8> = odd.take().into_iter().collect();
    bytes.extend_from_slice(chunk);
    if bytes.len() % 2 == 1 {
        *odd = bytes.pop();
    }
    heard.extend(bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
    let over = heard.len().saturating_sub(cap);
    heard.drain(..over);
}

/// The microphone read by a program (see [`CaptureCmd`]): a thread keeps what came in until it is drained. Closing kills the program.
pub struct CommandMic {
    cmd: CaptureCmd,
    child: Option<std::process::Child>,
    heard: Arc<std::sync::Mutex<Vec<i16>>>,
    reader: Option<JoinHandle<()>>,
}

impl CommandMic {
    pub fn new(cmd: CaptureCmd) -> CommandMic {
        CommandMic { cmd, child: None, heard: Arc::new(std::sync::Mutex::new(Vec::new())), reader: None }
    }

    /// The microphone of this computer's own system.
    pub fn for_this_system() -> Result<CommandMic, String> {
        Ok(CommandMic::new(capture_for(std::env::consts::OS, &run_lister)?))
    }
}

impl Mic for CommandMic {
    fn open(&mut self) -> Result<(), String> {
        if self.child.is_some() {
            return Ok(());
        }
        let mut child = std::process::Command::new(&self.cmd.program).args(&self.cmd.args).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).stdin(std::process::Stdio::null()).spawn().map_err(|_| self.cmd.missing.clone())?;
        let mut out = child.stdout.take().ok_or("the microphone gave no stream")?;
        let heard = self.heard.clone();
        self.reader = Some(std::thread::spawn(move || {
            use std::io::Read;
            let mut buf = [0u8; 3200];
            let mut odd: Option<u8> = None;
            while let Ok(n) = out.read(&mut buf) {
                if n == 0 {
                    break;
                }
                push_pcm(&mut heard.lock().unwrap_or_else(std::sync::PoisonError::into_inner), &mut odd, &buf[..n], 32_000);
            }
        }));
        self.child = Some(child);
        Ok(())
    }
    fn is_open(&self) -> bool {
        self.child.is_some()
    }
    fn drain(&mut self) -> Vec<i16> {
        std::mem::take(&mut *self.heard.lock().unwrap_or_else(std::sync::PoisonError::into_inner))
    }
    fn alive(&mut self) -> bool {
        self.child.as_mut().is_some_and(|c| matches!(c.try_wait(), Ok(None)))
    }
    fn close(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
        self.heard.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
    }
}

impl Drop for CommandMic {
    fn drop(&mut self) {
        self.close();
    }
}

/// Where the answer's sound goes on this computer.
pub trait Sink: Send {
    fn play(&mut self, samples: &[i16]) -> Result<(), String>;
}

/// The answer played by a program (see [`PlayCmd`]): one run per piece of sound, the bytes given on its standard input.
pub struct CommandSink {
    cmd: PlayCmd,
}

impl CommandSink {
    pub fn new(cmd: PlayCmd) -> CommandSink {
        CommandSink { cmd }
    }
    pub fn for_this_system() -> Result<CommandSink, String> {
        Ok(CommandSink::new(play_for(std::env::consts::OS)?))
    }
}

/// Samples as the raw bytes the player reads.
pub fn pcm_bytes(samples: &[i16]) -> Vec<u8> {
    samples.iter().flat_map(|x| x.to_le_bytes()).collect()
}

impl Sink for CommandSink {
    fn play(&mut self, samples: &[i16]) -> Result<(), String> {
        if samples.is_empty() {
            return Ok(());
        }
        let mut child = std::process::Command::new(&self.cmd.program).args(&self.cmd.args).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().map_err(|_| self.cmd.missing.clone())?;
        if let Some(mut i) = child.stdin.take() {
            let _ = i.write_all(&pcm_bytes(samples));
        }
        child.wait().map(|_| ()).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DSHOW_NEW: &str = "[dshow @ 000001] DirectShow video devices (some may be both video and audio devices)\n[dshow @ 000001]  \"Integrated Camera\" (video)\n[dshow @ 000001]   Alternative name \"@device_pnp_x\"\n[dshow @ 000001] DirectShow audio devices\n[dshow @ 000001]  \"Microphone Array (Realtek)\" (audio)\n[dshow @ 000001]   Alternative name \"@device_cm_{33D9}\"\n[dshow @ 000001]  \"Headset (USB)\" (audio)\n";
    const DSHOW_OLD: &str = "[dshow @ 01] DirectShow video devices\n[dshow @ 01]  \"Cam\"\n[dshow @ 01] DirectShow audio devices\n[dshow @ 01]  \"Line In\"\n[dshow @ 01]  \"Mic\"\n";
    const AVF: &str = "[AVFoundation indev @ 0x1] AVFoundation video devices:\n[AVFoundation indev @ 0x1] [0] FaceTime HD Camera\n[AVFoundation indev @ 0x1] AVFoundation audio devices:\n[AVFoundation indev @ 0x1] [0] MacBook Pro Microphone\n[AVFoundation indev @ 0x1] [1] External Headset\n";

    #[test]
    fn each_system_gets_its_own_capture_command_and_the_linux_one_is_what_it_always_was() {
        let l = CaptureCmd::linux();
        assert_eq!((l.program.as_str(), l.args.join(" ")), ("arecord", "-q -t raw -f S16_LE -r 16000 -c 1".to_string()));
        let m = CaptureCmd::macos(1);
        assert_eq!(m.program, "ffmpeg");
        assert!(m.args.windows(2).any(|w| w == ["-f", "avfoundation"]) && m.args.windows(2).any(|w| w == ["-i", ":1"]) && m.args.ends_with(&["-f".to_string(), "s16le".to_string(), "-".to_string()]));
        let w = CaptureCmd::windows("Headset (USB)").unwrap();
        assert!(w.args.windows(2).any(|a| a == ["-f", "dshow"]) && w.args.windows(2).any(|a| a == ["-i", "audio=Headset (USB)"]));
        assert!(w.args.windows(2).any(|a| a == ["-ar", "16000"]) && w.args.windows(2).any(|a| a == ["-ac", "1"]), "16 kHz mono, as the link wants");
    }

    #[test]
    fn a_device_name_that_is_empty_quoted_or_holds_control_characters_is_never_put_in_a_command() {
        for bad in ["", "a\"b", "a\nb", &"x".repeat(201)] {
            assert!(CaptureCmd::windows(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn the_device_lists_of_both_ffmpeg_layouts_are_read_and_only_audio_devices_come_out() {
        assert_eq!(parse_dshow_audio_devices(DSHOW_NEW), vec!["Microphone Array (Realtek)", "Headset (USB)"]);
        assert_eq!(parse_dshow_audio_devices(DSHOW_OLD), vec!["Line In", "Mic"]);
        assert_eq!(parse_avfoundation_audio_devices(AVF), vec![(0, "MacBook Pro Microphone".to_string()), (1, "External Headset".to_string())]);
        assert!(parse_dshow_audio_devices("nothing here").is_empty() && parse_avfoundation_audio_devices("").is_empty());
    }

    #[test]
    fn the_first_listed_microphone_is_chosen_and_a_computer_without_one_says_so() {
        let ok = |text: &'static str| move |_: &str, _: &[String]| Ok(text.to_string());
        assert_eq!(capture_for("macos", &ok(AVF)).unwrap(), CaptureCmd::macos(0));
        assert_eq!(capture_for("windows", &ok(DSHOW_NEW)).unwrap(), CaptureCmd::windows("Microphone Array (Realtek)").unwrap());
        assert_eq!(capture_for("linux", &ok("")).unwrap(), CaptureCmd::linux(), "Linux needs no list");
        assert!(capture_for("macos", &ok("[x] nothing")).unwrap_err().contains("no microphone"));
        assert!(capture_for("windows", &|_: &str, _: &[String]| Err("`ffmpeg` is not on this computer".into())).unwrap_err().contains("ffmpeg"));
        assert!(capture_for("plan9", &ok("")).unwrap_err().contains("not written"));
        let asked = std::sync::Mutex::new(Vec::new());
        capture_for("macos", &|p: &str, a: &[String]| {
            asked.lock().unwrap().push(format!("{p} {}", a.join(" ")));
            Ok(AVF.to_string())
        })
        .unwrap();
        assert!(asked.lock().unwrap()[0].contains("-list_devices true"), "the list is only read, nothing records");
    }

    #[test]
    fn the_answer_is_played_by_the_systems_own_player_or_ffplay() {
        assert_eq!(play_for("linux").unwrap(), PlayCmd::linux());
        assert_eq!(play_for("macos").unwrap(), PlayCmd::ffplay());
        assert_eq!(play_for("windows").unwrap(), PlayCmd::ffplay());
        assert!(play_for("plan9").is_err());
        assert!(PlayCmd::ffplay().args.contains(&"-nodisp".to_string()));
    }

    #[test]
    fn raw_bytes_become_samples_across_chunk_edges_and_never_pile_up() {
        let mut h = Vec::new();
        let mut odd = None;
        let all = pcm_bytes(&[1, -2, 300, i16::MIN, i16::MAX]);
        push_pcm(&mut h, &mut odd, &all[..3], 100);
        assert_eq!((h.clone(), odd), (vec![1], Some(all[2])), "an odd byte waits for its partner");
        push_pcm(&mut h, &mut odd, &all[3..], 100);
        assert_eq!((h.clone(), odd), (vec![1, -2, 300, i16::MIN, i16::MAX], None));
        push_pcm(&mut h, &mut odd, &pcm_bytes(&[7, 8, 9]), 4);
        assert_eq!(h, vec![i16::MIN, i16::MAX].into_iter().chain([7, 8, 9]).skip(1).collect::<Vec<_>>(), "the oldest go first");
    }

    #[test]
    fn a_program_that_is_not_there_is_named_in_words_and_nothing_stays_open() {
        let mut m = CommandMic::new(CaptureCmd { program: "iyakki-no-such-recorder".into(), args: vec![], missing: "no recorder here".into() });
        assert_eq!(m.open().unwrap_err(), "no recorder here");
        assert!(!m.is_open() && m.drain().is_empty());
        let mut k = CommandSink::new(PlayCmd { program: "iyakki-no-such-player".into(), args: vec![], missing: "no player here".into() });
        assert_eq!(k.play(&[1, 2]).unwrap_err(), "no player here");
        assert!(k.play(&[]).is_ok(), "nothing to play is not an error");
    }
}
