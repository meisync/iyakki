//! The MEDIA tile against a fake port: no signed-in `AppCx`, no real player program. The port gives the tile its own folders, index and download queue
//! (over a temp folder) and answers what the policy lets the person read and write; the tests prove that the tile asks the port before it plays, that
//! each change writes one audit line with the file name only, and that the tile draws from what the port gave it (the third tile moved onto the app
//! standard, docs/development/APP-DEVELOPMENT-PROCEDURE.md section 12). The same behaviours are also proved through the real port in `mod tests`.

use super::*;
use iyakki_agent::media::{Launcher, MediaPlayer, Running};
use std::path::PathBuf;

struct Run(Arc<Mutex<bool>>);
impl Running for Run {
    fn alive(&mut self) -> bool {
        *self.0.lock().unwrap()
    }
    fn stop(&mut self) {
        *self.0.lock().unwrap() = false;
    }
}

#[derive(Default)]
struct Launch {
    launched: Mutex<Vec<String>>,
}
impl Launcher for Launch {
    fn launch(&self, target: &str) -> Result<Box<dyn Running>, String> {
        self.launched.lock().unwrap().push(target.to_string());
        Ok(Box::new(Run(Arc::new(Mutex::new(true)))))
    }
}

struct Off;
impl iyakki_proxy::Upstream for Off {
    fn request(&self, _: &str, _: &str, _: &[(String, String)], _: &[u8]) -> Result<iyakki_proxy::UpstreamResponse, String> {
        Err("offline".into())
    }
}

struct FakePort {
    dir: PathBuf,
    music: PathBuf,
    log: Mutex<Vec<String>>,
    refused: Mutex<Vec<String>>,
    writable: Mutex<Vec<String>>,
    svc: Arc<crate::agent_handles_b2::services::Services>,
}

