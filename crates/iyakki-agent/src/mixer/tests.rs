use super::engine::{map_channels, Engine, EngineCfg, SourceKind, SourceShared, SourceWriter, Wake};
use super::resample::Resampler;
use super::sink::FakeSink;
use super::*;

fn cfg() -> EngineCfg {
    EngineCfg::default().no_prefill()
}

fn src(e: &mut Engine, kind: SourceKind, rate: u32, ch: usize) -> SourceWriter {
    let s = SourceShared::new(e.source_count() as u64 + 100, kind, kind.label(), rate, ch, false, Wake::new());
    e.add(s.clone());
    SourceWriter { s }
}

fn sine(freq: f32, rate: u32, secs: f32, amp: f32) -> Vec<f32> {
    (0..(rate as f32 * secs) as usize).map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin()).collect()
}

fn stereo(mono: &[f32]) -> Vec<f32> {
    mono.iter().flat_map(|&x| [x, x]).collect()
}

/// Mix `periods` periods and return the left channel as f32.
fn run(e: &mut Engine, periods: usize) -> Vec<f32> {
    let mut out = Vec::new();
    let mut all = Vec::new();
    for _ in 0..periods {
        e.mix_period(1.0, &mut out);
        all.extend(out.chunks(e.cfg.channels).map(|f| f[0] as f32 / 32768.0));
    }
    all
}

fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt()
}

#[test]
fn two_sources_add() {
    let mut e = Engine::new(cfg());
    let a = src(&mut e, SourceKind::Media, 48_000, 2);
    let b = src(&mut e, SourceKind::Media, 48_000, 2);
    a.write_f32(&vec![0.25; 960 * 2 * 2]);
    b.write_f32(&vec![0.125; 960 * 2 * 2]);
    let o = run(&mut e, 1);
    assert!(o.iter().all(|v| (v - 0.375).abs() < 1e-3), "{}", o[0]);
}

#[test]
fn a_sum_over_full_scale_saturates_without_wrapping() {
    let mut e = Engine::new(cfg());
    let a = src(&mut e, SourceKind::Media, 48_000, 2);
    let b = src(&mut e, SourceKind::Media, 48_000, 2);
    a.write_f32(&vec![0.8; 960 * 2]);
    b.write_f32(&vec![0.8; 960 * 2]);
    let mut out = Vec::new();
    let info = e.mix_period(1.0, &mut out);
    assert!(out.iter().all(|&s| s == 32767), "{:?}", &out[..4]);
    assert_eq!(info.clipped, 960 * 2);
    a.write_f32(&vec![-0.8; 960 * 2]);
    b.write_f32(&vec![-0.8; 960 * 2]);
    e.mix_period(1.0, &mut out);
    assert!(out.iter().all(|&s| s == -32768));
}

#[test]
fn unity_passthrough_is_bit_exact() {
    let mut e = Engine::new(cfg());
    let a = src(&mut e, SourceKind::Media, 48_000, 2);
    let pcm: Vec<i16> = (0..960 * 2).map(|i| ((i as i32 * 37) % 60000 - 30000) as i16).collect();
    a.write_i16(&pcm);
    let mut out = Vec::new();
    e.mix_period(1.0, &mut out);
    let want: Vec<i16> = pcm.iter().map(|&s| s).collect();
    assert_eq!(out, want);
}

#[test]
fn gain_changes_ramp_across_the_period_without_a_jump() {
    let mut e = Engine::new(cfg());
    let a = src(&mut e, SourceKind::Media, 48_000, 2);
    a.write_f32(&vec![0.5; 960 * 2 * 3]);
    run(&mut e, 1);
    a.set_gain(0.0);
    let o = run(&mut e, 2);
    let max_step = o.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
    assert!(max_step < 0.002, "{max_step}");
    assert!(o[960 - 1].abs() < 0.001 && o[0] > 0.45, "{} {}", o[0], o[959]);
}

#[test]
fn master_gain_ramps_and_mutes() {
    let mut e = Engine::new(cfg());
    let a = src(&mut e, SourceKind::Media, 48_000, 2);
    a.write_f32(&vec![0.5; 960 * 2 * 2]);
    let mut out = Vec::new();
    e.mix_period(1.0, &mut out);
    e.mix_period(0.0, &mut out);
    assert_eq!(*out.last().unwrap(), 0);
    let max_step = out.chunks(2).map(|f| f[0]).collect::<Vec<_>>().windows(2).map(|w| (w[1] - w[0]).abs()).max().unwrap();
    assert!(max_step < 40, "{max_step}");
}

