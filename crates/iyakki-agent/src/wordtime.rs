//! Which word is being said right now: the timing model behind the word highlight of reading aloud (TALES, chat).
//!
//! Three sources of timing, best first:
//!
//! 1. **Exact**: the browser voice reports `boundary` events (the character where each word starts). [`WordClock::boundary`]
//!    takes them; the word shown is the word the voice reported.
//! 2. **Fitted estimate**: the engine made a sound whose length is known (a WAV). [`WordTimeline::fitted`] spreads the words
//!    over that length with a pronunciation-weighted model (syllable-like weights, pauses at punctuation).
//! 3. **Rate estimate**: nothing is known about the length (a play-only engine such as `spd-say`, or a browser voice that
//!    fires no boundary events). [`WordTimeline::estimate`] uses a words-per-minute style model for the speed.
//!
//! Everything here is pure: time is passed in, nothing sleeps, nothing is drawn. Characters are `char` offsets into the
//! text given to the timeline, which is the same text the page lays out, so a word range can be highlighted from the layout
//! that draws the text (word ranges start and end at word boundaries, never inside a grapheme cluster).

/// Milliseconds of speech of one syllable-like unit at normal speed.
const UNIT_MS: f32 = 235.0;
/// Pause after a comma, semicolon or colon (milliseconds at normal speed).
const PAUSE_SOFT: f32 = 170.0;
/// Pause after a full stop, question or exclamation mark.
const PAUSE_HARD: f32 = 380.0;
/// Pause after a dash or a closing quote or bracket.
const PAUSE_LIGHT: f32 = 90.0;
/// After this long without a new boundary event, an exact clock follows its own estimate again.
pub const STALL_MS: f32 = 900.0;
/// Start latency of a speaker process (milliseconds between starting it and hearing the first sound).
pub const START_LATENCY_MS: f32 = 90.0;
/// A heard position that has not moved for this long (ms) means the sound is starved: the clock stops with it.
pub const HEARD_STILL_MS: f32 = 60.0;
/// A step back of the heard position smaller than this (ms) while the sound runs is jitter of the estimate, not a rewind.
pub const JITTER_BACK_MS: f32 = 25.0;
/// Loudness (0 to 1, see `story::envelope`) at or below which a 20 ms frame counts as silence when looking for pauses.
pub const PIN_QUIET: f32 = 0.03;
/// The shortest silence inside speech that is taken for a pause of the punctuation.
pub const PIN_MIN_MS: f32 = 60.0;
/// Milliseconds per value of the loudness list.
pub const ENV_FRAME_MS: f32 = 20.0;
/// The most words the highlight moves ahead in one correction (a longer jump is taken in steps by the next polls).
pub const MAX_LEAP_WORDS: usize = 4;

/// What [`WordTimeline::fitted_pinned`] did, for the log line of a sentence.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FitReport {
    /// Real speech length / the model's length for the same words (1.0: the model fits as it is).
    pub stretch: f32,
    /// Silences of the sound matched to punctuation.
    pub pins: usize,
    /// The largest difference between the plain fitted time of a pinned boundary and its pinned time (ms).
    pub worst_boundary_error_ms: f32,
}

/// One word: its characters `start..end` in the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WordSpan {
    pub start: usize,
    pub end: usize,
}

/// The script of a character, as far as timing cares.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Script {
    Latin,
    Indic,
    Cjk,
    Kana,
    Arabic,
    Other,
}

fn is_indic(c: char) -> bool {
    ('\u{0900}'..='\u{0DFF}').contains(&c)
}

fn is_kana(c: char) -> bool {
    ('\u{3040}'..='\u{30FF}').contains(&c)
}

fn is_han_or_hangul(c: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&c) || ('\u{3400}'..='\u{4DBF}').contains(&c) || ('\u{AC00}'..='\u{D7AF}').contains(&c) || ('\u{F900}'..='\u{FAFF}').contains(&c)
}

fn is_arabic(c: char) -> bool {
    ('\u{0600}'..='\u{06FF}').contains(&c) || ('\u{0750}'..='\u{077F}').contains(&c) || ('\u{0590}'..='\u{05FF}').contains(&c)
}

/// Characters that attach to the letter before them without being a letter themselves (combining marks, joiners).
fn is_attached(c: char) -> bool {
    ('\u{0300}'..='\u{036F}').contains(&c) || ('\u{064B}'..='\u{065F}').contains(&c) || ('\u{0591}'..='\u{05C7}').contains(&c) || c == '\u{200C}' || c == '\u{200D}' || c == '\u{FE0F}'
}

