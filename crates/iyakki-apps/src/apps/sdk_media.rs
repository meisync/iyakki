//! The core-backed [`MediaPort`] of an SDK app's `AudioView` (F-SDK-145): the SDK's own port types mapped onto the core's media player
//! (`iyakki_core_api::Media`, reached as `cx.core.media()`). One adapter serves every SDK app that embeds the player; the person and the app id
//! are fixed when it is made, so a view can only reach the session of its own app. The adapter holds no file, sound card or network: the core
//! reads and decodes the file, asks the person about a remote source, and plays the sound through its mixer.

use std::sync::Arc;

use iyakki_core_api::media::{MediaEvent, MediaSource, MediaSpec, MediaState, TrackKind as CoreKind, NO_MEDIA};
use iyakki_core_api::Core;
use iyakki_ui_sdk::media::{Event, Info, Item, MediaError, MediaPort, PlayState, Source, Track, TrackKind};

pub struct CoreMediaPort {
    core: Arc<dyn Core>,
    user: String,
    app: String,
    session: Option<String>,
}

impl CoreMediaPort {
    pub fn new(core: Arc<dyn Core>, user: &str, app: &str) -> CoreMediaPort {
        CoreMediaPort { core, user: user.into(), app: app.into(), session: None }
    }

    fn id(&self) -> Result<String, MediaError> {
        self.session.clone().ok_or_else(|| MediaError::Refused("Nothing is open.".into()))
    }
}

fn err(e: String) -> MediaError {
    if e == NO_MEDIA { MediaError::Unavailable } else { MediaError::Refused(sentence(&e)) }
}

/// The core's sentences are lower case at the start; the widget shows them as sentences.
fn sentence(s: &str) -> String {
    let mut c = s.chars();
    let t = match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    };
    if t.ends_with('.') { t } else { format!("{t}.") }
}

fn state(s: MediaState) -> PlayState {
    match s {
        MediaState::AwaitingApproval => PlayState::AwaitingApproval,
        MediaState::Opening => PlayState::Opening,
        MediaState::Ready => PlayState::Ready,
        MediaState::Playing => PlayState::Playing,
        MediaState::Paused => PlayState::Paused,
        MediaState::Buffering => PlayState::Buffering,
        MediaState::Ended => PlayState::Ended,
        MediaState::Failed => PlayState::Failed,
    }
}

fn event(e: MediaEvent) -> Event {
    match e {
        MediaEvent::Approval { summary } => Event::Approval(summary),
        MediaEvent::Opened { info } => Event::Opened(Info {
            title: info.title,
            duration_ms: info.duration_ms,
            seekable: info.seekable,
            has_video: info.has_video,
            codec: info.codec,
            tracks: info.tracks.into_iter().map(|t| Track { id: t.id, kind: match t.kind { CoreKind::Audio => TrackKind::Audio, CoreKind::Video => TrackKind::Video, CoreKind::Caption => TrackKind::Caption }, label: t.label, lang: t.lang, selected: t.selected }).collect(),
        }),
        MediaEvent::State { state: s } => Event::State(state(s)),
        MediaEvent::Position { ms } => Event::Position(ms),
        MediaEvent::Level { peak } => Event::Level(peak),
        MediaEvent::Error { text } => Event::Error(sentence(&text)),
    }
}

impl MediaPort for CoreMediaPort {
    fn list_home(&mut self) -> Result<Vec<Item>, MediaError> {
        // a core with no player lists nothing: the first `open` says so; ask it once so the view can say it at the start
        if let Err(e) = self.core.media().play(&self.user, "") {
            if e == NO_MEDIA {
                return Err(MediaError::Unavailable);
            }
        }
        Ok(self.core.media().list_home(&self.user).into_iter().map(|e| Item { name: e.name, size: e.size, video: e.video }).collect())
    }

    fn open(&mut self, source: &Source, title: &str, autoplay: bool) -> Result<bool, MediaError> {
        let src = match source {
            Source::Home(n) => MediaSource::Home { name: n.clone() },
            Source::Stream(u) => MediaSource::Stream { url: u.clone() },
            Source::Station(n) => MediaSource::Station { name: n.clone() },
        };
        let mut spec = MediaSpec::new(src);
        spec.title = title.chars().filter(|c| !c.is_control()).take(120).collect();
        spec.autoplay = autoplay;
        let s = self.core.media().open(&self.user, &self.app, &spec).map_err(err)?;
        self.session = Some(s.id);
        Ok(s.needs_approval)
    }

    fn decide(&mut self, allow: bool) -> Result<(), MediaError> {
        let id = self.id()?;
        self.core.media().decide(&self.user, &id, allow).map_err(err)
    }

    fn play(&mut self) -> Result<(), MediaError> {
        let id = self.id()?;
        self.core.media().play(&self.user, &id).map_err(err)
    }

    fn pause(&mut self) -> Result<(), MediaError> {
        let id = self.id()?;
        self.core.media().pause(&self.user, &id).map_err(err)
    }

    fn seek(&mut self, ms: u64) -> Result<(), MediaError> {
        let id = self.id()?;
        self.core.media().seek(&self.user, &id, ms).map_err(err)
    }

    fn set_rate(&mut self, permille: u32) -> Result<(), MediaError> {
        let id = self.id()?;
        self.core.media().set_rate(&self.user, &id, permille).map_err(err)
    }

    fn set_volume(&mut self, percent: u32) -> Result<(), MediaError> {
        let id = self.id()?;
        self.core.media().set_volume(&self.user, &id, percent).map_err(err)
    }

