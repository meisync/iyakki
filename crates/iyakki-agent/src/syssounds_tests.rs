use super::*;
use std::sync::Mutex;
use std::time::Instant;

/// A made-up output: records what it was given, takes `delay` to "play" it, and may have no output at all.
struct Fake {
    delay: Duration,
    ready: Result<(), String>,
    played: Mutex<Vec<usize>>,
    fail: Option<String>,
}

impl Fake {
    fn new(delay_ms: u64) -> Arc<Fake> {
        Arc::new(Fake { delay: Duration::from_millis(delay_ms), ready: Ok(()), played: Mutex::new(vec![]), fail: None })
    }
}

impl CueOut for Fake {
    fn ready(&self) -> Result<(), String> {
        self.ready.clone()
    }
    fn play(&self, wav: &[u8]) -> Result<(), String> {
        std::thread::sleep(self.delay);
        self.played.lock().unwrap().push(wav.len());
        self.fail.clone().map_or(Ok(()), Err)
    }
}

fn peak(p: &[i16]) -> i32 {
    p.iter().map(|s| (*s as i32).abs()).max().unwrap_or(0)
}

#[test]
fn every_cue_is_short_quiet_unclipped_and_starts_and_ends_at_zero() {
    for c in Cue::ALL {
        let pcm = render(c, DEFAULT_OFFSET_DB);
        let ms = pcm.len() as u32 * 1000 / RATE;
        assert!((250..=600).contains(&ms), "{c:?} is {ms} ms");
        assert_eq!(duration_ms(c) / 10, ms / 10, "{c:?}");
        assert_eq!((pcm[0], *pcm.last().unwrap()), (0, 0), "{c:?} starts and ends at zero");
        // the last 5 ms are already tiny: the envelope fades out, it does not just stop
        let tail = &pcm[pcm.len() - (RATE as usize / 200)..];
        assert!(peak(tail) < 120, "{c:?} tail peak {}", peak(tail));
        // 12 dB under the cue peak (0.6 of full scale): about 4,900, far from clipping
        let p = peak(&pcm);
        assert!((3500..=5500).contains(&p), "{c:?} peak {p}");
        assert!(pcm.iter().all(|s| *s != i16::MIN && *s != i16::MAX), "{c:?} never clips");
        // it makes sound all the way through (not a click)
        assert!(pcm.iter().filter(|s| s.abs() > 200).count() > pcm.len() / 6, "{c:?}");
    }
}

#[test]
fn the_offset_scales_the_level_and_is_kept_in_range() {
    let loud = peak(&render(Cue::Login, 0));
    let mid = peak(&render(Cue::Login, -12));
    let soft = peak(&render(Cue::Login, -24));
    assert!((loud as f32 - 0.6 * 32767.0).abs() < 40.0, "{loud}");
    assert!((mid as f32 / loud as f32 - 0.251).abs() < 0.01, "-12 dB is a quarter");
    assert!((soft as f32 / mid as f32 - 0.251).abs() < 0.01);
    // outside the range is clamped: a cue is never louder than IYAKKI's volume
    assert_eq!(peak(&render(Cue::Login, 20)), loud);
    assert_eq!(peak(&render(Cue::Login, -90)), peak(&render(Cue::Login, MIN_OFFSET_DB)));
    let w = wav(Cue::Logout, -12);
    let parsed = voice::parse_wav(&w).unwrap();
    assert_eq!(parsed.rate, RATE);
}

#[test]
fn login_rises_and_logout_falls() {
    // the pitch of the first and last quarter, from zero crossings
    fn hz(p: &[i16]) -> f32 {
        let z = p.windows(2).filter(|w| (w[0] < 0) != (w[1] < 0)).count();
        z as f32 / 2.0 / (p.len() as f32 / RATE as f32)
    }
    for (c, rising) in [(Cue::Login, true), (Cue::Logout, false)] {
        let p = render(c, -12);
        let (a, b) = (hz(&p[..p.len() / 5]), hz(&p[p.len() * 3 / 4..]));
        assert_eq!(b > a, rising, "{c:?}: {a} then {b}");
    }
}