fn script_of(c: char) -> Script {
    if is_indic(c) {
        Script::Indic
    } else if is_kana(c) {
        Script::Kana
    } else if is_han_or_hangul(c) {
        Script::Cjk
    } else if is_arabic(c) {
        Script::Arabic
    } else if c.is_alphabetic() && (c as u32) < 0x250 {
        Script::Latin
    } else {
        Script::Other
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || (is_indic(c) && !matches!(c, '\u{0964}' | '\u{0965}' | '\u{0970}')) || is_attached(c)
}

/// Is `c` a letter that is its own word (Chinese, Japanese, Korean text has no spaces)?
fn is_solo(c: char) -> bool {
    is_han_or_hangul(c) && !('\u{AC00}'..='\u{D7AF}').contains(&c) || is_kana(c)
}

/// The words of `text` as character ranges. Words are runs of letters and digits (with an inner apostrophe, hyphen, or the
/// dot or comma of a number); every Chinese or Japanese character is a word of its own, Korean is split at spaces.
pub fn words_of(text: &str) -> Vec<WordSpan> {
    let c: Vec<char> = text.chars().collect();
    let n = c.len();
    let mut out = Vec::new();
    let mut i = 0;
    while i < n {
        if !is_word_char(c[i]) || (is_attached(c[i]) && !c[i].is_alphanumeric()) {
            i += 1;
            continue;
        }
        if is_solo(c[i]) {
            let mut j = i + 1;
            while j < n && is_attached(c[j]) {
                j += 1;
            }
            out.push(WordSpan { start: i, end: j });
            i = j;
            continue;
        }
        let start = i;
        let mut j = i + 1;
        while j < n {
            if is_solo(c[j]) {
                break;
            }
            if is_word_char(c[j]) {
                j += 1;
                continue;
            }
            // an inner mark: don't, it's, 3.14, 1,000, well-known
            let inner = j + 1 < n && is_word_char(c[j + 1]) && !is_solo(c[j + 1]);
            let ok = match c[j] {
                '\'' | '\u{2019}' | '-' | '\u{2010}' => c[j - 1].is_alphanumeric() && inner,
                '.' | ',' => c[j - 1].is_ascii_digit() && inner && c[j + 1].is_ascii_digit(),
                _ => false,
            };
            if ok {
                j += 1;
            } else {
                break;
            }
        }
        out.push(WordSpan { start, end: j });
        i = j;
    }
    out
}

fn is_latin_vowel(c: char) -> bool {
    matches!(c.to_ascii_lowercase(), 'a' | 'e' | 'i' | 'o' | 'u' | 'y') || "àáâäãåæèéêëìíîïòóôöõøœùúûüýÿÀÁÂÄÃÅÆÈÉÊËÌÍÎÏÒÓÔÖÕØŒÙÚÛÜÝ".contains(c)
}

/// How long a word takes to say, in syllable-like units.
///
/// Latin: groups of vowels (a word has at least one), a little for the consonants, digits count one each. Indic scripts: one
/// unit per akshara (a consonant with its vowel sign), a consonant after a virama joins the cluster before it, a consonant
/// ending in a virama is short. Chinese and Korean: one per character, kana a little less. Arabic and Hebrew: letters are
/// consonants mostly, so a letter is a fraction of a unit. Other alphabets: a fraction per letter.
pub fn word_units(w: &str) -> f32 {
    let c: Vec<char> = w.chars().collect();
    if c.is_empty() {
        return 0.0;
    }
    // abbreviations are said in full ("Doctor"), numbers by their spoken length ("nineteen eighty-four")
    let low: String = w.to_lowercase();
    match low.as_str() {
        "dr" | "mr" | "vs" => return 2.0,
        "mrs" => return 3.0,
        "ms" | "st" | "no" => return 1.2,
        "etc" => return 4.0,
        _ => {}
    }
    if c.iter().all(|ch| ch.is_ascii_digit()) {
        const SPOKEN: [f32; 6] = [0.0, 1.2, 2.0, 4.0, 5.5, 6.5];
        return if c.len() < SPOKEN.len() { SPOKEN[c.len()] } else { 1.3 * c.len() as f32 };
    }
    let mut units = 0.0f32;
    let mut vowel_run = false;
    let mut prev_virama = false;
    for ch in c.iter().copied() {
        match script_of(ch) {
            Script::Latin => {
                if is_latin_vowel(ch) {
                    if !vowel_run {
                        units += 1.0;
                    }
                    vowel_run = true;
                } else {
                    units += 0.12;
                    vowel_run = false;
                }
            }
            Script::Indic => {
                vowel_run = false;
                let low = (ch as u32) & 0x7F;
                match low {
                    0x04..=0x14 | 0x60 | 0x61 => units += 1.0,
                    0x15..=0x39 | 0x58..=0x5F => units += if prev_virama { 0.5 } else { 1.0 },
                    0x4D => units -= 0.5,
                    0x66..=0x6F => units += 1.0,
                    _ => {}
                }
                prev_virama = low == 0x4D;
                continue;
            }
            Script::Cjk => units += 1.0,
            Script::Kana => units += 0.65,
            Script::Arabic => {
                if !is_attached(ch) {
                    units += 0.45;
                }
            }
            Script::Other => {
                if ch.is_ascii_digit() || ch.is_numeric() {
                    units += 1.0;
                } else if ch.is_alphabetic() {
                    units += 0.4;
                }
            }
        }
        prev_virama = false;
    }
    units.max(0.5)
}

/// The pause (ms at normal speed) that the characters between two words ask for.
fn pause_between(gap: &[char]) -> f32 {
    let mut p = 0.0f32;
    // a spaced hyphen is a dash
    if gap.len() >= 3 && gap.contains(&'-') && gap.iter().all(|c| c.is_whitespace() || *c == '-') {
        p = PAUSE_LIGHT;
    }
    for ch in gap {
        let q = match ch {
            '.' | '!' | '?' | '\u{2026}' | '\u{3002}' | '\u{FF01}' | '\u{FF1F}' | '\u{0964}' | '\u{0965}' | '\u{06D4}' | '\u{061F}' | '\n' => PAUSE_HARD,
            ',' | ';' | ':' | '\u{3001}' | '\u{FF0C}' | '\u{FF1B}' | '\u{FF1A}' | '\u{060C}' | '\u{061B}' => PAUSE_SOFT,
            '\u{2014}' | '\u{2013}' | ')' | ']' | '"' | '\u{201D}' | '\u{300D}' | '\u{300F}' => PAUSE_LIGHT,
            _ => 0.0,
        };
        p = p.max(q);
    }
    p
}

/// The pause after word `i` of `words`: [`pause_between`] of the characters up to the next word, except that the full stop of
/// an abbreviation ("Dr.", "Mr.", "e.g.", an initial like "J.") is no sentence end and gets none.
fn gap_after(c: &[char], words: &[WordSpan], i: usize) -> f32 {
    let w = words[i];
    let next = words.get(i + 1).map_or(c.len(), |n| n.start);
    let gap = &c[w.end.min(c.len())..next.max(w.end).min(c.len())];
    if gap.first() == Some(&'.') && i + 1 < words.len() {
        let word: String = c[w.start..w.end].iter().collect::<String>().to_lowercase();
        let abbr = matches!(word.as_str(), "dr" | "mr" | "mrs" | "ms" | "st" | "prof" | "sr" | "jr" | "vs" | "etc" | "no" | "inc" | "ltd" | "co" | "mt" | "e.g" | "i.e")
            || (word.chars().count() == 1 && c[w.start].is_uppercase());
        if abbr && gap.len() == 2 && gap[1] == ' ' && c.get(next).is_some_and(|ch| ch.is_alphanumeric()) {
            return 0.0;
        }
    }
    pause_between(gap)
}

/// Words with their start times: which word is being said at a given time.
#[derive(Clone, Debug, PartialEq)]
pub struct WordTimeline {
    words: Vec<WordSpan>,
    /// Where each word starts, in milliseconds from the start of the sound, then the end of the last word's pause.
    begin: Vec<f32>,
    fitted: bool,
}

impl WordTimeline {
    /// Timing from the speed alone (`speed` 1.0 is normal; clamped to 0.25 to 4).
    pub fn estimate(text: &str, speed: f32) -> WordTimeline {
        let speed = if speed.is_finite() { speed.clamp(0.25, 4.0) } else { 1.0 };
        Self::build(text, 1.0 / speed, None)
    }

    /// Timing spread over a sound that is `duration_ms` long.
    pub fn fitted(text: &str, duration_ms: f32) -> WordTimeline {
        if !duration_ms.is_finite() || duration_ms <= 0.0 {
            return Self::estimate(text, 1.0);
        }
        Self::build(text, 1.0, Some(duration_ms))
    }

    /// Timing spread over the speech of a sound that is `duration_ms` long and starts with `lead_ms` and ends with `trail_ms`
    /// of silence (measured from the sound, see `story::speech_edges`): the words fill the speech between them, the pause after
    /// the last word is not given any of it. Engines pad their sounds with silence, and spreading the words over the padding too
    /// makes every word late, the later ones the most. Falls back to [`WordTimeline::fitted`] when the edges leave no room.
    pub fn fitted_in(text: &str, duration_ms: f32, lead_ms: f32, trail_ms: f32) -> WordTimeline {
        let plain = Self::fitted(text, duration_ms);
        if !duration_ms.is_finite() || duration_ms <= 0.0 || plain.words.is_empty() {
            return plain;
        }
        let (lead, trail) = (lead_ms.max(0.0), trail_ms.max(0.0));
        let speech = duration_ms - lead - trail;
        if !lead.is_finite() || !trail.is_finite() || speech < duration_ms * 0.2 || (lead < 1.0 && trail < 1.0) {
            return plain;
        }
        let raw = Self::build(text, 1.0, None);
        let c: Vec<char> = text.chars().collect();
        let last_end = raw.words.last().map_or(0, |w| w.end);
        let tail = pause_between(&c[last_end.min(c.len())..]);
        let model = raw.total_ms() - tail;
        if model <= 0.0 {
            return plain;
        }
        let k = speech / model;
        let n = raw.words.len();
        let mut begin: Vec<f32> = raw.begin[..n].iter().map(|b| lead + b * k).collect();
        begin.push(duration_ms);
        WordTimeline { words: raw.words, begin, fitted: true }
    }

    /// The one builder for a sound whose PCM is known (`env`: loudness per [`ENV_FRAME_MS`], see `story::envelope`): like
    /// [`WordTimeline::fitted_in`], but the silences found inside the speech (>= [`PIN_MIN_MS`]) are pinned to the punctuation
    /// of the text (commas, dashes, full stops inside the sentence) by a monotone matching of their positions, and the words
    /// between two pins are spread over exactly the speech between them by their syllable-like weights. A pause the model
    /// puts in the wrong place, or too long or short, can then not push all later words late: the error cannot grow beyond one
    /// stretch between pins. Without usable silences it is `fitted_in`.
    pub fn fitted_pinned(text: &str, duration_ms: f32, lead_ms: f32, trail_ms: f32, env: &[f32]) -> (WordTimeline, FitReport) {
        let base = Self::fitted_in(text, duration_ms, lead_ms, trail_ms);
        let mut rep = FitReport { stretch: 1.0, ..FitReport::default() };
        let n = base.words.len();
        if n < 2 || !duration_ms.is_finite() || duration_ms <= 0.0 || env.is_empty() {
            rep.stretch = Self::stretch_vs_model(text, duration_ms - lead_ms.max(0.0) - trail_ms.max(0.0));
            return (base, rep);
        }
        let (s0, s1) = (lead_ms.max(0.0), (duration_ms - trail_ms.max(0.0)).max(0.0));
        rep.stretch = Self::stretch_vs_model(text, s1 - s0);
        if s1 - s0 < duration_ms * 0.2 {
            return (base, rep);
        }
        let c: Vec<char> = text.chars().collect();
        let raw = Self::build(text, 1.0, None);
        let last_end = raw.words[n - 1].end;
        let model_total = raw.total_ms() - pause_between(&c[last_end.min(c.len())..]);
        if model_total <= 0.0 {
            return (base, rep);
        }
        let k = (s1 - s0) / model_total;
        // silences inside the speech: (start, end) in ms
        let mut sil: Vec<(f32, f32)> = Vec::new();
        let mut run: Option<usize> = None;
        for (j, e) in env.iter().chain(std::iter::once(&1.0)).enumerate() {
            if *e <= PIN_QUIET {
                run.get_or_insert(j);
            } else if let Some(a) = run.take() {
                let (x, y) = (a as f32 * ENV_FRAME_MS, j as f32 * ENV_FRAME_MS);
                if y - x >= PIN_MIN_MS && x > s0 + ENV_FRAME_MS && y < s1 - ENV_FRAME_MS {
                    sil.push((x, y));
                }
            }
        }
        // punctuation slots: (word before, expected centre of the pause in the plain fit)
        let mut slots: Vec<(usize, f32)> = Vec::new();
        for i in 0..n - 1 {
            let p = gap_after(&c, &raw.words, i);
            if p > 0.0 {
                slots.push((i, s0 + (raw.begin[i + 1] - p / 2.0) * k));
            }
        }
        if sil.is_empty() || slots.is_empty() {
            return (base, rep);
        }
        // monotone matching, best total closeness; a match farther than the tolerance is not allowed
        let tol = 250.0f32.max((s1 - s0) * 0.2);
        let (ns, nl) = (sil.len(), slots.len());
        let mut dp = vec![vec![0.0f32; nl + 1]; ns + 1];
        for a in 1..=ns {
            for b in 1..=nl {
                let d = ((sil[a - 1].0 + sil[a - 1].1) / 2.0 - slots[b - 1].1).abs();
                let mut best = dp[a - 1][b].max(dp[a][b - 1]);
                if d <= tol {
                    best = best.max(dp[a - 1][b - 1] + (tol - d));
                }
                dp[a][b] = best;
            }
        }
        let mut pins: Vec<(usize, f32, f32)> = Vec::new();
        let (mut a, mut b) = (ns, nl);
        while a > 0 && b > 0 {
            let d = ((sil[a - 1].0 + sil[a - 1].1) / 2.0 - slots[b - 1].1).abs();
            if d <= tol && (dp[a][b] - (dp[a - 1][b - 1] + (tol - d))).abs() < 1e-3 {
                pins.push((slots[b - 1].0, sil[a - 1].0, sil[a - 1].1));
                a -= 1;
                b -= 1;
            } else if dp[a - 1][b] >= dp[a][b - 1] {
                a -= 1;
            } else {
                b -= 1;
            }
        }
        pins.reverse();
        if pins.is_empty() {
            return (base, rep);
        }
        // weight of word i alone, and of the gap after it when it is not a pin
        let unit = |i: usize| -> f32 {
            let w: String = c[raw.words[i].start..raw.words[i].end].iter().collect();
            word_units(&w) * UNIT_MS
        };
        let gap = |i: usize| -> f32 { gap_after(&c, &raw.words, i) };
        let mut begin = vec![0.0f32; n + 1];
        let mut first = 0usize;
        let mut t0 = s0;
        for pi in 0..=pins.len() {
            let (last, t1, next0) = match pins.get(pi) {
                Some((i, x, y)) => (*i, *x, (*y - 10.0).max(*x)),
                None => (n - 1, s1, s1),
            };
            let total: f32 = (first..=last).map(|i| unit(i) + if i < last { gap(i) } else { 0.0 }).sum();
            let span = (t1 - t0).max(0.0);
            let mut acc = 0.0f32;
            for i in first..=last {
                begin[i] = t0 + if total > 0.0 { acc / total * span } else { 0.0 };
                acc += unit(i) + if i < last { gap(i) } else { 0.0 };
            }
            first = last + 1;
            t0 = next0;
        }
        begin[n] = duration_ms;
        for i in 1..=n {
            if begin[i] < begin[i - 1] {
                begin[i] = begin[i - 1];
            }
        }
        for (i, _, _) in &pins {
            let e = (base.begin[i + 1] - begin[i + 1]).abs();
            rep.worst_boundary_error_ms = rep.worst_boundary_error_ms.max(e);
        }
        rep.pins = pins.len();
        (WordTimeline { words: raw.words, begin, fitted: true }, rep)
    }

    /// Speech length divided by the length the model gives the same words (1.0: the model is right; above 1: the voice is
    /// slower than the model).
    fn stretch_vs_model(text: &str, speech_ms: f32) -> f32 {
        let raw = Self::build(text, 1.0, None);
        let c: Vec<char> = text.chars().collect();
        let last_end = raw.words.last().map_or(0, |w| w.end);
        let model = raw.total_ms() - pause_between(&c[last_end.min(c.len())..]);
        if model > 0.0 && speech_ms > 0.0 {
            speech_ms / model
        } else {
            1.0
        }
    }

    /// One line per word for the developer (DEBUG log): index, text, start and end in milliseconds.
    pub fn debug_line(&self, text: &str) -> String {
        let c: Vec<char> = text.chars().collect();
        let mut s = String::new();
        for (i, w) in self.words.iter().enumerate() {
            let t: String = c[w.start.min(c.len())..w.end.min(c.len())].iter().collect();
            s.push_str(&format!("{i}:{t}@{:.0}-{:.0} ", self.begin[i], self.begin[i + 1]));
        }
        s
    }

    /// Timing measured elsewhere (the engine's own word times, or a recognition aligned to the text, see `speechtime`):
    /// `begin` holds where each word of `text` starts, in milliseconds, then the end of the sound. None when the count does
    /// not match the words of `text` or the times are not finite and rising (equal times are allowed).
    pub fn with_starts(text: &str, begin: Vec<f32>) -> Option<WordTimeline> {
        let words = words_of(text);
        if begin.len() != words.len() + 1 || begin.iter().any(|b| !b.is_finite() || *b < 0.0) || begin.windows(2).any(|w| w[1] < w[0]) {
            return None;
        }
        Some(WordTimeline { words, begin, fitted: true })
    }

    fn build(text: &str, slow: f32, fit: Option<f32>) -> WordTimeline {
        let c: Vec<char> = text.chars().collect();
        let words = words_of(text);
        let mut begin = Vec::with_capacity(words.len() + 1);
        let mut t = 0.0f32;
        for (i, w) in words.iter().enumerate() {
            begin.push(t);
            let s: String = c[w.start..w.end].iter().collect();
            t += word_units(&s) * UNIT_MS * slow;
            // closing marks directly after the word belong to it; the rest of the gap too
            t += gap_after(&c, &words, i) * slow;
        }
        begin.push(t);
        if let Some(d) = fit {
            if t > 0.0 {
                let k = d / t;
                for b in begin.iter_mut() {
                    *b *= k;
                }
            } else {
                for b in begin.iter_mut() {
                    *b = 0.0;
                }
            }
        }
        WordTimeline { words, begin, fitted: fit.is_some() }
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    pub fn words(&self) -> &[WordSpan] {
        &self.words
    }

    pub fn span(&self, i: usize) -> Option<WordSpan> {
        self.words.get(i).copied()
    }

    /// The whole sound, in milliseconds.
    pub fn total_ms(&self) -> f32 {
        self.begin.last().copied().unwrap_or(0.0)
    }

    /// Whether the timing was fitted to a measured length (otherwise it is a rate estimate).
    pub fn is_fitted(&self) -> bool {
        self.fitted
    }

    /// When word `i` starts, in milliseconds.
    pub fn start_ms(&self, i: usize) -> Option<f32> {
        (i < self.words.len()).then(|| self.begin[i])
    }

    /// When word `i` and its trailing pause end.
    pub fn end_ms(&self, i: usize) -> Option<f32> {
        (i < self.words.len()).then(|| self.begin[i + 1])
    }

    /// The word being said `ms` after the sound started: none before it starts (or without words); after the end the last
    /// word stays.
    pub fn word_at(&self, ms: f32) -> Option<usize> {
        if self.words.is_empty() || !ms.is_finite() || ms < 0.0 {
            return None;
        }
        let n = self.words.len();
        // the last word that starts at or before ms
        let k = self.begin[..n].partition_point(|b| *b <= ms);
        Some(k.saturating_sub(1).min(n - 1))
    }

    /// The characters `(start, end)` of the word being said `ms` after the start.
    pub fn range_at(&self, ms: f32) -> Option<(usize, usize)> {
        self.word_at(ms).map(|i| (self.words[i].start, self.words[i].end))
    }

    /// The word that contains character `ch`, or the next word when `ch` is between words (a voice reports where a word
    /// starts, which may be a quote or a space before it).
    pub fn word_at_char(&self, ch: usize) -> Option<usize> {
        let k = self.words.partition_point(|w| w.end <= ch);
        (k < self.words.len()).then_some(k)
    }
}

/// Map the index of a word among `from` words to the matching word among `to` words (the text sent to a voice has its
/// markup removed, so it can have fewer words than the text on the page). The same count maps one to one.
pub fn map_index(i: usize, from: usize, to: usize) -> usize {
    if to == 0 {
        return 0;
    }
    if from == to || from <= 1 {
        return i.min(to - 1);
    }
    ((i as f32 * (to - 1) as f32 / (from - 1) as f32).round() as usize).min(to - 1)
}

/// UTF-16 code unit offset (what a browser reports as `charIndex`) to a `char` offset of `text`.
pub fn utf16_to_char(text: &str, units: usize) -> usize {
    let mut u = 0;
    for (k, ch) in text.chars().enumerate() {
        if u >= units {
            return k;
        }
        u += ch.len_utf16();
    }
    text.chars().count()
}

fn norm(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric() || is_indic(*c) || is_attached(*c)).flat_map(char::to_lowercase).collect()
}

/// Match the words that were spoken with the words that were drawn: markup, link addresses and code are drawn but not
/// spoken, list markers and the like are drawn from the layout. For every spoken word the index of the drawn word it is
/// (the next drawn word with the same letters, in order), or None when it cannot be found. Both lists are in reading order.
pub fn align_words<A: AsRef<str>, B: AsRef<str>>(spoken: &[A], drawn: &[B]) -> Vec<Option<usize>> {
    let d: Vec<String> = drawn.iter().map(|w| norm(w.as_ref())).collect();
    let mut at = 0usize;
    let mut out = Vec::with_capacity(spoken.len());
    for w in spoken {
        let w = norm(w.as_ref());
        match (!w.is_empty()).then(|| d[at.min(d.len())..].iter().position(|x| *x == w)).flatten() {
            Some(k) => {
                out.push(Some(at + k));
                at += k + 1;
            }
            None => out.push(None),
        }
    }
    out
}

/// Whether the timing of a [`WordClock`] comes from the voice or from a model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimingMode {
    /// A boundary event arrived lately: the word is the one the voice reported.
    Exact,
    /// No boundary event yet (or none for a while): the word follows the model.
    Estimated,
}

