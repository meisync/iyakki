//! Tests of the service with a fake card that hears gaps in what it is given. The mixer runs on a manual clock
//! ([`MixClock::manual`]): the test moves time one period (or one stall) at a time and feeds the sources at the moments it chooses, so
//! the card's gaps, the heard position and the starvation counters depend only on what was mixed, never on how fast a thread ran on a
//! loaded machine. One test at the end keeps the real clock honest with a generous budget.

use super::*;
use std::sync::{Arc, Mutex};

#[derive(Default)]
struct CardLog {
    samples: Vec<i16>,
    /// (milliseconds of silence the card played, index in `samples` where the audio resumed)
    gaps: Vec<(u64, usize)>,
    open: bool,
    first: bool,
}

/// A card on the mixer's clock: it plays what it was given at the real-time rate of that clock, and a write that finds nothing left
/// to play is a gap (the card underran). Writing never blocks: the mixer keeps less than a buffer ahead.
#[derive(Clone)]
struct CardSink {
    log: Arc<Mutex<CardLog>>,
    clock: MixClock,
    ends: Arc<Mutex<Duration>>,
    /// What the card holds before the speaker when full, as the sink reports it (ms).
    lat: u32,
}

impl CardSink {
    fn new(clock: &MixClock) -> CardSink {
        CardSink { log: Arc::default(), clock: clock.clone(), ends: Arc::new(Mutex::new(Duration::ZERO)), lat: 0 }
    }
    fn with_latency(mut self, ms: u32) -> CardSink {
        self.lat = ms;
        self
    }
    fn gaps(&self) -> Vec<(u64, usize)> {
        self.log.lock().unwrap().gaps.clone()
    }
    fn samples(&self) -> Vec<i16> {
        self.log.lock().unwrap().samples.clone()
    }
}

impl Sink for CardSink {
    fn open(&mut self, _rate: u32, _channels: usize) -> Result<(), String> {
        let mut l = self.log.lock().unwrap();
        l.open = true;
        l.first = true;
        *self.ends.lock().unwrap() = self.clock.now();
        Ok(())
    }
    fn write(&mut self, pcm: &[i16]) -> Result<(), String> {
        let dur = Duration::from_micros(pcm.len() as u64 / 2 * 1_000_000 / 48_000);
        let now = self.clock.now();
        let mut l = self.log.lock().unwrap();
        let mut e = self.ends.lock().unwrap();
        if now > *e + Duration::from_millis(2) && !l.first {
            let at = l.samples.len();
            l.gaps.push(((now - *e).as_millis() as u64, at));
        }
        l.first = false;
        *e = (*e).max(now) + dur;
        l.samples.extend_from_slice(pcm);
        Ok(())
    }
    fn close(&mut self) {
        self.log.lock().unwrap().open = false;
    }
    fn is_open(&self) -> bool {
        self.log.lock().unwrap().open
    }
    fn name(&self) -> String {
        "card".into()
    }
    fn latency_ms(&self) -> u32 {
        self.lat
    }
}

