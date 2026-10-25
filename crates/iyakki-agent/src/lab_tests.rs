//! LAB logic with fakes: the model catalogue per tab, frame sampling, the caps, the log, and the real backends against a fake
//! provider (no network, no real model, no ffmpeg).

use super::*;
use crate::providers::ProviderCfg;
use iyakki_proxy::{Upstream, UpstreamResponse};
use std::sync::Mutex;

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-lab-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

type Seen = Arc<Mutex<Vec<(String, String, Vec<(String, String)>, String)>>>;

/// A provider that answers from a closure and remembers what it was asked.
struct Fake {
    seen: Seen,
    reply: Box<dyn Fn(&str) -> (u16, String) + Send + Sync>,
}

impl Upstream for Fake {
    fn request(&self, m: &str, url: &str, h: &[(String, String)], b: &[u8]) -> Result<UpstreamResponse, String> {
        self.seen.lock().unwrap().push((m.into(), url.into(), h.to_vec(), String::from_utf8_lossy(b).into()));
        let (status, body) = (self.reply)(url);
        Ok(UpstreamResponse { status, body: body.into_bytes() })
    }
}

fn services(tag: &str, reply: impl Fn(&str) -> (u16, String) + Send + Sync + 'static) -> (Arc<Services>, Seen) {
    let seen: Seen = Default::default();
    let up: Arc<dyn Upstream> = Arc::new(Fake { seen: seen.clone(), reply: Box::new(reply) });
    (Services::open(&tmp(tag), up.clone(), up).unwrap(), seen)
}

fn lemonade_list() -> serde_json::Value {
    serde_json::json!({"data": [
        {"id": "Qwen3-0.6B-GGUF", "recipe": "llamacpp", "size": 0.38, "downloaded": true, "labels": ["reasoning"]},
        {"id": "Phi-4-Multimodal-GGUF", "recipe": "llamacpp", "size": 3.1, "downloaded": true, "labels": ["vision"]},
        {"id": "nomic-embed-text-v1-GGUF", "recipe": "llamacpp", "size": 0.07, "downloaded": true, "labels": ["embeddings"]},
        {"id": "SD-Turbo", "recipe": "sd-cpp", "size": 2.6, "downloaded": true, "labels": ["image"], "image_defaults": {"steps": 4, "width": 512, "height": 512}},
        {"id": "SDXL-Base-1.0", "recipe": "sd-cpp", "size": 6.94, "downloaded": false, "labels": ["image"], "image_defaults": {"steps": 20, "width": 1024, "height": 1024}},
        {"id": "Whisper-Base", "recipe": "whispercpp", "size": 0.14, "downloaded": true, "labels": ["transcription"]},
        {"id": "Wan-Video-1", "recipe": "wan-video", "size": 9.0, "downloaded": true, "labels": ["video"]}
    ]})
}

fn add_lemonade(svc: &Services, id: &str, base: &str, list: &serde_json::Value) {
    let mut p = ProviderCfg::new(id, Kind::Lemonade);
    p.base_url = Some(base.into());
    svc.settings.lock().unwrap().upsert_provider(p).unwrap();
    crate::lemonade::remember(&crate::lemonade::api_base(base), &crate::lemonade::parse_models(list, &[]));
}

fn add_compat(svc: &Services, id: &str, base: &str, models: &[&str]) {
    let mut p = ProviderCfg::new(id, Kind::Compat);
    p.base_url = Some(base.into());
    p.models = models.iter().map(|m| m.to_string()).collect();
    svc.settings.lock().unwrap().upsert_provider(p).unwrap();
}

