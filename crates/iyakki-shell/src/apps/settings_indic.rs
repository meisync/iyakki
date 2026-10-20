//! Settings → Indic voices: the neural voices of AI4Bharat Indic-TTS for Tamil, Telugu, Malayalam, Kannada, Hindi, Bengali, Marathi
//! and the other Indic languages, and the Indian English voice.
//!
//! One list of languages with their state (installed, partly downloaded, not installed), and the actions of an administrator: download
//! one (an approval card names the host the files come from and how big they are; nothing is fetched before it is confirmed),
//! delete one, take one from a folder or a USB stick (for a computer that is never online), say where the bundles are published, say
//! where the voice server listens, switch the reading to these voices, choose female or male, test the voice and check the
//! server. The models are checked against built-in SHA-256 sums; they stay on this device. See `iyakki_agent::indic` and
//! docs/VOICE.md. Default: off.

use super::*;
use iyakki_agent::indic::{self, models};
use std::path::PathBuf;

pub(super) struct IndicPanel {
    pub(super) streamer: Arc<dyn iyakki_agent::iyakki_proxy::Streamer>,
    /// The download that is running: language and its progress.
    pub(super) progress: Option<(String, Arc<models::Progress>)>,
    /// What the last check of the server found.
    pub(super) check: Option<String>,
    /// The gender chosen for the voices (the setting holds it once the voices are switched on).
    pub(super) female: bool,
    /// Does the Test button play sound? Off in tests.
    pub(super) play: bool,
    /// Where the models are kept instead of the data folder (tests).
    pub(super) root: Option<PathBuf>,
}