/// A running (or paused) reading of one piece: turns the clock into the current word. Time is given in milliseconds of any
/// steady clock; pausing freezes it, [`WordClock::restart`] and [`WordClock::seek_word`] jump without leaving drift.
#[derive(Clone, Debug)]
pub struct WordClock {
    tl: WordTimeline,
    latency: f32,
    /// Milliseconds of sound played before the last resume.
    base: f32,
    since: Option<f32>,
    /// A boundary event moved the estimate: milliseconds to add to the raw time.
    offset: f32,
    exact: Option<usize>,
    last_boundary_at: f32,
    started: bool,
    /// When the clock was frozen (everything is judged at that moment while it is).
    frozen_at: f32,
}

impl WordClock {
    /// `latency_ms`: how long after starting the speaker the first sound is heard (the estimate starts late by that much).
    pub fn new(tl: WordTimeline, latency_ms: f32) -> WordClock {
        WordClock { tl, latency: latency_ms.max(0.0), base: 0.0, since: None, offset: 0.0, exact: None, last_boundary_at: 0.0, started: false, frozen_at: 0.0 }
    }

    pub fn timeline(&self) -> &WordTimeline {
        &self.tl
    }

    /// Better timing arrived while the piece plays (an alignment finished): the words follow it from now on. The time
    /// played is kept, so the word jumps to where the sound really is; a calibration from boundary events is dropped.
    pub fn set_timeline(&mut self, tl: WordTimeline) {
        self.tl = tl;
        self.offset = 0.0;
        self.exact = None;
    }

