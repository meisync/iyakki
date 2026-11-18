//! Speech for the data generator. `Tts` is the trait a real engine plugs into (a command such as a local speech tool, see
//! `CommandTts`). `FormantTts` is a small pure-Rust source-filter synthesiser (glottal pulses and noise through formant resonators).
//! It makes speech-LIKE sound with the right rough spectral shape and timing, NOT natural speech: a model trained only on it has not
//! heard a real voice. It exists so the whole pipeline (generate, train, evaluate, detect) runs end to end on a machine with no
//! speech engine and no microphone.

use crate::features::SAMPLE_RATE;
use crate::rng::Rng;
use std::f32::consts::PI;

/// A source of synthetic speech.
pub trait Tts {
    fn name(&self) -> String;
    /// How many different voices `synth` accepts (`voice` is `0..voices()`).
    fn voices(&self) -> usize;
    /// True only for engines that make real-sounding speech. The formant stub says false, and the tools print it.
    fn is_real_speech(&self) -> bool;
    /// 16 kHz mono samples for `text`; `speed` 1.0 is normal, 1.2 faster.
    fn synth(&self, text: &str, voice: usize, speed: f32) -> Result<Vec<f32>, String>;
}

// ---------------------------------------------------------------------------------------------------------- phonemes

#[derive(Clone, Copy)]
enum Cls {
    Vowel,
    Nasal,
    Glide,
    Fric { voiced: bool },
    Stop { voiced: bool },
}

#[derive(Clone, Copy)]
struct Ph {
    cls: Cls,
    ms: f32,
    /// Formants at the start and the end of the segment (Hz); noise band centre for consonants (in `f[2]` of the first).
    f0s: [f32; 3],
    f1s: [f32; 3],
    /// Noise band centre and width (Hz).
    nz: (f32, f32),
}

fn v(ms: f32, a: [f32; 3], b: [f32; 3]) -> Ph {
    Ph { cls: Cls::Vowel, ms, f0s: a, f1s: b, nz: (0.0, 0.0) }
}

/// The phoneme codes: i ee, I ih, e eh, a ah, A ae, o oh, u oo, U uh, @ schwa, Y as in "my", W as in "now", O as in "boy";
/// consonants m n N l r w j h f v s z S T p b t d k g C J.
fn phoneme(c: char) -> Option<Ph> {
    let i = [280.0, 2250.0, 2900.0];
    let ih = [400.0, 1920.0, 2560.0];
    let e = [530.0, 1840.0, 2480.0];
    let a = [730.0, 1090.0, 2440.0];
    let ae = [660.0, 1720.0, 2410.0];
    let o = [500.0, 700.0, 2400.0];
    let u = [310.0, 870.0, 2250.0];
    let uh = [640.0, 1190.0, 2390.0];
    let sc = [500.0, 1500.0, 2500.0];
    let g = |cls, ms, f: [f32; 3], nz| Ph { cls, ms, f0s: f, f1s: f, nz };
    Some(match c {
        'i' => v(110.0, i, i),
        'I' => v(80.0, ih, ih),
        'e' => v(95.0, e, e),
        'a' => v(130.0, a, a),
        'A' => v(120.0, ae, ae),
        'o' => v(120.0, o, u),
        'u' => v(120.0, u, u),
        'U' => v(90.0, uh, uh),
        '@' => v(60.0, sc, sc),
        'Y' => v(190.0, a, ih),
        'W' => v(190.0, a, u),
        'O' => v(190.0, o, ih),
        'm' => g(Cls::Nasal, 70.0, [250.0, 1100.0, 2200.0], (0.0, 0.0)),
        'n' => g(Cls::Nasal, 65.0, [250.0, 1500.0, 2500.0], (0.0, 0.0)),
        'N' => g(Cls::Nasal, 70.0, [250.0, 1300.0, 2400.0], (0.0, 0.0)),
        'l' => g(Cls::Glide, 65.0, [360.0, 1300.0, 2700.0], (0.0, 0.0)),
        'r' => g(Cls::Glide, 70.0, [420.0, 1300.0, 1600.0], (0.0, 0.0)),
        'w' => g(Cls::Glide, 60.0, [300.0, 700.0, 2300.0], (0.0, 0.0)),
        'j' => g(Cls::Glide, 55.0, [260.0, 2200.0, 3000.0], (0.0, 0.0)),
        'h' => g(Cls::Fric { voiced: false }, 70.0, sc, (1500.0, 2500.0)),
        'f' => g(Cls::Fric { voiced: false }, 90.0, sc, (5000.0, 4000.0)),
        'v' => g(Cls::Fric { voiced: true }, 70.0, sc, (4500.0, 3500.0)),
        's' => g(Cls::Fric { voiced: false }, 110.0, sc, (6500.0, 2500.0)),
        'z' => g(Cls::Fric { voiced: true }, 90.0, sc, (6200.0, 2500.0)),
        'S' => g(Cls::Fric { voiced: false }, 110.0, sc, (3800.0, 2200.0)),
        'T' => g(Cls::Fric { voiced: false }, 80.0, sc, (5500.0, 4500.0)),
        'p' => g(Cls::Stop { voiced: false }, 85.0, sc, (1200.0, 1800.0)),
        'b' => g(Cls::Stop { voiced: true }, 70.0, sc, (1000.0, 1500.0)),
        't' => g(Cls::Stop { voiced: false }, 80.0, sc, (4500.0, 2500.0)),
        'd' => g(Cls::Stop { voiced: true }, 65.0, sc, (4000.0, 2500.0)),
        'k' => g(Cls::Stop { voiced: false }, 95.0, sc, (2200.0, 1500.0)),
        'g' => g(Cls::Stop { voiced: true }, 75.0, sc, (1900.0, 1400.0)),
        'C' => g(Cls::Stop { voiced: false }, 105.0, sc, (3800.0, 2000.0)),
        'J' => g(Cls::Stop { voiced: true }, 90.0, sc, (3500.0, 2000.0)),
        _ => return None,
    })
}