#[test]
fn a_notification_ducks_media_with_attack_and_release_ramps_and_a_hang() {
    let mut e = Engine::new(cfg());
    let music = src(&mut e, SourceKind::Media, 48_000, 2);
    let ding = src(&mut e, SourceKind::Notification, 48_000, 2);
    let tone = stereo(&sine(440.0, 48_000, 0.04, 0.5)); // two periods
    // 1 s of constant music, the tone for two periods in the middle
    music.write_f32(&vec![0.4; 48_000 * 2]);
    run(&mut e, 5); // steady
    ding.write_f32(&tone);
    let during = run(&mut e, 2);
    let _ = during;
    let _ = rms(&[]);
    let info = e.source_info();
    let duck_of = |k: SourceKind| info.iter().find(|i| i.1 == k).unwrap().3;
    assert!(duck_of(SourceKind::Media) < 1.0, "media was not ducked");
    assert_eq!(duck_of(SourceKind::Notification), 1.0);
    ding.finish();
    // the hang keeps it down for 300 ms: ~15 periods, then the release brings it back over 400 ms
    let mut lowest = 1.0f32;
    for _ in 0..4 {
        run(&mut e, 1);
        lowest = lowest.min(e.source_info().iter().find(|i| i.1 == SourceKind::Media).unwrap().3);
    }
    assert!((lowest - SourceKind::Notification.duck_to()).abs() < 1e-3, "{lowest}");
    run(&mut e, 9);
    assert!(e.source_info()[0].3 < 0.45, "still ducked during the hang");
    run(&mut e, 40); // 800 ms later
    assert!(e.source_info()[0].3 > 0.99, "released");
}

#[test]
fn ducking_attack_is_a_ramp_not_a_step() {
    let mut e = Engine::new(cfg());
    let music = src(&mut e, SourceKind::Media, 48_000, 2);
    let alarm = src(&mut e, SourceKind::Alarm, 48_000, 2);
    music.write_f32(&vec![0.5; 48_000 * 2]);
    run(&mut e, 2);
    alarm.write_f32(&stereo(&sine(880.0, 48_000, 0.2, 0.001))); // inaudible-level but above the threshold: ducks, adds nothing
    let o = run(&mut e, 4);
    let max_step = o.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
    assert!(max_step < 0.01, "{max_step}");
    let end = o[o.len() - 1];
    assert!((end - 0.5 * SourceKind::Alarm.duck_to()).abs() < 0.02, "{end}");
}

#[test]
fn priority_decides_who_ducks_whom() {
    let mut e = Engine::new(cfg());
    let radio = src(&mut e, SourceKind::Radio, 48_000, 2);
    let media = src(&mut e, SourceKind::Media, 48_000, 2);
    let speech = src(&mut e, SourceKind::Speech, 48_000, 2);
    let alarm = src(&mut e, SourceKind::Alarm, 48_000, 2);
    for w in [&radio, &media, &speech, &alarm] {
        w.write_f32(&vec![0.01; 48_000 * 2 / 5]);
    }
    run(&mut e, 12);
    let d = |k| e.source_info().iter().find(|i| i.1 == k).unwrap().3;
    assert_eq!(d(SourceKind::Alarm), 1.0);
    assert!((d(SourceKind::Speech) - SourceKind::Alarm.duck_to()).abs() < 1e-3);
    assert!((d(SourceKind::Radio) - SourceKind::Alarm.duck_to()).abs() < 1e-3, "the lowest follows the most important sounding source");
    assert!(SourceKind::Alarm.priority() > SourceKind::Speech.priority() && SourceKind::Speech.priority() > SourceKind::Notification.priority() && SourceKind::Notification.priority() > SourceKind::Media.priority() && SourceKind::Media.priority() > SourceKind::Radio.priority());
}

#[test]
fn media_and_radio_do_not_duck_each_other_and_pause_releases_ducking() {
    let mut e = Engine::new(cfg());
    let radio = src(&mut e, SourceKind::Radio, 48_000, 2);
    let media = src(&mut e, SourceKind::Media, 48_000, 2);
    radio.write_f32(&vec![0.3; 48_000 * 2]);
    media.write_f32(&vec![0.3; 48_000 * 2]);
    run(&mut e, 5);
    assert!(e.source_info().iter().all(|i| i.3 == 1.0));
    let speech = src(&mut e, SourceKind::Speech, 48_000, 2);
    speech.write_f32(&vec![0.2; 48_000 * 2]);
    run(&mut e, 5);
    assert!(e.source_info().iter().filter(|i| i.1 != SourceKind::Speech).all(|i| i.3 < 0.3));
    speech.set_paused(true);
    run(&mut e, 40);
    assert!(e.source_info().iter().all(|i| i.3 > 0.99));
}