    /// The sound begins at `now`.
    pub fn start(&mut self, now: f32) {
        self.base = 0.0;
        self.since = Some(now);
        self.offset = 0.0;
        self.exact = None;
        self.started = true;
    }

    /// Back to the start of the piece, running from `now` (a replay, or a resumed piece that is said again from its start).
    pub fn restart(&mut self, now: f32) {
        self.start(now);
    }

    pub fn is_running(&self) -> bool {
        self.since.is_some()
    }

    /// Freeze the clock at `now`.
    pub fn pause(&mut self, now: f32) {
        if let Some(s) = self.since.take() {
            self.base += (now - s).max(0.0);
            self.frozen_at = now;
        }
    }

    /// Carry on from where it froze.
    pub fn resume(&mut self, now: f32) {
        if self.started && self.since.is_none() {
            self.since = Some(now);
        }
    }

    /// The speaker reports how much of the piece has been heard (`played_ms`): the clock is set to it, and runs on from there
    /// while the sound `advancing`; when the sound stands still (starved) the clock stands still with it.
    ///
    /// The sound is the truth, so the clock goes to it at once, forward as far as it is (a stall that ends, a sentence that
    /// began late: the word jumps to where the sound is, that is the fastest catch-up there is) and backward too, except for a
    /// step back of less than [`JITTER_BACK_MS`] while the sound runs, which is the speaker's position estimate wobbling: the
    /// word never flickers back, and the clock is at most that far ahead of the sound.
    pub fn follow(&mut self, played_ms: f32, now: f32, advancing: bool) {
        if !self.started {
            return;
        }
        let mut played_ms = played_ms;
        if advancing && self.since.is_some() {
            let cur = self.position(now);
            if played_ms < cur && cur - played_ms < JITTER_BACK_MS {
                played_ms = cur;
            }
            // never leap more than a few words at once; the next polls (15 ms apart) carry on
            if let (Some(a), Some(b)) = (self.tl.word_at(cur), self.tl.word_at(played_ms)) {
                if b > a + MAX_LEAP_WORDS {
                    played_ms = self.tl.start_ms(a + MAX_LEAP_WORDS).unwrap_or(played_ms);
                }
            }
        }
        self.base = played_ms + self.latency;
        self.since = advancing.then_some(now);
        self.frozen_at = now;
        self.offset = 0.0;
        self.exact = None;
    }

    /// Jump so that word `i` is being said at `now`.
    pub fn seek_word(&mut self, i: usize, now: f32) {
        let t = self.tl.start_ms(i.min(self.tl.len().saturating_sub(1))).unwrap_or(0.0);
        self.base = t + self.latency;
        self.since = if self.since.is_some() { Some(now) } else { None };
        self.frozen_at = now;
        self.last_boundary_at = now - STALL_MS - 1.0;
        self.offset = 0.0;
        self.exact = None;
        self.started = true;
    }

    /// Where the clock is in the sound at `now`, in milliseconds (what the word shown is chosen by).
    pub fn position(&self, now: f32) -> f32 {
        self.elapsed(self.at(now))
    }

    /// Milliseconds of the sound played at `now` (frozen while paused, minus the start latency, plus the calibration).
    pub fn elapsed(&self, now: f32) -> f32 {
        let raw = self.base + self.since.map_or(0.0, |s| (now - s).max(0.0));
        raw - self.latency + self.offset
    }

    /// The voice says word `i` of the timeline starts now (`now` is the time of the event). Later estimates continue from it.
    pub fn boundary(&mut self, word: usize, now: f32) {
        if !self.started || word >= self.tl.len() {
            return;
        }
        let raw = self.base + self.since.map_or(0.0, |s| (now - s).max(0.0)) - self.latency;
        self.offset = self.tl.start_ms(word).unwrap_or(0.0) - raw;
        self.exact = Some(word);
        self.last_boundary_at = now;
    }

    /// A boundary event with a character offset into the timeline's text.
    pub fn boundary_char(&mut self, ch: usize, now: f32) {
        if let Some(w) = self.tl.word_at_char(ch) {
            self.boundary(w, now);
        }
    }

    /// The time to judge things at: `now`, or the moment the clock froze while it is frozen.
    fn at(&self, now: f32) -> f32 {
        if self.since.is_some() {
            now
        } else {
            self.frozen_at
        }
    }

    pub fn mode(&self, now: f32) -> TimingMode {
        let now = self.at(now);
        match self.exact {
            Some(_) if now - self.last_boundary_at <= STALL_MS => TimingMode::Exact,
            _ => TimingMode::Estimated,
        }
    }

    /// The word being said at `now`.
    pub fn current(&self, now: f32) -> Option<usize> {
        if !self.started {
            return None;
        }
        let est = self.tl.word_at(self.elapsed(self.at(now)));
        match (self.exact, self.mode(now)) {
            (Some(w), TimingMode::Exact) => Some(w),
            (Some(w), TimingMode::Estimated) => est.map(|e| e.max(w)),
            _ => est,
        }
    }

    /// The characters of the word being said at `now`.
    pub fn range(&self, now: f32) -> Option<(usize, usize)> {
        self.current(now).and_then(|i| self.tl.span(i)).map(|w| (w.start, w.end))
    }
}

/// What a [`HeardFollower`] saw during one piece, for the log line of a sentence.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HeardStats {
    pub polls: u32,
    /// The longest time between two polls of the speaker (ms): the highlight cannot be fresher than this.
    pub max_gap_ms: f32,
    /// The most the clock was behind the sound at a poll (ms): how far one correction had to jump forward.
    pub max_behind_ms: f32,
    /// The most the clock was ahead of the sound at a poll (ms).
    pub max_ahead_ms: f32,
    /// The last heard position given (ms, after the offset).
    pub last_played_ms: f32,
}

/// Feeds the heard position of the speaker into a [`WordClock`]: the one place that decides whether the sound moves (a position
/// that stands still for [`HEARD_STILL_MS`] stops the clock), and measures how the clock and the sound agree.
#[derive(Clone, Debug, Default)]
pub struct HeardFollower {
    heard: Option<(f32, f32)>,
    last_poll: Option<f32>,
    stats: HeardStats,
}

impl HeardFollower {
    pub fn new() -> HeardFollower {
        HeardFollower::default()
    }

    /// The speaker says `played_ms` of the piece were heard at `now` (both in the clock's units and already corrected by the
    /// person's highlight offset).
    pub fn poll(&mut self, k: &mut WordClock, played_ms: f32, now: f32) {
        let advancing = self.heard.is_none_or(|(prev, at)| played_ms != prev || now - at < HEARD_STILL_MS);
        if self.heard.is_none_or(|(prev, _)| played_ms != prev) {
            self.heard = Some((played_ms, now));
        }
        if k.started {
            let cur = k.position(now);
            self.stats.max_behind_ms = self.stats.max_behind_ms.max(played_ms - cur);
            self.stats.max_ahead_ms = self.stats.max_ahead_ms.max(cur - played_ms);
        }
        if let Some(t) = self.last_poll {
            self.stats.max_gap_ms = self.stats.max_gap_ms.max(now - t);
        }
        self.last_poll = Some(now);
        self.stats.polls += 1;
        self.stats.last_played_ms = played_ms;
        k.follow(played_ms, now, advancing);
    }

    pub fn stats(&self) -> HeardStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(t: &str) -> Vec<String> {
        let c: Vec<char> = t.chars().collect();
        words_of(t).iter().map(|w| c[w.start..w.end].iter().collect()).collect()
    }

    #[test]
    fn latin_words_keep_apostrophes_hyphens_and_numbers_whole() {
        assert_eq!(texts("Don't stop, well-known 3.14 and 1,000 times."), vec!["Don't", "stop", "well-known", "3.14", "and", "1,000", "times"]);
        assert_eq!(texts("\"Quoted,\" she said."), vec!["Quoted", "she", "said"]);
        assert!(texts("... -- !!").is_empty());
    }

    #[test]
    fn char_ranges_are_chars_not_bytes() {
        let t = "Zoë went — ñandú";
        let w = words_of(t);
        let c: Vec<char> = t.chars().collect();
        assert_eq!(c[w[0].start..w[0].end].iter().collect::<String>(), "Zoë");
        assert_eq!(c[w[2].start..w[2].end].iter().collect::<String>(), "ñandú");
    }

    #[test]
    fn indic_words_are_whole_clusters_and_never_split_inside() {
        for t in ["வணக்கம் உலகம்", "नमस्ते दुनिया", "తెలుగు భాష", "മലയാളം ഭാഷ"] {
            let w = words_of(t);
            assert_eq!(w.len(), 2, "{t}");
            let c: Vec<char> = t.chars().collect();
            // a word never starts with a vowel sign, virama or other mark that belongs to the letter before it
            for s in &w {
                let low = (c[s.start] as u32) & 0x7F;
                assert!(!(0x3A..=0x4D).contains(&low), "{t}: starts with a mark");
                assert!(s.end == c.len() || c[s.end].is_whitespace(), "{t}: ends mid-word");
            }
        }
    }