impl Default for IndicPanel {
    fn default() -> IndicPanel {
        IndicPanel { streamer: Arc::new(iyakki_agent::iyakki_proxy::https::UreqStreamer::default()), progress: None, check: None, female: true, play: true, root: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum IndicAct {
    Download(String),
    Delete(String),
}

impl SettingsApp {
    /// Use other seams for the Indic voices page (tests).
    pub fn with_indic(mut self, streamer: Arc<dyn iyakki_agent::iyakki_proxy::Streamer>, root: PathBuf, play: bool) -> SettingsApp {
        self.indic = IndicPanel { streamer, progress: None, check: None, female: true, play, root: Some(root) };
        self
    }

    fn indic_root(&self, cx: &AppCx) -> PathBuf {
        self.indic.root.clone().unwrap_or_else(|| models::root(Self::svc(cx).dir()))
    }

    /// The Indic engine of the current text-to-speech setting: its server address and voice.
    pub(super) fn indic_engine(cx: &AppCx) -> Option<(String, Option<String>)> {
        match Self::svc(cx).voice_settings().tts {
            Some(SpeechEngine::Host { engine, program, voice }) if engine == indic::ENGINE_ID => Some((program.filter(|p| !p.is_empty()).unwrap_or_else(|| indic::DEFAULT_URL.to_string()), voice)),
            _ => None,
        }
    }

    fn indic_gender(&self, cx: &AppCx) -> &'static str {
        match Self::indic_engine(cx).and_then(|(_, v)| v) {
            Some(v) if v.ends_with("male") && !v.ends_with("female") => "male",
            Some(v) if v.ends_with("female") => "female",
            _ => {
                if self.indic.female {
                    "female"
                } else {
                    "male"
                }
            }
        }
    }

    pub(super) fn indic_len() -> usize {
        indic::LANGS.len()
    }

    pub(super) fn indic_lang(&self) -> &'static str {
        indic::LANGS[self.sel().min(indic::LANGS.len() - 1)].code
    }

    fn indic_tick(&mut self) {
        if self.job.is_none() {
            self.indic.progress = None;
        }
    }

    pub(super) fn handle_indic(&mut self, k: &Key, cx: &AppCx) {
        self.indic_tick();
        let lang = self.indic_lang();
        let root = self.indic_root(cx);
        match k {
            Key::Up => return self.move_sel(-1, Self::indic_len()),
            Key::Down => return self.move_sel(1, Self::indic_len()),
            Key::Char('r') => {
                self.indic.check = None;
                indic::clear_cache();
                return self.ok("Read again.");
            }
            _ => {}
        }
        if !matches!(k, Key::Char('d' | 'x' | 'u' | 'b' | 'a' | 'e' | 'g' | 'y' | 'c' | 'k') | Key::Enter) {
            return;
        }
        if !self.admin(cx) {
            return;
        }
        match k {
            Key::Char('d') | Key::Enter => match models::plan(&root, lang, &models::source(&root)) {
                Err(e) => self.fail(e),
                Ok(_) if matches!(models::status(&root, lang), models::Status::Installed { .. }) => self.ok(format!("{} is already installed.", Self::indic_name(lang))),
                Ok(p) => {
                    let text = format!("{}\n\n{}", p.card(), indic::LICENCE_NOTICE);
                    self.modal = Some(Modal::Confirm(ConfirmKind::Indic(IndicAct::Download(lang.to_string())), text));
                }
            },
            Key::Char('x') => match models::status(&root, lang) {
                models::Status::NotInstalled | models::Status::NoBundle => self.fail(format!("{} is not installed.", Self::indic_name(lang))),
                _ => self.modal = Some(Modal::Confirm(ConfirmKind::Indic(IndicAct::Delete(lang.to_string())), format!("Delete the {} voice from this device? It can be installed again.", Self::indic_name(lang)))),
            },
            Key::Char('u') => {
                if matches!(models::pins(&root, lang), None) {
                    return self.fail(format!("This build has no bundle for {}; nothing can be installed from a folder either.", Self::indic_name(lang)));
                }
                self.open_form(FormKind::IndicFolder(lang.to_string()), Form::new(&format!("Install {} from a folder or USB stick", Self::indic_name(lang))).text("Folder (the stick's mount point, or the folder holding fastpitch.onnx, hifigan.onnx and vocab.json)", "", "/media/usb/indic"));
            }
            Key::Char('b') => {
                let cur = models::source(&root);
                self.open_form(FormKind::IndicSource, Form::new("Where the voice bundles are published").text("Address (https://…, or http:// on your own network; empty to clear)", &cur, "https://models.example.org/indic"));
            }
            Key::Char('a') => {
                let cur = Self::indic_engine(cx).map(|(u, _)| u).unwrap_or_else(|| models::server(&root));
                self.open_form(FormKind::IndicServer, Form::new("Where the voice server listens").text("Address", &cur, indic::DEFAULT_URL));
            }
            Key::Char('e') => self.indic_toggle(cx, &root),
            Key::Char('g') => {
                self.indic.female = self.indic_gender(cx) != "female";
                if let Some((url, _)) = Self::indic_engine(cx) {
                    let v = if self.indic.female { "female" } else { "male" };
                    self.set_tts(cx, Some(SpeechEngine::Host { engine: indic::ENGINE_ID.into(), program: Some(url), voice: Some(v.into()) }), &format!("Indic voices, {v}"));
                } else {
                    self.ok(format!("Voice: {}. It is used when the Indic voices are switched on (press e).", self.indic_gender(cx)));
                }
            }
            Key::Char('c') => self.indic_check(cx, &root),
            Key::Char('y') => self.indic_test(cx, &root, lang),
            Key::Char('k') => match &self.indic.progress {
                Some((_, p)) => {
                    p.cancel.store(true, std::sync::atomic::Ordering::Relaxed);
                    self.ok("Cancelling; what was fetched is kept and the download can be continued.");
                }
                None => self.fail("No download is running."),
            },
            _ => {}
        }
    }

    fn indic_name(code: &str) -> &'static str {
        indic::lang_info(code).map_or("this language", |l| l.name)
    }

    fn indic_toggle(&mut self, cx: &AppCx, root: &std::path::Path) {
        if Self::indic_engine(cx).is_some() {
            // back to automatic
            self.set_tts(cx, None, "automatic (the Indic voices are off)");
            return;
        }
        if models::installed(root).is_empty() && self.indic.check.is_none() {
            // the voices may live on a server elsewhere: allowed, but say what is (not) known
            self.ok("No voice is installed on this device. If the voice server is somewhere else, check it with c; reading falls back to the next engine for a language the server lacks.");
        }
        let url = models::server(root);
        let v = if self.indic.female { "female" } else { "male" };
        self.set_tts(cx, Some(SpeechEngine::Host { engine: indic::ENGINE_ID.into(), program: Some(url), voice: Some(v.into()) }), "Indic neural voices");
    }

    fn indic_server_url(&self, cx: &AppCx, root: &std::path::Path) -> String {
        Self::indic_engine(cx).map(|(u, _)| u).unwrap_or_else(|| models::server(root))
    }