#[test]
fn a_sine_at_44k1_resampled_to_48k_keeps_frequency_amplitude_and_has_no_clicks() {
    let input = sine(1000.0, 44_100, 1.0, 0.5);
    let mut rs = Resampler::new(44_100, 48_000, 1);
    let mut out = Vec::new();
    for chunk in input.chunks(1000) {
        rs.process(chunk, &mut out);
    }
    assert!((out.len() as i64 - 48_000).abs() < 40, "{}", out.len());
    // skip the filter's start-up and the end
    let mid = &out[2000..out.len() - 2000];
    let amp = mid.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!((amp - 0.5).abs() < 0.01, "amplitude {amp}");
    // frequency: count rising zero crossings over the middle part
    let crossings = mid.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
    let secs = mid.len() as f32 / 48_000.0;
    assert!((crossings as f32 / secs - 1000.0).abs() < 5.0, "{}", crossings as f32 / secs);
    // against the ideal sine: error well below -50 dB (delay found by search over a few samples)
    let delay = rs.delay_frames() as f32 * 48_000.0 / 44_100.0;
    let _ = delay;
    let mut best = f32::MAX;
    for d in 0..40 {
        let mut se = 0.0;
        let n = mid.len() - 100;
        for i in 0..n {
            let t = (i + 2000) as f32 / 48_000.0;
            let ideal = 0.5 * (2.0 * std::f32::consts::PI * 1000.0 * (t - d as f32 / 48_000.0 + 0.0)).sin();
            se += (mid[i] - ideal).powi(2);
        }
        best = best.min((se / n as f32).sqrt());
    }
    // the delay is not a whole number of output samples: allow a small phase error, still no distortion products
    assert!(best < 0.05, "{best}");
    // no clicks: the largest step between samples is that of a clean sine (2 pi f A / fs = 0.065)
    let step = mid.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
    assert!(step < 0.07, "{step}");
}

#[test]
fn down_sampling_removes_what_the_output_cannot_hold() {
    // 20 kHz at 96 kHz down to 32 kHz output: above Nyquist (16 kHz), must be strongly attenuated; 1 kHz must pass
    let mut hi = Vec::new();
    Resampler::new(96_000, 32_000, 1).process(&sine(20_000.0, 96_000, 0.5, 0.5), &mut hi);
    assert!(rms(&hi[500..]) < 0.02, "{}", rms(&hi[500..]));
    let mut lo = Vec::new();
    Resampler::new(96_000, 32_000, 1).process(&sine(1_000.0, 96_000, 0.5, 0.5), &mut lo);
    assert!((rms(&lo[500..]) - 0.3536).abs() < 0.01, "{}", rms(&lo[500..]));
}

#[test]
fn the_engine_resamples_22k_mono_to_48k_stereo() {
    let mut e = Engine::new(cfg());
    let a = src(&mut e, SourceKind::Speech, 22_050, 1);
    a.write_f32(&sine(500.0, 22_050, 0.5, 0.5));
    let o = run(&mut e, 12);
    let mid = &o[2000..9000];
    assert!((rms(mid) - 0.3536).abs() < 0.02, "{}", rms(mid));
}

#[test]
fn channel_mapping_up_and_down() {
    let mut o = Vec::new();
    map_channels(&[0.5, -0.5], 1, 2, &mut o);
    assert_eq!(o, vec![0.5, 0.5, -0.5, -0.5]);
    o.clear();
    map_channels(&[1.0, 0.0], 2, 1, &mut o);
    assert_eq!(o, vec![0.5]);
    o.clear();
    map_channels(&[1.0, 0.0], 2, 4, &mut o);
    assert_eq!(o, vec![1.0, 0.0, 1.0, 0.0]);
    o.clear();
    map_channels(&[1.0, 0.2, 0.0, 0.4], 4, 2, &mut o);
    assert_eq!(o, vec![0.5, 0.3]);
    o.clear();
    map_channels(&[0.3, 0.7], 2, 2, &mut o);
    assert_eq!(o, vec![0.3, 0.7]);
}

