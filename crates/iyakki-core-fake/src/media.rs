//! The fake media player: no decoder, no sound card, a clock the test moves (`advance`). It follows the same rules as the real one (limits
//! from `iyakki_core_api::media`, a session belongs to one person and one app, a remote source waits for the person, a live stream cannot
//! be moved, audit lines carry names only) so a test of an SDK app against it means the same thing as against the device.

use iyakki_core_api::media::{MediaFrame, SWITCHED_OFF, check_rate, check_spec, check_volume, Media, MediaEntry, MediaEvent, MediaInfo, MediaSession, MediaSource, MediaSpec, MediaState, TrackInfo, TrackKind, MAX_MEDIA_SESSIONS};
use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;

#[derive(Clone, Debug)]
struct Item {
    info: MediaInfo,
    video: bool,
}

struct Session {
    user: String,
    app: String,
    item: Option<Item>,
    state: MediaState,
    pos: u64,
    rate: u32,
    volume: u32,
    muted: bool,
    out: VecDeque<MediaEvent>,
    autoplay: bool,
    audible_ms: u64,
}

#[derive(Default)]
struct State {
    files: BTreeMap<(String, String), Item>,
    streams: BTreeMap<String, Item>,
    stations: BTreeMap<String, String>,
    sessions: BTreeMap<String, Session>,
    audit: Vec<String>,
    next: u32,
    refuse_open: Option<String>,
    off_for: std::collections::BTreeSet<String>,
}

#[derive(Default)]
pub struct FakeMedia {
    st: Mutex<State>,
}

fn lock(m: &Mutex<State>) -> std::sync::MutexGuard<'_, State> {
    m.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn item(title: &str, duration_ms: Option<u64>, video: bool) -> Item {
    let mut tracks = vec![TrackInfo { id: 0, kind: TrackKind::Audio, label: "Audio".into(), lang: String::new(), selected: true }];
    if video {
        tracks.push(TrackInfo { id: 1, kind: TrackKind::Video, label: "Video".into(), lang: String::new(), selected: true });
    }
    Item { info: MediaInfo { title: title.into(), duration_ms, seekable: duration_ms.is_some(), has_audio: true, has_video: video, sample_rate: 44_100, channels: 2, codec: "MP3".into(), tracks }, video }
}

impl FakeMedia {
    pub fn new() -> FakeMedia {
        FakeMedia::default()
    }
    /// A file of `user`'s home.
    pub fn add_file(&self, user: &str, name: &str, duration_ms: u64, video: bool) {
        lock(&self.st).files.insert((user.into(), name.into()), item(name.rsplit('/').next().unwrap_or(name), Some(duration_ms), video));
    }
    /// A stream address; `duration_ms` `None` is a live stream.
    pub fn add_stream(&self, url: &str, duration_ms: Option<u64>) {
        lock(&self.st).streams.insert(url.into(), item(url, duration_ms, false));
    }
    pub fn add_station(&self, name: &str, url: &str) {
        lock(&self.st).stations.insert(name.into(), url.into());
        self.add_stream(url, None);
    }
    /// The audit lines the core wrote (`media.open player home`, ...).
    pub fn audit(&self) -> Vec<String> {
        lock(&self.st).audit.clone()
    }
    pub fn refuse_open(&self, why: Option<&str>) {
        lock(&self.st).refuse_open = why.map(str::to_string);
    }
    /// Move the clock: every playing session advances by `ms` at its rate, emits a position and ends at its end.
    pub fn advance(&self, ms: u64) {
        let mut st = lock(&self.st);
        for s in st.sessions.values_mut() {
            if s.state != MediaState::Playing {
                continue;
            }
            let step = ms * u64::from(s.rate) / 1000;
            s.pos += step;
            if !s.muted && s.volume > 0 {
                s.audible_ms += step;
            }
            let dur = s.item.as_ref().and_then(|i| i.info.duration_ms);
            if let Some(d) = dur {
                if s.pos >= d {
                    s.pos = d;
                    s.out.push_back(MediaEvent::Position { ms: d });
                    s.state = MediaState::Ended;
                    s.out.push_back(MediaEvent::State { state: MediaState::Ended });
                    continue;
                }
            }
            let p = s.pos;
            s.out.push_back(MediaEvent::Position { ms: p });
        }
    }
    /// What the session's sound is now: `(volume percent, muted, milliseconds that were audible)`.
    pub fn sound_of(&self, session: &str) -> Option<(u32, bool, u64)> {
        lock(&self.st).sessions.get(session).map(|s| (s.volume, s.muted, s.audible_ms))
    }
    pub fn position_of(&self, session: &str) -> Option<u64> {
        lock(&self.st).sessions.get(session).map(|s| s.pos)
    }
    pub fn rate_of(&self, session: &str) -> Option<u32> {
        lock(&self.st).sessions.get(session).map(|s| s.rate)
    }
}

