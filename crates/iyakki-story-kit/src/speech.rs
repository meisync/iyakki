//! The speech status of TALES: which engine reads, what the page says about it, the voices it offers and the one-click engines
//! that need no provider. Pure rules over the voice leaf crate; the only file-system reads are `Path::is_file`, `read_dir` for
//! piper voices and the `which` search (alpha.11 AP1, D-P6-03). `iyakki_shared::story` re-exports every item.

use std::path::{Path, PathBuf};

use iyakki_story_model::*;
use iyakki_voice_model::hostvoice::HostVoice;
use iyakki_voice_model::voice::{self, VoiceSettings};
use iyakki_voice_model::which;
use iyakki_voice_model::{hostengines, hostvoice};

pub fn tts_status(v: &VoiceSettings, voice_allowed: bool, path: &str) -> TtsStatus {
    tts_status_for(v, voice_allowed, path, &HostVoice::new())
}

/// The engine that reads: the one of Settings → Voice, or, when none is set up and the screen can speak, the browser voice
/// (nothing to install, nothing to configure).
pub fn effective_engine(v: &VoiceSettings, host: &HostVoice) -> Option<voice::SpeechEngine> {
    effective_engine_auto(v, host, None)
}

/// [`effective_engine`] with the best speech engine found on this computer (`hostengines`): what the person set, else the
/// engine of this computer, else the browser voice of the screen.
pub fn effective_engine_auto(v: &VoiceSettings, host: &HostVoice, auto: Option<&voice::SpeechEngine>) -> Option<voice::SpeechEngine> {
    match &v.tts {
        Some(e) => Some(e.clone()),
        None => match auto {
            Some(a) => Some(a.clone()),
            None if host.is_ready() => Some(voice::SpeechEngine::Browser),
            None => None,
        },
    }
}

/// [`tts_status`] knowing what the screen in front of the person can do.
pub fn tts_status_for(v: &VoiceSettings, voice_allowed: bool, path: &str, host: &HostVoice) -> TtsStatus {
    tts_status_auto(v, voice_allowed, path, host, None)
}

/// [`tts_status_for`] with the engine of this computer that is used when nothing is set (see [`effective_engine_auto`]).
pub fn tts_status_auto(v: &VoiceSettings, voice_allowed: bool, path: &str, host: &HostVoice, auto: Option<&voice::SpeechEngine>) -> TtsStatus {
    use hostvoice::{Front, Support};
    if !voice_allowed {
        return TtsStatus::NotAllowed;
    }
    let Some(engine) = effective_engine_auto(v, host, auto) else { return TtsStatus::NoEngine };
    if let voice::SpeechEngine::Host { engine: id, program, voice: vo } = &engine {
        if let Err(e) = hostengines::validate_host(id, program.as_deref(), vo.as_deref()) {
            return TtsStatus::HostUnavailable(e);
        }
        if let Some(p) = program.as_deref().filter(|p| !p.is_empty()) {
            if !Path::new(p).is_file() {
                return TtsStatus::HostUnavailable(format!("{p} is no longer there; open Settings → Voice and press Re-check"));
            }
        }
        if hostengines::needs_player(&engine) {
            let player = voice::split_command(&v.play_cmd).ok().and_then(|a| a.into_iter().next()).unwrap_or_default();
            if hostengines::player_command(&v.play_cmd, path).is_none() {
                return TtsStatus::NoPlayer(player);
            }
        }
        return TtsStatus::Ready { engine: if v.tts.is_some() { engine.label() } else { format!("{} (found on this computer)", engine.label()) } };
    }
    if engine == voice::SpeechEngine::Browser {
        return match host.support() {
            Support::Yes => TtsStatus::Ready { engine: if v.tts.is_some() { "the browser voice".into() } else { "the browser voice (nothing else is set up)".into() } },
            Support::Checking => TtsStatus::BrowserChecking,
            Support::No(why) => TtsStatus::BrowserUnavailable(why),
            Support::Unknown => TtsStatus::BrowserUnavailable(match host.front() {
                Front::Console => BROWSER_VOICE_CONSOLE.to_string(),
                _ => "the screen has not answered yet".to_string(),
            }),
        };
    }
    let player = voice::split_command(&v.play_cmd).ok().and_then(|a| a.into_iter().next()).unwrap_or_default();
    if which(&player, path).is_none() {
        return TtsStatus::NoPlayer(player);
    }
    TtsStatus::Ready { engine: engine.label() }
}

/// Why the browser voice cannot be used on a console, and how to get it.
pub const BROWSER_VOICE_CONSOLE: &str = "this screen is the console, which has no browser. Browser voice works when IYAKKI is opened in a web browser: start the browser front end (iyakki web, or iyakki serve --web), open its address in your browser (https://this-computer:8443), sign in and open TALES there";