const WORDS: &[(&str, &str)] = &[
    ("my", "mY"), ("iyakki", "ijaki"), ("yaki", "jaki"), ("hey", "hei"), ("ikki", "iki"), ("hi", "hY"), ("taxi", "tAksi"),
    ("sticky", "stIki"), ("icky", "Iki"), ("lucky", "lUki"), ("yucky", "jUki"), ("mighty", "mYti"), ("mickey", "mIki"),
    ("kiki", "kiki"), ("yakitori", "jakitori"), ("hockey", "haki"), ("a", "@"), ("the", "d@"), ("and", "And"), ("to", "tu"),
    ("of", "@v"), ("is", "Iz"), ("in", "In"), ("it", "It"), ("you", "ju"), ("that", "dAt"), ("he", "hi"), ("was", "wUz"),
    ("for", "for"), ("on", "an"), ("are", "ar"), ("with", "wIT"), ("as", "Az"), ("his", "hIz"), ("they", "dei"), ("be", "bi"),
    ("at", "At"), ("one", "wUn"), ("have", "hAv"), ("this", "dIs"), ("from", "frUm"), ("or", "or"), ("had", "hAd"), ("by", "bY"),
    ("what", "wUt"), ("we", "wi"), ("all", "ol"), ("your", "jor"), ("when", "wen"), ("can", "kAn"), ("said", "sed"), ("there", "der"),
    ("each", "iC"), ("which", "wIC"), ("she", "Si"), ("do", "du"), ("how", "hW"), ("their", "der"), ("if", "If"), ("will", "wIl"),
    ("up", "Up"), ("other", "Ud@r"), ("about", "@bWt"), ("out", "Wt"), ("many", "meni"), ("then", "den"), ("them", "dem"),
    ("these", "diz"), ("so", "so"), ("some", "sUm"), ("her", "hUr"), ("would", "wUd"), ("make", "mek"), ("like", "lYk"),
    ("him", "hIm"), ("into", "Intu"), ("time", "tYm"), ("has", "hAz"), ("look", "luk"), ("two", "tu"), ("more", "mor"),
    ("write", "rYt"), ("go", "go"), ("see", "si"), ("number", "nUmb@r"), ("no", "no"), ("way", "we"), ("could", "kud"),
    ("people", "pipl"), ("than", "dAn"), ("first", "fUrst"), ("water", "wot@r"), ("been", "bIn"), ("call", "kol"),
    ("who", "hu"), ("oil", "Ol"), ("its", "Its"), ("now", "nW"), ("find", "fYnd"), ("long", "loN"), ("down", "dWn"),
    ("day", "de"), ("did", "dId"), ("get", "get"), ("come", "kUm"), ("made", "med"), ("may", "me"), ("part", "part"),
    ("play", "ple"), ("music", "mjuzIk"), ("weather", "wed@r"), ("news", "njuz"), ("tonight", "tunYt"), ("home", "hom"),
    ("kitchen", "kIC@n"), ("lights", "lYts"), ("turn", "tUrn"), ("open", "op@n"), ("door", "dor"), ("today", "tude"),
];