fn mp3(title: &str) -> Vec<u8> {
    let frame = |id: &str, t: &str| {
        let mut f = id.as_bytes().to_vec();
        f.extend(((t.len() + 1) as u32).to_be_bytes());
        f.extend([0, 0, 3]);
        f.extend(t.as_bytes());
        f
    };
    let body = [frame("TIT2", title), frame("TPE1", "Ann"), frame("TALB", "First")].concat();
    let mut v = b"ID3\x03\x00\x00".to_vec();
    let n = body.len();
    v.extend([0, 0, ((n >> 7) & 0x7F) as u8, (n & 0x7F) as u8]);
    v.extend(body);
    let mut fr = vec![0u8; 417];
    fr[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
    v.extend(fr);
    v
}

impl FakePort {
    fn new(tag: &str) -> FakePort {
        let dir = std::env::temp_dir().join(format!("iyakki-mediafake-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let music = dir.join("music");
        std::fs::create_dir_all(&music).unwrap();
        for (n, t) in [("a1.mp3", "Alpha"), ("a2.mp3", "Beta")] {
            std::fs::write(music.join(n), mp3(t)).unwrap();
        }
        let svc = crate::agent_handles_b2::services::Services::open(&dir.join("svc"), Arc::new(Off), Arc::new(Off)).unwrap();
        let p = FakePort { dir, music, log: Mutex::default(), refused: Mutex::default(), writable: Mutex::default(), svc };
        // the library is this one folder (the person's usual folders are not there in a test)
        let store = p.parts().store;
        store.add_folder(p.music.to_str().unwrap()).unwrap();
        for f in store.folders() {
            if f != p.music.to_str().unwrap() {
                let _ = store.remove_folder(&f);
            }
        }
        p
    }
    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

impl MediaPort for FakePort {
    fn user(&self) -> String {
        "ann".into()
    }
    fn policy(&self) -> iyakki_sec::policy::Policy {
        let mut p = iyakki_sec::policy::Role::User.template();
        p.fs_read = vec![self.music.to_str().unwrap().to_string()];
        p
    }
    fn may_read(&self, path: &str) -> bool {
        !self.refused.lock().unwrap().iter().any(|r| path.contains(r.as_str())) && path.starts_with(self.music.to_str().unwrap())
    }
    fn may_write(&self, path: &str) -> bool {
        self.writable.lock().unwrap().iter().any(|w| path.starts_with(w.as_str()))
    }
    fn parts(&self) -> MediaParts {
        MediaParts { store: MediaStore::new(&self.dir.join("svc"), "ann"), index: self.svc.media_index("ann"), dlq: self.svc.b5().media_queue("ann").unwrap() }
    }
    fn download_context(&self, folders: Vec<String>) -> iyakki_shared::mediaqueue::Who {
        iyakki_shared::mediaqueue::Who { policy: self.policy(), user: "ann".into(), folders }
    }
    fn planned_folder(&self, folders: &[String], kind: Option<ItemKind>) -> Option<PathBuf> {
        iyakki_agent::mediadl::planned_folder(folders, &self.policy(), "ann", kind)
    }
    fn web_profile(&self) -> Result<PathBuf, String> {
        Err("no web engine in a test".into())
    }
    fn audit(&self, action: &str, subject: &str) {
        self.log.lock().unwrap().push(format!("{action} {subject}"));
    }
}

fn open(tag: &str) -> (MediaApp, FakePort, Arc<Launch>) {
    let p = FakePort::new(tag);
    let l = Arc::new(Launch::default());
    let mut a = MediaApp::with_player(&p, Arc::new(MediaPlayer::new(l.clone())));
    for _ in 0..500 {
        a.handle_with(&Event::Tick, &p);
        if !a.is_scanning() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    (a, p, l)
}

#[test]
fn the_library_is_what_the_port_scans_and_a_track_plays_with_one_audit_line_naming_only_the_file() {
    let (mut a, p, l) = open("play");
    let names: Vec<String> = a.lib.tracks.iter().map(|t| t.title.clone()).collect();
    assert_eq!(names.len(), 2, "{names:?}");
    // Enter plays the chosen track; the list after it is the queue
    a.handle_with(&Event::Key(Key::Enter), &p);
    assert_eq!(l.launched.lock().unwrap().len(), 1, "the player was started once");
    assert!(l.launched.lock().unwrap()[0].starts_with(p.music.to_str().unwrap()));
    let plays: Vec<String> = p.log().into_iter().filter(|x| x.starts_with("media.play ")).collect();
    assert_eq!(plays.len(), 1, "{:?}", p.log());
    assert!(plays[0].ends_with(".mp3") && !plays[0].contains('/'), "the audit line names the file, not the path: {}", plays[0]);
    assert_eq!(a.player.queue_state().user, "ann");
    assert!(a.player.queue_state().active && a.player.queue_state().items.len() == 2);
}

#[test]
fn a_file_the_port_refuses_to_read_is_never_started_and_the_person_is_told() {
    let (mut a, p, l) = open("refuse");
    let first = a.lib.tracks[0].path.clone();
    p.refused.lock().unwrap().push(first.rsplit('/').next().unwrap().to_string());
    a.start_queue(vec![first], 0, &p);
    assert!(l.launched.lock().unwrap().is_empty(), "nothing was started");
    assert!(a.status.as_ref().is_some_and(|s| s.0.contains("may not read") && !s.1), "{:?}", a.status);
    assert!(!p.log().iter().any(|x| x.starts_with("media.play")), "{:?}", p.log());
    assert!(!a.queue_active);
}

#[test]
fn rating_a_track_is_kept_in_the_index_the_port_gave_and_audited() {
    let (mut a, p, _) = open("rate");
    let path = a.selected_path().expect("a track is chosen");
    a.rate_selected(4, &p);
    let id = mediaindex::item_id(&path);
    assert_eq!(a.index.lock().unwrap().get(&id).map(|i| i.rating), Some(4));
    let file = path.rsplit('/').next().unwrap();
    assert!(p.log().contains(&format!("media.rate {file} 4")), "{:?}", p.log());
    a.rate_selected(0, &p);
    assert_eq!(a.index.lock().unwrap().get(&id).map(|i| i.rating), Some(0));
}

#[test]
fn downloads_may_go_only_to_folders_the_port_says_are_writable() {
    let (a, p, _) = open("dlfolders");
    assert!(a.download_folders(&p).is_empty(), "nothing is writable yet");
    p.writable.lock().unwrap().push(p.music.to_str().unwrap().to_string());
    let ok = a.download_folders(&p);
    assert!(ok.iter().any(|f| f == p.music.to_str().unwrap()), "{ok:?}");
    assert!(ok.iter().all(|f| p.may_write(&format!("{f}/x"))), "only what the port allows: {ok:?}");
}

#[test]
fn the_tile_draws_every_page_at_the_usual_sizes_from_what_the_port_gave_it() {
    use crate::gfx::Gfx;
    let (mut a, p, _) = open("draw");
    for page in [Page::Tracks, Page::Artists, Page::Albums, Page::Playlists, Page::Folders] {
        a.page = page;
        for (w, h) in [(360u32, 740u32), (800, 480), (1280, 720), (1920, 1080)] {
            let mut g = Gfx::new(w, h);
            a.render_with(&mut g, R::new(0.0, 0.0, w as f32, h as f32), 1.0, &p, 3);
        }
    }
}