#[test]
fn each_tab_gets_the_models_that_fit_it_with_where_they_run() {
    let (svc, _) = services("cat", |_| (200, "{}".into()));
    add_lemonade(&svc, "home", "http://127.0.0.1:13391", &lemonade_list());
    add_lemonade(&svc, "gpu", "http://192.168.1.50:13392", &lemonade_list());
    add_compat(&svc, "oa", "http://127.0.0.1:13393/v1", &["gpt-4o", "dall-e-3", "text-embedding-3-small", "llama3.2", "whisper-1"]);
    let c = catalogue(&svc);
    let pics: Vec<String> = c.pictures.iter().map(|m| format!("{}/{}", m.provider, m.model)).collect();
    assert_eq!(pics, ["home/SD-Turbo", "oa/dall-e-3", "gpu/SD-Turbo"], "downloaded picture models, this computer first: {pics:?}");
    let vis: Vec<String> = c.vision.iter().map(|m| format!("{}/{}", m.provider, m.model)).collect();
    assert_eq!(vis, ["home/Phi-4-Multimodal-GGUF", "oa/gpt-4o", "gpu/Phi-4-Multimodal-GGUF"]);
    let other: Vec<String> = c.chat_other.iter().map(|m| format!("{}/{}", m.provider, m.model)).collect();
    assert_eq!(other, ["home/Qwen3-0.6B-GGUF", "oa/llama3.2", "gpu/Qwen3-0.6B-GGUF"], "embeddings, speech and picture models are not chat models");
    assert_eq!(c.pictures_not_downloaded, 2, "SDXL on both servers is listed but not downloaded");
    assert!(c.vision.iter().all(|m| m.sees == Sees::Yes) && c.chat_other.iter().all(|m| m.sees == Sees::Unknown));
    // "any chat model" adds the others after the vision ones
    assert_eq!(c.vision_choices(false).len(), 3);
    assert_eq!(c.vision_choices(true).len(), 6);
    assert_eq!(c.vision_choices(true)[3].sees, Sees::Unknown);
    // size and steps come from the server's own list; where it runs from the address
    let turbo = c.find_picture("home", "SD-Turbo").unwrap();
    assert_eq!((turbo.defaults.unwrap().width, turbo.defaults.unwrap().steps, turbo.remote), (Some(512), Some(4), false));
    assert!(turbo.label().contains("2.6 GB") && turbo.label().contains("512x512, 4 steps") && turbo.label().contains("on home"), "{}", turbo.label());
    assert_eq!(turbo.runs_on(), "Runs on home (this computer)");
    let gpu = c.find_picture("gpu", "SD-Turbo").unwrap();
    assert!(gpu.remote && gpu.runs_on() == "Runs on gpu (remote, the network)", "{}", gpu.runs_on());
    assert!(gpu.label().contains("remote"));
    assert!(turbo.takes_negative() && !c.find_picture("oa", "dall-e-3").unwrap().takes_negative(), "only a Lemonade server takes a negative prompt");
    // a model that says it makes video is found, and the line says plainly that nothing is sent for it
    assert_eq!(c.video.len(), 2);
    let line = video_line(&c);
    assert!(line.contains("Wan-Video-1") && line.contains("no call for them yet") && line.contains("nothing is faked"), "{line}");
}

#[test]
fn with_no_video_model_the_video_line_is_the_one_honest_sentence() {
    let (svc, _) = services("novideo", |_| (200, "{}".into()));
    add_compat(&svc, "oa", "http://127.0.0.1:13394/v1", &["gpt-4o"]);
    let c = catalogue(&svc);
    assert!(c.video.is_empty());
    assert_eq!(video_line(&c), NO_VIDEO_MODEL);
    assert!(NO_VIDEO_MODEL.starts_with("No video generation model is available from your providers") && NO_VIDEO_MODEL.contains("Settings"));
    assert!(is_video_model(&["Video".into()], "") && is_video_model(&[], "ltx-video") && !is_video_model(&["image".into()], "sd-cpp"));
    // a disabled provider is not looked at
    let mut p = svc.settings.lock().unwrap().get().provider("oa").cloned().unwrap();
    p.enabled = false;
    svc.settings.lock().unwrap().upsert_provider(p).unwrap();
    assert!(catalogue(&svc).vision.is_empty());
}