/// Letters to phoneme codes for words the little dictionary does not hold (rough on purpose).
fn letters_to_sound(w: &str) -> String {
    let c: Vec<char> = w.chars().filter(|c| c.is_ascii_alphabetic()).map(|c| c.to_ascii_lowercase()).collect();
    let mut out = String::new();
    let mut i = 0;
    while i < c.len() {
        let two: String = c[i..(i + 2).min(c.len())].iter().collect();
        match two.as_str() {
            "th" => { out.push('T'); i += 2; continue }
            "sh" => { out.push('S'); i += 2; continue }
            "ch" => { out.push('C'); i += 2; continue }
            "ee" | "ea" => { out.push('i'); i += 2; continue }
            "oo" => { out.push('u'); i += 2; continue }
            "ng" => { out.push('N'); i += 2; continue }
            "ck" => { out.push('k'); i += 2; continue }
            _ => {}
        }
        match c[i] {
            'a' => out.push('A'),
            'e' => if i + 1 == c.len() && c.len() > 2 { } else { out.push('e') },
            'i' => out.push('I'),
            'o' => out.push('o'),
            'u' => out.push('U'),
            'y' => out.push(if i == 0 { 'j' } else { 'i' }),
            'c' | 'q' => out.push('k'),
            'x' => out.push_str("ks"),
            ch if "mnlrwhfvszpbtdkgj".contains(ch) => out.push(ch),
            _ => {}
        }
        i += 1;
    }
    out
}

/// Text to phoneme codes, with `|` for a word gap.
pub fn to_phonemes(text: &str) -> String {
    let mut out = String::new();
    for w in text.split(|c: char| !c.is_ascii_alphabetic()).filter(|w| !w.is_empty()) {
        let lw = w.to_ascii_lowercase();
        if !out.is_empty() {
            out.push('|');
        }
        match WORDS.iter().find(|(k, _)| *k == lw) {
            Some((_, p)) => out.push_str(p),
            None => out.push_str(&letters_to_sound(&lw)),
        }
    }
    out
}

// ------------------------------------------------------------------------------------------------------------- voices

#[derive(Clone, Copy, Debug)]
pub struct Voice {
    pub f0: f32,
    pub scale: f32,
    pub breath: f32,
    pub jitter: f32,
}

/// The `i`th voice of a fixed set (deterministic): low to high pitched, with the vocal tract length moving with it.
pub fn voice(i: usize) -> Voice {
    let mut r = Rng::new(0x5EED_0000 + i as u64);
    let f0 = r.range(85.0, 240.0);
    let scale = (0.86 + (f0 - 85.0) / 155.0 * 0.3 + r.range(-0.04, 0.04)).clamp(0.82, 1.25);
    Voice { f0, scale, breath: r.range(0.01, 0.12), jitter: r.range(0.002, 0.02) }
}

struct Res {
    a: f32,
    b: f32,
    c: f32,
    y1: f32,
    y2: f32,
}