    fn indic_check(&mut self, cx: &AppCx, root: &std::path::Path) {
        let url = self.indic_server_url(cx, root);
        indic::clear_cache();
        self.start("asking the voice server", move || match indic::check_server(&url) {
            Ok(v) => {
                let mut langs: Vec<String> = v.iter().map(|x| x.lang.clone()).collect();
                langs.dedup();
                JobOut::Msg(Ok(format!("The voice server answers: {} voices for {}.", v.len(), if langs.is_empty() { "no language yet".to_string() } else { langs.join(", ") })))
            }
            Err(e) => JobOut::Msg(Err(e)),
        });
    }

    fn indic_test(&mut self, cx: &AppCx, root: &std::path::Path, lang: &'static str) {
        let url = self.indic_server_url(cx, root);
        let voice = Some(self.indic_gender(cx).to_string());
        let play_cmd = Self::svc(cx).voice_settings().play_cmd;
        let play = self.indic.play;
        let name = Self::indic_name(lang);
        self.start(&format!("reading a {name} sentence"), move || {
            let text = hostengines::sample_sentence(lang);
            match indic::synth_wav(&url, voice.as_deref(), Some(lang), text, 1.0) {
                Err(e) => JobOut::Msg(Err(format!("{} Nothing was played.", e.line()))),
                Ok(wav) => {
                    let secs = voice::parse_wav(&wav).map_or(0.0, |w| w.samples.len() as f32 / w.rate.max(1) as f32);
                    if !play {
                        return JobOut::Msg(Ok(format!("Made {secs:.1} s of {name} speech (not played in this window).")));
                    }
                    match hostengines::player_command(&play_cmd, &std::env::var("PATH").unwrap_or_default()).ok_or("no sound player was found").and_then(|c| Player::start(&c, &wav).map_err(|_| "the sound player failed")) {
                        Ok(mut p) => {
                            let t = std::time::Instant::now();
                            while p.is_playing() && t.elapsed() < Duration::from_secs(60) {
                                std::thread::sleep(Duration::from_millis(40));
                            }
                            p.stop();
                            JobOut::Msg(Ok(format!("Read a {name} sentence ({secs:.1} s) with the Indic voice.")))
                        }
                        Err(e) => JobOut::Msg(Err(format!("Made {secs:.1} s of {name} speech but could not play it: {e}"))),
                    }
                }
            }
        });
    }

    pub(super) fn indic_submit(&mut self, cx: &AppCx, kind: &FormKind, form: &Form) -> Result<String, String> {
        if !self.admin(cx) {
            return Ok(String::new());
        }
        let root = self.indic_root(cx);
        match kind {
            FormKind::IndicFolder(lang) => {
                let folder = PathBuf::from(form.value(0).trim());
                if !folder.is_dir() {
                    return Err("That folder does not exist.".into());
                }
                let lang = lang.clone();
                cx.audit.record(&cx.session.user, "settings.indic.import", &lang);
                self.start(&format!("checking and copying the {} voice", Self::indic_name(&lang)), move || match models::import_folder(&root, &lang, &folder) {
                    Ok(()) => JobOut::Msg(Ok(format!("The {} voice is installed from the folder (every file matched its checksum).", Self::indic_name(&lang)))),
                    Err(e) => JobOut::Msg(Err(e)),
                });
                Ok("Checking the files…".into())
            }
            FormKind::IndicSource => {
                models::set_source(&root, form.value(0).trim())?;
                cx.audit.record(&cx.session.user, "settings.indic.source", &iyakki_sec::url_host(form.value(0).trim()).unwrap_or_default());
                Ok(if form.value(0).trim().is_empty() { "The bundle address is cleared.".into() } else { format!("Bundles come from {}.", form.value(0).trim().trim_end_matches('/')) })
            }
            FormKind::IndicServer => {
                let url = form.value(0).trim().trim_end_matches('/').to_string();
                let url = if url.is_empty() { indic::DEFAULT_URL.to_string() } else { url };
                indic::validate_url(&url)?;
                models::set_server(&root, &url)?;
                indic::clear_cache();
                self.indic.check = None;
                if let Some((_, v)) = Self::indic_engine(cx) {
                    self.set_tts(cx, Some(SpeechEngine::Host { engine: indic::ENGINE_ID.into(), program: Some(url.clone()), voice: v }), "Indic neural voices");
                }
                cx.audit.record(&cx.session.user, "settings.indic.server", &iyakki_sec::url_host(&url).unwrap_or_default());
                Ok(format!("The voice server is {url}. Text is sent only to it."))
            }
            _ => Ok(String::new()),
        }
    }

