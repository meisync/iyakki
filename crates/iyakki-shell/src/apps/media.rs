//! MEDIA: the music, videos and pictures in the folders this account may read, in a futuristic player. Browse by tracks,
//! artists, albums, videos, playlists, folders, *types* (kuthu, melody, beats, …), top rated and photos; search with `/`
//! in plain words ("melody songs by anirudh above 4 stars", "latest tamil beats", "photos from 2024"); rate with 1-5, tag
//! with `t`, queue downloads from addresses with `D` (the Downloads page shows the queue: `iyakki_agent::mediaqueue`). The
//! hybrid index behind the search lives in `iyakki_agent::mediaindex`.
//! MP3 plays inside IYAKKI (the in-house decoder of the radio player); other audio and video go to an external player
//! program (mpv, VLC, ffplay or mplayer) running as its own process, and on the `web` image a video plays in the embedded
//! web engine in this app's page area (served from a loopback-only server, see `iyakki_agent::mediaserve`). The play queue
//! lives with the shared player, so the assistant's media tools and this app see the same one. Music goes on when the app is
//! left. The built-in samples (`/usr/share/iyakki/samples`) are listed as the read-only folder "IYAKKI samples".

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use iyakki_agent::media::{self, Check, Kind, Library, MediaPlayer, MediaStore, Repeat, Track};
use iyakki_agent::mediadl;
use iyakki_agent::mediaindex::{self, Item, ItemKind, MediaIndex, Query, Sort};
use iyakki_agent::mediaqueue::{self, DownloadQueue, Status, View};
use iyakki_agent::mediaserve::LocalServe;

use super::browser_full::{Full, Note, Start};
use super::neon::{glow_card, level_bars, mix, round_button, with_alpha, Glyph, CYAN, MAGENTA};
use super::widgets::{render_confirm, Form, FormEvent, PickEvent, Picker};
use super::{blink, App, AppCx, Flow};
use crate::gfx::{rgb, Face, Gfx, Icon};
use crate::ui::{Event, Key, TextField, CARD, CARD_HOVER, CARD_SEL, DIM, ERR, MUTED, OK, R, TEXT};

const ACCENT: [u8; 4] = rgb(240, 120, 220);
const AMBER: [u8; 4] = rgb(255, 184, 64);
const NEW_LIST: &str = "+ New playlist";
/// Bars in the level display. Cosmetic: there is no audio analysis, the bars move while something plays.
pub const BARS: usize = 28;
const SEARCH_LIMIT: usize = 500;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Tracks,
    Artists,
    Albums,
    Videos,
    Playlists,
    Folders,
    Types,
    TopRated,
    Photos,
    /// The download queue. Added after the others: the indexes of the older pages stay as they were.
    Downloads,
}

const PAGES: [(Page, &str); 10] = [(Page::Tracks, "Tracks"), (Page::Artists, "Artists"), (Page::Albums, "Albums"), (Page::Videos, "Videos"), (Page::Playlists, "Playlists"), (Page::Folders, "Folders"), (Page::Types, "Types"), (Page::TopRated, "Top rated"), (Page::Photos, "Photos"), (Page::Downloads, "Downloads")];
/// The index of the Downloads page in [`PAGES`] (a click on the header chip goes there).
const DOWNLOADS_PAGE: usize = 9;

/// Examples shown under the category field of the download form; any other word may be typed.
pub const VIDEO_CHIPS: [&str; 10] = ["action", "horror", "cartoon", "comedy", "drama", "sci-fi", "documentary", "kids", "thriller", "romance"];
pub const SONG_CHIPS: [&str; 9] = ["melody", "kuthu", "beats", "devotional", "classical", "rock", "hip-hop", "lofi", "ghazal"];
pub const PHOTO_CHIPS: [&str; 3] = ["family", "travel", "nature"];

/// The rows of example chips for what is known of the file: only the kind's row once the kind is known, else every row.
pub fn chip_rows(kind: Option<ItemKind>) -> Vec<(String, Vec<String>)> {
    let row = |label: &str, w: &[&str]| (label.to_string(), w.iter().map(|x| x.to_string()).collect::<Vec<_>>());
    match kind {
        Some(ItemKind::Video) => vec![row("Videos", &VIDEO_CHIPS)],
        Some(ItemKind::Image) => vec![row("Photos", &PHOTO_CHIPS)],
        Some(ItemKind::Song | ItemKind::Podcast) => vec![row("Songs", &SONG_CHIPS)],
        None => vec![row("Videos", &VIDEO_CHIPS), row("Songs", &SONG_CHIPS), row("Photos", &PHOTO_CHIPS)],
    }
}

/// How many heartbeats the header chip flashes a finished download.
const FLASH_TICKS: u32 = 120;

#[derive(Clone, PartialEq, Eq, Debug)]
enum Drill {
    Artist(String),
    Album(String, String),
    Playlist(String),
    Type(String),
    Lang(String),
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Act {
    Track(String),
    Artist(String),
    Album(String, String),
    Playlist(String),
    /// An entry of the open playlist: its place and the file.
    Item(usize, String),
    Folder(String),
    Type(String),
    Lang(String),
    Photo(String),
    /// A download in the queue, by its number.
    Download(u64),
}

#[derive(Clone)]
struct Row {
    main: String,
    sub: String,
    right: String,
    act: Act,
    /// Types shown as chips, and the star rating (0 = unrated).
    types: Vec<String>,
    rating: u8,
    /// The queue entry behind a row of the Downloads page.
    dl: Option<View>,
}

impl Row {
    fn plain(main: String, sub: String, right: String, act: Act) -> Row {
        Row { main, sub, right, act, types: vec![], rating: 0, dl: None }
    }
}

enum FormFor {
    NewPlaylist,
    Rename(String),
    Folder,
}

enum Mode {
    Browse,
    Form(Form, FormFor),
    /// Choosing the playlist to add `pending` to.
    Pick(Picker),
    DeletePlaylist(String),
    RemoveFolder(String),
    /// Types and tags of the item with this id.
    Tag(Form, String),
    /// Add addresses to the download queue; the writable folders offered after "Automatic", in the order of the choice field.
    Download(Form, Vec<String>),
}

/// How a video can be played here.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VideoRoute {
    /// IYAKKI's own player: `ffmpeg` decodes, the picture is drawn in this app's page area (`mediaplay.rs`).
    Native,
    /// An installed player program (mpv, VLC, ffplay, mplayer) in its own window.
    External,
    /// The embedded full web engine of the `web` image (this helper program), in this app's page area.
    Embedded(std::path::PathBuf),
    /// Neither: the app says what is needed.
    Missing,
}

/// The message when a video cannot be played at all.
pub const VIDEO_NEEDS: &str = "Install the media player: IYAKKI menu \u{2192} Quick install \u{2192} Media";

/// A video playing in the embedded engine: the engine's view, and the loopback server that gives it the one file.
struct VideoView {
    full: Full,
    title: String,
    /// The shell has pressed `p` in the page once it loaded: a key press is the user activation the engine wants before it
    /// starts a video with sound (autoplay alone is refused, and the page then asks for Space).
    kicked: bool,
    // dropped after the view: the server stays up while the engine may still ask
    _serve: LocalServe,
}

pub struct MediaApp {
    page: Page,
    sel: [usize; 10],
    drill: Option<Drill>,
    drill_sel: usize,
    query: TextField,
    searching: bool,
    lib: Library,
    scanning: Option<Arc<Mutex<Option<Library>>>>,
    /// The shared host folders that were reachable at the last scan (a change, a folder unplugged or back, starts a new scan).
    host_seen: Vec<String>,
    host_ticks: u64,
    /// A shared host folder whose options (convert to a copy, stop sharing) were asked for.
    host_menu: Option<String>,
    store: MediaStore,
    player: Arc<MediaPlayer>,
    index: Arc<Mutex<MediaIndex>>,
    sort: Sort,
    /// Bumped whenever what the lists show may have changed (a scan, a rating, a tag).
    epoch: u64,
    cache: RefCell<Option<(String, Vec<Row>)>>,
    mode: Mode,
    /// Files waiting to be added to a playlist.
    pending: Vec<String>,
    queue: Vec<String>,
    qpos: usize,
    queue_active: bool,
    /// The shared queue revision this app has seen.
    seen_rev: u64,
    shuffle: bool,
    repeat: Repeat,
    status: Option<(String, bool)>,
    /// The person's download queue (shared with the assistant's tools).
    dlq: Arc<DownloadQueue>,
    /// What the header chip flashes after a download ended: the words, whether it went well, heartbeats left.
    flash: Option<(String, bool, u32)>,
    /// How many finished downloads this app has seen announced.
    seen_finished: u64,
    /// The built-in sample folder, when there is one.
    samples: Option<std::path::PathBuf>,
    /// A video playing in the embedded engine.
    video: Option<VideoView>,
    /// A video or sound playing in IYAKKI's own player.
    native: Option<super::mediaplay::NativeView>,
    /// For tests and unusual setups: how videos are played instead of looking for a player or the engine's helper.
    route_override: Option<VideoRoute>,
    /// A download finished: look at the folders again once the current scan is done.
    rescan: bool,
    /// Where the app was drawn last (the video fills it).
    area: R,
    ui_scale: f32,
}

/// The bar heights of the level display: moving while something plays, flat and low otherwise.
pub fn visualizer(tick: u64, playing: bool) -> [f32; BARS] {
    let mut v = [0.06f32; BARS];
    if playing {
        let t = tick as f32;
        for (i, b) in v.iter_mut().enumerate() {
            let a = (t * 0.13 + i as f32 * 0.62).sin();
            let c = (t * 0.071 + i as f32 * 1.37).cos();
            let env = 0.55 + 0.45 * (t * 0.03 + i as f32 * 0.2).sin();
            *b = (0.12 + 0.88 * (0.5 + 0.5 * (a * 0.6 + c * 0.4)) * env).clamp(0.05, 1.0);
        }
    }
    v
}

