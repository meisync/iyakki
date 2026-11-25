//! The pure helpers of TALES that are not data: the request messages, the audio pieces (levels, assembling, splitting, the prebuilt
//! synth, rendering a whole story) and the HTML export. Base layer, on the story and voice leaf crates only. `iyakki_shared::story`
//! re-exports every item under the old path (alpha.10 Q3, D-P6-03).

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use iyakki_chat_model::Message;
pub use iyakki_k8s_model::b64::base64_encode;
use iyakki_story_model::*;
use iyakki_voice_model::voice;

pub mod follow;
pub mod library;
pub mod speech;
pub mod steer;
pub mod storyillus;

pub use follow::*;
pub use iyakki_story_model::*;
pub use library::*;
pub use speech::*;
pub use steer::*;

/// The whole request to the model: validated options, a system message and a user message. No tools are ever passed with it.
pub fn messages(o: &StoryOptions) -> Result<Vec<Message>, String> {
    let o = o.validated()?;
    Ok(vec![Message::system(system_prompt(&o)), Message::user(user_prompt(&o))])
}

// ------------------------------------------------------------------------------------------------------------ audio helpers

/// How loud each 20 ms of a WAV is, 0 to 1 (for the moving level display), and how long it plays in seconds.
pub fn envelope(wav: &[u8]) -> (Vec<f32>, f32) {
    let Ok(w) = voice::parse_wav(wav) else { return (Vec::new(), 0.0) };
    let ch = w.channels.max(1) as usize;
    let frame = ((w.rate as usize) * 20 / 1000).max(1) * ch;
    let secs = w.samples.len() as f32 / (w.rate.max(1) as f32 * ch as f32);
    let env = w
        .samples
        .chunks(frame)
        .map(|f| {
            let ms = f.iter().map(|s| (*s as f32 / 32768.0).powi(2)).sum::<f32>() / f.len() as f32;
            (ms.sqrt() * 5.0).min(1.0)
        })
        .collect();
    (env, secs)
}

/// The silence at the start and at the end of a sound, ms, from its [`envelope`] (20 ms per value) and its length: engines pad
/// their sounds, and the words are timed over the speech between the paddings. (0, 0) when the sound has no clear speech.
pub fn speech_edges(env: &[f32], secs: f32) -> (f32, f32) {
    const QUIET: f32 = 0.03;
    const FRAME_MS: f32 = 20.0;
    let total = secs * 1000.0;
    let (Some(a), Some(b)) = (env.iter().position(|e| *e > QUIET), env.iter().rposition(|e| *e > QUIET)) else { return (0.0, 0.0) };
    if total <= 0.0 || b <= a {
        return (0.0, 0.0);
    }
    // a word begins a little before its first loud frame
    let lead = (a as f32 * FRAME_MS - FRAME_MS).max(0.0);
    let trail = (total - (b + 1) as f32 * FRAME_MS).max(0.0);
    let cap = total * 0.35;
    (lead.min(cap), trail.min(cap))
}

/// Join the WAV pieces into one file with a short silence between sentences and a longer one between paragraphs.
pub fn assemble_audio(chunks: &[Chunk], wavs: &[Arc<Vec<u8>>]) -> Result<Vec<u8>, String> {
    assemble_audio_timed(chunks, wavs).map(|(w, _)| w)
}

/// [`assemble_audio`] and where each piece starts in the joined file, in seconds.
pub fn assemble_audio_timed(chunks: &[Chunk], wavs: &[Arc<Vec<u8>>]) -> Result<(Vec<u8>, Vec<f32>), String> {
    if wavs.is_empty() || wavs.len() != chunks.len() {
        return Err("the audio is not complete yet".into());
    }
    let mut rate = 0;
    let mut pcm: Vec<i16> = Vec::new();
    let mut starts = Vec::with_capacity(chunks.len());
    for (i, w) in wavs.iter().enumerate() {
        let p = voice::parse_wav(w).map_err(|e| format!("piece {} of the audio: {e}", i + 1))?;
        if rate == 0 {
            rate = p.rate;
        } else if rate != p.rate {
            return Err(format!("the engine changed its sample rate in the middle ({rate} Hz then {} Hz)", p.rate));
        }
        if i > 0 {
            let gap_ms = if chunks[i].unit != chunks[i - 1].unit { 420 } else { 140 };
            pcm.extend(std::iter::repeat(0i16).take(rate as usize * gap_ms / 1000));
        }
        starts.push(pcm.len() as f32 / rate.max(1) as f32);
        let ch = p.channels.max(1) as usize;
        if ch == 1 {
            pcm.extend_from_slice(&p.samples);
        } else {
            pcm.extend(p.samples.chunks(ch).map(|f| (f.iter().map(|s| *s as i32).sum::<i32>() / ch as i32) as i16));
        }
    }
    Ok((voice::encode_wav(&pcm, rate), starts))
}