    fn set_mute(&mut self, mute: bool) -> Result<(), MediaError> {
        let id = self.id()?;
        self.core.media().set_mute(&self.user, &id, mute).map_err(err)
    }

    fn select_track(&mut self, track: u32) -> Result<(), MediaError> {
        let id = self.id()?;
        self.core.media().select_track(&self.user, &id, track).map_err(err)
    }

    fn poll(&mut self) -> Vec<Event> {
        match &self.session {
            Some(id) => self.core.media().poll(&self.user, id).into_iter().map(event).collect(),
            None => Vec::new(),
        }
    }

    fn frame(&mut self, after: u64) -> Result<Option<iyakki_ui_sdk::canvas::Image>, MediaError> {
        let Some(id) = self.session.clone() else { return Ok(None) };
        let f = self.core.media().frame(&self.user, &id, after).map_err(err)?;
        Ok(f.map(|f| iyakki_ui_sdk::canvas::Image { format: iyakki_ui_sdk::canvas::ImageFormat::Rgba, width: f.width, height: f.height, seq: f.seq, data: f.rgba }))
    }

    fn close(&mut self) {
        if let Some(id) = self.session.take() {
            self.core.media().close(&self.user, &id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iyakki_core_fake::FakeCore;
    use iyakki_ui_sdk::audioview::AudioView;

    fn fake() -> Arc<FakeCore> {
        let c = Arc::new(FakeCore::new());
        c.media_fake().add_file("ann", "music/a.mp3", 4000, false);
        c.media_fake().add_file("ann", "b.wav", 2000, false);
        c.media_fake().add_station("Tone FM", "https://radio.example.org/tone");
        c
    }

    #[test]
    fn the_view_plays_through_the_core_and_the_core_decides_what_it_shows() {
        let core = fake();
        let mut v = AudioView::new(Box::new(CoreMediaPort::new(core.clone(), "ann", "sample")), 900);
        v.tick();
        assert_eq!(v.names(), vec!["b.wav".to_string(), "music/a.mp3".to_string()]);
        v.play_index(1);
        assert_eq!(v.title(), Some("a.mp3"));
        assert_eq!(v.state(), PlayState::Playing);
        core.media_fake().advance(1500);
        v.tick();
        assert_eq!(v.position_ms(), 1500);
        assert_eq!(v.duration_ms(), Some(4000));
        v.seek_to(3000);
        core.media_fake().advance(2000);
        v.tick();
        // the end goes on to nothing (the last file); the state is the core's
        assert_eq!(v.state(), PlayState::Ended);
        assert_eq!(core.media_fake().audit(), vec!["media.open sample home".to_string()], "the core's audit line names the app and the kind, never the file");
    }

    #[test]
    fn a_remote_source_is_the_cores_question_and_a_refusal_is_a_sentence() {
        let core = fake();
        let mut v = AudioView::new(Box::new(CoreMediaPort::new(core.clone(), "ann", "sample")), 900);
        v.open(iyakki_ui_sdk::media::Source::Station("Tone FM".into()), "");
        assert_eq!(v.state(), PlayState::AwaitingApproval);
        assert!(v.approval().is_some_and(|s| s.contains("Tone FM")));
        v.action(900 + 8); // Deny
        assert_eq!(v.state(), PlayState::Failed);
        assert_eq!(v.error(), Some("You did not allow that source."));
        assert!(core.media_fake().audit().contains(&"media.decide sample denied".to_string()));
        core.media_fake().refuse_open(Some("at most 4 apps may play at the same time"));
        v.open(iyakki_ui_sdk::media::Source::Home("b.wav".into()), "");
        assert_eq!(v.error(), Some("At most 4 apps may play at the same time."));
    }

    #[test]
    fn a_video_comes_back_as_the_picture_that_is_due_and_nothing_new_twice() {
        let core = fake();
        core.media_fake().add_file("ann", "clip.avi", 5000, true);
        let mut p = CoreMediaPort::new(core.clone(), "ann", "sample");
        assert!(p.frame(0).unwrap().is_none(), "nothing is open");
        p.open(&Source::Home("clip.avi".into()), "", true).unwrap();
        let a = p.frame(0).unwrap().unwrap();
        assert_eq!((a.width, a.height, a.seq, a.format), (8, 6, 1, iyakki_ui_sdk::canvas::ImageFormat::Rgba));
        assert!(a.check().is_ok());
        assert!(p.frame(a.seq).unwrap().is_none());
        core.media_fake().advance(2000);
        assert_eq!(p.frame(a.seq).unwrap().unwrap().seq, 21);
    }

    #[test]
    fn another_person_has_another_home_and_a_core_with_no_player_says_so() {
        let core = fake();
        let mut p = CoreMediaPort::new(core.clone(), "bob", "sample");
        assert!(p.list_home().unwrap().is_empty());
        assert!(matches!(p.open(&Source::Home("music/a.mp3".into()), "", true), Err(MediaError::Refused(_))));
        struct None_;
        impl Core for None_ {
            fn mixer(&self) -> &dyn iyakki_core_api::Mixer {
                unreachable!()
            }
            fn app_status(&self) -> &dyn iyakki_core_api::AppStatuses {
                unreachable!()
            }
        }
        let mut n = CoreMediaPort::new(Arc::new(None_), "ann", "sample");
        assert_eq!(n.list_home(), Err(MediaError::Unavailable));
        assert_eq!(n.open(&Source::Home("a.mp3".into()), "", true), Err(MediaError::Unavailable));
    }
}