/// The colour a type is drawn in: kuthu hot magenta, melody cyan, beats amber, and a fixed colour for the others.
pub fn type_color(t: &str) -> [u8; 4] {
    match t {
        "kuthu" => rgb(255, 60, 170),
        "melody" => CYAN,
        "beats" => AMBER,
        "devotional" => rgb(255, 215, 120),
        "rock" | "metal" => rgb(255, 100, 100),
        "classical" | "carnatic" | "hindustani" => rgb(170, 140, 255),
        "hip-hop" | "edm" => rgb(110, 255, 150),
        "lofi" | "sad" => rgb(120, 200, 200),
        "romantic" | "ghazal" => rgb(255, 140, 190),
        _ => ACCENT,
    }
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

impl MediaApp {
    pub fn new(cx: &AppCx) -> MediaApp {
        MediaApp::with_player(cx, media::shared())
    }

    pub fn with_player(cx: &AppCx, player: Arc<MediaPlayer>) -> MediaApp {
        iyakki_agent::player::register_dir(cx.services.clis.bin_dir());
        let store = MediaStore::new(cx.services.dir(), &cx.session.user);
        let (shuffle, repeat) = store.flags();
        let mut a = MediaApp {
            page: Page::Tracks,
            sel: [0; 10],
            drill: None,
            drill_sel: 0,
            query: TextField::new("", false),
            searching: false,
            lib: Library::default(),
            scanning: None,
            host_seen: Vec::new(),
            host_ticks: 0,
            host_menu: None,
            store,
            player,
            index: cx.services.media_index(&cx.session.user),
            sort: Sort::Relevance,
            epoch: 0,
            cache: RefCell::new(None),
            mode: Mode::Browse,
            pending: vec![],
            queue: vec![],
            qpos: 0,
            queue_active: false,
            seen_rev: 0,
            shuffle,
            repeat,
            status: None,
            dlq: cx.services.media_queue(&cx.session.user),
            flash: None,
            seen_finished: 0,
            samples: media::samples_dir(),
            video: None,
            route_override: None,
            native: None,
            rescan: false,
            area: R::new(0.0, 0.0, 1280.0, 600.0),
            ui_scale: 1.0,
        };
        a.seen_finished = a.dlq.last_finished().0;
        a.dlq.set_context(a.dl_context(cx));
        a.adopt_queue(cx);
        a.start_scan(cx);
        a
    }

    pub fn page(&self) -> Page {
        self.page
    }

    pub fn is_scanning(&self) -> bool {
        self.scanning.is_some()
    }

    pub fn status_text(&self) -> Option<&str> {
        self.status.as_ref().map(|s| s.0.as_str())
    }

    pub fn library(&self) -> &Library {
        &self.lib
    }

    /// The names on the current page, as shown.
    pub fn row_names(&self) -> Vec<String> {
        self.rows().into_iter().map(|r| r.main).collect()
    }

    fn say(&mut self, t: impl Into<String>, ok: bool) {
        self.status = Some((t.into(), ok));
    }

    /// Paths of the shared host folders that can be reached now.
    fn host_available(&self) -> Vec<String> {
        self.store.host_mounts().into_iter().map(|m| m.path).collect()
    }

    /// The folders scanned: the person's own and the built-in samples.
    fn scan_list(&self) -> Vec<String> {
        let mut own = self.store.folders();
        // shared host folders are read where they are (read-only: nothing here writes to a folder)
        for m in self.store.host_mounts() {
            if !own.contains(&m.path) {
                own.push(m.path);
            }
        }
        media::scan_folders(&own, self.samples.as_deref())
    }

    /// May this account read (and so list and play) the file? The samples can always be read.
    fn may_read(&self, cx: &AppCx, path: &str) -> bool {
        cx.session.policy.allows_read(&cx.session.user, path) || self.samples.as_deref().is_some_and(|d| media::is_sample_in(d, path)) || self.store.host_mount_for(path).is_some()
    }

    /// Scan the folders in the background and bring the index up to date there too (the account's read scope decides what
    /// is looked at, plus the read-only sample folder).
    fn start_scan(&mut self, cx: &AppCx) {
        let folders = self.scan_list();
        let (policy, user) = (cx.session.policy.clone(), cx.session.user.clone());
        let samples = self.samples.clone();
        let hosts: Vec<String> = self.store.host_mounts().into_iter().map(|m| m.path).collect();
        self.host_seen = self.host_available();
        let slot: Arc<Mutex<Option<Library>>> = Arc::new(Mutex::new(None));
        let out = slot.clone();
        let index = self.index.clone();
        std::thread::spawn(move || {
            let allow = |p: &str| policy.allows_read(&user, p) || samples.as_deref().is_some_and(|d| media::is_sample_in(d, p)) || hosts.iter().any(|h| std::path::Path::new(p).starts_with(h));
            let (lib, images) = mediaindex::scan_for_index(&folders, &user, &allow);
            index.lock().unwrap_or_else(|e| e.into_inner()).sync(&lib, &images, iyakki_agent::clock::now_unix());
            if let Some(d) = &samples {
                index.lock().unwrap_or_else(|e| e.into_inner()).mark_samples(d);
            }
            mediaindex::embed_all(&index, 256, 400);
            let _ = index.lock().unwrap_or_else(|e| e.into_inner()).save();
            *out.lock().unwrap_or_else(|e| e.into_inner()) = Some(lib);
        });
        self.scanning = Some(slot);
    }

    fn poll_scan(&mut self) {
        let done = self.scanning.as_ref().and_then(|s| s.lock().unwrap_or_else(|e| e.into_inner()).take());
        if let Some(lib) = done {
            self.scanning = None;
            let (a, v) = (lib.tracks.iter().filter(|t| t.kind == Kind::Audio).count(), lib.tracks.iter().filter(|t| t.kind == Kind::Video).count());
            self.say(format!("Found {a} music file{} and {v} video{}{}.", if a == 1 { "" } else { "s" }, if v == 1 { "" } else { "s" }, if lib.truncated { " (the list is cut short)" } else { "" }), true);
            self.lib = lib;
            self.epoch += 1;
            self.clamp();
        }
    }

    fn q(&self) -> String {
        self.query.value.trim().to_string()
    }

    fn item(&self, path: &str) -> Option<Item> {
        self.index.lock().unwrap_or_else(|e| e.into_inner()).find_path(path).cloned()
    }

    fn track_row(&self, t: &Track, act: Act) -> Row {
        let sub = if t.kind == Kind::Video { t.path.clone() } else { format!("{} · {}", t.artist, t.album) };
        let it = self.item(&t.path);
        let (types, rating) = it.map(|i| (i.effective_types().iter().chain(i.tags.iter()).take(3).cloned().collect(), i.rating)).unwrap_or_default();
        Row { main: t.title.clone(), sub, right: t.duration_text(), act, types, rating, dl: None }
    }

    fn item_row(it: &Item) -> Row {
        let sub = match it.kind {
            ItemKind::Image => format!("{}{}", it.path.rsplit('/').next().unwrap_or(""), if it.year > 0 { format!(" · {}", it.year) } else { String::new() }),
            ItemKind::Video => it.path.clone(),
            _ => {
                let mut v = vec![];
                if !it.artist.is_empty() {
                    v.push(it.artist.clone());
                }
                if !it.album.is_empty() {
                    v.push(it.album.clone());
                }
                v.join(" · ")
            }
        };
        let act = if it.kind == ItemKind::Image { Act::Photo(it.path.clone()) } else { Act::Track(it.path.clone()) };
        Row { main: it.title.clone(), sub, right: it.duration_text(), act, types: it.effective_types().iter().chain(it.tags.iter()).take(3).cloned().collect(), rating: it.rating, dl: None }
    }

    /// Search the index with the typed phrase on top of what a page asks for.
    fn indexed(&self, kinds: &[ItemKind], base: impl FnOnce(&mut Query)) -> Vec<Row> {
        let mut q = Query::parse(&self.q());
        base(&mut q);
        if self.sort != Sort::Relevance {
            q.sort = self.sort;
        }
        q.limit = SEARCH_LIMIT;
        let g = self.index.lock().unwrap_or_else(|e| e.into_inner());
        g.search(&q).into_iter().map(|h| &g.items()[h.index]).filter(|it| q.kind.is_some() || kinds.contains(&it.kind)).map(Self::item_row).collect()
    }

    fn rows(&self) -> Vec<Row> {
        let key = format!("{:?}|{:?}|{}|{:?}|{}", self.page, self.drill, self.q(), self.sort, self.epoch);
        let cacheable = !matches!(self.drill, Some(Drill::Playlist(_) | Drill::Artist(_) | Drill::Album(..))) && !matches!(self.page, Page::Playlists | Page::Folders | Page::Downloads);
        if cacheable {
            if let Some((k, rows)) = self.cache.borrow().as_ref() {
                if *k == key {
                    return rows.clone();
                }
            }
        }
        let rows = self.build_rows();
        if cacheable {
            *self.cache.borrow_mut() = Some((key, rows.clone()));
        }
        rows
    }

    fn build_rows(&self) -> Vec<Row> {
        let q = self.q();
        let tr = |t: &&Track| self.track_row(t, Act::Track(t.path.clone()));
        if let Some(d) = &self.drill {
            return match d {
                Drill::Artist(a) => self.lib.by_artist(a).iter().filter(|t| q.is_empty() || t.title.to_lowercase().contains(&q.to_lowercase())).map(|t| tr(&t)).collect(),
                Drill::Album(a, b) => self.lib.by_album(a, b).iter().filter(|t| q.is_empty() || t.title.to_lowercase().contains(&q.to_lowercase())).map(|t| tr(&t)).collect(),
                Drill::Playlist(name) => self
                    .store
                    .playlist(name)
                    .map(|p| {
                        p.items
                            .iter()
                            .enumerate()
                            .map(|(i, path)| match self.lib.find(path) {
                                Some(t) => self.track_row(t, Act::Item(i, path.clone())),
                                None => Row::plain(path.rsplit('/').next().unwrap_or(path).to_string(), "not in the library".into(), String::new(), Act::Item(i, path.clone())),
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                Drill::Type(t) => self.indexed(&[ItemKind::Song, ItemKind::Video, ItemKind::Podcast], |qq| {
                    if !qq.types.contains(t) {
                        qq.types.push(t.clone());
                    }
                    if qq.sort == Sort::Relevance {
                        qq.sort = Sort::TopRated;
                    }
                }),
                Drill::Lang(l) => self.indexed(&[ItemKind::Song, ItemKind::Video, ItemKind::Podcast], |qq| {
                    if !qq.langs.contains(l) {
                        qq.langs.push(l.clone());
                    }
                    if qq.sort == Sort::Relevance {
                        qq.sort = Sort::TopRated;
                    }
                }),
            };
        }
        let indexed = !q.is_empty() || self.sort != Sort::Relevance;
        match self.page {
            Page::Tracks if indexed => self.indexed(&[ItemKind::Song, ItemKind::Podcast], |_| {}),
            Page::Videos if indexed => self.indexed(&[ItemKind::Video], |qq| {
                qq.kind.get_or_insert(ItemKind::Video);
            }),
            Page::Tracks => self.lib.audio("").iter().map(tr).collect(),
            Page::Videos => self.lib.videos("").iter().map(tr).collect(),
            Page::Photos => self.indexed(&[ItemKind::Image], |qq| {
                qq.kind.get_or_insert(ItemKind::Image);
            }),
            Page::TopRated => self.indexed(&[ItemKind::Song, ItemKind::Video, ItemKind::Podcast], |qq| {
                qq.min_rating = qq.min_rating.max(1);
                if qq.sort == Sort::Relevance {
                    qq.sort = Sort::TopRated;
                }
            }),
            Page::Types => {
                let g = self.index.lock().unwrap_or_else(|e| e.into_inner());
                let want = q.to_lowercase();
                let mut rows: Vec<Row> = g.type_counts().into_iter().filter(|(t, _)| want.is_empty() || t.contains(&want)).map(|(t, n)| Row { types: vec![t.clone()], ..Row::plain(t.clone(), "type".into(), plural(n, "item", "items"), Act::Type(t)) }).collect();
                rows.extend(g.language_counts().into_iter().filter(|(t, _)| want.is_empty() || t.contains(&want)).map(|(t, n)| Row::plain(t.clone(), "language".into(), plural(n, "item", "items"), Act::Lang(t))));
                rows
            }
            Page::Artists => self.lib.artists(&q).into_iter().map(|(a, n)| Row::plain(a.clone(), String::new(), plural(n, "track", "tracks"), Act::Artist(a))).collect(),
            Page::Albums => self.lib.albums(&q).into_iter().map(|(a, b, n)| Row::plain(b.clone(), a.clone(), plural(n, "track", "tracks"), Act::Album(a, b))).collect(),
            Page::Playlists => self
                .store
                .playlists()
                .into_iter()
                .filter(|p| q.is_empty() || p.name.to_lowercase().contains(&q.to_lowercase()))
                .map(|p| Row::plain(p.name.clone(), String::new(), plural(p.items.len(), "item", "items"), Act::Playlist(p.name)))
                .collect(),
            Page::Folders => {
                let mut rows: Vec<Row> = self.store.folders().into_iter().map(|f| Row::plain(f.clone(), String::new(), String::new(), Act::Folder(f))).collect();
                // folders shared from the host computer: read in place, read-only, with a plain state when they are away
                for (m, ok) in self.store.host_mount_states() {
                    let sub = if ok { format!("{} · shared from your computer, read-only", m.path) } else { format!("{} · folder not available (unplugged or moved); it is picked up again when it returns", m.path) };
                    rows.push(Row { types: vec!["host".to_string()], ..Row::plain(format!("host:{}", m.name), sub, if ok { "host · read-only".into() } else { "not available".into() }, Act::Folder(m.path.clone())) });
                }
                // the built-in samples: always there, read-only, not something to delete
                if let Some(d) = &self.samples {
                    rows.push(Row { types: vec![media::SAMPLES_TAG.to_string()], ..Row::plain(media::SAMPLES_LABEL.to_string(), format!("{} · built in, read-only", d.display()), "read-only".into(), Act::Folder(d.to_string_lossy().into_owned())) });
                }
                rows
            }
            Page::Downloads => self.download_rows(),
        }
    }

    fn page_idx(&self) -> usize {
        PAGES.iter().position(|(p, _)| *p == self.page).unwrap_or(0)
    }

    fn sel_now(&self) -> usize {
        if self.drill.is_some() { self.drill_sel } else { self.sel[self.page_idx()] }
    }

    fn set_sel(&mut self, i: usize) {
        let n = self.rows().len();
        let i = i.min(n.saturating_sub(1));
        if self.drill.is_some() {
            self.drill_sel = i;
        } else {
            let k = self.page_idx();
            self.sel[k] = i;
        }
    }

    fn clamp(&mut self) {
        self.set_sel(self.sel_now());
    }

    fn goto(&mut self, p: Page) {
        self.page = p;
        self.drill = None;
        self.status = None;
        self.clamp();
    }

    fn step_page(&mut self, d: i32) {
        let i = (self.page_idx() as i32 + d).rem_euclid(PAGES.len() as i32) as usize;
        self.goto(PAGES[i].0);
    }

    fn selected(&self) -> Option<Row> {
        self.rows().get(self.sel_now()).cloned()
    }

    /// The file of the chosen row when it is a track, playlist entry or photo.
    fn selected_path(&self) -> Option<String> {
        match self.selected()?.act {
            Act::Track(p) | Act::Item(_, p) | Act::Photo(p) => Some(p),
            _ => None,
        }
    }

    // ---------------------------------------------------------------- the shared queue

    /// Tell the shared player about the queue, so the assistant's tools and a later visit to this app see it.
    fn publish_queue(&mut self, cx: &AppCx) {
        self.seen_rev = self.player.set_queue(&cx.session.user, self.queue.clone(), self.qpos, self.queue_active);
    }

    /// Take over the queue somebody else (the assistant) started, when it is this person's.
    fn adopt_queue(&mut self, cx: &AppCx) {
        let st = self.player.queue_state();
        if st.rev == self.seen_rev {
            return;
        }
        self.seen_rev = st.rev;
        if st.user == cx.session.user {
            self.queue = st.items;
            self.qpos = st.pos;
            self.queue_active = st.active;
        }
    }

    // ---------------------------------------------------------------- playing

    fn label_of(&self, path: &str) -> String {
        match self.lib.find(path) {
            Some(t) if t.artist != media::UNKNOWN_ARTIST => format!("{} - {}", t.artist, t.title),
            Some(t) => t.title.clone(),
            None => path.rsplit('/').next().unwrap_or(path).to_string(),
        }
    }

    fn play_current(&mut self, cx: &AppCx) {
        let Some(path) = self.queue.get(self.qpos).cloned() else { return };
        if !self.may_read(cx, &path) {
            self.queue_active = false;
            self.say("Your account may not read that file.", false);
            self.publish_queue(cx);
            return;
        }
        if !std::path::Path::new(&path).is_file() {
            self.queue_active = false;
            self.say("That file is gone.", false);
            self.publish_queue(cx);
            return;
        }
        let label = self.label_of(&path);
        // a video with no player program goes to the embedded engine (the web image), or the person is told what is needed
        let non_mp3_audio = !media::is_mp3(&path) && !media::is_video_path(&path);
        if self.video_route() == VideoRoute::Native && (media::is_video_path(&path) || non_mp3_audio) {
            self.player.stop();
            self.queue_active = false;
            self.publish_queue(cx);
            self.start_native(&path, &label, cx);
            return;
        }
        if media::is_video_path(&path) {
            match self.video_route() {
                VideoRoute::External | VideoRoute::Native => {}
                VideoRoute::Embedded(helper) => {
                    self.player.stop();
                    self.queue_active = false;
                    self.publish_queue(cx);
                    self.start_video(helper, &path, &label, cx);
                    return;
                }
                VideoRoute::Missing => {
                    self.queue_active = false;
                    self.say(format!("{VIDEO_NEEDS}."), false);
                    self.publish_queue(cx);
                    return;
                }
            }
        }
        match self.player.play(&cx.session.user, &label, &path) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "media.play", path.rsplit('/').next().unwrap_or(""));
                self.status = None;
                let mut g = self.index.lock().unwrap_or_else(|e| e.into_inner());
                if g.record_play(&mediaindex::item_id(&path), iyakki_agent::clock::now_unix()).is_ok() {
                    let _ = g.save();
                }
                drop(g);
                self.epoch += 1;
            }
            Err(e) => {
                self.queue_active = false;
                self.say(e, false);
            }
        }
        self.publish_queue(cx);
    }

    fn start_queue(&mut self, mut paths: Vec<String>, mut pos: usize, cx: &AppCx) {
        if paths.is_empty() {
            return;
        }
        pos = pos.min(paths.len() - 1);
        if self.shuffle {
            let first = paths.remove(pos);
            let mut rest = media::shuffled(&paths, media::clock_seed());
            rest.insert(0, first);
            paths = rest;
            pos = 0;
        }
        self.queue = paths;
        self.qpos = pos;
        self.queue_active = true;
        self.play_current(cx);
    }

    fn stop(&mut self, cx: &AppCx) {
        self.queue_active = false;
        self.player.stop();
        self.publish_queue(cx);
    }

    fn step_queue(&mut self, d: i32, cx: &AppCx) {
        if !self.queue_active || self.queue.is_empty() {
            self.say("Nothing is queued: press Enter on a track first.", false);
            return;
        }
        self.qpos = (self.qpos as i32 + d).rem_euclid(self.queue.len() as i32) as usize;
        self.play_current(cx);
    }

    /// The player ended by itself: carry on with the queue.
    fn advance(&mut self, cx: &AppCx) {
        if !self.queue_active {
            return;
        }
        match self.repeat {
            Repeat::One => {}
            _ if self.qpos + 1 < self.queue.len() => self.qpos += 1,
            Repeat::All => {
                self.qpos = 0;
                if self.shuffle {
                    self.queue = media::shuffled(&self.queue, media::clock_seed());
                }
            }
            Repeat::Off => {
                self.queue_active = false;
                self.publish_queue(cx);
                return;
            }
        }
        self.play_current(cx);
    }

    fn activate(&mut self, cx: &AppCx) {
        let Some(row) = self.selected() else { return };
        match row.act {
            Act::Track(_) | Act::Item(..) => {
                let paths: Vec<String> = self
                    .rows()
                    .into_iter()
                    .filter_map(|r| match r.act {
                        Act::Track(p) | Act::Item(_, p) => Some(p),
                        _ => None,
                    })
                    .collect();
                self.start_queue(paths, self.sel_now(), cx);
            }
            Act::Artist(a) => {
                self.drill = Some(Drill::Artist(a));
                self.drill_sel = 0;
            }
            Act::Album(a, b) => {
                self.drill = Some(Drill::Album(a, b));
                self.drill_sel = 0;
            }
            Act::Playlist(n) => {
                self.drill = Some(Drill::Playlist(n));
                self.drill_sel = 0;
            }
            Act::Type(t) => {
                self.drill = Some(Drill::Type(t));
                self.drill_sel = 0;
            }
            Act::Lang(l) => {
                self.drill = Some(Drill::Lang(l));
                self.drill_sel = 0;
            }
            Act::Photo(p) => {
                let info = self.item(&p).map(|i| format!("{} · {} KB{}", i.title, i.size / 1024, if i.year > 0 { format!(" · {}", i.year) } else { String::new() })).unwrap_or_default();
                self.say(format!("Photo: {p}  {info}"), true);
            }
            Act::Folder(f) if self.samples.as_deref().is_some_and(|d| d.to_string_lossy() == f.as_str()) => self.say("The IYAKKI samples are built in and read-only: they cannot be changed or removed.", true),
            Act::Folder(f) if self.store.host_mount_states().iter().any(|(m, _)| m.path == f) => self.say("This folder is shared from your computer, read-only: nothing here changes it. Stop sharing, or turn it into a copy, under Host files in the DATA tile.", true),
            Act::Folder(_) => self.say("Press R to look at the folders again, n to add one, d to remove the chosen one.", true),
            Act::Download(_) => self.activate_download(cx),
        }
    }

    /// Hand a whole playlist (an M3U file under the person's data folder) to the player in one go.
    fn play_playlist_file(&mut self, name: &str, cx: &AppCx) {
        let Some(p) = self.store.playlist(name) else { return };
        let mut items: Vec<String> = p.items.into_iter().filter(|i| self.may_read(cx, i) && std::path::Path::new(i).is_file()).collect();
        if items.is_empty() {
            self.say("Nothing in that playlist can be played.", false);
            return;
        }
        if self.shuffle {
            items = media::shuffled(&items, media::clock_seed());
        }
        let file = match self.store.write_m3u(name, &items, &self.lib) {
            Ok(f) => f,
            Err(e) => {
                self.say(format!("Could not write the playlist file: {e}"), false);
                return;
            }
        };
        self.queue_active = false;
        match self.player.play(&cx.session.user, &format!("Playlist {name}"), &file.to_string_lossy()) {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "media.play", &format!("playlist {name}"));
                self.say(format!("Playing {} item{} of {name} in the player.", items.len(), if items.len() == 1 { "" } else { "s" }), true);
            }
            Err(e) => self.say(e, false),
        }
        self.publish_queue(cx);
    }

    // ---------------------------------------------------------------- ratings, tags, downloads

    fn rate_selected(&mut self, stars: u8, cx: &AppCx) {
        let Some(path) = self.selected_path() else { return };
        let id = mediaindex::item_id(&path);
        let r = {
            let mut g = self.index.lock().unwrap_or_else(|e| e.into_inner());
            let r = g.rate(&id, stars);
            if r.is_ok() {
                let _ = g.save();
            }
            r
        };
        match r {
            Ok(()) => {
                cx.audit.record(&cx.session.user, "media.rate", &format!("{} {stars}", path.rsplit('/').next().unwrap_or("")));
                self.epoch += 1;
                self.say(if stars == 0 { "Rating cleared.".to_string() } else { format!("Rated {}", mediaindex::stars_text(stars)) }, true);
            }
            Err(e) => self.say(e, false),
        }
    }

    fn open_tags(&mut self) {
        let Some(path) = self.selected_path() else {
            self.say("Choose a track, video or photo to tag.", false);
            return;
        };
        let Some(it) = self.item(&path) else {
            self.say("That file is not in the index yet.", false);
            return;
        };
        let types = it.effective_types().iter().cloned().collect::<Vec<_>>().join(", ");
        let tags = it.tags.iter().cloned().collect::<Vec<_>>().join(", ");
        let f = Form::new(&format!("Types and tags: {}", it.title)).text("Types", &types, "known types, e.g. kuthu, melody, beats (empty = automatic)").text("Tags", &tags, "your own words, comma separated");
        self.mode = Mode::Tag(f, it.id);
    }

    fn submit_tags(&mut self, cx: &AppCx) {
        let Mode::Tag(form, id) = &mut self.mode else { return };
        let split = |s: String| s.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty()).collect::<Vec<_>>();
        let (types, tags) = (split(form.value(0)), split(form.value(1)));
        let r = {
            let mut g = self.index.lock().unwrap_or_else(|e| e.into_inner());
            let old: Vec<String> = g.get(id).map(|i| i.tags.iter().cloned().collect()).unwrap_or_default();
            g.set_types(id, &types).and_then(|_| {
                g.untag(id, &old)?;
                g.tag(id, &tags)?;
                g.save()
            })
        };
        match r {
            Err(e) => form.error = Some(e),
            Ok(()) => {
                cx.audit.record(&cx.session.user, "media.tag", id);
                self.mode = Mode::Browse;
                self.epoch += 1;
                self.say("Types and tags saved.", true);
            }
        }
    }

    /// The library folders this account may write to (for downloads), with Pictures. The built-in samples are never among them.
    fn download_folders(&self, cx: &AppCx) -> Vec<String> {
        let all = mediaindex::picture_folders(&self.store.folders(), &cx.session.user);
        all.into_iter().filter(|f| cx.session.policy.allows_write(&cx.session.user, &format!("{f}/x"))).collect()
    }

    fn dl_context(&self, cx: &AppCx) -> mediaqueue::Context {
        mediaqueue::Context { svc: cx.services.clone(), policy: cx.session.policy.clone(), user: cx.session.user.clone(), folders: self.download_folders(cx) }
    }

    /// What the first address in the form's address field says about the kind of file.
    fn form_kind(form: &Form) -> Option<ItemKind> {
        form.value(0).split_whitespace().next().and_then(mediadl::kind_of_url)
    }

    /// Keep what the download form shows in step with what was typed: the example chips follow the kind of file as soon as the
    /// address names it, and the folder line says where the file will go.
    fn refresh_download_form(&self, form: &mut Form, folders: &[String], cx: &AppCx) {
        let kind = Self::form_kind(form);
        form.set_chips(1, chip_rows(kind));
        let label = match form.choice_idx(4) {
            0 => {
                let name = mediadl::planned_folder(folders, &cx.session.policy, &cx.session.user, kind).map(|p| mediaqueue::folder_label(&p.to_string_lossy()));
                match (kind, name) {
                    (Some(_), Some(n)) => format!("Folder: {n} (auto)"),
                    (None, _) => "Folder: by file type, decided when it arrives (auto)".to_string(),
                    (Some(_), None) => "Folder: none of your folders can be written (auto)".to_string(),
                }
            }
            i => format!("Folder: {}", folders.get(i - 1).map_or("?".to_string(), |f| mediaqueue::folder_label(f))),
        };
        form.set_label(4, &format!("{label}    (Tab here, then ← → to choose another)"));
    }

    fn open_download(&mut self, cx: &AppCx) {
        let folders = self.download_folders(cx);
        if folders.is_empty() {
            self.say("None of your library folders may be written to, so nothing can be downloaded.", false);
            return;
        }
        let mut names: Vec<String> = vec!["Automatic (by file type)".to_string()];
        names.extend(folders.iter().cloned());
        let opts: Vec<&str> = names.iter().map(String::as_str).collect();
        let mut f = Form::new("Add to the download queue")
            .text("Address(es)", "", "https://…/film.webm  (direct file links; several: separate with spaces; web pages are refused)")
            .text("Type / category", "", "any words, comma separated: action, horror, kuthu, melody…")
            .text("Tags", "", "your own words, comma separated")
            .text("Rating (0-5)", "0", "0 = unrated")
            .choice("Folder", &opts, 0);
        self.refresh_download_form(&mut f, &folders, cx);
        self.mode = Mode::Download(f, folders);
    }

    fn submit_download(&mut self, cx: &AppCx) {
        let Mode::Download(form, folders) = &mut self.mode else { return };
        let urls: Vec<String> = form.value(0).split_whitespace().map(str::to_string).collect();
        if urls.is_empty() {
            form.error = Some("type or paste an address first".into());
            return;
        }
        let rating = match form.value(3).parse::<u8>() {
            Ok(r) if r <= 5 => r,
            _ if form.value(3).is_empty() => 0,
            _ => {
                form.error = Some("the rating is 0 to 5".into());
                return;
            }
        };
        // the category is free text: nothing the person types here is refused
        let categories = mediadl::split_words(&form.value(1));
        let tags = mediadl::split_words(&form.value(2));
        let folder = match form.choice_idx(4) {
            0 => None,
            i => folders.get(i - 1).cloned(),
        };
        let ctx = mediaqueue::Context { svc: cx.services.clone(), policy: cx.session.policy.clone(), user: cx.session.user.clone(), folders: folders.clone() };
        let (mut queued, mut errors) = (vec![], vec![]);
        for url in urls {
            match self.dlq.enqueue(ctx.clone(), mediaqueue::Request { url: url.clone(), categories: categories.clone(), tags: tags.clone(), rating, folder: folder.clone() }) {
                Ok(id) => {
                    cx.audit.record(&cx.session.user, "media.download", &url.chars().take(200).collect::<String>());
                    queued.push(id);
                }
                Err(e) => errors.push(format!("{}: {e}", url.chars().take(60).collect::<String>())),
            }
        }
        if queued.is_empty() {
            if let Mode::Download(form, _) = &mut self.mode {
                form.error = errors.into_iter().next();
            }
            return;
        }
        self.mode = Mode::Browse;
        let (run, wait) = self.dlq.counts();
        let mut msg = format!("Added {} to the download queue ({run} running, {wait} waiting). Press Tab to the Downloads page to watch.", plural(queued.len(), "address", "addresses"));
        if !errors.is_empty() {
            msg = format!("{msg} Not added: {}", errors.join("; "));
        }
        self.goto(Page::Downloads);
        self.say(msg, errors.is_empty());
    }

    /// Look at what the queue did since the last heartbeat: flash the header chip, bring the library up to date.
    fn poll_downloads(&mut self) {
        let (seq, last) = self.dlq.last_finished();
        if seq != self.seen_finished {
            self.seen_finished = seq;
            // something ended (maybe several between two heartbeats): the folders are looked at again once
            self.rescan = true;
            if let Some((id, st)) = last {
                let v = self.dlq.view(id);
                let name = v.as_ref().map(|v| if v.name.is_empty() { mediaqueue::folder_label(&v.url) } else { v.name.clone() }).unwrap_or_default();
                match st {
                    Status::Succeeded => self.flash = Some((format!("Download succeeded: {name}"), true, FLASH_TICKS)),
                    Status::Failed => self.flash = Some((format!("Download failed: {name}"), false, FLASH_TICKS)),
                    _ => {}
                }
                self.epoch += 1;
            }
        }
        if let Some((_, _, t)) = &mut self.flash {
            *t = t.saturating_sub(1);
            if *t == 0 {
                self.flash = None;
            }
        }
    }

    /// The header chip: what the queue is doing now, or what just ended.
    pub fn downloads_chip(&self) -> Option<(String, bool)> {
        if let Some((t, ok, _)) = &self.flash {
            return Some((t.clone(), *ok));
        }
        let (run, wait) = self.dlq.counts();
        (run + wait > 0).then(|| (format!("Downloads: {run} running, {wait} queued"), true))
    }

    /// The rows of the Downloads page: running first, then waiting (in line order), then finished (newest first).
    fn download_rows(&self) -> Vec<Row> {
        let mut v = self.dlq.views();
        let rank = |s: Status| match s {
            Status::Downloading => 0,
            Status::Queued => 1,
            _ => 2,
        };
        v.sort_by(|a, b| rank(a.status).cmp(&rank(b.status)).then(if rank(a.status) == 2 { b.id.cmp(&a.id) } else { a.id.cmp(&b.id) }));
        v.into_iter()
            .map(|d| {
                let main = if d.status == Status::Succeeded && !d.name.is_empty() { d.name.clone() } else { url_label(&d.url) };
                Row { main, sub: status_line(&d), right: format!("#{}", d.id), act: Act::Download(d.id), types: d.categories.iter().take(3).cloned().collect(), rating: 0, dl: Some(d) }
            })
            .collect()
    }

    fn selected_download(&self) -> Option<View> {
        match self.selected()?.act {
            Act::Download(id) => self.dlq.view(id),
            _ => None,
        }
    }

    /// Enter on a download: play a finished one, say why a failed one failed.
    fn activate_download(&mut self, cx: &AppCx) {
        let Some(d) = self.selected_download() else { return };
        match d.status {
            Status::Succeeded if !d.file.is_empty() => {
                if std::path::Path::new(&d.file).is_file() {
                    self.start_queue(vec![d.file.clone()], 0, cx);
                } else {
                    self.say("That file is gone from the folder.", false);
                }
            }
            Status::Succeeded => self.say(format!("#{}: {}", d.id, d.note), true),
            Status::Failed | Status::Cancelled => self.say(format!("#{} {} — press r to retry, d to remove it from the list.", d.id, d.error), false),
            Status::Queued => self.say(format!("#{} is waiting for a free place (two downloads run at a time). x cancels it.", d.id), true),
            Status::Downloading => self.say(format!("#{} is downloading. x cancels it.", d.id), true),
        }
    }

    fn download_key(&mut self, c: char, cx: &AppCx) {
        let Some(d) = self.selected_download() else {
            if c == 'c' {
                let n = self.dlq.clear_finished();
                self.say(if n == 0 { "Nothing finished to clear.".to_string() } else { format!("Cleared {}.", plural(n, "finished entry", "finished entries")) }, true);
            }
            return;
        };
        let r = match c {
            'x' => self.dlq.cancel(d.id).map(|_| format!("Cancelling #{}…", d.id)),
            'r' => {
                self.dlq.set_context(self.dl_context(cx));
                self.dlq.retry(d.id).map(|_| format!("#{} is back in the queue.", d.id))
            }
            'd' => self.dlq.remove(d.id).map(|_| format!("Removed #{} from the list (the file, if any, stays).", d.id)),
            'c' => {
                let n = self.dlq.clear_finished();
                Ok(format!("Cleared {}.", plural(n, "finished entry", "finished entries")))
            }
            'f' => self.show_in_library(&d),
            _ => return,
        };
        match r {
            Ok(m) => self.say(m, true),
            Err(e) => self.say(e, false),
        }
        self.clamp();
    }

    /// Go to the page that lists a finished download and search for it there.
    fn show_in_library(&mut self, d: &View) -> Result<String, String> {
        if d.status != Status::Succeeded || d.file.is_empty() {
            return Err("only a finished download is in the library".into());
        }
        let page = match mediadl::kind_of_url(&d.file) {
            Some(ItemKind::Video) => Page::Videos,
            Some(ItemKind::Image) => Page::Photos,
            _ => Page::Tracks,
        };
        let title = self.item(&d.file).map(|i| i.title).unwrap_or_else(|| d.name.rsplit_once('.').map_or(d.name.clone(), |x| x.0.to_string()));
        self.goto(page);
        self.query.set(&title);
        self.epoch += 1;
        self.set_sel(0);
        Ok(format!("{} in the library: the search shows it (Esc clears the search).", d.name))
    }

    // ---------------------------------------------------------------- video in the embedded engine

    /// How a video is played here: an installed player, else the embedded engine's helper, else not at all.
    pub fn video_route(&self) -> VideoRoute {
        if let Some(r) = &self.route_override {
            return r.clone();
        }
        if iyakki_agent::player::available() {
            return VideoRoute::Native;
        }
        if iyakki_agent::voice::find_media_player("").is_some() {
            return VideoRoute::External;
        }
        match iyakki_agent::webengine::embedded_helper() {
            Some(h) => VideoRoute::Embedded(h),
            None => VideoRoute::Missing,
        }
    }

    /// Choose how videos are played (tests; a custom setup).
    pub fn set_video_route(&mut self, r: Option<VideoRoute>) {
        self.route_override = r;
    }

    /// Use another sample folder (tests) or none.
    pub fn set_samples_dir(&mut self, d: Option<std::path::PathBuf>, cx: &AppCx) {
        self.samples = d;
        self.start_scan(cx);
    }

    pub fn video_playing(&self) -> bool {
        self.video.is_some() || self.native.is_some()
    }

    /// Play a file in IYAKKI's own player (the view takes the page area).
    fn start_native(&mut self, path: &str, label: &str, cx: &AppCx) {
        self.native = None;
        let area = if self.area.w > 64.0 && self.area.h > 64.0 { self.area } else { R::new(0.0, 0.0, 800.0, 520.0) };
        match super::mediaplay::NativeView::start(path, label, area) {
            Ok(v) => {
                cx.audit.record(&cx.session.user, "media.video", path.rsplit('/').next().unwrap_or(""));
                let mut g = self.index.lock().unwrap_or_else(|e| e.into_inner());
                if g.record_play(&mediaindex::item_id(path), iyakki_agent::clock::now_unix()).is_ok() {
                    let _ = g.save();
                }
                drop(g);
                self.native = Some(v);
                self.status = None;
            }
            Err(e) => self.say(format!("Could not play it: {e}"), false),
        }
    }

    fn start_video(&mut self, helper: std::path::PathBuf, path: &str, label: &str, cx: &AppCx) {
        self.close_video();
        let serve = match LocalServe::start(std::path::Path::new(path), label) {
            Ok(s) => s,
            Err(e) => {
                self.say(format!("Could not start the video: {e}"), false);
                return;
            }
        };
        let profile = match iyakki_agent::webengine::prepare_profile(cx.services.dir(), &cx.session.user) {
            Ok(p) => p,
            Err(e) => {
                self.say(format!("The web engine's profile folder could not be made: {e}"), false);
                return;
            }
        };
        let bar = 40.0 * self.ui_scale;
        let start = Start {
            env: iyakki_agent::webengine::helper_env(&profile),
            helper,
            profile,
            size: (self.area.w.max(64.0) as u32, (self.area.h - bar).max(64.0) as u32),
            scale: self.ui_scale,
            zoom: 1.0,
            // the one address the page may have: this machine, for this playback
            allow_hosts: vec!["127.0.0.1".into()],
            user_agent: iyakki_agent::webengine::user_agent_suffix(),
            locale: String::new(),
        };
        match Full::start(start) {
            Ok(mut full) => {
                full.navigate(&serve.url);
                cx.audit.record(&cx.session.user, "media.video", path.rsplit('/').next().unwrap_or(""));
                let mut g = self.index.lock().unwrap_or_else(|e| e.into_inner());
                if g.record_play(&mediaindex::item_id(path), iyakki_agent::clock::now_unix()).is_ok() {
                    let _ = g.save();
                }
                drop(g);
                self.video = Some(VideoView { full, title: label.to_string(), kicked: false, _serve: serve });
                self.status = None;
            }
            Err(e) => self.say(format!("The video engine could not be started: {e}"), false),
        }
    }

    fn close_video(&mut self) {
        self.video = None;
    }

    /// Heartbeat and keys while a video plays.
    fn video_event(&mut self, ev: &Event, _cx: &AppCx) {
        let Some(v) = &mut self.video else { return };
        match ev {
            Event::Tick => {
                v.full.pump(&|h: &str| h == "127.0.0.1");
                if !v.kicked && v.full.is_ready() && !v.full.loading && v.full.url.starts_with("http://127.0.0.1:") {
                    v.kicked = true;
                    v.full.key(&Key::Char('p'));
                }
                let mut gone = None;
                for n in v.full.take_notes() {
                    match n {
                        Note::Gone(why) => gone = Some(why),
                        Note::Trouble(t) => self.status = Some((format!("Video engine: {t}"), false)),
                        _ => {}
                    }
                }
                if let Some(why) = gone {
                    self.video = None;
                    self.say(format!("The video engine stopped: {why}"), false);
                }
            }
            Event::Key(Key::Esc) | Event::Tab(200) => self.close_video(),
            Event::Key(Key::Char('s')) => self.close_video(),
            Event::Tab(201) => v.full.key(&Key::Char(' ')),
            Event::Key(k) => v.full.key(k),
            _ => {}
        }
    }

    // ---------------------------------------------------------------- playlists and folders

    fn paths_of(&self, row: &Row) -> Vec<String> {
        match &row.act {
            Act::Track(p) | Act::Item(_, p) => vec![p.clone()],
            Act::Artist(a) => self.lib.by_artist(a).iter().map(|t| t.path.clone()).collect(),
            Act::Album(a, b) => self.lib.by_album(a, b).iter().map(|t| t.path.clone()).collect(),
            _ => vec![],
        }
    }

    fn ask_add(&mut self) {
        let Some(row) = self.selected() else { return };
        let paths = self.paths_of(&row);
        if paths.is_empty() {
            self.say("Choose a track, an artist or an album to add.", false);
            return;
        }
        self.pending = paths;
        let mut items: Vec<String> = self.store.playlists().into_iter().map(|p| p.name).collect();
        items.push(NEW_LIST.into());
        self.mode = Mode::Pick(Picker::new("Add to playlist", items));
    }

    fn add_pending(&mut self, name: &str, cx: &AppCx) {
        let mut n = 0;
        for p in std::mem::take(&mut self.pending) {
            match self.store.add_item(name, &p) {
                Ok(_) => n += 1,
                Err(e) => {
                    self.say(e, false);
                    return;
                }
            }
        }
        cx.audit.record(&cx.session.user, "media.playlist.add", name);
        self.say(format!("Added {n} item{} to {name}.", if n == 1 { "" } else { "s" }), true);
    }

    fn submit_form(&mut self, cx: &AppCx) {
        let Mode::Form(form, what) = &mut self.mode else { return };
        let value = form.value(0);
        let result: Result<String, String> = match what {
            FormFor::NewPlaylist => self.store.create(&value).map(|n| {
                cx.audit.record(&cx.session.user, "media.playlist.create", &n);
                n
            }),
            FormFor::Rename(old) => self.store.rename(old, &value).inspect(|n| cx.audit.record(&cx.session.user, "media.playlist.rename", n)),
            FormFor::Folder => self.store.add_folder(&value).map(|_| value.trim().to_string()),
        };
        match result {
            Err(e) => {
                if let Mode::Form(f, _) = &mut self.mode {
                    f.error = Some(e);
                }
            }
            Ok(name) => {
                let Mode::Form(_, what) = std::mem::replace(&mut self.mode, Mode::Browse) else { return };
                match what {
                    FormFor::NewPlaylist => {
                        if self.pending.is_empty() {
                            self.say(format!("Created {name}."), true);
                        } else {
                            self.add_pending(&name, cx);
                        }
                    }
                    FormFor::Rename(_) => self.say(format!("Renamed to {name}."), true),
                    FormFor::Folder => {
                        let ok = cx.session.policy.allows_read(&cx.session.user, &name);
                        cx.audit.record(&cx.session.user, "media.folder.add", &name);
                        if ok {
                            self.start_scan(cx);
                            self.say(format!("Added {name}; looking at it now."), true);
                        } else {
                            self.say(format!("Added {name}, but your account may not read it, so it stays empty."), false);
                        }
                    }
                }
            }
        }
    }

    fn confirmed(&mut self, cx: &AppCx) {
        match std::mem::replace(&mut self.mode, Mode::Browse) {
            Mode::DeletePlaylist(n) => match self.store.delete(&n) {
                Ok(()) => {
                    cx.audit.record(&cx.session.user, "media.playlist.delete", &n);
                    self.say(format!("Deleted {n}."), true);
                    self.clamp();
                }
                Err(e) => self.say(e, false),
            },
            Mode::RemoveFolder(f) => match self.store.remove_folder(&f) {
                Ok(()) => {
                    cx.audit.record(&cx.session.user, "media.folder.remove", &f);
                    self.start_scan(cx);
                    self.say(format!("Removed {f} from the list; your files are untouched."), true);
                    self.clamp();
                }
                Err(e) => self.say(e, false),
            },
            other => self.mode = other,
        }
    }

    fn open_list(&self) -> Option<String> {
        match &self.drill {
            Some(Drill::Playlist(n)) => Some(n.clone()),
            _ => None,
        }
    }

    fn move_item(&mut self, d: i32) {
        let (Some(name), Some(Row { act: Act::Item(i, _), .. })) = (self.open_list(), self.selected()) else { return };
        let to = i as i32 + d;
        if to < 0 {
            return;
        }
        if self.store.move_item(&name, i, to as usize).is_ok() {
            self.drill_sel = to as usize;
        }
    }

    fn flags_changed(&mut self) {
        let _ = self.store.set_flags(self.shuffle, self.repeat);
    }

    fn pick_event(&mut self, ev: &Event, cx: &AppCx) {
        let Mode::Pick(p) = &mut self.mode else { return };
        match p.handle_event(ev) {
            PickEvent::Cancel => {
                self.mode = Mode::Browse;
                self.pending.clear();
            }
            PickEvent::Chosen(name) => {
                self.mode = Mode::Browse;
                if name == NEW_LIST {
                    self.mode = Mode::Form(Form::new("New playlist").text("Name", "", "e.g. Evening"), FormFor::NewPlaylist);
                } else {
                    self.add_pending(&name, cx);
                }
            }
            PickEvent::None => {}
        }
    }

    /// A key or click in the download form: the form follows what is typed (chips, the folder line).
    fn download_event(&mut self, ev: &Event, cx: &AppCx) {
        let Mode::Download(mut form, folders) = std::mem::replace(&mut self.mode, Mode::Browse) else { return };
        match form.handle_event(ev) {
            FormEvent::Cancel => return,
            FormEvent::Submit => {
                self.mode = Mode::Download(form, folders);
                self.submit_download(cx);
                return;
            }
            _ => {}
        }
        self.refresh_download_form(&mut form, &folders, cx);
        self.mode = Mode::Download(form, folders);
    }

    fn cycle_sort(&mut self) {
        self.sort = self.sort.next();
        self.epoch += 1;
        self.set_sel(0);
        self.say(format!("Sorted by {}.", self.sort.label()), true);
    }

    /// What the typed phrase was understood as, for the chips under the search box.
    fn chips(&self) -> Vec<String> {
        let q = self.q();
        if q.is_empty() {
            return vec![];
        }
        Query::parse(&q).chips()
    }
}

