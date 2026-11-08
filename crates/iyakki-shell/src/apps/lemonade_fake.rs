//! For tests: a fake AMD Lemonade Server on a free loopback port (health, a typed model list with every kind of model,
//! pictures, speech, transcription, load and a streamed pull). Every request is logged as (method, path, body).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

pub(crate) type Seen = Arc<Mutex<Vec<(String, String, String)>>>;

pub(crate) struct FakeLemonade {
    pub port: u16,
    pub seen: Seen,
}

impl FakeLemonade {
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}/api/v1", self.port)
    }

    pub fn bodies(&self, path: &str) -> Vec<String> {
        self.seen.lock().unwrap().iter().filter(|(_, p, _)| p.starts_with(path)).map(|(_, _, b)| b.clone()).collect()
    }
}

pub(crate) const MODELS: &str = r#"{"object":"list","data":[
 {"id":"Qwen3-0.6B-GGUF","owned_by":"lemonade","recipe":"llamacpp","size":0.38,"downloaded":true,"labels":["reasoning"]},
 {"id":"Phi-4-Multimodal-GGUF","owned_by":"lemonade","recipe":"llamacpp","size":3.1,"downloaded":true,"labels":["vision"]},
 {"id":"nomic-embed-text-v1-GGUF","owned_by":"lemonade","recipe":"llamacpp","size":0.0781,"downloaded":true,"labels":["embeddings"]},
 {"id":"bge-reranker-v2-m3-GGUF","owned_by":"lemonade","recipe":"llamacpp","size":0.636,"downloaded":true,"labels":["reranking"]},
 {"id":"SD-Turbo","owned_by":"lemonade","recipe":"sd-cpp","size":2.6,"downloaded":true,"labels":["image"],"image_defaults":{"steps":4,"width":512,"height":512}},
 {"id":"SDXL-Base-1.0","owned_by":"lemonade","recipe":"sd-cpp","size":6.94,"downloaded":false,"labels":["image"],"image_defaults":{"steps":20,"width":1024,"height":1024}},
 {"id":"Whisper-Base","owned_by":"lemonade","recipe":"whispercpp","size":0.148,"downloaded":true,"labels":["transcription"]},
 {"id":"kokoro-v1","owned_by":"lemonade","recipe":"kokoro","size":0.354,"downloaded":true,"labels":["tts"]}
]}"#;

fn reply(method: &str, path: &str, body: &str) -> (u16, &'static str, Vec<u8>) {
    let json = |s: String| (200, "application/json", s.into_bytes());
    match (method, path.split('?').next().unwrap_or("")) {
        ("GET", "/api/v1/health") => json(r#"{"status":"ok","version":"2026.42.1","model_loaded":"Qwen3-0.6B-GGUF","all_models_loaded":[{"model_name":"Qwen3-0.6B-GGUF","type":"llm"}],"max_models":{"llm":1}}"#.into()),
        ("GET", "/api/v1/models") => json(MODELS.into()),
        ("POST", "/api/v1/images/generations") => json(format!(r#"{{"created":1,"data":[{{"b64_json":"{}"}}]}}"#, iyakki_flow::report::base64(&iyakki_agent::qaimage::Image::blank(8, 8, [30, 140, 90, 255]).to_png()))),
        ("POST", "/api/v1/audio/speech") => (200, "audio/wav", iyakki_agent::voice::encode_wav(&iyakki_agent::voice::tone(440.0, 800), 16_000)),
        ("POST", "/api/v1/audio/transcriptions") => json(r#"{"text":"hello lemonade"}"#.into()),
        // a tidy request answers with the transcript it quotes, capitalised and ended with a full stop (or "Hello, Lemonade." for the canned one)
        ("POST", "/api/v1/chat/completions") => {
            let said = if body.contains("turn on the lights") { "Turn on the lights." } else { "Hello, Lemonade." };
            json(format!(r#"{{"id":"c1","object":"chat.completion","choices":[{{"index":0,"message":{{"role":"assistant","content":"{said}"}},"finish_reason":"stop"}}]}}"#))
        }
        ("POST", "/api/v1/load") => json(r#"{"status":"success","message":"Loaded model: SD-Turbo"}"#.into()),
        ("POST", "/api/v1/pull") => (200, "text/event-stream", b"event: progress\ndata: {\"file\":\"sdxl.safetensors\",\"bytes_downloaded\":5,\"bytes_total\":10,\"percent\":50}\n\nevent: complete\ndata: {\"file_index\":1,\"total_files\":1,\"percent\":100}\n\n".to_vec()),
        _ => (404, "application/json", br#"{"error":{"message":"not found"}}"#.to_vec()),
    }
}

pub(crate) fn serve() -> FakeLemonade {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let seen: Seen = Default::default();
    let log = seen.clone();
    std::thread::spawn(move || {
        for mut s in l.incoming().flatten() {
            let log = log.clone();
            std::thread::spawn(move || {
                let mut r = BufReader::new(s.try_clone().unwrap());
                let mut line = String::new();
                if r.read_line(&mut line).is_err() {
                    return;
                }
                let mut it = line.split_whitespace();
                let (method, path) = (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string());
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
                let _ = r.read_exact(&mut body);
                log.lock().unwrap().push((method.clone(), path.clone(), String::from_utf8_lossy(&body).into_owned()));
                let (status, ctype, out) = reply(&method, &path, &String::from_utf8_lossy(&body));
                let _ = s.write_all(format!("HTTP/1.1 {status} X\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", out.len()).as_bytes());
                let _ = s.write_all(&out);
            });
        }
    });
    FakeLemonade { port, seen }
}