fn mine<'a>(st: &'a mut State, user: &str, id: &str) -> Result<&'a mut Session, String> {
    match st.sessions.get_mut(id) {
        Some(s) if s.user == user => Ok(s),
        _ => Err("that media session is not open".into()),
    }
}

fn start(s: &mut Session) {
    s.state = MediaState::Opening;
    s.out.push_back(MediaEvent::State { state: MediaState::Opening });
    match s.item.clone() {
        Some(i) => {
            s.out.push_back(MediaEvent::Opened { info: i.info });
            s.state = MediaState::Ready;
            s.out.push_back(MediaEvent::State { state: MediaState::Ready });
            if s.autoplay {
                s.state = MediaState::Playing;
                s.out.push_back(MediaEvent::State { state: MediaState::Playing });
            }
        }
        None => {
            s.state = MediaState::Failed;
            s.out.push_back(MediaEvent::Error { text: "that source could not be opened".into() });
            s.out.push_back(MediaEvent::State { state: MediaState::Failed });
        }
    }
}

impl Media for FakeMedia {
    fn enabled(&self, user: &str) -> bool {
        !lock(&self.st).off_for.contains(user)
    }

    fn set_enabled(&self, user: &str, on: bool) -> Result<(), String> {
        let mut st = lock(&self.st);
        if on {
            st.off_for.remove(user);
        } else {
            st.off_for.insert(user.to_string());
            st.sessions.retain(|_, s| s.user != user);
        }
        st.audit.push(format!("media.switch {}", if on { "on" } else { "off" }));
        Ok(())
    }

    fn open_sessions(&self, user: &str) -> usize {
        lock(&self.st).sessions.values().filter(|s| s.user == user).count()
    }

    fn list_home(&self, user: &str) -> Vec<MediaEntry> {
        let st = lock(&self.st);
        st.files.iter().filter(|((u, _), _)| u == user).map(|((_, n), i)| MediaEntry { name: n.clone(), size: 1000, video: i.video }).collect()
    }

    fn open(&self, user: &str, app: &str, spec: &MediaSpec) -> Result<MediaSession, String> {
        check_spec(app, spec)?;
        let mut st = lock(&self.st);
        if st.off_for.contains(user) {
            return Err(SWITCHED_OFF.into());
        }
        if let Some(why) = st.refuse_open.clone() {
            return Err(why);
        }
        // one session per person and app: the earlier one of this app is closed
        let old: Vec<String> = st.sessions.iter().filter(|(_, s)| s.user == user && s.app == app).map(|(k, _)| k.clone()).collect();
        for k in old {
            st.sessions.remove(&k);
        }
        if st.sessions.values().filter(|s| s.user == user).count() >= MAX_MEDIA_SESSIONS {
            return Err(format!("at most {MAX_MEDIA_SESSIONS} apps may play at the same time"));
        }
        let found = match &spec.source {
            MediaSource::Home { name } => st.files.get(&(user.to_string(), name.clone())).cloned().ok_or("that file is not in your home folder")?,
            MediaSource::Stream { url } => st.streams.get(url).cloned().unwrap_or_else(|| item(url, None, false)),
            MediaSource::Station { name } => {
                let url = st.stations.get(name).cloned().ok_or("that station is not in your list")?;
                st.streams.get(&url).cloned().unwrap_or_else(|| item(name, None, false))
            }
        };
        st.next += 1;
        let id = format!("m{}", st.next);
        st.audit.push(format!("media.open {app} {}", spec.source.audit_kind()));
        let remote = spec.source.is_remote();
        let mut s = Session { user: user.into(), app: app.into(), item: Some(found), state: MediaState::Opening, pos: 0, rate: 1000, volume: spec.volume.unwrap_or(100), muted: false, out: VecDeque::new(), autoplay: spec.autoplay, audible_ms: 0 };
        if remote {
            s.state = MediaState::AwaitingApproval;
            s.out.push_back(MediaEvent::State { state: MediaState::AwaitingApproval });
            let what = match &spec.source {
                MediaSource::Stream { url } => format!("Play {url} from the internet"),
                MediaSource::Station { name } => format!("Play the radio station {name}"),
                MediaSource::Home { .. } => String::new(),
            };
            s.out.push_back(MediaEvent::Approval { summary: what });
        } else {
            start(&mut s);
        }
        st.sessions.insert(id.clone(), s);
        Ok(MediaSession { id, needs_approval: remote })
    }