impl App for MediaApp {
    /// Files dropped on the window (window mode) go into the first music folder the account may write to (a dropped folder
    /// can be linked read-only instead), and the library looks again.
    fn on_drop(&mut self, _paths: &[std::path::PathBuf], cx: &AppCx) -> super::DropAction {
        match self.store.folders().into_iter().find(|f| std::path::Path::new(f).is_dir() && cx.session.policy.allows_write(&cx.session.user, f)) {
            Some(f) => super::DropAction::Into { dest: std::path::PathBuf::from(f), label: "your music folder".into() },
            None => super::DropAction::Uploads,
        }
    }
    fn dropped_done(&mut self, _files: &[std::path::PathBuf], _cx: &AppCx) {
        self.rescan = true;
    }
    fn take_host_mount_menu(&mut self) -> Option<String> {
        self.host_menu.take()
    }
    fn title(&self) -> &'static str {
        "Media"
    }
    fn icon(&self) -> Icon {
        Icon::Media
    }
    fn accent(&self) -> [u8; 4] {
        ACCENT
    }
    fn hint(&self) -> String {
        if let Some(n) = &self.native {
            return n.hint().into();
        }
        if self.video.is_some() {
            return "Space pause or play · ← → seek 5 s · m mute · Esc stop the video".into();
        }
        if self.page == Page::Downloads {
            return "Enter play · x cancel · r retry · d remove · c clear finished · f show in library · D add addresses · Tab pages · Esc back".into();
        }
        "Tab pages · ↑↓ choose · Enter play or open · / search (\"kutthu song\") · 1-5 rate · t tags · D download · 0 downloads · o sort · a playlist · s stop · Esc back".into()
    }
    fn help(&self) -> Vec<String> {
        [
            "Tab / Shift+Tab · Tracks, Artists, Albums, Videos, Playlists, Folders, Types, Top rated, Photos, Downloads (the number keys 1-9 jump to a page where no track is chosen, 0 goes to Downloads)",
            "Up / Down / PageUp / PageDown / Home / End · choose; Enter plays a track (the list after it is queued) or opens an artist, album, type or playlist; Esc steps back out",
            "/ · search in plain words: \"kutthu song\", \"melody songs by anirudh above 4 stars\", \"latest tamil beats\", \"videos\", \"photos from 2024\" (Enter keeps the filter, Esc clears it)",
            "1-5 · rate the chosen track (stars), 0 clears the rating     t · edit its types and tags (kuthu, melody, beats, …)     o · sort: best match, latest, top rated, most played",
            "D · add direct links to the download queue (a song, video or picture; several addresses separated by spaces). The type is free text (action, horror, kuthu, melody, anything): click an example chip, or Ctrl+←/→ then Ctrl+E. The folder follows the file type (songs Music, videos Videos, pictures Pictures); Tab to the Folder line and use ← → to choose another",
            "Types · kuthu, melody, beats and the others (and your own words) with how many items each has; Enter lists them     Top rated · your best rated items",
            "Downloads · the queue (two run at a time): Enter plays a finished one or tells why one failed, x cancels, r retries, d removes from the list, c clears the finished ones, f shows it in the library; the chip in the header shows what is running",
            "Video · plays in the installed player, or on the web image inside this page (Space pause, ← → seek, Esc stop)",
            "a · add the chosen track, artist or album to a playlist (or to a new one)",
            "Playlists · n new, r rename, d delete (asks first), Enter opens one, P plays the whole list in the player",
            "Inside a playlist · x remove the entry, [ moves it up, ] moves it down",
            "Folders · n add a folder, d remove it from the list (files are never deleted), R look again",
            "Space or s · stop (Space plays the chosen track when nothing plays)    , and . · previous and next in the queue",
            "z · shuffle on or off     l · repeat off, all or one (kept for next time)",
            "Esc · back; the player carries on in the background. You can also ask the chat: \"play a kutthu song\"",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }

    fn open_item(&mut self, item: &str, cx: &AppCx) {
        // the IYAKKI menu's Media player and other apps: `play:<path>`
        if let Some(path) = item.strip_prefix("play:") {
            self.start_queue(vec![path.to_string()], 0, cx);
        }
    }

    fn wants_redraw(&mut self) -> bool {
        if self.native.is_some() {
            return true;
        }
        self.video.as_mut().is_some_and(|v| v.full.take_redraw())
    }

    fn handle(&mut self, ev: &Event, cx: &AppCx) -> Flow {
        if matches!(ev, Event::Tick) {
            self.poll_scan();
            // a shared host folder that was unplugged or came back (or was added or removed) means a new look, about every 5 s
            self.host_ticks += 1;
            if self.host_ticks % 150 == 0 && self.scanning.is_none() && self.host_available() != self.host_seen {
                self.rescan = true;
            }
            self.poll_downloads();
            if self.rescan && self.scanning.is_none() {
                self.rescan = false;
                self.start_scan(cx);
            }
            self.adopt_queue(cx);
            if self.video.is_some() {
                self.video_event(ev, cx);
            }
            if let Some(n) = &mut self.native {
                n.event(ev);
            }
            if let Check::Ended { user } = self.player.check() {
                if user == cx.session.user {
                    self.advance(cx);
                }
            }
            return Flow::Stay;
        }
        // IYAKKI's own player has the keys and the page area while it plays
        if let Some(n) = &mut self.native {
            if n.event(ev) == super::mediaplay::Out::Close {
                self.native = None;
            }
            return Flow::Stay;
        }
        // a video has the keys and the page area while it plays
        if self.video.is_some() {
            self.video_event(ev, cx);
            return Flow::Stay;
        }
        match &mut self.mode {
            Mode::Form(form, _) => {
                match form.handle_event(ev) {
                    FormEvent::Cancel => {
                        self.mode = Mode::Browse;
                        self.pending.clear();
                    }
                    FormEvent::Submit => self.submit_form(cx),
                    _ => {}
                }
                return Flow::Stay;
            }
            Mode::Tag(form, _) => {
                match form.handle_event(ev) {
                    FormEvent::Cancel => self.mode = Mode::Browse,
                    FormEvent::Submit => self.submit_tags(cx),
                    _ => {}
                }
                return Flow::Stay;
            }
            Mode::Download(..) => {
                self.download_event(ev, cx);
                return Flow::Stay;
            }
            Mode::Pick(_) => {
                self.pick_event(ev, cx);
                return Flow::Stay;
            }
            Mode::DeletePlaylist(_) | Mode::RemoveFolder(_) => {
                match ev {
                    Event::Key(Key::Char('y')) | Event::Key(Key::Enter) => self.confirmed(cx),
                    Event::Key(Key::Esc) | Event::Key(Key::Char('n')) => self.mode = Mode::Browse,
                    _ => {}
                }
                return Flow::Stay;
            }
            Mode::Browse => {}
        }
        match ev {
            Event::Row(i) => {
                if *i == self.sel_now() && !self.searching {
                    self.activate(cx);
                } else {
                    self.set_sel(*i);
                }
                return Flow::Stay;
            }
            Event::Scroll(d) => {
                let i = (self.sel_now() as i32 - d).max(0) as usize;
                self.set_sel(i);
                return Flow::Stay;
            }
            Event::Tab(n) if *n < PAGES.len() => {
                self.goto(PAGES[*n].0);
                return Flow::Stay;
            }
            // the Play and Show in library buttons of a finished download
            Event::Tab(n) if (2000..7000).contains(n) => {
                self.set_sel(*n % 1000);
                match *n / 1000 {
                    3 => self.download_key('f', cx),
                    5 => self.download_key('r', cx),
                    6 => self.download_key('x', cx),
                    _ => self.activate_download(cx),
                }
                return Flow::Stay;
            }
            Event::Tab(100) => {
                self.stop(cx);
                return Flow::Stay;
            }
            Event::Tab(101) => {
                self.shuffle = !self.shuffle;
                self.flags_changed();
                return Flow::Stay;
            }
            Event::Tab(102) => {
                self.repeat = self.repeat.next();
                self.flags_changed();
                return Flow::Stay;
            }
            Event::Tab(103) => {
                self.step_queue(-1, cx);
                return Flow::Stay;
            }
            Event::Tab(104) => {
                self.step_queue(1, cx);
                return Flow::Stay;
            }
            Event::Tab(105) => {
                self.open_download(cx);
                return Flow::Stay;
            }
            Event::Tab(106) => {
                self.cycle_sort();
                return Flow::Stay;
            }
            Event::Focus(_) | Event::FocusAt(..) => {
                self.searching = true;
                if let Event::FocusAt(_, x) = ev {
                    self.query.click(*x);
                }
                return Flow::Stay;
            }
            Event::Paste(t) if self.searching => {
                self.query.paste(t);
                self.clamp();
                return Flow::Stay;
            }
            _ => {}
        }
        let Event::Key(k) = ev else { return Flow::Stay };
        if self.searching {
            match k {
                Key::Enter => self.searching = false,
                Key::Esc => {
                    self.query.clear();
                    self.searching = false;
                    self.set_sel(0);
                }
                Key::Up => self.set_sel(self.sel_now().saturating_sub(1)),
                Key::Down => self.set_sel(self.sel_now() + 1),
                other => {
                    self.query.handle(other);
                    self.set_sel(0);
                }
            }
            return Flow::Stay;
        }
        let sel = self.sel_now();
        let on_item = self.selected_path().is_some();
        match k {
            Key::Esc => {
                if self.drill.is_some() {
                    self.drill = None;
                    self.clamp();
                } else if !self.query.value.is_empty() {
                    self.query.clear();
                    self.set_sel(0);
                } else {
                    return Flow::Back;
                }
            }
            Key::Tab => self.step_page(1),
            Key::BackTab => self.step_page(-1),
            Key::Char(c @ '0'..='5') if on_item => self.rate_selected(*c as u8 - b'0', cx),
            Key::Char(c @ '1'..='9') => self.goto(PAGES[*c as usize - '1' as usize].0),
            Key::Char('0') => self.goto(PAGES[DOWNLOADS_PAGE].0),
            Key::Char(c @ ('x' | 'r' | 'd' | 'c' | 'f')) if self.page == Page::Downloads => self.download_key(*c, cx),
            Key::Up => self.set_sel(sel.saturating_sub(1)),
            Key::Down => self.set_sel(sel + 1),
            Key::PageUp => self.set_sel(sel.saturating_sub(10)),
            Key::PageDown => self.set_sel(sel + 10),
            Key::Home => self.set_sel(0),
            Key::End => self.set_sel(usize::MAX),
            Key::Enter => self.activate(cx),
            Key::Char('/') => self.searching = true,
            Key::Char('a') => self.ask_add(),
            Key::Char('t') => self.open_tags(),
            Key::Char('D') => self.open_download(cx),
            Key::Char('o') => self.cycle_sort(),
            Key::Char('s') => self.stop(cx),
            Key::Char(' ') => {
                if self.player.now_playing().is_some() {
                    self.stop(cx);
                } else {
                    self.activate(cx);
                }
            }
            Key::Char(',') => self.step_queue(-1, cx),
            Key::Char('.') => self.step_queue(1, cx),
            Key::Char('z') => {
                self.shuffle = !self.shuffle;
                self.flags_changed();
            }
            Key::Char('l') => {
                self.repeat = self.repeat.next();
                self.flags_changed();
            }
            Key::Char('R') => {
                self.start_scan(cx);
                self.say("Looking at the folders again.", true);
            }
            Key::Char('i') => {
                if let Some(p) = self.selected_path() {
                    if let Some(it) = self.item(&p) {
                        let src = if it.source_url.is_empty() { String::new() } else { format!(" · downloaded from {}", it.source_url) };
                        self.say(format!("{} · played {} time{}{} · {}{}", it.kind.name(), it.plays, if it.plays == 1 { "" } else { "s" }, if it.rating > 0 { format!(" · {}", it.stars()) } else { String::new() }, p, src), true);
                    }
                }
            }
            Key::Char('n') if self.page == Page::Playlists && self.drill.is_none() => {
                self.pending.clear();
                self.mode = Mode::Form(Form::new("New playlist").text("Name", "", "e.g. Evening"), FormFor::NewPlaylist);
            }
            Key::Char('n') if self.page == Page::Folders => {
                self.mode = Mode::Form(Form::new("Add a folder").text("Folder", "", "/home/me/Music"), FormFor::Folder);
            }
            Key::Char('r') if self.page == Page::Playlists => {
                let name = match (&self.drill, self.selected()) {
                    (Some(Drill::Playlist(n)), _) => Some(n.clone()),
                    (None, Some(Row { act: Act::Playlist(n), .. })) => Some(n),
                    _ => None,
                };
                if let Some(n) = name {
                    self.mode = Mode::Form(Form::new("Rename playlist").text("Name", &n, ""), FormFor::Rename(n));
                }
            }
            Key::Char('d') if self.page == Page::Playlists && self.drill.is_none() => {
                if let Some(Row { act: Act::Playlist(n), .. }) = self.selected() {
                    self.mode = Mode::DeletePlaylist(n);
                }
            }
            // a linked host folder: convert it to a copy, or stop sharing it (the options open over the page)
            Key::Char('c') | Key::Char('u') if self.page == Page::Folders && matches!(self.selected(), Some(Row { act: Act::Folder(f), .. }) if self.store.host_mount_states().iter().any(|(m, _)| m.path == *f)) => {
                if let Some(Row { act: Act::Folder(f), .. }) = self.selected() {
                    self.host_menu = self.store.host_mount_states().into_iter().find(|(m, _)| m.path == f).map(|(m, _)| m.name);
                }
            }
            Key::Char('d') if self.page == Page::Folders => {
                if let Some(Row { act: Act::Folder(f), .. }) = self.selected() {
                    if self.samples.as_deref().is_some_and(|d| d.to_string_lossy() == f.as_str()) {
                        self.say("The IYAKKI samples are built in and read-only: they cannot be removed.", false);
                    } else if self.store.host_mount_states().iter().any(|(m, _)| m.path == f) {
                        self.say("A shared host folder is stopped under Host files in the DATA tile (it is never deleted).", false);
                    } else {
                        self.mode = Mode::RemoveFolder(f);
                    }
                }
            }
            Key::Char('x') => {
                if let (Some(name), Some(Row { act: Act::Item(i, _), .. })) = (self.open_list(), self.selected()) {
                    let _ = self.store.remove_item(&name, i);
                    self.clamp();
                }
            }
            Key::Char('[') => self.move_item(-1),
            Key::Char(']') => self.move_item(1),
            Key::Char('P') if self.page == Page::Playlists => {
                let name = match (&self.drill, self.selected()) {
                    (Some(Drill::Playlist(n)), _) => Some(n.clone()),
                    (None, Some(Row { act: Act::Playlist(n), .. })) => Some(n),
                    _ => None,
                };
                if let Some(n) = name {
                    self.play_playlist_file(&n, cx);
                }
            }
            _ => {}
        }
        Flow::Stay
    }

    fn render(&mut self, g: &mut Gfx, area: R, s: f32, _cx: &AppCx, tick: u64) {
        self.area = area;
        self.ui_scale = s;
        if let Some(n) = &mut self.native {
            n.render(g, area, s);
            return;
        }
        if self.video.is_some() {
            self.render_video(g, area, s);
            return;
        }
        let x = area.x + 24.0 * s;
        let w = area.w - 48.0 * s;
        let mut y = area.y + 10.0 * s;
        y += self.draw_hero(g, R::new(x, y, w, 0.0), s, tick) + 14.0 * s;

        // ---- pages, as glowing pills (the row slides along when the window is narrow)
        let names: Vec<&str> = PAGES.iter().map(|p| p.1).collect();
        let widths: Vec<f32> = names.iter().map(|n| g.text_width(14.0 * s, n, Face::Bold) + 26.0 * s).collect();
        let cur = self.page_idx();
        let mut first = 0;
        while first < cur && widths[first..=cur].iter().sum::<f32>() + 8.0 * s * (cur - first) as f32 > w - 40.0 * s {
            first += 1;
        }
        let mut tx = x;
        if first > 0 {
            g.text(tx, y + 6.0 * s, 16.0 * s, DIM, "‹", Face::Bold);
            tx += 18.0 * s;
        }
        for i in first..PAGES.len() {
            if tx + widths[i] > x + w - 14.0 * s {
                g.text(x + w - 12.0 * s, y + 6.0 * s, 16.0 * s, DIM, "›", Face::Bold);
                break;
            }
            let r = R::new(tx, y, widths[i], 30.0 * s);
            let on = i == cur;
            g.rrect(r.x, r.y, r.w, r.h, 15.0 * s, if on { with_alpha(ACCENT, 46) } else if g.hovered(r) { CARD_HOVER } else { rgb(18, 24, 46) });
            if on {
                g.rrect_outline(r.x, r.y, r.w, r.h, 15.0 * s, 1.5 * s, ACCENT);
            }
            g.text(r.x + 13.0 * s, r.y + 6.0 * s, 14.0 * s, if on { ACCENT } else { MUTED }, names[i], Face::Bold);
            g.hit(r, Event::Tab(i), names[i]);
            tx += widths[i] + 8.0 * s;
        }
        y += 40.0 * s;

        // ---- search, what it understood, sort and download
        let sw = (w * 0.4).clamp(160.0 * s, 380.0 * s);
        let sr = R::new(x, y, sw, 36.0 * s);
        if self.query.value.is_empty() && !self.searching {
            self.query.placeholder = "/ search: kutthu song, melody above 4 stars…".into();
        } else {
            self.query.placeholder.clear();
        }
        self.query.render(g, sr, self.searching, s, blink(tick), Face::Sans, 0);
        let mut cx_ = x + sw + 12.0 * s;
        let right_btns = g.text_width(13.0 * s, "Download (D)", Face::Bold) + 28.0 * s + g.text_width(13.0 * s, &format!("Sort: {} (o)", self.sort.label()), Face::Bold) + 28.0 * s + 8.0 * s;
        let chip_room = (x + w - right_btns - cx_).max(0.0);
        let mut used = 0.0;
        for chip in self.chips() {
            let t = chip.strip_prefix("type: ").unwrap_or(&chip).to_string();
            let col = if chip.starts_with("type: ") { type_color(&t) } else { CYAN };
            let cw = g.text_width(12.0 * s, &chip, Face::Bold) + 20.0 * s;
            if used + cw > chip_room {
                break;
            }
            g.rrect(cx_, y + 5.0 * s, cw, 26.0 * s, 13.0 * s, with_alpha(col, 40));
            g.rrect_outline(cx_, y + 5.0 * s, cw, 26.0 * s, 13.0 * s, 1.0 * s, with_alpha(col, 170));
            g.text(cx_ + 10.0 * s, y + 10.0 * s, 12.0 * s, col, &chip, Face::Bold);
            cx_ += cw + 6.0 * s;
            used += cw + 6.0 * s;
        }
        let mut bx = x + w;
        let dw = g.text_width(13.0 * s, "Download (D)", Face::Bold) + 28.0 * s;
        bx -= dw;
        g.button(bx, y + 3.0 * s, 13.0 * s, "Download (D)", CYAN, Event::Tab(105));
        let sl = format!("Sort: {} (o)", self.sort.label());
        let sw2 = g.text_width(13.0 * s, &sl, Face::Bold) + 28.0 * s;
        bx -= sw2 + 8.0 * s;
        g.button(bx, y + 3.0 * s, 13.0 * s, &sl, if self.sort == Sort::Relevance { MUTED } else { AMBER }, Event::Tab(106));
        y += 46.0 * s;

        // ---- the last thing said
        if let Some((m, ok)) = &self.status {
            let m = g.fit(14.0 * s, m, Face::Sans, w);
            g.text(x, y, 14.0 * s, if *ok { OK } else { ERR }, &m, Face::Sans);
        }
        y += 24.0 * s;
        if let Some(d) = &self.drill {
            let t = match d {
                Drill::Artist(a) => format!("Artist: {a}   (Esc goes back)"),
                Drill::Album(a, b) => format!("Album: {b} by {a}   (Esc goes back)"),
                Drill::Playlist(n) => format!("Playlist: {n}   (x remove · [ ] move · P play it all · Esc back)"),
                Drill::Type(t) => format!("Type: {t}   (Esc goes back)"),
                Drill::Lang(l) => format!("Language: {l}   (Esc goes back)"),
            };
            let t = g.fit(14.0 * s, &t, Face::Bold, w);
            g.text(x, y, 14.0 * s, ACCENT, &t, Face::Bold);
            y += 24.0 * s;
        }

        // ---- the list, in dark glass cards
        let rows = self.rows();
        let sel = self.sel_now().min(rows.len().saturating_sub(1));
        if rows.is_empty() {
            let msg = if self.scanning.is_some() {
                "Looking through your folders…".to_string()
            } else {
                match self.page {
                    _ if !self.q().is_empty() => "Nothing matches the search.".to_string(),
                    Page::Playlists if self.drill.is_none() => "No playlists yet. Press n to make one, or a on a track.".to_string(),
                    Page::Playlists => "This playlist is empty. Add tracks with a.".to_string(),
                    Page::Folders => "No folders. Press n to add one.".to_string(),
                    Page::Types => "No types yet: they appear as the library is scanned. Press t on a track to choose its types.".to_string(),
                    Page::TopRated => "Nothing is rated yet. Choose a track and press 1 to 5.".to_string(),
                    Page::Photos => "No pictures found. They are looked for in your Pictures folder; press D to download one.".to_string(),
                    Page::Downloads => "No downloads. Press D to add one or more addresses to the queue.".to_string(),
                    _ => "No media files in your folders. Press 6 to see which folders are looked at, or n there to add one.".to_string(),
                }
            };
            g.text(x, y + 8.0 * s, 16.0 * s, MUTED, &msg, Face::Sans);
        } else {
            let rh = 50.0 * s;
            let fit = (((area.y + area.h - y - 8.0 * s) / (rh + 5.0 * s)).floor() as usize).max(1);
            let start = sel.saturating_sub(fit - 1);
            let now = self.queue.get(self.qpos).filter(|_| self.queue_active).cloned().or_else(|| self.player.now_target());
            let playing_any = self.player.now_playing().is_some();
            for (i, row) in rows.iter().enumerate().skip(start).take(fit) {
                let r = R::new(x, y, w, rh);
                let is_sel = i == sel;
                let playing = matches!((&row.act, &now), (Act::Track(p) | Act::Item(_, p), Some(n)) if p == n) && playing_any;
                let tint = row.types.first().map_or(ACCENT, |t| type_color(t));
                g.rrect(r.x, r.y, r.w, r.h, 12.0 * s, if is_sel { CARD_SEL } else if g.hovered(r) { CARD_HOVER } else { CARD });
                if playing {
                    // the playing row pulses
                    let pulse = 0.5 + 0.5 * (tick as f32 * 0.15).sin();
                    g.rrect_outline(r.x, r.y, r.w, r.h, 12.0 * s, 1.6 * s, with_alpha(OK, (110.0 + 120.0 * pulse) as u8));
                } else if is_sel {
                    g.rrect_outline(r.x, r.y, r.w, r.h, 12.0 * s, 1.2 * s, with_alpha(ACCENT, 190));
                }
                g.rect(r.x + 3.0 * s, r.y + 9.0 * s, 3.0 * s, r.h - 18.0 * s, with_alpha(tint, if is_sel || playing { 255 } else { 110 }));
                g.hit(r, Event::Row(i), &row.main);
                if let Some(d) = &row.dl {
                    draw_download_row(g, r, d, row, i, is_sel, s, tick);
                    y += rh + 5.0 * s;
                    continue;
                }
                let mut left = 18.0 * s;
                if playing {
                    for k in 0..4 {
                        let hh = (6.0 + 14.0 * (0.5 + 0.5 * ((tick as f32 * 0.25) + k as f32 * 1.7).sin())) * s;
                        g.rrect(r.x + 16.0 * s + k as f32 * 6.0 * s, r.y + rh / 2.0 + 9.0 * s - hh, 4.0 * s, hh, 1.5 * s, mix(CYAN, MAGENTA, k as f32 / 3.0));
                    }
                    left = 46.0 * s;
                }
                // right side: duration, stars, type chips (from the edge inwards)
                let mut rx = r.x + r.w - 14.0 * s;
                let rw = g.text_width(13.0 * s, &row.right, Face::Sans);
                rx -= rw;
                g.text(rx, r.y + 17.0 * s, 13.0 * s, DIM, &row.right, Face::Sans);
                if row.rating > 0 {
                    let st = mediaindex::stars_text(row.rating);
                    let sw = g.text_width(13.0 * s, &st, Face::Sans);
                    rx -= sw + 14.0 * s;
                    g.text(rx, r.y + 17.0 * s, 13.0 * s, AMBER, &st, Face::Sans);
                }
                let mut chips_left = rx;
                if r.w > 520.0 * s {
                    for t in row.types.iter().take(2) {
                        let cw = g.text_width(11.0 * s, t, Face::Bold) + 16.0 * s;
                        chips_left -= cw + 6.0 * s;
                        let col = type_color(t);
                        g.rrect(chips_left, r.y + 14.0 * s, cw, 22.0 * s, 11.0 * s, with_alpha(col, 38));
                        g.rrect_outline(chips_left, r.y + 14.0 * s, cw, 22.0 * s, 11.0 * s, 1.0 * s, with_alpha(col, 160));
                        g.text(chips_left + 8.0 * s, r.y + 18.0 * s, 11.0 * s, col, t, Face::Bold);
                    }
                }
                let room = (chips_left - r.x - left - 16.0 * s).max(40.0 * s);
                let main = g.fit(16.0 * s, &row.main, Face::Bold, room);
                if row.sub.is_empty() {
                    g.text(r.x + left, r.y + 14.0 * s, 16.0 * s, if playing { OK } else { TEXT }, &main, Face::Bold);
                } else {
                    g.text(r.x + left, r.y + 6.0 * s, 16.0 * s, if playing { OK } else { TEXT }, &main, Face::Bold);
                    let sub = g.fit(12.0 * s, &row.sub, Face::Sans, room);
                    g.text(r.x + left, r.y + 28.0 * s, 12.0 * s, MUTED, &sub, Face::Sans);
                }
                y += rh + 5.0 * s;
            }
        }

        match &self.mode {
            Mode::Form(f, _) => f.render(g, area, s, ACCENT, blink(tick)),
            Mode::Tag(f, _) => f.render(g, area, s, ACCENT, blink(tick)),
            Mode::Download(f, _) => f.render(g, area, s, CYAN, blink(tick)),
            Mode::Pick(p) => p.render(g, area, s, ACCENT),
            Mode::DeletePlaylist(n) => render_confirm(g, area, s, &format!("Delete the playlist {n}? The files stay."), "Delete"),
            Mode::RemoveFolder(f) => render_confirm(g, area, s, &format!("Stop looking in {f}? The files stay."), "Remove"),
            Mode::Browse => {}
        }
    }
}

