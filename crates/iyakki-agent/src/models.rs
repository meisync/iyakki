//! Finding models that draw pictures, downloading models into an Ollama server, and reading what a picture model
//! answers with. Whether a model draws is judged by its name (the servers do not say), so the list of names is short and
//! easy to extend.

use serde_json::Value;

/// Name parts of models that make pictures from text.
const IMAGE_NAMES: [&str; 22] = [
    "qwen-image", "qwen_image", "qwenimage", "flux", "stable-diffusion", "stablediffusion", "sdxl", "sd3", "sd-turbo", "dall-e", "dalle", "gpt-image", "imagen", "z-image", "hunyuan-image", "hidream", "kolors", "pixart", "sana", "text-to-image", "image-gen", "janus",
];

pub fn is_image_model(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    IMAGE_NAMES.iter().any(|p| n.contains(p)) || n.ends_with("-t2i") || n.contains("/t2i")
}

pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            b'=' | b'\n' | b'\r' | b' ' => continue,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The picture inside a model's answer: Ollama's `image` / `images`, or the OpenAI-style `data[0].b64_json`.
pub fn image_from_response(v: &Value) -> Option<Vec<u8>> {
    let s = v.get("image").and_then(Value::as_str).or_else(|| v.pointer("/images/0").and_then(Value::as_str)).or_else(|| v.pointer("/message/images/0").and_then(Value::as_str)).or_else(|| v.pointer("/data/0/b64_json").and_then(Value::as_str))?;
    b64_decode(s.split_once("base64,").map_or(s, |(_, b)| b))
}

/// The bytes with the right file extension, if they are a PNG, JPEG, GIF or WebP.
pub fn sniff_image(b: Vec<u8>) -> Option<(Vec<u8>, &'static str)> {
    let ext = if b.starts_with(&[0x89, b'P', b'N', b'G']) {
        "png"
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "jpg"
    } else if b.starts_with(b"GIF8") {
        "gif"
    } else if b.len() > 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        "webp"
    } else {
        return None;
    };
    Some((b, ext))
}

/// What Ollama says as it downloads: one JSON object per line.
///
/// Lemonade's download speaks server-sent events instead (`event: progress` then `data: {"file":…,"bytes_downloaded":…,
/// "bytes_total":…,"percent":…}`, `event: complete`, `event: error` with `data: {"error":…}`); its `data:` lines are read here
/// too, and its plain answer (`{"status":"success"}`) is the end.
pub fn pull_line(line: &str) -> Option<PullLine> {
    let t = line.trim();
    let t = t.strip_prefix("data:").map_or(t, str::trim);
    let v: Value = serde_json::from_str(t).ok()?;
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Some(PullLine::Error(e.to_string()));
    }
    if v.get("status").and_then(Value::as_str) == Some("error") {
        return Some(PullLine::Error(v.get("message").and_then(Value::as_str).unwrap_or("the server refused").to_string()));
    }
    if v.get("bytes_total").is_some() || v.get("bytes_downloaded").is_some() {
        let file = v.get("file").and_then(Value::as_str).unwrap_or("model");
        let files = match (v.get("file_index").and_then(Value::as_u64), v.get("total_files").and_then(Value::as_u64)) {
            (Some(i), Some(n)) if n > 1 => format!(" (file {i} of {n})"),
            _ => String::new(),
        };
        return Some(PullLine::Progress { status: format!("downloading {file}{files}"), completed: v.get("bytes_downloaded").and_then(Value::as_u64).unwrap_or(0), total: v.get("bytes_total").and_then(Value::as_u64).unwrap_or(0) });
    }
    if v.get("status").is_none() && v.get("percent").and_then(Value::as_f64).is_some_and(|p| p >= 100.0) {
        return Some(PullLine::Done);
    }
    let status = v.get("status").and_then(Value::as_str)?.to_string();
    if status == "success" {
        return Some(PullLine::Done);
    }
    Some(PullLine::Progress { status, completed: v.get("completed").and_then(Value::as_u64).unwrap_or(0), total: v.get("total").and_then(Value::as_u64).unwrap_or(0) })
}

#[derive(Debug, PartialEq, Eq)]
pub enum PullLine {
    Progress { status: String, completed: u64, total: u64 },
    Done,
    Error(String),
}

/// A model name Ollama can be asked for: letters, digits and `. _ : - /`.
pub fn valid_pull_name(s: &str) -> bool {
    crate::providers::valid_model_name(s) && !s.contains('@') && !s.contains('+') && !s.starts_with('/') && !s.starts_with('-')
}