    #[test]
    fn cjk_is_one_word_per_character_and_korean_splits_at_spaces() {
        assert_eq!(texts("你好世界"), vec!["你", "好", "世", "界"]);
        assert_eq!(texts("こんにちは"), vec!["こ", "ん", "に", "ち", "は"]);
        assert_eq!(texts("안녕 세상"), vec!["안녕", "세상"]);
        assert_eq!(texts("Hello 世界!"), vec!["Hello", "世", "界"]);
    }

    #[test]
    fn arabic_words_split_at_spaces_with_their_marks() {
        let t = "مرحبا بالعالم";
        assert_eq!(texts(t).len(), 2);
        assert_eq!(texts("كَتَبَ الولد"), vec!["كَتَبَ", "الولد"]);
    }

    #[test]
    fn weights_follow_syllables_not_letters() {
        assert!(word_units("strengths") < word_units("beautiful") + 0.01, "one syllable beats nothing, three syllables win");
        assert!(word_units("a") >= 1.0 && word_units("sky") >= 1.0);
        assert!((word_units("beautiful") - 3.0).abs() < 1.0);
        assert!(word_units("extraordinarily") > word_units("extra"));
        assert!(word_units("12") >= 2.0, "each digit is said");
        assert!(word_units("x") >= 0.5);
    }

    #[test]
    fn indic_weights_count_aksharas() {
        // வணக்கம்: வ ண க்க ம் -> 3 aksharas (வ, ணக்?, ...) at least 3 and far from the 7 code points
        let v = word_units("வணக்கம்");
        assert!((2.5..=4.5).contains(&v), "{v}");
        // नमस्ते: न म स्ते -> 3
        let d = word_units("नमस्ते");
        assert!((2.5..=3.8).contains(&d), "{d}");
        // a conjunct is not two full units
        assert!(word_units("क्क") < 2.0 * word_units("क"));
        // a final virama is short
        assert!(word_units("க்") < word_units("க"));
        assert!(word_units("తెలుగు") > 2.0);
        assert!(word_units("മലയാളം") > 2.5);
        assert!(word_units("ऄ") > 0.0);
    }

    #[test]
    fn cjk_and_arabic_weights() {
        assert_eq!(word_units("你"), 1.0);
        assert!(word_units("こ") < word_units("你"));
        assert!(word_units("مرحبا") > 1.5 && word_units("مرحبا") < 3.0);
        assert!(word_units("привет") > 1.0);
    }

    #[test]
    fn empty_and_marks_only_text_has_no_words() {
        let t = WordTimeline::estimate("", 1.0);
        assert!(t.is_empty() && t.word_at(100.0).is_none() && t.total_ms() == 0.0);
        let t = WordTimeline::fitted("... ,,", 1000.0);
        assert!(t.is_empty() && t.range_at(5.0).is_none());
    }

    #[test]
    fn punctuation_adds_pauses_stronger_for_a_stop_than_a_comma() {
        let plain = WordTimeline::estimate("one two three four", 1.0);
        let comma = WordTimeline::estimate("one, two three four", 1.0);
        let stop = WordTimeline::estimate("one. two three four", 1.0);
        assert!(comma.total_ms() > plain.total_ms());
        assert!(stop.total_ms() > comma.total_ms());
        // the pause belongs to the word before it: "two" starts later after a stop
        assert!(stop.start_ms(1).unwrap() - comma.start_ms(1).unwrap() > 150.0);
        let dash = WordTimeline::estimate("one — two three four", 1.0);
        assert!(dash.total_ms() > plain.total_ms() && dash.total_ms() < comma.total_ms());
        let hindi = WordTimeline::estimate("नमस्ते। दुनिया", 1.0);
        let hindi_plain = WordTimeline::estimate("नमस्ते दुनिया", 1.0);
        assert!(hindi.total_ms() > hindi_plain.total_ms() + 300.0, "the danda is a full stop");
    }

    #[test]
    fn a_rate_estimate_scales_with_speed() {
        let a = WordTimeline::estimate("The fox woke early and looked at the river.", 1.0);
        let b = WordTimeline::estimate("The fox woke early and looked at the river.", 2.0);
        assert!(!a.is_fitted());
        assert!((a.total_ms() / b.total_ms() - 2.0).abs() < 0.01);
        // about 150 words a minute: 9 words take 3 to 6 seconds
        assert!((2500.0..=6500.0).contains(&a.total_ms()), "{}", a.total_ms());
        assert!(WordTimeline::estimate("hi", f32::NAN).total_ms() > 0.0);
    }

    #[test]
    fn a_fitted_timeline_ends_exactly_at_the_measured_length() {
        let t = WordTimeline::fitted("The fox woke early. She looked at the river.", 4200.0);
        assert!(t.is_fitted());
        assert!((t.total_ms() - 4200.0).abs() < 0.01);
        assert_eq!(t.start_ms(0), Some(0.0));
        let mut last = -1.0;
        for i in 0..t.len() {
            let s = t.start_ms(i).unwrap();
            assert!(s > last, "starts rise");
            last = s;
        }
        assert_eq!(t.word_at(0.0), Some(0));
        assert_eq!(t.word_at(4199.0), Some(t.len() - 1));
        assert_eq!(t.word_at(99999.0), Some(t.len() - 1), "the last word stays after the end");
        assert_eq!(t.word_at(-5.0), None);
        assert_eq!(WordTimeline::fitted("a b", 0.0).total_ms() > 0.0, true, "no length: a rate estimate");
    }

    #[test]
    fn a_longer_word_gets_a_longer_slot() {
        let t = WordTimeline::fitted("a extraordinarily a", 3000.0);
        let d0 = t.end_ms(0).unwrap() - t.start_ms(0).unwrap();
        let d1 = t.end_ms(1).unwrap() - t.start_ms(1).unwrap();
        assert!(d1 > 3.0 * d0, "{d0} {d1}");
    }

    #[test]
    fn word_at_char_finds_the_word_or_the_next_one() {
        let t = WordTimeline::estimate("Hello, big world", 1.0);
        assert_eq!(t.word_at_char(0), Some(0));
        assert_eq!(t.word_at_char(3), Some(0));
        assert_eq!(t.word_at_char(5), Some(1), "the comma: the next word");
        assert_eq!(t.word_at_char(7), Some(1));
        assert_eq!(t.word_at_char(13), Some(2));
        assert_eq!(t.word_at_char(99), None);
    }

    #[test]
    fn index_mapping_and_utf16_offsets() {
        assert_eq!(map_index(3, 5, 5), 3);
        assert_eq!(map_index(0, 4, 8), 0);
        assert_eq!(map_index(3, 4, 8), 7);
        assert_eq!(map_index(9, 5, 5), 4, "clamped");
        assert_eq!(map_index(2, 3, 0), 0);
        // an emoji is two UTF-16 units but one char
        let t = "😀 hi there";
        assert_eq!(utf16_to_char(t, 0), 0);
        assert_eq!(utf16_to_char(t, 3), 2);
        assert_eq!(utf16_to_char(t, 999), t.chars().count());
    }

    #[test]
    fn spoken_words_are_matched_to_drawn_words_across_markup_and_code() {
        // drawn: a list marker, a link address and a code block that are not spoken
        let drawn = ["1", "Use", "the", "tool", "https", "example", "org", "x", "=", "1", "then", "stop"];
        let spoken = ["1", "Use", "the", "tool", "then", "stop"];
        let m = align_words(&spoken, &drawn);
        assert_eq!(m, vec![Some(0), Some(1), Some(2), Some(3), Some(10), Some(11)]);
        // case and punctuation are ignored, a word that is not drawn is None and does not derail the rest
        let m = align_words(&["Hello", "ghost", "WORLD"], &["hello", "world"]);
        assert_eq!(m, vec![Some(0), None, Some(1)]);
        // Indic words match too
        assert_eq!(align_words(&["வணக்கம்", "உலகம்"], &["வணக்கம்", "உலகம்"]), vec![Some(0), Some(1)]);
        let none: Vec<&str> = vec![];
        assert_eq!(align_words(&["a"], &none), vec![None]);
        assert!(align_words(&none, &["a"]).is_empty());
        // a repeated word maps in order
        assert_eq!(align_words(&["the", "the"], &["the", "x", "the"]), vec![Some(0), Some(2)]);
    }

    fn clock(text: &str, ms: f32) -> WordClock {
        WordClock::new(WordTimeline::fitted(text, ms), 0.0)
    }

    #[test]
    fn the_clock_walks_the_words_in_time_and_none_before_start() {
        let mut c = clock("one two three four", 4000.0);
        assert_eq!(c.current(0.0), None, "not started");
        c.start(1000.0);
        assert_eq!(c.current(1000.0), Some(0));
        let mut seen = vec![];
        for t in (0..4000).step_by(50) {
            let w = c.current(1000.0 + t as f32).unwrap();
            if seen.last() != Some(&w) {
                seen.push(w);
            }
        }
        assert_eq!(seen, vec![0, 1, 2, 3]);
        assert_eq!(c.current(99999.0), Some(3));
    }

    #[test]
    fn start_latency_delays_the_first_word() {
        let tl = WordTimeline::fitted("one two three four", 4000.0);
        let mut c = WordClock::new(tl, 200.0);
        c.start(0.0);
        assert_eq!(c.current(100.0), None, "the speaker has not made a sound yet");
        assert_eq!(c.current(250.0), Some(0));
        let w = c.timeline().start_ms(2).unwrap();
        assert_eq!(c.current(w + 199.0), Some(1));
        assert_eq!(c.current(w + 201.0), Some(2));
    }