impl MediaApp {
    /// The now-playing card: an orb that pulses while something plays, the title, type chips and stars, round transport
    /// buttons, a progress bar and the level display. Returns its height.
    fn draw_hero(&mut self, g: &mut Gfx, at: R, s: f32, tick: u64) -> f32 {
        let (x, w, y) = (at.x, at.w, at.y);
        let ch = 178.0 * s;
        let card = R::new(x, y, w, ch);
        let label = self.player.now_playing();
        let playing = label.is_some();
        let target = self.player.now_target();
        let item = target.as_deref().and_then(|t| self.item(t));
        let accent = item.as_ref().and_then(|i| i.effective_types().iter().next().map(|t| type_color(t))).unwrap_or(CYAN);
        let t = tick as f32;
        let glow = if playing { 0.45 + 0.4 * (0.5 + 0.5 * (t * 0.1).sin()) } else { 0.15 };
        glow_card(g, card, s, accent, MAGENTA, glow);

        // the orb
        let dr = 44.0 * s;
        let (ox, oy) = (card.x + 26.0 * s + dr, card.y + 20.0 * s + dr);
        let pulse = if playing { 5.0 * s * (0.5 + 0.5 * (t * 0.12).sin()) } else { 0.0 };
        g.circle(ox, oy, dr + 10.0 * s + pulse, with_alpha(accent, if playing { 26 } else { 12 }));
        g.circle(ox, oy, dr, rgb(12, 17, 38));
        g.ring(ox, oy, dr, 2.0 * s, with_alpha(accent, 170));
        if playing {
            for k in 0..24 {
                let a = t * 0.03 + k as f32 * std::f32::consts::TAU / 24.0;
                let (r0, r1) = (dr - 11.0 * s, dr - 5.0 * s);
                let lit = k % 6 == 0;
                g.line(ox + a.cos() * r0, oy + a.sin() * r0, ox + a.cos() * r1, oy + a.sin() * r1, 2.0 * s, if lit { MAGENTA } else { with_alpha(MUTED, 90) });
            }
        }
        let note = "♪";
        let nw = g.text_width(34.0 * s, note, Face::Bold);
        g.text(ox - nw / 2.0, oy - 20.0 * s, 34.0 * s, if playing { accent } else { DIM }, note, Face::Bold);

        // the words
        let tx = ox + dr + 28.0 * s;
        let tw = (card.x + card.w - tx - 24.0 * s).max(80.0 * s);
        g.text(tx, card.y + 16.0 * s, 12.0 * s, if playing { MAGENTA } else { DIM }, if playing { "● NOW PLAYING" } else { "○ STANDBY" }, Face::Bold);
        let (head, sub) = match (&item, &label) {
            (Some(i), _) => (i.title.clone(), [i.artist.as_str(), i.album.as_str()].iter().filter(|x| !x.is_empty()).cloned().collect::<Vec<_>>().join(" · ")),
            (None, Some(l)) => (l.clone(), String::new()),
            (None, None) => ("Nothing is playing".to_string(), "Press Enter on a track, or ask the chat: play a kutthu song".to_string()),
        };
        let head = g.fit(26.0 * s, &head, Face::Bold, tw);
        g.text(tx, card.y + 34.0 * s, 26.0 * s, TEXT, &head, Face::Bold);
        let sub = g.fit(14.0 * s, &sub, Face::Sans, tw);
        g.text(tx, card.y + 68.0 * s, 14.0 * s, MUTED, &sub, Face::Sans);
        // type chips and the stars
        if let Some(i) = &item {
            let mut cxp = tx;
            for ty in i.effective_types().iter().chain(i.tags.iter()).take(4) {
                let col = type_color(ty);
                let cw = g.text_width(12.0 * s, ty, Face::Bold) + 20.0 * s;
                if cxp + cw > tx + tw - 90.0 * s {
                    break;
                }
                g.rrect(cxp, card.y + 90.0 * s, cw, 24.0 * s, 12.0 * s, with_alpha(col, 40));
                g.rrect_outline(cxp, card.y + 90.0 * s, cw, 24.0 * s, 12.0 * s, 1.0 * s, with_alpha(col, 180));
                g.text(cxp + 10.0 * s, card.y + 95.0 * s, 12.0 * s, col, ty, Face::Bold);
                cxp += cw + 6.0 * s;
            }
            if i.rating > 0 {
                g.text(cxp + 6.0 * s, card.y + 94.0 * s, 15.0 * s, AMBER, &i.stars(), Face::Sans);
            }
        }

        // the downloads chip, top right: what the queue is doing, flashing when a download ended
        if let Some((text, ok)) = self.downloads_chip() {
            let col = if self.flash.is_some() { if ok { OK } else { ERR } } else { CYAN };
            let cwid = g.text_width(12.0 * s, &text, Face::Bold) + 22.0 * s;
            let cx0 = card.x + card.w - cwid - 18.0 * s;
            if cx0 > tx + 150.0 * s {
                let pulse = if self.flash.is_some() { 120 + (110.0 * (0.5 + 0.5 * (t * 0.35).sin())) as u8 } else { 150 };
                let rect = R::new(cx0, card.y + 12.0 * s, cwid, 26.0 * s);
                g.rrect(rect.x, rect.y, rect.w, rect.h, 13.0 * s, with_alpha(col, if self.flash.is_some() { 60 } else { 34 }));
                g.rrect_outline(rect.x, rect.y, rect.w, rect.h, 13.0 * s, 1.2 * s, with_alpha(col, pulse));
                g.text(rect.x + 11.0 * s, rect.y + 6.0 * s, 12.0 * s, col, &text, Face::Bold);
                g.hit(rect, Event::Tab(DOWNLOADS_PAGE), "Downloads");
            }
        }

        // round transport buttons (placed from the left of the words), then shuffle and repeat
        let by = card.y + 124.0 * s;
        let bd = 40.0 * s;
        let mut bx = tx;
        let prev = round_button(g, R::new(bx, by, bd, bd), s, Glyph::Previous, None);
        g.hit(prev, Event::Tab(103), "Previous");
        bx += bd + 10.0 * s;
        let main = R::new(bx - 2.0 * s, by - 4.0 * s, bd + 8.0 * s, bd + 8.0 * s);
        round_button(g, main, s, if playing { Glyph::Stop } else { Glyph::Play }, Some(if playing { MAGENTA } else { accent }));
        g.hit(main, if playing { Event::Tab(100) } else { Event::Key(Key::Char(' ')) }, "Play or stop");
        bx += bd + 18.0 * s;
        let next = round_button(g, R::new(bx, by, bd, bd), s, Glyph::Next, None);
        g.hit(next, Event::Tab(104), "Next");
        bx += bd + 16.0 * s;
        let sl = format!("Shuffle {} (z)", if self.shuffle { "on" } else { "off" });
        bx += g.button(bx, by + 4.0 * s, 13.0 * s, &sl, if self.shuffle { ACCENT } else { MUTED }, Event::Tab(101)) + 8.0 * s;
        let rl = format!("Repeat {} (l)", self.repeat.label());
        g.button(bx, by + 4.0 * s, 13.0 * s, &rl, if self.repeat == Repeat::Off { MUTED } else { ACCENT }, Event::Tab(102));
        // a stop button is always reachable by click, also while idle
        if !playing {
            g.hit(R::new(0.0, 0.0, 0.0, 0.0), Event::Tab(100), "Stop");
        }

        // the level display, on the right of the card
        let lw = (card.w * 0.30).clamp(120.0 * s, 360.0 * s);
        let lx = card.x + card.w - lw - 24.0 * s;
        if lw > 0.0 && tx + 330.0 * s < lx {
            level_bars(g, lx, card.y + 106.0 * s, lw, 70.0 * s, s, &visualizer(tick, playing), accent, MAGENTA);
        }

        // progress: elapsed over the length when it is known
        let py = card.y + card.h - 16.0 * s;
        let (pl, pw) = (card.x + 26.0 * s, card.w - 52.0 * s);
        g.rrect(pl, py, pw, 5.0 * s, 2.5 * s, rgb(28, 36, 62));
        if playing {
            let el = self.player.elapsed().unwrap_or(0);
            let dur = item.as_ref().map_or(0, |i| i.seconds) as u64;
            let frac = if dur > 0 { (el as f32 / dur as f32).min(1.0) } else { ((tick % 90) as f32 / 90.0) * 0.2 + 0.05 };
            g.rrect(pl, py, (pw * frac).max(6.0 * s), 5.0 * s, 2.5 * s, mix(accent, MAGENTA, frac));
            g.circle(pl + (pw * frac).max(6.0 * s), py + 2.5 * s, 5.0 * s, TEXT);
            let clock = if dur > 0 { format!("{}:{:02} / {}:{:02}", el / 60, el % 60, dur / 60, dur % 60) } else { format!("{}:{:02}", el / 60, el % 60) };
            let cw = g.text_width(12.0 * s, &clock, Face::Mono);
            g.text(pl + pw - cw, py - 16.0 * s, 12.0 * s, MUTED, &clock, Face::Mono);
        }
        ch
    }
}

