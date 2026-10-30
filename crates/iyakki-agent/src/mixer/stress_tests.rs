//! Real-time tests of the service with a fake card that blocks like one and records the gaps in what it was given.

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

/// A card: it plays what it was given at the real-time rate from a buffer of `cap`, a write blocks while the buffer is full,
/// and a write that finds the buffer empty is a gap (the card underran).
#[derive(Clone)]
struct CardSink {
    log: Arc<Mutex<CardLog>>,
    ends: Arc<Mutex<Instant>>,
    cap: Duration,
    /// What the card holds before the speaker when full, as the sink reports it (ms).
    lat: u32,
}

impl CardSink {
    fn new(cap_ms: u64) -> CardSink {
        CardSink { log: Arc::default(), ends: Arc::new(Mutex::new(Instant::now())), cap: Duration::from_millis(cap_ms), lat: 0 }
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
        *self.ends.lock().unwrap() = Instant::now();
        Ok(())
    }
    fn write(&mut self, pcm: &[i16]) -> Result<(), String> {
        let dur = Duration::from_micros(pcm.len() as u64 / 2 * 1_000_000 / 48_000);
        let end = *self.ends.lock().unwrap();
        // block while the card's buffer cannot take this period
        let room_at = (end + dur).checked_sub(self.cap);
        if let Some(t) = room_at {
            let now = Instant::now();
            if t > now {
                std::thread::sleep(t - now);
            }
        }
        let now = Instant::now();
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

/// A live stream: audio of constant value `v` that "arrives" in real time but in bursts after 0..=`max_gap_ms` of nothing.
fn live(w: SourceWriter, rate: u32, ch: usize, v: i16, secs: f64, max_gap_ms: u64, mut seed: u64) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let t0 = Instant::now();
        let mut sent = 0usize;
        while t0.elapsed().as_secs_f64() < secs {
            std::thread::sleep(Duration::from_millis(rng(&mut seed) % (max_gap_ms + 1)));
            let due = (t0.elapsed().as_secs_f64().min(secs) * rate as f64) as usize;
            if due > sent {
                if w.write_i16_wait(&vec![v; (due - sent) * ch], Duration::from_secs(10)).is_err() {
                    return;
                }
                sent = due;
            }
        }
        w.finish();
    })
}

fn rt_cfg(lead: u32) -> MixerCfg {
    MixerCfg { idle_ms: 20_000, lead_periods: lead, raise_priority: false, ..MixerCfg::default() }
}

fn done(m: &Mixer) {
    std::thread::sleep(Duration::from_millis(700));
    m.shutdown();
}

#[test]
fn four_jittery_sources_within_their_budgets_give_the_card_no_gap() {
    let card = CardSink::new(500);
    let m = Mixer::start(rt_cfg(15), Box::new(card.clone()));
    let mut hs = vec![];
    // radio, radio, media: up to 300 ms without data (budget 800 ms); speech: up to 150 ms (budget 250 ms)
    for (kind, rate, ch, v, gap, seed) in [(SourceKind::Radio, 44_100, 2, 1000, 300, 7), (SourceKind::Radio, 48_000, 2, 2000, 300, 11), (SourceKind::Media, 22_050, 1, 3000, 300, 13), (SourceKind::Speech, 24_000, 1, 500, 150, 17)] {
        let w = m.open_source(kind, rate, ch).unwrap();
        hs.push(live(w, rate, ch, v, 4.0, gap, seed));
    }
    for h in hs {
        h.join().unwrap();
    }
    done(&m);
    let st = m.stats();
    assert_eq!(card.gaps(), vec![], "the card ran dry");
    assert_eq!(st.underruns, 0, "{st:?}");
    assert_eq!(st.sink_late, 0, "{st:?}");
    let s = card.samples();
    assert!(s.len() > 48_000 * 2 * 4);
    assert!(s.iter().all(|&x| (0..=6500).contains(&x)), "garbage outside the sum of the sources");
}