#[test]
fn a_model_that_cannot_take_pictures_is_refused_before_anything_is_sent() {
    let (svc, seen) = services("cannot", |_| (200, "{}".into()));
    add_lemonade(&svc, "home", "http://127.0.0.1:13395", &lemonade_list());
    add_compat(&svc, "oa", "http://127.0.0.1:13396/v1", &["gpt-4o", "dall-e-3", "llama3.2", "text-embedding-3-small"]);
    let c = catalogue(&svc);
    assert_eq!(check_vision_model(&c, &svc, "oa", "gpt-4o"), Ok(Sees::Yes));
    assert_eq!(check_vision_model(&c, &svc, "home", "Phi-4-Multimodal-GGUF"), Ok(Sees::Yes));
    assert_eq!(check_vision_model(&c, &svc, "oa", "llama3.2"), Ok(Sees::Unknown), "a plain chat model may be tried, with a warning");
    let e = check_vision_model(&c, &svc, "oa", "dall-e-3").unwrap_err();
    assert!(e.contains("draws pictures") && e.contains("cannot look at one"), "{e}");
    let e = check_vision_model(&c, &svc, "home", "nomic-embed-text-v1-GGUF").unwrap_err();
    assert!(e.contains("embeddings model") && e.contains("cannot answer questions about a picture"), "{e}");
    assert!(check_vision_model(&c, &svc, "oa", "text-embedding-3-small").is_err());
    assert!(seen.lock().unwrap().is_empty(), "checking sends nothing");
    // what a server says when a chat model gets a picture it cannot take
    let m = cannot_see_message("llama3.2", "HTTP 400: this model does not support image input").unwrap();
    assert!(m.starts_with("llama3.2 cannot take pictures") && m.contains("vision model"), "{m}");
    assert!(cannot_see_message("m", "HTTP 503: overloaded").is_none(), "other errors are not blamed on the model");
}

#[test]
fn frames_are_placed_evenly_or_every_so_many_seconds_within_four_to_sixteen() {
    // even: the middle of n equal parts
    assert_eq!(sample_times(40.0, 4, Sampling::Even).unwrap(), [5.0, 15.0, 25.0, 35.0]);
    assert_eq!(sample_times(10.0, 2, Sampling::Even).unwrap().len(), MIN_FRAMES, "fewer than four is raised to four");
    assert_eq!(sample_times(100.0, 99, Sampling::Even).unwrap().len(), MAX_FRAMES, "more than sixteen is cut");
    let t = sample_times(61.0, 8, Sampling::Even).unwrap();
    assert!(t.windows(2).all(|w| w[1] > w[0]) && t[0] > 0.0 && *t.last().unwrap() < 61.0, "{t:?}");
    // every k seconds from the start, at most n, never past the end
    assert_eq!(sample_times(100.0, 6, Sampling::EveryK(10.0)).unwrap(), [0.0, 10.0, 20.0, 30.0, 40.0, 50.0]);
    assert_eq!(sample_times(25.0, 16, Sampling::EveryK(10.0)).unwrap(), [0.0, 10.0, 20.0], "the video ends first");
    assert_eq!(sample_times(100.0, 4, Sampling::EveryK(0.0)).unwrap(), [0.0, 0.5, 1.0, 1.5], "a step under half a second is half a second");
    assert_eq!(sample_times(0.2, 4, Sampling::EveryK(5.0)).unwrap(), [0.0]);
    assert!(sample_times(0.0, 4, Sampling::Even).is_err() && sample_times(f64::NAN, 4, Sampling::Even).is_err());
    assert_eq!(clock_text(65.0), "1:05");
    let q = frames_question("What happens?", &[0.0, 65.0]);
    assert!(q.starts_with("These 2 pictures are frames from one video, in order, taken at 0:00, 1:05.") && q.ends_with("What happens?"), "{q}");
}

#[test]
fn the_ffmpeg_call_is_fixed_arguments_with_the_file_never_read_as_an_option() {
    let a = frame_args("/home/u/clip.mp4", 12.5, 640, 360);
    assert_eq!(a.join(" "), "-hide_banner -nostdin -v error -ss 12.500 -i /home/u/clip.mp4 -frames:v 1 -an -sn -vf scale=640:360 -f rawvideo -pix_fmt rgba -");
    assert!(frame_args("-evil.mp4", 0.0, 2, 2).contains(&"./-evil.mp4".to_string()));
    assert_eq!(frame_size((1920, 1080)), (768, 432));
    assert_eq!(frame_size((320, 240)), (768, 576), "small videos are fitted to the same box (the decoder scales up)");
}

struct Frames {
    ok: bool,
    asked: Mutex<Vec<(f64, (u32, u32))>>,
}

impl FrameSource for Frames {
    fn available(&self) -> Result<(), String> {
        if self.ok { Ok(()) } else { Err(NO_FFMPEG.to_string()) }
    }
    fn info(&self, _f: &str) -> Result<VideoInfo, String> {
        self.available().map(|_| VideoInfo { duration: 40.0, size: (640, 360) })
    }
    fn frame_rgba(&self, _f: &str, t: f64, size: (u32, u32), _c: &Arc<AtomicBool>) -> Result<Vec<u8>, String> {
        self.asked.lock().unwrap().push((t, size));
        Ok(vec![(t as u8).wrapping_mul(5); size.0 as usize * size.1 as usize * 4])
    }
}