#[test]
fn the_defaults_load_from_an_old_settings_file_and_the_range_is_checked() {
    let s: SystemSounds = serde_json::from_str("{}").unwrap();
    assert_eq!(s, SystemSounds::default());
    assert!(s.enabled && s.offset_db == -12 && Cue::ALL.iter().all(|c| s.allows(*c)));
    let partial: SystemSounds = serde_json::from_str(r#"{"logout":false}"#).unwrap();
    assert!(!partial.allows(Cue::Logout) && partial.allows(Cue::Login) && partial.enabled);
    let off = SystemSounds { enabled: false, ..Default::default() };
    assert!(Cue::ALL.iter().all(|c| !off.allows(*c)));
    assert!(SystemSounds { offset_db: 3, ..Default::default() }.validate().is_err());
    assert!(SystemSounds { offset_db: -31, ..Default::default() }.validate().is_err());
    assert!(SystemSounds::default().validate().is_ok());
    let mut t = SystemSounds::default();
    t.set(Cue::JobDone, false);
    assert!(!t.get(Cue::JobDone) && t.get(Cue::Notify));
}

#[test]
fn a_cue_plays_in_the_background_and_a_slow_output_never_holds_sign_out_back() {
    let slow = Fake::new(2000);
    let t0 = Instant::now();
    let h = start(&SystemSounds::default(), &SoundSettings::default(), Cue::Logout, slow.clone());
    assert!(t0.elapsed() < Duration::from_millis(100), "starting returns at once");
    // sign-out waits for its cue, but never longer than the bound
    let w0 = Instant::now();
    assert_eq!(h.wait(Duration::from_millis(150)), None, "still playing");
    assert_eq!(h.wait(LOGOUT_WAIT.min(Duration::from_millis(200))), None);
    assert!(w0.elapsed() < Duration::from_millis(600), "{:?}", w0.elapsed());
    assert!(slow.played.lock().unwrap().is_empty(), "the slow output has not finished: sign-out went on");
    // a quick output finishes inside the wait
    let quick = Fake::new(30);
    let h = start(&SystemSounds::default(), &SoundSettings::default(), Cue::Logout, quick.clone());
    assert_eq!(h.wait(LOGOUT_WAIT), Some(Outcome::Played));
    assert_eq!(h.wait(Duration::ZERO), Some(Outcome::Played), "asking again gives the same answer");
    let sent = quick.played.lock().unwrap().clone();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0], 44 + 2 * render(Cue::Logout, -12).len(), "a 44.1 kHz WAV of the cue");
}

#[test]
fn nothing_plays_when_switched_off_muted_or_without_an_output() {
    let out = Fake::new(0);
    let sound = SoundSettings::default();
    let off = SystemSounds { enabled: false, ..Default::default() };
    assert!(matches!(start(&off, &sound, Cue::Login, out.clone()).wait(Duration::from_secs(1)), Some(Outcome::Off(_))));
    let one = SystemSounds { notify: false, ..Default::default() };
    assert!(matches!(start(&one, &sound, Cue::Notify, out.clone()).wait(Duration::from_secs(1)), Some(Outcome::Off(_))));
    let muted = SoundSettings { muted: true, ..Default::default() };
    assert!(matches!(start(&SystemSounds::default(), &muted, Cue::Login, out.clone()).wait(Duration::from_secs(1)), Some(Outcome::Off(m)) if m.contains("muted")));
    let none = Arc::new(Fake { delay: Duration::ZERO, ready: Err("no sound output: connect a monitor".into()), played: Mutex::new(vec![]), fail: None });
    let h = start(&SystemSounds::default(), &sound, Cue::Login, none.clone());
    assert!(matches!(h.wait(Duration::from_secs(1)), Some(Outcome::NoOutput(_))));
    assert!(out.played.lock().unwrap().is_empty() && none.played.lock().unwrap().is_empty(), "no sound was made");
    // a failing player is reported, not raised
    let bad = Arc::new(Fake { delay: Duration::ZERO, ready: Ok(()), played: Mutex::new(vec![]), fail: Some("aplay: no such device".into()) });
    assert!(matches!(start(&SystemSounds::default(), &sound, Cue::Login, bad).wait(Duration::from_secs(1)), Some(Outcome::Failed(_))));
}

#[test]
fn a_preview_ignores_the_switches_but_not_mute_or_a_missing_output() {
    let out = Fake::new(0);
    let off = SystemSounds { enabled: false, ..Default::default() };
    let h = preview(&off, &SoundSettings::default(), Cue::JobDone, out.clone());
    assert_eq!(h.wait(Duration::from_secs(1)), Some(Outcome::Played));
    let muted = SoundSettings { muted: true, ..Default::default() };
    assert!(matches!(preview(&off, &muted, Cue::JobDone, out.clone()).wait(Duration::from_secs(1)), Some(Outcome::Off(_))));
    let none = Arc::new(Fake { delay: Duration::ZERO, ready: Err("no card".into()), played: Mutex::new(vec![]), fail: None });
    let m = preview(&off, &SoundSettings::default(), Cue::Login, none).wait(Duration::from_secs(1)).unwrap().say(Cue::Login);
    assert_eq!(m, Err("No sound output: no card".to_string()));
}

#[test]
fn the_settings_file_keeps_system_sounds_and_an_old_file_without_them_loads_with_the_defaults() {
    use crate::settings::SettingsStore;
    let dir = std::env::temp_dir().join(format!("iyakki-syssounds-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("settings.json");
    // a file written before system sounds existed
    std::fs::write(&p, r#"{"providers":[],"sound":{"volume_pct":40}}"#).unwrap();
    let mut s = SettingsStore::open(&p).unwrap();
    assert_eq!(s.get().sounds, SystemSounds::default());
    assert_eq!(s.get().sound.volume_pct, 40);
    s.update(|x| {
        x.sounds.offset_db = -18;
        x.sounds.logout = false;
    })
    .unwrap();
    let again = SettingsStore::open(&p).unwrap();
    assert_eq!((again.get().sounds.offset_db, again.get().sounds.logout, again.get().sounds.login), (-18, false, true));
    // a level outside the range is refused and the saved settings stay
    assert!(s.update(|x| x.sounds.offset_db = 5).is_err());
    assert_eq!(SettingsStore::open(&p).unwrap().get().sounds.offset_db, -18);
    let _ = std::fs::remove_dir_all(&dir);
}
