//! Lemonade against a fake Lemonade server on loopback (and a fake Ollama and a plain OpenAI-compatible server for
//! telling them apart): detection, typed models, provider creation that never overwrites, pictures through
//! `generate_image` and the TALES checks, the reading voice and its word timing, speech-to-text, a streamed chat with a
//! tool call, and load / pull.

use super::*;
use crate::providers::{Kind, ProviderCfg};
use crate::services::Services;
use std::io::{BufRead, BufReader};
use std::net::TcpListener;
use std::sync::Arc;

type Seen = Arc<Mutex<Vec<(String, String, String)>>>;

pub(crate) struct Fake {
    pub port: u16,
    pub seen: Seen,
}

impl Fake {
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}/api/v1", self.port)
    }

    fn bodies(&self, path: &str) -> Vec<String> {
        self.seen.lock().unwrap().iter().filter(|(_, p, _)| p.starts_with(path)).map(|(_, _, b)| b.clone()).collect()
    }
}

fn tmp(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("iyakki-lemonade-{}-{tag}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub(crate) fn png_b64() -> String {
    iyakki_flow::report::base64(&crate::qaimage::Image::blank(8, 8, [200, 60, 40, 255]).to_png())
}

pub(crate) fn models_json() -> String {
    serde_json::json!({"object": "list", "data": [
        {"id": "Qwen3-0.6B-GGUF", "object": "model", "owned_by": "lemonade", "checkpoint": "unsloth/Qwen3-0.6B-GGUF:Q4_0", "recipe": "llamacpp", "size": 0.38, "downloaded": true, "suggested": true, "labels": ["reasoning", "tool-calling"]},
        {"id": "Phi-4-Multimodal-GGUF", "owned_by": "lemonade", "recipe": "llamacpp", "size": 3.1, "downloaded": true, "labels": ["vision"]},
        {"id": "nomic-embed-text-v1-GGUF", "owned_by": "lemonade", "recipe": "llamacpp", "size": 0.0781, "downloaded": true, "labels": ["embeddings"]},
        {"id": "bge-reranker-v2-m3-GGUF", "owned_by": "lemonade", "recipe": "llamacpp", "size": 0.636, "downloaded": true, "labels": ["reranking"]},
        {"id": "SD-Turbo", "owned_by": "lemonade", "recipe": "sd-cpp", "size": 2.6, "downloaded": true, "labels": ["image"], "image_defaults": {"steps": 4, "cfg_scale": 1.0, "width": 512, "height": 512}},
        {"id": "SDXL-Base-1.0", "owned_by": "lemonade", "recipe": "sd-cpp", "size": 6.94, "downloaded": false, "labels": ["image"], "image_defaults": {"steps": 20, "width": 1024, "height": 1024}},
        {"id": "Whisper-Base", "owned_by": "lemonade", "recipe": "whispercpp", "size": 0.148, "downloaded": true, "labels": ["transcription", "realtime-transcription"]},
        {"id": "kokoro-v1", "owned_by": "lemonade", "checkpoint": "mikkoph/kokoro-onnx", "recipe": "kokoro", "size": 0.354, "downloaded": true, "labels": ["tts"]}
    ]})
    .to_string()
}

/// One second of tone at 16 kHz, as Lemonade's Kokoro answers with `response_format: wav`.
pub(crate) fn speech_wav() -> Vec<u8> {
    crate::voice::encode_wav(&crate::voice::tone(440.0, 1000), 16_000)
}

fn reply(method: &str, path: &str, kind: &str, body: &[u8]) -> (u16, &'static str, Vec<u8>) {
    let json = |s: String| (200, "application/json", s.into_bytes());
    if String::from_utf8_lossy(body).contains("No-Such-Model") {
        return (404, "application/json", br#"{"error":{"message":"model No-Such-Model not found","type":"not_found"}}"#.to_vec());
    }
    match (kind, method, path.split('?').next().unwrap_or("")) {
        ("lemonade", "GET", "/api/v1/health") => json(r#"{"status":"ok","version":"2026.42.1","model_loaded":"Qwen3-0.6B-GGUF","all_models_loaded":[{"model_name":"Qwen3-0.6B-GGUF","type":"llm","recipe":"llamacpp"}],"max_models":{"llm":1,"embedding":1,"reranking":1,"transcription":1,"image":1,"tts":1}}"#.into()),
        ("lemonade", "GET", "/api/v1/models") => json(models_json()),
        ("lemonade", "POST", "/api/v1/images/generations") => json(format!(r#"{{"created":1,"data":[{{"b64_json":"{}"}}]}}"#, png_b64())),
        ("lemonade", "POST", "/api/v1/audio/transcriptions") => json(r#"{"text":"hello lemonade [BLANK_AUDIO]"}"#.into()),
        ("lemonade", "POST", "/api/v1/audio/speech") => (200, "audio/wav", speech_wav()),
        ("lemonade", "POST", "/api/v1/embeddings") => json(r#"{"data":[{"embedding":[0.1,0.2,0.3]}]}"#.into()),
        ("lemonade", "POST", "/api/v1/load") => json(r#"{"status":"success","message":"Loaded model: SD-Turbo"}"#.into()),
        ("lemonade", "POST", "/api/v1/pull") => (200, "text/event-stream", b"event: progress\ndata: {\"file\":\"sd_xl_base_1.0.safetensors\",\"file_index\":1,\"total_files\":1,\"bytes_downloaded\":512,\"bytes_total\":1024,\"percent\":50}\n\nevent: complete\ndata: {\"file_index\":1,\"total_files\":1,\"percent\":100}\n\n".to_vec()),
        ("lemonade", "POST", "/api/v1/chat/completions") => (
            200,
            "text/event-stream",
            b"data: {\"choices\":[{\"delta\":{\"content\":\"Let me look.\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call_1\",\"function\":{\"name\":\"web_search\",\"arguments\":\"{\\\"q\\\":\"}}]}}]}\n\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"amd\\\"}\"}}]}}]}\n\ndata: [DONE]\n\n".to_vec(),
        ),
        ("ollama", "GET", "/api/version") => json(r#"{"version":"0.12.3"}"#.into()),
        ("ollama", "GET", "/api/tags") => json(r#"{"models":[{"name":"qwen3:8b"}]}"#.into()),
        ("ollama", "GET", "/v1/models") | ("compat", "GET", "/v1/models") => json(r#"{"object":"list","data":[{"id":"qwen3:8b"}]}"#.into()),
        _ => (404, "application/json", br#"{"error":{"message":"not found"}}"#.to_vec()),
    }
}

/// A fake server of `kind` (`lemonade`, `ollama`, `compat`) on a free loopback port.
pub(crate) fn serve(kind: &'static str) -> Fake {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Seen = Default::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let log = log.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                if r.read_line(&mut line).is_err() {
                    return;
                }
                let mut parts = line.split_whitespace();
                let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
                let mut len = 0usize;
                loop {
                    let mut h = String::new();
                    if r.read_line(&mut h).unwrap_or(0) == 0 || h.trim().is_empty() {
                        break;
                    }
                    if let Some((k, v)) = h.split_once(':') {
                        if k.eq_ignore_ascii_case("content-length") {
                            len = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
                let mut body = vec![0u8; len];
                let _ = std::io::Read::read_exact(&mut r, &mut body);
                log.lock().unwrap().push((method.clone(), path.clone(), String::from_utf8_lossy(&body).into_owned()));
                let (status, ctype, out) = reply(&method, &path, kind, &body);
                let _ = s.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out.len()).as_bytes());
                let _ = s.write_all(&out);
            });
        }
    });
    Fake { port, seen }
}

fn real_services(tag: &str) -> (Arc<Services>, std::path::PathBuf) {
    let d = tmp(tag);
    let up: Arc<dyn Upstream> = Arc::new(iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(10)));
    let s = Services::open(&d, up.clone(), up).unwrap();
    s.set_streamer(Arc::new(iyakki_proxy::https::UreqStreamer::default()));
    (s, d)
}

fn unused_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

#[test]
fn lemonade_ollama_and_a_plain_compatible_server_are_told_apart() {
    let (lem, oll, compat) = (serve("lemonade"), serve("ollama"), serve("compat"));
    let up = iyakki_proxy::https::UreqUpstream::with_timeout(Duration::from_secs(5));
    let l = detect(&format!("http://127.0.0.1:{}", lem.port), &up, &[]).unwrap();
    assert_eq!(l, Server::Lemonade { base: lem.base(), version: Some("2026.42.1".into()) });
    assert_eq!(l.found(), format!("Lemonade found at {} (version 2026.42.1)", lem.base()));
    assert_eq!(detect(&lem.base(), &up, &[]).unwrap().kind(), Kind::Lemonade, "the API address works too");
    assert_eq!(detect(&format!("http://127.0.0.1:{}", oll.port), &up, &[]).unwrap(), Server::Ollama { base: format!("http://127.0.0.1:{}", oll.port), version: Some("0.12.3".into()) });
    assert_eq!(detect(&format!("http://127.0.0.1:{}", compat.port), &up, &[]).unwrap(), Server::Compat { base: format!("http://127.0.0.1:{}/v1", compat.port) });
    assert!(detect(&format!("http://127.0.0.1:{}", unused_port()), &up, &[]).unwrap_err().contains("no Ollama, Lemonade"));
    // only GET requests were made
    assert!(lem.seen.lock().unwrap().iter().chain(oll.seen.lock().unwrap().iter()).all(|(m, _, _)| m == "GET"));
    // the quick probe of this machine's ports
    assert_eq!(probe_local(lem.port), Some((lem.base(), Some("2026.42.1".into()))));
    assert_eq!(probe_local(oll.port), None, "Ollama is not Lemonade");
    assert_eq!(probe_local(unused_port()), None);
}

#[test]
fn detection_adds_lemonade_local_once_with_its_typed_models_and_never_overwrites_the_persons_providers() {
    let lem = serve("lemonade");
    let (svc, d) = real_services("detect");
    let msgs = svc.detect_local_servers_at(unused_port(), &[unused_port(), lem.port]);
    assert_eq!(msgs, vec![format!("Lemonade found at {} (version 2026.42.1); added to Models as lemonade-local.", lem.base())]);
    let p = svc.settings.lock().unwrap().get().provider("lemonade-local").cloned().unwrap();
    assert_eq!((p.kind, p.base_url.as_deref()), (Kind::Lemonade, Some(lem.base().as_str())));
    // the chat list holds the downloaded chat and vision models; every model keeps its type
    assert_eq!(p.models, vec!["Qwen3-0.6B-GGUF", "Phi-4-Multimodal-GGUF"]);
    let kinds: Vec<(&str, &str)> = p.model_kinds.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    for want in [("SD-Turbo", "image"), ("SDXL-Base-1.0", "image"), ("Whisper-Base", "stt"), ("kokoro-v1", "tts"), ("nomic-embed-text-v1-GGUF", "embeddings"), ("bge-reranker-v2-m3-GGUF", "reranking"), ("Phi-4-Multimodal-GGUF", "vision")] {
        assert!(kinds.contains(&want), "{want:?} in {kinds:?}");
    }
    assert!(crate::providers::model_sees_pictures("Phi-4-Multimodal-GGUF"), "the vision label flags it for the screenshot node");
    let cat = svc.lemonade_cached("lemonade-local");
    assert_eq!(cat.iter().find(|m| m.id == "Qwen3-0.6B-GGUF").unwrap().status(), "loaded");
    assert_eq!(svc.lemonade_models(ModelType::Image), vec![("lemonade-local".to_string(), "SD-Turbo".to_string())], "not-downloaded SDXL is not offered");
    // a second look adds nothing
    let again = svc.detect_local_servers_at(unused_port(), &[lem.port]);
    assert!(again[0].ends_with("already in Models as lemonade-local."), "{again:?}");
    assert_eq!(svc.settings.lock().unwrap().get().providers.len(), 1);
    // the person renames it and changes its address: a new detection never touches it, and a provider called lemonade-local
    // that points elsewhere is kept too
    svc.settings.lock().unwrap().update(|s| s.providers[0].base_url = Some("http://10.0.0.9:13305/api/v1".into())).unwrap();
    let third = svc.detect_local_servers_at(unused_port(), &[lem.port]);
    assert!(third[0].contains(&format!("added to Models as lemonade-local-{}", lem.port)), "{third:?}");
    let ps = svc.settings.lock().unwrap().get().providers.clone();
    assert_eq!(ps.iter().find(|p| p.id == "lemonade-local").unwrap().base_url.as_deref(), Some("http://10.0.0.9:13305/api/v1"));
    // a provider the person made at this address (any name) means nothing is added
    let mut mine = ProviderCfg::new("amd-box", Kind::Compat);
    mine.base_url = Some(lem.base());
    let (svc2, d2) = real_services("detect2");
    svc2.settings.lock().unwrap().upsert_provider(mine.clone()).unwrap();
    let m = svc2.detect_local_servers_at(unused_port(), &[lem.port]);
    assert!(m[0].ends_with("already in Models as amd-box."), "{m:?}");
    assert_eq!(svc2.settings.lock().unwrap().get().providers, vec![mine]);
    let _ = (std::fs::remove_dir_all(d), std::fs::remove_dir_all(d2));
}

#[test]
fn pictures_go_through_generate_image_at_the_models_own_size_and_pass_the_tales_checks() {
    let lem = serve("lemonade");
    let (svc, d) = real_services("image");
    svc.use_local_lemonade(&lem.base(), None);
    assert!(svc.image_models().contains(&("lemonade-local".to_string(), "SD-Turbo".to_string())), "found by type through the live list");
    let (provider, model, bytes, ext) = svc.generate_image(None, "a fox in the snow", "1024x1024").unwrap();
    assert_eq!((provider.as_str(), model.as_str(), ext), ("lemonade-local", "SD-Turbo", "png"));
    assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
    let body: Value = serde_json::from_str(&lem.bodies("/api/v1/images/generations")[0]).unwrap();
    assert_eq!((body["model"].as_str(), body["size"].as_str(), body["steps"].as_u64(), body["response_format"].as_str()), (Some("SD-Turbo"), Some("512x512"), Some(4), Some("b64_json")));
    // the TALES picture pipeline judges the bytes themselves (magic bytes, a real decode)
    let f = crate::scene::reply::validate(bytes, false, "the picture model").unwrap();
    assert_eq!(f.ext, "png");
    // a model that is not on the server gets a message that says to pull it
    let e = svc.generate_image(Some(("lemonade-local", "No-Such-Model")), "x", "512x512").unwrap_err();
    assert!(e.contains("pull it first"), "{e}");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn kokoro_reads_aloud_with_a_wav_whose_length_drives_the_word_timing_and_whisper_listens() {
    let lem = serve("lemonade");
    let (svc, d) = real_services("voice");
    svc.use_local_lemonade(&lem.base(), None);
    svc.refresh_lemonade("lemonade-local").unwrap();
    assert!(svc.use_lemonade_voice("lemonade-local", "kokoro-v1", Some("af_heart")).unwrap().contains("kokoro-v1"));
    let text = "The fox ran over the hill.";
    let wav = svc.synthesize_text(text).unwrap();
    let body: Value = serde_json::from_str(&lem.bodies("/api/v1/audio/speech")[0]).unwrap();
    assert_eq!((body["model"].as_str(), body["voice"].as_str(), body["response_format"].as_str()), (Some("kokoro-v1"), Some("af_heart"), Some("wav")));
    let w = crate::voice::parse_wav(&wav).unwrap();
    let ms = w.samples.len() as f32 * 1000.0 / w.rate as f32 / w.channels as f32;
    assert!((ms - 1000.0).abs() < 5.0, "{ms}");
    let tl = crate::wordtime::WordTimeline::fitted(text, ms);
    assert!(tl.is_fitted() && (tl.total_ms() - ms).abs() < 1.0, "the words are spread over the real length");
    // the voice is offered as a one-click engine too
    let ps = svc.settings.lock().unwrap().get().providers.clone();
    let quick = crate::story::quick_engines(&ps, "", &[]);
    assert!(quick.iter().any(|q| q.label == "Use kokoro-v1 on lemonade-local"), "{:?}", quick.iter().map(|q| &q.label).collect::<Vec<_>>());
    // speech-to-text
    svc.use_lemonade_stt("lemonade-local", "Whisper-Base").unwrap();
    assert_eq!(svc.transcribe_samples(&crate::voice::tone(300.0, 500)).unwrap(), "hello lemonade");
    let sent = &lem.bodies("/api/v1/audio/transcriptions")[0];
    assert!(sent.contains("name=\"model\"") && sent.contains("Whisper-Base") && sent.contains("RIFF"));
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn chat_streams_with_a_tool_call_embeddings_work_and_load_and_pull_reach_the_server() {
    let lem = serve("lemonade");
    let (svc, d) = real_services("chat");
    svc.use_local_lemonade(&lem.base(), None);
    let c = svc.client_for("lemonade-local", false).unwrap();
    let tools = vec![crate::chat::ToolSpec { name: "web_search".into(), description: "search".into(), schema: serde_json::json!({"type": "object"}) }];
    let mut shown = String::new();
    let turn = c.chat_turn_stream("Qwen3-0.6B-GGUF", &[crate::chat::Message::user("news about AMD?")], &tools, &iyakki_proxy::https::UreqStreamer::default(), &mut |t| {
        shown.push_str(t);
        true
    })
    .unwrap();
    assert_eq!((turn.text.as_str(), shown.as_str()), ("Let me look.", "Let me look."));
    assert_eq!((turn.calls[0].name.as_str(), turn.calls[0].args.clone()), ("web_search", serde_json::json!({"q": "amd"})));
    let sent: Value = serde_json::from_str(&lem.bodies("/api/v1/chat/completions")[0]).unwrap();
    assert_eq!((sent["stream"].as_bool(), sent["tools"][0]["function"]["name"].as_str()), (Some(true), Some("web_search")));
    assert_eq!(c.embed("nomic-embed-text-v1-GGUF", &["hi".to_string()]).unwrap(), vec![vec![0.1f32, 0.2, 0.3]]);
    // load into memory, then a download with progress
    assert_eq!(svc.lemonade_load("lemonade-local", "SD-Turbo", true).unwrap(), "Loaded model: SD-Turbo");
    assert_eq!(lem.bodies("/api/v1/load")[0], r#"{"model_name":"SD-Turbo"}"#);
    svc.pull_model("lemonade-local", "SDXL-Base-1.0").unwrap();
    for _ in 0..400 {
        if svc.pull_status().iter().any(|p| p.done || p.error.is_some()) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let st = svc.pull_status();
    assert!(st[0].done && st[0].error.is_none(), "{st:?}");
    let pulled: Value = serde_json::from_str(&lem.bodies("/api/v1/pull")[0]).unwrap();
    assert_eq!((pulled["model_name"].as_str(), pulled["stream"].as_bool()), (Some("SDXL-Base-1.0"), Some(true)));
    let _ = std::fs::remove_dir_all(d);
}