    fn decide(&self, user: &str, session: &str, allow: bool) -> Result<(), String> {
        let mut st = lock(&self.st);
        let s = mine(&mut st, user, session)?;
        if s.state != MediaState::AwaitingApproval {
            return Err("nothing is waiting for your answer".into());
        }
        let app = s.app.clone();
        if allow {
            start(s);
        } else {
            s.state = MediaState::Failed;
            s.out.push_back(MediaEvent::Error { text: "you did not allow that source".into() });
            s.out.push_back(MediaEvent::State { state: MediaState::Failed });
        }
        st.audit.push(format!("media.decide {app} {}", if allow { "approved" } else { "denied" }));
        Ok(())
    }

    fn play(&self, user: &str, session: &str) -> Result<(), String> {
        let mut st = lock(&self.st);
        let s = mine(&mut st, user, session)?;
        match s.state {
            MediaState::Ready | MediaState::Paused | MediaState::Ended => {
                if s.state == MediaState::Ended {
                    s.pos = 0;
                }
                s.state = MediaState::Playing;
                s.out.push_back(MediaEvent::State { state: MediaState::Playing });
                Ok(())
            }
            MediaState::Playing => Ok(()),
            MediaState::AwaitingApproval => Err("allow the source first".into()),
            _ => Err("this cannot be played now".into()),
        }
    }

    fn pause(&self, user: &str, session: &str) -> Result<(), String> {
        let mut st = lock(&self.st);
        let s = mine(&mut st, user, session)?;
        if s.state == MediaState::Playing {
            s.state = MediaState::Paused;
            s.out.push_back(MediaEvent::State { state: MediaState::Paused });
        }
        Ok(())
    }

    fn seek(&self, user: &str, session: &str, ms: u64) -> Result<(), String> {
        let mut st = lock(&self.st);
        let s = mine(&mut st, user, session)?;
        let Some(d) = s.item.as_ref().and_then(|i| i.info.duration_ms) else { return Err("a live stream cannot be moved".into()) };
        s.pos = ms.min(d);
        let p = s.pos;
        s.out.push_back(MediaEvent::Position { ms: p });
        Ok(())
    }

    fn set_rate(&self, user: &str, session: &str, permille: u32) -> Result<(), String> {
        check_rate(permille)?;
        let mut st = lock(&self.st);
        mine(&mut st, user, session)?.rate = permille;
        Ok(())
    }

    fn set_volume(&self, user: &str, session: &str, percent: u32) -> Result<(), String> {
        check_volume(percent)?;
        let mut st = lock(&self.st);
        mine(&mut st, user, session)?.volume = percent;
        Ok(())
    }

    fn set_mute(&self, user: &str, session: &str, mute: bool) -> Result<(), String> {
        let mut st = lock(&self.st);
        mine(&mut st, user, session)?.muted = mute;
        Ok(())
    }