    pub(super) fn indic_confirm(&mut self, cx: &AppCx, act: IndicAct) {
        if !self.admin(cx) {
            return;
        }
        let root = self.indic_root(cx);
        match act {
            IndicAct::Delete(lang) => match models::delete(&root, &lang) {
                Ok(()) => {
                    cx.audit.record(&cx.session.user, "settings.indic.delete", &lang);
                    indic::clear_cache();
                    self.ok(format!("The {} voice is deleted from this device.", Self::indic_name(&lang)));
                }
                Err(e) => self.fail(e),
            },
            IndicAct::Download(lang) => {
                let plan = match models::plan(&root, &lang, &models::source(&root)) {
                    Ok(p) => p,
                    Err(e) => return self.fail(e),
                };
                cx.audit.record(&cx.session.user, "settings.indic.download", &format!("{lang} from {}", plan.host));
                let progress = Arc::new(models::Progress::default());
                self.indic.progress = Some((lang.clone(), progress.clone()));
                let streamer = self.indic.streamer.clone();
                let name = Self::indic_name(&lang);
                self.start(&format!("downloading the {name} voice"), move || match models::download(&root, &plan, &*streamer, &progress) {
                    Ok(()) => {
                        indic::clear_cache();
                        JobOut::Msg(Ok(format!("The {name} voice is installed ({}). Switch the reading to the Indic voices with e.", models::human(plan.total_bytes))))
                    }
                    Err(e) => JobOut::Msg(Err(e)),
                });
            }
        }
    }

    // ------------------------------------------------------------------------------------------------ the page

    pub(super) fn render_indic(&mut self, g: &mut Gfx, a: R, s: f32, cx: &AppCx) {
        self.indic_tick();
        let root = self.indic_root(cx);
        let warn = rgb(255, 196, 94);
        let compact = a.h < 340.0 * s;
        let label_w = (130.0 * s).min(a.w * 0.3);
        let mut y = a.y;
        let engine = Self::indic_engine(cx);
        let reading = match (&engine, Self::svc(cx).voice_settings().tts) {
            (Some((url, v)), _) => (format!("Indic voices · {} · server {}", v.clone().unwrap_or_else(|| "female".into()), url), OK),
            (None, None) => ("off · the best engine found on this computer reads (Settings → Voice)".to_string(), MUTED),
            (None, Some(_)) => ("off · another engine is chosen in Settings → Voice".to_string(), MUTED),
        };
        let installed = models::installed(&root);
        let rows: Vec<(&str, String, [u8; 4])> = {
            let mut v = vec![("Reading", reading.0, reading.1), ("Voice", format!("{}  (g changes it)", self.indic_gender(cx)), TEXT)];
            let server = self.indic_server_url(cx, &root);
            v.push(("Voice server", format!("{server} · {}", self.indic.check.clone().unwrap_or_else(|| "not checked (c)".into())), TEXT));
            if !compact {
                v.push(("On this device", format!("{} of {} installed · {} on disk · {}", installed.len(), indic::LANGS.len(), models::human(models::disk_used(&root)), root.display()), TEXT));
                let src = models::source(&root);
                v.push(("Bundles from", if src.is_empty() { "no address set (b), or use a folder or USB stick (u)".to_string() } else { src }, if models::source(&root).is_empty() { warn } else { TEXT }));
                if let Some(n) = indic::last_note() {
                    v.push(("Last fallback", n, warn));
                }
            }
            v
        };
        for (label, value, col) in &rows {
            g.text(a.x, y, 14.0 * s, MUTED, label, Face::Sans);
            let t = g.fit(14.0 * s, value, Face::Bold, a.w - label_w);
            g.text(a.x + label_w, y, 14.0 * s, *col, &t, Face::Bold);
            y += 22.0 * s;
        }
        if let Some((lang, p)) = &self.indic.progress {
            let t = format!("Downloading {}: {}% ({} of {})  ·  k cancels", Self::indic_name(lang), p.percent(), models::human(p.bytes.load(std::sync::atomic::Ordering::Relaxed)), models::human(p.total.load(std::sync::atomic::Ordering::Relaxed)));
            let t = g.fit(13.0 * s, &t, Face::Bold, a.w);
            g.text(a.x, y, 13.0 * s, ACCENT, &t, Face::Bold);
            y += 20.0 * s;
        }
        y += 4.0 * s;
        // the licence notice sits at the bottom: two lines when the window is small, all of it when there is room
        let note_lines = g.wrap(12.0 * s, indic::LICENCE_NOTICE, Face::Sans, a.w);
        let note_n = if compact { 2 } else { 4 }.min(note_lines.len());
        let note_h = note_n as f32 * 16.0 * s + 4.0 * s;
        let row_h = 54.0 * s;
        let avail = (a.y + a.h - note_h - y).max(row_h);
        let visible = ((avail / row_h) as usize).clamp(1, indic::LANGS.len());
        let sel = self.sel().min(indic::LANGS.len() - 1);
        let first = sel.saturating_sub(visible - 1).min(indic::LANGS.len() - visible);
        let first = if sel < first { sel } else { first };
        for (i, l) in indic::LANGS.iter().enumerate().skip(first).take(visible) {
            let st = models::status(&root, l.code);
            let size = models::pins(&root, l.code).map(|p| models::human(p.iter().map(|x| x.bytes).sum()));
            let (sub, chip): (String, (&str, [u8; 4])) = match &st {
                models::Status::Installed { bytes } => (format!("Installed · {} · female and male", models::human(*bytes)), ("installed", OK)),
                models::Status::Partial { have, total } => (format!("Partly downloaded · {} of {}", models::human(*have), models::human(*total)), ("partial", warn)),
                models::Status::NotInstalled => (format!("Not installed · {} to download", size.clone().unwrap_or_default()), ("not installed", MUTED)),
                models::Status::Damaged(w) => (w.clone(), ("damaged", ERR)),
                models::Status::NoBundle => (format!("No bundle for this language in this build · AI4Bharat checkpoint {}", models::human(l.zip_bytes)), ("no bundle", DIM)),
            };
            row(g, i, a.x, y, a.w, row_h - 4.0 * s, s, i == sel, &format!("{} · {}", l.name, l.native), &sub, &[chip]);
            y += row_h;
        }
        let mut ny = a.y + a.h - note_h;
        for l in note_lines.iter().take(note_n) {
            g.text(a.x, ny, 12.0 * s, DIM, l, Face::Sans);
            ny += 16.0 * s;
        }
    }

