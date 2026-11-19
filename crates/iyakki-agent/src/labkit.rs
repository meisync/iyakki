//! The agent-side parts of the LAB (F-SPLIT-03): the catalogue read from the services, the backends over `Services` and the ffmpeg
//! frame source. The rest of the lab (models, caps, log, frame maths, entries) is `iyakki_shared::lab`.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use std::io::Read;

use iyakki_shared::lab::*;
use iyakki_shared::lemonade::ModelType;
use iyakki_shared::chat::Message;
use iyakki_shared::providers::Kind;
use iyakki_shared::piclog;
use iyakki_shared::providers::{ImageParams, ImageTrace};
use crate::services::Services;

/// Read the catalogue from the settings (the saved lists and the last typed Lemonade list); it never waits on the network.
pub fn catalogue(svc: &Services) -> Catalogue {
    let providers: Vec<crate::providers::ProviderCfg> = svc.settings.lock().unwrap_or_else(|e| e.into_inner()).get().providers.iter().filter(|p| p.enabled).cloned().collect();
    let mut c = Catalogue::default();
    for p in providers {
        let (remote, place) = p.base().map(|b| place_of(&b)).unwrap_or((false, "this computer"));
        let mk = |model: &str, sees: Sees, why: &str| LabModel { provider: p.id.clone(), kind: p.kind, model: model.to_string(), remote, place, size_gb: None, defaults: None, loaded: false, sees, why: why.to_string() };
        if p.kind == Kind::Lemonade {
            for m in svc.lemonade_cached(&p.id) {
                if !m.downloaded {
                    if m.kind == ModelType::Image {
                        c.pictures_not_downloaded += 1;
                    }
                    continue;
                }
                let mut e = mk(&m.id, Sees::Yes, "");
                (e.size_gb, e.defaults, e.loaded) = (m.size_gb, m.image_defaults, m.loaded);
                if is_video_model(&m.labels, &m.recipe) {
                    e.why = "listed with a video label".into();
                    c.video.push(e);
                    continue;
                }
                match m.kind {
                    ModelType::Image => {
                        e.why = "the server's list says it draws pictures".into();
                        c.pictures.push(e);
                    }
                    ModelType::Vision => {
                        e.why = "the server's list labels it vision".into();
                        c.vision.push(e);
                    }
                    ModelType::Chat => {
                        e.sees = Sees::Unknown;
                        e.why = "a chat model".into();
                        c.chat_other.push(e);
                    }
                    _ => {}
                }
            }
            continue;
        }
        for m in &p.models {
            if crate::models::is_image_model(m) {
                c.pictures.push(mk(m, Sees::Yes, "the name says it draws pictures"));
            } else if not_a_chat_model(m) {
                continue;
            } else if crate::providers::model_sees_pictures(m) {
                c.vision.push(mk(m, Sees::Yes, "the name says it sees pictures"));
            } else {
                c.chat_other.push(mk(m, Sees::Unknown, "a chat model"));
            }
        }
    }
    let order = |v: &mut Vec<LabModel>| v.sort_by(|a, b| (a.remote, &a.provider, a.model.to_ascii_lowercase()).cmp(&(b.remote, &b.provider, b.model.to_ascii_lowercase())));
    order(&mut c.pictures);
    order(&mut c.vision);
    order(&mut c.chat_other);
    order(&mut c.video);
    c
}

/// Can this model take pictures at all? `Err` says why not (an image, speech, embedding or reranking model); `Ok` says how sure.
pub fn check_vision_model(c: &Catalogue, svc: &Services, provider: &str, model: &str) -> Result<Sees, String> {
    if let Some(m) = c.vision.iter().find(|m| m.provider == provider && m.model == model) {
        return Ok(m.sees);
    }
    if c.pictures.iter().any(|m| m.provider == provider && m.model == model) || crate::models::is_image_model(model) {
        return Err(format!("{model} draws pictures from text; it cannot look at one. Pick a vision model."));
    }
    if let Some(m) = svc.lemonade_model(provider, model) {
        if !m.kind.chats() {
            return Err(format!("{model} is a {} model, not a chat model; it cannot answer questions about a picture.", m.kind.label().to_ascii_lowercase()));
        }
    }
    if not_a_chat_model(model) {
        return Err(format!("{model} is not a chat model; it cannot answer questions about a picture."));
    }
    Ok(Sees::Unknown)
}

/// The real backends: pictures and questions go through the providers set up in Settings.
pub struct ServicesBackend(pub Arc<Services>);