    fn select_track(&self, user: &str, session: &str, track: u32) -> Result<(), String> {
        let mut st = lock(&self.st);
        let s = mine(&mut st, user, session)?;
        let Some(i) = s.item.as_mut() else { return Err("nothing is open".into()) };
        let Some(kind) = i.info.tracks.iter().find(|t| t.id == track).map(|t| t.kind) else { return Err("that track does not exist".into()) };
        for t in i.info.tracks.iter_mut().filter(|t| t.kind == kind) {
            t.selected = t.id == track;
        }
        Ok(())
    }

    fn poll(&self, user: &str, session: &str) -> Vec<MediaEvent> {
        let mut st = lock(&self.st);
        match mine(&mut st, user, session) {
            Ok(s) => s.out.drain(..).collect(),
            Err(_) => Vec::new(),
        }
    }

    /// A video item shows ten pictures a second: picture `n` is a plain 8 x 6 field whose red is `n` (so a test sees which one was due).
    fn frame(&self, user: &str, session: &str, after: u64) -> Result<Option<MediaFrame>, String> {
        let mut st = lock(&self.st);
        let s = mine(&mut st, user, session)?;
        let Some(i) = s.item.as_ref().filter(|i| i.video) else { return Ok(None) };
        let _ = i;
        let n = s.pos / 100;
        let seq = n + 1;
        if seq <= after {
            return Ok(None);
        }
        let px: Vec<u8> = (0..48).flat_map(|_| [(n % 256) as u8, 10, 20, 255]).collect();
        Ok(Some(MediaFrame { seq, pts_ms: n * 100, width: 8, height: 6, rgba: std::sync::Arc::new(px) }))
    }