#[test]
fn frames_are_taken_in_order_with_progress_and_stop_when_cancelled() {
    let src = Frames { ok: true, asked: Default::default() };
    let info = src.info("a.mp4").unwrap();
    let times = sample_times(info.duration, 4, Sampling::Even).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let mut seen = Vec::new();
    let frames = extract(&src, "a.mp4", &info, &times, &cancel, &mut |d, n| seen.push((d, n))).unwrap();
    assert_eq!(frames.iter().map(|f| f.time).collect::<Vec<_>>(), times);
    assert_eq!(seen, [(1, 4), (2, 4), (3, 4), (4, 4)]);
    assert!(frames.iter().all(|f| f.png.starts_with(&[0x89, b'P', b'N', b'G']) && (f.w, f.h) == (768, 432)));
    // cancelled after the second frame
    let src = Frames { ok: true, asked: Default::default() };
    let c2 = cancel.clone();
    let mut n = 0;
    let r = extract(&src, "a.mp4", &info, &times, &cancel, &mut |_, _| {
        n += 1;
        if n == 2 {
            c2.store(true, Ordering::SeqCst);
        }
    });
    assert_eq!(r.unwrap_err(), "cancelled");
    assert_eq!(src.asked.lock().unwrap().len(), 2, "no frame is taken after the cancel");
}

#[test]
fn without_ffmpeg_the_source_says_what_is_missing_and_where_to_get_it() {
    let src = Frames { ok: false, asked: Default::default() };
    let e = src.available().unwrap_err();
    assert!(e.contains("ffmpeg is not installed") && e.contains("Store") && e.contains("Media player (ffmpeg)") && e.contains("does not ship it"), "{e}");
    assert!(src.info("a.mp4").is_err());
    // the real source on a machine without one answers the same way (or works when there is one: either is honest)
    match Ffmpeg.available() {
        Ok(()) => {}
        Err(e) => assert_eq!(e, NO_FFMPEG),
    }
}

#[test]
fn the_history_is_capped_by_count_and_by_bytes_and_keeps_the_newest() {
    assert_eq!(over_cap(&[]), 0);
    assert_eq!(over_cap(&[10; HISTORY_MAX_ITEMS]), 0);
    assert_eq!(over_cap(&[10; HISTORY_MAX_ITEMS + 3]), 3, "three too many results");
    let big = HISTORY_MAX_BYTES / 2 + 1;
    assert_eq!(over_cap(&[big, big, big]), 2, "bytes: only the newest fits");
    assert_eq!(over_cap(&[HISTORY_MAX_BYTES * 2]), 0, "a single huge result is still kept");
    assert_eq!(over_cap(&[big, big, 1]), 1, "the oldest goes, the rest fit");
    assert_eq!(over_cap(&[1, big, 1]), 0);
}

#[test]
fn the_fields_are_bounded_and_the_seed_is_a_number_or_made() {
    assert_eq!((bound_side("512", 1), bound_side("5", 1), bound_side("99999", 1), bound_side("abc", 1000), bound_side(" 1023 ", 1)), (512, 64, 2048, 1000, 1016));
    assert_eq!((bound_steps("20"), bound_steps("0"), bound_steps("9999"), bound_steps(""), bound_steps("x")), (Some(20), Some(1), Some(100), None, None));
    assert_eq!((parse_seed(" 42 "), parse_seed(""), parse_seed("x"), parse_seed("-3")), (Some(42), None, None, None));
    assert_ne!(fresh_seed(1), fresh_seed(2));
    assert!(fresh_seed(123456789) < 4_000_000_000);
    assert_eq!(clean_text("  a\u{7}b\n c  "), "ab\n c");
    assert_eq!(clean_text(&"x".repeat(MAX_TEXT + 50)).chars().count(), MAX_TEXT);
    assert_eq!((ms_text(340), ms_text(1234), ms_text(45_000)), ("340 ms".into(), "1.2 s".into(), "45 s".into()));
    assert_eq!((bytes_text(812), bytes_text(34 * 1024), bytes_text(3 << 19)), ("812 B".into(), "34 KB".into(), "1.5 MB".into()));
}