impl PictureBackend for ServicesBackend {
    fn draw(&self, r: &PictureRequest) -> (Result<(Vec<u8>, &'static str), String>, ImageTrace) {
        let p = ImageParams { seed: Some(r.seed), negative: r.negative.clone().filter(|n| !n.is_empty()), steps: r.steps, size: Some((r.width, r.height)), ..Default::default() };
        let (res, trace) = self.0.generate_image_traced(Some((&r.provider, &r.model)), &r.prompt, &format!("{}x{}", r.width, r.height), &p);
        (res.map(|(_, _, b, ext)| (b, ext)), trace)
    }
}

impl VisionBackend for ServicesBackend {
    fn ask(&self, r: &VisionRequest, cancel: &AtomicBool, on_text: &mut dyn FnMut(&str)) -> (Result<VisionAnswer, String>, CallTrace) {
        let mut t = CallTrace { request_bytes: r.images.iter().map(|i| i.len() as u64).sum::<u64>() + r.question.len() as u64, ..Default::default() };
        let client = match self.0.client_for(&r.provider, false) {
            Ok(c) => c,
            Err(e) => {
                t.error = e.clone();
                return (Err(e), t);
            }
        };
        let secrets = client.headers().map(|h| piclog::secret_values(&h)).unwrap_or_default();
        let msgs = vec![Message::user_with_images(r.question.clone(), r.images.clone())];
        let streamer = self.0.chat_streamer();
        let started = Instant::now();
        let mut got = false;
        let out = match &streamer {
            Some(s) => client.chat_turn_stream(&r.model, &msgs, &[], s.as_ref(), &mut |piece| {
                if !piece.is_empty() {
                    got = true;
                    on_text(piece);
                }
                !cancel.load(Ordering::SeqCst)
            }),
            None => {
                t.notes.push("answer arrived in one piece (no streaming connection)".into());
                client.chat_turn(&r.model, &msgs, &[])
            }
        };
        t.ms = started.elapsed().as_millis() as u64;
        let tokens = crate::providers::reported_usage();
        match out {
            Ok(turn) => {
                if cancel.load(Ordering::SeqCst) {
                    t.error = "cancelled".into();
                    return (Err("cancelled".into()), t);
                }
                t.status = Some(200);
                t.response_bytes = turn.text.len() as u64;
                if turn.text.trim().is_empty() {
                    t.error = "the model returned no text".into();
                    return (Err("the model returned no text (a model that cannot take pictures sometimes does this)".into()), t);
                }
                (Ok(VisionAnswer { text: turn.text, tokens, streamed: streamer.is_some() && got }), t)
            }
            Err(e) => {
                let e = piclog::scrub(&e, &secrets);
                t.status = status_of(&e);
                t.error = e.clone();
                (Err(cannot_see_message(&r.model, &e).unwrap_or(e)), t)
            }
        }
    }
}

/// `ffmpeg` from the catalogue or the system (the same program the MEDIA player uses); absent when it is not installed.
pub struct Ffmpeg;

impl FrameSource for Ffmpeg {
    fn available(&self) -> Result<(), String> {
        if crate::player::available() { Ok(()) } else { Err(NO_FFMPEG.to_string()) }
    }

    fn info(&self, file: &str) -> Result<VideoInfo, String> {
        let cfg = crate::player::Config::system().ok_or_else(|| NO_FFMPEG.to_string())?;
        let i = crate::player::probe(&cfg, file)?;
        let v = i.video.ok_or("that file has no picture stream (it may be only sound)")?;
        let duration = i.duration.ok_or("ffmpeg could not tell how long the video is")?;
        Ok(VideoInfo { duration, size: (v.w, v.h) })
    }

    fn frame_rgba(&self, file: &str, t: f64, size: (u32, u32), cancel: &Arc<AtomicBool>) -> Result<Vec<u8>, String> {
        use std::os::unix::process::CommandExt;
        let cfg = crate::player::Config::system().ok_or_else(|| NO_FFMPEG.to_string())?;
        let want = size.0 as usize * size.1 as usize * 4;
        let mut child = std::process::Command::new(&cfg.ffmpeg)
            .args(frame_args(file, t, size.0, size.1))
            .process_group(0)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("could not start ffmpeg: {e}"))?;
        let pid = child.id() as i32;
        let done = Arc::new(AtomicBool::new(false));
        // a watchdog: stops ffmpeg when the time is up or the person cancelled
        let (d2, c2) = (done.clone(), cancel.clone());
        let started = Instant::now();
        let guard = std::thread::spawn(move || {
            while !d2.load(Ordering::SeqCst) {
                if started.elapsed() > Duration::from_secs(FFMPEG_SECS) || c2.load(Ordering::SeqCst) {
                    // SAFETY: kill(2) on the process group of the child this call started
                    unsafe { libc::kill(-pid, libc::SIGKILL) };
                    return;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        let mut out = child.stdout.take().ok_or("no output from ffmpeg")?;
        let mut buf = Vec::with_capacity(want);
        let mut chunk = [0u8; 65536];
        while buf.len() < want {
            match out.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n.min(want - buf.len())]),
            }
        }
        done.store(true, Ordering::SeqCst);
        // SAFETY: as above (a process group that has already ended is not an error here)
        unsafe { libc::kill(-pid, libc::SIGKILL) };
        let _ = child.wait();
        let _ = guard.join();
        if cancel.load(Ordering::SeqCst) {
            return Err("cancelled".into());
        }
        if buf.len() != want {
            return Err(format!("ffmpeg gave no picture at {} (the video may end sooner, or the file is damaged)", clock_text(t)));
        }
        Ok(buf)
    }
}