    #[test]
    fn pause_freezes_the_word_and_resume_carries_on_without_drift() {
        let mut c = clock("one two three four", 4000.0);
        c.start(0.0);
        let before = c.current(1500.0).unwrap();
        c.pause(1500.0);
        assert!(!c.is_running());
        assert_eq!(c.current(1500.0), Some(before));
        assert_eq!(c.current(60_000.0), Some(before), "an hour later it is still the same word");
        c.resume(60_000.0);
        assert_eq!(c.current(60_000.0), Some(before));
        // 1500 ms played + 1000 more = 2500 ms into the sound, whatever the pause was
        let want = c.timeline().word_at(2500.0);
        assert_eq!(c.current(61_000.0), want);
        // a second pause/resume pair adds up exactly
        c.pause(61_000.0);
        c.resume(70_000.0);
        assert!((c.elapsed(70_500.0) - 3000.0).abs() < 0.001);
    }

    #[test]
    fn seek_and_restart_jump_to_a_word_or_the_start() {
        let mut c = clock("one two three four", 4000.0);
        c.start(0.0);
        c.seek_word(2, 500.0);
        assert_eq!(c.current(500.0), Some(2));
        let d = c.timeline().end_ms(2).unwrap() - c.timeline().start_ms(2).unwrap();
        assert_eq!(c.current(500.0 + d + 1.0), Some(3));
        // seeking while paused stays paused on that word
        c.pause(900.0);
        c.seek_word(1, 1000.0);
        assert!(!c.is_running());
        assert_eq!(c.current(5000.0), Some(1));
        c.resume(5000.0);
        assert_eq!(c.current(5000.0), Some(1));
        c.restart(8000.0);
        assert_eq!(c.current(8000.0), Some(0));
        c.seek_word(99, 8000.0);
        assert_eq!(c.current(8000.0), Some(3), "past the end is the last word");
    }

    #[test]
    fn boundary_events_make_the_word_exact_and_beat_the_estimate() {
        let mut c = clock("one two three four", 4000.0);
        c.start(0.0);
        // the estimate says word 0 at 100 ms; the voice says word 2 started (it speaks faster than the model)
        c.boundary_char(8, 100.0); // "three" starts at char 8
        assert_eq!(c.mode(100.0), TimingMode::Exact);
        assert_eq!(c.current(100.0), Some(2));
        assert_eq!(c.current(300.0), Some(2), "exact: stays until the next event, even if the model would have moved on");
        c.boundary_char(14, 700.0);
        assert_eq!(c.current(700.0), Some(3));
        assert_eq!(c.range(700.0), Some((14, 18)));
    }

    #[test]
    fn without_events_the_clock_is_an_estimate_and_after_events_stop_it_follows_the_model_again() {
        let mut c = clock("one two three four five six seven eight", 8000.0);
        c.start(0.0);
        assert_eq!(c.mode(5000.0), TimingMode::Estimated);
        c.boundary(1, 1000.0);
        assert_eq!(c.mode(1000.0), TimingMode::Exact);
        assert_eq!(c.current(1500.0), Some(1));
        // the voice goes quiet (no events): the model, calibrated at word 1, carries on
        let late = 1000.0 + STALL_MS + 2000.0;
        assert_eq!(c.mode(late), TimingMode::Estimated);
        assert!(c.current(late).unwrap() > 1);
        // never earlier than the last reported word
        assert!(c.current(1000.0 + STALL_MS + 1.0).unwrap() >= 1);
    }

    #[test]
    fn a_clock_frozen_after_the_events_stopped_keeps_the_word_it_showed() {
        let mut c = clock("one two three four five six seven eight", 8000.0);
        c.start(0.0);
        c.boundary(1, 1000.0);
        let at = 1000.0 + STALL_MS + 2000.0;
        let shown = c.current(at).unwrap();
        assert!(shown > 1, "the model took over");
        c.pause(at);
        assert_eq!(c.current(at + 50_000.0), Some(shown), "pausing does not bring back the old exact word");
        c.resume(at + 50_000.0);
        assert_eq!(c.current(at + 50_000.0), Some(shown));
    }

    #[test]
    fn a_frozen_clock_keeps_an_exact_word_exact() {
        let mut c = clock("one two three four", 4000.0);
        c.start(0.0);
        c.boundary(2, 500.0);
        c.pause(600.0);
        assert_eq!(c.mode(100_000.0), TimingMode::Exact);
        assert_eq!(c.current(100_000.0), Some(2));
    }

    #[test]
    fn drift_stays_within_one_word_when_the_voice_is_15_percent_off_the_model() {
        // the real voice takes 15 % longer than the fitted length; events arrive every 3rd word
        let text = (0..40).map(|i| format!("word{i}")).collect::<Vec<_>>().join(" ");
        let model = WordTimeline::fitted(&text, 10_000.0);
        let mut c = WordClock::new(model.clone(), 0.0);
        c.start(0.0);
        let truth = |ms: f32| model.word_at(ms / 1.15).unwrap();
        let mut worst = 0i32;
        let mut prev = usize::MAX;
        for t in (0..11_500).step_by(20) {
            let now = t as f32;
            let tw = truth(now);
            if tw != prev && tw % 3 == 0 {
                c.boundary(tw, now);
            }
            prev = tw;
            if now > 500.0 {
                let w = c.current(now).unwrap() as i32;
                worst = worst.max((w - tw as i32).abs());
            }
        }
        assert!(worst <= 2, "worst error {worst} words");
        // without any event the error is bounded by the 15 % mismatch: at most 15 % of 40 words
        let mut c = WordClock::new(model.clone(), 0.0);
        c.start(0.0);
        let worst = (0..11_500).step_by(50).map(|t| (c.current(t as f32).unwrap() as i32 - truth(t as f32) as i32).abs()).max().unwrap();
        assert!(worst <= 7, "{worst}");
    }

    #[test]
    fn very_long_words_and_texts_are_handled() {
        let long = "a".repeat(5000);
        let t = WordTimeline::fitted(&long, 2000.0);
        assert_eq!(t.len(), 1);
        assert_eq!(t.range_at(1000.0), Some((0, 5000)));
        let many = (0..5000).map(|i| i.to_string()).collect::<Vec<_>>().join(" ");
        let t = WordTimeline::estimate(&many, 1.0);
        assert_eq!(t.len(), 5000);
        assert_eq!(t.word_at(t.total_ms() / 2.0).map(|i| i > 2000 && i < 3000), Some(true));
    }

    #[test]
    fn a_clock_with_no_words_never_names_one() {
        let mut c = WordClock::new(WordTimeline::estimate("", 1.0), 0.0);
        c.start(0.0);
        c.boundary(0, 5.0);
        c.boundary_char(0, 5.0);
        assert_eq!(c.current(100.0), None);
        assert_eq!(c.range(100.0), None);
    }
}

#[cfg(test)]
mod follow_tests {
    use super::*;

    #[test]
    fn a_clock_that_follows_the_heard_position_stands_still_while_the_sound_does() {
        let tl = WordTimeline::estimate("one two three four five six seven eight", 1.0);
        let mut k = WordClock::new(tl, 0.0);
        k.start(0.0);
        k.follow(0.0, 10.0, false); // nothing is audible yet (the lead): the first word waits
        assert_eq!(k.elapsed(400.0), 0.0);
        k.follow(300.0, 500.0, true); // 300 ms are heard
        assert!((k.elapsed(520.0) - 320.0).abs() < 0.01);
        k.follow(340.0, 540.0, false); // the sound stalls: the clock freezes at what was heard
        assert_eq!(k.elapsed(540.0), 340.0);
        assert_eq!(k.elapsed(5000.0), 340.0, "no word advances during the stall");
        k.follow(340.0, 5000.0, true);
        assert!(k.elapsed(5100.0) > 340.0);
    }
}

/// A simulation of what the reader does while it reads aloud: a fake speaker reports the heard position, the reader polls it
/// every 15 ms into a [`WordClock`] through a [`HeardFollower`], and the page asks for the lit word on every UI tick. The true
/// word is known (the sound is made from a known timeline), so lag is measured, not argued.
#[cfg(test)]
mod lag_tests {
    use super::*;

    const POLL_MS: f32 = 15.0;
    const UI_MS: f32 = 33.0;

    /// The sentence as the speaker says it: word durations that differ from the model's by up to +-25 % (so the test is not
    /// the model checking itself), `lead` ms of silence before and `trail` ms after the speech, scaled by `speed` (2.0: twice as fast).
    struct Spoken {
        text: String,
        /// Where each word really starts in the sound, then the end of the sound (ms).
        truth: Vec<f32>,
        audio_ms: f32,
        lead: f32,
        trail: f32,
    }

    fn spoken(text: &str, speed: f32, lead: f32, trail: f32, seed: u32) -> Spoken {
        let model = WordTimeline::estimate(text, 1.0);
        let n = model.len();
        let mut rng = seed.wrapping_mul(2654435761).wrapping_add(12345);
        let mut begin = vec![lead];
        let mut t = lead;
        for i in 0..n {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            let jitter = 0.75 + ((rng >> 16) & 0xFF) as f32 / 255.0 * 0.5;
            let mut dur = model.end_ms(i).unwrap() - model.start_ms(i).unwrap();
            if i == n - 1 {
                // the pause after the last word is silence at the end of the sound, not speech
                let c: Vec<char> = text.chars().collect();
                dur -= pause_between(&c[model.span(i).unwrap().end..]);
            }
            let dur = dur / speed * jitter;
            t += dur;
            begin.push(t);
        }
        let audio_ms = t + trail;
        // the last entry is the end of the speech; words are said from lead to there
        Spoken { text: text.to_string(), truth: begin, audio_ms, lead, trail }
    }