fn png_b64() -> String {
    iyakki_flow::report::base64(&crate::browserjobs::png::encode_rgba(4, 4, &[200u8; 64]))
}

#[test]
fn the_picture_backend_sends_the_seed_the_size_and_a_negative_prompt_only_to_a_server_that_takes_it() {
    let b64 = png_b64();
    let (svc, seen) = services("pic", move |u| if u.contains("/images/generations") { (200, format!(r#"{{"data":[{{"b64_json":"{b64}"}}]}}"#)) } else { (200, "{}".into()) });
    add_lemonade(&svc, "home", "http://127.0.0.1:13397", &lemonade_list());
    let be = ServicesBackend(svc.clone());
    let req = PictureRequest { provider: "home".into(), model: "SD-Turbo".into(), prompt: "a red kite".into(), negative: Some("blurry".into()), width: 768, height: 768, steps: None, seed: 77 };
    let (res, trace) = be.draw(&req);
    let (bytes, ext) = res.unwrap();
    assert!(ext == "png" && bytes.starts_with(&[0x89, b'P']));
    let body: serde_json::Value = serde_json::from_str(&seen.lock().unwrap().iter().find(|c| c.1.contains("/images/")).unwrap().3).unwrap();
    assert_eq!((body["seed"].as_u64(), body["negative_prompt"].as_str(), body["steps"].as_u64()), (Some(77), Some("blurry"), Some(4)));
    assert_eq!(body["size"], "768x768", "the size asked for is the size sent");
    let e = picture_entry(&req, &trace, &Ok((bytes.clone(), ext)), 1_700_000_000);
    assert_eq!((e.status, e.tab.as_str(), e.provider.as_str()), (Some(200), "picture", "home"));
    assert!(e.request.contains("768x768 seed 77 +negative") && e.request.contains("a red kite"));
    assert!(!e.notes.iter().any(|n| n.contains("model's own size")), "nothing was overridden: {:?}", e.notes);
    assert!(e.notes.iter().any(|n| n.contains("4 steps (the model's own)")), "{:?}", e.notes);
    // steps asked for are sent and logged as asked for
    let req2 = PictureRequest { steps: Some(8), ..req.clone() };
    let (res2, trace2) = be.draw(&req2);
    let body2: serde_json::Value = serde_json::from_str(&seen.lock().unwrap().iter().rev().find(|c| c.1.contains("/images/")).unwrap().3).unwrap();
    assert_eq!(body2["steps"].as_u64(), Some(8));
    assert!(picture_entry(&req2, &trace2, &res2, 1).notes.iter().any(|n| n == "8 steps (asked for)"));
    // when a server draws at its own size anyway, the log says so
    let mut forced = trace.clone();
    (forced.width, forced.height) = (512, 512);
    assert!(picture_entry(&req, &forced, &Ok((bytes, ext)), 1).notes.iter().any(|n| n.contains("model's own size 512x512, not the 768x768")));
    // a failure keeps what the server said and its status
    let (svc2, _) = services("pic2", |_| (500, r#"{"error":{"message":"out of memory"}}"#.into()));
    add_lemonade(&svc2, "home", "http://127.0.0.1:13398", &lemonade_list());
    let (res, trace) = ServicesBackend(svc2).draw(&req);
    let e = picture_entry(&req, &trace, &res, 1);
    assert_eq!(e.status, Some(500));
    assert!(e.error.contains("out of memory"), "{e:?}");
}

#[test]
fn the_vision_backend_sends_the_picture_reads_the_tokens_and_never_logs_the_key() {
    std::env::set_var("IYAKKI_LAB_TEST_KEY", "sk-lab-SECRET-9999");
    let (svc, seen) = services("vis", |u| {
        if u.ends_with("/chat/completions") {
            (200, r#"{"choices":[{"message":{"content":"A red square."}}],"usage":{"prompt_tokens":120,"completion_tokens":5}}"#.into())
        } else {
            (200, "{}".into())
        }
    });
    let mut p = ProviderCfg::new("oa", Kind::Compat);
    p.base_url = Some("http://127.0.0.1:13399/v1".into());
    p.key_ref = Some("env:IYAKKI_LAB_TEST_KEY".into());
    p.models = vec!["gpt-4o".into()];
    svc.settings.lock().unwrap().upsert_provider(p).unwrap();
    let be = ServicesBackend(svc.clone());
    let req = VisionRequest { provider: "oa".into(), model: "gpt-4o".into(), question: "What is this?".into(), images: vec![crate::browserjobs::png::encode_rgba(4, 4, &[255u8; 64])] };
    let cancel = AtomicBool::new(false);
    let mut pieces = Vec::new();
    let (res, trace) = be.ask(&req, &cancel, &mut |t| pieces.push(t.to_string()));
    let a = res.unwrap();
    assert_eq!((a.text.as_str(), a.tokens, a.streamed), ("A red square.", Some((120, 5)), false), "no streaming connection here: one piece");
    assert!(trace.notes.iter().any(|n| n.contains("one piece")));
    let call = seen.lock().unwrap().iter().find(|c| c.1.ends_with("/chat/completions")).cloned().unwrap();
    assert!(call.3.contains("image_url") && call.3.contains("data:image/png;base64,"), "the picture goes as an image_url data address");
    let e = vision_entry(&req, &trace, &Ok(a), 5);
    assert!(e.notes.iter().any(|n| n == "120 tokens in, 5 out") && e.status == Some(200), "{e:?}");
    assert!(e.request.starts_with("1 picture (") && e.request.contains("What is this?"));
    let line = e.line(&crate::clock::Zone::system());
    assert!(!line.contains("SECRET") && !line.contains("Bearer") && !line.contains("sk-lab"), "{line}");
    // the server echoing the key in an error does not put it in the log either
    let (svc2, _) = services("vis2", |_| (400, r#"{"error":{"message":"bad key sk-lab-SECRET-9999, and this model does not support images"}}"#.into()));
    let mut p2 = svc2.settings.lock().unwrap().get().providers.first().cloned().unwrap_or_else(|| ProviderCfg::new("oa", Kind::Compat));
    p2 = ProviderCfg { key_ref: Some("env:IYAKKI_LAB_TEST_KEY".into()), base_url: Some("http://127.0.0.1:13400/v1".into()), ..p2 };
    svc2.settings.lock().unwrap().upsert_provider(p2).unwrap();
    let (res, trace) = ServicesBackend(svc2).ask(&req, &cancel, &mut |_| {});
    let msg = res.unwrap_err();
    assert!(msg.contains("cannot take pictures") && !msg.contains("SECRET"), "{msg}");
    assert_eq!(trace.status, Some(400));
    assert!(!trace.error.contains("SECRET"), "{}", trace.error);
    assert_eq!(status_of("HTTP 429: slow down"), Some(429));
    assert_eq!(status_of("nope"), None);
}

#[test]
fn the_log_keeps_the_newest_lines_and_copies_as_text() {
    let mut log = Log::default();
    assert!(log.is_empty() && log.text().is_empty());
    for i in 0..LOG_MAX + 5 {
        log.push(LogEntry { ts: 1_700_000_000 + i as u64, tab: "picture".into(), provider: "home".into(), model: format!("m{i}"), request: "512x512 seed 1".into(), status: Some(200), ms: 1500, bytes: 2048, ..Default::default() });
    }
    assert_eq!(log.len(), LOG_MAX);
    assert_eq!(log.entries().next().unwrap().model, "m5", "the oldest five went");
    let t = log.text();
    assert_eq!(t.lines().count(), LOG_MAX);
    assert!(t.lines().last().unwrap().contains("home / m304") && t.contains("HTTP 200") && t.contains("1.5 s") && t.contains("2 KB"));
    let bad = LogEntry { tab: "vision".into(), error: "HTTP 500: boom".into(), notes: vec!["downscaled".into()], ..Default::default() };
    let l = bad.line(&crate::clock::Zone::system());
    assert!(l.contains("no HTTP status") && l.contains("ERROR: HTTP 500: boom") && l.contains("[downscaled]"), "{l}");
    log.clear();
    assert!(log.is_empty());
    let v = video_entry("/home/u/Videos/a clip.mp4", 8, 3 << 20, 4200, "", 1);
    assert!(v.request == "8 frame(s) from a clip.mp4" && v.provider == "ffmpeg" && v.bytes == 3 << 20, "{v:?}");
}