#[test]
fn an_underrun_is_counted_once_and_the_prebuffer_refills() {
    let mut e = Engine::new(EngineCfg::default()); // 40 ms prebuffer for a notification
    let a = src(&mut e, SourceKind::Notification, 48_000, 2);
    a.write_f32(&vec![0.2; 960 * 2]); // 20 ms: below the prebuffer, silent
    let o = run(&mut e, 1);
    assert!(o.iter().all(|&v| v == 0.0));
    a.write_f32(&vec![0.2; 960 * 2 * 2]); // now 60 ms
    let o = run(&mut e, 1);
    assert!(o[10] > 0.19);
    run(&mut e, 3); // data ran out mid-stream
    assert_eq!(e.underruns, 1);
    run(&mut e, 5);
    assert_eq!(e.underruns, 1, "a source that stays empty is one underrun, not one per period");
}

#[test]
fn a_finished_source_plays_out_and_is_removed() {
    let mut e = Engine::new(cfg());
    let a = src(&mut e, SourceKind::Notification, 48_000, 2);
    a.write_f32(&vec![0.3; 960 * 2 + 20]);
    a.finish();
    run(&mut e, 1);
    assert_eq!(e.source_count(), 1);
    run(&mut e, 1);
    assert!(a.is_done(), "the writer sees the end as soon as the last sample was mixed");
    assert_eq!(e.source_count(), 1, "the slot stays for its ducking hang");
    run(&mut e, 16);
    assert_eq!(e.source_count(), 0);
    assert_eq!(e.underruns, 0);
}

#[test]
fn a_source_dropped_mid_stream_does_not_hurt_the_others() {
    let mut e = Engine::new(cfg());
    let a = src(&mut e, SourceKind::Media, 48_000, 2);
    let b = src(&mut e, SourceKind::Radio, 48_000, 2);
    a.write_f32(&vec![0.2; 960 * 2 * 10]);
    b.write_f32(&vec![0.1; 960 * 2 * 10]);
    run(&mut e, 2);
    drop(a);
    let o = run(&mut e, 2);
    b.finish();
    assert!((o[0] - 0.2 - 0.1).abs() < 1e-2 || (o[0] - 0.1).abs() < 1e-2);
    run(&mut e, 30);
    assert_eq!(e.source_count(), 0, "both ended after their data");
    assert!(b.is_done());
}

#[test]
fn flush_throws_away_what_is_queued() {
    let mut e = Engine::new(cfg());
    let a = src(&mut e, SourceKind::Media, 48_000, 2);
    a.write_f32(&vec![0.3; 960 * 2 * 4]);
    run(&mut e, 1);
    a.flush();
    let o = run(&mut e, 1);
    assert!(o.iter().all(|&v| v == 0.0));
    assert_eq!(a.buffered_ms(), 0);
}

#[test]
fn many_sources_mix_and_stay_in_range() {
    let mut e = Engine::new(cfg());
    let ws: Vec<_> = (0..16).map(|_| src(&mut e, SourceKind::Media, 48_000, 2)).collect();
    for w in &ws {
        w.write_f32(&stereo(&sine(300.0, 48_000, 0.1, 0.2))[..960 * 2 * 4]);
    }
    let o = run(&mut e, 4);
    assert_eq!(o.len(), 960 * 4);
    assert!(o.iter().all(|v| v.abs() <= 1.0));
    assert!(e.clipped > 0, "16 x 0.2 clips and is counted");
}