    fn close(&self, user: &str, session: &str) {
        let mut st = lock(&self.st);
        if mine(&mut st, user, session).is_ok() {
            st.sessions.remove(session);
            st.audit.push("media.close".into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn play_home(f: &FakeMedia) -> String {
        f.add_file("ann", "music/a.mp3", 3000, false);
        f.open("ann", "player", &MediaSpec::new(MediaSource::Home { name: "music/a.mp3".into() })).unwrap().id
    }

    #[test]
    fn a_home_file_plays_to_its_end_and_the_events_come_in_order() {
        let f = FakeMedia::new();
        let id = play_home(&f);
        let ev = f.poll("ann", &id);
        assert!(matches!(ev[0], MediaEvent::State { state: MediaState::Opening }) && matches!(ev[1], MediaEvent::Opened { .. }));
        assert!(matches!(ev.last(), Some(MediaEvent::State { state: MediaState::Playing })));
        f.advance(1000);
        f.advance(2500);
        let ev = f.poll("ann", &id);
        assert_eq!(ev[0], MediaEvent::Position { ms: 1000 });
        assert!(ev.contains(&MediaEvent::State { state: MediaState::Ended }));
        assert_eq!(f.audit(), vec!["media.open player home".to_string()], "names only: no file name in the audit");
    }

    #[test]
    fn a_remote_source_waits_for_the_person_and_a_no_stops_it() {
        let f = FakeMedia::new();
        let s = f.open("ann", "player", &MediaSpec::new(MediaSource::Stream { url: "https://r.example.org/live.mp3".into() })).unwrap();
        assert!(s.needs_approval);
        assert!(f.play("ann", &s.id).unwrap_err().contains("allow"));
        f.advance(500);
        assert_eq!(f.position_of(&s.id), Some(0));
        f.decide("ann", &s.id, false).unwrap();
        let ev = f.poll("ann", &s.id);
        assert!(ev.iter().any(|e| matches!(e, MediaEvent::Approval { summary } if summary.contains("internet"))));
        assert!(ev.iter().any(|e| matches!(e, MediaEvent::Error { text } if text.contains("did not allow"))));
        assert!(f.audit().contains(&"media.decide player denied".to_string()));
    }

    #[test]
    fn one_session_per_app_and_never_another_persons() {
        let f = FakeMedia::new();
        let a = play_home(&f);
        let b = play_home(&f);
        assert_ne!(a, b);
        assert!(f.pause("ann", &a).is_err(), "the first session of the app was closed by the second open");
        assert!(f.pause("bob", &b).is_err() && f.poll("bob", &b).is_empty());
        assert!(f.open("ann", "player", &MediaSpec::new(MediaSource::Home { name: "missing.mp3".into() })).unwrap_err().contains("not in your home"));
    }

    #[test]
    fn seek_rate_volume_mute_and_live_streams_follow_the_rules() {
        let f = FakeMedia::new();
        let id = play_home(&f);
        f.seek("ann", &id, 99_000).unwrap();
        assert_eq!(f.position_of(&id), Some(3000));
        f.set_rate("ann", &id, 2000).unwrap();
        assert!(f.set_rate("ann", &id, 100).is_err() && f.set_volume("ann", &id, 101).is_err());
        f.seek("ann", &id, 0).unwrap();
        f.advance(500);
        assert_eq!(f.position_of(&id), Some(1000), "double speed");
        f.set_mute("ann", &id, true).unwrap();
        f.advance(100);
        assert_eq!(f.sound_of(&id).unwrap().2, 1000, "muted time is not audible");
        f.add_station("Radio X", "https://x.example.org/s");
        let live = f.open("ann", "radio", &MediaSpec::new(MediaSource::Station { name: "Radio X".into() })).unwrap();
        f.decide("ann", &live.id, true).unwrap();
        assert!(f.seek("ann", &live.id, 5).unwrap_err().contains("live"));
    }

    #[test]
    fn the_switch_refuses_new_sessions_and_closes_the_open_ones() {
        let f = FakeMedia::new();
        let id = play_home(&f);
        assert!(f.enabled("ann") && f.open_sessions("ann") == 1);
        f.set_enabled("ann", false).unwrap();
        assert_eq!(f.open_sessions("ann"), 0);
        assert!(f.poll("ann", &id).is_empty() && f.pause("ann", &id).is_err());
        assert!(f.open("ann", "player", &MediaSpec::new(MediaSource::Home { name: "music/a.mp3".into() })).unwrap_err().contains("switch is off"));
        assert!(f.enabled("bob"), "the switch is the person's own");
        f.set_enabled("ann", true).unwrap();
        assert!(f.open("ann", "player", &MediaSpec::new(MediaSource::Home { name: "music/a.mp3".into() })).is_ok());
    }

    #[test]
    fn a_video_session_gives_the_picture_that_is_due_and_nothing_new_twice() {
        let f = FakeMedia::new();
        f.add_file("ann", "v.avi", 5000, true);
        let s = f.open("ann", "player", &MediaSpec::new(MediaSource::Home { name: "v.avi".into() })).unwrap();
        let a = f.frame("ann", &s.id, 0).unwrap().unwrap();
        assert_eq!((a.seq, a.width, a.height, a.rgba[0]), (1, 8, 6, 0));
        assert!(f.frame("ann", &s.id, a.seq).unwrap().is_none(), "the same picture is not given again");
        f.advance(1000);
        let b = f.frame("ann", &s.id, a.seq).unwrap().unwrap();
        assert_eq!((b.seq, b.rgba[0]), (11, 10), "the picture of one second in, the ones between were dropped");
        f.add_file("ann", "a.mp3", 5000, false);
        let m = f.open("ann", "other", &MediaSpec::new(MediaSource::Home { name: "a.mp3".into() })).unwrap();
        assert!(f.frame("ann", &m.id, 0).unwrap().is_none(), "a sound has no picture");
        assert!(f.frame("bob", &s.id, 0).is_err());
    }

    #[test]
    fn tracks_are_chosen_by_id_within_their_kind() {
        let f = FakeMedia::new();
        f.add_file("ann", "v.mp4", 1000, true);
        let s = f.open("ann", "player", &MediaSpec::new(MediaSource::Home { name: "v.mp4".into() })).unwrap();
        assert!(f.select_track("ann", &s.id, 7).is_err());
        f.select_track("ann", &s.id, 1).unwrap();
        assert_eq!(f.list_home("ann")[0], MediaEntry { name: "v.mp4".into(), size: 1000, video: true });
    }
}