fn rng(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

/// One period of the default engine: the step of a thread that is always on time.
const STEP: Duration = Duration::from_millis(20);

/// A mixer on a manual clock, started and asleep.
fn manual(cfg: MixerCfg, sink: Box<dyn Sink>, clock: &MixClock) -> Mixer {
    let m = Mixer::start_with_clock(cfg, sink, clock.clone());
    m.settle();
    m
}

/// Moves time forward `total` in steps, calling `each(now_ms)` before every step (the test's feeders act at that moment).
fn run_for(m: &Mixer, total: Duration, step: Duration, mut each: impl FnMut(u64)) {
    let end = m.clock().now() + total;
    while m.clock().now() < end {
        each(m.clock().now().as_millis() as u64);
        m.advance(step);
    }
}

/// A live stream: audio of constant value `v` that "arrives" in real time but in bursts after 0..=`max_gap_ms` of nothing, for `secs`.
struct Live {
    w: SourceWriter,
    rate: u32,
    ch: usize,
    v: i16,
    secs: f64,
    max_gap_ms: u64,
    seed: u64,
    next_at: u64,
    sent: usize,
    finished: bool,
}

impl Live {
    fn new(w: SourceWriter, rate: u32, ch: usize, v: i16, secs: f64, max_gap_ms: u64, seed: u64) -> Live {
        Live { w, rate, ch, v, secs, max_gap_ms, seed, next_at: 0, sent: 0, finished: false }
    }
    /// The end of the stream: the rest of what is due, and the end of the source.
    fn end(&mut self) {
        let total = (self.secs * self.rate as f64) as usize;
        if total > self.sent {
            self.w.write_i16(&vec![self.v; (total - self.sent) * self.ch]);
            self.sent = total;
        }
        self.w.finish();
        self.finished = true;
    }
    fn tick(&mut self, now_ms: u64) {
        if self.finished || now_ms < self.next_at {
            return;
        }
        self.next_at = now_ms + rng(&mut self.seed) % (self.max_gap_ms + 1);
        let due = ((now_ms as f64 / 1000.0).min(self.secs) * self.rate as f64) as usize;
        if due > self.sent {
            let n = self.w.write_i16(&vec![self.v; (due - self.sent) * self.ch]);
            assert_eq!(n, (due - self.sent) * self.ch, "the ring took all of the burst");
            self.sent = due;
        }
        if now_ms as f64 / 1000.0 >= self.secs {
            self.w.finish();
            self.finished = true;
        }
    }
}

/// Keeps a source's ring full from a long buffer (what a decoder paced by the speaker does).
struct Topup {
    data: Vec<i16>,
    pos: usize,
}

impl Topup {
    fn tick(&mut self, w: &SourceWriter) {
        if self.pos < self.data.len() {
            self.pos += w.write_i16(&self.data[self.pos..]);
        }
    }
    fn done(&self) -> bool {
        self.pos >= self.data.len()
    }
}

fn rt_cfg(lead: u32) -> MixerCfg {
    MixerCfg { idle_ms: 20_000, lead_periods: lead, raise_priority: false, ..MixerCfg::default() }
}

/// Lets what is still queued play out (700 ms of the clock), then stops the mixer.
fn done(m: &Mixer) {
    run_for(m, Duration::from_millis(700), STEP, |_| {});
    m.shutdown();
}

#[test]
fn four_jittery_sources_within_their_budgets_give_the_card_no_gap() {
    let clock = MixClock::manual();
    let card = CardSink::new(&clock);
    let m = manual(rt_cfg(15), Box::new(card.clone()), &clock);
    let mut feeders = vec![];
    // radio, radio, media: up to 300 ms without data (budget 800 ms); speech: up to 150 ms (budget 250 ms)
    for (kind, rate, ch, v, gap, seed) in [(SourceKind::Radio, 44_100, 2, 1000, 300, 7), (SourceKind::Radio, 48_000, 2, 2000, 300, 11), (SourceKind::Media, 22_050, 1, 3000, 300, 13), (SourceKind::Speech, 24_000, 1, 500, 150, 17)] {
        let w = m.open_source(kind, rate, ch).unwrap();
        feeders.push(Live::new(w, rate, ch, v, 4.0, gap, seed));
    }
    run_for(&m, Duration::from_millis(4_000), STEP, |now| feeders.iter_mut().for_each(|f| f.tick(now)));
    feeders.iter_mut().for_each(Live::end);
    done(&m);
    let st = m.stats();
    assert_eq!(card.gaps(), vec![], "the card ran dry");
    assert_eq!(st.underruns, 0, "{st:?}");
    assert_eq!(st.sink_late, 0, "{st:?}");
    let s = card.samples();
    assert!(s.len() > 48_000 * 2 * 4, "{} samples", s.len());
    assert!(s.iter().all(|&x| (0..=6500).contains(&x)), "garbage outside the sum of the sources");
}

#[test]
fn a_source_past_its_budget_goes_silent_and_does_not_hurt_the_others_or_the_card() {
    let clock = MixClock::manual();
    let card = CardSink::new(&clock);
    let m = manual(rt_cfg(15), Box::new(card.clone()), &clock);
    let media = m.open_source(SourceKind::Media, 48_000, 2).unwrap();
    let mut media = Live::new(media, 48_000, 2, 3000, 4.5, 20, 3);
    let radio = m.open_source(SourceKind::Radio, 48_000, 2).unwrap();
    let mut sent = 0usize;
    // 1.5 s of stream, a 1.5 s hole (past the 800 ms budget), then it carries on in real time
    run_for(&m, Duration::from_millis(4_300), STEP, |now| {
        media.tick(now);
        let el = now as f64 / 1000.0;
        if !(1.5..3.0).contains(&el) {
            let due = ((if el >= 3.0 { el - 1.5 } else { el }) * 48_000.0) as usize;
            if due > sent {
                radio.write_i16(&vec![1000; (due - sent) * 2]);
                sent = due;
            }
        }
    });
    radio.finish();
    media.end();
    done(&m);
    let st = m.stats();
    assert_eq!(card.gaps(), vec![], "the others keep the card fed");
    assert!(st.starved[0] >= 1, "the radio ran dry: {st:?}");
    assert_eq!(st.starved[1], 0, "the media did not: {st:?}");
    let s = card.samples();
    // the only values are the sources and their sum, apart from the two short fades
    let odd = s.iter().filter(|&&x| ![0, 1000, 3000, 4000].contains(&x)).count();
    assert!(odd < 2 * 240 * 2 * 2, "{odd} samples are neither silence nor a source");
    let longest = s.chunks(2).fold((0, 0), |(best, run), f| if f[0] == 3000 { (best.max(run + 1), run + 1) } else { (best, 0) }).0;
    assert!(longest > 48_000 / 2, "the radio's hole is silent, the media plays on ({longest} frames)");
}

/// 4 s of audio through a mixer whose thread is not scheduled for 100 ms of the clock after 2 s.
fn stalled_run(lead: u32) -> (Vec<(u64, usize)>, Stats) {
    let clock = MixClock::manual();
    let card = CardSink::new(&clock);
    let m = manual(rt_cfg(lead), Box::new(card.clone()), &clock);
    let w = m.open_source(SourceKind::Media, 48_000, 2).unwrap();
    let mut feed = Topup { data: vec![3000; 48_000 * 2 * 4], pos: 0 };
    run_for(&m, Duration::from_millis(2_000), STEP, |_| feed.tick(&w));
    feed.tick(&w);
    m.advance(Duration::from_millis(100)); // the mixer thread was not scheduled for 100 ms
    run_for(&m, Duration::from_millis(2_100), STEP, |_| feed.tick(&w));
    w.finish();
    m.advance(STEP);
    let st = m.stats();
    let gaps = card.gaps();
    m.shutdown();
    (gaps, st)
}

#[test]
fn a_100_ms_stall_of_the_mixer_thread_is_covered_by_the_lead() {
    let (gaps, st) = stalled_run(15);
    assert_eq!(gaps, vec![], "{st:?}");
    assert_eq!(st.sink_late, 0, "{st:?}");
}

#[test]
fn the_same_stall_with_the_old_two_period_lead_does_underrun() {
    let (gaps, st) = stalled_run(2);
    assert!(!gaps.is_empty() && st.sink_late >= 1, "{gaps:?} {st:?}");
}

#[test]
fn the_card_is_prerolled_with_the_whole_lead_at_open() {
    let clock = MixClock::manual();
    let sink = FakeSink::new();
    let m = manual(MixerCfg { idle_ms: 5000, raise_priority: false, ..MixerCfg::default() }, Box::new(sink.clone()), &clock);
    let w = m.open_source(SourceKind::Media, 48_000, 2).unwrap();
    assert!(w.write_i16(&vec![1000; 48_000 * 2 * 3]) > 0);
    m.settle();
    let n = sink.lock().writes;
    assert!((15..=16).contains(&n), "{n} periods at the moment of the open (the lead is 15)");
    run_for(&m, Duration::from_millis(150), STEP, |_| {});
    let n = sink.lock().writes;
    assert!((15..=26).contains(&n), "{n} periods after the first 150 ms (the lead is 15)");
    m.shutdown();
}

#[test]
fn a_source_still_filling_its_buffer_is_silent_and_does_not_open_the_card() {
    let clock = MixClock::manual();
    let sink = FakeSink::new();
    let m = manual(MixerCfg { raise_priority: false, ..MixerCfg::default() }, Box::new(sink.clone()), &clock);
    let w = m.open_source(SourceKind::Radio, 48_000, 2).unwrap();
    w.write_i16(&vec![1000; 48_000 * 2 / 2]); // 500 ms of 800
    m.settle();
    run_for(&m, Duration::from_millis(300), STEP, |_| {});
    assert_eq!(sink.lock().opens, 0);
    w.write_i16(&vec![1000; 48_000 * 2 / 2]);
    m.settle();
    run_for(&m, Duration::from_millis(300), STEP, |_| {});
    assert_eq!(sink.lock().opens, 1);
    m.shutdown();
}

/// The owner heard breaks only with two sources (radio and reading aloud): a continuous radio must stay one unbroken waveform
/// while a voice-like source starts and stops again and again.
#[test]
fn a_radio_stays_one_unbroken_waveform_while_voice_bursts_come_and_go() {
    let clock = MixClock::manual();
    let card = CardSink::new(&clock);
    let m = manual(rt_cfg(15), Box::new(card.clone()), &clock);
    let (rate, f) = (44_100u32, 440.0f64);
    let radio = m.open_source(SourceKind::Radio, rate, 2).unwrap();
    let sine: Vec<i16> = (0..rate as usize * 6).flat_map(|i| { let v = (16000.0 * (2.0 * std::f64::consts::PI * f * i as f64 / rate as f64).sin()) as i16; [v, v] }).collect();
    let mut feed = Topup { data: sine, pos: 0 };
    // a sentence: a fresh speech source of 300 ms (24 kHz mono, just audible so that it ducks the radio), then 200 ms of nothing;
    // the first one comes when the radio's jitter buffer has been filling for a second
    let mut sentences = 0;
    let mut voice: Option<SourceWriter> = None;
    let mut quiet_until = 1_000u64;
    let mut end_at: Option<u64> = None;
    while end_at.is_none_or(|t| (m.clock().now().as_millis() as u64) < t) {
        let now = m.clock().now().as_millis() as u64;
        assert!(now < 20_000, "the test did not end: {sentences} sentences, radio fed {}", feed.pos);
        feed.tick(&radio);
        if feed.done() {
            radio.finish();
        }
        if let Some(v) = &voice {
            if v.is_done() {
                voice = None;
                quiet_until = now + 200;
            }
        } else if sentences < 7 && now >= quiet_until {
            let v = m.open_source(SourceKind::Speech, 24_000, 1).unwrap();
            assert_eq!(v.write_i16(&vec![40; 24_000 * 3 / 10]), 24_000 * 3 / 10);
            v.finish();
            voice = Some(v);
            sentences += 1;
        }
        if end_at.is_none() && sentences == 7 && voice.is_none() && radio.is_done() {
            end_at = Some(now);
        }
        m.advance(STEP);
    }
    done(&m);
    assert_eq!(card.gaps(), vec![]);
    let left: Vec<f64> = card.samples().chunks(2).map(|f| f[0] as f64).collect();
    let first = left.iter().position(|v| v.abs() > 1000.0).unwrap();
    let last = left.iter().rposition(|v| v.abs() > 1000.0).unwrap() - 50;
    let c = 2.0 * (2.0 * std::f64::consts::PI * f / 48_000.0).cos();
    // a sine obeys x[n+1] + x[n-1] = 2cos(w) x[n]; a dropped, repeated or zero-padded frame breaks it by hundreds
    let worst = (first + 1..last).map(|n| (left[n + 1] + left[n - 1] - c * left[n]).abs()).fold(0.0, f64::max);
    assert!(worst < 120.0, "the radio's waveform has a break of {worst}");
    assert!(last - first > 48_000 * 4, "{} frames of radio", last - first);
}

#[test]
fn the_heard_position_trails_the_mixed_audio_by_the_lead_and_stands_still_when_the_source_is_starved() {
    let clock = MixClock::manual();
    let card = CardSink::new(&clock);
    let m = manual(rt_cfg(15), Box::new(card.clone()), &clock);
    let w = m.open_source(SourceKind::Speech, 24_000, 1).unwrap();
    assert_eq!(w.write_i16(&vec![3000; 24_000 / 2]), 24_000 / 2); // 500 ms, then nothing (the stall)
    m.settle();
    run_for(&m, Duration::from_millis(150), STEP, |_| {});
    assert!(w.played_ms() <= 20, "the lead (300 ms of silence) is still in front of the card: {} ms", w.played_ms());
    run_for(&m, Duration::from_millis(500), STEP, |_| {});
    let mid = w.played_ms();
    assert!((100..=400).contains(&mid), "{mid} ms heard 650 ms after the start");
    run_for(&m, Duration::from_millis(900), STEP, |_| {});
    let a = w.played_ms();
    run_for(&m, Duration::from_millis(400), STEP, |_| {});
    let b = w.played_ms();
    assert_eq!(a, b, "no more is heard while the source is starved");
    assert!((480..=520).contains(&a), "{a} ms heard in the end (500 written)");
    m.shutdown();
}

/// A card that starts only when its 500 ms buffer is full is heard 200 ms later than the 300 ms lead says; the sink tells, and the
/// heard position (what the word highlight follows) holds it back by that much.
#[test]
fn the_heard_position_is_held_back_by_what_the_card_holds_beyond_the_lead() {
    // three cards run side by side from the same moment (each on its own clock, moved together) and are read at the same moment
    let lats = [0u32, 500, 200];
    let mixers: Vec<(Mixer, SourceWriter)> = lats
        .iter()
        .map(|lat| {
            let clock = MixClock::manual();
            let m = manual(rt_cfg(15), Box::new(CardSink::new(&clock).with_latency(*lat)), &clock);
            let w = m.open_source(SourceKind::Speech, 24_000, 1).unwrap();
            assert_eq!(w.write_i16(&vec![3000; 24_000 * 2]), 24_000 * 2);
            m.settle();
            (m, w)
        })
        .collect();
    // until the first one has been heard for a good while, then read all three together
    while mixers[0].1.played_ms() < 700 {
        assert!(mixers[0].0.clock().now() < Duration::from_secs(10), "nothing was heard");
        mixers.iter().for_each(|(m, _)| m.advance(STEP));
    }
    let heard: Vec<u64> = mixers.iter().map(|(_, w)| w.played_ms()).collect();
    for (m, _) in &mixers {
        m.shutdown();
    }
    let (plain, held, small) = (heard[0], heard[1], heard[2]);
    assert!((140..=260).contains(&(plain.saturating_sub(held))), "no latency told: heard {plain} ms; 500 ms told: {held} ms");
    // a card that holds less than the lead adds nothing
    assert!(small.abs_diff(plain) <= 40, "{small} vs {plain}");
}

/// Reading aloud, mixer on: each sentence is a fresh speech source handed over as soon as the last was mixed. The card is never
/// closed or starved between them (one sink open, no gap).
#[test]
fn sentence_hand_off_keeps_the_card_open_and_gapless() {
    let clock = MixClock::manual();
    let card = CardSink::new(&clock);
    let m = manual(rt_cfg(15), Box::new(card.clone()), &clock);
    for i in 0..4 {
        // what `play_wav` does: a source, the whole sentence, the end; the next sentence follows the moment this one was mixed
        let src = m.open_source_with(SourceKind::Speech, 24_000, 1, "speech", false).unwrap();
        assert_eq!(src.write_i16(&vec![2000 + i; 24_000 * 700 / 1000]), 24_000 * 700 / 1000);
        src.finish();
        let t0 = m.clock().now();
        while !src.is_done() {
            assert!(m.clock().now() - t0 < Duration::from_secs(10), "sentence {i} was never mixed");
            m.advance(STEP);
        }
    }
    done(&m);
    assert_eq!(card.gaps(), vec![]);
    assert_eq!(m.stats().sink_opens, 1, "one open for the whole reading");
    let s = card.samples();
    let heard = s.chunks(2).filter(|f| f[0] > 1900).count();
    assert!(heard >= 4 * 48_000 * 700 / 1000 - 3 * 48_000 / 100, "all four sentences reached the card ({heard} frames)");
}

/// The heard position moves like a clock: polled every 15 ms it never steps ahead or back by more than a period or two, so the
/// word highlight that follows it is not pushed by jumps of the position itself.
#[test]
fn the_heard_position_advances_smoothly_between_polls() {
    let clock = MixClock::manual();
    let card = CardSink::new(&clock);
    let m = manual(rt_cfg(15), Box::new(card.clone()), &clock);
    let w = m.open_source(SourceKind::Speech, 24_000, 1).unwrap();
    let mut feed = Topup { data: vec![3000; 24_000 * 3], pos: 0 };
    let (mut prev, mut prev_t) = (0u64, 0.0f64);
    let (mut worst_fwd, mut worst_back) = (0.0f64, 0.0f64);
    let mut started = false;
    let mut first = 0u64;
    let poll = Duration::from_millis(15);
    while m.clock().now() < Duration::from_millis(4200) {
        feed.tick(&w);
        if feed.done() {
            w.finish();
        }
        m.advance(poll);
        let (p, t) = (w.played_ms(), m.clock().now().as_secs_f64() * 1000.0);
        if p > 0 {
            if !started {
                first = p;
            }
            if started {
                let d = (p as f64 - prev as f64) - (t - prev_t);
                worst_fwd = worst_fwd.max(d);
                worst_back = worst_back.min(d);
            }
            started = true;
        }
        prev = p;
        prev_t = t;
    }
    m.shutdown();
    assert!(started);
    assert!(first <= 40, "the heard position starts at {first} ms instead of near 0: the first sound would be lit late");
    assert!(worst_fwd < 45.0 && worst_back > -45.0, "the heard position stepped {worst_fwd:.0} ms ahead and {worst_back:.0} ms back of the clock between polls (allowed 45 ms)");
}

/// The one test of the real clock: the thread paces itself on the wall clock, a source that is fed and finished is mixed to the end
/// and the card is opened once. Only the outcome is asserted, with a generous budget; how fast the thread ran is not.
#[test]
fn the_real_clock_mixes_a_finished_source_to_the_end() {
    let _rt = crate::mixer::rt_serial();
    let sink = FakeSink::new();
    let m = Mixer::start(MixerCfg { idle_ms: 20_000, raise_priority: false, ..MixerCfg::default() }, Box::new(sink.clone()));
    let w = m.open_source(SourceKind::Speech, 24_000, 1).unwrap();
    assert_eq!(w.write_i16(&vec![2000; 24_000 * 3 / 10]), 24_000 * 3 / 10);
    w.finish();
    iyakki_testkit::wait_until(iyakki_testkit::BUDGET, || w.is_done());
    iyakki_testkit::wait_until(iyakki_testkit::BUDGET, || sink.lock().samples.iter().filter(|&&x| x > 1900).count() >= 48_000 * 3 / 10 - 48_000 / 50);
    assert_eq!(sink.lock().opens, 1);
    m.shutdown();
}