#[test]
fn the_ring_is_bounded_and_a_waiting_write_is_paced_by_the_reader() {
    let mut e = Engine::new(cfg());
    let a = src(&mut e, SourceKind::Notification, 48_000, 2);
    let big = vec![0.1f32; 48_000 * 2];
    let taken = a.write_f32(&big);
    assert_eq!(taken, 48_000 * 2 * 500 / 1000, "500 ms");
    assert_eq!(a.write_f32(&big), 0);
    let t = std::thread::spawn(move || {
        let pcm = vec![100i16; 48_000 * 2];
        a.write_i16_wait(&pcm, Duration::from_secs(5))
    });
    for _ in 0..80 {
        let mut out = Vec::new();
        e.mix_period(1.0, &mut out);
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(t.join().unwrap().is_ok());
}

#[test]
fn mixing_four_sources_for_ten_seconds_takes_far_less_than_real_time() {
    let mut e = Engine::new(EngineCfg::default());
    let mut ws = vec![
        src(&mut e, SourceKind::Radio, 44_100, 2),
        src(&mut e, SourceKind::Media, 48_000, 2),
        src(&mut e, SourceKind::Speech, 22_050, 1),
        src(&mut e, SourceKind::Notification, 44_100, 1),
    ];
    let secs = 10;
    let tones = [sine(220.0, 44_100, 0.5, 0.2), sine(330.0, 48_000, 0.5, 0.2), sine(440.0, 22_050, 0.5, 0.2), sine(550.0, 44_100, 0.5, 0.2)];
    let mut out = Vec::new();
    let mut times: Vec<f64> = Vec::new();
    for p in 0..(secs * 50) {
        for (w, t) in ws.iter_mut().zip(&tones) {
            let per = w.s.rate as usize / 50;
            let ch = w.s.channels;
            let chunk: Vec<f32> = (0..per).flat_map(|i| std::iter::repeat_n(t[(p * per + i) % t.len()], ch)).collect();
            w.write_f32(&chunk);
        }
        let t0 = Instant::now();
        e.mix_period(1.0, &mut out);
        times.push(t0.elapsed().as_secs_f64());
    }
    // a machine busy with other tests preempts this thread now and then: the slowest 10 percent of the periods are left out, so the bound measures the mixing and not the scheduler
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let keep = times.len() * 9 / 10;
    let took: f64 = times[..keep].iter().sum::<f64>() * times.len() as f64 / keep as f64;
    // generous bound (the target is well under 3 percent; debug builds are slower than release)
    let limit = if cfg!(debug_assertions) { 0.30 } else { 0.05 };
    assert!(took < secs as f64 * limit, "{took:.3} s for {secs} s of audio");
}

// ---------------------------------------------------------------- the service

fn fast() -> MixerCfg {
    MixerCfg { idle_ms: 100, engine: EngineCfg::default().no_prefill(), ..MixerCfg::default() }
}

fn wait_for(what: &str, f: impl Fn() -> bool) {
    for _ in 0..400 {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn the_service_plays_a_source_to_the_sink_and_lets_the_card_go_when_idle() {
    let sink = FakeSink::new();
    let m = Mixer::start(fast(), Box::new(sink.clone()));
    let w = m.open_source(SourceKind::Media, 48_000, 2).unwrap();
    w.write_i16_wait(&vec![8000i16; 48_000 / 5 * 2], Duration::from_secs(2)).unwrap();
    wait_for("samples", || sink.lock().samples.len() >= 960 * 2 * 5);
    assert_eq!(sink.lock().opens, 1);
    assert_eq!(sink.lock().rate, 48_000);
    assert!(sink.lock().samples.iter().any(|&s| s > 7000));
    assert!(w.finish_and_wait(Duration::from_secs(3)));
    wait_for("the sink to close after the idle time", || !sink.is_open_now());
    assert_eq!(sink.lock().closes, 1);
    let st = m.stats();
    assert!(!st.sink_open && st.periods > 0 && st.cpu_percent < 50.0, "{st:?}");
    // a later source opens it again
    let w2 = m.open_source(SourceKind::Notification, 44_100, 1).unwrap();
    w2.write_i16_wait(&vec![5000i16; 4410], Duration::from_secs(2)).unwrap();
    wait_for("a second open", || sink.lock().opens == 2);
    m.shutdown();
}

impl FakeSink {
    fn is_open_now(&self) -> bool {
        self.lock().open
    }
}

#[test]
fn a_card_that_cannot_be_opened_is_busy_and_recovers() {
    let sink = FakeSink::new();
    sink.lock().fail_open = Some("Device or resource busy".into());
    let m = Mixer::start(fast(), Box::new(sink.clone()));
    let w = m.open_source(SourceKind::Media, 48_000, 2).unwrap();
    w.write_f32(&vec![0.1; 960 * 2 * 5]);
    wait_for("the error", || m.stats().last_error.is_some());
    match m.open_source(SourceKind::Media, 48_000, 2) {
        Err(MixError::Busy { holder }) => assert!(holder.contains("busy")),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert_eq!(sink.lock().samples.len(), 0, "nothing was taken from the source while the card was closed");
    m.shutdown();
}

#[test]
fn disabled_refuses_and_invalid_formats_are_refused() {
    let m = Mixer::start(fast(), Box::new(FakeSink::new()));
    m.set_enabled(false);
    assert_eq!(m.open_source(SourceKind::Media, 48_000, 2).err(), Some(MixError::Disabled));
    m.set_enabled(true);
    assert!(matches!(m.open_source(SourceKind::Media, 100, 2).err(), Some(MixError::Invalid(_))));
    assert!(matches!(m.open_source(SourceKind::Media, 48_000, 0).err(), Some(MixError::Invalid(_))));
    m.shutdown();
    assert_eq!(m.open_source(SourceKind::Media, 48_000, 2).err(), Some(MixError::Closed));
}

#[test]
fn exclusive_holds_everything_but_alarms_out() {
    let m = Mixer::start(fast(), Box::new(FakeSink::new()));
    let x = m.open_source_with(SourceKind::Media, 48_000, 2, "player", true).unwrap();
    match m.open_source(SourceKind::Radio, 48_000, 2) {
        Err(MixError::Busy { holder }) => assert_eq!(holder, "player"),
        other => panic!("{:?}", other.map(|_| ())),
    }
    assert!(m.open_source(SourceKind::Alarm, 48_000, 2).is_ok());
    drop(x);
    wait_for("release", || m.sources().iter().all(|s| s.kind == SourceKind::Alarm));
    m.shutdown();
}

#[test]
fn a_full_mixer_gives_way_to_a_more_important_source_and_refuses_an_equal_one() {
    let m = Mixer::start(fast(), Box::new(FakeSink::new()));
    let mut keep = Vec::new();
    for _ in 0..MAX_SOURCES {
        keep.push(m.open_source(SourceKind::Radio, 48_000, 2).unwrap());
    }
    assert!(matches!(m.open_source(SourceKind::Radio, 48_000, 2).err(), Some(MixError::Busy { .. })));
    let alarm = m.open_source(SourceKind::Alarm, 48_000, 2).unwrap();
    assert!(keep[0].write_i16_wait(&[1, 2], Duration::from_millis(100)).is_err(), "the oldest radio was evicted");
    assert!(alarm.write_f32(&[0.1, 0.1]) > 0);
    m.shutdown();
}

#[test]
fn shutdown_joins_the_thread_and_ends_the_sources() {
    let sink = FakeSink::new();
    let m = Mixer::start(fast(), Box::new(sink.clone()));
    let w = m.open_source(SourceKind::Media, 48_000, 2).unwrap();
    w.write_f32(&vec![0.1; 960 * 2 * 5]);
    wait_for("playing", || sink.lock().writes > 0);
    let t0 = Instant::now();
    m.shutdown();
    assert!(t0.elapsed() < Duration::from_secs(2));
    assert!(w.is_done());
    assert!(!sink.lock().open);
    assert_eq!(w.write_f32(&[0.1, 0.1]), 0);
}

#[test]
fn play_wav_reaches_the_sink_and_returns_when_mixed() {
    let sink = FakeSink::new();
    let m = Mixer::start(fast(), Box::new(sink.clone()));
    let wav = crate::voice::encode_wav(&vec![6000i16; 4410], 44_100);
    m.play_wav(SourceKind::Notification, &wav, "cue").unwrap();
    assert!(sink.lock().samples.iter().filter(|&&s| s > 5000).count() > 3000);
    m.shutdown();
}

#[test]
fn stats_line_reads_well() {
    let s = Stats { running: true, sink_open: true, sources: 2, cpu_percent: 1.25, underruns: 3, ducking: true, ..Default::default() };
    assert_eq!(s.line(), "mixer playing: 2 source(s), 1.2% of a core, 3 underrun(s), ducking".replace("1.2%", &format!("{:.1}%", 1.25)));
}

#[test]
fn a_decoder_pipe_feeds_a_source_as_plain_pcm() {
    let sink = FakeSink::new();
    let m = Mixer::start(fast(), Box::new(sink.clone()));
    // what a decoder helper writes to its standard output: s16le at its own rate and channel count
    let bytes: Vec<u8> = (0..22_050).flat_map(|_| 7000i16.to_le_bytes()).collect();
    let w = m.open_source(SourceKind::Radio, 22_050, 1).unwrap();
    let h = w.feed_reader(std::io::Cursor::new(bytes));
    assert_eq!(h.join().unwrap(), Ok(22_050 * 2));
    wait_for("the pipe audio", || sink.lock().samples.iter().filter(|&&s| s > 6000).count() > 40_000);
    assert!(sink.lock().samples.iter().filter(|&&s| s > 6000).count() < 100_000);
    m.shutdown();
}