    impl Spoken {
        fn true_word(&self, pos: f32) -> usize {
            let n = self.truth.len() - 1;
            self.truth[..n].partition_point(|b| *b <= pos).saturating_sub(1).min(n - 1)
        }
    }

    /// The speaker: heard position of the piece that sounds from `audible_at`, frozen inside the `stalls` (start, end) windows.
    struct Speaker {
        audible_at: f32,
        stalls: Vec<(f32, f32)>,
        /// Constant error of the position estimate (ms): positive reports the sound further than it is.
        bias: f32,
        wobble: f32,
    }

    impl Speaker {
        fn true_pos(&self, now: f32) -> f32 {
            let mut t = (now - self.audible_at).max(0.0);
            for (a, b) in &self.stalls {
                let (a, b) = (a - self.audible_at, b - self.audible_at);
                if t > a {
                    t -= (t.min(b) - a).max(0.0);
                }
            }
            t
        }
        fn heard(&self, now: f32) -> f32 {
            // a few ms of wobble from the mixer thread's timing
            let w = ((now as i64 * 7) % 5) as f32 / 4.0 * self.wobble;
            if self.true_pos(now) <= 0.0 { 0.0 } else { (self.true_pos(now) + self.bias + w).max(0.0) }
        }
    }

    struct Run {
        /// Largest |lit word - spoken word| over the sentence, in words.
        worst_words: usize,
        /// Largest delay of the clock behind the true position (ms), where the sound was running.
        worst_lag_ms: f32,
        worst_ahead_ms: f32,
        /// Time after a stall ends until the lit word is the spoken word again (ms), the largest.
        worst_catch_up_ms: f32,
        stats: HeardStats,
    }

    /// Read `sentences` in a row. `tl` builds the timeline the reader uses for a sentence.
    fn run(sentences: &[Spoken], stalls: &[(f32, f32)], bias: f32, tl: &dyn Fn(&Spoken) -> WordTimeline) -> Vec<Run> {
        let mut out = Vec::new();
        let mut start = 100.0f32; // when the first piece begins (the speaker is audible 80 ms later)
        for s in sentences {
            let sp = Speaker { audible_at: start + 80.0, stalls: stalls.to_vec(), bias, wobble: 6.0 };
            let mut k = WordClock::new(tl(s), START_LATENCY_MS);
            k.start(start);
            let mut f = HeardFollower::new();
            let (mut next_poll, mut next_ui) = (start, start);
            let (mut worst_words, mut worst_lag, mut worst_ahead, mut worst_catch) = (0usize, 0.0f32, 0.0f32, 0.0f32);
            let mut off_since: Option<f32> = None;
            let mut now = start;
            loop {
                if now >= next_poll {
                    f.poll(&mut k, sp.heard(now), now);
                    next_poll += POLL_MS;
                }
                if now >= next_ui {
                    next_ui += UI_MS;
                    let pos = sp.true_pos(now);
                    if pos > s.lead + 50.0 && pos < s.audio_ms - s.trail {
                        let lit = k.current(now).unwrap();
                        let truth = s.true_word(pos);
                        // right when within a word of the spoken one, or when the spoken word is within 120 ms of the lit one's time
                        let near = |w: usize| s.truth[w] - 120.0 <= pos && pos <= s.truth[w + 1] + 120.0;
                        let miss = if near(lit) { 0 } else { lit.abs_diff(truth) };
                        worst_words = worst_words.max(miss);
                        let lag = pos - k.position(now);
                        worst_lag = worst_lag.max(lag);
                        worst_ahead = worst_ahead.max(-lag);
                        // after a stall: how long the lit word stays more than one word off
                        if miss > 1 {
                            let t0 = *off_since.get_or_insert(now);
                            worst_catch = worst_catch.max(now - t0);
                        } else {
                            off_since = None;
                        }
                    }
                }
                if sp.true_pos(now) >= s.audio_ms {
                    break;
                }
                now += 1.0;
            }
            out.push(Run { worst_words, worst_lag_ms: worst_lag, worst_ahead_ms: worst_ahead, worst_catch_up_ms: worst_catch, stats: f.stats() });
            // the reader moves on when the sound has ended; the next piece starts at once
            start = now;
        }
        out
    }

    const TEXTS: [&str; 4] = [
        "The old fox woke early and ran across the quiet meadow towards home.",
        "Nobody noticed, because the rain was loud on the roof of the barn.",
        "\"Wait for me,\" said the little owl, and then it was gone.",
        "Morning came slowly; the farmer opened the gate and counted his sheep twice.",
    ];

    fn sentences(speed: f32) -> Vec<Spoken> {
        TEXTS.iter().enumerate().map(|(i, t)| spoken(t, speed, 60.0, 250.0, i as u32 + 1)).collect()
    }

    fn fitted(s: &Spoken) -> WordTimeline {
        WordTimeline::fitted_in(&s.text, s.audio_ms, s.lead, s.trail)
    }

    #[test]
    fn at_normal_and_fast_speed_the_lit_word_stays_within_one_word_and_the_clock_within_40_ms_of_the_sound() {
        for speed in [0.8f32, 1.0, 1.5, 2.0, 2.5] {
            let runs = run(&sentences(speed), &[], 0.0, &fitted);
            for (i, r) in runs.iter().enumerate() {
                assert!(r.worst_words <= 1, "speed {speed} sentence {i}: lit word {} words from the spoken one", r.worst_words);
                assert!(r.worst_lag_ms <= 40.0, "speed {speed} sentence {i}: the clock lags the sound by {} ms", r.worst_lag_ms);
                assert!(r.worst_ahead_ms <= 40.0, "speed {speed} sentence {i}: the clock is ahead of the sound by {} ms", r.worst_ahead_ms);
                assert!(r.stats.max_gap_ms <= 16.0, "polls keep their cadence: {}", r.stats.max_gap_ms);
            }
        }
    }

    #[test]
    fn a_stall_that_ends_is_caught_up_at_once_and_the_clock_never_runs_ahead_of_a_stalled_sound() {
        // the sound stops for 400 ms in the middle of the first sentence (the speaker was starved), and again in the third
        let s = sentences(1.0);
        let stalls = [(1500.0, 1900.0)];
        let runs = run(&s[..1], &stalls, 0.0, &fitted);
        let r = &runs[0];
        assert!(r.worst_words <= 1, "{}", r.worst_words);
        assert!(r.worst_ahead_ms <= 80.0, "the clock ran {} ms ahead of the stalled sound (60 ms still-detection + a poll)", r.worst_ahead_ms);
        assert!(r.worst_catch_up_ms <= UI_MS * 2.0, "{} ms to be back on the word after the stall", r.worst_catch_up_ms);
        assert!(r.worst_lag_ms <= 40.0, "{}", r.worst_lag_ms);
    }

    #[test]
    fn several_sentences_in_a_row_each_start_on_their_first_word_with_no_leftover_lag() {
        let runs = run(&sentences(1.5), &[], 0.0, &fitted);
        assert_eq!(runs.len(), 4);
        for (i, r) in runs.iter().enumerate() {
            assert!(r.worst_words <= 1 && r.worst_lag_ms <= 40.0, "sentence {i}: {} words, {} ms", r.worst_words, r.worst_lag_ms);
            // a sentence's clock ends where the sound ends
            assert!((r.stats.last_played_ms - sentences(1.5)[i].audio_ms).abs() <= 20.0, "sentence {i}: heard {} of {}", r.stats.last_played_ms, sentences(1.5)[i].audio_ms);
        }
    }

    #[test]
    fn taking_the_silence_of_the_sound_out_of_the_fit_makes_the_late_words_on_time() {
        // 250 ms of silence after the speech: a fit over the whole sound puts every word late, the last ones the most
        let s = spoken(TEXTS[0], 1.0, 60.0, 250.0, 1);
        let plain = WordTimeline::fitted(&s.text, s.audio_ms);
        let trimmed = fitted(&s);
        let n = plain.len();
        let err = |tl: &WordTimeline| (0..n).map(|i| (tl.start_ms(i).unwrap() - s.truth[i]).abs()).fold(0.0f32, f32::max);
        // (the truth has +-25 % jitter per word, so the bound is the model's own spread)
        assert!(err(&trimmed) < err(&plain), "trimmed {} ms vs plain {} ms", err(&trimmed), err(&plain));
        assert!(trimmed.start_ms(0).unwrap() >= 59.0 && trimmed.total_ms() == s.audio_ms);
        // with no silence to take out it is the plain fit
        assert_eq!(WordTimeline::fitted_in(&s.text, 3000.0, 0.0, 0.0), WordTimeline::fitted(&s.text, 3000.0));
        // silence that leaves no room is ignored
        assert_eq!(WordTimeline::fitted_in(&s.text, 1000.0, 600.0, 600.0), WordTimeline::fitted(&s.text, 1000.0));
    }

    #[test]
    fn a_constant_error_of_the_heard_position_is_the_error_of_the_lit_word_and_the_offset_removes_it() {
        // the speaker reports 350 ms less than is heard (the old card-buffer subtraction): the clock lags by exactly that
        let late = run(&sentences(1.5), &[], -350.0, &fitted);
        assert!(late.iter().all(|r| (300.0..=400.0).contains(&r.worst_lag_ms)), "{:?}", late.iter().map(|r| r.worst_lag_ms).collect::<Vec<_>>());
        // at 1.5x speech a word is about 270 ms: that is more than a word
        assert!(late.iter().any(|r| r.worst_words >= 1));
        // an offset of -350 (the person tunes the highlight earlier) puts it back
        let fixed = run(&sentences(1.5), &[], 0.0, &fitted);
        assert!(fixed.iter().all(|r| r.worst_lag_ms <= 40.0));
    }