/// A friendlier message for the usual reasons a download fails.
pub fn explain_pull_error(model: &str, e: &str) -> String {
    let l = e.to_ascii_lowercase();
    if l.contains("file does not exist") || l.contains("not found") || l.contains("404") {
        format!("`{model}` was not found in the Ollama library (the server said: {e}). Check the exact name on ollama.com/library, or a publisher's name such as user/model.")
    } else if l.contains("disk") || l.contains("space") {
        format!("the server has no room for `{model}`: {e}")
    } else {
        format!("the download of `{model}` failed: {e}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picture_models_are_told_apart_by_name() {
        for m in ["qwen-image:latest", "Qwen-Image-2.1", "x/flux2-klein", "stable-diffusion-xl", "dall-e-3", "gpt-image-1", "x/z-image-turbo", "SDXL-turbo", "hidream-i1", "my/t2i", "janus-pro-7b"] {
            assert!(is_image_model(m), "{m}");
        }
        for m in ["qwen3:8b", "llama3.2", "gemma3:27b", "qwen2.5vl:7b", "nomic-embed-text", "gpt-4o", "mistral"] {
            assert!(!is_image_model(m), "{m}");
        }
    }

    #[test]
    fn base64_and_picture_answers_are_read_in_every_shape() {
        assert_eq!(b64_decode("aGVsbG8=").unwrap(), b"hello");
        assert_eq!(b64_decode("aGVs\nbG8gd29ybGQ").unwrap(), b"hello world");
        assert_eq!(b64_decode("").unwrap(), b"");
        assert!(b64_decode("a*b").is_none());
        let png = [0x89u8, b'P', b'N', b'G', 1, 2, 3, 4];
        let b64 = |b: &[u8]| {
            const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
            let mut s = String::new();
            for c in b.chunks(3) {
                let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
                s.push(T[(n >> 18) as usize & 63] as char);
                s.push(T[(n >> 12) as usize & 63] as char);
                s.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
                s.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
            }
            s
        };
        for body in [serde_json::json!({"image": b64(&png)}), serde_json::json!({"images": [b64(&png)]}), serde_json::json!({"data": [{"b64_json": b64(&png)}]}), serde_json::json!({"image": format!("data:image/png;base64,{}", b64(&png))}), serde_json::json!({"message": {"role": "assistant", "images": [b64(&png)]}})] {
            let got = image_from_response(&body).unwrap();
            assert_eq!(got, png);
            assert_eq!(sniff_image(got).unwrap().1, "png");
        }
        assert!(image_from_response(&serde_json::json!({"response": "hi"})).is_none());
        assert!(sniff_image(b"not a picture".to_vec()).is_none());
        assert_eq!(sniff_image(vec![0xFF, 0xD8, 0xFF, 0xE0]).unwrap().1, "jpg");
    }

    #[test]
    fn download_progress_lines_and_errors_are_understood() {
        assert_eq!(pull_line("{\"status\":\"pulling manifest\"}"), Some(PullLine::Progress { status: "pulling manifest".into(), completed: 0, total: 0 }));
        assert_eq!(pull_line("{\"status\":\"pulling abc\",\"total\":1000,\"completed\":250}"), Some(PullLine::Progress { status: "pulling abc".into(), completed: 250, total: 1000 }));
        assert_eq!(pull_line("{\"status\":\"success\"}"), Some(PullLine::Done));
        assert_eq!(pull_line("{\"error\":\"pull model manifest: file does not exist\"}"), Some(PullLine::Error("pull model manifest: file does not exist".into())));
        assert_eq!(pull_line("garbage"), None);
        // Lemonade's server-sent events
        assert_eq!(pull_line("event: progress"), None);
        assert_eq!(pull_line("data: {\"file\":\"sd_xl.safetensors\",\"file_index\":1,\"total_files\":2,\"bytes_downloaded\":10,\"bytes_total\":40,\"percent\":25}"), Some(PullLine::Progress { status: "downloading sd_xl.safetensors (file 1 of 2)".into(), completed: 10, total: 40 }));
        assert_eq!(pull_line("data: {\"file_index\":2,\"total_files\":2,\"percent\":100}"), Some(PullLine::Done));
        assert_eq!(pull_line("data: {\"error\":\"disk full\"}"), Some(PullLine::Error("disk full".into())));
        assert_eq!(pull_line("{\"status\":\"error\",\"message\":\"no such model\"}"), Some(PullLine::Error("no such model".into())));
        assert!(explain_pull_error("qwen-image-2.1", "pull model manifest: file does not exist").contains("was not found in the Ollama library"));
        assert!(explain_pull_error("m", "no space left on device").contains("no room"));
        assert!(valid_pull_name("x/flux2-klein:latest") && !valid_pull_name("a b") && !valid_pull_name("../x") && !valid_pull_name("-rf"));
    }
}
