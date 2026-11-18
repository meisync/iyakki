//! RADIO in the shell: the tile is the crate `iyakki-tile-radio` (docs/development/RADIO-EXTRACTION-PLAN.md); this file gives it the shell's
//! context. It builds the tile's `TileCx` with the real radio port (`CxPort`, in `radio_port.rs`) on every call and forwards the `App` methods.
//! The tests below are the ones that need real services (the radio player with a fake streamer, the account's station files); the tile's own tests
//! run in the crate against a fake port. `Deref` shows the tile to the tests.

use std::ops::{Deref, DerefMut};

use iyakki_tile_api::App as TileApp;
use iyakki_tile_api::TileCx;

use super::radio_port::{CxPort, RadioPort};
use super::{App, AppCx, Flow};
use crate::gfx::{Gfx, Icon};
use crate::ui::{Event, R};

pub struct RadioApp(iyakki_tile_radio::RadioApp);

impl Deref for RadioApp {
    type Target = iyakki_tile_radio::RadioApp;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl DerefMut for RadioApp {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl RadioApp {
    pub fn new(p: &dyn RadioPort) -> RadioApp {
        RadioApp(iyakki_tile_radio::RadioApp::new(p))
    }

    fn with<T>(cx: &AppCx, f: impl FnOnce(&TileCx) -> T) -> T {
        let port = CxPort(cx);
        f(&TileCx { user: cx.session.user.as_str(), manage_settings: cx.session.policy.manage_settings, usage: &iyakki_tile_api::usage::NoUsage, calendar: &iyakki_tile_api::calendar::NoCalendar, audit: &iyakki_tile_api::NoAudit, remote: &iyakki_tile_api::remote::NoRemote, cameras: &iyakki_tile_api::cameras::NoCameras, radio: &port })
    }
}

impl App for RadioApp {
    fn title(&self) -> &'static str {
        self.0.title()
    }
    fn icon(&self) -> Icon {
        self.0.icon()
    }
    fn accent(&self) -> [u8; 4] {
        self.0.accent()
    }
    fn hint(&self) -> String {
        self.0.hint()
    }
    fn help(&self) -> Vec<String> {
        self.0.help()
    }
    fn media_key(&mut self, k: crate::fnkeys::FnKey, cx: &AppCx) -> bool {
        Self::with(cx, |t| self.0.media_key(k, t))
    }
    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        match Self::with(cx, |t| self.0.handle(ev, t)) {
            iyakki_tile_api::Flow::Stay => Flow::Stay,
            iyakki_tile_api::Flow::Back => Flow::Back,
        }
    }
    fn take_request(&mut self) -> Option<(String, String)> {
        self.0.take_request()
    }
    fn render(&mut self, g: &mut Gfx, area: R, s: f32, cx: &AppCx, tick: u64) {
        Self::with(cx, |t| self.0.render(g, area, s, t, tick));
    }
}

#[cfg(test)]
#[cfg(test)]
mod tests {
    use iyakki_agent::facets_h::RealServices as _;
    use super::*;
    use crate::apps::radio_port::CxPort;
    use crate::gfx::rgb;
    use crate::ui::Key;
    use iyakki_tile_api::radio::{self, RadioState};
    use iyakki_tile_radio::Mode;
    use std::sync::Arc;
    use iyakki_agent::radio::LANGUAGES;
    use iyakki_proxy::certinfo;

    const TABS: usize = 1 + LANGUAGES.len();

    use iyakki_agent::radio::StationStore;

    fn store(cx: &AppCx) -> StationStore {
        StationStore::new(cx.data_dir(), &cx.session.user)
    }
    use crate::apps::tests::{cx_with_admin_store, services_in, temp_dir};
    use crate::apps::tile_harness::TileHarness;
    use iyakki_proxy::{StreamHead, Streamer};
    use iyakki_sec::policy::Role;
    use std::sync::Mutex;