/// A short name for an address: its last path part (decoded), else its host.
pub fn url_label(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let last = path.rsplit('/').find(|p| !p.is_empty() && !p.ends_with(':')).unwrap_or(url);
    let decoded = last.replace("%20", " ");
    if decoded.is_empty() { url.chars().take(60).collect() } else { decoded }
}

/// 1536 -> "1.5 KB", 5 * 2^20 -> "5.0 MB".
pub fn human_bytes(n: u64) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1 << 20 {
        format!("{:.1} KB", n as f64 / 1024.0)
    } else if n < 1 << 30 {
        format!("{:.1} MB", n as f64 / (1u64 << 20) as f64)
    } else {
        format!("{:.2} GB", n as f64 / (1u64 << 30) as f64)
    }
}

/// "26 s", "3 min 05 s", "1 h 02 min".
pub fn human_secs(n: u64) -> String {
    if n < 60 {
        format!("{n} s")
    } else if n < 3600 {
        format!("{} min {:02} s", n / 60, n % 60)
    } else {
        format!("{} h {:02} min", n / 3600, n % 3600 / 60)
    }
}

/// The second line of a download row: what state it is in, in plain words.
pub fn status_line(d: &View) -> String {
    let folder = mediaqueue::folder_label(&d.folder);
    let place = if folder.is_empty() { String::new() } else { format!("Folder: {folder}{}", if d.auto { " (auto)" } else { "" }) };
    match d.status {
        Status::Queued => format!("Queued · waiting for a free place{}", if place.is_empty() { String::new() } else { format!(" · {place}") }),
        Status::Downloading => {
            let mut t = format!("Downloading {}", human_bytes(d.bytes));
            if d.total > 0 {
                t.push_str(&format!(" of {} ({}%)", human_bytes(d.total), (d.bytes * 100 / d.total).min(100)));
            }
            if d.speed > 0 {
                t.push_str(&format!(" · {}/s", human_bytes(d.speed)));
            }
            if let Some(e) = d.eta_secs {
                t.push_str(&format!(" · {} left", human_secs(e)));
            }
            if !place.is_empty() {
                t.push_str(&format!(" · {place}"));
            }
            t
        }
        Status::Succeeded if d.name.is_empty() => format!("Succeeded · {}", d.note),
        Status::Succeeded => format!("Succeeded · {} · {} · {}", d.name, if folder.is_empty() { "?".to_string() } else { folder }, human_bytes(d.size)),
        Status::Failed => format!("Failed · {}", d.error),
        Status::Cancelled => "Cancelled".to_string(),
    }
}

fn status_color(st: Status) -> [u8; 4] {
    match st {
        Status::Queued => MUTED,
        Status::Downloading => CYAN,
        Status::Succeeded => OK,
        Status::Failed => ERR,
        Status::Cancelled => AMBER,
    }
}

/// One row of the Downloads page: the name, the state in words, a progress bar while it runs and the actions that fit.
#[allow(clippy::too_many_arguments)]
fn draw_download_row(g: &mut Gfx, r: R, d: &View, row: &Row, i: usize, is_sel: bool, s: f32, tick: u64) {
    let col = status_color(d.status);
    g.rect(r.x + 3.0 * s, r.y + 9.0 * s, 3.0 * s, r.h - 18.0 * s, with_alpha(col, if is_sel { 255 } else { 140 }));
    // the actions on the right, from the edge inwards
    let mut rx = r.x + r.w - 12.0 * s;
    let button = |g: &mut Gfx, label: &str, ev: Event, c: [u8; 4], rx: &mut f32| {
        let bw = g.text_width(12.0 * s, label, Face::Bold) + 22.0 * s;
        *rx -= bw;
        g.rrect(*rx, r.y + 12.0 * s, bw, 26.0 * s, 13.0 * s, with_alpha(c, 34));
        g.rrect_outline(*rx, r.y + 12.0 * s, bw, 26.0 * s, 13.0 * s, 1.0 * s, with_alpha(c, 170));
        g.text(*rx + 11.0 * s, r.y + 18.0 * s, 12.0 * s, c, label, Face::Bold);
        g.hit(R::new(*rx, r.y + 12.0 * s, bw, 26.0 * s), ev, label);
        *rx -= 8.0 * s;
    };
    match d.status {
        Status::Succeeded if !d.file.is_empty() => {
            button(g, "Show in library", Event::Tab(3000 + i), CYAN, &mut rx);
            button(g, "Play", Event::Tab(2000 + i), OK, &mut rx);
        }
        Status::Failed | Status::Cancelled => button(g, "Retry", Event::Tab(5000 + i), AMBER, &mut rx),
        Status::Queued => button(g, "Cancel", Event::Tab(6000 + i), MUTED, &mut rx),
        Status::Downloading => {
            button(g, "Cancel", Event::Tab(6000 + i), MUTED, &mut rx);
            let bw = (r.w * 0.22).clamp(70.0 * s, 220.0 * s);
            let bar = R::new(rx - bw, r.y + 20.0 * s, bw, 9.0 * s);
            g.rrect(bar.x, bar.y, bar.w, bar.h, 4.5 * s, rgb(28, 36, 62));
            let frac = if d.total > 0 { (d.bytes as f32 / d.total as f32).clamp(0.0, 1.0) } else { ((tick % 60) as f32 / 60.0) * 0.3 + 0.1 };
            g.rrect(bar.x, bar.y, (bar.w * frac).max(6.0 * s), bar.h, 4.5 * s, mix(CYAN, MAGENTA, frac));
            rx = bar.x - 10.0 * s;
        }
        _ => {}
    }
    let room = (rx - r.x - 22.0 * s).max(60.0 * s);
    let main = g.fit(16.0 * s, &row.main, Face::Bold, room);
    g.text(r.x + 18.0 * s, r.y + 6.0 * s, 16.0 * s, TEXT, &main, Face::Bold);
    let sub = g.fit(12.0 * s, &row.sub, Face::Sans, room);
    g.text(r.x + 18.0 * s, r.y + 28.0 * s, 12.0 * s, col, &sub, Face::Sans);
}