impl Res {
    fn new() -> Res {
        Res { a: 1.0, b: 0.0, c: 0.0, y1: 0.0, y2: 0.0 }
    }
    fn set(&mut self, f: f32, bw: f32) {
        let f = f.clamp(60.0, SAMPLE_RATE as f32 / 2.0 - 200.0);
        let r = (-PI * bw / SAMPLE_RATE as f32).exp();
        self.b = 2.0 * r * (2.0 * PI * f / SAMPLE_RATE as f32).cos();
        self.c = -r * r;
        self.a = 1.0 - self.b - self.c;
    }
    fn run(&mut self, x: f32) -> f32 {
        let y = self.a * x + self.b * self.y1 + self.c * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Per-sample control targets for the segments of one utterance.
struct Target {
    n: usize,
    amp_v: f32,
    amp_n: f32,
    f: [f32; 3],
    f_end: [f32; 3],
    nz: (f32, f32),
    stress: f32,
}

fn targets(ph: &str, speed: f32, rng: &mut Rng) -> Vec<Target> {
    let sr = SAMPLE_RATE as f32 / 1000.0;
    let mut out = Vec::new();
    let sp = 1.0 / speed.clamp(0.5, 2.0);
    let quiet = |ms: f32| Target { n: (ms * sp * sr) as usize, amp_v: 0.0, amp_n: 0.0, f: [500.0, 1500.0, 2500.0], f_end: [500.0, 1500.0, 2500.0], nz: (3000.0, 2000.0), stress: 1.0 };
    out.push(quiet(60.0));
    for c in ph.chars() {
        if c == '|' {
            out.push(quiet(rng.range(25.0, 70.0)));
            continue;
        }
        let Some(p) = phoneme(c) else { continue };
        let ms = p.ms * sp * rng.range(0.9, 1.12);
        let stress = rng.range(0.94, 1.1);
        match p.cls {
            Cls::Vowel => out.push(Target { n: (ms * sr) as usize, amp_v: 1.0, amp_n: 0.02, f: p.f0s, f_end: p.f1s, nz: (2000.0, 2000.0), stress }),
            Cls::Glide => out.push(Target { n: (ms * sr) as usize, amp_v: 0.7, amp_n: 0.02, f: p.f0s, f_end: p.f1s, nz: (2000.0, 2000.0), stress }),
            Cls::Nasal => out.push(Target { n: (ms * sr) as usize, amp_v: 0.45, amp_n: 0.0, f: p.f0s, f_end: p.f1s, nz: (2000.0, 2000.0), stress }),
            Cls::Fric { voiced } => out.push(Target { n: (ms * sr) as usize, amp_v: if voiced { 0.3 } else { 0.0 }, amp_n: 0.55, f: p.f0s, f_end: p.f1s, nz: p.nz, stress }),
            Cls::Stop { voiced } => {
                let closure = Target { n: (ms * 0.55 * sr) as usize, amp_v: if voiced { 0.18 } else { 0.0 }, amp_n: 0.0, f: [250.0, 1200.0, 2400.0], f_end: [250.0, 1200.0, 2400.0], nz: p.nz, stress };
                let burst = Target { n: (ms * 0.12 * sr) as usize, amp_v: 0.0, amp_n: 1.0, f: p.f0s, f_end: p.f1s, nz: p.nz, stress };
                let asp = Target { n: (ms * if voiced { 0.15 } else { 0.35 } * sr) as usize, amp_v: if voiced { 0.4 } else { 0.0 }, amp_n: if voiced { 0.05 } else { 0.3 }, f: p.f0s, f_end: p.f1s, nz: (2500.0, 3000.0), stress };
                out.push(closure);
                out.push(burst);
                out.push(asp);
            }
        }
    }
    out.push(quiet(120.0));
    out
}

/// Synthesise phoneme codes.
pub fn render(ph: &str, vo: Voice, speed: f32, rng: &mut Rng) -> Vec<f32> {
    let tg = targets(ph, speed, rng);
    let total: usize = tg.iter().map(|t| t.n).sum();
    let mut out = Vec::with_capacity(total);
    let mut res = [Res::new(), Res::new(), Res::new(), Res::new(), Res::new()];
    let mut nz_res = Res::new();
    let (mut phase, mut s_amp_v, mut s_amp_n) = (0.0f32, 0.0f32, 0.0f32);
    let mut s_f = [500.0f32, 1500.0, 2500.0];
    let (mut s_nzf, mut s_nzb) = (3000.0f32, 2000.0f32);
    let sm = 1.0 - (-1.0 / (0.012 * SAMPLE_RATE as f32)).exp();
    let (mut lp1, mut lp2) = (0.0f32, 0.0f32);
    let mut done = 0usize;
    let drift: f32 = rng.range(-0.04, 0.04);
    for t in &tg {
        for k in 0..t.n {
            let x = k as f32 / t.n.max(1) as f32;
            let prog = (done + k) as f32 / total.max(1) as f32;
            let decl = 1.0 - 0.18 * prog + drift * (prog * PI).sin();
            let f0 = vo.f0 * decl * t.stress * (1.0 + vo.jitter * rng.normal());
            for j in 0..3 {
                let target = (t.f[j] + (t.f_end[j] - t.f[j]) * x) * vo.scale;
                s_f[j] += sm * (target - s_f[j]);
            }
            s_amp_v += sm * (t.amp_v - s_amp_v);
            s_amp_n += sm * (t.amp_n - s_amp_n);
            s_nzf += sm * (t.nz.0 * vo.scale - s_nzf);
            s_nzb += sm * (t.nz.1 - s_nzb);
            phase += f0 / SAMPLE_RATE as f32;
            if phase >= 1.0 {
                phase -= 1.0;
            }
            // Rosenberg-style glottal pulse, differentiated by the lip radiation below
            let g = if phase < 0.4 { 0.5 * (1.0 - (PI * phase / 0.4).cos()) } else if phase < 0.55 { (PI / 2.0 * (phase - 0.4) / 0.15).cos() } else { 0.0 };
            let src = s_amp_v * (g - lp1) * 6.0 + vo.breath * s_amp_v * rng.normal() * 0.5;
            lp1 += 0.003 * (g - lp1);
            res[0].set(s_f[0], 70.0);
            res[1].set(s_f[1], 100.0);
            res[2].set(s_f[2], 160.0);
            res[3].set(3500.0 * vo.scale, 250.0);
            res[4].set(4500.0 * vo.scale, 300.0);
            let mut y = src;
            for r in res.iter_mut() {
                y = r.run(y);
            }
            nz_res.set(s_nzf, s_nzb);
            let nzs = nz_res.run(rng.normal()) * s_amp_n * 1.2;
            let s = y * 0.35 + nzs;
            // radiation: mild high-pass
            lp2 += 0.1 * (s - lp2);
            out.push(s - 0.7 * lp2);
        }
        done += t.n;
    }
    let peak = out.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
    out.iter_mut().for_each(|v| *v = *v / peak * 0.8);
    out
}

pub struct FormantTts {
    pub seed: u64,
}

impl Tts for FormantTts {
    fn name(&self) -> String {
        "formant stub (pure Rust, NOT real speech)".into()
    }
    fn voices(&self) -> usize {
        64
    }
    fn is_real_speech(&self) -> bool {
        false
    }
    fn synth(&self, text: &str, voice_i: usize, speed: f32) -> Result<Vec<f32>, String> {
        let ph = to_phonemes(text);
        if ph.is_empty() {
            return Err("nothing to say".into());
        }
        let mut rng = Rng::new(self.seed ^ (voice_i as u64).wrapping_mul(0x9E37_79B9) ^ text.len() as u64);
        Ok(render(&ph, voice(voice_i % self.voices()), speed, &mut rng))
    }
}

/// An external speech tool run as a command, for a machine that has one (a local engine). `template` is split on spaces and may hold
/// `{text}`, `{out}` (a WAV path in `work_dir`), `{voice}` (the index) and `{speed}`. The tool must write a WAV to `{out}`. Not
/// exercised against a real engine in this repository's tests (none is installed on the build machine).
pub struct CommandTts {
    pub template: String,
    pub work_dir: std::path::PathBuf,
    pub voice_count: usize,
}

impl CommandTts {
    pub fn argv(&self, text: &str, voice: usize, speed: f32, out: &std::path::Path) -> Vec<String> {
        self.template
            .split_whitespace()
            .map(|a| a.replace("{text}", text).replace("{out}", &out.to_string_lossy()).replace("{voice}", &voice.to_string()).replace("{speed}", &format!("{speed:.2}")))
            .collect()
    }
}

impl Tts for CommandTts {
    fn name(&self) -> String {
        format!("command: {}", self.template.split_whitespace().next().unwrap_or("?"))
    }
    fn voices(&self) -> usize {
        self.voice_count.max(1)
    }
    fn is_real_speech(&self) -> bool {
        true
    }
    fn synth(&self, text: &str, voice: usize, speed: f32) -> Result<Vec<f32>, String> {
        let out = self.work_dir.join(format!("tts_{}_{}.wav", std::process::id(), voice));
        let argv = self.argv(text, voice, speed, &out);
        let (prog, args) = argv.split_first().ok_or("empty command")?;
        let st = std::process::Command::new(prog).args(args).status().map_err(|e| format!("cannot run {prog}: {e}"))?;
        if !st.success() {
            return Err(format!("{prog} failed"));
        }
        let bytes = std::fs::read(&out).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(&out);
        crate::wav::decode(&bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_phrase_has_its_pronunciation_and_unknown_words_get_a_guess() {
        assert_eq!(to_phonemes("My IYAKKI"), "mY|ijaki");
        assert_eq!(to_phonemes("hey, ikki!"), "hei|iki");
        assert!(!to_phonemes("zebra crossing").is_empty());
        assert_eq!(to_phonemes("123 !!"), "");
    }

    #[test]
    fn a_voice_is_speech_shaped_in_time_and_level() {
        let t = FormantTts { seed: 1 };
        let x = t.synth("my iyakki", 3, 1.0).unwrap();
        let secs = x.len() as f32 / SAMPLE_RATE as f32;
        assert!((0.7..2.0).contains(&secs), "{secs} s");
        assert!(x.iter().all(|v| v.is_finite() && v.abs() <= 0.81));
        let faster = t.synth("my iyakki", 3, 1.3).unwrap();
        assert!(faster.len() < x.len());
        assert!(x.iter().any(|v| v.abs() > 0.3));
        // the same request twice gives the same samples
        assert_eq!(x, t.synth("my iyakki", 3, 1.0).unwrap());
        assert!(!t.is_real_speech());
    }

    #[test]
    fn a_vowel_puts_its_energy_near_its_formants() {
        let mut r = Rng::new(1);
        let x = render("i", Voice { f0: 120.0, scale: 1.0, breath: 0.02, jitter: 0.0 }, 1.0, &mut r);
        let aa = render("a", Voice { f0: 120.0, scale: 1.0, breath: 0.02, jitter: 0.0 }, 1.0, &mut r);
        let band = |s: &[f32], lo: f32, hi: f32| -> f32 {
            let n = 2048;
            let mid = s.len() / 2 - n / 2;
            let f = crate::fft::Fft::new(n);
            let (mut re, mut im) = (s[mid..mid + n].to_vec(), vec![0.0; n]);
            f.forward(&mut re, &mut im);
            (0..n / 2).filter(|k| (*k as f32 * 16000.0 / n as f32) >= lo && (*k as f32 * 16000.0 / n as f32) < hi).map(|k| re[k] * re[k] + im[k] * im[k]).sum()
        };
        // "ee" has a strong second formant near 2.2 kHz; "ah" has more around 1 kHz than "ee"
        assert!(band(&x, 2000.0, 2500.0) > band(&x, 1000.0, 1500.0));
        assert!(band(&aa, 900.0, 1300.0) > band(&x, 900.0, 1300.0));
    }

    #[test]
    fn voices_differ_and_the_command_template_expands() {
        assert!(voice(0).f0 != voice(1).f0 && voice(5).f0 == voice(5).f0);
        let c = CommandTts { template: "speak -v {voice} -s {speed} -w {out} {text}".into(), work_dir: "w".into(), voice_count: 2 };
        let a = c.argv("hello", 1, 1.0, std::path::Path::new("w/o.wav"));
        assert_eq!(a, vec!["speak", "-v", "1", "-s", "1.00", "-w", "w/o.wav", "hello"]);
        assert!(c.is_real_speech());
    }
}