    #[test]
    fn the_heard_position_wobbling_a_few_ms_never_steps_the_clock_back() {
        let tl = WordTimeline::estimate("one two three four five six seven eight", 1.0);
        let mut k = WordClock::new(tl, START_LATENCY_MS);
        k.start(0.0);
        let mut f = HeardFollower::new();
        let mut last = 0.0f32;
        for step in 1..200 {
            let now = step as f32 * 15.0;
            let p = now + if step % 2 == 0 { -10.0 } else { 10.0 };
            f.poll(&mut k, p, now);
            let pos = k.position(now);
            assert!(pos + 0.01 >= last, "stepped back from {last} to {pos} at {now}");
            assert!((pos - now).abs() <= 10.5, "{pos} vs {now}: at most the wobble ahead");
            last = pos;
        }
        // a real rewind (a new piece, a seek) is followed
        f.poll(&mut k, 10.0, 3200.0);
        assert!(k.position(3200.0) < 100.0);
    }
}

#[cfg(test)]
mod pin_tests {
    use super::*;

    /// A voice whose true word times differ from the character model: `ramp` makes it start fast and end slow (first word
    /// `1/ramp` of the model, last `ramp` times), commas pause `comma_ms`, full stops inside `stop_ms`; `lead`/`trail` of padding.
    struct Voice {
        truth: Vec<f32>,
        env: Vec<f32>,
        audio_ms: f32,
        lead: f32,
        trail: f32,
    }

    fn voice(text: &str, ramp: f32, comma_ms: f32, lead: f32, trail: f32) -> Voice {
        let c: Vec<char> = text.chars().collect();
        let spans = words_of(text);
        let n = spans.len();
        let mut truth = Vec::new();
        let mut quiet: Vec<(f32, f32)> = Vec::new();
        let mut t = lead;
        for (i, w) in spans.iter().enumerate() {
            truth.push(t);
            let s: String = c[w.start..w.end].iter().collect();
            let f = (1.0 / ramp) * (ramp * ramp).powf(i as f32 / (n.max(2) - 1) as f32);
            t += word_units(&s) * UNIT_MS * f;
            if i + 1 < n {
                let p = gap_after(&c, &spans, i);
                if p > 0.0 {
                    let d = if p >= PAUSE_HARD { comma_ms * 2.0 } else { comma_ms };
                    quiet.push((t, t + d));
                    t += d;
                }
            }
        }
        truth.push(t);
        let audio_ms = t + trail;
        let frames = (audio_ms / ENV_FRAME_MS).ceil() as usize;
        let env = (0..frames)
            .map(|j| {
                let (a, b) = (j as f32 * ENV_FRAME_MS, (j + 1) as f32 * ENV_FRAME_MS);
                let silent = a + 1.0 < lead || b > t + 1.0 || quiet.iter().any(|(x, y)| a >= *x - 1.0 && b <= *y + 1.0);
                if silent {
                    0.0
                } else {
                    0.4
                }
            })
            .collect();
        Voice { truth, env, audio_ms, lead, trail }
    }

    fn worst(tl: &WordTimeline, v: &Voice) -> f32 {
        (0..tl.len()).map(|i| (tl.start_ms(i).unwrap() - v.truth[i]).abs()).fold(0.0, f32::max)
    }

    const TEXTS: [&str; 5] = [
        "Well, the extraordinary thing about the old lighthouse, which nobody had visited for years, was its light.",
        "He ran. She did not; they waited, quietly, in the cold, until morning came and the snow stopped.",
        "In 1984, Dr. Smith paid 250 dollars for it - a lot of money, then.",
        "Internationalization is hard, but cats sit on mats, and dogs run in the park.",
        "Short one.",
    ];

    #[test]
    fn pinned_words_stay_within_120_ms_of_the_true_times_where_the_plain_fit_drifts() {
        let mut plain_worst = 0.0f32;
        for (k, text) in TEXTS.iter().enumerate() {
            for (ramp, comma) in [(1.0, 100.0), (1.1, 140.0), (1.15, 90.0), (1.15, 260.0)] {
                let v = voice(text, ramp, comma, 160.0, 380.0);
                let plain = WordTimeline::fitted_in(text, v.audio_ms, v.lead, v.trail);
                let (pin, rep) = WordTimeline::fitted_pinned(text, v.audio_ms, v.lead, v.trail, &v.env);
                plain_worst = plain_worst.max(worst(&plain, &v));
                if text.chars().filter(|c| matches!(c, ',' | ';' | '.' | '-')).count() > 1 && *text != TEXTS[4] {
                    assert!(rep.pins >= 1, "text {k} ramp {ramp}: no pin");
                }
                let e = worst(&pin, &v);
                assert!(e <= 120.0, "text {k} ramp {ramp} comma {comma}: pinned error {e:.0} ms (plain {:.0}), pins {}, truth {:?}; {}", worst(&plain, &v), rep.pins, v.truth.iter().map(|x| *x as i32).collect::<Vec<_>>(), pin.debug_line(text));
                assert!((pin.total_ms() - v.audio_ms).abs() < 1.0);
                assert!(rep.stretch.is_finite() && rep.stretch > 0.2);
                assert!((0..pin.len()).all(|i| pin.start_ms(i) <= pin.end_ms(i)), "never runs backwards");
            }
        }
        assert!(plain_worst > 120.0, "the synthetic voices must really drift without pinning ({plain_worst:.0})");
    }

    #[test]
    fn pinning_beats_the_plain_fit_on_average_and_never_runs_backwards() {
        let (mut ep, mut ef) = (0.0f32, 0.0f32);
        for text in &TEXTS[..4] {
            let v = voice(text, 1.8, 200.0, 200.0, 300.0);
            let plain = WordTimeline::fitted_in(text, v.audio_ms, v.lead, v.trail);
            let (pin, _) = WordTimeline::fitted_pinned(text, v.audio_ms, v.lead, v.trail, &v.env);
            ep += worst(&pin, &v);
            ef += worst(&plain, &v);
            for i in 1..pin.len() {
                assert!(pin.start_ms(i) >= pin.start_ms(i - 1));
            }
        }
        assert!(ep < ef, "pinned {ep:.0} vs plain {ef:.0}");
    }

    #[test]
    fn indic_and_unpunctuated_text_and_missing_pcm_fall_back_cleanly() {
        for text in ["நான் நேற்று கடைக்குச் சென்றேன், ஆனால் அது மூடியிருந்தது. பிறகு வீடு திரும்பினேன்.", "मैं कल बाज़ार गया, लेकिन दुकान बंद थी। फिर घर लौट आया।", "no punctuation at all in this one sentence here"] {
            let v = voice(text, 1.5, 150.0, 120.0, 300.0);
            let (pin, rep) = WordTimeline::fitted_pinned(text, v.audio_ms, v.lead, v.trail, &v.env);
            let plain = WordTimeline::fitted_in(text, v.audio_ms, v.lead, v.trail);
            // without punctuation there is nothing to pin: the plain fit, never worse
            assert!(worst(&pin, &v) <= worst(&plain, &v) + 1.0, "{text}: {:.0}; {}", worst(&pin, &v), pin.debug_line(text));
            if rep.pins == 0 {
                assert_eq!(pin, plain);
            }
            assert!(rep.stretch > 0.0);
            let (none, r2) = WordTimeline::fitted_pinned(text, v.audio_ms, v.lead, v.trail, &[]);
            assert_eq!(r2.pins, 0);
            assert_eq!(none, WordTimeline::fitted_in(text, v.audio_ms, v.lead, v.trail));
        }
    }

    /// The whole path of a sentence: the speaker's heard position drives the clock, polled every 15 ms; the highlighted word
    /// is compared with the word really being said, and never jumps more than the limit.
    #[test]
    fn the_follower_with_a_pinned_timeline_tracks_the_true_word_and_never_leaps() {
        for text in &TEXTS[..4] {
            let v = voice(text, 1.8, 180.0, 160.0, 380.0);
            let (tl, _) = WordTimeline::fitted_pinned(text, v.audio_ms, v.lead, v.trail, &v.env);
            let n = tl.len();
            let mut k = WordClock::new(tl, START_LATENCY_MS);
            let mut f = HeardFollower::new();
            k.start(0.0);
            let (mut last, mut worst_late) = (0usize, 0.0f32);
            let mut now = 0.0f32;
            while now < v.audio_ms {
                f.poll(&mut k, now, now);
                let w = k.current(now).unwrap_or(0);
                assert!(w >= last && w <= last + MAX_LEAP_WORDS, "{text}: {last} -> {w} at {now}");
                last = w;
                // how late is the highlight: the true time the shown word started, versus now
                if now >= v.lead && now < v.truth[n] {
                    let shown = k.current(now).unwrap_or(0).min(n - 1);
                    worst_late = worst_late.max(now - v.truth[shown]).max(0.0);
                    let ahead = v.truth[shown] - now;
                    assert!(ahead < 130.0, "{text}: word {shown} lit {ahead:.0} ms early at {now}");
                }
                now += 15.0;
            }
            assert!(last >= n - 1, "ends on the last word");
        }
    }
}