/// Cut a joined WAV back into its pieces using the start times saved beside it.
pub fn split_audio(wav: &[u8], starts: &[f32]) -> Result<Vec<Vec<u8>>, String> {
    let w = voice::parse_wav(wav)?;
    if w.channels != 1 {
        return Err("the saved audio is not mono".into());
    }
    let at = |t: f32| ((t * w.rate as f32) as usize).min(w.samples.len());
    let mut out = Vec::with_capacity(starts.len());
    for (i, t) in starts.iter().enumerate() {
        let a = at(*t);
        let mut b = starts.get(i + 1).map_or(w.samples.len(), |n| at(*n));
        // the silence that was put between pieces is not part of this one
        while b > a && w.samples[b - 1] == 0 {
            b -= 1;
        }
        out.push(voice::encode_wav(&w.samples[a..b.max(a)], w.rate));
    }
    Ok(out)
}

/// Serves sound that already exists (a saved story's audio) instead of asking an engine.
pub struct PrebuiltSynth {
    pub pieces: std::collections::HashMap<String, Vec<u8>>,
}

impl PrebuiltSynth {
    pub fn new(pieces: &[(Chunk, Vec<u8>)]) -> PrebuiltSynth {
        PrebuiltSynth { pieces: pieces.iter().map(|(c, w)| (c.text.clone(), w.clone())).collect() }
    }
}

impl Synth for PrebuiltSynth {
    fn synth(&self, text: &str) -> Result<Vec<u8>, String> {
        self.pieces.get(text).cloned().ok_or_else(|| "that part is not in the saved audio".to_string())
    }
}


/// Make the whole story's sound for saving, using what is already made. `done` counts the pieces that are ready; setting
/// `cancel` ends it early.
pub fn render_audio(chunks: &[Chunk], have: &[Option<Arc<Vec<u8>>>], synth: &dyn Synth, cancel: &AtomicBool, done: &AtomicUsize) -> Result<(Vec<u8>, Vec<f32>), String> {
    let mut all = Vec::with_capacity(chunks.len());
    for (i, c) in chunks.iter().enumerate() {
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        let w = match have.get(i).cloned().flatten() {
            Some(w) => w,
            None => Arc::new(synth.synth(&c.text).map_err(|e| format!("part {} of {}: {e}", i + 1, chunks.len()))?),
        };
        all.push(w);
        done.store(i + 1, Ordering::SeqCst);
    }
    assemble_audio_timed(chunks, &all)
}

pub fn mime_of(ext: &str) -> &'static str {
    match ext {
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

pub fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}



/// The whole story as one HTML page that needs nothing else: the pictures are inside it as data addresses, there is no script
/// and no link to anything outside.
pub fn export_html(story: &Story, o: &StoryOptions, pics: &[StoredPic]) -> String {
    export_html_with(story, o, pics, false)
}