#[test]
fn a_source_past_its_budget_goes_silent_and_does_not_hurt_the_others_or_the_card() {
    let card = CardSink::new(500);
    let m = Mixer::start(rt_cfg(15), Box::new(card.clone()));
    let media = m.open_source(SourceKind::Media, 48_000, 2).unwrap();
    let h1 = live(media, 48_000, 2, 3000, 4.5, 20, 3);
    let radio = m.open_source(SourceKind::Radio, 48_000, 2).unwrap();
    let h2 = std::thread::spawn(move || {
        let t0 = Instant::now();
        let mut sent = 0usize;
        // 1.5 s of stream, a 1.5 s hole (past the 800 ms budget), then it carries on in real time
        while t0.elapsed() < Duration::from_millis(4_300) {
            let el = t0.elapsed().as_secs_f64();
            if !(1.5..3.0).contains(&el) {
                let due = ((if el >= 3.0 { el - 1.5 } else { el }) * 48_000.0) as usize;
                if due > sent {
                    let _ = radio.write_i16_wait(&vec![1000; (due - sent) * 2], Duration::from_secs(5));
                    sent = due;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        radio.finish();
    });
    h1.join().unwrap();
    h2.join().unwrap();
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

fn stalled_run(lead: u32) -> (Vec<(u64, usize)>, Stats) {
    let card = CardSink::new(500);
    let m = Mixer::start(rt_cfg(lead), Box::new(card.clone()));
    let calls = Arc::new(AtomicU64::new(0));
    let c2 = calls.clone();
    m.set_master_fn(move || {
        if c2.fetch_add(1, Ordering::SeqCst) == 100 {
            std::thread::sleep(Duration::from_millis(100)); // the mixer thread is not scheduled for 100 ms
        }
        1.0
    });
    let w = m.open_source(SourceKind::Media, 48_000, 2).unwrap();
    w.write_i16_wait(&vec![3000; 48_000 * 2 * 4], Duration::from_secs(5)).unwrap();
    w.finish();
    std::thread::sleep(Duration::from_millis(4_200));
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
    let sink = FakeSink::new();
    let m = Mixer::start(MixerCfg { idle_ms: 5000, raise_priority: false, ..MixerCfg::default() }, Box::new(sink.clone()));
    let w = m.open_source(SourceKind::Media, 48_000, 2).unwrap();
    w.write_i16_wait(&vec![1000; 48_000 * 2 * 3], Duration::from_secs(5)).unwrap();
    std::thread::sleep(Duration::from_millis(150));
    let n = sink.lock().writes;
    assert!((15..=26).contains(&n), "{n} periods after the first 150 ms (the lead is 15)");
    m.shutdown();
}

#[test]
fn a_source_still_filling_its_buffer_is_silent_and_does_not_open_the_card() {
    let sink = FakeSink::new();
    let m = Mixer::start(MixerCfg { raise_priority: false, ..MixerCfg::default() }, Box::new(sink.clone()));
    let w = m.open_source(SourceKind::Radio, 48_000, 2).unwrap();
    w.write_i16_wait(&vec![1000; 48_000 * 2 / 2], Duration::from_secs(5)).unwrap(); // 500 ms of 800
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(sink.lock().opens, 0);
    w.write_i16_wait(&vec![1000; 48_000 * 2 / 2], Duration::from_secs(5)).unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(sink.lock().opens, 1);
    m.shutdown();
}

/// The owner heard breaks only with two sources (radio and reading aloud): a continuous radio must stay one unbroken waveform
/// while a voice-like source starts and stops again and again.
#[test]
fn a_radio_stays_one_unbroken_waveform_while_voice_bursts_come_and_go() {
    let card = CardSink::new(500);
    let m = Mixer::start(rt_cfg(15), Box::new(card.clone()));
    let (rate, f) = (44_100u32, 440.0f64);
    let radio = m.open_source(SourceKind::Radio, rate, 2).unwrap();
    let sine: Vec<i16> = (0..rate as usize * 6).flat_map(|i| { let v = (16000.0 * (2.0 * std::f64::consts::PI * f * i as f64 / rate as f64).sin()) as i16; [v, v] }).collect();
    let rh = std::thread::spawn(move || {
        radio.write_i16_wait(&sine, Duration::from_secs(10)).unwrap();
        radio.finish();
    });
    std::thread::sleep(Duration::from_millis(1000)); // the radio's jitter buffer fills
    for _ in 0..7 {
        // a sentence: a fresh speech source of 300 ms (24 kHz mono, just audible so that it ducks the radio), then 200 ms of nothing
        let v = m.open_source(SourceKind::Speech, 24_000, 1).unwrap();
        v.write_i16_wait(&vec![40; 24_000 * 3 / 10], Duration::from_secs(5)).unwrap();
        v.finish_and_wait(Duration::from_secs(5));
        std::thread::sleep(Duration::from_millis(200));
    }
    rh.join().unwrap();
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
    let card = CardSink::new(500);
    let m = Mixer::start(rt_cfg(15), Box::new(card.clone()));
    let w = m.open_source(SourceKind::Speech, 24_000, 1).unwrap();
    w.write_i16_wait(&vec![3000; 24_000 / 2], Duration::from_secs(5)).unwrap(); // 500 ms, then nothing (the stall)
    std::thread::sleep(Duration::from_millis(150));
    assert!(w.played_ms() <= 20, "the lead (300 ms of silence) is still in front of the card: {} ms", w.played_ms());
    std::thread::sleep(Duration::from_millis(500));
    let mid = w.played_ms();
    assert!((100..=400).contains(&mid), "{mid} ms heard 650 ms after the start");
    std::thread::sleep(Duration::from_millis(900));
    let a = w.played_ms();
    std::thread::sleep(Duration::from_millis(400));
    let b = w.played_ms();
    assert_eq!(a, b, "no more is heard while the source is starved");
    assert!((480..=520).contains(&a), "{a} ms heard in the end (500 written)");
    m.shutdown();
}

/// A card that starts only when its 500 ms buffer is full is heard 200 ms later than the 300 ms lead says; the sink tells, and the
/// heard position (what the word highlight follows) holds it back by that much.
#[test]
fn the_heard_position_is_held_back_by_what_the_card_holds_beyond_the_lead() {
    let at = |lat: u32| {
        let m = Mixer::start(rt_cfg(15), Box::new(CardSink::new(500).with_latency(lat)));
        let w = m.open_source(SourceKind::Speech, 24_000, 1).unwrap();
        w.write_i16_wait(&vec![3000; 24_000 * 2], Duration::from_secs(5)).unwrap();
        std::thread::sleep(Duration::from_millis(1000));
        let p = w.played_ms();
        m.shutdown();
        p
    };
    let (plain, held) = (at(0), at(500));
    assert!((140..=260).contains(&(plain.saturating_sub(held))), "no latency told: heard {plain} ms; 500 ms told: {held} ms");
    // a card that holds less than the lead adds nothing
    let small = at(200);
    assert!(small.abs_diff(plain) <= 40, "{small} vs {plain}");
}

/// Reading aloud, mixer on: each sentence is a fresh speech source handed over as soon as the last was mixed. The card is never
/// closed or starved between them (one sink open, no gap).
#[test]
fn sentence_hand_off_keeps_the_card_open_and_gapless() {
    let card = CardSink::new(500);
    let m = Mixer::start(rt_cfg(15), Box::new(card.clone()));
    let wav = |ms: usize, v: i16| {
        let n = 24_000 * ms / 1000;
        let mut b = Vec::new();
        b.extend_from_slice(b"RIFF");
        b.extend_from_slice(&((36 + n * 2) as u32).to_le_bytes());
        b.extend_from_slice(b"WAVEfmt ");
        for x in [16u32, 0x0001_0001, 24_000, 48_000, 0x0010_0002] {
            b.extend_from_slice(&x.to_le_bytes());
        }
        b.extend_from_slice(b"data");
        b.extend_from_slice(&((n * 2) as u32).to_le_bytes());
        (0..n).for_each(|_| b.extend_from_slice(&v.to_le_bytes()));
        b
    };
    for i in 0..4 {
        m.play_wav(SourceKind::Speech, &wav(700, 2000 + i), "speech").unwrap();
    }
    std::thread::sleep(Duration::from_millis(500));
    m.shutdown();
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
    let card = CardSink::new(500);
    let m = Mixer::start(rt_cfg(15), Box::new(card.clone()));
    let w = m.open_source(SourceKind::Speech, 24_000, 1).unwrap();
    let feeder = {
        let w2 = w.clone_handle();
        std::thread::spawn(move || {
            w2.write_i16_wait(&vec![3000; 24_000 * 3], Duration::from_secs(10)).unwrap();
            w2.finish();
        })
    };
    let t0 = Instant::now();
    let (mut prev, mut prev_t) = (0u64, 0.0f64);
    let (mut worst_fwd, mut worst_back) = (0.0f64, 0.0f64);
    let mut started = false;
    let mut first = 0u64;
    while t0.elapsed() < Duration::from_millis(4200) {
        std::thread::sleep(Duration::from_millis(15));
        let (p, t) = (w.played_ms(), t0.elapsed().as_secs_f64() * 1000.0);
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
    feeder.join().unwrap();
    m.shutdown();
    assert!(started);
    assert!(first <= 40, "the heard position starts at {first} ms instead of near 0: the first sound would be lit late");
    assert!(worst_fwd < 45.0 && worst_back > -45.0, "the heard position stepped {worst_fwd:.0} ms ahead and {worst_back:.0} ms back of the wall clock between polls");
}