impl MediaApp {
    /// A video in the page area: a thin bar with the title and the keys, then the engine's picture.
    fn render_video(&mut self, g: &mut Gfx, area: R, s: f32) {
        let Some(v) = &mut self.video else { return };
        let bar = 40.0 * s;
        g.rect(area.x, area.y, area.w, bar, rgb(12, 16, 32));
        let t = g.fit(15.0 * s, &v.title, Face::Bold, (area.w - 330.0 * s).max(60.0 * s));
        g.text(area.x + 16.0 * s, area.y + 11.0 * s, 15.0 * s, TEXT, &t, Face::Bold);
        let mut bx = area.x + area.w - 14.0 * s;
        for (label, ev, col) in [("Stop (Esc)", Event::Tab(200), MAGENTA), ("Pause / play (Space)", Event::Tab(201), CYAN)] {
            let bw = g.text_width(12.0 * s, label, Face::Bold) + 24.0 * s;
            bx -= bw;
            g.button(bx, area.y + 7.0 * s, 12.0 * s, label, col, ev);
            bx -= 8.0 * s;
        }
        let page = R::new(area.x, area.y + bar, area.w, (area.h - bar).max(1.0));
        v.full.draw(g, page, s);
        if !v.full.is_ready() {
            g.text(area.x + 24.0 * s, area.y + bar + 24.0 * s, 16.0 * s, MUTED, "Starting the video player…", Face::Sans);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::tests::{cx_with_admin_store, temp_dir};
    use iyakki_agent::media::{Launcher, Running};

    struct FakeRun(Arc<Mutex<(bool, bool)>>);
    impl Running for FakeRun {
        fn alive(&mut self) -> bool {
            self.0.lock().unwrap().0
        }
        fn stop(&mut self) {
            let mut g = self.0.lock().unwrap();
            *g = (false, true);
        }
    }

    #[derive(Default)]
    struct Fake {
        launched: Mutex<Vec<String>>,
        procs: Mutex<Vec<Arc<Mutex<(bool, bool)>>>>,
    }
    impl Launcher for Fake {
        fn launch(&self, target: &str) -> Result<Box<dyn Running>, String> {
            self.launched.lock().unwrap().push(target.to_string());
            let st = Arc::new(Mutex::new((true, false)));
            self.procs.lock().unwrap().push(st.clone());
            Ok(Box::new(FakeRun(st)))
        }
    }

    fn put(dir: &std::path::Path, rel: &str, bytes: &[u8]) -> String {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, bytes).unwrap();
        p.to_str().unwrap().to_string()
    }

    fn mp3(title: &str, artist: &str, album: &str) -> Vec<u8> {
        let frame = |id: &str, t: &str| {
            let mut f = id.as_bytes().to_vec();
            f.extend(((t.len() + 1) as u32).to_be_bytes());
            f.extend([0, 0, 3]);
            f.extend(t.as_bytes());
            f
        };
        let body: Vec<u8> = [frame("TIT2", title), frame("TPE1", artist), frame("TALB", album)].concat();
        let mut v = b"ID3\x03\x00\x00".to_vec();
        let n = body.len();
        v.extend([0, 0, ((n >> 7) & 0x7F) as u8, (n & 0x7F) as u8]);
        v.extend(body);
        let mut fr = vec![0u8; 417];
        fr[..4].copy_from_slice(&[0xFF, 0xFB, 0x90, 0x64]);
        v.extend(fr);
        v
    }

    /// An app over a temp library: Ann has two tracks, Bob one, plus a video; a second folder is outside the scope.
    fn setup(tag: &str) -> (MediaApp, AppCx, Arc<Fake>, std::path::PathBuf) {
        let mut cx = cx_with_admin_store(tag);
        let d = temp_dir(&format!("{tag}-lib"));
        put(&d, "mine/music/a1.mp3", &mp3("Alpha", "Ann", "First"));
        put(&d, "mine/music/a2.mp3", &mp3("Beta", "Ann", "First"));
        put(&d, "mine/music/b1.mp3", &mp3("Gamma", "Bob", "Other"));
        put(&d, "mine/clip.mp4", b"\0\0\0\x14ftypisom\0\0\0\0isom");
        put(&d, "other/secret.mp3", &mp3("Secret", "Eve", "Hidden"));
        cx.session.policy.fs_read = vec![d.join("mine").to_str().unwrap().to_string()];
        let store = MediaStore::new(cx.services.dir(), &cx.session.user);
        store.add_folder(d.join("mine").to_str().unwrap()).unwrap();
        store.add_folder(d.join("other").to_str().unwrap()).unwrap();
        store.remove_folder(&format!("/home/{}/Music", cx.session.user)).unwrap();
        store.remove_folder(&format!("/home/{}/Videos", cx.session.user)).unwrap();
        let fake = Arc::new(Fake::default());
        let app = MediaApp::with_player(&cx, Arc::new(MediaPlayer::new(fake.clone())));
        let mut app = app;
        wait_scan(&mut app, &cx);
        (app, cx, fake, d)
    }

    fn wait_scan(a: &mut MediaApp, cx: &AppCx) {
        for _ in 0..500 {
            a.handle(&Event::Tick, cx);
            if !a.is_scanning() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the scan did not finish");
    }

    fn key(a: &mut MediaApp, cx: &AppCx, k: Key) {
        a.handle(&Event::Key(k), cx);
    }

    fn typ(a: &mut MediaApp, cx: &AppCx, s: &str) {
        for c in s.chars() {
            key(a, cx, Key::Char(c));
        }
    }

    fn draw(a: &mut MediaApp, cx: &AppCx, w: u32, h: u32) -> Gfx {
        let mut g = Gfx::new(w, h);
        a.render(&mut g, R::new(0.0, 72.0, w as f32, h as f32 - 72.0), (w as f32 / 1280.0).clamp(0.6, 3.0), cx, 0);
        g
    }

    #[test]
    fn the_library_shows_only_what_the_account_may_read_and_browses_by_page() {
        let (mut a, cx, _f, _d) = setup("media-browse");
        assert_eq!(a.row_names(), vec!["Alpha", "Beta", "Gamma"], "the folder outside the read scope is not scanned");
        assert!(a.status_text().unwrap().contains("3 music files and 1 video"));
        key(&mut a, &cx, Key::Tab);
        assert_eq!((a.page(), a.row_names()), (Page::Artists, vec!["Ann".to_string(), "Bob".to_string()]));
        key(&mut a, &cx, Key::Tab);
        assert_eq!(a.row_names(), vec!["First", "Other"]);
        key(&mut a, &cx, Key::Tab);
        assert_eq!((a.page(), a.row_names()), (Page::Videos, vec!["clip".to_string()]));
        // an artist opens to its tracks, Esc steps back out, then leaves
        key(&mut a, &cx, Key::BackTab);
        key(&mut a, &cx, Key::BackTab);
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.row_names(), vec!["Alpha", "Beta"]);
        for w in [(1280, 720), (700, 500)] {
            let _ = draw(&mut a, &cx, w.0, w.1);
        }
        key(&mut a, &cx, Key::Esc);
        assert_eq!(a.row_names().len(), 2);
        assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
        // search
        a.handle(&Event::Tab(0), &cx);
        key(&mut a, &cx, Key::Char('/'));
        typ(&mut a, &cx, "gam");
        assert_eq!(a.row_names(), vec!["Gamma"]);
        key(&mut a, &cx, Key::Esc);
        assert_eq!(a.row_names().len(), 3, "Esc clears the search");
    }

    #[test]
    fn enter_plays_through_the_player_and_the_queue_follows_repeat_and_stop() {
        let (mut a, cx, fake, d) = setup("media-play");
        key(&mut a, &cx, Key::Enter);
        let first = d.join("mine/music/a1.mp3").to_str().unwrap().to_string();
        assert_eq!(fake.launched.lock().unwrap().clone(), vec![first.clone()]);
        assert_eq!(a.player.now_playing().as_deref(), Some("Ann - Alpha"));
        let _ = draw(&mut a, &cx, 1280, 720);
        // the player ends by itself: the next queued file starts
        fake.procs.lock().unwrap()[0].lock().unwrap().0 = false;
        a.handle(&Event::Tick, &cx);
        assert_eq!(fake.launched.lock().unwrap().len(), 2);
        assert_eq!(a.player.now_playing().as_deref(), Some("Ann - Beta"));
        // next and previous
        key(&mut a, &cx, Key::Char('.'));
        assert_eq!(a.player.now_playing().as_deref(), Some("Bob - Gamma"));
        key(&mut a, &cx, Key::Char(','));
        assert_eq!(a.player.now_playing().as_deref(), Some("Ann - Beta"));
        // repeat one restarts the same file; repeat off ends the queue after the last
        key(&mut a, &cx, Key::Char('l'));
        key(&mut a, &cx, Key::Char('l'));
        assert_eq!(a.repeat, Repeat::One);
        let n = fake.procs.lock().unwrap().len();
        fake.procs.lock().unwrap()[n - 1].lock().unwrap().0 = false;
        a.handle(&Event::Tick, &cx);
        assert_eq!(a.player.now_playing().as_deref(), Some("Ann - Beta"));
        key(&mut a, &cx, Key::Char('l'));
        assert_eq!(a.repeat, Repeat::Off);
        // stop kills the launched process
        key(&mut a, &cx, Key::Char('s'));
        assert!(fake.procs.lock().unwrap().last().unwrap().lock().unwrap().1, "the player process was stopped");
        assert_eq!(a.player.now_playing(), None);
        let started = fake.launched.lock().unwrap().len();
        a.handle(&Event::Tick, &cx);
        assert_eq!(fake.launched.lock().unwrap().len(), started, "a stopped queue does not restart");
        // the flags are kept for the next visit
        key(&mut a, &cx, Key::Char('z'));
        let b = MediaApp::with_player(&cx, a.player.clone());
        assert!(b.shuffle && b.repeat == Repeat::Off);
        // the log names the files played
        assert!(std::fs::read_to_string(cx.audit.path()).unwrap().contains("media.play"));
    }

    #[test]
    fn shuffle_keeps_the_chosen_track_first_and_a_file_out_of_scope_is_refused() {
        let (mut a, cx, fake, d) = setup("media-shuffle");
        key(&mut a, &cx, Key::Char('z'));
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Enter);
        assert_eq!(fake.launched.lock().unwrap()[0], d.join("mine/music/a2.mp3").to_str().unwrap());
        assert_eq!(a.queue.len(), 3);
        // a playlist entry outside the read scope is not handed to the player
        let outside = d.join("other/secret.mp3").to_str().unwrap().to_string();
        a.store.create("Sneaky").unwrap();
        a.store.add_item("Sneaky", &outside).unwrap();
        a.handle(&Event::Tab(4), &cx);
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Enter);
        assert!(a.status_text().unwrap().contains("may not read"), "{:?}", a.status_text());
        assert_eq!(fake.launched.lock().unwrap().len(), 1);
    }

    #[test]
    fn playlists_are_made_filled_reordered_played_as_a_file_and_kept() {
        let (mut a, cx, fake, d) = setup("media-lists");
        // add the first track to a new playlist from the Tracks page
        key(&mut a, &cx, Key::Char('a'));
        assert!(matches!(a.mode, Mode::Pick(_)));
        key(&mut a, &cx, Key::Enter); // the only entry is "+ New playlist"
        assert!(matches!(a.mode, Mode::Form(..)));
        typ(&mut a, &cx, "Evening");
        key(&mut a, &cx, Key::Ctrl('s'));
        assert!(matches!(a.mode, Mode::Browse));
        assert_eq!(a.store.playlist("Evening").unwrap().items.len(), 1);
        // add an artist's tracks to the existing one
        a.handle(&Event::Tab(1), &cx);
        key(&mut a, &cx, Key::Char('a'));
        key(&mut a, &cx, Key::Enter); // "Evening" is the first entry
        assert_eq!(a.store.playlist("Evening").unwrap().items.len(), 3);
        // open it, move and remove entries
        a.handle(&Event::Tab(4), &cx);
        assert_eq!(a.row_names(), vec!["Evening"]);
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.row_names(), vec!["Alpha", "Alpha", "Beta"]);
        key(&mut a, &cx, Key::Char(']'));
        key(&mut a, &cx, Key::Char(']'));
        assert_eq!(a.row_names(), vec!["Alpha", "Beta", "Alpha"]);
        key(&mut a, &cx, Key::Char('x'));
        assert_eq!(a.row_names(), vec!["Alpha", "Beta"]);
        let _ = draw(&mut a, &cx, 1280, 720);
        // P hands the whole list as an M3U file under the data folder
        key(&mut a, &cx, Key::Char('P'));
        let launched = fake.launched.lock().unwrap().last().unwrap().clone();
        assert!(launched.starts_with(cx.services.dir().to_str().unwrap()) && launched.ends_with("Evening.m3u"), "{launched}");
        let m3u = std::fs::read_to_string(&launched).unwrap();
        assert!(m3u.starts_with("#EXTM3U") && m3u.contains(d.join("mine/music/a1.mp3").to_str().unwrap()) && m3u.contains("Ann - Beta"), "{m3u}");
        assert_eq!(a.player.now_playing().as_deref(), Some("Playlist Evening"));
        // rename, then delete after the question
        key(&mut a, &cx, Key::Esc);
        key(&mut a, &cx, Key::Char('r'));
        for _ in 0..7 {
            key(&mut a, &cx, Key::Backspace);
        }
        typ(&mut a, &cx, "Night");
        key(&mut a, &cx, Key::Ctrl('s'));
        assert_eq!(a.row_names(), vec!["Night"]);
        let _ = draw(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Char('d'));
        assert!(matches!(a.mode, Mode::DeletePlaylist(_)));
        let _ = draw(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Esc);
        assert_eq!(a.store.playlists().len(), 1, "Esc keeps it");
        key(&mut a, &cx, Key::Char('d'));
        key(&mut a, &cx, Key::Char('y'));
        assert!(a.store.playlists().is_empty());
        // a bad name keeps the form open
        key(&mut a, &cx, Key::Char('n'));
        key(&mut a, &cx, Key::Ctrl('s'));
        assert!(matches!(&a.mode, Mode::Form(f, _) if f.error.is_some()));
    }

    #[test]
    fn folders_can_be_added_and_removed_and_playlists_are_per_person() {
        let (mut a, cx, _f, d) = setup("media-folders");
        a.handle(&Event::Tab(5), &cx);
        assert_eq!(a.row_names().len(), 2);
        key(&mut a, &cx, Key::Char('n'));
        typ(&mut a, &cx, "relative/path");
        key(&mut a, &cx, Key::Ctrl('s'));
        assert!(matches!(&a.mode, Mode::Form(f, _) if f.error.is_some()));
        if let Mode::Form(f, _) = &mut a.mode {
            f.set_value(0, d.join("mine/music").to_str().unwrap());
        }
        key(&mut a, &cx, Key::Ctrl('s'));
        assert_eq!(a.row_names().len(), 3);
        let _ = draw(&mut a, &cx, 1280, 720);
        wait_scan(&mut a, &cx);
        assert_eq!(a.library().audio("").len(), 3, "a folder listed twice does not double its files");
        key(&mut a, &cx, Key::Char('d'));
        let _ = draw(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Char('y'));
        assert_eq!(a.row_names().len(), 2);
        // another account has its own lists
        a.store.create("Mine").unwrap();
        let other = MediaStore::new(cx.services.dir(), "someone");
        assert!(other.playlists().is_empty());
    }

    #[test]
    fn a_linked_host_folder_is_played_in_place_read_only_and_goes_away_and_comes_back() {
        let _g = iyakki_agent::hostfiles::test_guard();
        let (mut a, cx, _f, d) = setup("media-host");
        let host = std::fs::canonicalize(d.join("hosthome")).unwrap_or_else(|_| {
            std::fs::create_dir_all(d.join("hosthome")).unwrap();
            std::fs::canonicalize(d.join("hosthome")).unwrap()
        });
        let t1 = put(&host, "Songs/Rock/h1.mp3", &mp3("HostOne", "Hal", "Live"));
        put(&host, "Songs/Rock/h2.mp3", &mp3("HostTwo", "Hal", "Live"));
        let before: Vec<_> = std::fs::read_dir(host.join("Songs/Rock")).unwrap().flatten().map(|e| (e.file_name(), e.metadata().unwrap().len())).collect();
        iyakki_agent::hostfiles::set_enabled(true);
        iyakki_agent::hostfiles::set_host_home(Some(host.clone()));
        let before_n = a.library().audio("").len();
        assert!(!a.may_read(&cx, &t1), "not reachable before it is shared");
        let mut m = iyakki_agent::hostfiles::HostMounts::load(cx.services.dir(), &cx.session.user);
        m.add(&host, "~/Songs", Some("Songs"), false).unwrap();
        rescan(&mut a, &cx);
        assert_eq!(a.library().audio("").len(), before_n + 2, "the shared folder's songs are in the library");
        assert!(a.may_read(&cx, &t1), "playback resolves through the mount");
        // the Folders page says where it is from
        a.handle(&Event::Tab(5), &cx);
        let rows = a.rows();
        let r = rows.iter().find(|r| r.main == "host:Songs").expect("a row for the shared folder");
        assert_eq!(r.right, "host · read-only");
        assert!(r.sub.contains("shared from your computer"), "{}", r.sub);
        // d never deletes it, and nothing is written to the host
        let at = rows.iter().position(|r| r.main == "host:Songs").unwrap();
        a.set_sel(at);
        key(&mut a, &cx, Key::Char('d'));
        assert!(a.status_text().unwrap().contains("never deleted") && !matches!(a.mode, Mode::RemoveFolder(_)), "{:?}", a.status_text());
        let after: Vec<_> = std::fs::read_dir(host.join("Songs/Rock")).unwrap().flatten().map(|e| (e.file_name(), e.metadata().unwrap().len())).collect();
        assert_eq!(before.len(), after.len(), "the host folder is untouched");
        assert_eq!(std::fs::read_dir(host.join("Songs")).unwrap().count(), 1, "no index or tag files were written there");
        // unplugged: a plain state, the songs leave the library (nothing is cached from a folder that is not there)
        std::fs::rename(host.join("Songs"), host.join("Songs-away")).unwrap();
        rescan(&mut a, &cx);
        let rows = a.rows();
        let r = rows.iter().find(|r| r.main == "host:Songs").expect("still listed");
        assert_eq!(r.right, "not available");
        assert!(r.sub.contains("folder not available"), "{}", r.sub);
        assert_eq!(a.library().audio("").len(), before_n, "the library no longer lists them");
        // back: noticed by the periodic look, no key needed
        std::fs::rename(host.join("Songs-away"), host.join("Songs")).unwrap();
        for _ in 0..160 {
            a.handle(&Event::Tick, &cx);
        }
        wait_scan(&mut a, &cx);
        assert_eq!(a.library().audio("").len(), before_n + 2, "it returned and was looked at again");
        // c and u on a linked folder ask for its options (convert to a copy, stop sharing)
        a.handle(&Event::Tab(5), &cx);
        let at = a.rows().iter().position(|r| r.main == "host:Songs").unwrap();
        a.set_sel(at);
        key(&mut a, &cx, Key::Char('u'));
        assert_eq!(a.take_host_mount_menu().as_deref(), Some("Songs"));
        key(&mut a, &cx, Key::Char('c'));
        assert_eq!(a.take_host_mount_menu().as_deref(), Some("Songs"));
        // a drop goes into the first music folder the account may write to, and the library looks again afterwards
        match a.on_drop(&[], &cx) {
            crate::apps::DropAction::Into { label, .. } => assert_eq!(label, "your music folder"),
            crate::apps::DropAction::Uploads => {}
            other => panic!("{other:?}"),
        }
        a.dropped_done(&[], &cx);
        assert!(a.rescan);
        // stop sharing: gone from the library and not readable
        m.remove("Songs").unwrap();
        rescan(&mut a, &cx);
        assert_eq!(a.library().audio("").len(), before_n);
        assert!(!a.may_read(&cx, &t1));
        iyakki_agent::hostfiles::set_host_home(None);
        iyakki_agent::hostfiles::set_enabled(false);
    }


    #[test]
    fn the_tile_is_in_the_policy_for_every_role_that_has_apps() {
        use iyakki_sec::policy::{App, Role};
        assert!(App::ALL.contains(&App::Media) && App::Media.id() == "media" && App::Media.label() == "MEDIA");
        for r in [Role::Admin, Role::Operator, Role::User] {
            assert!(r.template().apps.contains(&App::Media), "{r:?}");
        }
    }

    #[test]
    fn mouse_clicks_choose_play_and_stop() {
        let (mut a, cx, fake, _d) = setup("media-mouse");
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Alpha") && g.hits.iter().any(|h| h.ev == Event::Tab(100)));
        a.handle(&Event::Row(2), &cx);
        assert_eq!(a.sel_now(), 2);
        a.handle(&Event::Row(2), &cx); // a second click on the chosen row plays it
        assert_eq!(fake.launched.lock().unwrap().len(), 1);
        a.handle(&Event::Tab(100), &cx);
        assert_eq!(a.player.now_playing(), None);
        a.handle(&Event::Tab(101), &cx);
        assert!(a.shuffle);
    }

    fn rescan(a: &mut MediaApp, cx: &AppCx) {
        key(a, cx, Key::Char('R'));
        wait_scan(a, cx);
    }

    fn cur(a: &MediaApp) -> Option<Item> {
        a.selected_path().and_then(|p| a.item(&p))
    }

    #[test]
    fn plain_words_search_the_index_chips_show_what_was_understood_and_digits_rate() {
        let (mut a, cx, _f, d) = setup("media-index");
        put(&d, "mine/music/kuthu/k1.mp3", &mp3("Fire", "Ann", "First"));
        put(&d, "mine/music/melody/m1.mp3", &mp3("Rain", "Bob", "Other"));
        rescan(&mut a, &cx);
        assert_eq!(a.row_names().len(), 5);
        key(&mut a, &cx, Key::Char('/'));
        typ(&mut a, &cx, "kutthu song");
        assert_eq!(a.row_names(), vec!["Fire"]);
        assert_eq!(a.chips(), vec!["type: kuthu".to_string()]);
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Fire"));
        key(&mut a, &cx, Key::Enter); // leave the search box, keep the filter
        // rating: digits when a track is chosen, 0 clears
        key(&mut a, &cx, Key::Char('5'));
        assert_eq!(cur(&a).unwrap().rating, 5);
        assert_eq!(a.status_text(), Some("Rated ★★★★★"));
        key(&mut a, &cx, Key::Char('0'));
        assert_eq!(cur(&a).unwrap().rating, 0);
        key(&mut a, &cx, Key::Char('4'));
        assert_eq!(a.rows()[0].rating, 4, "stars show in the row");
        // the person's rating survives a restart of the app and a rescan
        rescan(&mut a, &cx);
        assert_eq!(cur(&a).unwrap().rating, 4);
        let b = MediaApp::with_player(&cx, a.player.clone());
        assert_eq!(b.index.lock().unwrap().find_path(&d.join("mine/music/kuthu/k1.mp3").to_str().unwrap().to_string()).unwrap().rating, 4);
        // "melody songs above 4 stars" finds nothing yet; Top rated shows the 4-star one
        key(&mut a, &cx, Key::Esc);
        key(&mut a, &cx, Key::Char('/'));
        typ(&mut a, &cx, "melody above 4 stars");
        assert!(a.row_names().is_empty());
        key(&mut a, &cx, Key::Esc);
        a.handle(&Event::Tab(7), &cx);
        assert_eq!((a.page(), a.row_names()), (Page::TopRated, vec!["Fire".to_string()]));
        // a digit on a page without tracks jumps to that page
        a.handle(&Event::Tab(1), &cx);
        key(&mut a, &cx, Key::Char('1'));
        assert_eq!(a.page(), Page::Tracks);
        // sorting
        key(&mut a, &cx, Key::Char('o'));
        assert_eq!(a.sort, Sort::Latest);
        assert_eq!(a.row_names().len(), 5);
        key(&mut a, &cx, Key::Char('o'));
        assert_eq!(a.row_names()[0], "Fire", "top rated first");
        let _ = draw(&mut a, &cx, 1280, 720);
        let _ = draw(&mut a, &cx, 600, 500);
    }

    #[test]
    fn types_are_browsed_with_counts_and_edited_with_t() {
        let (mut a, cx, _f, d) = setup("media-types");
        put(&d, "mine/music/kuthu/k1.mp3", &mp3("Fire", "Ann", "First"));
        put(&d, "mine/music/kuthu/k2.mp3", &mp3("Smoke", "Bob", "Other"));
        put(&d, "mine/music/melody/m1.mp3", &mp3("Rain", "Bob", "Other"));
        rescan(&mut a, &cx);
        a.handle(&Event::Tab(6), &cx);
        assert_eq!(a.page(), Page::Types);
        assert_eq!(a.row_names(), vec!["kuthu", "melody"]);
        assert_eq!(a.rows()[0].right, "2 items");
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.row_names().len(), 2);
        let _ = draw(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Esc);
        assert!(a.drill.is_none());
        // t on a track: its types and tags in a form
        a.handle(&Event::Tab(0), &cx);
        key(&mut a, &cx, Key::Char('/'));
        typ(&mut a, &cx, "rain");
        key(&mut a, &cx, Key::Enter);
        assert_eq!(a.row_names(), vec!["Rain"]);
        key(&mut a, &cx, Key::Char('t'));
        assert!(matches!(a.mode, Mode::Tag(..)));
        let _ = draw(&mut a, &cx, 1280, 720);
        if let Mode::Tag(f, _) = &mut a.mode {
            f.set_value(0, "melody, nonsense");
        }
        key(&mut a, &cx, Key::Ctrl('s'));
        // a word the vocabulary does not know is accepted (free text), kept as written
        assert!(matches!(a.mode, Mode::Browse), "{:?}", a.mode_error());
        assert_eq!(cur(&a).unwrap().effective_types().iter().cloned().collect::<Vec<_>>(), vec!["melody".to_string(), "nonsense".to_string()]);
        key(&mut a, &cx, Key::Char('t'));
        if let Mode::Tag(f, _) = &mut a.mode {
            f.set_value(0, "beats, melody");
            f.set_value(1, "rainy day");
        }
        key(&mut a, &cx, Key::Ctrl('s'));
        assert!(matches!(a.mode, Mode::Browse));
        let it = cur(&a).unwrap();
        assert_eq!((it.effective_types().iter().cloned().collect::<Vec<_>>(), it.tags.iter().cloned().collect::<Vec<_>>()), (vec!["beats".to_string(), "melody".to_string()], vec!["rainy day".to_string()]));
        assert!(std::fs::read_to_string(cx.audit.path()).unwrap().contains("media.tag"));
        key(&mut a, &cx, Key::Esc); // forget the search
        a.handle(&Event::Tab(6), &cx);
        let names = a.row_names();
        assert!(["kuthu", "melody", "beats", "rainy day"].iter().all(|n| names.contains(&n.to_string())), "{names:?}");
    }

    #[test]
    fn the_assistant_and_the_app_share_one_player_and_one_queue() {
        let (mut a, cx, fake, d) = setup("media-shared");
        let (p1, p2) = (d.join("mine/music/a1.mp3").to_str().unwrap().to_string(), d.join("mine/music/a2.mp3").to_str().unwrap().to_string());
        // the chat's tool started something: the app shows it and takes over the queue
        a.player.play("root", "Ann - Beta", &p2).unwrap();
        a.player.set_queue("root", vec![p1.clone(), p2.clone()], 1, true);
        a.handle(&Event::Tick, &cx);
        assert_eq!((a.queue.clone(), a.qpos, a.queue_active), (vec![p1.clone(), p2.clone()], 1, true));
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.ev == Event::Tab(100)), "Stop is there");
        // the app's own start is published for the tools
        key(&mut a, &cx, Key::Enter);
        let st = a.player.queue_state();
        assert!(st.active && st.user == "root" && st.items.len() == 3 && st.pos == 0, "{st:?}");
        assert_eq!(fake.launched.lock().unwrap().len(), 2, "the tool's start and the app's own");
        // somebody else's queue is not taken over
        a.player.set_queue("someone", vec![p2.clone()], 0, true);
        a.handle(&Event::Tick, &cx);
        assert_eq!(a.queue.len(), 3);
        key(&mut a, &cx, Key::Char('s'));
        assert!(!a.player.queue_state().active);
    }

    /// A tiny HTTP server for the download tests: path -> (content type, body), each answer after `delay`.
    fn serve_slow(delay: std::time::Duration, routes: Vec<(&'static str, &'static str, Vec<u8>)>) -> String {
        use std::io::{Read, Write};
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        std::thread::spawn(move || {
            for s in l.incoming().flatten() {
                let routes = routes.clone();
                std::thread::spawn(move || {
                    let mut s = s;
                    let mut buf = [0u8; 4096];
                    let n = s.read(&mut buf).unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                    std::thread::sleep(delay);
                    let out = match routes.iter().find(|r| r.0 == path) {
                        Some((_, ct, body)) => [format!("HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes(), body.clone()].concat(),
                        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
                    };
                    let _ = s.write_all(&out);
                });
            }
        });
        format!("http://{addr}")
    }

    fn serve(routes: Vec<(&'static str, &'static str, Vec<u8>)>) -> String {
        serve_slow(std::time::Duration::ZERO, routes)
    }

    /// Run heartbeats until the queue has nothing running or waiting and the library scan is done.
    fn wait_downloads(a: &mut MediaApp, cx: &AppCx) {
        for _ in 0..1500 {
            a.handle(&Event::Tick, cx);
            if a.dlq.counts() == (0, 0) && a.seen_finished == a.dlq.last_finished().0 && !a.is_scanning() && !a.rescan {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the downloads did not finish: {:?}", a.dlq.views().iter().map(|v| (v.id, v.status, v.error.clone())).collect::<Vec<_>>());
    }

    /// An app whose library folders are Music, Videos and Pictures under one temp folder the account may read and write, with
    /// a real streamer and the loopback host allowed.
    fn dsetup(tag: &str) -> (MediaApp, AppCx, Arc<Fake>, std::path::PathBuf) {
        let mut cx = cx_with_admin_store(tag);
        let d = temp_dir(&format!("{tag}-lib"));
        for sub in ["Music", "Videos", "Pictures"] {
            std::fs::create_dir_all(d.join(sub)).unwrap();
        }
        let lib = d.to_str().unwrap().to_string();
        cx.session.policy.fs_read = vec![lib.clone()];
        cx.session.policy.fs_write = vec![lib];
        cx.session.policy.net_hosts = vec!["127.0.0.1".into()];
        cx.services.set_streamer(Arc::new(iyakki_agent::iyakki_proxy::https::UreqStreamer::default()));
        let store = MediaStore::new(cx.services.dir(), &cx.session.user);
        for sub in ["Music", "Videos", "Pictures"] {
            store.add_folder(d.join(sub).to_str().unwrap()).unwrap();
        }
        store.remove_folder(&format!("/home/{}/Music", cx.session.user)).unwrap();
        store.remove_folder(&format!("/home/{}/Videos", cx.session.user)).unwrap();
        let fake = Arc::new(Fake::default());
        let mut app = MediaApp::with_player(&cx, Arc::new(MediaPlayer::new(fake.clone())));
        app.set_samples_dir(None, &cx);
        wait_scan(&mut app, &cx);
        (app, cx, fake, d)
    }

    impl MediaApp {
        fn mode_error(&self) -> Option<String> {
            match &self.mode {
                Mode::Tag(f, _) | Mode::Download(f, _) => f.error.clone(),
                _ => None,
            }
        }
    }

    fn form_of(a: &mut MediaApp) -> &mut Form {
        match &mut a.mode {
            Mode::Download(f, _) => f,
            _ => panic!("the download form is not open"),
        }
    }

    const WEBM: &[u8] = b"\x1a\x45\xdf\xa3\x01\x00\x00\x00\x00\x00\x00\x1fB\x86\x81\x01B\xf7\x81\x01B\xf2\x81\x04B\xf3\x81\x08B\x82\x84webm";

    #[test]
    fn d_adds_addresses_to_the_queue_with_free_text_categories_and_the_files_land_in_the_library() {
        let (mut a, cx, _f, d) = dsetup("media-dl");
        let base = serve(vec![("/files/Remote%20Song.mp3", "audio/mpeg", mp3("Remote Song", "Net", "Web")), ("/page.mp3", "text/html", b"<html>hi</html>".to_vec()), ("/clip.webm", "video/webm", WEBM.to_vec())]);
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Download (D)"));
        key(&mut a, &cx, Key::Char('D'));
        assert!(matches!(a.mode, Mode::Download(..)));
        let _ = draw(&mut a, &cx, 1280, 720);
        // nothing typed, a bad rating, an address that is not one: the form stays open and says why
        key(&mut a, &cx, Key::Ctrl('s'));
        assert!(form_of(&mut a).error.as_deref().is_some_and(|e| e.contains("address")));
        form_of(&mut a).set_value(0, "ftp://x/y.mp3");
        key(&mut a, &cx, Key::Ctrl('s'));
        assert!(form_of(&mut a).error.as_deref().is_some_and(|e| e.contains("web address")), "{:?}", form_of(&mut a).error);
        // a category the vocabulary does not know is accepted: free text
        {
            let f = form_of(&mut a);
            f.set_value(0, &format!("{base}/files/Remote%20Song.mp3 {base}/clip.webm {base}/page.mp3"));
            f.set_value(1, "nonsense, kutthu, action");
            f.set_value(2, "gym mix");
            f.set_value(3, "9");
        }
        key(&mut a, &cx, Key::Ctrl('s'));
        assert!(form_of(&mut a).error.as_deref().is_some_and(|e| e.contains("0 to 5")));
        form_of(&mut a).set_value(3, "4");
        key(&mut a, &cx, Key::Ctrl('s'));
        assert!(matches!(a.mode, Mode::Browse), "{:?}", a.status_text());
        assert_eq!(a.page(), Page::Downloads, "the queue is shown");
        assert!(a.status_text().unwrap().contains("Added 3 addresses"), "{:?}", a.status_text());
        // the three are in the list at once (two run, one waits), then everything ends
        assert_eq!(a.row_names().len(), 3);
        let _ = draw(&mut a, &cx, 1280, 720);
        wait_downloads(&mut a, &cx);
        let rows = a.rows();
        let line = |name: &str| rows.iter().find(|r| r.main == name).map(|r| r.sub.clone()).unwrap_or_else(|| panic!("{name} in {:?}", rows.iter().map(|r| (r.main.clone(), r.sub.clone())).collect::<Vec<_>>()));
        assert!(line("Remote Song.mp3").starts_with("Succeeded · Remote Song.mp3 · Music · "), "{}", line("Remote Song.mp3"));
        assert!(line("clip.webm").starts_with("Succeeded · clip.webm · Videos · "), "{}", line("clip.webm"));
        assert!(line("page.mp3").starts_with("Failed · Not a media file") && line("page.mp3").contains("web page"), "{}", line("page.mp3"));
        // the files went to the folder of their kind, and into the index with what was typed
        let song = d.join("Music/Remote Song.mp3");
        assert!(song.is_file() && d.join("Videos/clip.webm").is_file());
        let it = a.item(song.to_str().unwrap()).unwrap();
        assert_eq!(it.rating, 4);
        assert!(it.effective_types().contains("kuthu") && it.effective_types().contains("action") && it.tags.contains("nonsense") && it.tags.contains("gym mix"), "{:?} {:?}", it.effective_types(), it.tags);
        assert!(it.source_url.ends_with("/files/Remote%20Song.mp3"));
        assert!(a.library().audio("").iter().any(|t| t.title == "Remote Song"), "the library was scanned again: {:?} {:?}", a.library().tracks.iter().map(|t| t.path.clone()).collect::<Vec<_>>(), a.status_text());
        assert!(std::fs::read_to_string(cx.audit.path()).unwrap().contains("media.download"));
        let inbox = cx.services.inbox.list("root");
        assert!(inbox.iter().any(|n| n.kind == iyakki_agent::inbox::Kind::Download) && inbox.iter().any(|n| n.kind == iyakki_agent::inbox::Kind::Error && n.title.contains("Download failed")));
        // the same address again never overwrites
        key(&mut a, &cx, Key::Char('D'));
        form_of(&mut a).set_value(0, &format!("{base}/files/Remote%20Song.mp3"));
        key(&mut a, &cx, Key::Ctrl('s'));
        wait_downloads(&mut a, &cx);
        assert!(d.join("Music/Remote Song (2).mp3").is_file());
        // an account that may not write anywhere is told so
        let mut cx2 = cx.clone();
        cx2.session.policy.fs_write.clear();
        key(&mut a, &cx2, Key::Char('D'));
        assert!(matches!(a.mode, Mode::Browse) && a.status_text().unwrap().contains("may be written"), "{:?}", a.status_text());
    }

    #[test]
    fn example_chips_fill_the_category_field_and_follow_the_kind_of_file() {
        let (mut a, cx, _f, _d) = dsetup("media-chips");
        key(&mut a, &cx, Key::Char('D'));
        // nothing known about the file yet: every row of examples, with the folder decided later
        let rows = form_of(&mut a).fields[1].chips.clone();
        assert_eq!(rows.iter().map(|r| r.0.as_str()).collect::<Vec<_>>(), vec!["Videos", "Songs", "Photos"]);
        assert_eq!(rows[0].1, VIDEO_CHIPS.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(rows[1].1.len() + rows[0].1.len() + rows[2].1.len(), 22);
        assert!(form_of(&mut a).fields[4].label.starts_with("Folder: by file type"), "{}", form_of(&mut a).fields[4].label);
        let _ = draw(&mut a, &cx, 1280, 720);
        // a click on a chip puts its word in the field; a second click takes it out
        a.handle(&Event::Tab(iyakki_shell_chip(1, 0)), &cx);
        assert_eq!(form_of(&mut a).value(1), "action");
        a.handle(&Event::Tab(iyakki_shell_chip(1, 2)), &cx);
        assert_eq!(form_of(&mut a).value(1), "action, cartoon");
        a.handle(&Event::Tab(iyakki_shell_chip(1, 0)), &cx);
        assert_eq!(form_of(&mut a).value(1), "cartoon");
        // the person's own words stay when a chip is used
        form_of(&mut a).set_value(1, "my road trip");
        a.handle(&Event::Tab(iyakki_shell_chip(1, 10)), &cx); // the first song example
        assert_eq!(form_of(&mut a).value(1), "my road trip, melody");
        // the address names a video: only the video examples, and the folder line says where it goes
        form_of(&mut a).set_value(0, "http://127.0.0.1:1/clip.mp4");
        a.handle(&Event::FocusAt(1, 3.0), &cx);
        assert_eq!(form_of(&mut a).focus, 1);
        let rows = form_of(&mut a).fields[1].chips.clone();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "Videos");
        assert_eq!(form_of(&mut a).fields[4].label.split("    ").next().unwrap(), "Folder: Videos (auto)");
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "horror") && !g.hits.iter().any(|h| h.label == "melody"), "chips drawn and clickable");
        // keyboard: Ctrl+Right moves along the chips, Ctrl+E picks the one the highlight is on
        form_of(&mut a).set_value(1, "");
        for _ in 0..3 {
            a.handle(&Event::Key(Key::Nav { nav: crate::ui::Nav::Right, shift: false, ctrl: true }), &cx);
        }
        key(&mut a, &cx, Key::Ctrl('e'));
        assert_eq!(form_of(&mut a).value(1), "comedy");
        // an address that is a picture, then a song
        form_of(&mut a).set_value(0, "http://127.0.0.1:1/p.png");
        a.handle(&Event::FocusAt(1, 3.0), &cx);
        assert_eq!(form_of(&mut a).fields[1].chips[0].0, "Photos");
        assert_eq!(form_of(&mut a).fields[1].chips[0].1, vec!["family", "travel", "nature"]);
        assert!(form_of(&mut a).fields[4].label.starts_with("Folder: Pictures (auto)"));
        form_of(&mut a).set_value(0, "http://127.0.0.1:1/s.mp3");
        a.handle(&Event::FocusAt(1, 3.0), &cx);
        assert_eq!(form_of(&mut a).fields[1].chips[0].0, "Songs");
        assert!(form_of(&mut a).fields[4].label.starts_with("Folder: Music (auto)"));
        // a folder chosen by hand replaces the automatic one and the line says so
        key(&mut a, &cx, Key::Tab);
        key(&mut a, &cx, Key::Tab);
        key(&mut a, &cx, Key::Tab);
        assert_eq!(form_of(&mut a).focus, 4);
        key(&mut a, &cx, Key::Right);
        let label = form_of(&mut a).fields[4].label.clone();
        assert!(label.starts_with("Folder: ") && !label.contains("(auto)"), "{label}");
    }

    fn iyakki_shell_chip(field: usize, n: usize) -> usize {
        super::super::widgets::CHIP_BASE + field * 100 + n
    }

    #[test]
    fn the_download_form_lays_out_at_small_sizes_with_its_buttons_and_chips_inside_the_window() {
        let (mut a, cx, _f, _d) = dsetup("media-small");
        key(&mut a, &cx, Key::Char('D'));
        for (w, h) in [(1920u32, 1080u32), (1280, 720), (900, 600), (640, 400), (480, 320)] {
            for kind in [None, Some("http://127.0.0.1:1/a.mp4")] {
                form_of(&mut a).set_value(0, kind.unwrap_or(""));
                a.handle(&Event::FocusAt(1, 3.0), &cx);
                let g = draw(&mut a, &cx, w, h);
                let area = R::new(0.0, 72.0, w as f32, h as f32 - 72.0);
                let inside = |r: &R| r.x >= area.x - 1.0 && r.y >= area.y - 1.0 && r.x + r.w <= area.x + area.w + 1.0 && r.y + r.h <= area.y + area.h + 1.0;
                let save = g.hits.iter().find(|x| x.label == "Save").unwrap_or_else(|| panic!("Save at {w}x{h}"));
                assert!(inside(&save.rect), "Save inside the window at {w}x{h}: {:?}", save.rect);
                let chips: Vec<&crate::ui::Hit> = g.hits.iter().filter(|x| matches!(x.ev, Event::Tab(n) if n >= 4000)).collect();
                assert!(!chips.is_empty());
                for c in &chips {
                    assert!(inside(&c.rect), "chip {} inside the window at {w}x{h}: {:?}", c.label, c.rect);
                }
                // nothing drawn over the buttons: the chips end above them
                assert!(chips.iter().all(|c| c.rect.y + c.rect.h <= save.rect.y + 1.0 || c.rect.x + c.rect.w < save.rect.x - 1.0 || w < 700), "chips stay clear of the buttons at {w}x{h}");
            }
        }
        // a click on a drawn chip fills the field
        let g = draw(&mut a, &cx, 1280, 720);
        let chip = g.hits.iter().find(|h| h.label == "thriller").unwrap();
        let ev = chip.ev.clone();
        a.handle(&ev, &cx);
        assert_eq!(form_of(&mut a).value(1), "thriller");
    }

    #[test]
    fn the_downloads_page_lists_queue_runs_two_at_a_time_and_keys_cancel_retry_remove_and_clear() {
        let (mut a, mut cx, fake, d) = dsetup("media-queue");
        let mp3b = mp3("Slow One", "Net", "Web");
        let base = serve_slow(std::time::Duration::from_millis(2500), vec![("/a.mp3", "audio/mpeg", mp3b.clone()), ("/b.mp3", "audio/mpeg", mp3b.clone()), ("/c.mp3", "audio/mpeg", mp3b.clone()), ("/d.mp3", "audio/mpeg", mp3b)]);
        let fast = serve(vec![("/ok.mp3", "audio/mpeg", mp3("Fast One", "Net", "Web"))]);
        cx.session.policy.net_hosts = vec!["127.0.0.1".into()];
        // the page and the chip start empty
        a.handle(&Event::Tab(DOWNLOADS_PAGE), &cx);
        assert_eq!((a.page(), a.row_names().len(), a.downloads_chip()), (Page::Downloads, 0, None));
        let g = draw(&mut a, &cx, 1280, 720);
        assert_eq!(g.hits.iter().filter(|h| h.label == "Downloads").count(), 1, "only the page pill while the queue is idle");
        key(&mut a, &cx, Key::Char('D'));
        form_of(&mut a).set_value(0, &format!("{base}/a.mp3 {base}/b.mp3 {base}/c.mp3 {base}/d.mp3"));
        key(&mut a, &cx, Key::Ctrl('s'));
        // two run, two wait: the header chip says so, in the words of the request
        assert_eq!(a.downloads_chip(), Some(("Downloads: 2 running, 2 queued".to_string(), true)));
        let rows = a.rows();
        assert_eq!(rows.iter().map(|r| r.dl.as_ref().unwrap().status).collect::<Vec<_>>(), vec![Status::Downloading, Status::Downloading, Status::Queued, Status::Queued]);
        assert!(rows[0].sub.starts_with("Downloading") && rows[0].sub.contains("Folder: Music (auto)"), "{}", rows[0].sub);
        assert!(rows[2].sub.starts_with("Queued"));
        let g = draw(&mut a, &cx, 1280, 720);
        assert_eq!(g.hits.iter().filter(|h| h.label == "Downloads" && h.ev == Event::Tab(DOWNLOADS_PAGE)).count(), 2, "the pill and the chip, both lead to the page");
        assert!(g.hits.iter().filter(|h| h.label == "Cancel").count() >= 2);
        // x cancels the chosen one: the third (waiting) one
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Down);
        key(&mut a, &cx, Key::Char('x'));
        // it moves below the running and waiting ones
        assert_eq!(a.rows().iter().map(|r| r.dl.as_ref().unwrap().status).collect::<Vec<_>>(), vec![Status::Downloading, Status::Downloading, Status::Queued, Status::Cancelled]);
        // r puts it back in line; d refuses a running one; c clears what ended
        key(&mut a, &cx, Key::End);
        key(&mut a, &cx, Key::Char('r'));
        assert!(a.status_text().unwrap().contains("back in the queue"), "{:?}", a.status_text());
        key(&mut a, &cx, Key::Home);
        key(&mut a, &cx, Key::Char('d'));
        assert!(a.status_text().unwrap().contains("cancel it first"), "{:?}", a.status_text());
        wait_downloads(&mut a, &cx);
        assert!(a.rows().iter().all(|r| r.dl.as_ref().unwrap().status == Status::Succeeded), "{:?}", a.rows().iter().map(|r| r.sub.clone()).collect::<Vec<_>>());
        for n in ["a", "b", "c", "d"] {
            assert!(d.join(format!("Music/{n}.mp3")).is_file(), "{n}");
        }
        // the chip flashed the last result; the clock of the flash runs out
        assert!(a.downloads_chip().is_some_and(|(t, ok)| ok && t.starts_with("Download succeeded: ")), "{:?}", a.downloads_chip());
        for _ in 0..FLASH_TICKS + 2 {
            a.handle(&Event::Tick, &cx);
        }
        assert_eq!(a.downloads_chip(), None);
        // Enter plays a finished one through the player; f shows it in the library; the buttons are drawn and clickable
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Play") && g.hits.iter().any(|h| h.label == "Show in library"));
        key(&mut a, &cx, Key::Home);
        key(&mut a, &cx, Key::Enter);
        assert_eq!(fake.launched.lock().unwrap().len(), 1);
        assert!(fake.launched.lock().unwrap()[0].starts_with(d.join("Music").to_str().unwrap()));
        key(&mut a, &cx, Key::Char('f'));
        assert_eq!(a.page(), Page::Tracks);
        assert_eq!(a.row_names(), vec!["Slow One"; 4], "the search shows the downloaded files");
        assert_eq!(a.query.value, "Slow One");
        a.handle(&Event::Tab(DOWNLOADS_PAGE), &cx);
        // a failure: the row says why in plain words, Enter repeats it, r retries (and fails the same way)
        key(&mut a, &cx, Key::Char('c'));
        assert!(a.rows().is_empty());
        key(&mut a, &cx, Key::Char('D'));
        form_of(&mut a).set_value(0, &format!("{fast}/missing.mp3"));
        key(&mut a, &cx, Key::Ctrl('s'));
        wait_downloads(&mut a, &cx);
        let row = a.rows().remove(0);
        assert!(row.sub.starts_with("Failed · The server refused or failed") && row.sub.contains("404"), "{}", row.sub);
        assert!(a.downloads_chip().is_some_and(|(t, ok)| !ok && t.starts_with("Download failed")));
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Retry"));
        key(&mut a, &cx, Key::Enter);
        assert!(a.status_text().unwrap().contains("press r to retry"), "{:?}", a.status_text());
        // d removes it from the list; the digit 0 goes to the page from anywhere
        key(&mut a, &cx, Key::Char('d'));
        assert!(a.rows().is_empty());
        a.handle(&Event::Tab(1), &cx);
        key(&mut a, &cx, Key::Char('0'));
        assert_eq!(a.page(), Page::Downloads);
        // the older pages kept their places
        assert_eq!(PAGES.iter().take(9).map(|p| p.1).collect::<Vec<_>>(), vec!["Tracks", "Artists", "Albums", "Videos", "Playlists", "Folders", "Types", "Top rated", "Photos"]);
        let _ = draw(&mut a, &cx, 600, 500);
    }

    #[test]
    fn the_queue_is_kept_across_a_restart_and_unfinished_items_come_back_as_interrupted() {
        let (mut a, cx, _f, _d) = dsetup("media-restart");
        let big = serve_slow(std::time::Duration::from_millis(1500), vec![("/slow.mp3", "audio/mpeg", mp3("Slow", "Net", "Web")), ("/two.mp3", "audio/mpeg", mp3("Two", "Net", "Web"))]);
        let fast = serve(vec![("/ok.mp3", "audio/mpeg", mp3("Fast One", "Net", "Web"))]);
        key(&mut a, &cx, Key::Char('D'));
        form_of(&mut a).set_value(0, &format!("{fast}/ok.mp3"));
        key(&mut a, &cx, Key::Ctrl('s'));
        wait_downloads(&mut a, &cx);
        key(&mut a, &cx, Key::Char('D'));
        form_of(&mut a).set_value(0, &format!("{big}/slow.mp3"));
        key(&mut a, &cx, Key::Ctrl('s'));
        assert_eq!(a.dlq.counts(), (1, 0));
        // "restart": the saved file is read by a new queue, as a new program run would
        let again = iyakki_agent::mediaqueue::DownloadQueue::open(cx.services.dir(), "root");
        let v = again.views();
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].status, v[0].name.as_str()), (Status::Succeeded, "ok.mp3"));
        assert_eq!((v[1].status, v[1].error.as_str()), (Status::Failed, mediaqueue::INTERRUPTED));
        assert!(status_line(&v[1]).starts_with("Failed · Interrupted") && status_line(&v[1]).contains("Retry"));
        wait_downloads(&mut a, &cx);
    }

    #[test]
    fn status_words_sizes_and_names_are_plain() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1536), "1.5 KB");
        assert_eq!(human_bytes(5 << 20), "5.0 MB");
        assert_eq!(human_secs(26), "26 s");
        assert_eq!(human_secs(185), "3 min 05 s");
        assert_eq!(url_label("https://x.test/a/My%20Film.mp4?token=1"), "My Film.mp4");
        assert_eq!(url_label("https://x.test/"), "x.test");
        let v = View { id: 3, url: "https://x.test/f.webm".into(), status: Status::Downloading, kind: Some(ItemKind::Video), folder: "/home/a/Videos".into(), auto: true, file: String::new(), name: String::new(), size: 0, bytes: 2 << 20, total: 10 << 20, speed: 1 << 20, eta_secs: Some(8), error: String::new(), note: String::new(), categories: vec![], tags: vec![] };
        assert_eq!(status_line(&v), "Downloading 2.0 MB of 10.0 MB (20%) · 1.0 MB/s · 8 s left · Folder: Videos (auto)");
        let done = View { status: Status::Succeeded, name: "f.webm".into(), size: 10 << 20, ..v.clone() };
        assert_eq!(status_line(&done), "Succeeded · f.webm · Videos · 10.0 MB");
        let failed = View { status: Status::Failed, error: "Too large: the file is 900 MB, more than the 512 MB limit.".into(), ..v.clone() };
        assert!(status_line(&failed).starts_with("Failed · Too large"));
        assert_eq!(status_line(&View { status: Status::Cancelled, ..v.clone() }), "Cancelled");
        assert!(status_line(&View { status: Status::Queued, ..v }).starts_with("Queued"));
        assert_eq!(chip_rows(Some(ItemKind::Video))[0].1.len(), 10);
        assert_eq!(chip_rows(Some(ItemKind::Song))[0].1.len(), 9);
        assert_eq!(chip_rows(Some(ItemKind::Image))[0].1, vec!["family", "travel", "nature"]);
        assert_eq!(chip_rows(None).len(), 3);
    }

    /// The samples as they ship, copied to a temp folder that stands in for /usr/share/iyakki/samples.
    fn sample_dir(tag: &str) -> std::path::PathBuf {
        let d = temp_dir(&format!("{tag}-samples"));
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../dist/samples");
        for rel in ["Music/IYAKKI Sample Melody.mp3", "Music/IYAKKI Sample Beats.mp3", "Videos/IYAKKI Sample Video.webm"] {
            std::fs::create_dir_all(d.join(rel).parent().unwrap()).unwrap();
            std::fs::copy(src.join(rel), d.join(rel)).unwrap();
        }
        std::fs::canonicalize(d).unwrap()
    }

    #[test]
    fn the_samples_show_in_a_fresh_library_as_a_read_only_folder_and_kutthu_finds_the_beats() {
        let (mut a, cx, fake, _d) = dsetup("media-samples");
        // an account whose read scope does not include the samples still sees them
        let sd = sample_dir("media-samples");
        assert!(!cx.session.policy.allows_read("root", sd.to_str().unwrap()));
        a.set_samples_dir(Some(sd.clone()), &cx);
        wait_scan(&mut a, &cx);
        assert_eq!(a.row_names(), vec!["IYAKKI Sample Beats", "IYAKKI Sample Melody"]);
        a.handle(&Event::Tab(3), &cx);
        assert_eq!(a.row_names(), vec!["IYAKKI Sample Video"]);
        a.handle(&Event::Tab(0), &cx);
        let it = a.item(sd.join("Music/IYAKKI Sample Melody.mp3").to_str().unwrap()).unwrap();
        assert_eq!((it.artist.as_str(), it.album.as_str()), ("IYAKKI", "IYAKKI Samples"));
        assert!(it.tags.contains("sample") && it.effective_types().contains("melody"), "{:?} {:?}", it.tags, it.effective_types());
        // "kutthu song" finds the beats sample
        key(&mut a, &cx, Key::Char('/'));
        typ(&mut a, &cx, "kutthu song");
        assert_eq!(a.row_names(), vec!["IYAKKI Sample Beats"]);
        key(&mut a, &cx, Key::Enter);
        key(&mut a, &cx, Key::Enter);
        assert_eq!(fake.launched.lock().unwrap().clone(), vec![sd.join("Music/IYAKKI Sample Beats.mp3").to_str().unwrap().to_string()], "played although the account's scope does not name the folder");
        key(&mut a, &cx, Key::Esc);
        // the Folders page lists it as built in and read-only; d refuses to remove it; it is never a place to download to
        a.handle(&Event::Tab(5), &cx);
        let rows = a.rows();
        let last = rows.last().unwrap();
        assert_eq!((last.main.as_str(), last.right.as_str()), ("IYAKKI samples", "read-only"));
        assert!(last.sub.contains("built in, read-only") && last.types == vec!["sample"]);
        key(&mut a, &cx, Key::End);
        key(&mut a, &cx, Key::Char('d'));
        assert!(matches!(a.mode, Mode::Browse) && a.status_text().unwrap().contains("cannot be removed"), "{:?}", a.status_text());
        let _ = draw(&mut a, &cx, 1280, 720);
        key(&mut a, &cx, Key::Enter);
        assert!(a.status_text().unwrap().contains("read-only"));
        assert!(!a.download_folders(&cx).iter().any(|f| f.starts_with(sd.to_str().unwrap())));
        assert_eq!(a.store.folders().len(), 3, "the person's own list is untouched");
        // the sample tag is searchable, and without a sample folder nothing is listed
        a.handle(&Event::Tab(0), &cx);
        key(&mut a, &cx, Key::Char('/'));
        typ(&mut a, &cx, "sample");
        assert_eq!(a.row_names().len(), 3 - 1 + 0, "{:?}", a.row_names());
        a.set_samples_dir(None, &cx);
        wait_scan(&mut a, &cx);
        key(&mut a, &cx, Key::Esc);
        assert!(a.row_names().is_empty());
    }

    /// The fake web engine helper of `iyakki-webview` (built on demand), as a script that starts it.
    fn fake_helper_script(tag: &str) -> std::path::PathBuf {
        let exe = std::env::current_exe().unwrap();
        let dir = exe.parent().unwrap().parent().unwrap().to_path_buf();
        let bin = dir.join("iyakki-fake-webview");
        if !bin.is_file() {
            let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
            let ok = std::process::Command::new(cargo).args(["build", "-q", "-p", "iyakki-webview", "--bin", "iyakki-fake-webview"]).status().map(|s| s.success()).unwrap_or(false);
            assert!(ok && bin.is_file(), "could not build the fake helper");
        }
        let script = temp_dir(&format!("{tag}-helper")).join("helper.sh");
        std::fs::write(&script, format!("#!/bin/sh\nexec {} --static \"$@\"\n", bin.display())).unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        script
    }

    fn pump_video(a: &mut MediaApp, cx: &AppCx, what: &str, done: impl Fn(&MediaApp) -> bool) {
        for _ in 0..2000 {
            a.handle(&Event::Tick, cx);
            if done(a) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let log = a.video.as_ref().map(|v| v.full.log.iter().cloned().collect::<Vec<_>>()).unwrap_or_default();
        panic!("timed out waiting for {what}; status {:?}, log {log:?}", a.status_text());
    }

    #[test]
    fn a_video_plays_in_the_embedded_engine_from_a_loopback_server_and_space_and_esc_work() {
        use std::io::{Read, Write};
        let (mut a, cx, fake, _d) = dsetup("media-video");
        let sd = sample_dir("media-video");
        a.set_samples_dir(Some(sd.clone()), &cx);
        wait_scan(&mut a, &cx);
        let helper = fake_helper_script("media-video");
        a.set_video_route(Some(VideoRoute::Embedded(helper)));
        a.handle(&Event::Tab(3), &cx);
        assert_eq!(a.row_names(), vec!["IYAKKI Sample Video"]);
        key(&mut a, &cx, Key::Enter);
        assert!(a.video_playing(), "{:?}", a.status_text());
        assert!(fake.launched.lock().unwrap().is_empty(), "no external player was started");
        pump_video(&mut a, &cx, "the engine to show the player page", |a| a.video.as_ref().is_some_and(|v| v.full.pixmap().is_some() && v.full.url.starts_with("http://127.0.0.1:")));
        let url = a.video.as_ref().unwrap().full.url.clone();
        // the address is loopback with a long random token; the helper is told that host and no other
        let token = url.trim_end_matches('/').rsplit('/').next().unwrap().to_string();
        assert_eq!(token.len(), 32, "{url}");
        let log = a.video.as_ref().unwrap().full.log.iter().cloned().collect::<Vec<_>>().join("\n");
        assert!(log.contains("Navigate") && log.contains("127.0.0.1"), "{log}");
        // the server behind it gives that one file, the whole of it
        let port: u16 = url.trim_start_matches("http://127.0.0.1:").split('/').next().unwrap().parse().unwrap();
        let get = |path: &str| {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n").as_bytes()).unwrap();
            let mut out = vec![];
            let _ = s.read_to_end(&mut out);
            out
        };
        let file = std::fs::read(sd.join("Videos/IYAKKI Sample Video.webm")).unwrap();
        let reply = get(&format!("/{token}/media"));
        assert!(reply.starts_with(b"HTTP/1.1 200") && reply.ends_with(&file[file.len() - 100..]) && reply.len() > file.len(), "the file was served");
        assert!(get("/media").starts_with(b"HTTP/1.1 404") && get("/").starts_with(b"HTTP/1.1 404"));
        // the page area is drawn with the engine's picture (the fake paints coloured quarters), under a bar with the keys
        let g = draw(&mut a, &cx, 1280, 720);
        assert!(g.hits.iter().any(|h| h.label == "Stop (Esc)") && g.hits.iter().any(|h| h.ev == Event::Tab(201)));
        let px = |x: f32, y: f32| -> [u8; 4] {
            let o = ((y as u32 * g.width() + x as u32) * 4) as usize;
            g.pm.data()[o..o + 4].try_into().unwrap()
        };
        assert_eq!(px(100.0, 72.0 + 40.0 + 100.0), [220, 40, 40, 255], "the engine's picture fills the page area");
        assert!(a.hint().contains("Space pause") && a.status_text().is_none());
        // once the page has loaded the shell presses `p` in it (the user activation that lets the engine start the video)
        pump_video(&mut a, &cx, "the play key", |a| a.video.as_ref().is_some_and(|v| v.kicked && v.full.log.iter().any(|l| l.contains("Key") && l.contains("112"))));
        // Space goes to the page (its script pauses and plays); keys do not reach the library behind it
        key(&mut a, &cx, Key::Char(' '));
        pump_video(&mut a, &cx, "the key to reach the engine", |a| a.video.as_ref().is_some_and(|v| v.full.log.iter().any(|l| l.contains("Key") && l.contains("32"))));
        assert!(a.video_playing());
        assert!(a.player.now_playing().is_none());
        // Esc stops: the engine view and the server are gone
        key(&mut a, &cx, Key::Esc);
        assert!(!a.video_playing());
        std::thread::sleep(std::time::Duration::from_millis(150));
        assert!(std::net::TcpStream::connect(("127.0.0.1", port)).is_err(), "the server's port is closed");
        assert!(std::fs::read_to_string(cx.audit.path()).unwrap().contains("media.video"));
        // Esc again leaves the app as before
        assert!(matches!(a.handle(&Event::Key(Key::Esc), &cx), Flow::Back));
    }

    #[test]
    fn without_a_player_or_the_engine_a_video_says_what_is_needed_and_an_installed_player_is_used_first() {
        let (mut a, cx, fake, _d) = dsetup("media-vnone");
        let sd = sample_dir("media-vnone");
        a.set_samples_dir(Some(sd.clone()), &cx);
        wait_scan(&mut a, &cx);
        a.handle(&Event::Tab(3), &cx);
        a.set_video_route(Some(VideoRoute::Missing));
        key(&mut a, &cx, Key::Enter);
        assert!(!a.video_playing());
        assert_eq!(a.status_text(), Some("Install the media player: IYAKKI menu \u{2192} Quick install \u{2192} Media."), "{:?}", a.status_text());
        assert!(fake.launched.lock().unwrap().is_empty());
        let _ = draw(&mut a, &cx, 1280, 720);
        // with an installed player the video goes to it, and an audio file is not a video at all
        a.set_video_route(Some(VideoRoute::External));
        key(&mut a, &cx, Key::Enter);
        assert_eq!(fake.launched.lock().unwrap().clone(), vec![sd.join("Videos/IYAKKI Sample Video.webm").to_str().unwrap().to_string()]);
        a.set_video_route(Some(VideoRoute::Missing));
        a.handle(&Event::Tab(0), &cx);
        key(&mut a, &cx, Key::Enter);
        assert_eq!(fake.launched.lock().unwrap().len(), 2, "an MP3 never needs the video route");
        // an engine that cannot be started says so and the app carries on
        a.handle(&Event::Tab(3), &cx);
        a.set_video_route(Some(VideoRoute::Embedded(std::path::PathBuf::from("/nonexistent/helper"))));
        key(&mut a, &cx, Key::Enter);
        assert!(!a.video_playing() && a.status_text().unwrap().contains("could not be started"), "{:?}", a.status_text());
    }

    #[test]
    fn the_now_playing_card_draws_and_the_level_display_moves_only_while_something_plays() {
        let (mut a, cx, _f, _d) = setup("media-hero");
        // the bars are a pure function of the tick: moving while playing, still when stopped
        assert_ne!(visualizer(10, true), visualizer(40, true));
        assert_eq!(visualizer(10, false), visualizer(40, false));
        assert!(visualizer(10, true).iter().all(|v| (0.0..=1.0).contains(v)));
        assert!(visualizer(10, true).iter().cloned().fold(0.0, f32::max) > visualizer(10, false)[0]);
        let frame = |a: &mut MediaApp, tick: u64| {
            let mut g = Gfx::new(1280, 720);
            g.clear(rgb(9, 11, 20));
            a.render(&mut g, R::new(0.0, 46.0, 1280.0, 674.0), 1.0, &cx, tick);
            g
        };
        // stopped: two frames at different ticks are the same picture, and the card is drawn
        let (s1, s2) = (frame(&mut a, 10), frame(&mut a, 40));
        assert!(s1.rgba() == s2.rgba(), "nothing moves while nothing plays");
        let blank = { let mut g = Gfx::new(1280, 720); g.clear(rgb(9, 11, 20)); g };
        assert!(s1.rgba() != blank.rgba(), "the card is there");
        // playing: the picture changes between ticks, and the title and type chips are in the hero
        key(&mut a, &cx, Key::Enter);
        assert!(a.player.now_playing().is_some());
        let (p1, p2) = (frame(&mut a, 10), frame(&mut a, 40));
        assert!(p1.rgba() != p2.rgba(), "the visualizer and the pulse move");
        assert!(p1.hits.iter().any(|h| h.ev == Event::Tab(100) && h.label == "Play or stop") && p1.hits.iter().any(|h| h.ev == Event::Tab(103)) && p1.hits.iter().any(|h| h.ev == Event::Tab(104)));
        // back to standby: still again
        key(&mut a, &cx, Key::Char('s'));
        let (q1, q2) = (frame(&mut a, 11), frame(&mut a, 77));
        assert!(q1.rgba() == q2.rgba());
        // type colours: kuthu hot magenta, melody cyan, beats amber
        assert_eq!(type_color("kuthu"), rgb(255, 60, 170));
        assert_eq!(type_color("melody"), CYAN);
        assert_eq!(type_color("beats"), AMBER);
    }

    /// Not a check: writes pictures of the new screens to the folder named by IYAKKI_SCREENS (for looking at them).
    #[test]
    #[ignore]
    fn dump_screens() {
        let Some(out) = std::env::var_os("IYAKKI_SCREENS").map(std::path::PathBuf::from) else { return };
        std::fs::create_dir_all(&out).unwrap();
        let (mut a, cx, _f, _d) = dsetup("media-dump");
        let slow = serve_slow(std::time::Duration::from_millis(3000), vec![("/a.mp3", "audio/mpeg", mp3("Slow One", "Net", "Web")), ("/b.mp3", "audio/mpeg", mp3("Slow One", "Net", "Web")), ("/c.webm", "video/webm", WEBM.to_vec())]);
        let fast = serve(vec![("/ok.mp3", "audio/mpeg", mp3("Fast One", "Net", "Web"))]);
        let save = |g: &Gfx, n: &str| g.save_png(&out.join(n)).unwrap();
        key(&mut a, &cx, Key::Char('D'));
        form_of(&mut a).set_value(0, "http://127.0.0.1:1/clip.mp4");
        form_of(&mut a).set_value(1, "action, my road trip");
        a.handle(&Event::FocusAt(1, 3.0), &cx);
        save(&draw(&mut a, &cx, 1280, 720), "form-video-1280.png");
        form_of(&mut a).set_value(0, "");
        a.handle(&Event::FocusAt(1, 3.0), &cx);
        save(&draw(&mut a, &cx, 1280, 720), "form-unknown-1280.png");
        save(&draw(&mut a, &cx, 640, 400), "form-unknown-640.png");
        key(&mut a, &cx, Key::Esc);
        key(&mut a, &cx, Key::Char('D'));
        form_of(&mut a).set_value(0, &format!("{fast}/ok.mp3 {slow}/a.mp3 {slow}/b.mp3 {slow}/c.webm {fast}/missing.mp3"));
        key(&mut a, &cx, Key::Ctrl('s'));
        std::thread::sleep(std::time::Duration::from_millis(1200));
        a.handle(&Event::Tick, &cx);
        save(&draw(&mut a, &cx, 1280, 720), "downloads-1280.png");
        save(&draw(&mut a, &cx, 800, 600), "downloads-800.png");
    }
}