    /// Silence in MP3 frames that never ends (a little slower than real time, so the test stays quiet).
    struct Endless(Vec<u8>, usize);
    impl std::io::Read for Endless {
        fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
            std::thread::sleep(std::time::Duration::from_millis(2));
            let n = out.len().min(417);
            for b in out[..n].iter_mut() {
                *b = self.0[self.1 % self.0.len()];
                self.1 += 1;
            }
            Ok(n)
        }
    }

    struct Fake(Mutex<Vec<String>>);
    impl Streamer for Fake {
        fn open(&self, url: &str, _h: &[(String, String)]) -> Result<StreamHead, String> {
            self.0.lock().unwrap().push(url.to_string());
            let mut frame = vec![0u8; 417];
            frame[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
            Ok(StreamHead { status: 200, headers: vec![("content-type".into(), "audio/mpeg".into())], body: Box::new(Endless(frame, 0)) })
        }
    }

    fn cx(tag: &str) -> (AppCx, Arc<Fake>) {
        let mut cx = cx_with_admin_store(tag);
        let d = temp_dir(&format!("{tag}-svc"));
        cx.services = services_in(&d, Arc::new(crate::apps::tests::Offline));
        let f = Arc::new(Fake(Mutex::new(vec![])));
        cx.radio().set_streamer(f.clone());
        cx.radio().set_retry_wait(std::time::Duration::from_millis(20));
        cx.settings().lock().unwrap().update(|s| s.voice.play_cmd = "tee /dev/null".into()).unwrap();
        (cx, f)
    }

    fn typ(a: &mut RadioApp, cx: &AppCx, s: &str) {
        for c in s.chars() {
            a.handle(&Event::Key(Key::Char(c)), cx);
        }
    }

    fn wait(cx: &AppCx, what: impl Fn(&RadioState) -> bool) {
        for _ in 0..500 {
            if what(&cx.radio().state()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("{:?}", cx.radio().state());
    }

    #[test]
    fn a_station_waits_for_the_answer_when_the_browser_plays_and_starts_after_stop_it() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let (cx, fake) = cx("radio-hold");
        let mut a = RadioApp::new(&CxPort(&cx));
        let reg = Arc::new(iyakki_agent::soundhold::Registry::default());
        a.sound = Arc::new(crate::apps::radio_port::RegistrySound::new(reg.clone(), || false));
        a.handle(&Event::Tab(1), &cx);
        // another player holds the speaker
        let on = Arc::new(AtomicBool::new(true));
        let (h, st) = (on.clone(), on.clone());
        reg.announce("tales", Arc::new(move || h.load(Ordering::SeqCst).then(|| "TALES is reading".to_string())), Arc::new(move || st.store(false, Ordering::SeqCst)));
        a.handle(&Event::Key(Key::Enter), &cx);
        assert!(matches!(&a.mode, Mode::Hold(_, ask) if ask.question() == "TALES is reading. Stop it so the radio can play?"));
        assert_eq!(cx.radio().state(), RadioState::Idle, "nothing starts before the answer");
        let g = draw(&mut a, &cx, 1280, 720);
        for l in ["Stop it", "Keep it", "Cancel"] {
            assert!(g.hits.iter().any(|h| h.label == l), "{l}");
        }
        // Keep it: nothing starts, the other player goes on
        a.handle(&Event::Key(Key::Char('n')), &cx);
        assert!(matches!(a.mode, Mode::Browse) && on.load(Ordering::SeqCst));
        assert!(a.status_text().unwrap().contains("TALES is reading") && cx.radio().state() == RadioState::Idle);
        // Cancel
        a.handle(&Event::Key(Key::Enter), &cx);
        a.handle(&Event::Key(Key::Esc), &cx);
        assert!(matches!(a.mode, Mode::Browse) && on.load(Ordering::SeqCst));
        // Stop it: the other player is stopped cleanly, then the station starts
        a.handle(&Event::Key(Key::Enter), &cx);
        a.handle(&Event::Key(Key::Char('y')), &cx);
        let t0 = std::time::Instant::now();
        while matches!(a.mode, Mode::Hold(..)) {
            assert!(t0.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(10));
            a.handle(&Event::Tick, &cx);
        }
        assert!(!on.load(Ordering::SeqCst), "TALES was stopped");
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        assert!(!fake.0.lock().unwrap().is_empty());
        cx.radio().stop();
        // with the mixer on, a player that goes through it is no conflict
        on.store(true, Ordering::SeqCst);
        a.sound = Arc::new(crate::apps::radio_port::RegistrySound::new(reg.clone(), || true));
        a.handle(&Event::Key(Key::Enter), &cx);
        assert!(matches!(a.mode, Mode::Browse));
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        cx.radio().stop();
        reg.forget("tales");
    }

    #[test]
    fn media_keys_inside_the_radio_app_pause_resume_step_through_the_stations_and_stop() {
        use iyakki_uikit::keys::FnKey;
        let (cx, fake) = cx("radio-mediakeys");
        let mut a = RadioApp::new(&CxPort(&cx));
        a.handle(&Event::Tab(1), &cx);
        // nothing plays and the key starts the chosen station
        assert!(a.media_key(FnKey::PlayPause, &cx));
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        let first = match cx.radio().state() {
            RadioState::Playing { station, .. } => station,
            _ => unreachable!(),
        };
        assert_eq!(fake.0.lock().unwrap().len(), 1);
        // playing: pause, then resume with the same key (no second stream is opened)
        assert!(a.media_key(FnKey::PlayPause, &cx));
        assert!(cx.radio().is_paused());
        assert!(a.media_key(FnKey::PlayPause, &cx));
        assert!(!cx.radio().is_paused() && cx.radio().is_active());
        assert_eq!(fake.0.lock().unwrap().len(), 1, "pausing and resuming keeps the stream");
        // next and previous move through the list and start that station
        assert!(a.media_key(FnKey::Next, &cx));
        wait(&cx, |s| matches!(s, RadioState::Playing { station, .. } if *station != first));
        assert_eq!(fake.0.lock().unwrap().len(), 2, "a second station was opened");
        assert!(a.media_key(FnKey::Prev, &cx));
        wait(&cx, |s| matches!(s, RadioState::Playing { station, .. } if *station == first));
        assert_eq!(fake.0.lock().unwrap().len(), 3, "and the first one again");
        // stop, and a key the app does not use is not taken
        assert!(a.media_key(FnKey::Stop, &cx));
        assert_eq!(cx.radio().state(), RadioState::Idle);
        assert!(!a.media_key(FnKey::VolumeMute, &cx));
        // a form or popup in front takes no media key
        a.handle(&Event::Key(Key::Char('n')), &cx);
        assert!(!a.media_key(FnKey::PlayPause, &cx) && cx.radio().state() == RadioState::Idle);
    }

    #[test]
    fn a_station_is_added_played_in_the_background_and_stopped() {
        let (cx, fake) = cx("radio1");
        let mut a = RadioApp::new(&CxPort(&cx));
        assert_eq!(a.tab_name(), "Tamil", "with no stations of your own it opens on a language");
        a.handle(&Event::Tab(0), &cx);
        assert!(a.station_names().is_empty());
        let _ = draw(&mut a, &cx, 1280, 720); // the empty state
        a.handle(&Event::Key(Key::Char('n')), &cx);
        typ(&mut a, &cx, "Jazz FM");
        a.handle(&Event::Key(Key::Enter), &cx);
        typ(&mut a, &cx, "not a url");
        a.handle(&Event::Key(Key::Ctrl('s')), &cx);
        assert!(matches!(&a.mode, Mode::Form(f) if f.error.is_some()), "a bad address keeps the form open");
        if let Mode::Form(f) = &mut a.mode {
            f.set_value(1, "https://jazz.example.org/live.mp3");
        }
        a.handle(&Event::Key(Key::Ctrl('s')), &cx);
        assert_eq!(a.station_names(), vec!["Jazz FM"]);
        a.handle(&Event::Key(Key::Enter), &cx);
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        assert_eq!(fake.0.lock().unwrap()[0], "https://jazz.example.org/live.mp3");
        let _ = draw(&mut a, &cx, 1280, 720);
        // leave the app: the music goes on
        assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
        assert!(cx.radio().is_active());
        // volume (keys and the bars) and stop from a fresh visit
        let mut b = RadioApp::new(&CxPort(&cx));
        assert_eq!(b.station_names(), vec!["Jazz FM"], "stations are kept");
        let v = cx.radio().volume();
        b.handle(&Event::Key(Key::Char('-')), &cx);
        assert_eq!(cx.radio().volume(), v - 5);
        b.handle(&Event::Tab(210), &cx);
        assert_eq!(cx.radio().volume(), 50, "the tenth bar of twenty");
        let g = draw(&mut b, &cx, 1280, 720);
        assert!(g.hits.iter().filter(|h| h.label == "volume").count() == 20 && g.hits.iter().any(|h| h.label == "Play or stop"));
        b.handle(&Event::Tab(101), &cx); // the big button stops what plays
        assert_eq!(cx.radio().state(), RadioState::Idle);
        b.handle(&Event::Key(Key::Char(' ')), &cx); // and starts the chosen station again
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        b.handle(&Event::Key(Key::Char('s')), &cx);
        assert_eq!(cx.radio().state(), RadioState::Idle);
        let log = std::fs::read_to_string(cx.audit.path()).unwrap();
        assert!(log.contains("radio.add") && log.contains("radio.play"), "{log}");
    }

    #[test]
    fn the_chat_radio_tools_use_the_same_player_and_station_list_as_the_app() {
        use iyakki_shared::agent::{Tools};
        use iyakki_agent::agent::{UserTools};
        let (cx, fake) = cx("radio-chat");
        let tools = UserTools::new(&cx.session.user, cx.session.policy.clone(), iyakki_agent::services::Services::from_handle(&cx.services.clone()), cx.host.clone(), vec![]);
        let said = tools.call("radio_play", &serde_json::json!({"query": "play a tamil online fm"})).unwrap();
        assert!(said.starts_with("Playing "), "{said}");
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        assert!(fake.0.lock().unwrap()[0].starts_with("https://"), "the stream the chat chose was opened by the shared player");
        // the app, opened afterwards, finds the station playing; its stop key stops what the chat started
        let mut a = RadioApp::new(&CxPort(&cx));
        assert!(cx.radio().is_active());
        let _ = draw(&mut a, &cx, 1280, 720);
        // a station the chat keeps appears in the app's My stations
        tools.call("radio_save", &serde_json::json!({})).unwrap();
        let b = RadioApp::new(&CxPort(&cx));
        assert_eq!(b.tab_name(), "My stations");
        assert_eq!(b.station_names().len(), 1);
        a.handle(&Event::Key(Key::Char('s')), &cx);
        assert_eq!(cx.radio().state(), RadioState::Idle);
        assert_eq!(tools.call("radio_stop", &serde_json::json!({})).unwrap(), "The radio was not playing.");
    }

    #[test]
    fn the_language_tabs_hold_presets_that_can_be_played_kept_and_stepped_through() {
        let (cx, _fake) = cx("radio3");
        let mut a = RadioApp::new(&CxPort(&cx));
        // Tamil is the first language and is on the tab list
        a.handle(&Event::Tab(1), &cx);
        assert_eq!(a.tab_name(), "Tamil");
        assert!(a.station_names().contains(&"Sooriyan FM".to_string()));
        // Left and Right walk through My stations and every language, wrapping round
        let mut seen = vec![a.tab_name()];
        for _ in 0..TABS {
            a.handle(&Event::Key(Key::Right), &cx);
            seen.push(a.tab_name());
        }
        assert_eq!(seen[..4].to_vec(), vec!["Tamil", "Telugu", "Malayalam", "English"]);
        assert_eq!(seen.last().unwrap(), "Tamil", "one step per tab brings it back round");
        assert!(seen.contains(&"My stations".to_string()));
        a.handle(&Event::Key(Key::Left), &cx);
        assert_eq!(a.tab_name(), "My stations");
        // play a preset straight from its tab: no need to keep it first
        a.handle(&Event::Tab(4), &cx);
        a.handle(&Event::Key(Key::Enter), &cx);
        wait(&cx, |s| matches!(s, RadioState::Playing { station, .. } if station == "Radio Paradise"));
        // ] plays the next station, [ the previous, both wrapping
        a.handle(&Event::Key(Key::Char(']')), &cx);
        wait(&cx, |s| matches!(s, RadioState::Playing { station, .. } | RadioState::Connecting(station) if station == "KEXP Seattle"));
        a.handle(&Event::Key(Key::Char('[')), &cx);
        a.handle(&Event::Key(Key::Char('[')), &cx);
        let last = LANGUAGES[3].stations.last().unwrap().0;
        wait(&cx, |s| matches!(s, RadioState::Playing { station, .. } | RadioState::Connecting(station) if station == last));
        // keep one in My stations
        a.handle(&Event::Key(Key::Down), &cx);
        a.handle(&Event::Key(Key::Char('a')), &cx);
        assert!(a.status_text().unwrap().contains("is in My stations"), "{:?}", a.status_text());
        a.handle(&Event::Tab(0), &cx);
        assert_eq!(a.station_names().len(), 1);
        // every tab draws at several sizes, playing or not
        for t in 0..TABS {
            a.handle(&Event::Tab(t), &cx);
            let _ = draw(&mut a, &cx, 1280, 720);
        }
        for (w, h) in [(800, 600), (640, 480), (1920, 1080)] {
            let _ = draw(&mut a, &cx, w, h);
        }
        cx.radio().stop();
        let _ = draw(&mut a, &cx, 1280, 720);
    }

    #[test]
    fn the_wheel_scrolls_the_stations_and_a_click_chooses_a_row_without_moving_the_list() {
        let (cx, _d) = crate::apps::tests::cx_with_backend("radiowheel", iyakki_shared::chat::Unconfigured);
        let mut a = RadioApp::new(&CxPort(&cx));
        a.handle(&Event::Tab(1), &cx);
        // a window small enough that the tab's stations do not all fit
        let g = draw(&mut a, &cx, 1280, 720);
        let total = a.list().len();
        assert!(a.fit < total, "{} stations, {} fit", total, a.fit);
        assert_eq!((a.top, a.sel), (0, 0));
        let first_rows: Vec<usize> = g.hits.iter().filter_map(|h| if let Event::Row(i) = h.ev { Some(i) } else { None }).collect();
        assert_eq!(first_rows.first(), Some(&0));
        // the wheel moves the list and leaves the choice alone
        a.handle(&Event::Scroll(-1), &cx);
        let g = draw(&mut a, &cx, 1280, 720);
        assert_eq!((a.top, a.sel), (3, 0), "three rows down, the choice stays");
        let rows: Vec<usize> = g.hits.iter().filter_map(|h| if let Event::Row(i) = h.ev { Some(i) } else { None }).collect();
        assert_eq!(rows.first(), Some(&3));
        // a click chooses that row and the list does not shift under the pointer
        let clicked = rows[1];
        a.handle(&Event::Row(clicked), &cx);
        let g = draw(&mut a, &cx, 1280, 720);
        assert_eq!((a.top, a.sel), (3, clicked), "the same rows are shown after the click");
        let after: Vec<usize> = g.hits.iter().filter_map(|h| if let Event::Row(i) = h.ev { Some(i) } else { None }).collect();
        assert_eq!(after, rows);
        // the wheel stops at both ends
        for _ in 0..40 {
            a.handle(&Event::Scroll(-1), &cx);
        }
        draw(&mut a, &cx, 1280, 720);
        assert_eq!(a.top, total - a.fit);
        for _ in 0..40 {
            a.handle(&Event::Scroll(1), &cx);
        }
        assert_eq!(a.top, 0);
        // keys still bring the choice into view
        a.handle(&Event::Tab(1), &cx);
        for _ in 0..a.fit + 1 {
            a.handle(&Event::Key(Key::Down), &cx);
        }
        draw(&mut a, &cx, 1280, 720);
        assert!(a.top > 0 && a.sel >= a.top && a.sel < a.top + a.fit, "top {} sel {} fit {}", a.top, a.sel, a.fit);
    }

    #[test]
    fn the_language_list_is_on_the_left_a_station_is_added_by_name_and_address_and_removed_from_my_stations_with_a_button() {
        let (cx, _d) = {
            let (cx, d) = crate::apps::tests::cx_with_backend("radiolist", iyakki_shared::chat::Unconfigured);
            (cx, d)
        };
        let mut a = RadioApp::new(&CxPort(&cx));
        let g = draw(&mut a, &cx, 1280, 760);
        // the nav lists My stations and the languages, with counts, and has the add button
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(0) && h.label == "My stations"));
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(1) && h.label == "Tamil"));
        assert!(g.hits.iter().any(|h| h.ev == Event::Key(Key::Char('n'))), "an Add station button");
        // clicking it opens the form: a name and an address
        a.handle(&Event::Key(Key::Char('n')), &cx);
        let g = draw(&mut a, &cx, 1280, 760);
        let _ = g;
        for c in "Night Jazz".chars() {
            a.handle(&Event::Key(Key::Char(c)), &cx);
        }
        a.handle(&Event::Key(Key::Tab), &cx);
        for c in "https://jazz.example.org/live.mp3".chars() {
            a.handle(&Event::Key(Key::Char(c)), &cx);
        }
        a.handle(&Event::Key(Key::Enter), &cx);
        a.handle(&Event::Tab(0), &cx);
        assert_eq!(a.station_names(), vec!["Night Jazz".to_string()], "{:?}", a.status_text());
        // a bad address is explained and nothing is added
        a.handle(&Event::Key(Key::Char('n')), &cx);
        for c in "Bad".chars() {
            a.handle(&Event::Key(Key::Char(c)), &cx);
        }
        a.handle(&Event::Key(Key::Tab), &cx);
        for c in "not an address".chars() {
            a.handle(&Event::Key(Key::Char(c)), &cx);
        }
        a.handle(&Event::Key(Key::Enter), &cx);
        assert!(matches!(a.mode, Mode::Form(_)), "the form stays open with the reason");
        a.handle(&Event::Key(Key::Esc), &cx);
        // the station in My stations has a Remove button, which asks first
        let g = draw(&mut a, &cx, 1280, 760);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(301)), "a Remove button on the chosen station");
        a.handle(&Event::Tab(301), &cx);
        assert!(matches!(a.mode, Mode::ConfirmDelete(_)));
        draw(&mut a, &cx, 1280, 760);
        a.handle(&Event::Key(Key::Char('n')), &cx); // anything but y keeps it
        assert_eq!(a.station_names().len(), 1);
        a.handle(&Event::Tab(301), &cx);
        a.handle(&Event::Key(Key::Char('y')), &cx);
        assert!(a.station_names().is_empty());
        // the list scrolls to show the chosen language, and the wheel over it moves it
        a.handle(&Event::Tab(TABS - 1), &cx);
        draw(&mut a, &cx, 1280, 560);
        assert!(a.nav_top > 0, "the list moved down to the last language");
        // the narrow window falls back to tabs
        let g = draw(&mut a, &cx, 640, 480);
        assert!(g.hits.iter().any(|h| matches!(h.ev, Event::Tab(_))));
    }

    #[test]
    fn an_aac_station_without_the_add_on_offers_to_install_it() {
        struct AacOnly;
        impl Streamer for AacOnly {
            fn open(&self, _u: &str, _h: &[(String, String)]) -> Result<StreamHead, String> {
                Ok(StreamHead { status: 200, headers: vec![("content-type".into(), "audio/aacp".into())], body: Box::new(std::io::Cursor::new(vec![0u8; 64])) })
            }
        }
        let (cx, _) = cx("addon");
        cx.radio().set_streamer(Arc::new(AacOnly));
        cx.services.real().radio.set_aac_factory(Arc::new(|| Err(iyakki_agent::audiofmt::NEEDS_AUDIO_ADDON.to_string())));
        let mut t = TileHarness::new(RadioApp::new(&CxPort(&cx)), cx.clone());
        t.app.set_tab(1);
        t.app.sel = t.app.station_names().iter().position(|n| n == "80’s Tamil Hits").expect("80’s Tamil Hits is in the Tamil list");
        t.key(Key::Enter);
        wait(&cx, |s| matches!(s, RadioState::Failed { why, .. } if iyakki_agent::audiofmt::needs_addon(why)));
        assert!(t.app.take_request().is_none(), "nothing is installed without a click");
        t.draw_at(1100, 720);
        t.click_label("Install audio add-on");
        assert_eq!(t.app.take_request(), Some(("quickinstall".to_string(), "ffmpeg".to_string())));
        assert_eq!(t.app.retry.as_ref().map(|s| s.name.as_str()), Some("80’s Tamil Hits"), "the station waits to be tried again");
        assert!(t.app.take_request().is_none(), "one click, one request");
        // playing another station forgets the wait
        t.app.sel = 0;
        t.key(Key::Enter);
        assert!(t.app.retry.is_none());
        cx.radio().stop();
    }

    #[test]
    fn the_host_policy_and_deleting() {
        let (mut cx, fake) = cx("radio2");
        let mut a = RadioApp::new(&CxPort(&cx));
        a.handle(&Event::Tab(4), &cx);
        a.handle(&Event::Key(Key::Char('a')), &cx);
        a.handle(&Event::Tab(3), &cx);
        a.handle(&Event::Key(Key::Char('a')), &cx);
        a.handle(&Event::Tab(0), &cx);
        assert_eq!(a.station_names().len(), 2);
        a.handle(&Event::Key(Key::Char('d')), &cx);
        a.handle(&Event::Key(Key::Char('n')), &cx);
        assert_eq!(a.station_names().len(), 2, "anything but y cancels");
        a.handle(&Event::Key(Key::Char('d')), &cx);
        a.handle(&Event::Key(Key::Char('y')), &cx);
        assert_eq!(a.station_names().len(), 1);
        // an account that may not reach the host cannot play, and nothing is contacted
        cx.session.role = Role::User;
        cx.session.policy = Role::User.template();
        cx.session.policy.net_hosts = vec!["other.example".into()];
        let mut b = RadioApp::new(&CxPort(&cx));
        b.handle(&Event::Key(Key::Enter), &cx);
        assert!(b.status_text().unwrap().contains("may not reach"), "{:?}", b.status_text());
        assert!(fake.0.lock().unwrap().is_empty());
        assert!(!b.help().is_empty());
    }

    /// A server that sends its leaf only: the usual check fails, the exact certificate approved by a person plays.
    struct NoChain(Vec<u8>, Mutex<Vec<String>>);
    impl Streamer for NoChain {
        fn open(&self, url: &str, _h: &[(String, String)]) -> Result<StreamHead, String> {
            self.1.lock().unwrap().push(format!("open {url}"));
            Err("io: invalid peer certificate: UnknownIssuer".into())
        }
        fn open_tls(&self, url: &str, h: &[(String, String)], mode: &iyakki_proxy::TlsMode) -> Result<StreamHead, String> {
            self.1.lock().unwrap().push(format!("tls {url}"));
            match mode {
                iyakki_proxy::TlsMode::Pin(fp) if certinfo::norm_fingerprint(fp) == certinfo::norm_fingerprint(&certinfo::fingerprint(&self.0)) => Fake(Mutex::new(vec![])).open(url, h),
                _ => Err("io: invalid peer certificate: UnknownIssuer".into()),
            }
        }
        fn peer_chain(&self, _: &str, _: u16) -> Result<Vec<Vec<u8>>, String> {
            Ok(vec![self.0.clone()])
        }
    }

    fn chain_world(tag: &str, years: Option<(i32, i32)>) -> (AppCx, Arc<NoChain>, String) {
        let (cx, _) = cx(tag);
        let leaf = iyakki_proxy::testcerts::leaf_der(&["radios.example.org"], years, None);
        let fp = certinfo::fingerprint(&leaf);
        let net = Arc::new(NoChain(leaf, Mutex::new(vec![])));
        cx.radio().set_streamer(net.clone());
        store(&cx).add("Hello FM", "https://radios.example.org:8002/3").unwrap();
        (cx, net, fp)
    }

    fn pin_of(cx: &AppCx) -> Option<radio::CertPin> {
        store(cx).list().iter().find(|s| s.name == "Hello FM").and_then(|s| s.cert_pin.clone())
    }

    fn audit_has(cx: &AppCx, what: &str) -> bool {
        cx.audit.recent(50).iter().any(|l| l.contains(what))
    }

    #[test]
    fn a_certificate_is_trusted_only_after_the_person_confirms_and_can_be_forgotten() {
        let (cx, net, fp) = chain_world("radio-trust", None);
        let mut t = TileHarness::new(RadioApp::new(&CxPort(&cx)), cx.clone());
        t.key(Key::Enter);
        wait(&cx, |s| matches!(s, RadioState::Failed { .. }));
        t.draw_at(1280, 800);
        assert!(t.shows("did not send its full certificate chain"), "{:?}", t.texts());
        assert!(t.has_button("Review certificate… (t)"));
        assert!(pin_of(&cx).is_none() && !t.shows("trusted certificate"));
        // the question: what the server showed, a plain warning, Cancel is where it starts
        t.click_label("Review certificate… (t)");
        assert!(matches!(t.app.mode, Mode::Trust { focus: 0, .. }));
        for needle in ["radios.example.org:8002", "Hello FM", "someone on your network could pretend to be it", &radio::fingerprint_groups(&fp)[..8]] {
            assert!(t.shows(needle), "{needle}: {:?}", t.texts());
        }
        assert!(t.has_button("Cancel") && t.has_button("Trust this certificate"));
        // letters do not answer it; Enter on the first button, Esc and Cancel store nothing
        t.key(Key::Char('y'));
        t.key(Key::Char('t'));
        assert!(matches!(t.app.mode, Mode::Trust { .. }) && pin_of(&cx).is_none());
        t.key(Key::Enter);
        assert!(matches!(t.app.mode, Mode::Browse) && pin_of(&cx).is_none(), "Enter on the default Cancel");
        t.key(Key::Char('t'));
        t.key(Key::Esc);
        assert!(matches!(t.app.mode, Mode::Browse) && pin_of(&cx).is_none());
        t.key(Key::Char('t'));
        t.click_label("Cancel");
        assert!(pin_of(&cx).is_none() && !audit_has(&cx, "radio.trust"), "cancelling stores and audits nothing");
        assert!(!matches!(cx.radio().state(), RadioState::Playing { .. }));
        assert!(!net.1.lock().unwrap().iter().any(|l| l.starts_with("tls ")), "no unchecked connection was made");
        // confirming: the exact certificate is kept with the station, audited in short form, and the station plays
        t.key(Key::Char('t'));
        t.key(Key::Right);
        assert!(matches!(t.app.mode, Mode::Trust { focus: 1, .. }));
        t.key(Key::Enter);
        let pin = pin_of(&cx).expect("kept after the confirmation");
        assert_eq!((pin.host.as_str(), pin.port, pin.sha256.as_str()), ("radios.example.org", 8002, fp.as_str()));
        assert!(audit_has(&cx, &format!("radio.trust")) && audit_has(&cx, &radio::short_fingerprint(&fp)) && !cx.audit.recent(50).iter().any(|l| l.contains(&fp)));
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        t.draw_at(1280, 800);
        assert!(t.shows("trusted certificate") && t.has_button("Forget trust (f)"));
        // forgetting
        t.click_label("Forget trust (f)");
        assert!(pin_of(&cx).is_none() && audit_has(&cx, "radio.trust.revoked"));
        cx.radio().stop();
        // and the other button of the question also works
        t.key(Key::Enter);
        wait(&cx, |s| matches!(s, RadioState::Failed { .. }));
        t.draw_at(1280, 800);
        t.click_label("Review certificate… (t)");
        t.click_label("Trust this certificate");
        assert!(pin_of(&cx).is_some());
        cx.radio().stop();
    }

    #[test]
    fn a_certificate_that_cannot_work_is_shown_but_cannot_be_trusted() {
        let (cx, _net, _) = chain_world("radio-trust-old", Some((2019, 2020)));
        let mut t = TileHarness::new(RadioApp::new(&CxPort(&cx)), cx.clone());
        t.key(Key::Enter);
        wait(&cx, |s| matches!(s, RadioState::Failed { .. }));
        t.draw_at(1280, 800);
        assert!(t.shows("expired") && !t.has_button("Review certificate… (t)") && t.shows("cannot be trusted"), "{:?}", t.texts());
        t.key(Key::Char('t'));
        assert!(matches!(t.app.mode, Mode::Browse) && pin_of(&cx).is_none());
    }

    /// `IYAKKI_SHOT=/path.png cargo test -p iyakki-shell --lib radio_picture -- --ignored`: the player drawn while a live station plays.
    #[test]
    #[ignore]
    fn radio_picture() {
        let Ok(out) = std::env::var("IYAKKI_SHOT") else { return };
        let (cx, _) = cx("radio-pic");
        cx.radio().set_streamer(Arc::new(iyakki_proxy::https::UreqStreamer::default()));
        let mut a = RadioApp::new(&CxPort(&cx));
        a.handle(&Event::Tab(4), &cx);
        a.handle(&Event::Key(Key::Enter), &cx);
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        let mut g = Gfx::new(1280, 720);
        for t in 0..60u64 {
            std::thread::sleep(std::time::Duration::from_millis(40));
            g = Gfx::new(1280, 720);
            g.clear(rgb(9, 11, 20));
            a.render(&mut g, R::new(0.0, 46.0, 1280.0, 674.0), 1.0, &cx, 200 + t);
        }
        g.save_png(std::path::Path::new(&out)).unwrap();
        cx.radio().stop();
    }

    fn draw(a: &mut RadioApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
        let mut g = Gfx::new(w, h);
        a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), (w as f32 / 1280.0).clamp(0.6, 3.0), cx, 0);
        g
    }

    fn wait_search(a: &mut RadioApp, cx: &AppCx) {
        let t0 = std::time::Instant::now();
        while matches!(a.mode, Mode::Searching { .. }) && t0.elapsed() < std::time::Duration::from_secs(10) {
            a.poll_search();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert!(!matches!(a.mode, Mode::Searching { .. }), "the search finished");
        let _ = cx;
    }

    #[test]
    fn the_search_box_finds_stations_shows_them_as_the_last_entry_plays_one_and_keeps_one() {
        let (cx, fake) = cx("radiofind1");
        let mut a = RadioApp::new(&CxPort(&cx));
        assert!(a.found.is_none());
        // the key opens the box; an empty search says what to type and finds nothing
        a.handle(&Event::Key(Key::Char('/')), &cx);
        assert!(matches!(a.mode, Mode::Find(_)));
        a.handle(&Event::Key(Key::Enter), &cx);
        assert!(matches!(a.mode, Mode::Browse), "an empty search closes the box");
        assert!(a.found.is_none());
        assert!(a.status_text().unwrap_or("").contains("Type a name"), "{:?}", a.status_text());
        // the button opens it too
        a.handle(&Event::Tab(330), &cx);
        assert!(matches!(a.mode, Mode::Find(_)));
        typ(&mut a, &cx, "tamil");
        a.handle(&Event::Key(Key::Enter), &cx);
        assert!(matches!(a.mode, Mode::Searching { .. }), "the search runs on its own thread");
        wait_search(&mut a, &cx);
        let found = a.found.as_ref().expect("results");
        assert!(!found.stations.is_empty(), "the built-in Tamil list matches even with no internet");
        assert_eq!(a.tab, TABS, "the results are the entry after the languages and are shown");
        assert_eq!(a.tab_name(), "Search results");
        assert_eq!(a.tabs(), TABS + 1);
        assert!(a.status_text().unwrap_or("").contains("tamil"), "{:?}", a.status_text());
        assert!(a.status_text().unwrap_or("").contains("No internet"), "the missing directory is said: {:?}", a.status_text());
        // the left and right keys walk onto and off the results; Enter plays the chosen one
        a.handle(&Event::Key(Key::Right), &cx);
        assert_eq!(a.tab, 0);
        a.handle(&Event::Key(Key::Left), &cx);
        assert_eq!(a.tab, TABS);
        a.handle(&Event::Key(Key::Enter), &cx);
        wait(&cx, |s| matches!(s, RadioState::Playing { .. }));
        assert_eq!(fake.0.lock().unwrap().len(), 1);
        // a keeps it in My stations
        let before = a.mine.len();
        a.handle(&Event::Key(Key::Char('a')), &cx);
        assert_eq!(a.mine.len(), before + 1);
        assert!(cx.audit.recent(30).iter().any(|l| l.contains("radio.search")), "the search is audited");
        // a second search replaces the results
        a.handle(&Event::Key(Key::Char('/')), &cx);
        typ(&mut a, &cx, "zzzzqqqq");
        a.handle(&Event::Key(Key::Enter), &cx);
        wait_search(&mut a, &cx);
        assert!(a.found.as_ref().unwrap().stations.is_empty());
        assert!(a.status_text().unwrap_or("").starts_with("No station matches"), "{:?}", a.status_text());
        cx.radio().stop();
    }

    #[test]
    fn the_search_button_and_results_entry_draw_inside_the_window_at_four_sizes_and_esc_gives_a_search_up() {
        let (cx, _f) = cx("radiofind2");
        let mut a = RadioApp::new(&CxPort(&cx));
        a.handle(&Event::Key(Key::Char('/')), &cx);
        typ(&mut a, &cx, "tamil");
        a.handle(&Event::Key(Key::Enter), &cx);
        wait_search(&mut a, &cx);
        for &(w, h) in &[(1280u32, 720u32), (1920, 1080), (1024, 600), (800, 480)] {
            let g = draw(&mut a, &cx, w, h);
            let search: Vec<_> = g.hits.iter().filter(|x| x.ev == Event::Tab(330)).collect();
            assert_eq!(search.len(), 1, "{w}x{h}: one Search button");
            let r = search[0].rect;
            assert!(r.x >= 0.0 && r.x + r.w <= w as f32 + 0.5 && r.y >= 0.0 && r.y + r.h <= h as f32 + 0.5, "{w}x{h}: Search inside the window: {r:?}");
            assert!(g.hits.iter().any(|x| x.ev == Event::Tab(TABS)), "{w}x{h}: the Search results entry can be clicked");
        }
        // a click on the entry shows the results
        a.handle(&Event::Tab(0), &cx);
        a.handle(&Event::Tab(TABS), &cx);
        assert_eq!(a.tab, TABS);
        // Esc while a search runs gives it up (a search that never answers)
        let (_tx, rx) = std::sync::mpsc::channel();
        a.mode = Mode::Searching { words: "x".into(), rx };
        draw(&mut a, &cx, 1280, 720);
        a.handle(&Event::Key(Key::Esc), &cx);
        assert!(matches!(a.mode, Mode::Browse));
        assert!(a.status_text().unwrap_or("").contains("stopped"));
    }

    #[test]
    fn the_toolkit_free_fingerprint_helpers_equal_the_proxys() {
        for fp in ["aa:bb:cc:dd:ee:ff:00:11:22:33:44:55:66:77:88:99:aa:bb:cc:dd", "0011", "", "zz:XY:12:ab"] {
            assert_eq!(radio::norm_fingerprint(fp), certinfo::norm_fingerprint(fp), "{fp}");
            assert_eq!(radio::fingerprint_groups(fp), certinfo::fingerprint_groups(fp), "{fp}");
            assert_eq!(radio::short_fingerprint(fp), certinfo::short_fingerprint(fp), "{fp}");
        }
        for t in [0i64, 86_400, 1_700_000_000, 4_102_444_800] {
            assert_eq!(iyakki_tile_api::radio::date_text(t), certinfo::date_text(t));
        }
    }
}