/// [`export_html`], and with `all_variants` every other picture of a scene too, each under its style name.
pub fn export_html_with(story: &Story, o: &StoryOptions, pics: &[StoredPic], all_variants: bool) -> String {
    let base = story.unit_base();
    let title = if story.title.is_empty() { title_from_topic(&o.topic) } else { story.title.clone() };
    let mut body = String::new();
    for (i, b) in story.blocks.iter().enumerate() {
        for p in pics.iter().filter(|p| p.unit == base + i) {
            if matches!(p.ext.as_str(), "mp4" | "webm") {
                body.push_str(&format!("<figure><video muted loop controls playsinline src=\"data:{};base64,{}\"></video></figure>\n", mime_of(&p.ext), base64_encode(&p.bytes)));
            } else {
                body.push_str(&format!("<figure><img alt=\"{}\" src=\"data:{};base64,{}\"></figure>\n", html_escape(&p.title), mime_of(&p.ext), base64_encode(&p.bytes)));
            }
            if all_variants {
                for v in &p.variants {
                    body.push_str(&format!("<figure><img alt=\"{t}\" src=\"data:{};base64,{}\"><figcaption>{}</figcaption></figure>\n", mime_of(&v.ext), base64_encode(&v.bytes), html_escape(&v.style), t = html_escape(&p.title)));
                }
            }
        }
        match b {
            Block::Heading(h) => body.push_str(&format!("<h2>{}</h2>\n", html_escape(h))),
            Block::Para(p) => body.push_str(&format!("<p>{}</p>\n", html_escape(p))),
        }
    }
    format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{t}</title>\n<style>\nbody{{margin:0;background:#10142a;color:#e7ebf7;font:18px/1.7 Georgia,serif}}\nheader{{padding:56px 24px 40px;text-align:center;background:linear-gradient(135deg,#3a2878,#fa8c78)}}\nh1{{margin:0;font-size:2.2em}}\n.meta{{opacity:.85;font:14px sans-serif;margin-top:10px}}\narticle{{max-width:720px;margin:0 auto;padding:24px}}\nh2{{color:#ffdc82;margin-top:2em}}\nfigure{{margin:1.5em 0}}\nimg,video{{width:100%;border-radius:14px;background:#1a2040}}\n</style>\n</head>\n<body>\n<header><h1>{t}</h1><div class=\"meta\">{a} · {g} · {l}</div></header>\n<article>\n{body}</article>\n</body>\n</html>\n",
        t = html_escape(&title),
        a = o.audience.label(),
        g = o.genre.label(),
        l = html_escape(&o.language),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(text: &str) -> Chunk {
        Chunk { unit: 0, start: 0, end: text.chars().count(), text: text.into() }
    }

    fn wav(n: usize) -> Vec<u8> {
        voice::encode_wav(&vec![1000i16; n], voice::SAMPLE_RATE)
    }

    #[test]
    fn a_prebuilt_synth_serves_saved_pieces_and_refuses_others() {
        let c = chunk("hello");
        let s = PrebuiltSynth::new(&[(c.clone(), wav(160))]);
        assert_eq!(s.synth("hello").unwrap(), wav(160));
        assert!(s.synth("other").is_err());
    }

    #[test]
    fn rendering_uses_what_is_made_asks_the_synth_for_the_rest_and_counts() {
        let chunks = vec![chunk("one"), chunk("two")];
        let synth = PrebuiltSynth::new(&[(chunks[1].clone(), wav(320))]);
        let have = vec![Some(Arc::new(wav(160))), None];
        let (cancel, done) = (AtomicBool::new(false), AtomicUsize::new(0));
        let (out, starts) = render_audio(&chunks, &have, &synth, &cancel, &done).unwrap();
        assert!(!out.is_empty());
        assert_eq!((done.load(Ordering::SeqCst), starts.len()), (2, 2));
        cancel.store(true, Ordering::SeqCst);
        assert_eq!(render_audio(&chunks, &have, &synth, &cancel, &done).unwrap_err(), "cancelled");
        // a piece nobody can make names its place
        let missing = vec![chunk("zzz")];
        let (c2, d2) = (AtomicBool::new(false), AtomicUsize::new(0));
        assert!(render_audio(&missing, &[None], &synth, &c2, &d2).unwrap_err().starts_with("part 1 of 1"));
    }

    #[test]
    fn a_bad_wav_has_no_level_and_a_title_comes_from_the_first_line() {
        assert_eq!(envelope(b"nope"), (Vec::new(), 0.0));
        assert_eq!(title_from_topic("a short idea"), "a short idea");
        assert!(title_from_topic(&"word ".repeat(40)).ends_with('…'));
    }
}
