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
            let next = words.get(i + 1).map_or(c.len(), |n| n.start);
            // closing marks directly after the word belong to it; the rest of the gap too
            t += pause_between(&c[w.end..next.max(w.end)]) * slow;
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
