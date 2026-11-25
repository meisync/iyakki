//! The media library: which music and video files a person has in the folders their account may read (plus the built-in,
//! read-only sample folder), what they are called (tags where reading them is cheap, the file name otherwise), playlists kept per
//! person, and the shared player. MP3 plays inside IYAKKI itself (the radio player's in-house decoder, see [`SystemLauncher`]);
//! everything else (video, other audio formats, playlist files) is handed to an external player program (mpv, VLC, ffplay or
//! mplayer) started as a separate process by [`crate::voice`] and stopped again by killing its process group. A video on the
//! `web` image is played by the MEDIA app through the embedded web engine ([`crate::mediaserve`]).

pub use iyakki_shared::media::{MAX_DEPTH, MAX_FILES, Tags, be32, clean, describe_file, describe_path, ebml_vint, ext_kind, flac_info, id3_tags, id3_text, id3v1, le32, matroska_seconds, mp3_seconds, mp4_boxes, mp4_child, mp4_info, ogg_seconds, ogg_tags, read_at, scan, sniff, syncsafe, vorbis_comments, walk, wav_seconds, year_of};
use std::sync::{Arc, Mutex, OnceLock};

pub use iyakki_shared::media::{EXTENSIONS, MAX_ITEMS, MAX_PLAYLISTS};
pub use iyakki_shared::media::{MediaStore, Playlist, QueueState, SAMPLES_DIR, SAMPLES_ENV, SAMPLES_LABEL, SAMPLES_TAG, Saved, UNKNOWN_ALBUM, UNKNOWN_ARTIST, clock_seed, default_folders, is_mp3, is_sample_in, is_video_path, m3u_text, may_read, samples_dir, scan_folders, shuffled, valid_playlist_name};
pub use iyakki_shared::media::{Check, Kind, Library, Repeat, Track};















// ------------------------------------------------------------------ tags






















// ------------------------------------------------------------------ per-person settings and playlists












// ------------------------------------------------------------------ the external player

/// A started player process.
pub trait Running: Send {
    fn alive(&mut self) -> bool;
    fn stop(&mut self);
}

/// Starts the player on one file or playlist file.
pub trait Launcher: Send + Sync {
    fn launch(&self, target: &str) -> Result<Box<dyn Running>, String>;
    /// This launcher plays MP3 files inside IYAKKI itself (no other program), so a queue can be stepped through by the
    /// caller. False for a launcher that hands files to an external program.
    fn plays_mp3_in_process(&self) -> bool {
        false
    }
}

pub(crate) struct ChildRun(pub(crate) std::process::Child);

impl Running for ChildRun {
    fn alive(&mut self) -> bool {
        iyakki_core_api::child_status(&mut self.0).is_running()
    }
    fn stop(&mut self) {
        crate::voice::kill_child(&mut self.0);
    }
}

// ------------------------------------------------------------------ MP3 inside IYAKKI

/// What the in-house MP3 path needs: the radio's player (it decodes with `nanomp3` and writes WAV to the sound command) and
/// the sound command from Settings → Voice.
#[derive(Clone)]
struct Builtin {
    radio: crate::radio::RadioPlayer,
    play_cmd: Arc<dyn Fn() -> String + Send + Sync>,
}

static BUILTIN: Mutex<Option<Builtin>> = Mutex::new(None);

/// Let the shared player play MP3 files itself, through `radio` (the services call this once they exist).
pub fn set_builtin_audio(radio: crate::radio::RadioPlayer, play_cmd: Arc<dyn Fn() -> String + Send + Sync>) {
    *BUILTIN.lock().unwrap_or_else(|e| e.into_inner()) = Some(Builtin { radio, play_cmd });
}

fn builtin() -> Option<Builtin> {
    BUILTIN.lock().unwrap_or_else(|e| e.into_inner()).clone()
}


/// A track playing in the radio player. It lives while the radio player is on *this* file.
struct BuiltinRun {
    radio: crate::radio::RadioPlayer,
    name: String,
}

impl Running for BuiltinRun {
    fn alive(&mut self) -> bool {
        use crate::radio::RadioState::*;
        match self.radio.state() {
            Connecting(n) | Playing { station: n, .. } => n == self.name,
            _ => false,
        }
    }
    fn stop(&mut self) {
        if self.alive() {
            self.radio.stop();
        }
    }
}