/// Shown on the page when there is no speech engine. The story is still written, shown and saved as text.
pub const NO_ENGINE_HELP: &str = "No text-to-speech engine is set up and none was found on this computer (espeak-ng, speech-dispatcher, piper, macOS say or Windows speech), so the story is written and saved as text but not read aloud. Press Turn on reading aloud to look again (the desktop window can then use a voice helper in your web browser), or install one of them: docs/VOICE.md says how. The easiest way is the browser voice: open IYAKKI in a web browser (iyakki web, or iyakki serve --web, then open its address) and TALES reads with the voices of that browser, with nothing to install. Otherwise open Settings → Voice, choose Text to speech and set an OpenAI-compatible endpoint (an address ending in /v1/audio/speech, a model such as tts-1 and a voice) or a local command such as: piper --model voice.onnx --output_file {out}";

pub fn status_text(s: &TtsStatus) -> String {
    match s {
        TtsStatus::Ready { engine } => format!("Reading aloud with {engine}."),
        TtsStatus::NoEngine => NO_ENGINE_HELP.to_string(),
        TtsStatus::NotAllowed => "Voice is not enabled for your account, so the story is written but not read aloud. An administrator can enable it in Users.".into(),
        TtsStatus::NoPlayer(p) => format!("The play command `{p}` is not installed on this computer, so nothing can be heard. Install it, or change the play command in Settings → Voice."),
        TtsStatus::BrowserUnavailable(why) => format!("Browser voice is chosen in Settings → Voice, but it cannot be used here: {why}. The story is written and saved as text. Choose another engine in Settings → Voice, or open IYAKKI in a web browser."),
        TtsStatus::BrowserChecking => "Checking whether this screen can speak…".into(),
        TtsStatus::HostUnavailable(why) => format!("The speech engine chosen in Settings → Voice cannot be used: {why}. The story is written and saved as text."),
    }
}

/// Voices the page offers: the engine's own, and for OpenAI's service the six it has. A local engine's voice is part of
/// its command or its settings.
pub fn voice_choices(engine: &voice::SpeechEngine) -> Vec<String> {
    match engine {
        voice::SpeechEngine::Http { url, voice, .. } => {
            let mut v: Vec<String> = Vec::new();
            if let Some(x) = voice.as_ref().filter(|x| !x.is_empty()) {
                v.push(x.clone());
            }
            if iyakki_sec::url_host(url).is_some_and(|h| h.ends_with("openai.com")) {
                for x in ["alloy", "echo", "fable", "onyx", "nova", "shimmer"] {
                    if !v.iter().any(|y| y == x) {
                        v.push(x.to_string());
                    }
                }
            }
            if v.is_empty() {
                v.push("alloy".into());
            }
            v
        }
        voice::SpeechEngine::Command { .. } | voice::SpeechEngine::Browser | voice::SpeechEngine::Host { .. } | voice::SpeechEngine::Gemini { .. } => Vec::new(),
    }
}

/// Does the engine take a speed? An HTTP engine always may; a command only if it names `{speed}` or `{length_scale}`.
pub use voice::engine_takes_speed;

/// A way to set speech up with one click.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuickEngine {
    pub label: String,
    pub engine: voice::SpeechEngine,
}

pub fn shell_word(s: &str) -> String {
    if s.contains(char::is_whitespace) || s.contains('"') { format!("\"{}\"", s.replace('"', "\\\"")) } else { s.to_string() }
}

/// The first piper voice found in the usual places (a `.onnx` file next to its `.onnx.json`).
pub fn find_piper_voice(dirs: &[PathBuf]) -> Option<PathBuf> {
    let mut found = Vec::new();
    for d in dirs {
        let mut stack = vec![(d.clone(), 0)];
        while let Some((dir, depth)) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&dir) else { continue };
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() && depth < 2 {
                    stack.push((p, depth + 1));
                } else if p.extension().is_some_and(|x| x == "onnx") && PathBuf::from(format!("{}.json", p.display())).is_file() {
                    found.push(p);
                }
            }
        }
    }
    found.sort_by_key(|p| (!p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("en_")), p.clone()));
    found.into_iter().next()
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn voice_not_allowed_and_no_engine_are_told_apart() {
        let v = VoiceSettings::default();
        assert_eq!(tts_status(&v, false, ""), TtsStatus::NotAllowed);
        assert_eq!(tts_status(&v, true, ""), TtsStatus::NoEngine);
        assert!(status_text(&TtsStatus::NoEngine).contains("Settings"));
        assert_eq!(status_text(&TtsStatus::Ready { engine: "x".into() }), "Reading aloud with x.");
    }

    #[test]
    fn an_engine_that_is_set_wins_over_the_one_found() {
        let v = VoiceSettings { tts: Some(voice::SpeechEngine::Browser), ..Default::default() };
        let host = HostVoice::new();
        assert_eq!(effective_engine(&v, &host), Some(voice::SpeechEngine::Browser));
        let auto = voice::SpeechEngine::Command { command: "say {text}".into() };
        assert_eq!(effective_engine_auto(&VoiceSettings::default(), &host, Some(&auto)), Some(auto));
    }

    #[test]
    fn voices_and_shell_words() {
        assert!(voice_choices(&voice::SpeechEngine::Browser).is_empty());
        assert_eq!(shell_word("a b"), "\"a b\"");
        assert_eq!(shell_word("ab"), "ab");
        assert!(find_piper_voice(&[PathBuf::from("/nonexistent-iyakki-dir")]).is_none());
    }
}
