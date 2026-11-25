// The pure types of TALES (options, enums, story text, sentences, reading traits) live in the leaf crate `iyakki-story-model`; re-exported here under the old path.
pub use iyakki_story_model::*;

// ---- moved from iyakki-agent (F-SPLIT-03) ----

pub use iyakki_scene::{image_prompt_with, svg_messages};

// ---- merged from iyakki-agent (F-SPLIT-03) ----
// TALES: an AI story generator that reads the story aloud. Everything here is plain logic with no screen in it:
//
// * the options a person picks ([`StoryOptions`]) and the prompt built from them ([`messages`]), including the safety rules
//   for children; the model gets **no tools at all**;
// * turning the model's streamed text into a title, chapter headings and paragraphs ([`parse_story`]), even while it is
//   still being written;
// * splitting paragraphs into sentences and the sentences into pieces a speech engine says in one go ([`sentence_ranges`],
//   [`chunks_for`]);
// * the reading pipeline ([`Reader`]): piece N + 1 is synthesised while piece N plays, with pause, resume, stop, seek and a
//   level curve taken from the real audio; the speech engine and the speaker are traits, so tests use fakes;
// * the library of saved stories under the account's `Stories` folder ([`StoryStore`]), always checked against the
//   account's file scopes.
//
// Speech comes from [`crate::voice`] (an OpenAI-compatible endpoint or a local command such as piper), exactly as in CHAT.

use std::path::PathBuf;
use std::time::Duration;


use crate::voice;

pub use iyakki_story_kit::library::*;
pub use iyakki_story_kit::speech::*;
pub use iyakki_story_kit::steer::*;
pub use iyakki_story_kit::{assemble_audio, assemble_audio_timed, envelope, messages, render_audio, speech_edges, split_audio, PrebuiltSynth};

// ------------------------------------------------------------------------------------------------------------ speech engine status


pub use iyakki_voice_model::which;

/// One-click engines: an installed `piper` with a voice file, and the OpenAI-compatible speech endpoint of a provider that
/// is already set up under Models (with its key). `voice_dirs` is where to look for piper voices.
pub fn quick_engines(providers: &[crate::providers::ProviderCfg], path: &str, voice_dirs: &[PathBuf]) -> Vec<QuickEngine> {
    use crate::providers::Kind;
    let mut v = Vec::new();
    if let (Some(piper), Some(model)) = (which("piper", path), find_piper_voice(voice_dirs)) {
        let command = format!("{} --model {} --output_file {{out}} --length_scale {{length_scale}}", shell_word(&piper.to_string_lossy()), shell_word(&model.to_string_lossy()));
        v.push(QuickEngine { label: format!("Use piper ({})", model.file_stem().map_or(String::new(), |s| s.to_string_lossy().trim_end_matches(".onnx").to_string())), engine: voice::SpeechEngine::Command { command } });
    }
    for p in providers.iter().filter(|p| p.enabled) {
        if p.kind == Kind::Lemonade {
            // a Lemonade server's own speech models (Kokoro), by the type its model list gave them
            for (m, _) in p.model_kinds.iter().filter(|(_, k)| *k == crate::lemonade::ModelType::TextToSpeech.key()) {
                if let Ok(b) = p.base() {
                    v.push(QuickEngine { label: format!("Use {m} on {}", p.id), engine: crate::lemonade::tts_engine(&b, m, None, p.key_ref.clone()) });
                }
            }
            continue;
        }
        let base = match (p.kind, p.base_url.as_deref()) {
            (Kind::OpenAi, None) => "https://api.openai.com/v1".to_string(),
            (Kind::OpenAi | Kind::Compat, Some(b)) if !b.is_empty() => b.trim_end_matches('/').to_string(),
            _ => continue,
        };
        let base = if base.ends_with("/v1") { base } else { format!("{base}/v1") };
        let engine = voice::SpeechEngine::Http { url: format!("{base}/audio/speech"), model: "tts-1".into(), key_ref: p.key_ref.clone(), voice: Some("alloy".into()), language: None };
        if engine.validate().is_ok() {
            v.push(QuickEngine { label: format!("Use {} for speech", p.id), engine });
        }
    }
    v
}

// ------------------------------------------------------------------------------------------------------------ pictures
















pub use iyakki_k8s_model::b64::base64_encode;

pub use iyakki_story_kit::{export_html, export_html_with, html_escape, mime_of};

// ---------------------------------------------------------------------------------------------------------------- tests

// ------------------------------------------------------------------------------------------------------------ an agent writes

/// Follow an external agent's run (started with `no_tools`) until it ends: the loop is `iyakki_story_kit::follow_run`, here over the
/// agent's run handle.
pub fn follow_agent_run(h: &crate::agent::RunHandle, stopped: &dyn Fn() -> bool, on_text: &mut dyn FnMut(&str), on_thinking: &mut dyn FnMut(&str)) -> Result<String, String> {
    use crate::agent::{RunEvent, Verdict};
    use iyakki_story_kit::{RunFeed, RunMsg};
    struct Feed<'a>(&'a crate::agent::RunHandle);
    impl RunFeed for Feed<'_> {
        fn recv(&self, wait: Duration) -> Result<RunMsg, std::sync::mpsc::RecvTimeoutError> {
            Ok(match self.0.events.recv_timeout(wait)? {
                RunEvent::Delta { text } => RunMsg::Delta(text),
                RunEvent::Thinking { text, .. } => RunMsg::Thinking(text),
                RunEvent::Model { text, calls } => RunMsg::Model { text, calls: calls.into_iter().map(|c| c.id).collect() },
                RunEvent::Approval { id, .. } => RunMsg::Approval(id),
                RunEvent::Final { text } => RunMsg::Final(text),
                RunEvent::Failed { error } => RunMsg::Failed(error),
                RunEvent::Stopped { .. } => RunMsg::Stopped,
                _ => RunMsg::Other,
            })
        }
        fn cancel(&self) {
            self.0.cancel();
        }
        fn deny(&self, id: &str) {
            self.0.decide(id, Verdict::Deny);
        }
    }
    iyakki_story_kit::follow_run(&Feed(h), stopped, on_text, on_thinking)
}