fn launch_builtin(b: &Builtin, target: &str) -> Result<Box<dyn Running>, String> {
    let name = format!("{} (file)", target.rsplit('/').next().unwrap_or("audio"));
    let allow: Arc<dyn Fn(&str) -> bool + Send + Sync> = Arc::new(|_| false);
    b.radio.play(&crate::radio::Station::new(&name, &format!("file://{target}")), allow, &(b.play_cmd)());
    // a file starts at once or fails at once (a missing sound command, a file that is not MP3)
    for _ in 0..300 {
        match b.radio.state() {
            crate::radio::RadioState::Playing { .. } => return Ok(Box::new(BuiltinRun { radio: b.radio.clone(), name })),
            crate::radio::RadioState::Failed { why, .. } => return Err(format!("could not play it: {why}")),
            _ => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
    b.radio.stop();
    Err("the sound player did not start (is the speaker set up in Settings → Voice?)".into())
}

/// The real thing: MP3 in IYAKKI's own player when it is wired up; everything else (video, other audio formats, playlists)
/// through mpv, VLC, ffplay or mplayer (or the configured command) as a separate process.
pub struct SystemLauncher {
    pub configured: String,
}

impl Launcher for SystemLauncher {
    fn launch(&self, target: &str) -> Result<Box<dyn Running>, String> {
        if is_mp3(target) {
            if let Some(b) = builtin() {
                return launch_builtin(&b, target);
            }
        }
        let argv = crate::voice::find_media_player(&self.configured).ok_or_else(|| {
            if is_video_path(target) {
                "Video needs the web image or an installed player (mpv, VLC, ffplay)".to_string()
            } else {
                "this format needs an installed media player (mpv, VLC, ffplay or mplayer); MP3 plays in IYAKKI itself".to_string()
            }
        })?;
        Ok(Box::new(ChildRun(crate::voice::spawn_media_player(&argv, target)?)))
    }
    fn plays_mp3_in_process(&self) -> bool {
        builtin().is_some()
    }
}


// ------------------------------------------------------------------ the built-in sample folder







struct Current {
    run: Box<dyn Running>,
    user: String,
    label: String,
    target: String,
    since: std::time::Instant,
}

/// At most one thing plays at a time; starting another stops the first.
pub struct MediaPlayer {
    launcher: Arc<dyn Launcher>,
    cur: Mutex<Option<Current>>,
    queue: Mutex<QueueState>,
}


impl MediaPlayer {
    pub fn new(launcher: Arc<dyn Launcher>) -> MediaPlayer {
        MediaPlayer { launcher, cur: Mutex::new(None), queue: Mutex::new(QueueState::default()) }
    }

    /// MP3 plays inside IYAKKI (see [`Launcher::plays_mp3_in_process`]).
    pub fn plays_mp3_in_process(&self) -> bool {
        self.launcher.plays_mp3_in_process()
    }

    pub fn play(&self, user: &str, label: &str, target: &str) -> Result<(), String> {
        self.stop();
        let run = self.launcher.launch(target)?;
        *self.cur.lock().unwrap_or_else(|e| e.into_inner()) = Some(Current { run, user: user.into(), label: label.into(), target: target.into(), since: std::time::Instant::now() });
        Ok(())
    }

    /// Replace the shared queue; returns its new revision.
    pub fn set_queue(&self, user: &str, items: Vec<String>, pos: usize, active: bool) -> u64 {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        *q = QueueState { user: user.into(), items, pos, active, rev: q.rev + 1 };
        q.rev
    }

    pub fn queue_state(&self) -> QueueState {
        self.queue.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Step the shared queue by `delta` (1 next, -1 previous) and play that track: the queue must be active and the new place inside it.
    /// Returns the name of what plays now. The MEDIA app, when open, adopts the queue's new place by its revision.
    pub fn skip(&self, delta: i64) -> Option<String> {
        let q = self.queue_state();
        if !q.active || q.items.is_empty() || q.pos >= q.items.len() {
            return None;
        }
        let at = q.pos as i64 + delta;
        if at < 0 || at as usize >= q.items.len() {
            return None;
        }
        let path = q.items[at as usize].clone();
        let label = path.rsplit('/').next().unwrap_or(&path).to_string();
        self.play(&q.user, &label, &path).ok()?;
        self.set_queue(&q.user, q.items, at as usize, true);
        Some(label)
    }

    /// The end of a person's session (D-SESSION-END): what plays for them stops and their queue is dropped. A track started by somebody else
    /// is left alone. Returns whether something stopped.
    pub fn end_session(&self, user: &str) -> bool {
        let mut ended = false;
        {
            let mut g = self.cur.lock().unwrap_or_else(|e| e.into_inner());
            if g.as_ref().is_some_and(|c| c.user == user) {
                if let Some(mut c) = g.take() {
                    c.run.stop();
                    ended = true;
                }
            }
        }
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.user == user && (q.active || !q.items.is_empty()) {
            *q = QueueState { rev: q.rev + 1, ..QueueState::default() };
            ended = true;
        }
        ended
    }

    /// The Bluetooth headphones left by themselves (F-LIFE-14, P-05): the program that plays cannot be paused (it offers only alive and stop), so
    /// it is stopped and the queue and its place are kept; the person presses play and it carries on from the same track on the speakers. True
    /// when something was playing.
    pub fn pause_for_route_lost(&self) -> bool {
        match self.cur.lock().unwrap_or_else(|e| e.into_inner()).take() {
            Some(mut c) => {
                c.run.stop();
                true
            }
            None => false,
        }
    }

    pub fn stop(&self) {
        if let Some(mut c) = self.cur.lock().unwrap_or_else(|e| e.into_inner()).take() {
            c.run.stop();
        }
    }

    pub fn check(&self) -> Check {
        let mut g = self.cur.lock().unwrap_or_else(|e| e.into_inner());
        let Some(c) = g.as_mut() else { return Check::Idle };
        if c.run.alive() {
            return Check::Playing { user: c.user.clone(), label: c.label.clone() };
        }
        let user = g.take().map(|c| c.user).unwrap_or_default();
        Check::Ended { user }
    }

    /// The file (or playlist file) that plays now, if anything.
    pub fn now_target(&self) -> Option<String> {
        let mut g = self.cur.lock().unwrap_or_else(|e| e.into_inner());
        let c = g.as_mut()?;
        c.run.alive().then(|| c.target.clone())
    }

    /// Seconds since the current thing was started.
    pub fn elapsed(&self) -> Option<u64> {
        let mut g = self.cur.lock().unwrap_or_else(|e| e.into_inner());
        let c = g.as_mut()?;
        c.run.alive().then(|| c.since.elapsed().as_secs())
    }

    /// What plays now, if anything (looking does not use up the "ended" notice that [`MediaPlayer::check`] gives).
    pub fn now_playing(&self) -> Option<String> {
        let mut g = self.cur.lock().unwrap_or_else(|e| e.into_inner());
        let c = g.as_mut()?;
        c.run.alive().then(|| c.label.clone())
    }
}

/// The player the whole shell shares, so that music goes on when the MEDIA app is left.
pub fn shared() -> Arc<MediaPlayer> {
    static P: OnceLock<Arc<MediaPlayer>> = OnceLock::new();
    P.get_or_init(|| Arc::new(MediaPlayer::new(Arc::new(SystemLauncher { configured: String::new() })))).clone()
}

#[cfg(test)]
pub mod tests {
    use super::*;

    pub fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("iyakki-media-{}-{}", std::process::id(), tag));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn id3_frame(id: &str, text: &str) -> Vec<u8> {
        let mut data = vec![3u8];
        data.extend(text.as_bytes());
        let mut f = id.as_bytes().to_vec();
        f.extend((data.len() as u32).to_be_bytes());
        f.extend([0, 0]);
        f.extend(data);
        f
    }

    /// An MP3 with an ID3v2.3 tag and `frames` silent 128 kbps frames.
    pub fn mp3(tags: &[(&str, &str)], frames: usize) -> Vec<u8> {
        let body: Vec<u8> = tags.iter().flat_map(|(i, t)| id3_frame(i, t)).collect();
        let mut v = b"ID3\x03\x00\x00".to_vec();
        let n = body.len();
        v.extend([((n >> 21) & 0x7F) as u8, ((n >> 14) & 0x7F) as u8, ((n >> 7) & 0x7F) as u8, (n & 0x7F) as u8]);
        v.extend(body);
        for _ in 0..frames {
            let mut fr = vec![0u8; 417];
            fr[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
            v.extend(fr);
        }
        v
    }

    fn comments(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut v = 6u32.to_le_bytes().to_vec();
        v.extend(b"iyakki");
        v.extend((pairs.len() as u32).to_le_bytes());
        for (k, val) in pairs {
            let e = format!("{k}={val}");
            v.extend((e.len() as u32).to_le_bytes());
            v.extend(e.as_bytes());
        }
        v
    }

    /// A FLAC file: STREAMINFO (44100 Hz, 441000 samples = 10 s) and a Vorbis comment block.
    pub fn flac(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut v = b"fLaC".to_vec();
        let mut si = vec![0u8; 34];
        // sample rate 44100 in 20 bits, 1 channel, 16 bits, 441000 samples in 36 bits
        si[10] = (44100 >> 12) as u8;
        si[11] = (44100 >> 4) as u8;
        si[12] = ((44100 & 0xF) << 4) as u8;
        si[13] = ((441000u64 >> 32) & 0xF) as u8;
        si[14..18].copy_from_slice(&(441000u32).to_be_bytes());
        v.extend([0u8, 0, 0, 34]);
        v.extend(si);
        let c = comments(pairs);
        v.extend([0x84u8, (c.len() >> 16) as u8, (c.len() >> 8) as u8, c.len() as u8]);
        v.extend(c);
        v
    }

    /// An Ogg Vorbis start: a page holding the comment packet (no audio follows).
    pub fn ogg(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut v = b"OggS\0\x02".to_vec();
        v.extend([0u8; 20]);
        v.extend(b"\x03vorbis");
        v.extend(comments(pairs));
        v
    }

    pub fn wav(seconds: u32) -> Vec<u8> {
        let rate = 8000u32;
        let data = rate * seconds;
        let mut v = b"RIFF".to_vec();
        v.extend((36 + data).to_le_bytes());
        v.extend(b"WAVEfmt ");
        v.extend(16u32.to_le_bytes());
        v.extend([1, 0, 1, 0]);
        v.extend(rate.to_le_bytes());
        v.extend(rate.to_le_bytes());
        v.extend([1, 0, 8, 0]);
        v.extend(b"data");
        v.extend(data.to_le_bytes());
        v.extend(vec![128u8; 64]); // only the start of the samples
        v
    }

    fn mp4_box(ty: &[u8], content: &[u8]) -> Vec<u8> {
        let mut v = ((content.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend(ty);
        v.extend(content);
        v
    }

    /// An M4A with `moov` holding `mvhd` (1000 ticks a second, 90 s) and `©nam`/`©ART`/`©alb` items.
    pub fn m4a(title: &str, artist: &str, album: &str) -> Vec<u8> {
        let mut v = mp4_box(b"ftyp", b"M4A \0\0\0\0M4A isom");
        let mut mvhd = vec![0u8; 20];
        mvhd[12..16].copy_from_slice(&1000u32.to_be_bytes());
        mvhd[16..20].copy_from_slice(&90_000u32.to_be_bytes());
        let item = |ty: &[u8], text: &str| {
            let mut d = vec![0, 0, 0, 1, 0, 0, 0, 0];
            d.extend(text.as_bytes());
            mp4_box(ty, &mp4_box(b"data", &d))
        };
        let ilst: Vec<u8> = [item(b"\xA9nam", title), item(b"\xA9ART", artist), item(b"\xA9alb", album)].concat();
        let mut meta = vec![0u8; 4];
        meta.extend(mp4_box(b"ilst", &ilst));
        let udta = mp4_box(b"udta", &mp4_box(b"meta", &meta));
        let moov = mp4_box(b"moov", &[mp4_box(b"mvhd", &mvhd), udta].concat());
        v.extend(mdat());
        v.extend(moov); // moov after the media data, as in many files
        v
    }

    fn mdat() -> Vec<u8> {
        mp4_box(b"mdat", &[0u8; 100])
    }

    pub fn mp4_video() -> Vec<u8> {
        let mut v = mp4_box(b"ftyp", b"isom\0\0\0\0isomiso2");
        v.extend(mdat());
        v
    }

    pub fn mkv() -> Vec<u8> {
        let mut v = vec![0x1A, 0x45, 0xDF, 0xA3, 0x84, 0x42, 0x82, 0x84];
        v.extend(b"webm");
        v.extend([0u8; 20]);
        v
    }

    fn put(dir: &Path, rel: &str, bytes: &[u8]) -> String {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p.to_str().unwrap().to_string()
    }

    #[test]
    fn tags_and_lengths_are_read_from_each_container() {
        let d = temp("tags");
        let m = put(&d, "a.mp3", &mp3(&[("TIT2", "Sunrise"), ("TPE1", "The Band"), ("TALB", "Dawn")], 40));
        let f = put(&d, "b.flac", &flac(&[("TITLE", "Flac Song"), ("artist", "Flac Artist"), ("ALBUM", "Lossless")]));
        let o = put(&d, "c.ogg", &ogg(&[("TITLE", "Ogg Song"), ("ARTIST", "Ogg Artist")]));
        let w = put(&d, "d.wav", &wav(3));
        let a = put(&d, "e.m4a", &m4a("Mp4 Song", "Mp4 Artist", "Mp4 Album"));
        let lib = scan(&[d.to_str().unwrap().to_string()], &|_| true);
        let t = |p: &str| lib.find(p).unwrap().clone();
        assert_eq!((t(&m).title.as_str(), t(&m).artist.as_str(), t(&m).album.as_str()), ("Sunrise", "The Band", "Dawn"));
        assert!(t(&m).seconds.is_some_and(|s| s <= 2), "40 frames of 26 ms: {:?}", t(&m).seconds);
        assert_eq!((t(&f).title.as_str(), t(&f).artist.as_str(), t(&f).album.as_str(), t(&f).seconds), ("Flac Song", "Flac Artist", "Lossless", Some(10)));
        assert_eq!((t(&o).title.as_str(), t(&o).artist.as_str(), t(&o).album.as_str()), ("Ogg Song", "Ogg Artist", UNKNOWN_ALBUM));
        assert_eq!((t(&w).title.as_str(), t(&w).artist.as_str(), t(&w).seconds), ("d", UNKNOWN_ARTIST, Some(3)));
        assert_eq!((t(&a).title.as_str(), t(&a).artist.as_str(), t(&a).album.as_str(), t(&a).seconds), ("Mp4 Song", "Mp4 Artist", "Mp4 Album", Some(90)));
        assert_eq!(t(&a).kind, Kind::Audio, "an M4A brand is audio");
        assert_eq!(t(&f).duration_text(), "0:10");
    }

    #[test]
    fn ogg_and_webm_lengths_come_from_the_last_page_and_the_segment_info() {
        let d = temp("oggwebm");
        // Vorbis at 8000 Hz: identification header, then a last page with granule 8000 * 7
        let mut v = b"OggS\0\x02".to_vec();
        v.extend([0u8; 20]);
        v.extend(b"\x01vorbis");
        v.extend(0u32.to_le_bytes());
        v.push(1);
        v.extend(8000u32.to_le_bytes());
        v.extend([0u8; 12]);
        v.extend(b"\x03vorbis");
        v.extend(comments(&[("TITLE", "Len")]));
        v.extend(b"OggS\0\x04");
        v.extend((8000u64 * 7).to_le_bytes());
        v.extend([0u8; 12]);
        let o = put(&d, "v.ogg", &v);
        // Opus: pre-skip 312, granule 48000 * 5 + 312
        let mut op = b"OggS\0\x02".to_vec();
        op.extend([0u8; 20]);
        op.extend(b"OpusHead\x01\x02");
        op.extend(312u16.to_le_bytes());
        op.extend([0u8; 8]);
        op.extend(b"OpusTags");
        op.extend(comments(&[("TITLE", "Op")]));
        op.extend(b"OggS\0\x04");
        op.extend((48_000u64 * 5 + 312).to_le_bytes());
        op.extend([0u8; 12]);
        let p = put(&d, "o.opus", &op);
        // WebM: Info with TimecodeScale 1 000 000 and a 4-byte float Duration of 12500 ms
        let mut w = vec![0x1A, 0x45, 0xDF, 0xA3, 0x84, 0x42, 0x82, 0x84];
        w.extend(b"webm");
        w.extend([0x15, 0x49, 0xA9, 0x66, 0x8E, 0x2A, 0xD7, 0xB1, 0x83, 0x0F, 0x42, 0x40, 0x44, 0x89, 0x84]);
        w.extend(12500f32.to_be_bytes());
        let m = put(&d, "m.webm", &w);
        let lib = scan(&[d.to_str().unwrap().to_string()], &|_| true);
        assert_eq!(lib.find(&o).unwrap().seconds, Some(7));
        assert_eq!(lib.find(&p).unwrap().seconds, Some(5));
        assert_eq!(lib.find(&m).unwrap().seconds, Some(12));
    }

    #[test]
    fn id3v1_and_file_names_are_the_fallbacks() {
        let d = temp("fallback");
        let mut v1 = vec![0xFFu8, 0xFB, 0x90, 0x64];
        v1.extend(vec![0u8; 300]);
        let mut tag = b"TAG".to_vec();
        for s in ["Old Title", "Old Artist", "Old Album"] {
            let mut f = s.as_bytes().to_vec();
            f.resize(30, 0);
            tag.extend(f);
        }
        tag.extend([0u8; 35]);
        v1.extend(tag);
        let a = put(&d, "x.mp3", &v1);
        let b = put(&d, "Some Artist - Some_Song.mp3", &mp3(&[], 2));
        let c = put(&d, "plain_name.mp3", &mp3(&[], 2));
        let lib = scan(&[d.to_str().unwrap().to_string()], &|_| true);
        let (x, y, z) = (lib.find(&a).unwrap(), lib.find(&b).unwrap(), lib.find(&c).unwrap());
        assert_eq!((x.title.as_str(), x.artist.as_str(), x.album.as_str()), ("Old Title", "Old Artist", "Old Album"));
        assert_eq!((y.title.as_str(), y.artist.as_str()), ("Some Song", "Some Artist"));
        assert_eq!((z.title.as_str(), z.artist.as_str()), ("plain name", UNKNOWN_ARTIST));
    }

    #[test]
    fn files_are_recognised_by_extension_and_by_their_first_bytes() {
        let d = temp("kinds");
        put(&d, "v.mp4", &mp4_video());
        put(&d, "w.mkv", &mkv());
        put(&d, "noext", &mp3(&[("TIT2", "Hidden")], 2)); // no extension: bytes decide
        put(&d, "wrongname.dat", &mp3(&[("TIT2", "Hidden 2")], 2)); // an unknown extension is not listed
        put(&d, "notes.txt", b"hello");
        put(&d, "empty.mp3", b""); // media name, nothing inside: still listed by its name
        put(&d, "sub/deep/song.flac", &flac(&[("TITLE", "Deep")]));
        put(&d, "disguised.mp4", &flac(&[("TITLE", "Really audio")])); // the bytes win over the name
        let lib = scan(&[d.to_str().unwrap().to_string()], &|_| true);
        let names: Vec<_> = lib.tracks.iter().map(|t| (t.path.rsplit('/').next().unwrap().to_string(), t.kind)).collect();
        for want in [("v.mp4", Kind::Video), ("w.mkv", Kind::Video), ("noext", Kind::Audio), ("empty.mp3", Kind::Audio), ("song.flac", Kind::Audio), ("disguised.mp4", Kind::Audio)] {
            assert!(names.contains(&(want.0.to_string(), want.1)), "{want:?} in {names:?}");
        }
        assert!(!names.iter().any(|(n, _)| n == "notes.txt" || n == "wrongname.dat"), "{names:?}");
        assert_eq!(lib.videos("").len(), 2);
        assert_eq!(sniff(b"RIFF\0\0\0\0AVI LIST"), Some(("avi", Kind::Video)));
        assert_eq!(sniff(b"OggS\0\x02\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0OpusHead"), Some(("opus", Kind::Audio)));
    }

    #[test]
    fn the_scan_stays_inside_what_the_account_may_read() {
        let d = temp("scope");
        let ok = put(&d, "mine/a.mp3", &mp3(&[("TIT2", "Mine")], 2));
        put(&d, "theirs/b.mp3", &mp3(&[("TIT2", "Theirs")], 2));
        put(&d, "mine/private/c.mp3", &mp3(&[("TIT2", "Hidden")], 2));
        std::os::unix::fs::symlink(d.join("theirs"), d.join("mine/link")).unwrap();
        let mine = d.join("mine").to_str().unwrap().to_string();
        let private = d.join("mine/private").to_str().unwrap().to_string();
        let allow = move |p: &str| p.starts_with(&mine) && !p.starts_with(&private);
        let both = [d.join("mine").to_str().unwrap().to_string(), d.join("theirs").to_str().unwrap().to_string()];
        let lib = scan(&both, &allow);
        assert_eq!(lib.tracks.iter().map(|t| t.path.clone()).collect::<Vec<_>>(), vec![ok], "other folders, a forbidden subfolder and links are left out");
    }

    #[test]
    fn artists_albums_and_search() {
        let d = temp("browse");
        put(&d, "1.mp3", &mp3(&[("TIT2", "One"), ("TPE1", "Ann"), ("TALB", "First")], 2));
        put(&d, "2.mp3", &mp3(&[("TIT2", "Two"), ("TPE1", "Ann"), ("TALB", "First")], 2));
        put(&d, "3.mp3", &mp3(&[("TIT2", "Three"), ("TPE1", "Bob"), ("TALB", "Second")], 2));
        let lib = scan(&[d.to_str().unwrap().to_string()], &|_| true);
        assert_eq!(lib.artists(""), vec![("Ann".to_string(), 2), ("Bob".to_string(), 1)]);
        assert_eq!(lib.albums(""), vec![("Ann".to_string(), "First".to_string(), 2), ("Bob".to_string(), "Second".to_string(), 1)]);
        assert_eq!(lib.audio("thr").len(), 1);
        assert_eq!(lib.artists("bob"), vec![("Bob".to_string(), 1)]);
        assert_eq!(lib.by_album("Ann", "First").len(), 2);
        assert_eq!(lib.by_artist("Bob")[0].title, "Three");
    }

    #[test]
    fn playlists_and_flags_are_kept_per_person() {
        let d = temp("lists");
        let s = MediaStore::new(&d, "alice");
        assert_eq!(s.folders(), vec!["/home/alice/Music", "/home/alice/Videos"]);
        s.add_folder("/mnt/stick/Music/").unwrap();
        assert!(s.add_folder("/mnt/stick/Music").is_err() && s.add_folder("relative").is_err() && s.add_folder("/a/../b").is_err());
        s.remove_folder("/home/alice/Videos").unwrap();
        assert_eq!(s.folders(), vec!["/home/alice/Music", "/mnt/stick/Music"]);
        assert_eq!(s.create("  Road trip ").unwrap(), "Road trip");
        assert!(s.create("road TRIP").is_err() && s.create("").is_err());
        s.create("Focus").unwrap();
        for p in ["/m/a.mp3", "/m/b.mp3", "/m/c.mp3"] {
            s.add_item("Road trip", p).unwrap();
        }
        s.move_item("Road trip", 2, 0).unwrap();
        s.remove_item("Road trip", 1).unwrap();
        assert_eq!(s.playlist("Road trip").unwrap().items, vec!["/m/c.mp3", "/m/b.mp3"]);
        assert!(s.move_item("Road trip", 0, 5).is_err() && s.remove_item("Road trip", 9).is_err() && s.add_item("Nope", "/x").is_err());
        assert_eq!(s.rename("Focus", "Deep work").unwrap(), "Deep work");
        assert!(s.rename("Deep work", "road trip").is_err());
        s.set_flags(true, Repeat::One).unwrap();
        // a fresh handle (a restart) sees everything; another person sees nothing
        let again = MediaStore::new(&d, "alice");
        assert_eq!(again.playlists().iter().map(|p| p.name.clone()).collect::<Vec<_>>(), vec!["Road trip", "Deep work"]);
        assert_eq!(again.flags(), (true, Repeat::One));
        assert_eq!(again.folders()[1], "/mnt/stick/Music");
        let bob = MediaStore::new(&d, "bob");
        assert!(bob.playlists().is_empty() && bob.flags() == (false, Repeat::Off) && bob.folders()[0] == "/home/bob/Music");
        again.delete("Deep work").unwrap();
        assert!(again.delete("Deep work").is_err());
        assert_eq!(again.playlists().len(), 1);
    }

    #[test]
    fn m3u_files_and_shuffles() {
        let d = temp("m3u");
        let p = put(&d, "t.mp3", &mp3(&[("TIT2", "Song"), ("TPE1", "Ann")], 2));
        let lib = scan(&[d.to_str().unwrap().to_string()], &|_| true);
        let s = MediaStore::new(&d, "alice");
        let items = vec![p.clone(), "/elsewhere/x.flac".to_string(), "/bad\nname.mp3".to_string()];
        let path = s.write_m3u("Road trip!", &items, &lib).unwrap();
        assert!(path.starts_with(d.join("media-alice")) && path.to_str().unwrap().ends_with("Road_trip_.m3u"));
        let text = std::fs::read_to_string(path).unwrap();
        assert_eq!(text, format!("#EXTM3U\n#EXTINF:0,Ann - Song\n{p}\n/elsewhere/x.flac\n"));
        let many: Vec<String> = (0..30).map(|i| i.to_string()).collect();
        let (a, b) = (shuffled(&many, 7), shuffled(&many, 8));
        assert_ne!(a, many);
        assert_ne!(a, b);
        let mut sorted = a.clone();
        sorted.sort_by_key(|x| x.parse::<u32>().unwrap());
        assert_eq!(sorted, many, "a shuffle keeps every entry");
        assert_eq!(shuffled(&many, 7), a, "and is repeatable for one seed");
    }

    pub struct FakeRun(pub Arc<Mutex<(bool, bool)>>); // (alive, stopped)
    impl Running for FakeRun {
        fn alive(&mut self) -> bool {
            self.0.lock().unwrap().0
        }
        fn stop(&mut self) {
            let mut g = self.0.lock().unwrap();
            g.0 = false;
            g.1 = true;
        }
    }

    #[derive(Default)]
    pub struct FakeLauncher {
        pub launched: Mutex<Vec<String>>,
        pub procs: Mutex<Vec<Arc<Mutex<(bool, bool)>>>>,
        pub fail: Mutex<bool>,
    }
    impl Launcher for FakeLauncher {
        fn launch(&self, target: &str) -> Result<Box<dyn Running>, String> {
            if *self.fail.lock().unwrap() {
                return Err("no media player is installed".into());
            }
            self.launched.lock().unwrap().push(target.to_string());
            let st = Arc::new(Mutex::new((true, false)));
            self.procs.lock().unwrap().push(st.clone());
            Ok(Box::new(FakeRun(st)))
        }
    }

    #[test]
    fn the_player_starts_one_thing_stops_it_and_notices_its_end() {
        let l = Arc::new(FakeLauncher::default());
        let p = MediaPlayer::new(l.clone());
        assert_eq!(p.check(), Check::Idle);
        p.play("alice", "Song", "/m/a.mp3").unwrap();
        assert_eq!(p.now_playing().as_deref(), Some("Song"));
        p.play("alice", "Other", "/m/b.mp3").unwrap();
        assert!(l.procs.lock().unwrap()[0].lock().unwrap().1, "starting another stops the first");
        // the player quits by itself
        l.procs.lock().unwrap()[1].lock().unwrap().0 = false;
        assert_eq!(p.check(), Check::Ended { user: "alice".into() });
        assert_eq!(p.check(), Check::Idle);
        p.play("alice", "Third", "/m/c.mp3").unwrap();
        p.stop();
        assert!(l.procs.lock().unwrap()[2].lock().unwrap().1);
        assert_eq!(p.now_playing(), None);
        *l.fail.lock().unwrap() = true;
        assert!(p.play("alice", "x", "/y").unwrap_err().contains("no media player"));
    }

    #[test]
    fn a_lost_headset_stops_the_playing_track_and_keeps_the_queue() {
        let l = Arc::new(FakeLauncher::default());
        let p = MediaPlayer::new(l.clone());
        assert!(!p.pause_for_route_lost(), "nothing was playing");
        p.set_queue("alice", vec!["/m/a.mp3".into(), "/m/b.mp3".into()], 1, true);
        p.play("alice", "b", "/m/b.mp3").unwrap();
        assert!(p.pause_for_route_lost());
        assert!(l.procs.lock().unwrap()[0].lock().unwrap().1, "the program was stopped");
        assert_eq!(p.now_playing(), None);
        let q = p.queue_state();
        assert!(q.active && q.pos == 1 && q.items.len() == 2, "the queue and its place stay");
        assert!(!p.pause_for_route_lost(), "asked again: nothing to pause");
    }

    #[test]
    fn a_real_player_process_can_be_stopped() {
        // `sleep` stands in for a player: the launcher is given a program that ignores its argument and stays alive
        let argv = vec!["sleep".to_string(), "30".to_string()];
        let Ok(mut child) = crate::voice::spawn_media_player(&argv[..1], "30") else { return };
        assert!(iyakki_core_api::child_status(&mut child).is_running());
        crate::voice::kill_child(&mut child);
        assert!(iyakki_core_api::child_status(&mut child).is_over());
    }

    /// The sample files that ship in `dist/samples` (and in every image under /usr/share/iyakki/samples).
    fn sample(rel: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/samples").join(rel)
    }

    #[test]
    fn the_sample_mp3s_have_their_tags_and_decode_with_the_in_house_decoder() {
        for (file, title, genre, secs) in [("Music/IYAKKI Sample Melody.mp3", "IYAKKI Sample Melody", "melody", 26), ("Music/IYAKKI Sample Beats.mp3", "IYAKKI Sample Beats", "kuthu", 20)] {
            let p = sample(file);
            let t = describe_path(p.to_str().unwrap()).unwrap_or_else(|| panic!("{file} is a media file"));
            assert_eq!((t.title.as_str(), t.artist.as_str(), t.album.as_str(), t.genre.to_lowercase().as_str(), t.kind), (title, "IYAKKI", "IYAKKI Samples", genre, Kind::Audio), "{t:?}");
            assert!(t.seconds.is_some_and(|s| s.abs_diff(secs) <= 1), "{:?}", t.seconds);
            // the same decoder the radio and the MEDIA app use: every frame decodes, the length matches the tag
            let bytes = std::fs::read(&p).unwrap();
            assert!(bytes.len() < 400_000, "a small file");
            let skip = if bytes.starts_with(b"ID3") { 10 + ((bytes[6] as usize & 0x7f) << 21 | (bytes[7] as usize & 0x7f) << 14 | (bytes[8] as usize & 0x7f) << 7 | (bytes[9] as usize & 0x7f)) } else { 0 };
            let mut dec = nanomp3::Decoder::new();
            let mut pcm = vec![0i16; nanomp3::MAX_SAMPLES_PER_FRAME];
            let (mut pos, mut samples, mut rate, mut peak) = (skip, 0usize, 0u32, 0i16);
            while pos < bytes.len() {
                let (used, frame) = dec.decode(&bytes[pos..], &mut pcm);
                pos += used.max(1);
                if let Ok(f) = frame {
                    rate = f.sample_rate;
                    samples += f.samples_produced;
                    peak = peak.max(pcm[..f.samples_produced * f.channels as usize].iter().map(|v| v.saturating_abs()).max().unwrap_or(0));
                }
            }
            assert!(rate > 0 && (samples as f64 / rate as f64 - secs as f64).abs() < 1.0, "{file}: {} s decoded", samples as f64 / rate as f64);
            assert!(peak > 3000 && peak < 32700, "{file}: audible and not clipped, peak {peak}");
        }
        // the video is a WebM that the library scan lists as a video
        let v = describe_path(sample("Videos/IYAKKI Sample Video.webm").to_str().unwrap()).unwrap();
        assert_eq!((v.kind, v.title.as_str()), (Kind::Video, "IYAKKI Sample Video"));
        let total: u64 = ["Music/IYAKKI Sample Melody.mp3", "Music/IYAKKI Sample Beats.mp3", "Videos/IYAKKI Sample Video.webm"].iter().map(|f| std::fs::metadata(sample(f)).unwrap().len()).sum();
        assert!(total <= 4 << 20, "the samples are small: {total} bytes");
    }

    #[test]
    fn mp3_plays_through_the_in_house_player_and_everything_else_needs_an_external_one() {
        // the sound goes to the play command of this test: the process-wide mixer switch (flipped by other tests) must not take it
        let _route = crate::mixer::pin_command_route();
        let out = temp("builtin").join("sound.wav");
        let radio = crate::radio::RadioPlayer::default();
        let b = Builtin { radio: radio.clone(), play_cmd: Arc::new({ let o = out.display().to_string(); move || format!("tee {o}") }) };
        // the first 3 seconds of the beats sample (a debug build decodes slowly)
        let part = out.with_file_name("part.mp3");
        std::fs::write(&part, &std::fs::read(sample("Music/IYAKKI Sample Beats.mp3")).unwrap()[..36_000]).unwrap();
        let mut run = launch_builtin(&b, part.to_str().unwrap()).unwrap();
        let __wall = std::time::Instant::now();
        while __wall.elapsed() < std::time::Duration::from_secs(90) {
            if !run.alive() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(!run.alive(), "the track played to its end");
        // the sound command is a separate process (tee): it may still be starting or flushing when the decoder has finished
        let mut wav = Vec::new();
        let t0 = std::time::Instant::now();
        while t0.elapsed() < std::time::Duration::from_secs(20) {
            wav = std::fs::read(&out).unwrap_or_default();
            if wav.len() > 44 + 2 * 44100 * 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        assert_eq!(&wav[..4], b"RIFF");
        assert!(wav.len() > 44 + 2 * 44100 * 2, "seconds of 16-bit audio reached the sound command: {} bytes", wav.len());
        // a file that is not MP3 data says so instead of staying silent; a missing sound command too
        let junk = out.with_file_name("junk.mp3");
        std::fs::write(&junk, vec![9u8; 100_000]).unwrap();
        assert!(launch_builtin(&b, junk.to_str().unwrap()).is_err());
        let nocmd = Builtin { radio, play_cmd: Arc::new(|| "/nonexistent/player".to_string()) };
        assert!(launch_builtin(&nocmd, sample("Music/IYAKKI Sample Melody.mp3").to_str().unwrap()).is_err_and(|e| e.contains("could not play")));
        assert!(is_mp3("/a/B.MP3") && !is_mp3("/a/b.m4a") && !is_mp3("/a/mp3"));
        assert!(is_video_path("/a/b.WEBM") && is_video_path("c.mkv") && !is_video_path("c.mp3"));
    }

    #[test]
    fn the_sample_folder_is_read_only_scanned_and_readable_by_every_account() {
        let dir = Path::new("/usr/share/iyakki/samples");
        assert!(is_sample_in(dir, "/usr/share/iyakki/samples/Music/a.mp3") && is_sample_in(dir, "/usr/share/iyakki/samples/Videos/v.webm"));
        assert!(is_sample_in(dir, "/usr/share/iyakki/samples"), "the folder itself, so a scan can start in it");
        assert!(!is_sample_in(dir, "/usr/share/iyakki/samples/../../../etc/passwd") && !is_sample_in(dir, "/usr/share/iyakki/samples2/a.mp3") && !is_sample_in(dir, "/etc/passwd"));
        let own = vec!["/home/a/Music".to_string()];
        assert_eq!(scan_folders(&own, None), own);
        assert_eq!(scan_folders(&own, Some(dir)), vec!["/home/a/Music".to_string(), "/usr/share/iyakki/samples".to_string()]);
        assert_eq!(scan_folders(&scan_folders(&own, Some(dir)), Some(dir)).len(), 2, "not listed twice");
        // a scan of the real sample folder finds the three files whatever the account may read elsewhere
        let lib = scan(&[sample("").display().to_string()], &|_| true);
        let mut names: Vec<String> = lib.tracks.iter().map(|t| t.title.clone()).collect();
        names.sort();
        assert_eq!(names, vec!["IYAKKI Sample Beats", "IYAKKI Sample Melody", "IYAKKI Sample Video"]);
        // the account's own scope does not decide for the samples (the real folder exists only on an image)
        let p = iyakki_sec::policy::Role::User.template();
        assert!(!may_read(&p, "alice", "/etc/passwd"));
    }
}

#[cfg(test)]
use std::path::Path;
#[cfg(test)]
use std::path::PathBuf;