    pub(super) fn indic_buttons(&self, admin: bool) -> Vec<Btn> {
        let (acc, mut_, err) = (ACCENT, TEXT, ERR);
        if admin {
            vec![("Download", acc, 'd'), ("Delete", err, 'x'), ("From folder or USB", mut_, 'u'), ("Bundles from", mut_, 'b'), ("Voice server", mut_, 'a'), ("Use for reading", acc, 'e'), ("Female/male", mut_, 'g'), ("Test voice", mut_, 'y'), ("Check server", mut_, 'c'), ("Cancel", mut_, 'k')]
        } else {
            vec![("Look again", mut_, 'r')]
        }
    }
}

/// The approval card of a download or a delete: the full text, wrapped (the usual one-line card would cut the host and the size).
pub(super) fn render_card(g: &mut Gfx, area: R, s: f32, act: &IndicAct, text: &str) {
    let (title, yes, danger) = match act {
        IndicAct::Download(_) => ("Download a voice?", "Yes, download", false),
        IndicAct::Delete(_) => ("Delete a voice?", "Yes, delete", true),
    };
    let p = crate::fx::modal(g, area, s, 680.0 * s, 440.0 * s, ACCENT, danger);
    let y = crate::fx::modal_title(g, p, s, title, if danger { ERR } else { ACCENT });
    let bottom = p.y + p.h - 70.0 * s;
    let mut ly = y + 8.0 * s;
    'text: for para in text.split('\n') {
        if para.is_empty() {
            ly += 8.0 * s;
            continue;
        }
        for l in g.wrap(14.0 * s, para, Face::Sans, p.w - 52.0 * s) {
            if ly + 18.0 * s > bottom {
                break 'text;
            }
            g.text(p.x + 26.0 * s, ly, 14.0 * s, TEXT, &l, Face::Sans);
            ly += 19.0 * s;
        }
    }
    g.text(p.x + 26.0 * s, p.y + p.h - 92.0 * s, 13.0 * s, MUTED, "Press y to confirm, or Esc to cancel.", Face::Sans);
    g.button(p.x + 26.0 * s, p.y + p.h - 58.0 * s, 14.0 * s, yes, if danger { ERR } else { ACCENT }, Event::Key(Key::Char('y')));
    g.button(p.x + 26.0 * s + 170.0 * s, p.y + p.h - 58.0 * s, 14.0 * s, "Cancel", MUTED, Event::Key(Key::Esc));
}
